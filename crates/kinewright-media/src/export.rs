use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    AUDIO_QC_CLIPPED_RUN_SAMPLES, AUDIO_QC_SILENCE_DBFS_HUNDREDTHS,
    AUDIO_QC_SILENCE_WINDOW_MILLISECONDS, AudioBusId, AudioChannelClipping, AudioClipping,
    AudioQcMeasurements, AudioQcProvenance, AudioQcReport, AudioQcRequest, BusLevels,
    ColorDescription, DELIVERY_BIT_DEPTH_ALLOWED, DeliveryColorError, DeliveryColorMismatch,
    DeliveryEncodeDepth, DeliveryProfile, Document, Effect, EffectId, ExportAudioReport,
    ExportCancellation, ExportProgress, ExportReport, ExportSettings, FrameRounding,
    LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS, LoudnessTarget, MediaError, MixLevelReport,
    MixLevelRequest, MixSpectrumPoint, MixSpectrumReport, MixSpectrumRequest, ParamValue,
    ProgressSink, TimeCode, TrackId, TrackLevels, audio_qc_exceptions, audio_qc_technical_pass,
    delivery_color_mismatches, map_frames_with_rounding,
};

use crate::{
    audio::{
        AudioMixProcessor, ClipAudioShaping, decode_audio_range, graph_latency_frames,
        limit_audio_mix, process_buffer_static,
    },
    clock::{frame_to_samples, samples_to_frame},
    compositor::GpuContext,
    decode::backend,
    loudness::{LOUDNESS_GATING_BLOCK_FRAMES, LoudnessMeter},
    lut_store::LutLibrary,
    render::FrameRenderer,
    spectrum::{SPECTRUM_MINIMUM_FRAMES, third_octave_spectrum},
    timeline::timeline_audio_segments,
};

const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;

/// Export with no LUT library.
///
/// Test-only: the production `Export` impl always publishes the engine's
/// library, so this arity exists for the CC1 fixtures and the media matrix,
/// whose documents predate LUT nodes.
#[cfg(test)]
pub(crate) fn export_document(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
) -> Result<(), MediaError> {
    export_document_with_luts(
        document,
        out,
        settings,
        progress,
        gpu,
        Arc::new(LutLibrary::default()),
    )
    .map(|_| ())
}

/// Export with the verified CC4 LUT library (CC4 2.4).
///
/// `library` must have been bound to **this** `document`'s asset hashes, which
/// is what the engine's `Export` impl does immediately before calling here: an
/// export queue outlives focus, and `LutAssetId`s restart at 1 in every
/// project, so a library carried over from whichever project published last
/// would deliver another project's look.
///
/// The export path is the same production renderer as preview, so a clip
/// carrying an active `technical_lut` / `creative_look` node whose asset is not
/// in the library fails with `missing_lut_asset` and produces no file, rather
/// than delivering a frame without the look.
pub(crate) fn export_document_with_luts(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
    library: Arc<LutLibrary>,
) -> Result<ExportReport, MediaError> {
    export_document_inner(
        document,
        out,
        settings,
        progress,
        gpu,
        library,
        VideoPacketDuration::OneFrame,
    )
}

/// Re-run the production export with the pre-CC6 packet timing (test-only).
///
/// The only difference from [`export_document_with_luts`] is
/// [`VideoPacketDuration::Zero`]: the written file is the *defect* -- an MP4
/// whose `elst` presents one frame fewer than the track codes -- so the
/// verification refusal that catches it can be asserted on a real file rather
/// than on a hand-edited container.
#[cfg(test)]
pub(crate) fn export_document_with_zero_packet_durations(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
) -> Result<(), MediaError> {
    export_document_inner(
        document,
        out,
        settings,
        progress,
        gpu,
        Arc::new(LutLibrary::default()),
        VideoPacketDuration::Zero,
    )
    .map(|_| ())
}

fn export_document_inner(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
    library: Arc<LutLibrary>,
    packet_duration: VideoPacketDuration,
) -> Result<ExportReport, MediaError> {
    validate_settings(document, out, settings)?;
    let temporary = temporary_output(out);
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(backend)?;
    }
    let result = export_to_temporary(
        document,
        &temporary,
        settings,
        progress,
        gpu,
        library,
        packet_duration,
    );
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    replace_output(&temporary, out)?;
    Ok(report)
}

/// AU3 §5.6 step 1 / N4 Q3: the skip reason for a programme with no complete
/// gating block.
///
/// Deliberately distinct from [`NORMALIZATION_SILENT_REASON`]: a measurement
/// that refuses to gate because the programme is too short is a different
/// fact from a programme that is digitally silent, and the operator is told
/// which one happened.
pub(crate) const NORMALIZATION_SHORT_PROGRAMME_REASON: &str =
    "shorter than one 400 ms gating block";

/// AU3 §5.6 step 2: the skip reason for a master with no gated loudness at all.
pub(crate) const NORMALIZATION_SILENT_REASON: &str = "silent";

/// AU3 §5.6 step 3: the normalization gain range, the planner's guard
/// (`server.rs` `plan_audio_normalization`) spelled once more on the export
/// side so a job that asks for an impossible move is skipped rather than
/// attempted.
const NORMALIZATION_MINIMUM_GAIN_HUNDREDTHS: i32 = -6_000;
const NORMALIZATION_MAXIMUM_GAIN_HUNDREDTHS: i32 = 3_600;

/// The `ceiling_tenth_db` descriptor's lower bound (`effect.rs`), so a target
/// with an implausible ceiling cannot hand the node an out-of-range parameter.
const TRUE_PEAK_CEILING_MINIMUM_TENTH_DB: i64 = -120;

/// AU3 §5.6: bring the finished master to `target` before it is encoded.
///
/// The step runs between `mix_audio` and `encode_audio` (§5.5) and is entered
/// only when `settings.loudness_normalization` is `Some`, so an export that
/// does not ask for it encodes `mix_audio`'s bytes exactly.
///
/// A skip is not a failure: the three skip paths (no complete gating block, a
/// silent master, a gain outside the guard) each return a report naming the
/// reason with `before == after`, and the export continues with the mix
/// untouched. A miss after two passes is not a failure either (§5.6 F11): the
/// report says `on_target: false` and the delivery verification warns.
///
/// # Errors
///
/// Returns a media error when the master cannot be measured, when the target's
/// ceiling is outside the limiter node's range, when the node cannot be run
/// over the master, or when the export was cancelled between passes.
fn normalize_master(
    mix: &mut Vec<f32>,
    target: LoudnessTarget,
    settings: &ExportSettings,
) -> Result<ExportAudioReport, MediaError> {
    let before = LoudnessMeter::measure(mix, AUDIO_RATE, AUDIO_CHANNELS)?;
    let skipped = |reason: String| ExportAudioReport {
        target,
        before,
        after: before,
        applied_gain_hundredths: 0,
        limiter_passes: 0,
        peak_reduction_hundredths: 0,
        on_target: false,
        skipped_reason: Some(reason),
    };
    let sample_frames = u64::try_from(mix.len() / usize::from(AUDIO_CHANNELS)).unwrap_or(u64::MAX);
    if sample_frames < LOUDNESS_GATING_BLOCK_FRAMES {
        return Ok(skipped(NORMALIZATION_SHORT_PROGRAMME_REASON.to_owned()));
    }
    let Some(integrated) = before.integrated_lufs_hundredths else {
        return Ok(skipped(NORMALIZATION_SILENT_REASON.to_owned()));
    };
    let gain = target.integrated_lufs_hundredths - integrated;
    if !(NORMALIZATION_MINIMUM_GAIN_HUNDREDTHS..=NORMALIZATION_MAXIMUM_GAIN_HUNDREDTHS)
        .contains(&gain)
    {
        return Ok(skipped(format!(
            "required gain {gain} hundredths exceeds \
             {NORMALIZATION_MINIMUM_GAIN_HUNDREDTHS}..={NORMALIZATION_MAXIMUM_GAIN_HUNDREDTHS}"
        )));
    }

    // The ceiling is validated before the master is touched, so a refused
    // target leaves `mix` exactly as it was.
    let ceiling_tenth_db = delivery_limiter_ceiling_tenth_db(target)?;
    check_cancelled(settings)?;
    apply_gain(mix, gain);
    *mix = limit_to_delivery_ceiling(mix, ceiling_tenth_db)?;
    let mut limiter_passes = 1_u8;
    let mut applied_gain_hundredths = gain;
    let mut after = LoudnessMeter::measure(mix, AUDIO_RATE, AUDIO_CHANNELS)?;

    // AU3 §5.6 step 6: one corrective pass, and only when the programme
    // reads under `target − tolerance`, which for a well-formed non-negative
    // tolerance is the only direction a limiter can produce. Two passes is
    // the ceiling.
    if let Some(measured) = after.integrated_lufs_hundredths
        && measured < target.integrated_lufs_hundredths - target.tolerance_lu_hundredths
    {
        check_cancelled(settings)?;
        let correction = target.integrated_lufs_hundredths - measured;
        apply_gain(mix, correction);
        *mix = limit_to_delivery_ceiling(mix, ceiling_tenth_db)?;
        limiter_passes = 2;
        applied_gain_hundredths += correction;
        after = LoudnessMeter::measure(mix, AUDIO_RATE, AUDIO_CHANNELS)?;
    }

    let on_target = after.integrated_lufs_hundredths.is_some_and(|measured| {
        (measured - target.integrated_lufs_hundredths).abs() <= target.tolerance_lu_hundredths
    });
    // AU3 §5.2: derived from the two meter readings, so it is 0 when the
    // limiter was an identity and positive exactly when it did work.
    let peak_reduction_hundredths = match (
        before.true_peak_dbtp_hundredths,
        after.true_peak_dbtp_hundredths,
    ) {
        (Some(before_peak), Some(after_peak)) => {
            (before_peak + applied_gain_hundredths - after_peak).max(0)
        }
        _ => 0,
    };
    Ok(ExportAudioReport {
        target,
        before,
        after,
        applied_gain_hundredths,
        limiter_passes,
        peak_reduction_hundredths,
        on_target,
        skipped_reason: None,
    })
}

/// AU3 §5.6 step 4: scale every sample by `gain_hundredths` hundredths of a
/// decibel.
// The gain is a decibel ratio, not a count: `f32` is the buffer's own
// precision and the conversion is exactly the one `db_gain` performs per node.
#[allow(clippy::cast_possible_truncation)]
fn apply_gain(mix: &mut [f32], gain_hundredths: i32) {
    let gain = 10f64.powf(f64::from(gain_hundredths) / 2_000.0) as f32;
    for sample in mix {
        *sample *= gain;
    }
}

/// AU3 §5.6 step 5: the limiter ceiling for one target, in tenth-decibels.
///
/// Every §2.3 target has a −1 dBTP ceiling and the headroom constant is 200
/// hundredths, so the node is asked for −30 tenth-dB in production. But
/// `LoudnessTarget` is a public `Deserialize` struct with public fields, so a
/// hand-built one can name a ceiling outside the `ceiling_tenth_db`
/// descriptor's declared `−120..=0` range; that is refused by name rather
/// than quietly normalised into range, which would limit the master to a
/// ceiling nobody asked for.
///
/// # Errors
///
/// Returns a media error when the target's ceiling falls outside the
/// descriptor's `−120..=0` tenth-decibel range.
fn delivery_limiter_ceiling_tenth_db(target: LoudnessTarget) -> Result<i64, MediaError> {
    let ceiling = i64::from(
        target
            .true_peak_ceiling_dbtp_hundredths
            .saturating_sub(LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS)
            .div_euclid(10),
    );
    if !(TRUE_PEAK_CEILING_MINIMUM_TENTH_DB..=0).contains(&ceiling) {
        return Err(MediaError::Backend(format!(
            "delivery limiter ceiling {ceiling} tenth-dB is outside -120..=0"
        )));
    }
    Ok(ceiling)
}

/// AU3 §5.6 step 5: one AU2 true-peak limiter pass at
/// [`delivery_limiter_ceiling_tenth_db`], run over the whole master as a
/// static node.
///
/// # Errors
///
/// Returns a media error when the node cannot be run over the buffer. The
/// ceiling is validated by the caller through
/// [`delivery_limiter_ceiling_tenth_db`] before the master is gained.
fn limit_to_delivery_ceiling(mix: &[f32], ceiling_tenth_db: i64) -> Result<Vec<f32>, MediaError> {
    let limiter = Effect {
        id: EffectId(1),
        name: "audio_true_peak_limiter".to_owned(),
        parameters: BTreeMap::from([
            (
                "ceiling_tenth_db".to_owned(),
                ParamValue::Integer(ceiling_tenth_db),
            ),
            ("lookahead_milliseconds".to_owned(), ParamValue::Integer(5)),
            ("release_milliseconds".to_owned(), ParamValue::Integer(50)),
            ("true_peak".to_owned(), ParamValue::Integer(1)),
        ]),
        keyframes: BTreeMap::new(),
    };
    process_buffer_static(&limiter, AUDIO_RATE, usize::from(AUDIO_CHANNELS), mix)
}

// Encoder and muxer setup must stay in one ownership scope through trailer finalization.
#[allow(clippy::too_many_lines)]
fn export_to_temporary(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
    library: Arc<LutLibrary>,
    packet_duration: VideoPacketDuration,
) -> Result<ExportReport, MediaError> {
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

    // AU3 §5.5, normative: the normalization step sits between `mix_audio`
    // and `encode_audio`, and only the `Some` arm allocates. With `None` the
    // `map` is not entered and `encode_audio` receives `mix_audio`'s bytes
    // exactly (B6).
    let mut audio_mix = mix_audio(document, settings)?;
    check_cancelled(settings)?;
    let audio = settings
        .loudness_normalization
        .map(|target| normalize_master(&mut audio_mix, target, settings))
        .transpose()?;

    let mut muxer = ffmpeg::format::output(out).map_err(backend)?;
    let global_header = muxer
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);
    let video_codec = find_codec(&settings.video_codec, ffmpeg::codec::Id::H264)?;
    let audio_codec = find_codec(&settings.audio_codec, ffmpeg::codec::Id::AAC)?;
    // CC6 4.1/4.3: `settings.delivery_color.bit_depth` is the single authority
    // for the delivery lane. The codec pixel format and the filter graph's
    // `format` node are both derived from it here, so they cannot diverge, and
    // the encoder is asked for the lane's format before it is opened rather
    // than silently negotiating a different one.
    let delivery_depth = delivery_encode_depth(&settings.delivery_color)?;
    let delivery_pixel = checked_delivery_pixel_format(
        video_codec,
        delivery_depth,
        delivery_lane_pixel_format(delivery_depth),
    )?;
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
    video_encoder.set_format(delivery_pixel);
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
    if settings.video_codec == DELIVERY_VIDEO_CODEC {
        video_options.set("preset", "medium");
        // FFmpeg's generic codec-context colour fields do not reliably carry
        // primaries and transfer through libx264's SPS. These x264 options
        // are required for the tags to survive a post-export re-probe.
        //
        // Identical on both delivery lanes (CC6 4.3). `range=tv` is *not* an
        // x264 parameter in x264 core 165 -- it is parsed and discarded -- and
        // `profile=high10` is not set either: the pixel format selects High 10,
        // measured byte-identical with and without it on the pinned build.
        video_options.set("x264-params", DELIVERY_X264_PARAMS);
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
    renderer.set_lut_library(library);
    let mut delivery_filter = delivery_filter_graph(settings.resolution, delivery_depth)?;
    for output_frame in 0..total_frames {
        check_cancelled(settings)?;
        let output_at = TimeCode(i64::try_from(output_frame).unwrap_or(i64::MAX));
        let project_at =
            map_frames_with_rounding(output_at, settings.fps, document.fps, FrameRounding::Floor)
                .map_err(|error| MediaError::Backend(error.to_string()))?;
        let project_at = TimeCode(project_at.0.min(document.duration.0.saturating_sub(1)));
        // CC1 3/5: export selects the delivery transform, not the monitor
        // transform.  The compositor applies the BT.709 OETF in f32 and
        // quantizes once at 16 bits, so the full->limited conversion below
        // operates on 16-bit codes and the only delivery-depth quantization in
        // the whole path is the YUV420P/YUV420P10LE output itself.
        let composed = renderer.render_delivery(
            document,
            project_at,
            settings.resolution,
            crate::render::RenderScale::FullResolution,
            crate::render::DecodeStrategy::Sequential,
        )?;
        let mut rgba = ffmpeg::frame::Video::new(
            DELIVERY_INTERMEDIATE_PIXEL,
            settings.resolution.0,
            settings.resolution.1,
        );
        stamp_rgba_color(&mut rgba);
        copy_rgba64_to_frame(&composed.rgba64le, &mut rgba)?;
        let mut yuv = delivery_filter.run(&rgba)?;
        stamp_delivery_yuv_color(&mut yuv);
        yuv.set_pts(Some(i64::try_from(output_frame).unwrap_or(i64::MAX)));
        video_encoder.send_frame(&yuv).map_err(backend)?;
        drain_packets(
            &mut video_encoder,
            &mut muxer,
            video_stream_index,
            video_time_base,
            video_output_time_base,
            packet_duration,
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
        packet_duration,
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
    Ok(ExportReport { audio })
}

struct DeliveryFilter {
    graph: ffmpeg::filter::Graph,
    /// The lane's pixel format, asserted on every frame the graph produces.
    pixel_format: ffmpeg::format::Pixel,
}

impl DeliveryFilter {
    fn run(&mut self, rgba: &ffmpeg::frame::Video) -> Result<ffmpeg::frame::Video, MediaError> {
        {
            let mut source_context = self.graph.get("source").ok_or_else(|| {
                MediaError::Backend("delivery source filter disappeared".to_owned())
            })?;
            let mut source = source_context.source();
            source.add(rgba).map_err(|error| {
                MediaError::Backend(format!("delivery source submission failed: {error}"))
            })?;
        }
        let mut output = ffmpeg::frame::Video::empty();
        {
            let mut sink_context = self.graph.get("sink").ok_or_else(|| {
                MediaError::Backend("delivery sink filter disappeared".to_owned())
            })?;
            let mut sink = sink_context.sink();
            sink.frame(&mut output).map_err(|error| {
                MediaError::Backend(format!(
                    "explicit BT.709 limited-range delivery conversion failed: {error}"
                ))
            })?;
        }
        // The `format` node is configured from the delivery lane, so a frame
        // in any other format means the graph and the encoder have diverged.
        // Refuse rather than hand the encoder a frame at the wrong depth.
        if output.format() != self.pixel_format {
            return Err(DeliveryColorError::PixelFormatDepthMismatch {
                observed: pixel_format_name(output.format()).to_owned(),
                allowed: pixel_format_name(self.pixel_format).to_owned(),
            }
            .into());
        }
        Ok(output)
    }
}

/// Build the explicit BT.709 full-to-limited delivery conversion for one lane.
///
/// `depth` drives the `format` node exactly as it drives
/// `video_encoder.set_format`, so the graph output and the encoder input are
/// the same pixel format by construction (CC6 4.3).
fn delivery_filter_graph(
    resolution: (u32, u32),
    depth: DeliveryEncodeDepth,
) -> Result<DeliveryFilter, MediaError> {
    let source_filter = ffmpeg::filter::find("buffer")
        .ok_or_else(|| MediaError::Backend("FFmpeg buffer filter is unavailable".to_owned()))?;
    let sink_filter = ffmpeg::filter::find("buffersink")
        .ok_or_else(|| MediaError::Backend("FFmpeg buffersink filter is unavailable".to_owned()))?;
    let scale_filter = ffmpeg::filter::find("scale")
        .ok_or_else(|| MediaError::Backend("FFmpeg scale filter is unavailable".to_owned()))?;
    let format_filter = ffmpeg::filter::find("format")
        .ok_or_else(|| MediaError::Backend("FFmpeg format filter is unavailable".to_owned()))?;
    let mut graph = ffmpeg::filter::Graph::new();
    let args = format!(
        "video_size={}x{}:pix_fmt=rgba64le:time_base=1/1:pixel_aspect=1/1:colorspace=gbr:range=jpeg",
        resolution.0, resolution.1
    );
    let mut source_context = graph
        .add(&source_filter, "source", &args)
        .map_err(|error| {
            MediaError::Backend(format!("could not configure delivery source: {error}"))
        })?;
    let scale_args = format!(
        "w={}:h={}:flags={DELIVERY_SCALER_FLAGS}:in_range=jpeg:out_range=mpeg:out_color_matrix=bt709",
        resolution.0, resolution.1
    );
    let mut scale_context = graph
        .add(&scale_filter, "scale", &scale_args)
        .map_err(|error| {
            MediaError::Backend(format!(
                "could not configure delivery scale (args={scale_args:?}): {error}"
            ))
        })?;
    let pixel_format = delivery_lane_pixel_format(depth);
    let format_args = format!("pix_fmts={}", depth.pixel_format());
    let mut format_context =
        graph
            .add(&format_filter, "format", &format_args)
            .map_err(|error| {
                MediaError::Backend(format!(
                    "could not configure delivery {} format: {error}",
                    depth.pixel_format()
                ))
            })?;
    let mut sink_context = graph.add(&sink_filter, "sink", "").map_err(|error| {
        MediaError::Backend(format!("could not configure delivery sink: {error}"))
    })?;
    source_context.link(0, &mut scale_context, 0);
    scale_context.link(0, &mut format_context, 0);
    format_context.link(0, &mut sink_context, 0);
    graph.validate().map_err(|error| {
        MediaError::Backend(format!(
            "could not configure explicit BT.709 limited-range delivery conversion (scale_args={scale_args:?}): {error}"
        ))
    })?;
    Ok(DeliveryFilter {
        graph,
        pixel_format,
    })
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

/// Copy the compositor's RGBA64LE delivery readback into the filter graph's
/// input frame. Eight bytes per pixel: the delivery values are already
/// BT.709-coded 16-bit intermediate codes, on the
/// [`DELIVERY_INTERMEDIATE_WHITE`](crate::color_pipeline::DELIVERY_INTERMEDIATE_WHITE)
/// scale swscale expects, and must not be requantized or rescaled here.
fn copy_rgba64_to_frame(rgba: &[u8], frame: &mut ffmpeg::frame::Video) -> Result<(), MediaError> {
    copy_packed_rows(rgba, frame, DELIVERY_INTERMEDIATE_BYTES_PER_PIXEL)
}

fn copy_packed_rows(
    rgba: &[u8],
    frame: &mut ffmpeg::frame::Video,
    bytes_per_pixel: usize,
) -> Result<(), MediaError> {
    let row_bytes = usize::try_from(frame.width())
        .unwrap_or_default()
        .saturating_mul(bytes_per_pixel);
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

/// The delivery intermediate handed to `libavfilter`. 16-bit RGBA keeps the
/// compositor's single quantization intact until the delivery lane's
/// YUV420P/YUV420P10LE output.
///
/// Nominal white in this intermediate is
/// [`DELIVERY_INTERMEDIATE_WHITE`](crate::color_pipeline::DELIVERY_INTERMEDIATE_WHITE)
/// = `65_280` (`255 << 8`), which is `libswscale`'s convention for 16-bit RGB
/// input; `65_535` would be read as brighter than nominal white and would encode
/// to limited-range luma 236 instead of legal white 235 at 8 bits, and 943
/// instead of 940 at 10 bits.
const DELIVERY_INTERMEDIATE_PIXEL: ffmpeg::format::Pixel = ffmpeg::format::Pixel::RGBA64LE;
const DELIVERY_INTERMEDIATE_BYTES_PER_PIXEL: usize = 8;

/// The only video encoder that may carry the managed delivery tags.
const DELIVERY_VIDEO_CODEC: &str = "libx264";

/// The x264 parameter string both delivery lanes encode with (CC6 4.3).
///
/// Identical at 8 and 10 bits. `range=tv` is deliberately absent: it is not an
/// x264 parameter in x264 core 165, and `set_color_range(Range::MPEG)` on the
/// codec context is measured to reach the SPS on its own.
const DELIVERY_X264_PARAMS: &str = "colorprim=bt709:transfer=bt709:colormatrix=bt709";

/// The `libswscale` flags the delivery scaler runs with (CC6 5.3).
///
/// Named rather than inline so a change is a decision: measured on the HD
/// chart, bicubic is the best of bicubic/lanczos/spline on this path.
const DELIVERY_SCALER_FLAGS: &str = "bicubic";

/// The `FFmpeg` descriptor name of a pixel format, or `"unknown"`.
fn pixel_format_name(format: ffmpeg::format::Pixel) -> &'static str {
    format
        .descriptor()
        .map_or("unknown", ffmpeg::format::pixel::Descriptor::name)
}

/// The encoder pixel format for one delivery lane.
///
/// Pinned to [`DeliveryEncodeDepth::pixel_format`], which is the wire name for
/// the same fact; `delivery_lane_pixel_format_matches_the_core_lane_names`
/// asserts the two never drift apart.
const fn delivery_lane_pixel_format(depth: DeliveryEncodeDepth) -> ffmpeg::format::Pixel {
    match depth {
        DeliveryEncodeDepth::Eight => ffmpeg::format::Pixel::YUV420P,
        DeliveryEncodeDepth::Ten => ffmpeg::format::Pixel::YUV420P10LE,
    }
}

/// The delivery lane a delivery colour description selects.
///
/// `delivery_color.bit_depth` is the single authority for the delivery encode
/// depth (CC6 4.1), so this is the only place the depth is read.
fn delivery_encode_depth(color: &ColorDescription) -> Result<DeliveryEncodeDepth, MediaError> {
    DeliveryEncodeDepth::ALL
        .into_iter()
        .find(|depth| color.bit_depth == depth.color_bit_depth())
        .ok_or_else(|| {
            DeliveryColorError::UnsupportedField(DeliveryColorMismatch {
                field: "bit_depth".to_owned(),
                observed: format!("{:?}", color.bit_depth),
                allowed: DELIVERY_BIT_DEPTH_ALLOWED.to_owned(),
            })
            .into()
        })
}

/// Confirm this build's encoder can actually write the lane's pixel format.
///
/// Two typed refusals, both taken **before** the encoder is opened:
///
/// - `delivery_pixel_format_depth_mismatch` when the requested format is not
///   the one the declared depth names, which means the depth and the pixel
///   format came from two different sources;
/// - `delivery_encoder_pixel_format_unavailable` when this build's libx264
///   does not advertise the lane's format at all.
///
/// The second is the cross-platform rule: an `FFmpeg` build without
/// `yuv420p10le` fails loudly instead of silently delivering an 8-bit master
/// under a 10-bit request.
fn checked_delivery_pixel_format(
    codec: ffmpeg::Codec,
    depth: DeliveryEncodeDepth,
    requested: ffmpeg::format::Pixel,
) -> Result<ffmpeg::format::Pixel, MediaError> {
    let requested_name = pixel_format_name(requested);
    if requested_name != depth.pixel_format() {
        return Err(DeliveryColorError::PixelFormatDepthMismatch {
            observed: requested_name.to_owned(),
            allowed: depth.pixel_format().to_owned(),
        }
        .into());
    }
    let advertised = codec
        .video()
        .map_err(backend)?
        .formats()
        .map(|formats| formats.map(pixel_format_name).collect::<Vec<_>>())
        .unwrap_or_default();
    if !advertised.contains(&requested_name) {
        return Err(DeliveryColorError::EncoderPixelFormatUnavailable {
            observed: if advertised.is_empty() {
                "no advertised pixel formats".to_owned()
            } else {
                advertised.join(" ")
            },
            allowed: depth.pixel_format().to_owned(),
        }
        .into());
    }
    Ok(requested)
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

/// Stamp the limited-range `Y'CbCr` delivery frame with the exact metadata
/// emitted by the current H.264 path.
///
/// Identical on both delivery lanes: the lane changes the frame's pixel format
/// (`yuv420p` or `yuv420p10le`), never its colour description (CC6 4.3).
fn stamp_delivery_yuv_color(frame: &mut ffmpeg::frame::Video) {
    frame.set_color_space(ffmpeg::color::Space::BT709);
    frame.set_color_range(ffmpeg::color::Range::MPEG);
    frame.set_color_primaries(ffmpeg::color::Primaries::BT709);
    frame.set_color_transfer_characteristic(ffmpeg::color::TransferCharacteristic::BT709);
}

/// How long one coded video picture lasts, as muxed.
///
/// The encoder time base is `1/fps` and every delivery frame advances the
/// presentation timestamp by exactly one tick, so one picture is one tick.
/// libavcodec cannot stamp `AVPacket.duration` for us here: `ffmpeg-next` 8.0
/// exposes no setter for `AVFrame.duration`, and `unsafe_code` is forbidden
/// workspace-wide, so the duration is stamped on the packet instead --
/// `av_packet_rescale_ts` then carries it into the stream time base along with
/// the timestamps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VideoPacketDuration {
    /// Production: one picture is one tick of the encoder time base.
    ///
    /// Without this the mov muxer computes the track duration as the last
    /// packet's `pts + 0`, and -- because libx264's B-frame delay makes the
    /// muxer shift the media timeline and write an `elst` -- the edit list ends
    /// up one frame shorter than the track. `FFmpeg`'s demuxer then flags the
    /// final coded picture `AV_PKT_FLAG_DISCARD` and every player drops the
    /// last frame of the export.
    OneFrame,
    /// Test-only: mux with the zero duration libavcodec leaves on the packet,
    /// which reproduces the defect above on a real file.
    #[cfg(test)]
    Zero,
}

impl VideoPacketDuration {
    /// The duration to stamp, in ticks of the encoder time base.
    const fn ticks(self) -> i64 {
        match self {
            Self::OneFrame => 1,
            #[cfg(test)]
            Self::Zero => 0,
        }
    }
}

fn drain_packets(
    encoder: &mut ffmpeg::encoder::Video,
    muxer: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    encoder_time_base: ffmpeg::Rational,
    output_time_base: ffmpeg::Rational,
    packet_duration: VideoPacketDuration,
) -> Result<(), MediaError> {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(stream_index);
        packet.set_duration(packet_duration.ticks());
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

/// AU2 §3.7: drop one stem family's leading sample frames in place.
fn drop_leading_samples(samples: &mut Vec<f32>, count: usize) {
    let cut = count.min(samples.len());
    if cut > 0 {
        samples.drain(..cut);
    }
}

/// AU1 §6.1: the per-track, per-bus, and master stems of one mix pass.
///
/// `tracks` and `buses` are empty when the caller did not ask for stems.
pub(crate) struct MixStems {
    /// Post-track-stage, one entry per document track in document order.
    pub(crate) tracks: Vec<(TrackId, Vec<f32>)>,
    /// Post-effects, one entry per bus in document order.
    pub(crate) buses: Vec<(AudioBusId, Vec<f32>)>,
    /// Post-limiter.
    pub(crate) master: Vec<f32>,
}

/// AU3 §3.8: a per-chunk consumer of the mix pass.
///
/// The pass calls `track`, `bus`, and `master` with exactly the frames the
/// corresponding stem would have held for the requested range — the AU2 §3.7
/// head drop and tail truncation are applied to the feed by [`FamilyWindow`]
/// before any callback — in order, one 1 024-frame chunk (or the split of one
/// at a range boundary) at a time. `master` sees the summed master **before**
/// the single clamp (F7); a loudness consumer clamps a copy itself.
pub(crate) trait MixObserver {
    /// True when the pass must run `mix_chunk_with_stems`; `mix_audio` pays
    /// nothing for an observer that does not want them.
    fn wants_stems(&self) -> bool {
        false
    }
    /// Post-stage, pre-bus.
    fn track(&mut self, _track: TrackId, _chunk: &[f32]) -> Result<(), MediaError> {
        Ok(())
    }
    /// Post-bus-chain.
    fn bus(&mut self, _bus: AudioBusId, _chunk: &[f32]) -> Result<(), MediaError> {
        Ok(())
    }
    /// The summed master before the clamp.
    fn master(&mut self, _chunk: &[f32]) -> Result<(), MediaError> {
        Ok(())
    }
}

/// AU3 §3.8: the observer that observes nothing.
pub(crate) struct NoObserver;

impl MixObserver for NoObserver {}

/// AU3 §3.8: which whole-length buffers a mix pass keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MixCollect {
    /// Keep every track and bus stem (`measure_mix_spectrum`'s whole-stem path).
    pub(crate) stems: bool,
    /// Keep the post-clamp master (`mix_audio`).
    pub(crate) master: bool,
}

/// AU3 §3.8: AU2 §3.7's head drop and tail truncation as a window over a
/// streamed feed, in sample frames. A chunk straddling a boundary is split;
/// the observer receives exactly the frames the stem would have held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FamilyWindow {
    skip: usize,
    remaining: usize,
}

impl FamilyWindow {
    pub(crate) const fn new(skip: usize, remaining: usize) -> Self {
        Self { skip, remaining }
    }

    /// The part of `chunk` (interleaved over `channels`) inside the window.
    pub(crate) fn take<'a>(&mut self, chunk: &'a [f32], channels: usize) -> &'a [f32] {
        let channels = channels.max(1);
        let frames = chunk.len() / channels;
        let drop = self.skip.min(frames);
        self.skip -= drop;
        let take = (frames - drop).min(self.remaining);
        self.remaining -= take;
        &chunk[drop * channels..(drop + take) * channels]
    }
}

/// The whole-document export mix (AU1 §6.1: the same code path the measured
/// master comes from, with stem collection switched off).
pub(crate) fn mix_audio(
    document: &Document,
    settings: &ExportSettings,
) -> Result<Vec<f32>, MediaError> {
    mix_pass(
        document,
        TimeCode::ZERO..document.duration,
        settings,
        MixCollect {
            stems: false,
            master: true,
        },
        &mut NoObserver,
    )
    .map(|stems| stems.master)
}

/// AU1 §6.1: mix from frame 0 through `range.end` — the seek-preroll rule, so
/// stateful bus effects match export exactly — and keep `[range.start,
/// range.end)` of every stem. AU3 §3.8: the remaining whole-stem path, used by
/// `measure_mix_spectrum`.
pub(crate) fn mix_audio_stems(
    document: &Document,
    range: std::ops::Range<TimeCode>,
    settings: &ExportSettings,
) -> Result<MixStems, MediaError> {
    mix_pass(
        document,
        range,
        settings,
        MixCollect {
            stems: true,
            master: true,
        },
        &mut NoObserver,
    )
}

#[allow(clippy::too_many_lines)]
pub(crate) fn mix_pass(
    document: &Document,
    range: std::ops::Range<TimeCode>,
    settings: &ExportSettings,
    collect: MixCollect,
    observer: &mut dyn MixObserver,
) -> Result<MixStems, MediaError> {
    // AU2 §3.7: the processor holds `latency` sample frames of the mix, so the
    // pass runs that much past the requested end and each stem family drops the
    // leading frames its own tap carries. Zero for every pre-AU2 document.
    let latency = graph_latency_frames(&document.audio_mix.lookahead_milliseconds(), AUDIO_RATE);
    let total_sample_frames = frame_to_samples(range.end, AUDIO_RATE, document.fps)
        .saturating_add(u64::try_from(latency).unwrap_or(0));
    let total_samples = usize::try_from(total_sample_frames)
        .map_err(|_| MediaError::Backend("audio mix is too large".to_owned()))?
        .checked_mul(usize::from(AUDIO_CHANNELS))
        .ok_or_else(|| MediaError::Backend("audio mix is too large".to_owned()))?;
    let mut track_mixes = HashMap::<TrackId, Vec<f32>>::new();
    // AU2 §3.7: the extra `latency` input frames must carry real programme
    // audio, not silence, or a sub-range measurement's last `L` sample frames
    // are gained as if the programme stopped at `range.end`. Enumerate exactly
    // the project frames that contain the mixed sample frames: the last one
    // consumed is `total_sample_frames - 1`, so the smallest sufficient
    // exclusive end is that frame plus one. When `L == 0` this collapses to
    // `range.end` — at 48 kHz and 10 fps, `samples_to_frame(47_999) + 1 == 10`
    // — so no pre-AU2 measurement decodes a frame it did not decode before,
    // and for `mix_audio` the clamp holds it at the duration. The head and
    // tail trim below discards the surplus.
    let segment_end = if total_sample_frames == 0 {
        range.end
    } else {
        TimeCode(
            samples_to_frame(total_sample_frames - 1, AUDIO_RATE, document.fps)
                .0
                .saturating_add(1)
                .clamp(range.end.0, document.duration.0.max(range.end.0)),
        )
    };
    let segments = timeline_audio_segments(document, TimeCode::ZERO..segment_end)?;
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
    let mut processor = AudioMixProcessor::new(document, AUDIO_RATE, channel_count, None);
    let track_order = document
        .tracks
        .iter()
        .map(|track| track.id)
        .collect::<Vec<_>>();
    let bus_order = document
        .audio_mix
        .buses
        .iter()
        .map(|bus| bus.id)
        .collect::<Vec<_>>();
    // AU3 §3.8: the per-family windows the observer feed is trimmed through —
    // AU2 §3.7's head drop (per family) plus `keep_from`, then `T` frames.
    let keep_from_frames = usize::try_from(frame_to_samples(range.start, AUDIO_RATE, document.fps))
        .unwrap_or(usize::MAX);
    let kept_frames = usize::try_from(
        frame_to_samples(range.end, AUDIO_RATE, document.fps).saturating_sub(frame_to_samples(
            range.start,
            AUDIO_RATE,
            document.fps,
        )),
    )
    .unwrap_or(usize::MAX);
    let mut track_windows = track_order
        .iter()
        .map(|_| FamilyWindow::new(keep_from_frames, kept_frames))
        .collect::<Vec<_>>();
    let bus_head_frames = processor.bus_stage_frames();
    let mut bus_windows = bus_order
        .iter()
        .map(|_| {
            FamilyWindow::new(
                bus_head_frames.saturating_add(keep_from_frames),
                kept_frames,
            )
        })
        .collect::<Vec<_>>();
    let mut master_window =
        FamilyWindow::new(latency.saturating_add(keep_from_frames), kept_frames);
    let run_stems = collect.stems || observer.wants_stems();
    let mut mix = Vec::with_capacity(if collect.master { total_samples } else { 0 });
    let mut track_stems = track_order
        .iter()
        .map(|track| (*track, Vec::<f32>::new()))
        .collect::<Vec<_>>();
    let mut bus_stems = bus_order
        .iter()
        .map(|bus| (*bus, Vec::<f32>::new()))
        .collect::<Vec<_>>();
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
        if run_stems {
            let stems = processor.mix_chunk_with_stems(&chunk_tracks, start_frame, frame_count)?;
            for ((track, window), chunk) in track_order
                .iter()
                .zip(&mut track_windows)
                .zip(&stems.tracks)
            {
                observer.track(*track, window.take(chunk, channel_count))?;
            }
            for ((bus, window), chunk) in bus_order.iter().zip(&mut bus_windows).zip(&stems.buses) {
                observer.bus(*bus, window.take(chunk, channel_count))?;
            }
            observer.master(master_window.take(&stems.master, channel_count))?;
            if collect.stems {
                for (stem, chunk) in track_stems.iter_mut().zip(stems.tracks) {
                    stem.1.extend_from_slice(&chunk);
                }
                for (stem, chunk) in bus_stems.iter_mut().zip(stems.buses) {
                    stem.1.extend_from_slice(&chunk);
                }
            }
            if collect.master {
                mix.extend(stems.master);
            }
        } else {
            let chunk = processor.mix_chunk(&chunk_tracks, start_frame, frame_count)?;
            observer.master(master_window.take(&chunk, channel_count))?;
            if collect.master {
                mix.extend(chunk);
            }
        }
        start_frame = start_frame.saturating_add(u64::try_from(frame_count).unwrap_or(u64::MAX));
    }
    limit_audio_mix(&mut mix);
    // AU2 §3.7: the head trim, per stem family and dropped unconditionally.
    // Track stems are tapped pre-bus and carry nothing; bus stems are tapped
    // after the bus stage's alignment pad; the master carries the whole graph
    // latency. Unlike `keep_from`, this runs even when `range.start` is zero.
    drop_leading_samples(&mut mix, latency.saturating_mul(channel_count));
    let bus_head = bus_head_frames.saturating_mul(channel_count);
    for stem in &mut bus_stems {
        drop_leading_samples(&mut stem.1, bus_head);
    }
    let keep_from = keep_from_frames
        .saturating_mul(channel_count)
        .min(mix.len());
    if keep_from > 0 {
        mix.drain(..keep_from);
        for stem in &mut track_stems {
            let cut = keep_from.min(stem.1.len());
            stem.1.drain(..cut);
        }
        for stem in &mut bus_stems {
            let cut = keep_from.min(stem.1.len());
            stem.1.drain(..cut);
        }
    }
    // AU2 §3.7/A2: and the tail trim, so every family covers exactly
    // `[range.start, range.end)` and the meter reports one `sample_frames`
    // for one requested range.
    let kept = kept_frames.saturating_mul(channel_count);
    mix.truncate(kept);
    for stem in &mut track_stems {
        stem.1.truncate(kept);
    }
    for stem in &mut bus_stems {
        stem.1.truncate(kept);
    }
    Ok(MixStems {
        tracks: if collect.stems {
            track_stems
        } else {
            Vec::new()
        },
        buses: if collect.stems { bus_stems } else { Vec::new() },
        master: mix,
    })
}

/// AU3 §3.8: `measure_mix_levels`' observer — one meter per track, per bus,
/// and for the master, fed through the pass's family windows. The master
/// meter measures a clamped copy: the signal export encodes.
struct LevelsObserver {
    tracks: Vec<(TrackId, LoudnessMeter)>,
    buses: Vec<(AudioBusId, LoudnessMeter)>,
    master: LoudnessMeter,
    clamped: Vec<f32>,
}

impl LevelsObserver {
    fn new(document: &Document) -> Result<Self, MediaError> {
        let meter = || LoudnessMeter::new(AUDIO_RATE, AUDIO_CHANNELS);
        Ok(Self {
            tracks: document
                .tracks
                .iter()
                .map(|track| Ok((track.id, meter()?)))
                .collect::<Result<_, MediaError>>()?,
            buses: document
                .audio_mix
                .buses
                .iter()
                .map(|bus| Ok((bus.id, meter()?)))
                .collect::<Result<_, MediaError>>()?,
            master: meter()?,
            clamped: Vec::new(),
        })
    }
}

impl MixObserver for LevelsObserver {
    fn wants_stems(&self) -> bool {
        true
    }

    fn track(&mut self, track: TrackId, chunk: &[f32]) -> Result<(), MediaError> {
        if let Some((_, meter)) = self.tracks.iter_mut().find(|(id, _)| *id == track) {
            meter.push(chunk)?;
        }
        Ok(())
    }

    fn bus(&mut self, bus: AudioBusId, chunk: &[f32]) -> Result<(), MediaError> {
        if let Some((_, meter)) = self.buses.iter_mut().find(|(id, _)| *id == bus) {
            meter.push(chunk)?;
        }
        Ok(())
    }

    fn master(&mut self, chunk: &[f32]) -> Result<(), MediaError> {
        self.clamped.clear();
        self.clamped.extend_from_slice(chunk);
        limit_audio_mix(&mut self.clamped);
        self.master.push(&self.clamped)?;
        Ok(())
    }
}

/// AU3 §2.5 / §3.3: refuse a clamped range shorter than one gating block
/// before anything is decoded, so that after a refusal-free measurement
/// `integrated == None` means silent and only silent.
fn refuse_short_loudness_range(
    document: &Document,
    range: &std::ops::Range<TimeCode>,
) -> Result<(), MediaError> {
    let sample_frames = frame_to_samples(range.end, AUDIO_RATE, document.fps)
        .saturating_sub(frame_to_samples(range.start, AUDIO_RATE, document.fps));
    if sample_frames < LOUDNESS_GATING_BLOCK_FRAMES {
        return Err(MediaError::MixLoudnessRangeTooShort {
            sample_frames,
            required: LOUDNESS_GATING_BLOCK_FRAMES,
        });
    }
    Ok(())
}

/// AU1 §6.1: measure every track, bus, and the master over one project range.
///
/// AU3 §3.8: streamed through a [`LevelsObserver`] — no stem is held whole —
/// after the §2.5 length refusal.
///
/// # Errors
///
/// Returns [`MediaError::MixLoudnessRangeTooShort`] when the clamped range
/// holds fewer than one 400 ms gating block, or a media error when the range
/// is empty or the mix cannot be rendered or measured.
pub(crate) fn measure_mix_levels(
    document: &Document,
    request: &MixLevelRequest,
) -> Result<MixLevelReport, MediaError> {
    let range = clamped_measurement_range(document, request.range.clone(), "mix level")?;
    refuse_short_loudness_range(document, &range)?;
    let settings = measurement_settings(document);
    let mut observer = LevelsObserver::new(document)?;
    mix_pass(
        document,
        range.clone(),
        &settings,
        MixCollect {
            stems: false,
            master: false,
        },
        &mut observer,
    )?;
    let LevelsObserver {
        tracks: track_meters,
        buses: bus_meters,
        master,
        ..
    } = observer;
    let mut tracks = Vec::with_capacity(document.tracks.len());
    for (track, (_, meter)) in document.tracks.iter().zip(track_meters) {
        tracks.push(TrackLevels {
            track: track.id,
            kind: track.kind,
            mix: document.track_mix(track.id),
            audible: document.track_audible(track.id),
            bus: document
                .audio_mix
                .buses
                .iter()
                .find(|bus| bus.tracks.contains(&track.id))
                .map(|bus| bus.id),
            levels: meter.finish()?,
        });
    }
    let mut buses = Vec::with_capacity(document.audio_mix.buses.len());
    for (bus, (_, meter)) in document.audio_mix.buses.iter().zip(bus_meters) {
        buses.push(BusLevels {
            bus: bus.id,
            name: bus.name.clone(),
            levels: meter.finish()?,
        });
    }
    Ok(MixLevelReport {
        range,
        any_solo: document.audio_mix.any_solo(),
        tracks,
        buses,
        master: master.finish()?,
    })
}

/// AU3 §3.10: the silence window in sample frames at the measurement rate —
/// `detect_silences`' `ceil(rate · ms / 1 000)`, 480 at 48 kHz.
const AUDIO_QC_SILENCE_WINDOW_FRAMES: usize =
    (AUDIO_RATE as usize * AUDIO_QC_SILENCE_WINDOW_MILLISECONDS as usize).div_ceil(1_000);

/// AU3 §3.10: `audio_qc`'s observer over the master feed.
///
/// Clipping is counted per channel on the **pre-clamp** chunk with the run
/// state carried across callbacks; a clamped copy goes to the loudness meter
/// (`report.master`, the balance) and to the silence windows, whose square
/// sum and count persist across callbacks so a 480-frame window split by a
/// 1 024-frame chunk boundary is closed once, when its 480th frame arrives.
pub(crate) struct QcObserver {
    meter: LoudnessMeter,
    clamped: Vec<f32>,
    channel_frames: u64,
    over_full_scale: [u64; 2],
    clipped_runs: [u32; 2],
    run_length: [u32; 2],
    window_square_sum: f64,
    window_frames: usize,
    silence_threshold: f64,
    windows: u64,
    leading_silent_windows: u64,
    all_silent_so_far: bool,
    trailing_silent_windows: u64,
    trailing_silent_frames: u64,
}

impl QcObserver {
    pub(crate) fn new() -> Result<Self, MediaError> {
        Ok(Self {
            meter: LoudnessMeter::new(AUDIO_RATE, AUDIO_CHANNELS)?,
            clamped: Vec::new(),
            channel_frames: 0,
            over_full_scale: [0; 2],
            clipped_runs: [0; 2],
            run_length: [0; 2],
            window_square_sum: 0.0,
            window_frames: 0,
            silence_threshold: 10.0_f64.powf(f64::from(AUDIO_QC_SILENCE_DBFS_HUNDREDTHS) / 2_000.0),
            windows: 0,
            leading_silent_windows: 0,
            all_silent_so_far: true,
            trailing_silent_windows: 0,
            trailing_silent_frames: 0,
        })
    }

    /// Close one silence window of `frames` frames (480, or the partial one at
    /// `range.end`).
    #[allow(clippy::cast_precision_loss)]
    fn close_window(&mut self, frames: usize) {
        let samples = (frames * usize::from(AUDIO_CHANNELS)).max(1) as f64;
        let rms = (self.window_square_sum / samples).sqrt();
        self.window_square_sum = 0.0;
        self.window_frames = 0;
        self.windows += 1;
        let frames = u64::try_from(frames).unwrap_or(u64::MAX);
        if rms <= self.silence_threshold {
            if self.all_silent_so_far {
                self.leading_silent_windows += 1;
            }
            self.trailing_silent_windows += 1;
            self.trailing_silent_frames = self.trailing_silent_frames.saturating_add(frames);
        } else {
            self.all_silent_so_far = false;
            self.trailing_silent_windows = 0;
            self.trailing_silent_frames = 0;
        }
    }

    /// Sample frames covered by the leading silent run.
    fn leading_silent_frames(&self) -> u64 {
        self.leading_silent_windows
            .saturating_mul(u64::try_from(AUDIO_QC_SILENCE_WINDOW_FRAMES).unwrap_or(u64::MAX))
            .min(self.channel_frames)
    }

    fn clipping(&self) -> AudioClipping {
        let channel = |index: usize| AudioChannelClipping {
            over_full_scale_samples: self.over_full_scale[index],
            clipped_runs: self.clipped_runs[index],
            basis_points: self.over_full_scale[index]
                .saturating_mul(10_000)
                .checked_div(self.channel_frames)
                .map_or(0, |points| u32::try_from(points).unwrap_or(u32::MAX)),
        };
        AudioClipping {
            left: channel(0),
            right: channel(1),
        }
    }

    /// Finish the measurement: the last partial window is closed over its
    /// own length, the meter is finished, and the report is judged by core.
    pub(crate) fn finish(
        mut self,
        range: std::ops::Range<TimeCode>,
        fps: kinewright_core::Rational,
        target: Option<kinewright_core::LoudnessTarget>,
    ) -> Result<AudioQcReport, MediaError> {
        if self.window_frames > 0 {
            let frames = self.window_frames;
            self.close_window(frames);
        }
        let clipping = self.clipping();
        let leading_frames = self.leading_silent_frames();
        let trailing_frames = self.trailing_silent_frames;
        let channel_balance_lu_hundredths = self.meter.channel_balance_lu_hundredths();
        let master = self.meter.finish()?;
        let to_frames = |sample_frames: u64| samples_to_frame(sample_frames, AUDIO_RATE, fps);
        let to_milliseconds =
            |sample_frames: u64| sample_frames.saturating_mul(1_000) / u64::from(AUDIO_RATE);
        let measured = AudioQcMeasurements {
            master,
            channel_balance_lu_hundredths,
            clipping,
            leading_silence_frames: to_frames(leading_frames),
            leading_silence_milliseconds: to_milliseconds(leading_frames),
            trailing_silence_frames: to_frames(trailing_frames),
            trailing_silence_milliseconds: to_milliseconds(trailing_frames),
            target,
        };
        let exceptions = audio_qc_exceptions(&measured);
        Ok(AudioQcReport {
            range,
            master,
            channel_balance_lu_hundredths,
            clipping,
            leading_silence_frames: measured.leading_silence_frames,
            trailing_silence_frames: measured.trailing_silence_frames,
            target,
            technical_pass: audio_qc_technical_pass(&exceptions),
            exceptions,
            evidence_only: true,
            provenance: AudioQcProvenance::default(),
        })
    }
}

impl MixObserver for QcObserver {
    fn master(&mut self, chunk: &[f32]) -> Result<(), MediaError> {
        let channels = usize::from(AUDIO_CHANNELS);
        for frame in chunk.chunks_exact(channels) {
            self.channel_frames += 1;
            for (channel, sample) in frame.iter().enumerate().take(2) {
                if sample.abs() >= 1.0 {
                    self.over_full_scale[channel] += 1;
                    self.run_length[channel] += 1;
                    if self.run_length[channel] == AUDIO_QC_CLIPPED_RUN_SAMPLES {
                        self.clipped_runs[channel] += 1;
                    }
                } else {
                    self.run_length[channel] = 0;
                }
            }
        }
        let mut clamped = std::mem::take(&mut self.clamped);
        clamped.clear();
        clamped.extend_from_slice(chunk);
        limit_audio_mix(&mut clamped);
        // The `?` is deferred to the end so `clamped` — taken out of `self`
        // to borrow it while `self.meter` is borrowed mutably — is always put
        // back, and its buffer reused, on the error path too. The error is in
        // practice unreachable: the feed is whole stereo frames, and
        // `FamilyWindow::take` only ever slices on a frame boundary.
        let pushed = self.meter.push(&clamped);
        for frame in clamped.chunks_exact(channels) {
            self.window_square_sum += frame
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>();
            self.window_frames += 1;
            if self.window_frames == AUDIO_QC_SILENCE_WINDOW_FRAMES {
                self.close_window(AUDIO_QC_SILENCE_WINDOW_FRAMES);
            }
        }
        self.clamped = clamped;
        pushed.map(|_| ())
    }
}

/// AU3 §3.10: the audio QC measurement of the post-clamp master over one
/// project range, judged by core's `audio_qc_exceptions`.
///
/// # Errors
///
/// Returns [`MediaError::MixLoudnessRangeTooShort`] when the clamped range
/// holds fewer than one 400 ms gating block (before anything is decoded), or
/// a media error when the range is empty or the mix cannot be rendered.
pub(crate) fn measure_audio_qc(
    document: &Document,
    request: &AudioQcRequest,
) -> Result<AudioQcReport, MediaError> {
    let range = clamped_measurement_range(document, request.range.clone(), "audio QC")?;
    refuse_short_loudness_range(document, &range)?;
    let settings = measurement_settings(document);
    let mut observer = QcObserver::new()?;
    mix_pass(
        document,
        range.clone(),
        &settings,
        MixCollect {
            stems: false,
            master: false,
        },
        &mut observer,
    )?;
    observer.finish(
        range,
        document.fps,
        request.profile.map(DeliveryProfile::loudness_target),
    )
}

/// AU1 §6.1 / AU2 §5.9: the clamped project range one measurement covers.
///
/// # Errors
///
/// Returns a media error when the range is empty after clamping.
fn clamped_measurement_range(
    document: &Document,
    requested: Option<std::ops::Range<TimeCode>>,
    what: &str,
) -> Result<std::ops::Range<TimeCode>, MediaError> {
    let requested = requested.unwrap_or(TimeCode::ZERO..document.duration);
    let range = requested.start.max(TimeCode::ZERO)..requested.end.min(document.duration);
    if range.start >= range.end {
        return Err(MediaError::Backend(format!(
            "{what} range {}..{} is empty after clamping to 0..{}",
            requested.start.0, requested.end.0, document.duration.0
        )));
    }
    Ok(range)
}

/// AU2 §5.9: the third-octave spectrum of one mix point over a project range.
///
/// Measured on the stem `mix_audio_stems` produces — the same path
/// `measure_mix_levels` uses — so the reading runs through the real graph at
/// 48 kHz with §3.7's head-and-tail trim already applied.
///
/// # Errors
///
/// Returns [`MediaError::MixSpectrumRangeTooShort`] when the clamped range
/// holds fewer than two Welch segments, or a media error when the range is
/// empty, the mix point is not in the document, or the mix cannot be rendered.
pub(crate) fn measure_mix_spectrum(
    document: &Document,
    request: &MixSpectrumRequest,
) -> Result<MixSpectrumReport, MediaError> {
    let range = clamped_measurement_range(document, request.range.clone(), "mix spectrum")?;
    // AU2 §5.9/A23: rejected before any decoding, so a too-short range never
    // costs a mix pass and never returns a degenerate spectrum.
    let sample_frames = frame_to_samples(range.end, AUDIO_RATE, document.fps)
        .saturating_sub(frame_to_samples(range.start, AUDIO_RATE, document.fps));
    if sample_frames < SPECTRUM_MINIMUM_FRAMES {
        return Err(MediaError::MixSpectrumRangeTooShort {
            sample_frames,
            required: SPECTRUM_MINIMUM_FRAMES,
        });
    }
    let settings = measurement_settings(document);
    let stems = mix_audio_stems(document, range.clone(), &settings)?;
    let samples = match request.point {
        MixSpectrumPoint::Master => &stems.master,
        MixSpectrumPoint::Track(track) => stems
            .tracks
            .iter()
            .find(|(id, _)| *id == track)
            .map(|(_, samples)| samples)
            .ok_or_else(|| {
                MediaError::Backend(format!("track {} is not in the document", track.0))
            })?,
        MixSpectrumPoint::Bus(bus) => stems
            .buses
            .iter()
            .find(|(id, _)| *id == bus)
            .map(|(_, samples)| samples)
            .ok_or_else(|| MediaError::Backend(format!("bus {} is not in the document", bus.0)))?,
    };
    let channels = usize::from(AUDIO_CHANNELS);
    let measured = third_octave_spectrum(samples, channels, AUDIO_RATE).ok_or(
        MediaError::MixSpectrumRangeTooShort {
            sample_frames: u64::try_from(samples.len() / channels.max(1)).unwrap_or(0),
            required: SPECTRUM_MINIMUM_FRAMES,
        },
    )?;
    Ok(MixSpectrumReport {
        range,
        point: request.point,
        sample_rate: AUDIO_RATE,
        sample_frames: u64::try_from(samples.len() / channels.max(1)).unwrap_or(0),
        segments: measured.segments,
        bands: measured.bands,
    })
}

/// AU1 §6.1: the throwaway settings a measurement pass needs.
fn measurement_settings(document: &Document) -> ExportSettings {
    ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 1,
        audio_bitrate: 1,
        loudness_normalization: None,
        cancellation: ExportCancellation::default(),
    }
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
/// **8-bit or 10-bit SDR Rec.709** (BT.709 primaries, transfer, and matrix;
/// limited range; D65; nonzero confidence; `application_default` or
/// `user_override` provenance) in limited-range YUV420P/YUV420P10LE. Keep this
/// gate in front of encoder setup so an unknown or future colour description
/// cannot be mislabeled with today's tags.
///
/// Rejections are typed (CC6 4.2): every one carries `code`, `field`,
/// `observed`, `allowed`, and a recovery action, so an agent or a UI never has
/// to parse a sentence to learn which field was wrong.
fn validate_delivery_color(settings: &ExportSettings) -> Result<(), MediaError> {
    if settings.video_codec != DELIVERY_VIDEO_CODEC {
        return Err(DeliveryColorError::UnsupportedCodec {
            observed: settings.video_codec.clone(),
            allowed: DELIVERY_VIDEO_CODEC,
        }
        .into());
    }
    validate_delivery_description(&settings.delivery_color)
}

/// Gate one delivery colour description against the **8-bit or 10-bit SDR
/// Rec.709** delivery contract.
///
/// The accepted set is Core's, never a second transcription of it: the fields
/// and their allowed values come from `delivery_color_mismatches`, so this gate
/// and `delivery_conformance` cannot disagree -- including on
/// [`kinewright_core::ColorBitDepth`]'s canonical equality, which makes
/// `Integer(8)`/`Integer(10)` the same declared depths as `Eight`/`Ten`.
///
/// The first mismatch in Core's fixed check order is the reported one; a caller
/// that wants all of them calls `delivery_color_mismatches` directly.
fn validate_delivery_description(color: &ColorDescription) -> Result<(), MediaError> {
    match delivery_color_mismatches(color).into_iter().next() {
        Some(mismatch) => Err(DeliveryColorError::UnsupportedField(mismatch).into()),
        None => Ok(()),
    }
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
    use crate::cc1_fixtures::{fallback_gpu, generate_delivery_source, simple_document};
    use crate::color_pipeline::{DELIVERY_INTERMEDIATE_WHITE, encode_delivery_rgba16};
    use crate::compositor::{Compositor, CompositorLayer};
    use crate::decode::probe_path;
    use crate::test_support::TempDirectory;
    use crate::timeline::TransitionRenderParams;
    use kinewright_core::{
        AssetId, ColorBitDepth, ColorContext, ColorMatrix, ColorPrimaries, ColorProvenance,
        ColorRange, ColorTransfer, DeliveryEncodeDepth, DeliveryProfile, ExportCancellation,
        FrameTexture, Rational,
    };
    use std::collections::BTreeSet;
    use std::process::Command;
    use std::sync::Arc;

    /// The timing fixture: long enough that libx264's B-frame delay is in play
    /// and the mov muxer writes the negative-DTS edit list.
    const TIMING_FIXTURE_FRAMES: u32 = 30;
    const TIMING_FIXTURE_FPS: u32 = 25;
    const TIMING_FIXTURE_SIZE: (u32, u32) = (64, 32);

    /// The two rates the presented-frame check runs at.
    ///
    /// `30000/1001` is the rate an edit list is most likely to get wrong: the
    /// media time base carries no whole number of ticks per frame, so a track
    /// duration computed from the last packet's `pts + 0` lands *between*
    /// frame boundaries instead of exactly one frame short, and a fix that
    /// only ever added one integer tick at an integer rate would not survive
    /// it. A delivery is not less of a delivery for being NTSC.
    const TIMING_FIXTURE_RATES: [(&str, u32, u32); 2] =
        [("25", TIMING_FIXTURE_FPS, 1), ("30000/1001", 30_000, 1_001)];

    /// The typed delivery rejection behind a [`MediaError`].
    ///
    /// A `MediaError::Backend(String)` here would mean the gate lost its
    /// structure, which is exactly what CC6 4.2 removes; the panic says so.
    fn delivery_error(error: &MediaError) -> &DeliveryColorError {
        match error {
            MediaError::DeliveryColor(typed) => typed,
            other => panic!("delivery rejections must be typed, not a string: {other}"),
        }
    }

    /// Assert all four structured facts of one typed delivery rejection.
    fn assert_delivery_rejection(
        error: &MediaError,
        code: &str,
        field: &str,
        observed: &str,
        allowed: &str,
    ) {
        let typed = delivery_error(error);
        assert_eq!(typed.code(), code, "code of {typed}");
        assert_eq!(typed.field(), field, "field of {typed}");
        assert_eq!(typed.observed(), observed, "observed of {typed}");
        assert_eq!(typed.allowed_values(), allowed, "allowed of {typed}");
        assert!(
            !typed.recovery_action().is_empty(),
            "a typed rejection must state a recovery action: {typed}"
        );
        assert_eq!(
            error.recovery_code(),
            Some(code),
            "MediaError must surface the same code"
        );
    }

    /// Every luma sample of a filtered frame, as a set of distinct codes.
    ///
    /// `swscale` writes with a stride, and its 8-bit output carries a
    /// *deterministic* 8x8 ordered dither, so a flat input generally lands on
    /// two adjacent codes. Collecting the whole plane (rather than sampling
    /// pixel 0) is what makes "every sample is legal white" assertable.
    fn luma_codes(frame: &ffmpeg::frame::Video) -> BTreeSet<u8> {
        let width = usize::try_from(frame.width()).expect("filtered width");
        let height = usize::try_from(frame.height()).expect("filtered height");
        let stride = frame.stride(0);
        let plane = frame.data(0);
        (0..height)
            .flat_map(|row| plane[row * stride..row * stride + width].iter().copied())
            .collect()
    }

    /// The same, for one chroma plane of a 4:2:0 frame.
    fn chroma_codes(frame: &ffmpeg::frame::Video, plane_index: usize) -> BTreeSet<u8> {
        let width = usize::try_from(frame.width()).expect("filtered width") / 2;
        let height = usize::try_from(frame.height()).expect("filtered height") / 2;
        let stride = frame.stride(plane_index);
        let plane = frame.data(plane_index);
        (0..height)
            .flat_map(|row| plane[row * stride..row * stride + width].iter().copied())
            .collect()
    }

    /// Every luma sample of a 10-bit filtered frame, as a set of distinct codes.
    ///
    /// libswscale applies **no** dither on the 16-to-10-bit path (CC6 5.4), so
    /// a flat input legitimately yields a single code here where the 8-bit lane
    /// yields two.
    fn luma_codes_10bit(frame: &ffmpeg::frame::Video) -> BTreeSet<u16> {
        plane_codes_10bit(frame, 0, frame.width(), frame.height())
    }

    /// The same, for one chroma plane of a 4:2:0 10-bit frame.
    fn chroma_codes_10bit(frame: &ffmpeg::frame::Video, plane_index: usize) -> BTreeSet<u16> {
        plane_codes_10bit(frame, plane_index, frame.width() / 2, frame.height() / 2)
    }

    fn plane_codes_10bit(
        frame: &ffmpeg::frame::Video,
        plane_index: usize,
        width: u32,
        height: u32,
    ) -> BTreeSet<u16> {
        let width = usize::try_from(width).expect("filtered width");
        let height = usize::try_from(height).expect("filtered height");
        let stride = frame.stride(plane_index);
        let plane = frame.data(plane_index);
        (0..height)
            .flat_map(|row| {
                plane[row * stride..row * stride + width * 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|bytes| u16::from_le_bytes(*bytes))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Push one flat RGBA64LE code through the production delivery graph.
    fn filter_flat_delivery_code(
        code: [u16; 4],
        resolution: (u32, u32),
        depth: DeliveryEncodeDepth,
    ) -> ffmpeg::frame::Video {
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let mut filter = delivery_filter_graph(resolution, depth).expect("delivery filter graph");
        let count = usize::try_from(resolution.0 * resolution.1).expect("raster size");
        let pixels = std::iter::repeat_n(code, count)
            .flatten()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut rgba =
            ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, resolution.0, resolution.1);
        stamp_rgba_color(&mut rgba);
        copy_rgba64_to_frame(&pixels, &mut rgba).expect("RGBA64LE frame copy");
        let yuv = filter
            .run(&rgba)
            .expect("explicit BT.709 limited conversion");
        assert_eq!(yuv.format(), delivery_lane_pixel_format(depth));
        yuv
    }

    /// The pixel format an exported file actually decodes as.
    fn decoded_video_pixel_format(path: &Path) -> String {
        let input = ffmpeg::format::input(path).expect("exported video should open");
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .expect("exported video should have a video stream");
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .expect("exported video codec parameters should decode");
        let decoder = context
            .decoder()
            .video()
            .expect("exported video decoder should open");
        pixel_format_name(decoder.format()).to_owned()
    }

    /// A tagged, lossless, `TIMING_FIXTURE_FRAMES`-frame source at `rate`,
    /// written by the pinned CLI. Test-only: production never shells out.
    ///
    /// `rate` is an `FFmpeg` rate spelling (`"25"`, `"30000/1001"`), so the
    /// fractional lane is the *source*'s rate rather than a rate imposed on a
    /// file that was written at another one.
    fn generate_timing_source(directory: &TempDirectory, name: &str, rate: &str) -> PathBuf {
        let (width, height) = TIMING_FIXTURE_SIZE;
        let mut input = Vec::new();
        for frame in 0..TIMING_FIXTURE_FRAMES {
            // A moving bar, so consecutive pictures genuinely differ and the
            // encoder has a reason to emit B-frames.
            let column = (frame * 2) % width;
            for _y in 0..height {
                for x in 0..width {
                    input.push(if (column..column + 4).contains(&x) {
                        235_u8
                    } else {
                        16_u8
                    });
                }
            }
            input.extend(std::iter::repeat_n(
                128_u8,
                usize::try_from(width * height * 2).expect("chroma planes"),
            ));
        }
        let path = directory.path(name);
        let mut command = Command::new(crate::test_support::ffmpeg_executable());
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv444p",
                "-s",
                &format!("{width}x{height}"),
                "-r",
                rate,
                "-i",
                "pipe:0",
                "-vf",
                "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-g",
                "1",
                "-pix_fmt",
                "yuv444p",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
            ])
            .arg(&path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().expect("the pinned FFmpeg CLI should start");
        std::io::Write::write_all(
            &mut child.stdin.take().expect("timing source stdin"),
            &input,
        )
        .expect("write the timing source");
        let output = child.wait_with_output().expect("FFmpeg process");
        assert!(
            output.status.success(),
            "timing source generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    }

    /// One `ffprobe` field of the first video stream, decoding every frame.
    ///
    /// Test-only, and deliberately an *independent* reader: the crate's own
    /// decoder and the pinned CLI must agree that every coded picture is
    /// presented.
    fn ffprobe_video_field(path: &Path, entry: &str) -> String {
        let output = Command::new(crate::test_support::ffprobe_executable())
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                &format!("stream={entry}"),
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(path)
            .output()
            .expect("the provisioned ffprobe should run");
        assert!(
            output.status.success(),
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// CC6: an exported MP4 must present every frame it codes.
    ///
    /// libx264's B-frame delay makes the first packets carry negative DTS; the
    /// mov muxer answers by shifting the media timeline and writing an `elst`.
    /// When the packets are muxed with **zero duration** the muxer computes the
    /// track duration as the last packet's `pts + 0`, so that edit list is one
    /// frame short, `FFmpeg`'s demuxer flags the final coded picture
    /// `AV_PKT_FLAG_DISCARD`, and every player drops the last frame. Stamping
    /// one tick of the encoder time base on every packet is what makes the
    /// track -- and therefore the edit list -- cover all `T` pictures.
    ///
    /// Asserted twice, from two independent readers: the crate's own decoder
    /// reading the file exactly as a player would (edit list honoured), and the
    /// pinned `ffprobe -count_frames`.
    #[test]
    fn every_exported_frame_is_presented_after_the_mp4_edit_list() {
        // Rule 11.0.6: the panicking acquisition, never the skipping one. A
        // GPU-backed delivery fixture that reports `ok` without running is
        // indistinguishable from one that passed.
        let gpu = fallback_gpu().context();
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let directory = TempDirectory::new("cc6-export-presented-frames");
        // Both rates, in one test: the integer lane and the NTSC lane are the
        // same claim about the same muxer, and splitting them would let one be
        // fixed while the other rotted.
        for (label, numerator, denominator) in TIMING_FIXTURE_RATES {
            let source = generate_timing_source(
                &directory,
                &format!("cc6-timing-source-{numerator}-{denominator}.mkv"),
                label,
            );
            let asset = probe_path(&source, AssetId(2)).expect("the timing source should probe");
            assert_eq!(
                asset.fps,
                Rational::new(numerator, denominator).expect("the fixture rate"),
                "{label}: the source must carry the rate it was written at"
            );
            assert_eq!(
                asset.duration,
                TimeCode(i64::from(TIMING_FIXTURE_FRAMES)),
                "{label}: the fixture must code every frame it claims"
            );
            let document = simple_document(asset, TIMING_FIXTURE_SIZE);
            document
                .validate()
                .expect("the timing document should validate");
            let settings = DeliveryProfile::SourceMaster.export_settings(
                &document,
                DeliveryEncodeDepth::Eight,
                ExportCancellation::default(),
            );
            assert_eq!(settings.fps, document.fps);
            let output =
                directory.path(&format!("cc6-timing-export-{numerator}-{denominator}.mp4"));
            let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
            export_document(&document, &output, &settings, &progress_tx, gpu.clone())
                .expect("the production export should write the timing fixture");

            // (a) The crate's own decoder, opened with no options at all, so
            //     the edit list is honoured exactly as a player honours it.
            assert_eq!(
                crate::verify::presented_frame_count(&output).expect("the export decodes"),
                u64::from(TIMING_FIXTURE_FRAMES),
                "{label}: every exported frame must survive the MP4 edit list"
            );

            // (b) The pinned CLI, independently.
            assert_eq!(
                ffprobe_video_field(&output, "nb_read_frames"),
                TIMING_FIXTURE_FRAMES.to_string(),
                "{label}: ffprobe must decode every exported frame"
            );
            assert_eq!(
                ffprobe_video_field(&output, "nb_frames"),
                TIMING_FIXTURE_FRAMES.to_string(),
                "{label}: the container must count every exported frame"
            );
            let duration: f64 = ffprobe_video_field(&output, "duration")
                .parse()
                .expect("ffprobe should report a numeric stream duration");
            let expected =
                f64::from(TIMING_FIXTURE_FRAMES) * f64::from(denominator) / f64::from(numerator);
            assert!(
                (duration - expected).abs() < 1e-3,
                "{label}: the stream must last T/fps = {expected} s, not {duration} s"
            );
        }
    }

    #[test]
    fn accepts_the_current_sdr_rec709_delivery_contract() {
        let color = ColorContext::sdr_rec709().delivery;
        assert_eq!(color.bit_depth, ColorBitDepth::Eight);
        assert!(validate_delivery_description(&color).is_ok());
        assert_eq!(
            delivery_encode_depth(&color).expect("the 8-bit lane"),
            DeliveryEncodeDepth::Eight
        );
    }

    /// CC6 4.1: the 10-bit lane differs from the 8-bit lane in exactly one
    /// field, and that field is the single authority for the encode depth.
    #[test]
    fn accepts_the_ten_bit_sdr_rec709_delivery_contract() {
        let eight = ColorContext::sdr_rec709().delivery;
        let ten = ColorDescription {
            bit_depth: DeliveryEncodeDepth::Ten.color_bit_depth(),
            ..eight.clone()
        };
        assert_eq!(ten.bit_depth, ColorBitDepth::Ten);
        assert!(validate_delivery_description(&ten).is_ok());
        assert_eq!(
            delivery_encode_depth(&ten).expect("the 10-bit lane"),
            DeliveryEncodeDepth::Ten
        );
        // Only the depth moved: every other delivery field is byte-identical
        // to the 8-bit lane, which is why the tags and the x264 params are.
        assert_eq!(
            ColorDescription {
                bit_depth: eight.bit_depth.clone(),
                ..ten.clone()
            },
            eight
        );

        // The numeric spelling of the same declared depth is the same depth
        // (CC1 2.1 canonical equality), and the settings the queue materializes
        // for the 10-bit lane pass the gate unchanged.
        let numeric = ColorDescription {
            bit_depth: ColorBitDepth::Integer(10),
            ..ten
        };
        assert!(validate_delivery_description(&numeric).is_ok());
        assert_eq!(
            delivery_encode_depth(&numeric).expect("the 10-bit lane"),
            DeliveryEncodeDepth::Ten
        );

        let settings = DeliveryProfile::Youtube1080p.export_settings(
            &Document::default(),
            DeliveryEncodeDepth::Ten,
            ExportCancellation::default(),
        );
        assert_eq!(settings.delivery_color.bit_depth, ColorBitDepth::Ten);
        assert!(validate_delivery_color(&settings).is_ok());
    }

    #[test]
    fn rejects_unknown_delivery_metadata() {
        let error = validate_delivery_description(&ColorDescription::default())
            .expect_err("unknown delivery metadata must not be tagged as Rec.709");
        // Core's fixed check order reports `primaries` first.
        assert_delivery_rejection(
            &error,
            "unsupported_delivery_color",
            "primaries",
            "Unknown",
            "bt709",
        );
    }

    #[test]
    fn rejects_zero_confidence_delivery_metadata() {
        let mut color = ColorContext::sdr_rec709().delivery;
        color.confidence_basis_points = 0;
        let error = validate_delivery_description(&color)
            .expect_err("zero-confidence delivery metadata must not be tagged as Rec.709");
        assert_delivery_rejection(
            &error,
            "unsupported_delivery_color",
            "confidence_basis_points",
            "0",
            "1..=10000",
        );
    }

    /// Every delivery depth outside the two managed lanes is refused with the
    /// depth named, at both entry points (CC6 4.1).
    #[test]
    fn rejects_a_delivery_depth_outside_the_two_managed_lanes() {
        for (depth, observed) in [
            (ColorBitDepth::Unknown, "Unknown"),
            (ColorBitDepth::Integer(12), "Integer(12)"),
            (ColorBitDepth::Float16, "Float16"),
        ] {
            let color = ColorDescription {
                bit_depth: depth,
                ..ColorContext::sdr_rec709().delivery
            };
            let error = validate_delivery_description(&color)
                .expect_err("an unmanaged delivery depth must not be encoded");
            assert_delivery_rejection(
                &error,
                "unsupported_delivery_color",
                "bit_depth",
                observed,
                kinewright_core::DELIVERY_BIT_DEPTH_ALLOWED,
            );
            let error = delivery_encode_depth(&color)
                .expect_err("an unmanaged delivery depth selects no lane");
            assert_delivery_rejection(
                &error,
                "unsupported_delivery_color",
                "bit_depth",
                observed,
                kinewright_core::DELIVERY_BIT_DEPTH_ALLOWED,
            );
        }
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
            let observed = format!("{:?}", color.provenance);
            let error = validate_delivery_description(&color)
                .expect_err("unsupported provenance must not be tagged as Rec.709");
            assert_delivery_rejection(
                &error,
                "unsupported_delivery_color",
                "provenance",
                &observed,
                "application_default or user_override",
            );
        }
    }

    /// Non-Rec.709 delivery metadata is refused on **both** lanes: widening the
    /// depth widened nothing else (CC6 4.1).
    #[test]
    fn rejects_non_rec709_delivery_metadata() {
        for depth in DeliveryEncodeDepth::ALL {
            let mut color = ColorDescription {
                bit_depth: depth.color_bit_depth(),
                ..ColorContext::sdr_rec709().delivery
            };
            color.primaries = ColorPrimaries::Bt2020;
            let error = validate_delivery_description(&color)
                .expect_err("non-Rec.709 delivery metadata is not supported yet");
            assert_delivery_rejection(
                &error,
                "unsupported_delivery_color",
                "primaries",
                "Bt2020",
                "bt709",
            );

            // The other Rec.709 legs, at the same depth, with their own fields.
            for (mutate, field, observed, allowed) in [
                (
                    Box::new(|color: &mut ColorDescription| {
                        color.transfer = ColorTransfer::Smpte2084;
                    }) as Box<dyn Fn(&mut ColorDescription)>,
                    "transfer",
                    "Smpte2084",
                    "bt709",
                ),
                (
                    Box::new(|color: &mut ColorDescription| {
                        color.matrix = ColorMatrix::Bt2020Ncl;
                    }),
                    "matrix",
                    "Bt2020Ncl",
                    "bt709",
                ),
                (
                    Box::new(|color: &mut ColorDescription| {
                        color.range = ColorRange::Full;
                    }),
                    "range",
                    "Full",
                    "limited",
                ),
            ] {
                let mut color = ColorDescription {
                    bit_depth: depth.color_bit_depth(),
                    ..ColorContext::sdr_rec709().delivery
                };
                mutate(&mut color);
                let error = validate_delivery_description(&color)
                    .expect_err("a non-Rec.709 delivery field must be refused at both depths");
                assert_delivery_rejection(
                    &error,
                    "unsupported_delivery_color",
                    field,
                    observed,
                    allowed,
                );
            }
        }
    }

    #[test]
    fn rejects_non_libx264_for_explicit_color_tagging() {
        for depth in DeliveryEncodeDepth::ALL {
            let mut settings = DeliveryProfile::SourceMaster.export_settings(
                &Document::default(),
                depth,
                ExportCancellation::default(),
            );
            settings.video_codec = "h264_nvenc".to_owned();
            let error = validate_delivery_color(&settings)
                .expect_err("unmapped H.264 encoders must not claim tagged output");
            assert_delivery_rejection(
                &error,
                "unsupported_delivery_codec",
                "video_codec",
                "h264_nvenc",
                DELIVERY_VIDEO_CODEC,
            );
        }
    }

    /// The encoder pixel format and the declared depth are the same fact, named
    /// twice; this pins the two spellings together (CC6 4.1/4.3).
    #[test]
    fn delivery_lane_pixel_format_matches_the_core_lane_names() {
        assert_eq!(DeliveryEncodeDepth::Eight.pixel_format(), "yuv420p");
        assert_eq!(DeliveryEncodeDepth::Ten.pixel_format(), "yuv420p10le");
        for depth in DeliveryEncodeDepth::ALL {
            assert_eq!(
                pixel_format_name(delivery_lane_pixel_format(depth)),
                depth.pixel_format(),
                "the media lane format must be the name core declares"
            );
        }
        assert_eq!(
            DELIVERY_X264_PARAMS, "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "the x264 parameter string is identical on both lanes and carries no range= key"
        );
        assert_eq!(DELIVERY_SCALER_FLAGS, "bicubic");
    }

    /// R5 cross-platform rule, passing direction: this build must actually
    /// offer both lanes, and the fixture says so rather than skipping.
    #[test]
    fn libx264_advertises_both_delivery_lane_pixel_formats() {
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let codec = find_codec(DELIVERY_VIDEO_CODEC, ffmpeg::codec::Id::H264)
            .expect("the managed delivery encoder must be present");
        for depth in DeliveryEncodeDepth::ALL {
            let format =
                checked_delivery_pixel_format(codec, depth, delivery_lane_pixel_format(depth))
                    .expect("this build's libx264 must advertise both delivery lane pixel formats");
            assert_eq!(format, delivery_lane_pixel_format(depth));
        }
    }

    /// R5 cross-platform rule, failing direction: a pixel format that does not
    /// carry the declared depth is refused before the encoder opens, so the
    /// depth and the format can never come from two sources.
    #[test]
    fn rejects_a_pixel_format_that_does_not_carry_the_declared_delivery_depth() {
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let codec = find_codec(DELIVERY_VIDEO_CODEC, ffmpeg::codec::Id::H264)
            .expect("the managed delivery encoder must be present");
        let error = checked_delivery_pixel_format(
            codec,
            DeliveryEncodeDepth::Ten,
            ffmpeg::format::Pixel::YUV444P16LE,
        )
        .expect_err("a 16-bit 4:4:4 format may not be encoded as the 10-bit delivery lane");
        assert_delivery_rejection(
            &error,
            "delivery_pixel_format_depth_mismatch",
            "pixel_format",
            "yuv444p16le",
            "yuv420p10le",
        );

        // The lanes may not be crossed either way.
        let error = checked_delivery_pixel_format(
            codec,
            DeliveryEncodeDepth::Eight,
            delivery_lane_pixel_format(DeliveryEncodeDepth::Ten),
        )
        .expect_err("the 10-bit format may not be encoded as the 8-bit delivery lane");
        assert_delivery_rejection(
            &error,
            "delivery_pixel_format_depth_mismatch",
            "pixel_format",
            "yuv420p10le",
            "yuv420p",
        );
    }

    /// A build whose encoder does not offer the lane's format fails typed; it
    /// never silently falls back to the other depth (CC6 4.3).
    #[test]
    fn rejects_a_delivery_pixel_format_this_build_does_not_advertise() {
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        // `mpeg1video` is a real H.264-adjacent stand-in for "an encoder that
        // does not advertise this lane": it offers `yuv420p` only, so the
        // 10-bit lane is genuinely unavailable on it. No production path can
        // reach this encoder -- `validate_delivery_color` refuses every codec
        // but libx264 -- so this exercises the check itself.
        let codec = ffmpeg::encoder::find_by_name("mpeg1video")
            .expect("mpeg1video is part of every FFmpeg build");
        let error = checked_delivery_pixel_format(
            codec,
            DeliveryEncodeDepth::Ten,
            delivery_lane_pixel_format(DeliveryEncodeDepth::Ten),
        )
        .expect_err("an encoder without the lane's pixel format must fail, not fall back");
        let typed = delivery_error(&error);
        assert_eq!(typed.code(), "delivery_encoder_pixel_format_unavailable");
        assert_eq!(typed.field(), "pixel_format");
        assert_eq!(typed.allowed_values(), "yuv420p10le");
        assert!(
            typed
                .observed()
                .split(' ')
                .any(|format| format == "yuv420p"),
            "the refusal must report what the encoder does advertise: {}",
            typed.observed()
        );
        assert!(
            !typed.observed().contains("yuv420p10le"),
            "this stand-in encoder must genuinely lack the 10-bit lane: {}",
            typed.observed()
        );
        assert!(
            typed
                .recovery_action()
                .contains("never silently falls back")
        );
    }

    #[test]
    fn stamps_rgb_and_yuv_frames_with_their_explicit_ranges() {
        let mut rgba = ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, 2, 2);
        stamp_rgba_color(&mut rgba);
        assert_eq!(rgba.color_space(), ffmpeg::color::Space::RGB);
        assert_eq!(rgba.color_range(), ffmpeg::color::Range::JPEG);
        assert_eq!(rgba.color_primaries(), ffmpeg::color::Primaries::BT709);
        assert_eq!(
            rgba.color_transfer_characteristic(),
            ffmpeg::color::TransferCharacteristic::BT709
        );

        // Both delivery lanes carry the same colour description; only the
        // frame's pixel format differs (CC6 4.3).
        for depth in DeliveryEncodeDepth::ALL {
            let mut yuv = ffmpeg::frame::Video::new(delivery_lane_pixel_format(depth), 2, 2);
            stamp_delivery_yuv_color(&mut yuv);
            assert_eq!(pixel_format_name(yuv.format()), depth.pixel_format());
            assert_eq!(yuv.color_space(), ffmpeg::color::Space::BT709);
            assert_eq!(yuv.color_range(), ffmpeg::color::Range::MPEG);
            assert_eq!(yuv.color_primaries(), ffmpeg::color::Primaries::BT709);
            assert_eq!(
                yuv.color_transfer_characteristic(),
                ffmpeg::color::TransferCharacteristic::BT709
            );
        }
    }

    #[test]
    fn delivery_filter_converts_sixteen_bit_full_range_rgb_to_limited_yuv420p() {
        let resolution = (16_u32, 16_u32);

        // (a) Nominal white. `libswscale` reads 16-bit RGB on the `255 << 8`
        // scale, so the compositor's white must be DELIVERY_INTERMEDIATE_WHITE
        // and must land on legal white 235 exactly, on every sample. 65_535
        // would be read as *above* nominal white and quantize to 236.
        let white = encode_delivery_rgba16([1.0, 1.0, 1.0, 1.0]);
        assert_eq!(white, [DELIVERY_INTERMEDIATE_WHITE; 4]);
        let filtered_white =
            filter_flat_delivery_code(white, resolution, DeliveryEncodeDepth::Eight);
        assert_eq!(
            luma_codes(&filtered_white),
            BTreeSet::from([235]),
            "nominal white must convert to legal white on every luma sample"
        );

        // (b) Mid-gray at the compositor's single 16-bit quantization. An
        // 8-bit intermediate could not represent this code at all.
        // 46_056 / 65_280 = 0.705515, so limited luma is
        // 16 + 219 * 0.705515 = 170.51 and swscale's deterministic 8x8 ordered
        // dither splits a flat frame across exactly the two adjacent codes.
        let gray = encode_delivery_rgba16([0.5, 0.5, 0.5, 1.0]);
        assert_eq!(gray[0], 46_056);
        let filtered_gray = filter_flat_delivery_code(gray, resolution, DeliveryEncodeDepth::Eight);
        assert_eq!(
            luma_codes(&filtered_gray),
            BTreeSet::from([170, 171]),
            "mid-gray must land on the two codes straddling 170.51, nothing else"
        );

        // Neutral gray and neutral white are both exactly 128.0 in chroma, so
        // the *only* reason a neighbouring code appears is swscale's
        // deterministic ordered dither, which straddles the exact value: Cb
        // rounds up on part of the plane, Cr rounds down. Nothing may reach
        // 126 or 130.
        for frame in [&filtered_white, &filtered_gray] {
            assert_eq!(
                chroma_codes(frame, 1),
                BTreeSet::from([128, 129]),
                "neutral input must stay neutral in Cb"
            );
            assert_eq!(
                chroma_codes(frame, 2),
                BTreeSet::from([127, 128]),
                "neutral input must stay neutral in Cr"
            );
        }
    }

    /// The same conversion on the 10-bit lane (CC6 4.3/5.4).
    ///
    /// Two facts are asserted here that the 8-bit lane cannot assert:
    /// nominal white lands on legal white **940** exactly, and libswscale
    /// applies **no dither** on the 16-to-10-bit path, so every flat input --
    /// including mid-grey, which straddles two codes at 8 bits -- comes out as
    /// a *single* code. `sws_dither` and `accurate_rnd` are inert on this path,
    /// so this is a property of the build, not of an option we set.
    #[test]
    fn delivery_filter_converts_sixteen_bit_full_range_rgb_to_limited_yuv420p10le() {
        let resolution = (16_u32, 16_u32);

        // (a) Nominal white -> legal white at 10 bits: 64 + 876 = 940.
        let white = encode_delivery_rgba16([1.0, 1.0, 1.0, 1.0]);
        assert_eq!(white, [DELIVERY_INTERMEDIATE_WHITE; 4]);
        let filtered_white = filter_flat_delivery_code(white, resolution, DeliveryEncodeDepth::Ten);
        assert_eq!(
            luma_codes_10bit(&filtered_white),
            BTreeSet::from([940]),
            "nominal white must convert to legal 10-bit white on every luma sample"
        );

        // (b) Mid-gray, the same 16-bit intermediate code the 8-bit lane
        // splits across 170/171. 46_056 / 65_280 = 0.705515, so
        // 64 + 876 * 0.705515 = 682.03 -> a single code 682, because the
        // 16-to-10-bit path rounds and does not dither.
        let gray = encode_delivery_rgba16([0.5, 0.5, 0.5, 1.0]);
        assert_eq!(gray[0], 46_056);
        let filtered_gray = filter_flat_delivery_code(gray, resolution, DeliveryEncodeDepth::Ten);
        assert_eq!(
            luma_codes_10bit(&filtered_gray),
            BTreeSet::from([682]),
            "a flat 10-bit delivery patch must be a single luma code: this lane is undithered"
        );

        // Neutral stays neutral, and at 10 bits it is exactly neutral: 512 on
        // both chroma planes, with no straddling pair.
        for frame in [&filtered_white, &filtered_gray] {
            assert_eq!(
                chroma_codes_10bit(frame, 1),
                BTreeSet::from([512]),
                "neutral input must be exactly neutral in 10-bit Cb"
            );
            assert_eq!(
                chroma_codes_10bit(frame, 2),
                BTreeSet::from([512]),
                "neutral input must be exactly neutral in 10-bit Cr"
            );
        }

        // The 8-bit lane's dither is not a property the 10-bit lane inherits:
        // the same mid-grey is two codes there and one here.
        assert_eq!(
            luma_codes(&filter_flat_delivery_code(
                gray,
                resolution,
                DeliveryEncodeDepth::Eight
            )),
            BTreeSet::from([170, 171])
        );
    }

    /// Regression gate for the delivery intermediate's scale.
    ///
    /// A *rendered* white frame — the production compositor readback, not a
    /// hand-built raster — must survive the real export filter graph as legal
    /// white. When the intermediate was scaled to `65_535` every export encoded
    /// nominal white as 236, one code above legal white, on nearly every
    /// sample. This test fails on that scale.
    #[test]
    fn delivery_nominal_white_encodes_to_legal_white_through_the_export_filter() {
        // Rule 11.0.6: the panicking acquisition, never the skipping one. A
        // GPU-backed delivery fixture that reports `ok` without running is
        // indistinguishable from one that passed.
        let gpu = fallback_gpu().context();
        let compositor = Compositor::new(gpu);
        let resolution = (16_u32, 16_u32);
        let count = usize::try_from(resolution.0 * resolution.1).expect("raster size");
        let white_source = FrameTexture {
            width: resolution.0,
            height: resolution.1,
            rgba: Arc::new(std::iter::repeat_n(255_u8, count * 4).collect()),
        };
        let layer = CompositorLayer {
            frame: &white_source,
            effects: &[],
            transition: TransitionRenderParams::default(),
        };
        let delivery = ColorContext::sdr_rec709().delivery;
        let composed = compositor
            .render_delivery(resolution, std::slice::from_ref(&layer), &delivery)
            .expect("white delivery render");

        // The readback itself must already be on the intermediate's scale.
        let codes = composed
            .rgba64le
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| u16::from_le_bytes(*bytes))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            codes,
            BTreeSet::from([DELIVERY_INTERMEDIATE_WHITE]),
            "a rendered white frame must be nominal white in every channel"
        );

        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let mut filter = delivery_filter_graph(resolution, DeliveryEncodeDepth::Eight)
            .expect("delivery filter graph");
        let mut rgba =
            ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, resolution.0, resolution.1);
        stamp_rgba_color(&mut rgba);
        copy_rgba64_to_frame(&composed.rgba64le, &mut rgba).expect("RGBA64LE frame copy");
        let yuv = filter.run(&rgba).expect("delivery conversion");
        assert_eq!(
            luma_codes(&yuv),
            BTreeSet::from([235]),
            "every luma sample of a rendered white frame must be legal white 235"
        );

        // The same rendered raster on the 10-bit lane: legal white 940. The
        // buffer source consumed the frame above, so submit a fresh copy of
        // the same readback rather than the same buffer twice.
        let mut filter = delivery_filter_graph(resolution, DeliveryEncodeDepth::Ten)
            .expect("10-bit delivery filter graph");
        let mut rgba =
            ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, resolution.0, resolution.1);
        stamp_rgba_color(&mut rgba);
        copy_rgba64_to_frame(&composed.rgba64le, &mut rgba).expect("RGBA64LE frame copy");
        let yuv = filter.run(&rgba).expect("10-bit delivery conversion");
        assert_eq!(
            luma_codes_10bit(&yuv),
            BTreeSet::from([940]),
            "every luma sample of a rendered white frame must be legal 10-bit white 940"
        );
    }

    /// The 10-bit lane, end to end through the production export.
    ///
    /// The recipe is CC1's delivery-source pattern: a generated limited-range
    /// BT.709 source, the production `export_document`, then a re-probe of the
    /// written file. What is asserted here is the *lane*: the file must decode
    /// as `yuv420p10le` and probe as `Bt709` x 3 / `Limited` / ten bits. A
    /// silent 8-bit fallback -- the failure this lane exists to make
    /// impossible -- would fail both assertions.
    #[test]
    fn ten_bit_export_probes_as_rec709_limited_ten_bit_yuv420p10le() {
        // Rule 11.0.6: the panicking acquisition, never the skipping one. A
        // GPU-backed delivery fixture that reports `ok` without running is
        // indistinguishable from one that passed.
        let gpu = fallback_gpu().context();
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let directory = TempDirectory::new("cc6-ten-bit-delivery");
        let (width, height) = (32_u32, 16_u32);
        let (source_path, _source_bytes) = generate_delivery_source(&directory, width, height);
        let source_asset =
            probe_path(&source_path, AssetId(2)).expect("delivery source should probe");
        let document = simple_document(source_asset, (width, height));
        document
            .validate()
            .expect("the 10-bit delivery document should validate");

        let settings = DeliveryProfile::SourceMaster.export_settings(
            &document,
            DeliveryEncodeDepth::Ten,
            ExportCancellation::default(),
        );
        // The document keeps declaring the project's 8-bit delivery contract;
        // only the job's settings carry the 10-bit lane (CC6 4.1).
        assert_eq!(
            document.color_context.delivery.bit_depth,
            ColorBitDepth::Eight
        );
        assert_eq!(settings.delivery_color.bit_depth, ColorBitDepth::Ten);
        assert_eq!(settings.video_codec, DELIVERY_VIDEO_CODEC);

        let output_path = directory.path("cc6-ten-bit-export.mp4");
        let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
        export_document(
            &document,
            &output_path,
            &settings,
            &progress_tx,
            gpu.clone(),
        )
        .expect("the production export must write the 10-bit delivery lane");

        assert_eq!(decoded_video_pixel_format(&output_path), "yuv420p10le");
        // §4.3/R7: no `profile` option is ever set on either lane — the pixel
        // format alone selects High 10. Read back from the written SPS by the
        // pinned CLI, so "the depth reached the bitstream" is asserted rather
        // than inferred from the decoder's own format negotiation.
        assert_eq!(
            ffprobe_video_field(&output_path, "profile"),
            "High 10",
            "the 10-bit lane must write a High 10 bitstream"
        );
        let probed = probe_path(&output_path, AssetId(3))
            .expect("the 10-bit export should probe")
            .color_description;
        assert_eq!(probed.primaries, ColorPrimaries::Bt709);
        assert_eq!(probed.transfer, ColorTransfer::Bt709);
        assert_eq!(probed.matrix, ColorMatrix::Bt709);
        assert_eq!(probed.range, ColorRange::Limited);
        assert_eq!(
            probed.bit_depth,
            ColorBitDepth::Ten,
            "a 10-bit request that silently delivered 8 bits would land here"
        );
        assert_eq!(probed.provenance, ColorProvenance::StreamMetadata);

        // Same source, same renderer, 8-bit lane: the two lanes are genuinely
        // distinct files, so neither assertion above can pass by accident.
        let eight_bit = DeliveryProfile::SourceMaster.export_settings(
            &document,
            DeliveryEncodeDepth::Eight,
            ExportCancellation::default(),
        );
        let eight_bit_path = directory.path("cc6-eight-bit-export.mp4");
        export_document(&document, &eight_bit_path, &eight_bit, &progress_tx, gpu)
            .expect("the production export must still write the 8-bit delivery lane");
        assert_eq!(decoded_video_pixel_format(&eight_bit_path), "yuv420p");
        // The control: the same source, the same renderer, and a plain High
        // bitstream. A 10-bit request that silently fell back to eight bits
        // would make these two profiles equal.
        assert_eq!(
            ffprobe_video_field(&eight_bit_path, "profile"),
            "High",
            "the 8-bit lane must write a High bitstream"
        );
        assert_ne!(
            ffprobe_video_field(&output_path, "profile"),
            ffprobe_video_field(&eight_bit_path, "profile")
        );
        assert_eq!(
            probe_path(&eight_bit_path, AssetId(4))
                .expect("the 8-bit export should probe")
                .color_description
                .bit_depth,
            ColorBitDepth::Eight
        );
    }

    #[test]
    fn delivery_intermediate_is_the_sixteen_bit_full_range_rgba_contract() {
        // `libavfilter` only warns when an input frame's pixel format differs
        // from the configured buffer source, so the export path's single
        // quantization depends on this constant and the compositor readback
        // agreeing. Keep them pinned together.
        assert_eq!(DELIVERY_INTERMEDIATE_PIXEL, ffmpeg::format::Pixel::RGBA64LE);
        assert_eq!(DELIVERY_INTERMEDIATE_BYTES_PER_PIXEL, 8);
        // The intermediate's nominal white is swscale's 16-bit RGB white
        // (`255 << 8`), not `u16::MAX`: this graph feeds `scale` with
        // `in_range=jpeg`, and swscale maps 65_535 to *above* nominal white,
        // which encodes to limited luma 236 instead of legal white 235.
        assert_eq!(DELIVERY_INTERMEDIATE_WHITE, 65_280);
        assert_ne!(DELIVERY_INTERMEDIATE_WHITE, u16::MAX);
        let frame = ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, 16, 16);
        assert!(frame.stride(0) >= 16 * DELIVERY_INTERMEDIATE_BYTES_PER_PIXEL);
    }

    /// AU3 §7 item A11 (§3.8): a family window drops `skip` frames, then
    /// passes exactly `remaining` frames, splitting the chunks that straddle
    /// either boundary and never handing the observer a frame twice.
    #[test]
    fn a_family_window_splits_chunks_and_feeds_exactly_the_kept_frames() {
        let channels = 2_usize;
        let chunk_frames = 1_024_usize;
        let total_frames = 6 * chunk_frames;
        // Each sample encodes its frame index so the fed frames are checkable.
        #[allow(clippy::cast_precision_loss)]
        let programme = (0..total_frames)
            .flat_map(|frame| [frame as f32, -(frame as f32)])
            .collect::<Vec<_>>();
        let skip = 1_500_usize;
        let kept = 2_000_usize;
        let mut window = FamilyWindow::new(skip, kept);
        let mut fed = Vec::new();
        let mut split_chunks = 0_usize;
        for chunk in programme.chunks(chunk_frames * channels) {
            let taken = window.take(chunk, channels);
            if !taken.is_empty() && taken.len() != chunk.len() {
                split_chunks += 1;
            }
            fed.extend_from_slice(taken);
        }
        assert_eq!(
            fed.len(),
            kept * channels,
            "exactly T frames reach the observer"
        );
        assert_eq!(
            split_chunks, 2,
            "the chunks straddling both boundaries are split"
        );
        #[allow(clippy::cast_precision_loss)]
        for (index, frame) in fed.chunks_exact(channels).enumerate() {
            let expected = (skip + index) as f32;
            assert_eq!(frame, [expected, -expected], "fed frame {index}");
        }
        // Exhausted: nothing more is fed.
        assert!(window.take(&programme[..channels * 8], channels).is_empty());
        // A window with no skip and unbounded remaining passes chunks whole.
        let mut whole = FamilyWindow::new(0, usize::MAX);
        assert_eq!(
            whole.take(&programme[..channels * 8], channels).len(),
            channels * 8
        );
    }

    // ---- AU3 §5.6: the export's loudness normalization step ---------------

    /// AU3 §3.2/N1: the contract's calibration frequency. The Tech 3341 tone
    /// is "1 kHz per the standard; 997 Hz in this contract's calibration
    /// pins", and only at 997 Hz does the K-weighting gain cancel the −0.691
    /// offset exactly, which is what makes §5.6's `gain == 1_600` an equality
    /// rather than a tolerance.
    const CALIBRATION_HERTZ: f64 = 997.0;

    /// A stereo calibration sine at `amplitude`, at the export lane's rate.
    #[allow(clippy::cast_precision_loss)]
    fn normalization_tone(amplitude: f32, frames: usize) -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames * usize::from(AUDIO_CHANNELS));
        for frame in 0..frames {
            let phase =
                std::f64::consts::TAU * CALIBRATION_HERTZ * frame as f64 / f64::from(AUDIO_RATE);
            #[allow(clippy::cast_possible_truncation)]
            let value = amplitude * phase.sin() as f32;
            samples.push(value);
            samples.push(value);
        }
        samples
    }

    /// A stereo calibration sine whose integrated loudness is
    /// `lufs_hundredths`.
    ///
    /// AU3 §3.2/N1's calibration: the K-weighting gain at 997 Hz cancels the
    /// −0.691 offset and the two channels' mean-square halves sum to one, so
    /// a stereo sine of amplitude `A` reads `20·log10(A)` LUFS.
    fn normalization_tone_at_lufs(lufs_hundredths: i32, frames: usize) -> Vec<f32> {
        #[allow(clippy::cast_possible_truncation)]
        let amplitude = 10f64.powf(f64::from(lufs_hundredths) / 2_000.0) as f32;
        normalization_tone(amplitude, frames)
    }

    /// The step reads only `cancellation` from its settings (it is handed the
    /// target directly), so a minimal settings value stands in for a job's.
    fn normalization_settings() -> ExportSettings {
        ExportSettings {
            fps: Rational::new(25, 1).unwrap(),
            resolution: (64, 64),
            delivery_color: ColorContext::sdr_rec709().delivery,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 1,
            audio_bitrate: 1,
            loudness_normalization: None,
            cancellation: ExportCancellation::default(),
        }
    }

    /// AU3 §7 B5: the §5.6 pin — a −30 LUFS tone to the streaming target is
    /// +16 dB, one limiter pass, and the limiter is a bit-exact identity
    /// because the gained tone sits 11 dB under the node's ceiling.
    #[test]
    fn au3_normalizing_a_quiet_tone_is_one_gain_and_an_identity_limiter_pass() {
        let frames = 5 * AUDIO_RATE as usize;
        let mut mix = normalization_tone_at_lufs(-3_000, frames);
        let mut gained = mix.clone();
        apply_gain(&mut gained, 1_600);
        let target = kinewright_core::STREAMING_PLATFORM_TARGET;
        let report = normalize_master(&mut mix, target, &normalization_settings())
            .expect("a five-second tone must normalize");
        println!(
            "AU3_NORMALIZE case=quiet_tone before={:?} after={:?} gain={} passes={} reduction={} on_target={}",
            report.before.integrated_lufs_hundredths,
            report.after.integrated_lufs_hundredths,
            report.applied_gain_hundredths,
            report.limiter_passes,
            report.peak_reduction_hundredths,
            report.on_target,
        );
        assert_eq!(report.skipped_reason, None);
        assert_eq!(report.before.integrated_lufs_hundredths, Some(-3_000));
        assert_eq!(report.applied_gain_hundredths, 1_600);
        assert_eq!(report.limiter_passes, 1);
        assert_eq!(
            report.peak_reduction_hundredths, 0,
            "an under-ceiling programme leaves the true peak exactly where the gain put it"
        );
        assert!(report.on_target, "{report:?}");
        let integrated = report
            .after
            .integrated_lufs_hundredths
            .expect("a gained tone is not silent");
        assert!(
            (integrated + 1_400).abs() <= 5,
            "the normalized tone must land on -14.00 LUFS: {integrated}"
        );
        assert_eq!(
            mix, gained,
            "AU2 A27: with every sample under the node's ceiling the limiter is a bit-exact \
             identity, so the encoded master is the gained mix and nothing else"
        );
    }

    /// AU3 §7 B5: the impulse programme — the limiter has real work, so the
    /// step takes its one corrective pass and reports the reduction it made.
    #[test]
    fn au3_normalizing_an_impulse_programme_takes_a_second_corrective_pass() {
        let frames = 5 * AUDIO_RATE as usize;
        let mut mix = normalization_tone_at_lufs(-3_000, frames);
        // A 0 dBFS impulse every 500 ms, replacing the tone sample rather than
        // adding to it so the fixture's peak is exactly full scale.
        for frame in (0..frames).step_by(AUDIO_RATE as usize / 2) {
            mix[frame * usize::from(AUDIO_CHANNELS)] = 1.0;
            mix[frame * usize::from(AUDIO_CHANNELS) + 1] = 1.0;
        }
        let target = kinewright_core::STREAMING_PLATFORM_TARGET;
        let report = normalize_master(&mut mix, target, &normalization_settings())
            .expect("the impulse programme must normalize");
        println!(
            "AU3_NORMALIZE case=impulse before={:?} before_true_peak={:?} after={:?} after_true_peak={:?} gain={} passes={} reduction={} on_target={}",
            report.before.integrated_lufs_hundredths,
            report.before.true_peak_dbtp_hundredths,
            report.after.integrated_lufs_hundredths,
            report.after.true_peak_dbtp_hundredths,
            report.applied_gain_hundredths,
            report.limiter_passes,
            report.peak_reduction_hundredths,
            report.on_target,
        );
        assert_eq!(report.skipped_reason, None);
        assert_eq!(report.limiter_passes, 2, "{report:?}");
        assert!(report.on_target, "{report:?}");
        assert!(
            report.peak_reduction_hundredths > 0,
            "the limiter pulled the impulses down, so the reduction cannot be zero: {report:?}"
        );
        let ceiling = target.true_peak_ceiling_dbtp_hundredths
            - kinewright_core::LOSSY_CODEC_TRUE_PEAK_HEADROOM_HUNDREDTHS;
        let after_peak = report
            .after
            .true_peak_dbtp_hundredths
            .expect("a limited programme is not silent");
        assert!(
            after_peak <= ceiling + 5,
            "the pre-encode true peak must sit at the node's {ceiling} hundredths ceiling: \
             {after_peak}"
        );
        let first_gain = 1_600;
        assert!(
            report.applied_gain_hundredths > first_gain,
            "`applied_gain_hundredths` is the sum of the two passes' gains: {report:?}"
        );
    }

    /// AU3 §7 B5 / N4 Q3: the three skips are distinct, and a skip is not a
    /// failure — the report carries both measurements and the export goes on.
    #[test]
    fn au3_normalization_names_its_three_skips_distinctly() {
        let target = kinewright_core::STREAMING_PLATFORM_TARGET;
        let settings = normalization_settings();

        // 300 ms: no complete 400 ms gating block exists, which is a different
        // fact from silence and says so.
        let mut short = normalization_tone_at_lufs(-2_000, 3 * AUDIO_RATE as usize / 10);
        let short_report = normalize_master(&mut short, target, &settings)
            .expect("a short programme is not an error");
        assert_eq!(
            short_report.skipped_reason.as_deref(),
            Some(NORMALIZATION_SHORT_PROGRAMME_REASON)
        );
        assert_eq!(
            short_report.skipped_reason.as_deref(),
            Some("shorter than one 400 ms gating block")
        );

        // Digital silence, well over one gating block.
        let mut silent = vec![0.0_f32; 5 * AUDIO_RATE as usize * usize::from(AUDIO_CHANNELS)];
        let silent_report =
            normalize_master(&mut silent, target, &settings).expect("silence is not an error");
        assert_eq!(
            silent_report.skipped_reason.as_deref(),
            Some(NORMALIZATION_SILENT_REASON)
        );
        assert_eq!(silent_report.skipped_reason.as_deref(), Some("silent"));
        assert!(silent.iter().all(|sample| *sample == 0.0));

        // A +46 dB request: outside the planner's -6000..=3600 guard.
        let mut faint = normalization_tone_at_lufs(-6_000, 5 * AUDIO_RATE as usize);
        let faint_before = faint.clone();
        let faint_report = normalize_master(&mut faint, target, &settings)
            .expect("an impossible gain is not an error");
        assert_eq!(
            faint_report.skipped_reason.as_deref(),
            Some("required gain 4600 hundredths exceeds -6000..=3600"),
            "{faint_report:?}"
        );
        assert_eq!(
            faint, faint_before,
            "a skipped step leaves the master alone"
        );

        for report in [&short_report, &silent_report, &faint_report] {
            assert_eq!(report.before, report.after, "{report:?}");
            assert_eq!(report.applied_gain_hundredths, 0, "{report:?}");
            assert_eq!(report.limiter_passes, 0, "{report:?}");
            assert_eq!(report.peak_reduction_hundredths, 0, "{report:?}");
            assert!(!report.on_target, "{report:?}");
        }
        let reasons = [&short_report, &silent_report, &faint_report]
            .iter()
            .filter_map(|report| report.skipped_reason.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(reasons.len(), 3, "the three skip reasons must be distinct");
    }

    /// AU3 §7 B7: every §2.3 target is a −1 dBTP ceiling, so the node is
    /// handed −30 tenth-dB, inside its descriptor's `−120..=0` range.
    #[test]
    fn au3_the_delivery_limiter_ceiling_is_minus_thirty_tenth_decibels_for_every_target() {
        for profile in [
            DeliveryProfile::SourceMaster,
            DeliveryProfile::Youtube1080p,
            DeliveryProfile::VerticalShort,
            DeliveryProfile::SquareSocial,
        ] {
            let ceiling = delivery_limiter_ceiling_tenth_db(profile.loudness_target())
                .expect("every shipped profile's ceiling is inside the node's range");
            assert_eq!(ceiling, -30, "{profile:?}");
            assert!((-120..=0).contains(&ceiling), "{profile:?}");
        }
        for target in [
            kinewright_core::EBU_R128_PROGRAMME_TARGET,
            kinewright_core::STREAMING_PLATFORM_TARGET,
        ] {
            assert_eq!(
                delivery_limiter_ceiling_tenth_db(target)
                    .expect("a shipped target's ceiling is inside the node's range"),
                -30
            );
        }
    }

    /// AU3 §7 B7 / review F3: `LoudnessTarget` is a public `Deserialize`
    /// struct with public fields, so a hand-built target can name a ceiling
    /// the node's descriptor does not accept. It is refused by name, never
    /// clamped into range and limited to a ceiling nobody asked for.
    #[test]
    fn au3_a_ceiling_outside_the_limiter_nodes_range_is_refused_not_clamped() {
        let target = LoudnessTarget {
            integrated_lufs_hundredths: -1_400,
            tolerance_lu_hundredths: 100,
            true_peak_ceiling_dbtp_hundredths: -20_000,
            loudness_range_max_lu_hundredths: None,
        };
        let error = delivery_limiter_ceiling_tenth_db(target)
            .expect_err("a −200 dBTP ceiling is outside the descriptor's −120..=0 range");
        let message = error.to_string();
        assert!(
            message.contains("-2020") && message.contains("-120..=0"),
            "the refusal must name the offending ceiling and the range it is outside: {message}"
        );

        // And the step itself refuses rather than normalizing against a
        // ceiling the caller did not ask for.
        let mut mix = normalization_tone_at_lufs(-2_000, 5 * AUDIO_RATE as usize);
        let before = mix.clone();
        let error = normalize_master(&mut mix, target, &normalization_settings())
            .expect_err("the out-of-range ceiling must propagate out of the step");
        assert!(
            error.to_string().contains("delivery limiter ceiling"),
            "{error}"
        );
        assert_eq!(
            mix, before,
            "a refused normalization leaves the master untouched"
        );
    }

    /// AU3 §5.6: a master already on target takes a gain of exactly zero,
    /// which is inside the inclusive guard, so the limiter still runs and the
    /// report says one pass — the boundary between the skip paths (zero
    /// passes) and the acting path.
    #[test]
    fn au3_a_master_already_on_target_still_takes_one_limiter_pass() {
        let mut mix = normalization_tone_at_lufs(-1_400, 5 * AUDIO_RATE as usize);
        let target = kinewright_core::STREAMING_PLATFORM_TARGET;
        let report = normalize_master(&mut mix, target, &normalization_settings())
            .expect("an on-target tone must normalize");
        assert_eq!(report.skipped_reason, None);
        assert_eq!(report.applied_gain_hundredths, 0);
        assert_eq!(report.limiter_passes, 1);
        assert_eq!(report.peak_reduction_hundredths, 0);
        assert!(report.on_target, "{report:?}");
    }
}
