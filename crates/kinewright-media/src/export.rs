use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    CC8_HDR_DELIVERY_X264_PARAMS, CC8_SDR_DELIVERY_X264_PARAMS, ColorDescription, ColorMatrix,
    DELIVERY_BIT_DEPTH_ALLOWED, DeliveryColorError, DeliveryColorMismatch, DeliveryEncodeDepth,
    DeliveryLane, Document, ExportProgress, ExportSettings, FrameRounding, MediaError,
    ProgressSink, TimeCode, TrackId, delivery_color_mismatches, map_frames_with_rounding,
};

use crate::{
    audio::{AudioMixProcessor, ClipAudioShaping, decode_audio_range, limit_audio_mix},
    clock::frame_to_samples,
    compositor::GpuContext,
    decode::backend,
    lut_store::LutLibrary,
    render::FrameRenderer,
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
) -> Result<(), MediaError> {
    export_document_inner(
        document,
        out,
        settings,
        progress,
        gpu,
        library,
        VideoPacketDuration::OneFrame,
    )
    .map(|_terms| ())
}

/// Re-run the production export and report the delivery lane terms it used
/// (test-only, CC8 §9.1 fixture 6).
///
/// The same call [`export_document_with_luts`] makes, with the same arguments
/// and the same [`VideoPacketDuration::OneFrame`]: the only difference is that
/// the [`DeliveryLaneTerms`] the production path always builds is returned here
/// instead of discarded. It changes no encoder option, no filter-graph string,
/// and no written byte -- `cc8_sdr_regression_byte_equality_gate` exports
/// through *both* arities and asserts the two files are byte-identical, which
/// is what makes that claim a measurement rather than a comment.
///
/// The seam is observational, in the manner of
/// [`export_document_with_zero_packet_durations`] above and of
/// `Analysis::working_proof_for_document`: the fixture reads what production
/// did rather than re-deriving it, so a term it asserts is the term `FFmpeg`
/// received.
#[cfg(test)]
pub(crate) fn export_document_capturing_delivery_terms(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
) -> Result<DeliveryLaneTerms, MediaError> {
    export_document_inner(
        document,
        out,
        settings,
        progress,
        gpu,
        Arc::new(LutLibrary::default()),
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
    .map(|_terms| ())
}

fn export_document_inner(
    document: &Document,
    out: &Path,
    settings: &ExportSettings,
    progress: &ProgressSink,
    gpu: GpuContext,
    library: Arc<LutLibrary>,
    packet_duration: VideoPacketDuration,
) -> Result<DeliveryLaneTerms, MediaError> {
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
    let terms = match result {
        Ok(terms) => terms,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    replace_output(&temporary, out)?;
    Ok(terms)
}

/// The colour terms of one `FFmpeg` frame, as `FFmpeg` itself names them.
///
/// Read back from the frame after it is stamped, never recomputed from the
/// stamping function's arguments, so this records what the frame carried into
/// the filter graph and the encoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameColorTerms {
    pub(crate) pixel_format: String,
    pub(crate) space: String,
    pub(crate) range: String,
    pub(crate) primaries: String,
    pub(crate) transfer: String,
}

impl FrameColorTerms {
    fn of(frame: &ffmpeg::frame::Video) -> Self {
        Self {
            pixel_format: pixel_format_name(frame.format()).to_owned(),
            space: frame
                .color_space()
                .name()
                .unwrap_or("unspecified")
                .to_owned(),
            range: frame
                .color_range()
                .name()
                .unwrap_or("unspecified")
                .to_owned(),
            primaries: frame
                .color_primaries()
                .name()
                .unwrap_or("unspecified")
                .to_owned(),
            transfer: frame
                .color_transfer_characteristic()
                .name()
                .unwrap_or("unspecified")
                .to_owned(),
        }
    }
}

/// Every delivery colour term one export actually handed to `FFmpeg`.
///
/// CC8 §0.4 surveys six hard-coded BT.709 literals on the export path -- items
/// 3, 4, 6 and 8, at the pre-CC8 `export.rs:216-217`, `:425`, `:529`, `:626`
/// and `:638` -- that §5.2 turns into lane-derived values, and §12 names the
/// danger: "a lane-derivation bug is invisible on the HDR lane and catastrophic
/// on the SDR one." This record is
/// what makes that bug visible on the SDR lane *by construction*: every field
/// is read back from the object that received the term -- the opened encoder,
/// the options dictionary handed to `open_as_with`, the configured filter
/// graph, and the two stamped frames -- so the value recorded here is the value
/// the encode used, not a second transcription of it that could stay right
/// while production went wrong.
///
/// Building it is pure observation: no field is consulted by any production
/// decision, and [`export_document_with_luts`] discards the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryLaneTerms {
    /// `AVCodecContext.pix_fmt` of the opened encoder.
    pub(crate) encoder_pixel_format: String,
    /// `AVCodecContext.colorspace` of the opened encoder -- §0.4 item 3's
    /// `set_colorspace`.
    pub(crate) encoder_colorspace: String,
    /// `AVCodecContext.color_range` of the opened encoder -- §0.4 item 3's
    /// `set_color_range`.
    pub(crate) encoder_color_range: String,
    /// Every private encoder option, including `x264-params` (§0.4 item 4's
    /// [`DELIVERY_X264_PARAMS`]) and `preset`, as the dictionary held them at
    /// `open_as_with`.
    pub(crate) encoder_options: BTreeMap<String, String>,
    /// The `buffer` source's argument string.
    pub(crate) buffer_args: String,
    /// The `scale` filter's argument string, which carries
    /// [`DELIVERY_SCALER_FLAGS`] and §0.4 item 6's `out_color_matrix`.
    pub(crate) scale_args: String,
    /// The `format` node's argument string.
    pub(crate) format_args: String,
    /// The pixel format the graph is required to produce.
    pub(crate) graph_pixel_format: String,
    /// The stamped RGBA64LE delivery intermediate -- §0.4 item 8's first
    /// `set_color_primaries`, in [`stamp_rgba_color`].
    pub(crate) intermediate_frame: FrameColorTerms,
    /// The stamped `Y'CbCr` delivery frame -- §0.4 item 8's second
    /// `set_color_primaries`, in [`stamp_delivery_yuv_color`].
    pub(crate) delivery_frame: FrameColorTerms,
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
) -> Result<DeliveryLaneTerms, MediaError> {
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
    // CC8 §5.2 clause 1: "The encoder colourspace and range **must** be
    // selected from the delivery `ColorDescription`, never from a literal."
    // `validate_settings` has already refused every description outside the two
    // lanes, so these two assignments cannot mislabel a target; what changed is
    // that the values are now the lane's rather than BT.709's unconditionally
    // (§0.4 item 3). Pixel transforms remain a CC1 concern.
    let delivery_lane = DeliveryLane::for_description(&settings.delivery_color);
    video_encoder.set_colorspace(encoder_color_space(delivery_lane));
    video_encoder.set_color_range(encoder_color_range(delivery_lane));
    let mut video_options = ffmpeg::Dictionary::new();
    if settings.video_codec == DELIVERY_VIDEO_CODEC {
        video_options.set("preset", "medium");
        // FFmpeg's generic codec-context colour fields do not reliably carry
        // primaries and transfer through libx264's SPS. These x264 options
        // are required for the tags to survive a post-export re-probe.
        //
        // CC8 §5.2 item 2 makes the string a function of the lane; it is
        // identical at 8 and 10 bits on the SDR lane (CC6 4.3) and is the
        // BT.2020/HLG string §10 step 1's precondition proved on the HDR lane.
        // `range=tv` is *not* an x264 parameter in x264 core 165 -- it is
        // parsed and discarded -- and `profile=high10` is not set either: the
        // pixel format selects High 10, measured byte-identical with and
        // without it on the pinned build.
        video_options.set("x264-params", delivery_x264_params(delivery_lane));
    }
    // Observed, not recomputed (CC8 §9.1 fixture 6): the dictionary is disowned
    // by `open_as_with`, so the terms are read out of it here, immediately
    // before libavcodec receives exactly these entries.
    let encoder_options = video_options
        .iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect::<BTreeMap<String, String>>();
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
    let mut delivery_filter =
        delivery_filter_graph(settings.resolution, delivery_depth, delivery_lane)?;
    let mut intermediate_frame_terms: Option<FrameColorTerms> = None;
    let mut delivery_frame_terms: Option<FrameColorTerms> = None;
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
        stamp_rgba_color(&mut rgba, delivery_lane);
        if intermediate_frame_terms.is_none() {
            intermediate_frame_terms = Some(FrameColorTerms::of(&rgba));
        }
        copy_rgba64_to_frame(&composed.rgba64le, &mut rgba)?;
        let mut yuv = delivery_filter.run(&rgba)?;
        stamp_delivery_yuv_color(&mut yuv, delivery_lane);
        if delivery_frame_terms.is_none() {
            delivery_frame_terms = Some(FrameColorTerms::of(&yuv));
        }
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
    let missing_frame_terms =
        || MediaError::Backend("export wrote no video frame to observe".to_owned());
    Ok(DeliveryLaneTerms {
        encoder_pixel_format: pixel_format_name(video_encoder.format()).to_owned(),
        encoder_colorspace: video_encoder
            .colorspace()
            .name()
            .unwrap_or("unspecified")
            .to_owned(),
        encoder_color_range: video_encoder
            .color_range()
            .name()
            .unwrap_or("unspecified")
            .to_owned(),
        encoder_options,
        buffer_args: delivery_filter.buffer_args.clone(),
        scale_args: delivery_filter.scale_args.clone(),
        format_args: delivery_filter.format_args.clone(),
        graph_pixel_format: pixel_format_name(delivery_filter.pixel_format).to_owned(),
        intermediate_frame: intermediate_frame_terms.ok_or_else(missing_frame_terms)?,
        delivery_frame: delivery_frame_terms.ok_or_else(missing_frame_terms)?,
    })
}

struct DeliveryFilter {
    graph: ffmpeg::filter::Graph,
    /// The lane's pixel format, asserted on every frame the graph produces.
    pixel_format: ffmpeg::format::Pixel,
    /// The three argument strings this graph was configured with, kept
    /// verbatim so [`DeliveryLaneTerms`] reports the strings `libavfilter`
    /// parsed rather than a second formatting of the same fields.
    buffer_args: String,
    scale_args: String,
    format_args: String,
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
    lane: DeliveryLane,
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
    // CC8 §5.2 clause 3 / §0.4 item 6: `out_color_matrix` is lane-derived from
    // the same single source of truth as the encoder's colourspace, so the
    // codec context and the filter graph cannot claim different matrices. The
    // spelling is `vf_scale`'s own vocabulary and not `buffersrc`'s -- `bt2020`
    // there is `AVCOL_SPC_BT2020_NCL` -- which is why `decode.rs`'s
    // `managed_scale_color_matrix` is the one place that names it. Nothing else
    // in this string moves: `DELIVERY_SCALER_FLAGS` and the two range terms are
    // §5.1's "unchanged and not re-measured".
    let scale_args = format!(
        "w={}:h={}:flags={DELIVERY_SCALER_FLAGS}:in_range=jpeg:out_range=mpeg:out_color_matrix={}",
        resolution.0,
        resolution.1,
        delivery_scale_color_matrix(lane),
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
        buffer_args: args,
        scale_args,
        format_args,
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

/// The x264 parameter string one delivery lane encodes with.
///
/// CC8 §5.2 item 2: "`DELIVERY_X264_PARAMS` becomes a function of the lane. For
/// this lane: `colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc`.
/// The SDR lanes' string is **byte-identical to today's**, and a fixture
/// asserts that." Both strings are the authority module's
/// ([`CC8_SDR_DELIVERY_X264_PARAMS`], [`CC8_HDR_DELIVERY_X264_PARAMS`]) rather
/// than literals here, so §10 step 1's proven HDR string and the string this
/// export hands libx264 are the same object.
///
/// The SDR string stays identical at 8 and 10 bits (CC6 4.3). `range=tv` is
/// deliberately absent on both lanes: it is not an x264 parameter in x264 core
/// 165, and `set_color_range(Range::MPEG)` on the codec context is measured to
/// reach the SPS on its own.
const fn delivery_x264_params(lane: DeliveryLane) -> &'static str {
    match lane {
        DeliveryLane::SdrRec709 => CC8_SDR_DELIVERY_X264_PARAMS,
        DeliveryLane::HdrHlgRec2020 => CC8_HDR_DELIVERY_X264_PARAMS,
    }
}

/// The encoder colourspace one delivery lane declares -- CC8 §0.4 item 3's
/// first `set_colorspace`, now lane-derived (§5.2 clause 1).
const fn encoder_color_space(lane: DeliveryLane) -> ffmpeg::color::Space {
    match lane {
        DeliveryLane::SdrRec709 => ffmpeg::color::Space::BT709,
        DeliveryLane::HdrHlgRec2020 => ffmpeg::color::Space::BT2020NCL,
    }
}

/// The encoder colour range one delivery lane declares -- CC8 §0.4 item 3's
/// `set_color_range`.
///
/// Both lanes are limited range: §5.1's `Range | limited` row, and "**Full-range
/// HDR delivery is rejected** with a typed reason, as full-range SDR already
/// is." It is still written as a function of the lane rather than as the
/// literal it was, because §5.2 clause 1 requires the *selection* to come from
/// the description -- a later lane that differed here would otherwise inherit
/// a value nobody chose for it.
const fn encoder_color_range(lane: DeliveryLane) -> ffmpeg::color::Range {
    match lane {
        DeliveryLane::SdrRec709 | DeliveryLane::HdrHlgRec2020 => ffmpeg::color::Range::MPEG,
    }
}

/// The `scale` filter's `out_color_matrix` for one delivery lane -- CC8 §0.4
/// item 6, at the pre-CC8 `export.rs:425`.
///
/// Derived through `decode::managed_scale_color_matrix` from the lane's own
/// `ColorMatrix`, so the export side and the decode side cannot spell the same
/// matrix two ways; that function's doc comment records why `vf_scale`'s
/// vocabulary differs from `buffersrc`'s.
fn delivery_scale_color_matrix(lane: DeliveryLane) -> &'static str {
    crate::decode::managed_scale_color_matrix(&ColorDescription {
        matrix: delivery_lane_color_matrix(lane),
        ..ColorDescription::default()
    })
}

/// The `ColorMatrix` of one delivery lane.
const fn delivery_lane_color_matrix(lane: DeliveryLane) -> ColorMatrix {
    match lane {
        DeliveryLane::SdrRec709 => ColorMatrix::Bt709,
        DeliveryLane::HdrHlgRec2020 => ColorMatrix::Bt2020Ncl,
    }
}

/// The `FFmpeg` colour terms one delivery lane stamps on its frames -- CC8 §0.4
/// item 8's two `set_color_primaries` call sites, and the space and transfer
/// that travel with them.
const fn delivery_lane_frame_terms(
    lane: DeliveryLane,
) -> (
    ffmpeg::color::Primaries,
    ffmpeg::color::TransferCharacteristic,
) {
    match lane {
        DeliveryLane::SdrRec709 => (
            ffmpeg::color::Primaries::BT709,
            ffmpeg::color::TransferCharacteristic::BT709,
        ),
        DeliveryLane::HdrHlgRec2020 => (
            ffmpeg::color::Primaries::BT2020,
            ffmpeg::color::TransferCharacteristic::ARIB_STD_B67,
        ),
    }
}

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

/// Stamp the full-range RGB intermediate produced by the compositor -- CC8 §0.4
/// item 8's first `set_color_primaries`, now lane-derived (§5.2 clause 3).
///
/// RGB has identity matrix coefficients and full-range samples on every lane,
/// so the space and range are constant. The primaries and transfer are the
/// **lane's**, because that is what the samples in this buffer actually are:
/// the compositor has already applied the lane's delivery encode, so an SDR
/// export hands `libavfilter` BT.709-coded BT.709-primaried values and an HDR
/// export hands it HLG-coded Rec.2020-primaried ones. Stamping BT.709 on the
/// second would describe the buffer wrongly.
fn stamp_rgba_color(frame: &mut ffmpeg::frame::Video, lane: DeliveryLane) {
    let (primaries, transfer) = delivery_lane_frame_terms(lane);
    frame.set_color_space(ffmpeg::color::Space::RGB);
    frame.set_color_range(ffmpeg::color::Range::JPEG);
    frame.set_color_primaries(primaries);
    frame.set_color_transfer_characteristic(transfer);
}

/// Stamp the limited-range `Y'CbCr` delivery frame -- CC8 §0.4 item 8's second
/// `set_color_primaries`, now lane-derived (§5.2 clause 3).
///
/// Within the SDR lane this is still identical at both depths: the *depth*
/// changes the frame's pixel format (`yuv420p` or `yuv420p10le`), never its
/// colour description (CC6 4.3). What changes it is the **lane**.
fn stamp_delivery_yuv_color(frame: &mut ffmpeg::frame::Video, lane: DeliveryLane) {
    let (primaries, transfer) = delivery_lane_frame_terms(lane);
    frame.set_color_space(encoder_color_space(lane));
    frame.set_color_range(encoder_color_range(lane));
    frame.set_color_primaries(primaries);
    frame.set_color_transfer_characteristic(transfer);
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

/// The current encoder/scaler path supports exactly two explicit delivery
/// contracts, one per [`DeliveryLane`]:
///
/// * **8-bit or 10-bit SDR Rec.709** (BT.709 primaries, transfer, and matrix;
///   limited range; D65) in limited-range YUV420P/YUV420P10LE; and
/// * **CC8 §5.1's HDR lane** — `bt2020` primaries, `arib_std_b67` transfer,
///   `bt2020_ncl` matrix, limited range, D65, 10-bit — in YUV420P10LE.
///
/// Both additionally require nonzero confidence and `application_default` or
/// `user_override` provenance. Keep this gate in front of encoder setup so an
/// unknown or future colour description cannot be mislabeled with either lane's
/// tags.
///
/// Rejections are typed (CC6 4.2): every one carries `code`, `field`,
/// `observed`, `allowed`, and a recovery action, so an agent or a UI never has
/// to parse a sentence to learn which field was wrong.
pub(crate) fn validate_delivery_color(settings: &ExportSettings) -> Result<(), MediaError> {
    if settings.video_codec != DELIVERY_VIDEO_CODEC {
        return Err(DeliveryColorError::UnsupportedCodec {
            observed: settings.video_codec.clone(),
            allowed: DELIVERY_VIDEO_CODEC,
        }
        .into());
    }
    validate_delivery_description(&settings.delivery_color)
}

/// Gate one delivery colour description against the lane it selects (CC8 §5.2
/// clause 1, §5.3).
///
/// The accepted set is Core's, never a second transcription of it: the fields
/// and their allowed values come from `delivery_color_mismatches`, so this gate
/// and `delivery_conformance` cannot disagree -- including on
/// [`kinewright_core::ColorBitDepth`]'s canonical equality, which makes
/// `Integer(8)`/`Integer(10)` the same declared depths as `Eight`/`Ten`.
///
/// The first mismatch in Core's fixed check order is the reported one; a caller
/// that wants all of them calls `delivery_color_mismatches` directly.
pub(crate) fn validate_delivery_description(color: &ColorDescription) -> Result<(), MediaError> {
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
        let mut filter = delivery_filter_graph(resolution, depth, DeliveryLane::SdrRec709)
            .expect("delivery filter graph");
        let count = usize::try_from(resolution.0 * resolution.1).expect("raster size");
        let pixels = std::iter::repeat_n(code, count)
            .flatten()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut rgba =
            ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, resolution.0, resolution.1);
        stamp_rgba_color(&mut rgba, DeliveryLane::SdrRec709);
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
            delivery_x264_params(DeliveryLane::SdrRec709),
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "the SDR x264 parameter string is identical at both depths and carries no range= key"
        );
        // CC8 §5.2 item 2: the HDR lane's string is the other value the same
        // function returns, and it is the string §10 step 1 proved.
        assert_eq!(
            delivery_x264_params(DeliveryLane::HdrHlgRec2020),
            "colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc",
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
        stamp_rgba_color(&mut rgba, DeliveryLane::SdrRec709);
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
            stamp_delivery_yuv_color(&mut yuv, DeliveryLane::SdrRec709);
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

        // (c) Neutral gray and neutral white are both exactly 128.0 in chroma.
        // The claim is that neutral input stays neutral: every chroma sample
        // lands on 128 or on one of its two neighbours, 128 itself is always
        // present, and nothing reaches 126 or 130.
        //
        // The claim is stated as that window rather than as an exact code set
        // because the exact set is a property of the *build*, not of the
        // conversion. Chroma sits exactly on a code, and CC6 §5.4's normative
        // rule — "a flat 8-bit delivery patch legitimately produces two
        // adjacent Y codes in a fixed 8x8 tiling; no assertion may require a
        // single code from an 8-bit delivery output except where the input
        // lands exactly on a code" — permits both outcomes here, and both
        // occur. The Linux pin (`mifi/ffmpeg-builds 8.0-1`, libswscale
        // 9.1.100) straddles: `Cb {128, 129}`, `Cr {127, 128}`. The Windows CI
        // package (`System233/ffmpeg-msvc-prebuilt ffmpeg-8.0.1-r3`) does not:
        // `Cb {128}`. Both sets are recorded here because both are
        // measurements.
        //
        // No production change closes that gap. Adding `accurate_rnd` to
        // `DELIVERY_SCALER_FLAGS` reproduces the Windows chroma exactly on
        // Linux — measured, and CC6 §5.4's "accurate_rnd is inert" holds only
        // for the luma plane it was measured on — but it also moves the pinned
        // decode figures (CC7 scenario (a)'s 8-bit luma mean goes 18 677 ->
        // 18 688, which is the Windows number), and `DELIVERY_SCALER_FLAGS` is
        // a CC6 constant whose value CC6 §5.3 measured and declined to change.
        // The dither straddle is therefore recorded and the window asserted,
        // exactly as CC6 §5.4 requires of a flat 8-bit delivery patch.
        let neutral_window = BTreeSet::from([127_u8, 128, 129]);
        for frame in [&filtered_white, &filtered_gray] {
            for (plane, name) in [(1_usize, "Cb"), (2, "Cr")] {
                let codes = chroma_codes(frame, plane);
                assert!(
                    !codes.is_empty() && codes.is_subset(&neutral_window),
                    "neutral input must stay neutral in {name}: {codes:?}"
                );
                assert!(
                    codes.contains(&128),
                    "neutral input must reach the neutral code itself in {name}: {codes:?}"
                );
            }
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
        let mut filter = delivery_filter_graph(
            resolution,
            DeliveryEncodeDepth::Eight,
            DeliveryLane::SdrRec709,
        )
        .expect("delivery filter graph");
        let mut rgba =
            ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, resolution.0, resolution.1);
        stamp_rgba_color(&mut rgba, DeliveryLane::SdrRec709);
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
        let mut filter = delivery_filter_graph(
            resolution,
            DeliveryEncodeDepth::Ten,
            DeliveryLane::SdrRec709,
        )
        .expect("10-bit delivery filter graph");
        let mut rgba =
            ffmpeg::frame::Video::new(DELIVERY_INTERMEDIATE_PIXEL, resolution.0, resolution.1);
        stamp_rgba_color(&mut rgba, DeliveryLane::SdrRec709);
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
}
