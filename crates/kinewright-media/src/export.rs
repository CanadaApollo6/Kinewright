use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange,
    ColorTransfer, ColorWhitePoint, Document, ExportProgress, ExportSettings, FrameRounding,
    MediaError, ProgressSink, TimeCode, TrackId, map_frames_with_rounding,
};

use crate::{
    audio::{AudioMixProcessor, ClipAudioShaping, decode_audio_range, limit_audio_mix},
    clock::frame_to_samples,
    compositor::GpuContext,
    decode::backend,
    render::FrameRenderer,
    timeline::timeline_audio_segments,
};

const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;

pub(crate) fn export_document(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
) -> Result<(), MediaError> {
    validate_settings(document, out, settings)?;
    let temporary = temporary_output(out);
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(backend)?;
    }
    let result = export_to_temporary(document, &temporary, settings, progress, gpu);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    replace_output(&temporary, out)
}

// Encoder and muxer setup must stay in one ownership scope through trailer finalization.
#[allow(clippy::too_many_lines)]
fn export_to_temporary(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
) -> Result<(), MediaError> {
    check_cancelled(settings)?;
    let total_frames = map_frames_with_rounding(
        document.duration,
        document.fps,
        settings.fps,
        FrameRounding::Ceil,
    )
    .map_err(|error| MediaError::Backend(error.to_string()))?;
    let total_frames = u64::try_from(total_frames.0)
        .map_err(|_| MediaError::Backend("export frame count is invalid".to_owned()))?;
    send_progress(progress, 0, total_frames);

    let audio_mix = mix_audio(document, settings)?;
    check_cancelled(settings)?;

    let mut muxer = ffmpeg::format::output(out).map_err(backend)?;
    let global_header = muxer
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);
    let video_codec = find_codec(&settings.video_codec, ffmpeg::codec::Id::H264)?;
    let audio_codec = find_codec(&settings.audio_codec, ffmpeg::codec::Id::AAC)?;
    let video_time_base = ffmpeg::Rational(
        i32::try_from(settings.fps.denominator())
            .map_err(|_| MediaError::Backend("export fps denominator is too large".to_owned()))?,
        i32::try_from(settings.fps.numerator())
            .map_err(|_| MediaError::Backend("export fps numerator is too large".to_owned()))?,
    );
    let video_frame_rate = ffmpeg::Rational(
        i32::try_from(settings.fps.numerator())
            .map_err(|_| MediaError::Backend("export fps numerator is too large".to_owned()))?,
        i32::try_from(settings.fps.denominator())
            .map_err(|_| MediaError::Backend("export fps denominator is too large".to_owned()))?,
    );

    let mut video_encoder = ffmpeg::codec::context::Context::new_with_codec(video_codec)
        .encoder()
        .video()
        .map_err(backend)?;
    video_encoder.set_width(settings.resolution.0);
    video_encoder.set_height(settings.resolution.1);
    video_encoder.set_format(ffmpeg::format::Pixel::YUV420P);
    video_encoder.set_time_base(video_time_base);
    video_encoder.set_frame_rate(Some(video_frame_rate));
    video_encoder.set_bit_rate(
        usize::try_from(settings.video_bitrate)
            .map_err(|_| MediaError::Backend("video bitrate is too large".to_owned()))?,
    );
    video_encoder.set_gop(settings.fps.numerator().saturating_mul(2));
    if global_header {
        video_encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }
    // The current exporter is an explicit Rec.709 SDR metadata path. The
    // validation above rejects other delivery descriptions before any output
    // is created; this assignment therefore cannot silently mislabel another
    // target. Pixel transforms remain a CC1 concern.
    video_encoder.set_colorspace(ffmpeg::color::Space::BT709);
    video_encoder.set_color_range(ffmpeg::color::Range::MPEG);
    let mut video_options = ffmpeg::Dictionary::new();
    if settings.video_codec == "libx264" {
        video_options.set("preset", "medium");
        // FFmpeg's generic codec-context colour fields do not reliably carry
        // primaries and transfer through libx264's SPS. These x264 options
        // are required for the tags to survive a post-export re-probe.
        video_options.set(
            "x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
        );
    }
    let mut video_encoder = video_encoder
        .open_as_with(video_codec, video_options)
        .map_err(backend)?;
    let video_stream_index = {
        let mut stream = muxer.add_stream(video_codec).map_err(backend)?;
        stream.set_time_base(video_time_base);
        stream.set_rate(video_frame_rate);
        stream.set_parameters(&video_encoder);
        stream.index()
    };

    let audio_layout = ffmpeg::ChannelLayout::STEREO;
    let audio_format = audio_codec
        .audio()
        .map_err(backend)?
        .formats()
        .and_then(|formats| {
            formats.into_iter().find(|format| {
                *format == ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar)
            })
        })
        .ok_or_else(|| MediaError::Backend("AAC encoder does not support planar f32".to_owned()))?;
    let audio_time_base = ffmpeg::Rational(1, i32::try_from(AUDIO_RATE).unwrap_or(48_000));
    let mut audio_encoder = ffmpeg::codec::context::Context::new_with_codec(audio_codec)
        .encoder()
        .audio()
        .map_err(backend)?;
    audio_encoder.set_rate(i32::try_from(AUDIO_RATE).unwrap_or(48_000));
    audio_encoder.set_channel_layout(audio_layout);
    audio_encoder.set_format(audio_format);
    audio_encoder.set_time_base(audio_time_base);
    audio_encoder.set_bit_rate(
        usize::try_from(settings.audio_bitrate)
            .map_err(|_| MediaError::Backend("audio bitrate is too large".to_owned()))?,
    );
    if global_header {
        audio_encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }
    let mut audio_encoder = audio_encoder.open_as(audio_codec).map_err(backend)?;
    let audio_stream_index = {
        let mut stream = muxer.add_stream(audio_codec).map_err(backend)?;
        stream.set_time_base(audio_time_base);
        stream.set_parameters(&audio_encoder);
        stream.index()
    };

    muxer.write_header().map_err(backend)?;
    let video_output_time_base = muxer
        .stream(video_stream_index)
        .ok_or_else(|| MediaError::Backend("video output stream disappeared".to_owned()))?
        .time_base();
    let audio_output_time_base = muxer
        .stream(audio_stream_index)
        .ok_or_else(|| MediaError::Backend("audio output stream disappeared".to_owned()))?
        .time_base();

    let mut renderer = FrameRenderer::new(gpu);
    let mut scaler = ffmpeg::software::scaling::Context::get(
        ffmpeg::format::Pixel::RGBA,
        settings.resolution.0,
        settings.resolution.1,
        ffmpeg::format::Pixel::YUV420P,
        settings.resolution.0,
        settings.resolution.1,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )
    .map_err(backend)?;
    for output_frame in 0..total_frames {
        check_cancelled(settings)?;
        let output_at = TimeCode(i64::try_from(output_frame).unwrap_or(i64::MAX));
        let project_at =
            map_frames_with_rounding(output_at, settings.fps, document.fps, FrameRounding::Floor)
                .map_err(|error| MediaError::Backend(error.to_string()))?;
        let project_at = TimeCode(project_at.0.min(document.duration.0.saturating_sub(1)));
        let composed = renderer.render(
            document,
            project_at,
            settings.resolution,
            crate::render::RenderScale::FullResolution,
            crate::render::DecodeStrategy::Sequential,
        )?;
        let mut rgba = ffmpeg::frame::Video::new(
            ffmpeg::format::Pixel::RGBA,
            settings.resolution.0,
            settings.resolution.1,
        );
        stamp_rgba_color(&mut rgba);
        copy_rgba_to_frame(&composed.rgba, &mut rgba)?;
        let mut yuv = ffmpeg::frame::Video::new(
            ffmpeg::format::Pixel::YUV420P,
            settings.resolution.0,
            settings.resolution.1,
        );
        scaler.run(&rgba, &mut yuv).map_err(backend)?;
        stamp_yuv420p_color(&mut yuv);
        yuv.set_pts(Some(i64::try_from(output_frame).unwrap_or(i64::MAX)));
        video_encoder.send_frame(&yuv).map_err(backend)?;
        drain_packets(
            &mut video_encoder,
            &mut muxer,
            video_stream_index,
            video_time_base,
            video_output_time_base,
        )?;
        send_progress(progress, output_frame.saturating_add(1), total_frames);
    }
    video_encoder.send_eof().map_err(backend)?;
    drain_packets(
        &mut video_encoder,
        &mut muxer,
        video_stream_index,
        video_time_base,
        video_output_time_base,
    )?;

    encode_audio(
        &audio_mix,
        settings,
        &mut audio_encoder,
        &mut muxer,
        audio_stream_index,
        audio_time_base,
        audio_output_time_base,
    )?;
    muxer.write_trailer().map_err(backend)?;
    send_progress(progress, total_frames, total_frames);
    Ok(())
}

fn find_codec(name: &str, expected_id: ffmpeg::codec::Id) -> Result<ffmpeg::Codec, MediaError> {
    let codec = ffmpeg::encoder::find_by_name(name)
        .ok_or_else(|| MediaError::Backend(format!("encoder {name:?} is not available")))?;
    if codec.id() != expected_id {
        return Err(MediaError::Backend(format!(
            "encoder {name:?} is not the required {expected_id:?} codec"
        )));
    }
    Ok(codec)
}

fn copy_rgba_to_frame(rgba: &[u8], frame: &mut ffmpeg::frame::Video) -> Result<(), MediaError> {
    let row_bytes = usize::try_from(frame.width())
        .unwrap_or_default()
        .saturating_mul(4);
    let height = usize::try_from(frame.height()).unwrap_or_default();
    if rgba.len() != row_bytes.saturating_mul(height) {
        return Err(MediaError::Backend(
            "compositor readback size is invalid".to_owned(),
        ));
    }
    let stride = frame.stride(0);
    let plane = frame.data_mut(0);
    for row in 0..height {
        let source_start = row.saturating_mul(row_bytes);
        let target_start = row.saturating_mul(stride);
        plane[target_start..target_start + row_bytes]
            .copy_from_slice(&rgba[source_start..source_start + row_bytes]);
    }
    Ok(())
}

/// Stamp the full-range RGB intermediate produced by the compositor. RGB has
/// identity matrix coefficients and full-range samples; its primaries and
/// transfer still describe the explicit Rec.709 SDR working/display contract.
fn stamp_rgba_color(frame: &mut ffmpeg::frame::Video) {
    frame.set_color_space(ffmpeg::color::Space::RGB);
    frame.set_color_range(ffmpeg::color::Range::JPEG);
    frame.set_color_primaries(ffmpeg::color::Primaries::BT709);
    frame.set_color_transfer_characteristic(ffmpeg::color::TransferCharacteristic::BT709);
}

/// Stamp the limited-range YUV420P delivery frame with the exact metadata
/// emitted by the current H.264 path.
fn stamp_yuv420p_color(frame: &mut ffmpeg::frame::Video) {
    frame.set_color_space(ffmpeg::color::Space::BT709);
    frame.set_color_range(ffmpeg::color::Range::MPEG);
    frame.set_color_primaries(ffmpeg::color::Primaries::BT709);
    frame.set_color_transfer_characteristic(ffmpeg::color::TransferCharacteristic::BT709);
}

fn drain_packets(
    encoder: &mut ffmpeg::encoder::Video,
    muxer: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    encoder_time_base: ffmpeg::Rational,
    output_time_base: ffmpeg::Rational,
) -> Result<(), MediaError> {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(stream_index);
        packet.rescale_ts(encoder_time_base, output_time_base);
        packet.write_interleaved(muxer).map_err(backend)?;
    }
    Ok(())
}

fn encode_audio(
    mix: &[f32],
    settings: &ExportSettings,
    encoder: &mut ffmpeg::encoder::Audio,
    muxer: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    encoder_time_base: ffmpeg::Rational,
    output_time_base: ffmpeg::Rational,
) -> Result<(), MediaError> {
    let channels = usize::from(AUDIO_CHANNELS);
    let total_sample_frames = mix.len() / channels;
    let frame_size = usize::try_from(encoder.frame_size().max(1)).unwrap_or(1);
    let format = encoder.format();
    let layout = encoder.channel_layout();
    let mut start = 0_usize;
    while start < total_sample_frames {
        check_cancelled(settings)?;
        let sample_frames = frame_size.min(total_sample_frames - start);
        let mut frame = ffmpeg::frame::Audio::new(format, sample_frames, layout);
        frame.set_rate(AUDIO_RATE);
        frame.set_pts(Some(i64::try_from(start).unwrap_or(i64::MAX)));
        for channel in 0..channels {
            let plane = frame.plane_mut::<f32>(channel);
            for (offset, destination) in plane.iter_mut().enumerate().take(sample_frames) {
                *destination = mix[(start + offset) * channels + channel];
            }
        }
        encoder.send_frame(&frame).map_err(backend)?;
        drain_audio_packets(
            encoder,
            muxer,
            stream_index,
            encoder_time_base,
            output_time_base,
        )?;
        start += sample_frames;
    }
    encoder.send_eof().map_err(backend)?;
    drain_audio_packets(
        encoder,
        muxer,
        stream_index,
        encoder_time_base,
        output_time_base,
    )
}

fn drain_audio_packets(
    encoder: &mut ffmpeg::encoder::Audio,
    muxer: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    encoder_time_base: ffmpeg::Rational,
    output_time_base: ffmpeg::Rational,
) -> Result<(), MediaError> {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(stream_index);
        packet.rescale_ts(encoder_time_base, output_time_base);
        packet.write_interleaved(muxer).map_err(backend)?;
    }
    Ok(())
}

pub(crate) fn mix_audio(
    document: &Document,
    settings: &ExportSettings,
) -> Result<Vec<f32>, MediaError> {
    let total_sample_frames = frame_to_samples(document.duration, AUDIO_RATE, document.fps);
    let total_samples = usize::try_from(total_sample_frames)
        .map_err(|_| MediaError::Backend("audio mix is too large".to_owned()))?
        .checked_mul(usize::from(AUDIO_CHANNELS))
        .ok_or_else(|| MediaError::Backend("audio mix is too large".to_owned()))?;
    let mut track_mixes = HashMap::<TrackId, Vec<f32>>::new();
    let segments = timeline_audio_segments(document, TimeCode::ZERO..document.duration)?;
    for segment in segments {
        check_cancelled(settings)?;
        let clip = document.clip(segment.clip).ok_or_else(|| {
            MediaError::Backend(format!("timeline clip {} disappeared", segment.clip))
        })?;
        let asset = document.asset(segment.asset).ok_or_else(|| {
            MediaError::Backend(format!("timeline asset {} disappeared", segment.asset))
        })?;
        let start_frame = frame_to_samples(segment.project.start, AUDIO_RATE, document.fps);
        let end_frame = frame_to_samples(segment.project.end, AUDIO_RATE, document.fps);
        let wanted_frames = usize::try_from(end_frame.saturating_sub(start_frame))
            .map_err(|_| MediaError::Backend("audio clip is too large".to_owned()))?;
        let wanted_samples = wanted_frames
            .checked_mul(usize::from(AUDIO_CHANNELS))
            .ok_or_else(|| MediaError::Backend("audio clip is too large".to_owned()))?;
        let mut decoded = decode_audio_range(
            &asset.path,
            asset.fps,
            segment.source.start,
            segment.source.end,
            AUDIO_RATE,
            AUDIO_CHANNELS,
            &settings.cancellation,
        )?;
        decoded.resize(wanted_samples, 0.0);
        decoded.truncate(wanted_samples);
        let start = usize::try_from(start_frame)
            .map_err(|_| MediaError::Backend("audio clip start is too large".to_owned()))?
            .checked_mul(usize::from(AUDIO_CHANNELS))
            .ok_or_else(|| MediaError::Backend("audio clip start is too large".to_owned()))?;
        let clip_duration = document
            .clip_duration(clip)
            .map_err(|error| MediaError::Backend(error.to_string()))?;
        let shaping = ClipAudioShaping::new(clip, clip_duration, AUDIO_RATE, document.fps);
        let channel_count = usize::from(AUDIO_CHANNELS);
        let track_mix = track_mixes
            .entry(segment.track)
            .or_insert_with(|| vec![0.0; total_samples]);
        for (sample_index, (destination, sample)) in
            track_mix.iter_mut().skip(start).zip(decoded).enumerate()
        {
            let frame_offset = u64::try_from(sample_index / channel_count).unwrap_or(u64::MAX);
            let project_sample = start_frame.saturating_add(frame_offset);
            let gain = shaping.gain_at(project_sample);
            *destination += sample * gain;
        }
    }
    let channel_count = usize::from(AUDIO_CHANNELS);
    let mut processor = AudioMixProcessor::new(document, AUDIO_RATE, channel_count);
    let mut mix = Vec::with_capacity(total_samples);
    let mut start_frame = 0_u64;
    while start_frame < total_sample_frames {
        check_cancelled(settings)?;
        let frame_count = usize::try_from(total_sample_frames - start_frame)
            .unwrap_or(usize::MAX)
            .min(1_024);
        let start = usize::try_from(start_frame)
            .unwrap_or(usize::MAX)
            .saturating_mul(channel_count);
        let sample_count = frame_count.saturating_mul(channel_count);
        let chunk_tracks = track_mixes
            .iter()
            .map(|(track, samples)| {
                let end = start.saturating_add(sample_count).min(samples.len());
                (*track, samples[start..end].to_vec())
            })
            .collect::<HashMap<_, _>>();
        mix.extend(processor.mix_chunk(&chunk_tracks, start_frame, frame_count)?);
        start_frame = start_frame.saturating_add(u64::try_from(frame_count).unwrap_or(u64::MAX));
    }
    limit_audio_mix(&mut mix);
    Ok(mix)
}

fn validate_settings(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
) -> Result<(), MediaError> {
    validate_delivery_color(settings)?;
    if document.duration <= TimeCode::ZERO {
        return Err(MediaError::Backend(
            "cannot export an empty timeline".to_owned(),
        ));
    }
    if !settings.fps.is_valid() {
        return Err(MediaError::Backend(
            "export frame rate is invalid".to_owned(),
        ));
    }
    if settings.resolution.0 == 0
        || settings.resolution.1 == 0
        || !settings.resolution.0.is_multiple_of(2)
        || !settings.resolution.1.is_multiple_of(2)
    {
        return Err(MediaError::Backend(
            "H.264 export resolution must be non-zero and even".to_owned(),
        ));
    }
    if out.file_name().is_none() {
        return Err(MediaError::Backend(
            "export output must include a file name".to_owned(),
        ));
    }
    if out
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty() && !parent.exists())
    {
        return Err(MediaError::Backend(
            "export directory does not exist".to_owned(),
        ));
    }
    Ok(())
}

/// The current encoder/scaler path supports one explicit delivery contract:
/// 8-bit SDR Rec.709 in limited-range YUV420P. Keep this gate in front of
/// encoder setup so an unknown or future colour description cannot be
/// mislabeled with today's tags.
fn validate_delivery_color(settings: &ExportSettings) -> Result<(), MediaError> {
    if settings.video_codec != "libx264" {
        return Err(MediaError::Backend(
            "explicit delivery colour tagging currently requires the libx264 H.264 encoder"
                .to_owned(),
        ));
    }
    validate_delivery_description(&settings.delivery_color)
}

fn validate_delivery_description(color: &ColorDescription) -> Result<(), MediaError> {
    if !color.confidence_is_valid()
        || color.confidence_basis_points == 0
        || !matches!(
            &color.provenance,
            ColorProvenance::ApplicationDefault | ColorProvenance::UserOverride
        )
        || !matches!(&color.primaries, ColorPrimaries::Bt709)
        || !matches!(&color.transfer, ColorTransfer::Bt709)
        || !matches!(&color.matrix, ColorMatrix::Bt709)
        || !matches!(&color.range, ColorRange::Limited)
        || !matches!(&color.white_point, ColorWhitePoint::D65)
        || !matches!(&color.bit_depth, ColorBitDepth::Eight)
    {
        return Err(MediaError::Backend(
            "unsupported delivery colour: the current H.264/YUV420P path requires explicit 8-bit SDR Rec.709 (BT.709 primaries, transfer, and matrix; limited range; D65; nonzero confidence; application_default or user_override provenance)".to_owned(),
        ));
    }
    Ok(())
}

fn check_cancelled(settings: &ExportSettings) -> Result<(), MediaError> {
    if settings.cancellation.is_cancelled() {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

fn send_progress(progress: &ProgressSink, completed_frames: u64, total_frames: u64) {
    let _ = progress.send(ExportProgress {
        completed_frames,
        total_frames,
    });
}

fn temporary_output(out: &Path) -> PathBuf {
    let stem = out
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("kinewright-export");
    let name = format!(".{stem}.kinewright-part-{}.mp4", std::process::id());
    out.with_file_name(name)
}

fn replace_output(temporary: &Path, out: &Path) -> Result<(), MediaError> {
    if !out.exists() {
        return fs::rename(temporary, out).map_err(backend);
    }
    let backup = out.with_file_name(format!(
        ".{}.kinewright-backup-{}",
        out.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("export.mp4"),
        std::process::id()
    ));
    fs::rename(out, &backup).map_err(backend)?;
    if let Err(error) = fs::rename(temporary, out) {
        let _ = fs::rename(&backup, out);
        return Err(backend(error));
    }
    fs::remove_file(backup).map_err(backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{ColorContext, DeliveryProfile, ExportCancellation};

    #[test]
    fn accepts_the_current_sdr_rec709_delivery_contract() {
        let color = ColorContext::sdr_rec709().delivery;
        assert!(validate_delivery_description(&color).is_ok());
    }

    #[test]
    fn rejects_unknown_delivery_metadata() {
        let error = validate_delivery_description(&ColorDescription::default())
            .expect_err("unknown delivery metadata must not be tagged as Rec.709");
        assert!(error.to_string().contains("unsupported delivery colour"));
    }

    #[test]
    fn rejects_zero_confidence_delivery_metadata() {
        let mut color = ColorContext::sdr_rec709().delivery;
        color.confidence_basis_points = 0;
        let error = validate_delivery_description(&color)
            .expect_err("zero-confidence delivery metadata must not be tagged as Rec.709");
        assert!(error.to_string().contains("nonzero confidence"));
    }

    #[test]
    fn accepts_only_supported_project_delivery_provenance() {
        for provenance in [
            ColorProvenance::ApplicationDefault,
            ColorProvenance::UserOverride,
        ] {
            let mut color = ColorContext::sdr_rec709().delivery;
            color.provenance = provenance;
            assert!(
                validate_delivery_description(&color).is_ok(),
                "current project delivery provenance should be accepted: {:?}",
                color.provenance
            );
        }
    }

    #[test]
    fn rejects_unknown_or_non_project_delivery_provenance() {
        for provenance in [
            ColorProvenance::Unknown,
            ColorProvenance::Other("future_provenance".to_owned()),
            ColorProvenance::StreamMetadata,
            ColorProvenance::ContainerMetadata,
            ColorProvenance::SidecarMetadata,
            ColorProvenance::Inferred,
        ] {
            let mut color = ColorContext::sdr_rec709().delivery;
            color.provenance = provenance;
            let error = validate_delivery_description(&color)
                .expect_err("unsupported provenance must not be tagged as Rec.709");
            assert!(
                error
                    .to_string()
                    .contains("application_default or user_override provenance"),
                "unexpected delivery provenance error: {error}"
            );
        }
    }

    #[test]
    fn rejects_non_rec709_delivery_metadata() {
        let mut color = ColorContext::sdr_rec709().delivery;
        color.primaries = ColorPrimaries::Bt2020;
        let error = validate_delivery_description(&color)
            .expect_err("non-Rec.709 delivery metadata is not supported yet");
        assert!(
            error
                .to_string()
                .contains("requires explicit 8-bit SDR Rec.709")
        );
    }

    #[test]
    fn rejects_non_libx264_for_explicit_color_tagging() {
        let mut settings = DeliveryProfile::SourceMaster
            .export_settings(&Document::default(), ExportCancellation::default());
        settings.video_codec = "h264_nvenc".to_owned();
        let error = validate_delivery_color(&settings)
            .expect_err("unmapped H.264 encoders must not claim tagged output");
        assert!(
            error
                .to_string()
                .contains("requires the libx264 H.264 encoder")
        );
    }

    #[test]
    fn stamps_rgb_and_yuv_frames_with_their_explicit_ranges() {
        let mut rgba = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGBA, 2, 2);
        stamp_rgba_color(&mut rgba);
        assert_eq!(rgba.color_space(), ffmpeg::color::Space::RGB);
        assert_eq!(rgba.color_range(), ffmpeg::color::Range::JPEG);
        assert_eq!(rgba.color_primaries(), ffmpeg::color::Primaries::BT709);
        assert_eq!(
            rgba.color_transfer_characteristic(),
            ffmpeg::color::TransferCharacteristic::BT709
        );

        let mut yuv = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, 2, 2);
        stamp_yuv420p_color(&mut yuv);
        assert_eq!(yuv.color_space(), ffmpeg::color::Space::BT709);
        assert_eq!(yuv.color_range(), ffmpeg::color::Range::MPEG);
        assert_eq!(yuv.color_primaries(), ffmpeg::color::Primaries::BT709);
        assert_eq!(
            yuv.color_transfer_characteristic(),
            ffmpeg::color::TransferCharacteristic::BT709
        );
    }
}
