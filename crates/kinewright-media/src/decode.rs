use std::{path::Path, sync::Arc};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    AssetId, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance,
    ColorRange, ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, FrameTexture,
    MediaAsset, MediaError, MediaKind, Rational, RgbaImage, TimeCode,
    classify_source_with_assumption,
};

use crate::{
    cache::FrameCache,
    frame::{CachedFrame, WorkingFrame},
    sha256::source_fingerprint,
};

const AV_TIME_BASE: i64 = 1_000_000;
const COLOR_COVERAGE_PER_FIELD_BASIS_POINTS: u16 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoRotation {
    None,
    Clockwise90,
    HalfTurn,
    Clockwise270,
}

impl VideoRotation {
    fn display_dimensions(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Clockwise90 | Self::Clockwise270 => (height, width),
            Self::None | Self::HalfTurn => (width, height),
        }
    }

    fn encoded_dimensions(self, display_width: u32, display_height: u32) -> (u32, u32) {
        match self {
            Self::Clockwise90 | Self::Clockwise270 => (display_height, display_width),
            Self::None | Self::HalfTurn => (display_width, display_height),
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn probe_path(path: &Path, id: AssetId) -> Result<MediaAsset, MediaError> {
    let source_fingerprint = source_fingerprint(path)?;
    let input = media_input(path)?;
    let video = input.streams().best(ffmpeg::media::Type::Video);
    let audio = input.streams().best(ffmpeg::media::Type::Audio);

    if let Some(stream) = &video {
        ensure_decoder(stream, "video", path)?;
    }
    if let Some(stream) = &audio {
        ensure_decoder(stream, "audio", path)?;
    }

    let av_kind = match (video.is_some(), audio.is_some()) {
        (true, true) => MediaKind::AudioVideo,
        (true, false) => MediaKind::Video,
        (false, true) => MediaKind::Audio,
        (false, false) => {
            return Err(MediaError::Backend(format!(
                "media {} has no decodable audio or video stream; the file may be truncated or its codecs may be unsupported",
                path.display()
            )));
        }
    };

    let (fps, resolution, duration, color_description, kind) = if let Some(stream) = video {
        let timing =
            analyze_video_packets(path, stream.index(), normalized_start(stream.start_time()))?;
        let rate = select_video_rate(path, &stream, &timing)?;
        let rotation = stream_rotation(&stream).map_err(|error| {
            MediaError::Backend(format!(
                "could not read video orientation for {}: {error}",
                path.display()
            ))
        })?;
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| media_error(path, "could not read video codec parameters", error))?;
        let decoder = context
            .decoder()
            .video()
            .map_err(|error| media_error(path, "could not open the video decoder", error))?;
        // MO1 R7: a video-only still-image stream with exactly one packet is
        // a still. A still codec WITH audio stays `AudioVideo` (probed as a
        // one-frame video, exactly as before) so the audio never strands.
        let still =
            audio.is_none() && is_still_image_codec(decoder.id()) && timing.packet_count == 1;
        if still {
            let orientation = crate::still_orientation::still_file_orientation(path);
            let rotation = orientation.map_or(rotation, |found| found.rotation);
            let resolution = rotation.display_dimensions(decoder.width(), decoder.height());
            if resolution.0 == 0 || resolution.1 == 0 {
                return Err(MediaError::Backend(format!(
                    "still image {} has no decodable dimensions; the file may be truncated or corrupt",
                    path.display()
                )));
            }
            if resolution.0 > MAX_STILL_DIMENSION || resolution.1 > MAX_STILL_DIMENSION {
                return Err(MediaError::Backend(format!(
                    "still image {} is {}x{} but stills are limited to {MAX_STILL_DIMENSION}px on either axis; downscale it and re-import",
                    path.display(),
                    resolution.0,
                    resolution.1,
                )));
            }
            let reported = color_description_from_decoder(
                &decoder,
                negotiated_pixel_format(path, stream.index()),
            );
            // Decoder defaults (PNG's GBR matrix, JPEG's full range) are
            // pixel-format facts, not file tagging: a still whose primaries
            // AND transfer are both unknown carries `unknown()` into the
            // source-colour incident path, like any untagged source.
            let color_description = if reported.primaries == ColorPrimaries::Unknown
                && reported.transfer == ColorTransfer::Unknown
            {
                ColorDescription::unknown()
            } else {
                reported
            };
            (
                Rational::default(),
                Some(resolution),
                TimeCode(1),
                color_description,
                MediaKind::Image,
            )
        } else {
            let negotiated_pixel_format = negotiated_pixel_format(path, stream.index());
            let color_description =
                color_description_from_decoder(&decoder, negotiated_pixel_format);
            let stream_duration =
                timestamp_to_grid_ceil(stream.duration(), stream.time_base(), rate);
            let packet_duration = timing.duration.map_or(0, |duration| {
                timestamp_to_grid_ceil(duration, stream.time_base(), rate)
            });
            let container_duration = duration_us_to_frames(input.duration(), rate);
            let video_duration = stream_duration.max(packet_duration);
            let duration = TimeCode(if video_duration > 0 {
                video_duration
            } else {
                container_duration
            });
            let resolution = rotation.display_dimensions(decoder.width(), decoder.height());
            (rate, Some(resolution), duration, color_description, av_kind)
        }
    } else {
        let rate = Rational::default();
        (
            rate,
            None,
            TimeCode(duration_us_to_frames(input.duration(), rate)),
            ColorDescription::unknown(),
            av_kind,
        )
    };

    if duration <= TimeCode::ZERO {
        return Err(MediaError::Backend(format!(
            "media duration is missing or zero for {}; the file may be truncated",
            path.display()
        )));
    }

    Ok(MediaAsset {
        id,
        path: path.to_path_buf(),
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("media")
            .to_owned(),
        duration,
        fps,
        kind,
        resolution,
        source_fingerprint,
        color_description,
        assumed_from: None,
    })
}

fn color_description_from_decoder(
    decoder: &ffmpeg::codec::decoder::Video,
    negotiated_pixel_format: Option<ffmpeg::format::Pixel>,
) -> ColorDescription {
    let primaries = map_color_primaries(decoder.color_primaries());
    let transfer = map_color_transfer(decoder.color_transfer_characteristic());
    let matrix = map_color_matrix(decoder.color_space());
    let range = map_color_range(decoder.color_range());
    let bit_depth = negotiated_pixel_format
        .map(bit_depth_from_pixel)
        .filter(|depth| !matches!(depth, ColorBitDepth::Unknown))
        .unwrap_or_else(|| bit_depth_from_pixel(decoder.format()));
    let (provenance, confidence_basis_points) =
        color_metadata_coverage(&primaries, &transfer, &matrix, &range, &bit_depth);

    ColorDescription {
        primaries,
        transfer,
        matrix,
        range,
        white_point: ColorWhitePoint::Unknown,
        bit_depth,
        confidence_basis_points,
        provenance,
    }
}

/// Return the pixel format negotiated by the first decoded frame.
///
/// Some codecs leave `AVCodecContext::pix_fmt` at `AV_PIX_FMT_NONE` until the
/// first decoded frame. Probe-time color metadata uses the negotiated frame
/// format when available and preserves `Unknown` when the stream never yields
/// one.
fn negotiated_pixel_format(path: &Path, stream_index: usize) -> Option<ffmpeg::format::Pixel> {
    let mut input = media_input(path).ok()?;
    let mut decoder = {
        let stream = input
            .streams()
            .find(|stream| stream.index() == stream_index)?;
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters()).ok()?;
        context.decoder().video().ok()?
    };

    let mut frame = ffmpeg::frame::Video::empty();
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        decoder.send_packet(&packet).ok()?;
        match decoder.receive_frame(&mut frame) {
            Ok(()) => return Some(frame.format()),
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
            Err(_) => return None,
        }
    }

    decoder.send_eof().ok()?;
    match decoder.receive_frame(&mut frame) {
        Ok(()) => Some(frame.format()),
        Err(_) => None,
    }
}

/// Score how completely the probe described the source rather than claiming
/// that a declared value is intrinsically correct. Each of the four `FFmpeg`
/// stream colorimetry fields covers one fifth of the description, and a safely
/// inferred component depth covers the final fifth.
fn color_metadata_coverage(
    primaries: &ColorPrimaries,
    transfer: &ColorTransfer,
    matrix: &ColorMatrix,
    range: &ColorRange,
    bit_depth: &ColorBitDepth,
) -> (ColorProvenance, u16) {
    let known_stream_fields = [
        !matches!(primaries, ColorPrimaries::Unknown),
        !matches!(transfer, ColorTransfer::Unknown),
        !matches!(matrix, ColorMatrix::Unknown),
        !matches!(range, ColorRange::Unknown),
    ]
    .into_iter()
    .filter(|known| *known)
    .count();
    let bit_depth_known = !matches!(bit_depth, ColorBitDepth::Unknown);
    let known_fields = known_stream_fields + usize::from(bit_depth_known);
    let confidence_basis_points = u16::try_from(known_fields)
        .unwrap_or(5)
        .saturating_mul(COLOR_COVERAGE_PER_FIELD_BASIS_POINTS)
        .min(kinewright_core::COLOR_CONFIDENCE_MAX_BASIS_POINTS);
    let provenance = if known_stream_fields > 0 {
        ColorProvenance::StreamMetadata
    } else if bit_depth_known {
        ColorProvenance::Inferred
    } else {
        ColorProvenance::Unknown
    };
    (provenance, confidence_basis_points)
}

fn map_color_primaries(value: ffmpeg::color::Primaries) -> ColorPrimaries {
    use ffmpeg::color::Primaries;

    match value {
        Primaries::Reserved0 | Primaries::Unspecified | Primaries::Reserved => {
            ColorPrimaries::Unknown
        }
        Primaries::BT709 => ColorPrimaries::Bt709,
        Primaries::BT2020 => ColorPrimaries::Bt2020,
        Primaries::BT470M => ColorPrimaries::Bt470M,
        Primaries::BT470BG => ColorPrimaries::Bt470Bg,
        Primaries::SMPTE170M => ColorPrimaries::Smpte170M,
        Primaries::SMPTE240M => ColorPrimaries::Smpte240M,
        Primaries::SMPTE431 => ColorPrimaries::DciP3,
        Primaries::SMPTE432 => ColorPrimaries::DisplayP3,
        Primaries::Film => ColorPrimaries::Film,
        Primaries::SMPTE428 => ColorPrimaries::Other("smpte428".to_owned()),
        Primaries::EBU3213 => ColorPrimaries::Other("ebu3213".to_owned()),
    }
}

fn map_color_transfer(value: ffmpeg::color::TransferCharacteristic) -> ColorTransfer {
    use ffmpeg::color::TransferCharacteristic;

    match value {
        TransferCharacteristic::Reserved0
        | TransferCharacteristic::Unspecified
        | TransferCharacteristic::Reserved => ColorTransfer::Unknown,
        TransferCharacteristic::BT709 => ColorTransfer::Bt709,
        TransferCharacteristic::GAMMA22 => ColorTransfer::Gamma22,
        TransferCharacteristic::GAMMA28 => ColorTransfer::Gamma28,
        TransferCharacteristic::SMPTE170M => ColorTransfer::Smpte170M,
        TransferCharacteristic::Linear => ColorTransfer::Linear,
        TransferCharacteristic::Log => ColorTransfer::Log,
        TransferCharacteristic::IEC61966_2_1 => ColorTransfer::Srgb,
        TransferCharacteristic::SMPTE2084 => ColorTransfer::Smpte2084,
        TransferCharacteristic::ARIB_STD_B67 => ColorTransfer::AribStdB67,
        TransferCharacteristic::SMPTE240M => ColorTransfer::Other("smpte240m".to_owned()),
        TransferCharacteristic::LogSqrt => ColorTransfer::Other("log_sqrt".to_owned()),
        TransferCharacteristic::IEC61966_2_4 => ColorTransfer::Other("iec61966_2_4".to_owned()),
        TransferCharacteristic::BT1361_ECG => ColorTransfer::Other("bt1361_ecg".to_owned()),
        TransferCharacteristic::BT2020_10 => ColorTransfer::Other("bt2020_10".to_owned()),
        TransferCharacteristic::BT2020_12 => ColorTransfer::Other("bt2020_12".to_owned()),
        TransferCharacteristic::SMPTE428 => ColorTransfer::Other("smpte428".to_owned()),
    }
}

fn map_color_matrix(value: ffmpeg::color::Space) -> ColorMatrix {
    use ffmpeg::color::Space;

    match value {
        Space::Unspecified | Space::Reserved => ColorMatrix::Unknown,
        Space::RGB => ColorMatrix::Rgb,
        Space::BT709 => ColorMatrix::Bt709,
        Space::BT470BG => ColorMatrix::Other("bt470bg".to_owned()),
        Space::BT2020NCL => ColorMatrix::Bt2020Ncl,
        Space::BT2020CL => ColorMatrix::Bt2020Cl,
        Space::SMPTE170M => ColorMatrix::Smpte170M,
        Space::SMPTE240M => ColorMatrix::Smpte240M,
        Space::YCGCO => ColorMatrix::Ycgco,
        Space::ChromaDerivedNCL => ColorMatrix::ChromaDerivedNcl,
        Space::ChromaDerivedCL => ColorMatrix::ChromaDerivedCl,
        Space::ICTCP => ColorMatrix::Ictcp,
        Space::FCC => ColorMatrix::Other("fcc".to_owned()),
        Space::SMPTE2085 => ColorMatrix::Other("smpte2085".to_owned()),
        Space::IPT_C2 => ColorMatrix::Other("ipt_c2".to_owned()),
        Space::YCGCO_RE => ColorMatrix::Other("ycgco_re".to_owned()),
        Space::YCGCO_RO => ColorMatrix::Other("ycgco_ro".to_owned()),
    }
}

fn map_color_range(value: ffmpeg::color::Range) -> ColorRange {
    match value {
        ffmpeg::color::Range::Unspecified => ColorRange::Unknown,
        ffmpeg::color::Range::MPEG => ColorRange::Limited,
        ffmpeg::color::Range::JPEG => ColorRange::Full,
    }
}

#[allow(clippy::too_many_lines)]
fn bit_depth_from_pixel(pixel: ffmpeg::format::Pixel) -> ColorBitDepth {
    let Some(name) = pixel
        .descriptor()
        .map(ffmpeg::format::pixel::Descriptor::name)
    else {
        return ColorBitDepth::Unknown;
    };

    if matches!(
        name,
        "yuv420p"
            | "yuv422p"
            | "yuv444p"
            | "yuv440p"
            | "yuv411p"
            | "yuv410p"
            | "yuvj420p"
            | "yuvj422p"
            | "yuvj444p"
            | "yuvj440p"
            | "yuyv422"
            | "uyvy422"
            | "nv12"
            | "nv21"
            | "nv16"
            | "gbrp"
            | "gbrap"
            | "yuva420p"
            | "yuva422p"
            | "yuva444p"
            | "rgba"
            | "bgra"
            | "argb"
            | "abgr"
            | "rgb0"
            | "bgr0"
            | "0rgb"
            | "0bgr"
            | "rgb24"
            | "bgr24"
            | "gray"
            | "ya8"
    ) {
        return ColorBitDepth::Eight;
    }

    if matches!(
        name,
        "yuv420p9be"
            | "yuv420p9le"
            | "yuv422p9be"
            | "yuv422p9le"
            | "yuv444p9be"
            | "yuv444p9le"
            | "gbrp9be"
            | "gbrp9le"
            | "yuva420p9be"
            | "yuva420p9le"
            | "yuva422p9be"
            | "yuva422p9le"
            | "yuva444p9be"
            | "yuva444p9le"
            | "gray9be"
            | "gray9le"
    ) {
        return ColorBitDepth::Integer(9);
    }

    if matches!(
        name,
        "yuv420p10be"
            | "yuv420p10le"
            | "yuv422p10be"
            | "yuv422p10le"
            | "yuv444p10be"
            | "yuv444p10le"
            | "gbrp10be"
            | "gbrp10le"
            | "yuva420p10be"
            | "yuva420p10le"
            | "yuva422p10be"
            | "yuva422p10le"
            | "yuva444p10be"
            | "yuva444p10le"
            | "yuv440p10be"
            | "yuv440p10le"
            | "gbrap10be"
            | "gbrap10le"
            | "p010be"
            | "p010le"
            | "gray10be"
            | "gray10le"
    ) {
        return ColorBitDepth::Ten;
    }

    if matches!(
        name,
        "yuv420p12be"
            | "yuv420p12le"
            | "yuv422p12be"
            | "yuv422p12le"
            | "yuv440p12be"
            | "yuv440p12le"
            | "yuv444p12be"
            | "yuv444p12le"
            | "gbrp12be"
            | "gbrp12le"
            | "gbrap12be"
            | "gbrap12le"
            | "yuva422p12be"
            | "yuva422p12le"
            | "yuva444p12be"
            | "yuva444p12le"
            | "p012be"
            | "p012le"
            | "gray12be"
            | "gray12le"
    ) {
        return ColorBitDepth::Twelve;
    }

    if matches!(
        name,
        "yuv420p14be"
            | "yuv420p14le"
            | "yuv422p14be"
            | "yuv422p14le"
            | "yuv444p14be"
            | "yuv444p14le"
            | "gbrp14be"
            | "gbrp14le"
            | "gbrap14be"
            | "gbrap14le"
            | "gray14be"
            | "gray14le"
    ) {
        return ColorBitDepth::Integer(14);
    }

    if matches!(
        name,
        "yuv420p16be"
            | "yuv420p16le"
            | "yuv422p16be"
            | "yuv422p16le"
            | "yuv444p16be"
            | "yuv444p16le"
            | "gbrp16be"
            | "gbrp16le"
            | "gbrap16be"
            | "gbrap16le"
            | "yuva420p16be"
            | "yuva420p16le"
            | "yuva422p16be"
            | "yuva422p16le"
            | "yuva444p16be"
            | "yuva444p16le"
            | "p016be"
            | "p016le"
            | "gray16be"
            | "gray16le"
            | "ya16be"
            | "ya16le"
            | "rgb48be"
            | "rgb48le"
            | "rgba64be"
            | "rgba64le"
    ) {
        return ColorBitDepth::Sixteen;
    }

    if matches!(
        name,
        "gbrpf16be" | "gbrpf16le" | "gbrapf16be" | "gbrapf16le"
    ) {
        return ColorBitDepth::Float16;
    }
    if matches!(
        name,
        "gbrpf32be" | "gbrpf32le" | "gbrapf32be" | "gbrapf32le"
    ) {
        return ColorBitDepth::Float32;
    }

    ColorBitDepth::Unknown
}

fn declared_integer_depth(description: &ColorDescription) -> Result<u8, String> {
    if let Some(bits) = integer_depth_value(&description.bit_depth) {
        return Ok(bits);
    }
    if matches!(description.bit_depth, ColorBitDepth::Unknown) {
        return Err("source integer bit depth is unknown".to_owned());
    }
    Err(format!(
        "unsupported source integer bit depth: {:?}",
        description.bit_depth
    ))
}

/// The integer sample depths this decoder can expand.
///
/// Core owns the named/numeric mapping, so a new named depth cannot be
/// silently dropped here; this adds only the range the expansion supports.
fn integer_depth_value(depth: &ColorBitDepth) -> Option<u8> {
    depth.integer_bits().filter(|bits| (8..=16).contains(bits))
}

pub(crate) fn decoder_format_name(pixel: ffmpeg::format::Pixel) -> String {
    pixel
        .descriptor()
        .map_or("unknown", ffmpeg::format::pixel::Descriptor::name)
        .to_owned()
}

fn unsupported_decoder_format(
    path: &Path,
    pixel: ffmpeg::format::Pixel,
    declared_bit_depth: Option<u8>,
    decoder_bit_depth: Option<u8>,
    reason: impl Into<String>,
) -> MediaError {
    MediaError::UnsupportedDecoderFormat {
        path: path.to_path_buf(),
        format: decoder_format_name(pixel),
        declared_bit_depth,
        decoder_bit_depth,
        reason: reason.into(),
    }
}

fn validate_managed_decoder_depth(
    path: &Path,
    decoder: &ffmpeg::codec::decoder::Video,
    source: &ManagedSource,
) -> Result<(), MediaError> {
    let declared_depth = declared_integer_depth(&source.description).map_err(|error| {
        unsupported_decoder_format(
            path,
            decoder.format(),
            integer_depth_value(&source.description.bit_depth),
            None,
            format!("managed source depth rejected: {error}"),
        )
    })?;
    let decoder_depth = bit_depth_from_pixel(decoder.format());
    let Some(decoded_bits) = integer_depth_value(&decoder_depth) else {
        return Err(unsupported_decoder_format(
            path,
            decoder.format(),
            Some(declared_depth),
            None,
            "managed decoder depth could not be identified",
        ));
    };
    if declared_depth != decoded_bits {
        return Err(unsupported_decoder_format(
            path,
            decoder.format(),
            Some(declared_depth),
            Some(decoded_bits),
            format!(
                "managed source depth mismatch (declared={declared_depth}-bit, decoder={decoded_bits}-bit)"
            ),
        ));
    }
    Ok(())
}

fn valid_rate(rate: ffmpeg::Rational) -> Option<Rational> {
    let numerator = u32::try_from(rate.numerator()).ok()?;
    let denominator = u32::try_from(rate.denominator()).ok()?;
    Rational::new(numerator, denominator).ok()
}

fn duration_us_to_frames(duration: i64, fps: Rational) -> i64 {
    if duration <= 0 {
        return 0;
    }
    mul_div_ceil(
        duration,
        i64::from(fps.numerator()),
        AV_TIME_BASE.saturating_mul(i64::from(fps.denominator())),
    )
}

fn select_video_rate(
    path: &Path,
    stream: &ffmpeg::Stream<'_>,
    timing: &VideoPacketTiming,
) -> Result<Rational, MediaError> {
    let average = valid_rate(stream.avg_frame_rate());
    let nominal = valid_rate(stream.rate());
    match (average, nominal) {
        (Some(average), Some(nominal)) if average != nominal => {
            if timing.variable {
                Ok(average)
            } else {
                Ok(nominal)
            }
        }
        (Some(average), _) => Ok(average),
        (None, Some(nominal)) => Ok(nominal),
        (None, None) => Err(MediaError::Backend(format!(
            "video {} has no valid frame rate",
            path.display()
        ))),
    }
}

struct VideoPacketTiming {
    variable: bool,
    duration: Option<i64>,
    /// Every packet on the stream, pts-bearing or not — MO1 R7's "exactly
    /// one frame" still test counts packets, not timestamps.
    packet_count: u64,
}

fn analyze_video_packets(
    path: &Path,
    stream_index: usize,
    stream_start: i64,
) -> Result<VideoPacketTiming, MediaError> {
    let mut input = media_input(path)?;
    let mut reference_duration = None;
    let mut duration_variable = false;
    let mut pictures = Vec::new();
    let mut packet_count = 0_u64;
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        packet_count = packet_count.saturating_add(1);
        let packet_duration = packet.duration();
        if packet_duration > 0 {
            if reference_duration
                .is_some_and(|reference| timing_values_differ(reference, packet_duration))
            {
                duration_variable = true;
            }
            reference_duration.get_or_insert(packet_duration);
        }
        if let Some(pts) = packet.pts() {
            pictures.push((pts, packet_duration.max(0)));
        }
    }
    pictures.sort_unstable_by_key(|(pts, _)| *pts);
    pictures.dedup_by(|current, previous| {
        if current.0 == previous.0 {
            previous.1 = previous.1.max(current.1);
            true
        } else {
            false
        }
    });
    let mut delta = None;
    let mut delta_count = 0_u64;
    let mut pts_variable = false;
    for pair in pictures.windows(2) {
        let current = pair[1].0.saturating_sub(pair[0].0);
        if current <= 0 {
            continue;
        }
        delta_count = delta_count.saturating_add(1);
        if delta.is_some_and(|reference| timing_values_differ(reference, current)) {
            pts_variable = true;
        }
        delta.get_or_insert(current);
    }
    let duration = pictures.last().and_then(|(last_pts, last_duration)| {
        let final_interval = if *last_duration > 0 {
            *last_duration
        } else {
            delta.unwrap_or(0)
        };
        let end = last_pts.saturating_add(final_interval);
        (end > stream_start).then_some(end.saturating_sub(stream_start))
    });
    let variable = if delta_count >= 2 {
        pts_variable
    } else {
        duration_variable
    };
    Ok(VideoPacketTiming {
        variable,
        duration,
        packet_count,
    })
}

fn timing_values_differ(lhs: i64, rhs: i64) -> bool {
    let tolerance = lhs.abs().min(rhs.abs()).saturating_div(1_000).max(1);
    lhs.abs_diff(rhs) > u64::try_from(tolerance).unwrap_or(u64::MAX)
}

fn timestamp_to_grid_ceil(timestamp: i64, time_base: ffmpeg::Rational, fps: Rational) -> i64 {
    if timestamp <= 0 {
        return 0;
    }
    let numerator = i64::from(time_base.numerator()).saturating_mul(i64::from(fps.numerator()));
    let denominator =
        i64::from(time_base.denominator()).saturating_mul(i64::from(fps.denominator()));
    mul_div_ceil(timestamp, numerator, denominator)
}

fn mul_div_ceil(value: i64, multiplier: i64, divisor: i64) -> i64 {
    if divisor <= 0 {
        return 0;
    }
    let value = i128::from(value).saturating_mul(i128::from(multiplier));
    let divisor = i128::from(divisor);
    let rounded = value.saturating_add(divisor - 1) / divisor;
    i64::try_from(rounded).unwrap_or(i64::MAX)
}

pub(crate) fn frame_to_global_timestamp(frame: TimeCode, fps: Rational) -> i64 {
    if frame.0 <= 0 {
        return 0;
    }
    let numerator = i128::from(frame.0)
        .saturating_mul(i128::from(AV_TIME_BASE))
        .saturating_mul(i128::from(fps.denominator()));
    let timestamp = numerator / i128::from(fps.numerator());
    i64::try_from(timestamp).unwrap_or(i64::MAX)
}

pub(crate) fn stream_timestamp_to_global(timestamp: i64, time_base: ffmpeg::Rational) -> i64 {
    if timestamp <= 0 {
        return 0;
    }
    let numerator = i128::from(timestamp)
        .saturating_mul(i128::from(time_base.numerator()))
        .saturating_mul(i128::from(AV_TIME_BASE));
    let denominator = i128::from(time_base.denominator());
    i64::try_from(numerator / denominator).unwrap_or(i64::MAX)
}

pub(crate) fn media_input(path: &Path) -> Result<ffmpeg::format::context::Input, MediaError> {
    ffmpeg::format::input(path).map_err(|error| {
        MediaError::Backend(format!(
            "could not open media {}: {error}; the file may be truncated or its format may be unsupported",
            path.display()
        ))
    })
}

pub(crate) fn media_error(path: &Path, action: &str, error: impl std::fmt::Display) -> MediaError {
    MediaError::Backend(format!(
        "{action} for {}: {error}; the file may be truncated",
        path.display()
    ))
}

pub(crate) fn ensure_decoder(
    stream: &ffmpeg::Stream<'_>,
    kind: &str,
    path: &Path,
) -> Result<(), MediaError> {
    let codec = stream.parameters().id();
    if ffmpeg::decoder::find(codec).is_none() {
        return Err(MediaError::Backend(format!(
            "{kind} codec {codec:?} in {} is not supported by this FFmpeg build",
            path.display()
        )));
    }
    Ok(())
}

fn stream_rotation(stream: &ffmpeg::Stream<'_>) -> Result<VideoRotation, MediaError> {
    let side_data_rotation = stream.side_data().find_map(|side_data| {
        (side_data.kind() == ffmpeg::codec::packet::side_data::Type::DisplayMatrix)
            .then(|| rotation_from_display_matrix(side_data.data()))
    });
    if let Some(rotation) = side_data_rotation {
        return rotation;
    }
    let metadata = stream.metadata();
    let Some(value) = metadata.get("rotate") else {
        return Ok(VideoRotation::None);
    };
    let degrees = value.parse::<i32>().map_err(|_| {
        MediaError::Backend(format!("video rotation metadata {value:?} is invalid"))
    })?;
    rotation_from_degrees(degrees)
}

fn rotation_from_display_matrix(data: &[u8]) -> Result<VideoRotation, MediaError> {
    if data.len() < 36 {
        return Err(MediaError::Backend(
            "video display matrix is truncated".to_owned(),
        ));
    }
    let element = |index: usize| {
        let offset = index * 4;
        i32::from_ne_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    };
    let a = i64::from(element(0));
    let b = i64::from(element(1));
    let c = i64::from(element(3));
    let d = i64::from(element(4));
    let determinant = a.saturating_mul(d).saturating_sub(b.saturating_mul(c));
    if determinant < 0 {
        return Err(MediaError::Backend(
            "reflected video display matrices are not supported".to_owned(),
        ));
    }
    if determinant == 0 {
        return Err(MediaError::Backend(
            "video display matrix is singular or invalid".to_owned(),
        ));
    }
    let dominant = a.abs().max(b.abs());
    let residual = a.abs().min(b.abs());
    if dominant == 0 || residual.saturating_mul(1_000) > dominant {
        return Err(MediaError::Backend(
            "non-right-angle video display rotation is not supported".to_owned(),
        ));
    }
    let degrees = if a.abs() >= b.abs() {
        if a >= 0 { 0 } else { 180 }
    } else if b < 0 {
        90
    } else {
        270
    };
    rotation_from_degrees(degrees)
}

fn rotation_from_degrees(degrees: i32) -> Result<VideoRotation, MediaError> {
    match degrees.rem_euclid(360) {
        0 => Ok(VideoRotation::None),
        90 => Ok(VideoRotation::Clockwise90),
        180 => Ok(VideoRotation::HalfTurn),
        270 => Ok(VideoRotation::Clockwise270),
        angle => Err(MediaError::Backend(format!(
            "video display rotation {angle} degrees is not supported; use a right-angle rotation"
        ))),
    }
}

struct PendingVideoFrame {
    first_grid_frame: i64,
    decoded: Option<ffmpeg::frame::Video>,
}

pub(crate) struct ManagedSource {
    description: ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
}

enum VideoConverter {
    Legacy(ffmpeg::software::scaling::Context),
    Managed(ffmpeg::filter::Graph),
}

fn managed_filter_matrix(description: &ColorDescription) -> &'static str {
    match description.matrix {
        ColorMatrix::Rgb | ColorMatrix::Identity => "gbr",
        _ => "bt709",
    }
}

fn managed_filter_is_rgb(description: &ColorDescription) -> bool {
    matches!(description.matrix, ColorMatrix::Rgb | ColorMatrix::Identity)
}

fn managed_filter_range(description: &ColorDescription) -> &'static str {
    match description.range {
        ColorRange::Limited => "mpeg",
        _ => "jpeg",
    }
}

pub(crate) fn managed_filter_graph(
    path: &Path,
    decoder: &ffmpeg::codec::decoder::Video,
    stream_time_base: ffmpeg::Rational,
    scaled_width: u32,
    scaled_height: u32,
    description: &ColorDescription,
) -> Result<ffmpeg::filter::Graph, MediaError> {
    let source_filter = ffmpeg::filter::find("buffer").ok_or_else(|| {
        MediaError::Backend(format!(
            "managed source filter 'buffer' is unavailable for {}",
            path.display()
        ))
    })?;
    let sink_filter = ffmpeg::filter::find("buffersink").ok_or_else(|| {
        MediaError::Backend(format!(
            "managed sink filter 'buffersink' is unavailable for {}",
            path.display()
        ))
    })?;
    let scale_filter = ffmpeg::filter::find("scale").ok_or_else(|| {
        MediaError::Backend(format!(
            "managed scale filter is unavailable for {}",
            path.display()
        ))
    })?;
    let format_filter = ffmpeg::filter::find("format").ok_or_else(|| {
        MediaError::Backend(format!(
            "managed format filter is unavailable for {}",
            path.display()
        ))
    })?;
    let source_pixel = decoder_format_name(decoder.format());
    let source_range = managed_filter_range(description);
    let source_matrix = managed_filter_matrix(description);
    let rgb_source = managed_filter_is_rgb(description);
    let args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect=1/1:colorspace={}:range={}",
        decoder.width(),
        decoder.height(),
        source_pixel,
        stream_time_base.numerator(),
        stream_time_base.denominator(),
        source_matrix,
        source_range,
    );
    let scale_args = if rgb_source {
        format!(
            "w={scaled_width}:h={scaled_height}:flags=bicubic:in_range={source_range}:out_range=jpeg"
        )
    } else {
        format!(
            "w={scaled_width}:h={scaled_height}:flags=bicubic:in_color_matrix=bt709:out_color_matrix=bt709:in_range={source_range}:out_range=jpeg"
        )
    };
    let mut graph = ffmpeg::filter::Graph::new();
    let mut source_context = graph
        .add(&source_filter, "source", &args)
        .map_err(|error| {
            MediaError::Backend(format!(
                "could not configure managed source filter for {} (matrix={source_matrix}, range={source_range}): {error}",
                path.display()
            ))
        })?;
    let mut scale_context = graph
        .add(&scale_filter, "scale", &scale_args)
        .map_err(|error| {
            MediaError::Backend(format!(
                "could not configure managed scale filter for {} (args={scale_args:?}): {error}",
                path.display()
            ))
        })?;
    let mut format_context = graph
        .add(&format_filter, "format", "pix_fmts=rgba64le")
        .map_err(|error| {
            MediaError::Backend(format!(
                "could not configure managed RGBA64 format filter for {}: {error}",
                path.display()
            ))
        })?;
    let mut sink_context = graph.add(&sink_filter, "sink", "").map_err(|error| {
        MediaError::Backend(format!(
            "could not configure managed sink filter for {}: {error}",
            path.display()
        ))
    })?;
    source_context.link(0, &mut scale_context, 0);
    scale_context.link(0, &mut format_context, 0);
    format_context.link(0, &mut sink_context, 0);
    graph.validate().map_err(|error| {
        MediaError::Backend(format!(
            "could not validate managed colour conversion for {} (scale_args={scale_args:?}, matrix={source_matrix}, range={source_range}, rgb_source={rgb_source}, output=rgba64le/full): {error}",
            path.display()
        ))
    })?;
    Ok(graph)
}

pub(crate) trait DecoderFrame: CachedFrame {
    fn from_rgba_frame(
        rgba: &ffmpeg::frame::Video,
        width: u32,
        height: u32,
        rotation: VideoRotation,
        flip_horizontal: bool,
        managed_source: Option<&ManagedSource>,
    ) -> Result<Self, MediaError>;
}

impl DecoderFrame for FrameTexture {
    fn from_rgba_frame(
        rgba: &ffmpeg::frame::Video,
        width: u32,
        height: u32,
        rotation: VideoRotation,
        flip_horizontal: bool,
        managed_source: Option<&ManagedSource>,
    ) -> Result<Self, MediaError> {
        if managed_source.is_some() {
            return Err(MediaError::Backend(
                "legacy RGBA8 decoder output cannot carry managed source metadata".to_owned(),
            ));
        }
        let pixels = read_plane(rgba, width, height, 4)?;
        // MO1 R7: EXIF's mirrored orientations flip in stored-pixel space
        // before the right-angle rotation.
        let pixels = flip_optional(flip_horizontal, width, height, 4, pixels)?;
        let (width, height, pixels) = rotate_bytes(rotation, width, height, 4, pixels)?;
        Ok(Self {
            width,
            height,
            rgba: Arc::new(pixels),
        })
    }
}

impl DecoderFrame for WorkingFrame {
    fn from_rgba_frame(
        rgba: &ffmpeg::frame::Video,
        width: u32,
        height: u32,
        rotation: VideoRotation,
        flip_horizontal: bool,
        managed_source: Option<&ManagedSource>,
    ) -> Result<Self, MediaError> {
        let source = managed_source.ok_or_else(|| {
            MediaError::Backend(
                "managed RGBA64 decoder output is missing source colour metadata".to_owned(),
            )
        })?;
        let pixels = read_plane(rgba, width, height, 8)?;
        // MO1 R7: EXIF's mirrored orientations flip in stored-pixel space
        // before the right-angle rotation.
        let pixels = flip_optional(flip_horizontal, width, height, 8, pixels)?;
        let (width, height, pixels) = rotate_bytes(rotation, width, height, 8, pixels)?;
        WorkingFrame::from_rgba64_le(
            width,
            height,
            &pixels,
            &source.description,
            source.assumption,
        )
    }
}

pub(crate) struct VideoDecoder {
    path: std::path::PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    converter: VideoConverter,
    managed_source: Option<ManagedSource>,
    stream_index: usize,
    stream_time_base: ffmpeg::Rational,
    stream_start: i64,
    fps: Rational,
    rotation: VideoRotation,
    /// MO1 R7: EXIF's mirrored orientations (2/4/5/7) flip in stored-pixel
    /// space before `rotation` runs. Always false for video.
    flip_horizontal: bool,
    scaled_width: u32,
    scaled_height: u32,
    fallback_index: i64,
    pending: Option<PendingVideoFrame>,
    lookahead: Option<PendingVideoFrame>,
    continuation_at: Option<TimeCode>,
    eof_sent: bool,
    seek_count: u64,
}

impl VideoDecoder {
    #[cfg(test)]
    pub(crate) fn open(path: &Path, fps: Rational) -> Result<Self, MediaError> {
        Self::open_scaled(path, fps, None)
    }

    pub(crate) fn open_scaled(
        path: &Path,
        fps: Rational,
        max_width: Option<u32>,
    ) -> Result<Self, MediaError> {
        Self::open_scaled_internal(path, fps, max_width, None)
    }

    pub(crate) fn open_scaled_managed(
        path: &Path,
        fps: Rational,
        max_width: Option<u32>,
        description: &ColorDescription,
        assumption: Option<ColorSourceProfileAssumption>,
    ) -> Result<Self, MediaError> {
        // IN1 §4.2 rule 8: the typed refusal leaves the decoder intact. The
        // contextual sentence this site used to build by hand is rebuilt by
        // `SourceColorRefusal`'s `#[error(...)]` template once
        // `contextual_managed_decode_error` has added the asset and the path,
        // so no caller sees a different string.
        let source = classify_source_with_assumption(description, assumption)
            .map_err(MediaError::SourceColor)?;
        let _ = source;
        let _declared_depth = declared_integer_depth(description).map_err(|error| {
            unsupported_decoder_format(
                path,
                ffmpeg::format::Pixel::None,
                integer_depth_value(&description.bit_depth),
                None,
                format!("managed source depth rejected: {error}"),
            )
        })?;
        Self::open_scaled_internal(
            path,
            fps,
            max_width,
            Some(ManagedSource {
                description: description.clone(),
                assumption,
            }),
        )
    }

    #[allow(clippy::too_many_lines)]
    fn open_scaled_internal(
        path: &Path,
        fps: Rational,
        max_width: Option<u32>,
        managed_source: Option<ManagedSource>,
    ) -> Result<Self, MediaError> {
        let input = media_input(path)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| {
                MediaError::Backend(format!("media {} has no video stream", path.display()))
            })?;
        let stream_index = stream.index();
        let stream_time_base = stream.time_base();
        let stream_start = normalized_start(stream.start_time());
        ensure_decoder(&stream, "video", path)?;
        let mut rotation = stream_rotation(&stream).map_err(|error| {
            MediaError::Backend(format!(
                "could not read video orientation for {}: {error}",
                path.display()
            ))
        })?;
        let mut context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| media_error(path, "could not read video codec parameters", error))?;
        context.set_threading(ffmpeg::codec::threading::Config {
            kind: ffmpeg::codec::threading::Type::Frame,
            count: std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(16),
        });
        let decoder = context
            .decoder()
            .video()
            .map_err(|error| media_error(path, "could not open the video decoder", error))?;
        if let Some(source) = &managed_source {
            validate_managed_decoder_depth(path, &decoder, source)?;
        }
        // MO1 R7: EXIF orientation overrides the (absent, for stills)
        // container rotation, and only for still-image codecs — containers
        // fail the file-signature checks inside `still_file_orientation`
        // anyway, so an MJPEG video can never take this branch by accident.
        let flip_horizontal = if is_still_image_codec(decoder.id()) {
            if let Some(orientation) = crate::still_orientation::still_file_orientation(path) {
                rotation = orientation.rotation;
                orientation.flip_horizontal
            } else {
                false
            }
        } else {
            false
        };
        let source_width = decoder.width();
        let source_height = decoder.height();
        if source_width == 0 || source_height == 0 {
            return Err(MediaError::Backend(format!(
                "video {} reports an invalid zero-sized frame",
                path.display()
            )));
        }
        let display_source = rotation.display_dimensions(source_width, source_height);
        let display_width = max_width
            .unwrap_or(display_source.0)
            .min(display_source.0)
            .max(1);
        let display_height = u32::try_from(
            u64::from(display_source.1).saturating_mul(u64::from(display_width))
                / u64::from(display_source.0.max(1)),
        )
        .unwrap_or(display_source.1)
        .max(1);
        let (scaled_width, scaled_height) =
            rotation.encoded_dimensions(display_width, display_height);
        let target_pixel = if managed_source.is_some() {
            ffmpeg::format::Pixel::RGBA64LE
        } else {
            ffmpeg::format::Pixel::RGBA
        };
        let converter = if let Some(source) = &managed_source {
            VideoConverter::Managed(managed_filter_graph(
                path,
                &decoder,
                stream_time_base,
                scaled_width,
                scaled_height,
                &source.description,
            )?)
        } else {
            VideoConverter::Legacy(
                ffmpeg::software::scaling::Context::get(
                    decoder.format(),
                    source_width,
                    source_height,
                    target_pixel,
                    scaled_width,
                    scaled_height,
                    ffmpeg::software::scaling::Flags::BILINEAR,
                )
                .map_err(|error| {
                    media_error(path, "could not create video pixel converter", error)
                })?,
            )
        };

        Ok(Self {
            path: path.to_path_buf(),
            input,
            decoder,
            converter,
            managed_source,
            stream_index,
            stream_time_base,
            stream_start,
            fps,
            rotation,
            flip_horizontal,
            scaled_width,
            scaled_height,
            fallback_index: 0,
            pending: None,
            lookahead: None,
            continuation_at: None,
            eof_sent: false,
            seek_count: 0,
        })
    }

    pub(crate) fn decode_window<T: DecoderFrame>(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache<T>,
    ) -> Result<(), MediaError> {
        let timestamp = frame_to_global_timestamp(start, self.fps).saturating_add(
            stream_timestamp_to_global(self.stream_start, self.stream_time_base),
        );
        self.input
            .seek(timestamp, ..timestamp)
            .map_err(|error| media_error(&self.path, "video seek failed", error))?;
        self.decoder.flush();
        self.fallback_index = start.0;
        self.pending = None;
        self.lookahead = None;
        self.continuation_at = None;
        self.eof_sent = false;
        self.seek_count = self.seek_count.saturating_add(1);

        self.decode_from_cursor(start, end, cache)
    }

    /// Continue directly from the prior window when it ended immediately
    /// before `start`. A discontinuity falls back to the deterministic seek
    /// path used by scrubbing.
    pub(crate) fn decode_window_sequential<T: DecoderFrame>(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache<T>,
    ) -> Result<(), MediaError> {
        if self.continuation_at != Some(start) {
            return self.decode_window(start, end, cache);
        }
        self.decode_from_cursor(start, end, cache)
    }

    #[cfg(test)]
    pub(crate) fn seek_count(&self) -> u64 {
        self.seek_count
    }

    fn decode_from_cursor<T: DecoderFrame>(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache<T>,
    ) -> Result<(), MediaError> {
        if end < start {
            self.continuation_at = Some(start);
            return Ok(());
        }

        if self.apply_lookahead(start, end, cache)? {
            self.continuation_at = Some(TimeCode(end.0.saturating_add(1)));
            return Ok(());
        }

        loop {
            if self.receive_frames(start, end, cache)? {
                self.continuation_at = Some(TimeCode(end.0.saturating_add(1)));
                return Ok(());
            }
            if self.eof_sent {
                self.cache_pending_until(end.0.saturating_add(1), start, end, cache)?;
                self.continuation_at = Some(TimeCode(end.0.saturating_add(1)));
                return Ok(());
            }
            let next = self
                .input
                .packets()
                .next()
                .map(|(stream, packet)| (stream.index(), packet));
            let Some((stream_index, packet)) = next else {
                self.decoder.send_eof().map_err(|error| {
                    media_error(&self.path, "video decoder flush failed", error)
                })?;
                self.eof_sent = true;
                continue;
            };
            if stream_index != self.stream_index {
                continue;
            }
            self.decoder
                .send_packet(&packet)
                .map_err(|error| media_error(&self.path, "video decode failed", error))?;
        }
    }

    fn apply_lookahead<T: DecoderFrame>(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache<T>,
    ) -> Result<bool, MediaError> {
        let Some(next) = self.lookahead.take() else {
            return Ok(false);
        };
        self.accept_frame(next, start, end, cache)
    }

    fn receive_frames<T: DecoderFrame>(
        &mut self,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache<T>,
    ) -> Result<bool, MediaError> {
        loop {
            let mut decoded = ffmpeg::frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_err() {
                break;
            }
            let first_grid_frame = decoded.timestamp().map_or_else(
                || {
                    let index = self.fallback_index;
                    self.fallback_index = self.fallback_index.saturating_add(1);
                    index
                },
                |pts| {
                    timestamp_to_grid_ceil(
                        pts.saturating_sub(self.stream_start),
                        self.stream_time_base,
                        self.fps,
                    )
                },
            );
            let next = PendingVideoFrame {
                first_grid_frame,
                decoded: Some(decoded),
            };
            self.fallback_index = first_grid_frame.saturating_add(1);
            if self.accept_frame(next, start, end, cache)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn accept_frame<T: DecoderFrame>(
        &mut self,
        next: PendingVideoFrame,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache<T>,
    ) -> Result<bool, MediaError> {
        if self.pending.is_some() {
            self.cache_pending_until(next.first_grid_frame, start, end, cache)?;
        }
        if next.first_grid_frame > end.0 && self.pending.is_some() {
            self.lookahead = Some(next);
            return Ok(true);
        }
        self.pending = Some(next);
        Ok(false)
    }

    fn cache_pending_until<T: DecoderFrame>(
        &mut self,
        next_grid_frame: i64,
        start: TimeCode,
        end: TimeCode,
        cache: &mut FrameCache<T>,
    ) -> Result<(), MediaError> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(());
        };
        let first = pending.first_grid_frame.max(start.0);
        let last = next_grid_frame.saturating_sub(1).min(end.0);
        if first <= last {
            let decoded = pending
                .decoded
                .as_ref()
                .ok_or_else(|| MediaError::Backend("pending video frame has no pixels".to_owned()))?
                .clone();
            let texture = self.convert::<T>(&decoded)?;
            for index in first..=last {
                cache.insert(TimeCode(index), texture.clone());
            }
        }
        Ok(())
    }

    fn convert<T: DecoderFrame>(
        &mut self,
        decoded: &ffmpeg::frame::Video,
    ) -> Result<T, MediaError> {
        let mut rgba = ffmpeg::frame::Video::empty();
        match &mut self.converter {
            VideoConverter::Legacy(scaler) => scaler
                .run(decoded, &mut rgba)
                .map_err(|error| media_error(&self.path, "video pixel conversion failed", error))?,
            VideoConverter::Managed(graph) => {
                {
                    let mut source_context = graph.get("source").ok_or_else(|| {
                        MediaError::Backend(format!(
                            "managed source filter disappeared for {}",
                            self.path.display()
                        ))
                    })?;
                    let mut source = source_context.source();
                    source.add(decoded).map_err(|error| {
                        MediaError::Backend(format!(
                            "managed source frame submission failed for {}: {error}",
                            self.path.display()
                        ))
                    })?;
                }
                let mut sink_context = graph.get("sink").ok_or_else(|| {
                    MediaError::Backend(format!(
                        "managed sink filter disappeared for {}",
                        self.path.display()
                    ))
                })?;
                let mut sink = sink_context.sink();
                sink.frame(&mut rgba).map_err(|error| {
                    MediaError::Backend(format!(
                        "managed colour conversion failed for {} (output=rgba64le/full): {error}",
                        self.path.display()
                    ))
                })?;
            }
        }
        T::from_rgba_frame(
            &rgba,
            self.scaled_width,
            self.scaled_height,
            self.rotation,
            self.flip_horizontal,
            self.managed_source.as_ref(),
        )
    }
}

fn read_plane(
    frame: &ffmpeg::frame::Video,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> Result<Vec<u8>, MediaError> {
    let row_bytes = usize::try_from(width)
        .unwrap_or_default()
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| MediaError::Backend("decoded frame row is too large".to_owned()))?;
    let height = usize::try_from(height).unwrap_or_default();
    let mut pixels = Vec::with_capacity(row_bytes.saturating_mul(height));
    let plane = frame.data(0);
    let stride = frame.stride(0);
    for row in 0..height {
        let start = row.saturating_mul(stride);
        let end = start.saturating_add(row_bytes);
        let source = plane.get(start..end).ok_or_else(|| {
            MediaError::Backend("decoded RGBA frame has an invalid stride".to_owned())
        })?;
        pixels.extend_from_slice(source);
    }
    Ok(pixels)
}

fn rotate_bytes(
    rotation: VideoRotation,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    pixels: Vec<u8>,
) -> Result<(u32, u32, Vec<u8>), MediaError> {
    if rotation == VideoRotation::None {
        return Ok((width, height, pixels));
    }
    let expected = usize::try_from(width)
        .unwrap_or_default()
        .checked_mul(usize::try_from(height).unwrap_or_default())
        .and_then(|value| value.checked_mul(bytes_per_pixel))
        .ok_or_else(|| MediaError::Backend("rotated frame is too large".to_owned()))?;
    if pixels.len() != expected {
        return Err(MediaError::Backend(
            "decoded frame size does not match its dimensions".to_owned(),
        ));
    }
    let (output_width, output_height) = rotation.display_dimensions(width, height);
    let mut rotated = vec![0_u8; expected];
    for output_y in 0..output_height {
        for output_x in 0..output_width {
            let (source_x, source_y) = match rotation {
                VideoRotation::Clockwise90 => (output_y, height - 1 - output_x),
                VideoRotation::HalfTurn => (width - 1 - output_x, height - 1 - output_y),
                VideoRotation::Clockwise270 => (width - 1 - output_y, output_x),
                VideoRotation::None => (output_x, output_y),
            };
            let source = usize::try_from(source_y)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(width).unwrap_or_default())
                .saturating_add(usize::try_from(source_x).unwrap_or_default())
                .saturating_mul(bytes_per_pixel);
            let destination = usize::try_from(output_y)
                .unwrap_or_default()
                .saturating_mul(usize::try_from(output_width).unwrap_or_default())
                .saturating_add(usize::try_from(output_x).unwrap_or_default())
                .saturating_mul(bytes_per_pixel);
            rotated[destination..destination + bytes_per_pixel]
                .copy_from_slice(&pixels[source..source + bytes_per_pixel]);
        }
    }
    Ok((output_width, output_height, rotated))
}

/// Mirror a stored-pixel buffer left-right (MO1 R7, EXIF 2/4/5/7), or pass
/// it through untouched when the orientation needs no flip.
fn flip_optional(
    flip_horizontal: bool,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    pixels: Vec<u8>,
) -> Result<Vec<u8>, MediaError> {
    if !flip_horizontal {
        return Ok(pixels);
    }
    let row_bytes = usize::try_from(width)
        .unwrap_or_default()
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| MediaError::Backend("flipped frame is too large".to_owned()))?;
    let expected = row_bytes
        .checked_mul(usize::try_from(height).unwrap_or_default())
        .ok_or_else(|| MediaError::Backend("flipped frame is too large".to_owned()))?;
    if pixels.len() != expected {
        return Err(MediaError::Backend(
            "decoded frame size does not match its dimensions".to_owned(),
        ));
    }
    let mut flipped = vec![0_u8; expected];
    for row in 0..usize::try_from(height).unwrap_or_default() {
        for column in 0..usize::try_from(width).unwrap_or_default() {
            let source = row
                .saturating_mul(row_bytes)
                .saturating_add(column.saturating_mul(bytes_per_pixel));
            let mirror = usize::try_from(width)
                .unwrap_or_default()
                .saturating_sub(1)
                .saturating_sub(column);
            let destination = row
                .saturating_mul(row_bytes)
                .saturating_add(mirror.saturating_mul(bytes_per_pixel));
            flipped[destination..destination + bytes_per_pixel]
                .copy_from_slice(&pixels[source..source + bytes_per_pixel]);
        }
    }
    Ok(flipped)
}

pub(crate) fn thumbnail(
    path: &Path,
    fps: Rational,
    at: TimeCode,
    max_width: u32,
) -> Result<RgbaImage, MediaError> {
    let mut decoder = VideoDecoder::open_scaled(path, fps, Some(max_width.max(1)))?;
    let mut cache: FrameCache<FrameTexture> = FrameCache::new(2);
    decoder.decode_window(at, at, &mut cache)?;
    let frame = cache
        .frame_at_or_before(at)
        .ok_or_else(|| MediaError::Backend(format!("no video frame decoded at {at}")))?;
    Ok(RgbaImage {
        width: frame.width,
        height: frame.height,
        pixels: (*frame.rgba).clone(),
    })
}

pub(crate) fn normalized_start(start: i64) -> i64 {
    if start < -1_000_000_000_000 { 0 } else { start }
}

/// MO1 R7: stills over 8192 px on either axis are refused at probe (named
/// limit, typed refusal) — a larger still would blow the texture budget the
/// hold path pins by decoding once.
pub(crate) const MAX_STILL_DIMENSION: u32 = 8192;

/// MO1 R7: the still-image codec set — PNG and JPEG, per the design's
/// parenthetical. Other image codecs probe exactly as before (video).
fn is_still_image_codec(id: ffmpeg::codec::Id) -> bool {
    matches!(id, ffmpeg::codec::Id::PNG | ffmpeg::codec::Id::MJPEG)
}

pub(crate) fn backend(error: impl std::fmt::Display) -> MediaError {
    MediaError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_color_enums_map_known_and_unspecified_values() {
        assert_eq!(
            map_color_primaries(ffmpeg::color::Primaries::BT709),
            ColorPrimaries::Bt709
        );
        assert_eq!(
            map_color_primaries(ffmpeg::color::Primaries::SMPTE431),
            ColorPrimaries::DciP3
        );
        assert_eq!(
            map_color_primaries(ffmpeg::color::Primaries::SMPTE428),
            ColorPrimaries::Other("smpte428".to_owned())
        );
        assert_eq!(
            map_color_primaries(ffmpeg::color::Primaries::EBU3213),
            ColorPrimaries::Other("ebu3213".to_owned())
        );
        assert_eq!(
            map_color_primaries(ffmpeg::color::Primaries::Unspecified),
            ColorPrimaries::Unknown
        );

        assert_eq!(
            map_color_transfer(ffmpeg::color::TransferCharacteristic::BT709),
            ColorTransfer::Bt709
        );
        assert_eq!(
            map_color_transfer(ffmpeg::color::TransferCharacteristic::IEC61966_2_1),
            ColorTransfer::Srgb
        );
        assert_eq!(
            map_color_transfer(ffmpeg::color::TransferCharacteristic::BT2020_10),
            ColorTransfer::Other("bt2020_10".to_owned())
        );
        assert_eq!(
            map_color_transfer(ffmpeg::color::TransferCharacteristic::SMPTE428),
            ColorTransfer::Other("smpte428".to_owned())
        );
        assert_eq!(
            map_color_transfer(ffmpeg::color::TransferCharacteristic::Unspecified),
            ColorTransfer::Unknown
        );

        assert_eq!(
            map_color_matrix(ffmpeg::color::Space::RGB),
            ColorMatrix::Rgb
        );
        assert_eq!(
            map_color_matrix(ffmpeg::color::Space::BT2020NCL),
            ColorMatrix::Bt2020Ncl
        );
        assert_eq!(
            map_color_matrix(ffmpeg::color::Space::FCC),
            ColorMatrix::Other("fcc".to_owned())
        );
        assert_eq!(
            map_color_matrix(ffmpeg::color::Space::BT470BG),
            ColorMatrix::Other("bt470bg".to_owned())
        );
        assert_eq!(
            map_color_matrix(ffmpeg::color::Space::IPT_C2),
            ColorMatrix::Other("ipt_c2".to_owned())
        );
        assert_eq!(
            map_color_matrix(ffmpeg::color::Space::Unspecified),
            ColorMatrix::Unknown
        );

        assert_eq!(
            map_color_range(ffmpeg::color::Range::MPEG),
            ColorRange::Limited
        );
        assert_eq!(
            map_color_range(ffmpeg::color::Range::JPEG),
            ColorRange::Full
        );
        assert_eq!(
            map_color_range(ffmpeg::color::Range::Unspecified),
            ColorRange::Unknown
        );
    }

    #[test]
    fn common_pixel_formats_map_to_component_depth() {
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::YUV420P),
            ColorBitDepth::Eight
        );
        for pixel in [
            ffmpeg::format::Pixel::YUYV422,
            ffmpeg::format::Pixel::UYVY422,
            ffmpeg::format::Pixel::NV16,
            ffmpeg::format::Pixel::GBRP,
            ffmpeg::format::Pixel::YUVA420P,
            ffmpeg::format::Pixel::GBRAP,
            ffmpeg::format::Pixel::YUVJ440P,
        ] {
            assert_eq!(bit_depth_from_pixel(pixel), ColorBitDepth::Eight);
        }
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::YUV420P9LE),
            ColorBitDepth::Integer(9)
        );
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::YUV420P10LE),
            ColorBitDepth::Ten
        );
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::P010LE),
            ColorBitDepth::Ten
        );
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::YUV420P12LE),
            ColorBitDepth::Twelve
        );
        for pixel in [
            ffmpeg::format::Pixel::YUV440P12LE,
            ffmpeg::format::Pixel::YUVA422P12LE,
            ffmpeg::format::Pixel::YUVA444P12LE,
            ffmpeg::format::Pixel::GBRAP12LE,
        ] {
            assert_eq!(bit_depth_from_pixel(pixel), ColorBitDepth::Twelve);
        }
        for pixel in [
            ffmpeg::format::Pixel::YUV422P14LE,
            ffmpeg::format::Pixel::GBRAP14LE,
        ] {
            assert_eq!(bit_depth_from_pixel(pixel), ColorBitDepth::Integer(14));
        }
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::RGBA64LE),
            ColorBitDepth::Sixteen
        );
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::GBRPF32LE),
            ColorBitDepth::Float32
        );
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::None),
            ColorBitDepth::Unknown
        );
        assert_eq!(
            bit_depth_from_pixel(ffmpeg::format::Pixel::RGB565LE),
            ColorBitDepth::Unknown,
            "packed total-pixel widths must not be mistaken for component depth"
        );
    }

    #[test]
    fn ffmpeg_grayscale_descriptors_have_explicit_component_depths() {
        let cases = [
            (ffmpeg::format::Pixel::GRAY8, "gray", ColorBitDepth::Eight),
            (
                ffmpeg::format::Pixel::GRAY9BE,
                "gray9be",
                ColorBitDepth::Integer(9),
            ),
            (
                ffmpeg::format::Pixel::GRAY9LE,
                "gray9le",
                ColorBitDepth::Integer(9),
            ),
            (
                ffmpeg::format::Pixel::GRAY10BE,
                "gray10be",
                ColorBitDepth::Ten,
            ),
            (
                ffmpeg::format::Pixel::GRAY10LE,
                "gray10le",
                ColorBitDepth::Ten,
            ),
            (
                ffmpeg::format::Pixel::GRAY12BE,
                "gray12be",
                ColorBitDepth::Twelve,
            ),
            (
                ffmpeg::format::Pixel::GRAY12LE,
                "gray12le",
                ColorBitDepth::Twelve,
            ),
            (
                ffmpeg::format::Pixel::GRAY14BE,
                "gray14be",
                ColorBitDepth::Integer(14),
            ),
            (
                ffmpeg::format::Pixel::GRAY14LE,
                "gray14le",
                ColorBitDepth::Integer(14),
            ),
            (
                ffmpeg::format::Pixel::GRAY16BE,
                "gray16be",
                ColorBitDepth::Sixteen,
            ),
            (
                ffmpeg::format::Pixel::GRAY16LE,
                "gray16le",
                ColorBitDepth::Sixteen,
            ),
            (ffmpeg::format::Pixel::YA8, "ya8", ColorBitDepth::Eight),
            (
                ffmpeg::format::Pixel::YA16BE,
                "ya16be",
                ColorBitDepth::Sixteen,
            ),
            (
                ffmpeg::format::Pixel::YA16LE,
                "ya16le",
                ColorBitDepth::Sixteen,
            ),
        ];

        for (pixel, expected_name, expected_depth) in cases {
            let descriptor = pixel
                .descriptor()
                .expect("FFmpeg should expose the grayscale descriptor");
            assert_eq!(
                descriptor.name(),
                expected_name,
                "the regression must track FFmpeg's actual descriptor name"
            );
            assert_eq!(bit_depth_from_pixel(pixel), expected_depth);
        }
    }

    #[test]
    fn probe_infers_actual_ffmpeg_grayscale_integer_depths() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-grayscale-depths");
        let cases = [
            ("gray", ColorBitDepth::Eight),
            ("gray9le", ColorBitDepth::Integer(9)),
            ("gray10le", ColorBitDepth::Ten),
            ("gray12le", ColorBitDepth::Twelve),
            ("gray14le", ColorBitDepth::Integer(14)),
            ("gray16le", ColorBitDepth::Sixteen),
        ];

        for (pixel_format, expected_depth) in cases {
            let path = directory.path(&format!("{pixel_format}.nut"));
            let arguments = vec![
                "-f".to_owned(),
                "lavfi".to_owned(),
                "-i".to_owned(),
                "color=c=gray:size=4x1:rate=1:duration=1".to_owned(),
                "-vf".to_owned(),
                format!("format={pixel_format}"),
                "-frames:v".to_owned(),
                "1".to_owned(),
                "-an".to_owned(),
                "-c:v".to_owned(),
                "rawvideo".to_owned(),
                "-pix_fmt".to_owned(),
                pixel_format.to_owned(),
            ];
            crate::test_support::run_ffmpeg(&arguments, &path);

            let asset = probe_path(&path, AssetId(1))
                .unwrap_or_else(|error| panic!("{pixel_format} should probe: {error}"));
            assert_eq!(
                asset.color_description.bit_depth, expected_depth,
                "production probe should classify {pixel_format} from its decoded frame"
            );
        }
    }

    #[test]
    fn probe_infers_gbrp_depth_from_first_decoded_frame() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-gbrp-depth");
        let path = directory.path("gbrp.nut");
        crate::test_support::run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=gray:size=4x1:rate=1:duration=1",
                "-vf",
                "format=gbrp",
                "-frames:v",
                "1",
                "-an",
                "-c:v",
                "rawvideo",
                "-pix_fmt",
                "gbrp",
            ],
            &path,
        );

        let asset = probe_path(&path, AssetId(1)).expect("generated gbrp should probe");
        assert_eq!(asset.color_description.bit_depth, ColorBitDepth::Eight);
    }

    #[test]
    fn unsupported_decoder_format_has_a_stable_recovery_code() {
        let error = unsupported_decoder_format(
            Path::new("/tmp/source.mov"),
            ffmpeg::format::Pixel::None,
            Some(10),
            None,
            "decoder pixel format is unresolved",
        );
        assert_eq!(error.recovery_code(), Some("unsupported_decoder_format"));
        match error {
            MediaError::UnsupportedDecoderFormat {
                path,
                format,
                declared_bit_depth,
                decoder_bit_depth,
                reason,
            } => {
                assert_eq!(path, Path::new("/tmp/source.mov"));
                assert_eq!(format, "unknown");
                assert_eq!(declared_bit_depth, Some(10));
                assert_eq!(decoder_bit_depth, None);
                assert!(reason.contains("unresolved"));
            }
            other => panic!("expected structured decoder error, got {other:?}"),
        }
    }

    #[test]
    fn color_confidence_measures_partial_metadata_coverage() {
        let unknown_primaries = ColorPrimaries::Unknown;
        let unknown_transfer = ColorTransfer::Unknown;
        let unknown_matrix = ColorMatrix::Unknown;
        let unknown_range = ColorRange::Unknown;
        let unknown_depth = ColorBitDepth::Unknown;
        assert_eq!(
            color_metadata_coverage(
                &unknown_primaries,
                &unknown_transfer,
                &unknown_matrix,
                &unknown_range,
                &unknown_depth,
            ),
            (ColorProvenance::Unknown, 0)
        );

        let eight_bit = ColorBitDepth::Eight;
        assert_eq!(
            color_metadata_coverage(
                &unknown_primaries,
                &unknown_transfer,
                &unknown_matrix,
                &unknown_range,
                &eight_bit,
            ),
            (ColorProvenance::Inferred, 2_000)
        );

        let bt709_primaries = ColorPrimaries::Bt709;
        assert_eq!(
            color_metadata_coverage(
                &bt709_primaries,
                &unknown_transfer,
                &unknown_matrix,
                &unknown_range,
                &unknown_depth,
            ),
            (ColorProvenance::StreamMetadata, 2_000)
        );
        assert_eq!(
            color_metadata_coverage(
                &bt709_primaries,
                &unknown_transfer,
                &unknown_matrix,
                &unknown_range,
                &eight_bit,
            ),
            (ColorProvenance::StreamMetadata, 4_000)
        );

        assert_eq!(
            color_metadata_coverage(
                &bt709_primaries,
                &ColorTransfer::Bt709,
                &ColorMatrix::Bt709,
                &ColorRange::Limited,
                &eight_bit,
            ),
            (ColorProvenance::StreamMetadata, 10_000)
        );
    }

    /// Generate a one-frame still with lavfi (`color` source, PNG or MJPEG).
    fn still_fixture(
        directory: &crate::test_support::TempDirectory,
        name: &str,
        source: &str,
        codec: &str,
    ) -> std::path::PathBuf {
        let path = directory.path(name);
        crate::test_support::run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                source,
                "-frames:v",
                "1",
                "-c:v",
                codec,
                "-an",
            ],
            &path,
        );
        path
    }

    /// Table-free CRC-32 (ISO 3309) for hand-built PNG chunks.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = crc & 1;
                crc >>= 1;
                if mask == 1 {
                    crc ^= 0xEDB8_8320;
                }
            }
        }
        !crc
    }

    fn png_chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = u32::try_from(data.len()).unwrap().to_be_bytes().to_vec();
        chunk.extend_from_slice(&kind);
        chunk.extend_from_slice(data);
        let mut check = kind.to_vec();
        check.extend_from_slice(data);
        chunk.extend_from_slice(&crc32(&check).to_be_bytes());
        chunk
    }

    /// Insert `chunk` after IHDR in the PNG at `path`, in place.
    fn insert_png_chunk_after_ihdr(path: &std::path::Path, chunk: &[u8]) {
        let bytes = std::fs::read(path).expect("fixture PNG should read");
        assert_eq!(&bytes[12..16], b"IHDR", "fixture must be a PNG");
        let mut out = bytes[..33].to_vec();
        out.extend_from_slice(chunk);
        out.extend_from_slice(&bytes[33..]);
        std::fs::write(path, out).expect("chunked PNG should write");
    }

    /// Minimal little-endian TIFF header with IFD0 carrying Orientation.
    fn tiff_orientation(orientation: u8) -> Vec<u8> {
        let mut header = b"II*\0\x08\0\0\0".to_vec();
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&0x0112_u16.to_le_bytes());
        header.extend_from_slice(&3_u16.to_le_bytes());
        header.extend_from_slice(&1_u32.to_le_bytes());
        header.extend_from_slice(&[orientation, 0, 0, 0]);
        header.extend_from_slice(&0_u32.to_le_bytes());
        header
    }

    /// Inject a JPEG APP1 EXIF orientation segment after SOI, in place.
    fn inject_jpeg_orientation(path: &std::path::Path, orientation: u8) {
        let bytes = std::fs::read(path).expect("fixture JPEG should read");
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "fixture must be a JPEG");
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff_orientation(orientation));
        let mut out = bytes[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&u16::try_from(app1.len() + 2).unwrap().to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&bytes[2..]);
        std::fs::write(path, out).expect("oriented JPEG should write");
    }

    /// MO1 R7: PNG and JPEG stills probe as `Image` with nominal fps and a
    /// one-frame duration; untagged files carry `unknown()` colour (the
    /// decoder's GBR-matrix / full-range defaults are pixel-format facts,
    /// not file tagging).
    #[test]
    fn probe_classifies_png_and_jpeg_stills() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-stills");
        let png = still_fixture(
            &directory,
            "red.png",
            "color=c=red:size=64x36:rate=1:duration=1",
            "png",
        );
        let jpeg = still_fixture(
            &directory,
            "blue.jpg",
            "color=c=blue:size=64x36:rate=1:duration=1",
            "mjpeg",
        );

        for path in [&png, &jpeg] {
            let asset = probe_path(path, AssetId(1))
                .unwrap_or_else(|error| panic!("{} should probe: {error}", path.display()));
            assert_eq!(asset.kind, MediaKind::Image, "{}", path.display());
            assert_eq!(asset.fps, Rational::default(), "{}", path.display());
            assert_eq!(asset.duration, TimeCode(1), "{}", path.display());
            assert_eq!(asset.resolution, Some((64, 36)), "{}", path.display());
            assert_eq!(
                asset.color_description,
                ColorDescription::unknown(),
                "untagged {} must enter the incident path as unknown",
                path.display()
            );
        }
    }

    /// MO1 R7: a tagged still keeps the decoder description exactly as
    /// video does — only untagged stills collapse to `unknown()`.
    #[test]
    fn probe_takes_decoder_colorimetry_for_tagged_stills() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-tagged-still");
        let path = still_fixture(
            &directory,
            "red.png",
            "color=c=red:size=64x36:rate=1:duration=1",
            "png",
        );
        insert_png_chunk_after_ihdr(&path, &png_chunk(*b"sRGB", &[0]));

        let asset = probe_path(&path, AssetId(1)).expect("tagged still should probe");
        assert_eq!(asset.kind, MediaKind::Image);
        assert_eq!(asset.color_description.primaries, ColorPrimaries::Bt709);
        assert_eq!(asset.color_description.transfer, ColorTransfer::Srgb);
        assert_ne!(asset.color_description, ColorDescription::unknown());
    }

    /// MO1 R7: stills over 8192 px on either axis are refused at probe with
    /// the named limit and a recovery; 8192 itself probes.
    #[test]
    fn probe_refuses_stills_over_8192_pixels() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-still-limit");
        let big = still_fixture(
            &directory,
            "big.png",
            "color=c=gray:size=8193x16:rate=1:duration=1",
            "png",
        );
        let error = probe_path(&big, AssetId(1)).expect_err("8193 px must refuse");
        let message = error.to_string();
        assert!(
            message.contains("8192"),
            "the refusal must name the limit: {message}"
        );
        assert!(
            message.contains("downscale"),
            "the refusal must carry a recovery: {message}"
        );

        let edge = still_fixture(
            &directory,
            "edge.png",
            "color=c=gray:size=8192x16:rate=1:duration=1",
            "png",
        );
        let asset = probe_path(&edge, AssetId(1)).expect("8192 px must probe");
        assert_eq!(asset.kind, MediaKind::Image);
        assert_eq!(asset.resolution, Some((8192, 16)));
    }

    /// MO1 R7: a still codec beside an audio stream stays `AudioVideo`,
    /// probed as a one-frame video exactly as before — the audio never
    /// strands on a kind the segment walk skips.
    #[test]
    fn probe_keeps_audio_beside_a_still_codec_stream() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-still-audio");
        let jpeg = still_fixture(
            &directory,
            "blue.jpg",
            "color=c=blue:size=64x36:rate=1:duration=1",
            "mjpeg",
        );
        let muxed = directory.path("motion.mkv");
        crate::test_support::run_ffmpeg(
            &[
                "-i",
                jpeg.to_str().unwrap(),
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-c:v",
                "copy",
                "-c:a",
                "aac",
                "-shortest",
            ],
            &muxed,
        );

        let asset = probe_path(&muxed, AssetId(1)).expect("muxed still+audio should probe");
        assert_eq!(asset.kind, MediaKind::AudioVideo);
    }

    /// MO1 R7: an oriented still probes its DISPLAY dimensions.
    #[test]
    fn probe_reports_display_dimensions_for_oriented_stills() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-oriented-still");
        let path = still_fixture(
            &directory,
            "blue.jpg",
            "color=c=blue:size=64x36:rate=1:duration=1",
            "mjpeg",
        );
        inject_jpeg_orientation(&path, 6);

        let asset = probe_path(&path, AssetId(1)).expect("oriented still should probe");
        assert_eq!(asset.kind, MediaKind::Image);
        assert_eq!(
            asset.resolution,
            Some((36, 64)),
            "a 90° orientation swaps the probed dimensions"
        );
    }

    /// Decode one still frame through the legacy path (RGBA8) for content
    /// pins without managed-colour conversion in the way.
    fn decode_still_legacy(path: &std::path::Path) -> FrameTexture {
        let mut decoder =
            VideoDecoder::open(path, Rational::default()).expect("still should open to decode");
        let mut cache: FrameCache<FrameTexture> = FrameCache::new(2);
        decoder
            .decode_window(TimeCode::ZERO, TimeCode::ZERO, &mut cache)
            .expect("still frame should decode");
        cache
            .frame_at_or_before(TimeCode::ZERO)
            .expect("still frame should be cached")
    }

    fn still_pixel(frame: &FrameTexture, x: u32, y: u32) -> &[u8] {
        let index = usize::try_from((y * frame.width + x) * 4).unwrap();
        &frame.rgba[index..index + 4]
    }

    /// MO1 R7: EXIF orientation applies at decode. The fixture's vertical
    /// edge (left black, right white) rotates to a horizontal one: with
    /// orientation 6 (90° CW) display (x, y) = stored (y, 35 − x), so rows
    /// 0..32 decode black and rows 32..64 white; orientation 2 mirrors in
    /// place; the transpose (5) flips first, inverting the horizontal edge.
    #[test]
    fn still_decode_applies_exif_orientation() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("decode-oriented-still");
        let source =
            "color=c=black:size=64x36:rate=1:duration=1,drawbox=x=32:y=0:w=32:h=36:c=white:t=fill";

        let rotated = still_fixture(&directory, "edge.png", source, "png");
        insert_png_chunk_after_ihdr(&rotated, &png_chunk(*b"eXIf", &tiff_orientation(6)));
        let frame = decode_still_legacy(&rotated);
        assert_eq!((frame.width, frame.height), (36, 64));
        assert!(still_pixel(&frame, 0, 0)[0] < 56, "top-left must be black");
        assert!(
            still_pixel(&frame, 35, 0)[0] < 56,
            "top-right must be black"
        );
        assert!(still_pixel(&frame, 0, 31)[0] < 56, "row 31 must be black");
        assert!(still_pixel(&frame, 0, 32)[0] > 200, "row 32 must be white");
        assert!(
            still_pixel(&frame, 35, 63)[0] > 200,
            "bottom-right must be white"
        );

        let mirrored = still_fixture(&directory, "mirror.png", source, "png");
        insert_png_chunk_after_ihdr(&mirrored, &png_chunk(*b"eXIf", &tiff_orientation(2)));
        let frame = decode_still_legacy(&mirrored);
        assert_eq!((frame.width, frame.height), (64, 36));
        assert!(
            still_pixel(&frame, 0, 18)[0] > 200,
            "mirrored left must be white"
        );
        assert!(
            still_pixel(&frame, 63, 18)[0] < 56,
            "mirrored right must be black"
        );

        let transposed = still_fixture(&directory, "transpose.png", source, "png");
        insert_png_chunk_after_ihdr(&transposed, &png_chunk(*b"eXIf", &tiff_orientation(5)));
        let frame = decode_still_legacy(&transposed);
        assert_eq!((frame.width, frame.height), (36, 64));
        assert!(
            still_pixel(&frame, 0, 0)[0] > 200,
            "transposed top-left must be white"
        );
        assert!(
            still_pixel(&frame, 35, 0)[0] > 200,
            "transposed top-right must be white"
        );
        assert!(
            still_pixel(&frame, 0, 63)[0] < 56,
            "transposed bottom-left must be black"
        );
        assert!(
            still_pixel(&frame, 35, 63)[0] < 56,
            "transposed bottom-right must be black"
        );
    }

    /// MO1 R7: the mirrored orientations compose flip-before-rotation
    /// exactly, at the byte level.
    #[test]
    fn flip_before_rotation_composes_exactly() {
        // 2 × 1 stored: [A][B]. EXIF 5 = flip (→ [B][A]) then 90° CW,
        // which stacks the row into a 1 × 2 column, B over A.
        let flipped = flip_optional(true, 2, 1, 1, vec![10, 20]).expect("flip should apply");
        assert_eq!(flipped, vec![20, 10]);
        let (width, height, pixels) = rotate_bytes(VideoRotation::Clockwise90, 2, 1, 1, flipped)
            .expect("rotation should apply");
        assert_eq!((width, height), (1, 2));
        assert_eq!(pixels, vec![20, 10]);

        // No flip passes the buffer through untouched (same allocation).
        let pixels = vec![10, 20, 30, 40];
        let passed = flip_optional(false, 2, 2, 1, pixels).expect("pass-through");
        assert_eq!(passed, vec![10, 20, 30, 40]);

        // Truncated buffers refuse instead of panicking.
        assert!(flip_optional(true, 2, 2, 1, vec![1, 2, 3]).is_err());
    }

    /// MO1 R7: PNG alpha survives the still decode — the hold path composites
    /// it, so the decoder must not flatten it.
    #[test]
    fn still_decode_preserves_png_alpha() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("decode-still-alpha");
        let path = still_fixture(
            &directory,
            "half.png",
            "color=c=red@0.5:size=64x36:rate=1:duration=1,format=rgba",
            "png",
        );
        let frame = decode_still_legacy(&path);
        assert_eq!((frame.width, frame.height), (64, 36));
        let alpha = still_pixel(&frame, 32, 18)[3];
        assert!(
            (120..=140).contains(&alpha),
            "half alpha must survive decode, got {alpha}"
        );
    }

    /// MO1 R30: a still-probe failure is the existing `Backend` incident —
    /// no new code is minted for stills.
    #[test]
    fn still_probe_failure_rides_backend_unclassified() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("probe-still-garbage");
        let path = directory.path("garbage.png");
        std::fs::write(&path, b"not a still file at all").expect("garbage should write");

        let error = probe_path(&path, AssetId(1)).expect_err("garbage must not probe");
        assert!(
            matches!(error, MediaError::Backend(_)),
            "a still-probe failure must stay Backend, got {error:?}"
        );
        let observation = kinewright_core::IncidentObservation::from_media_error(
            &error,
            kinewright_core::IncidentSubject::Project,
            kinewright_core::TimelineRevision(1),
        );
        assert_eq!(
            observation.code,
            kinewright_core::IncidentCode::Media(
                kinewright_core::MediaIncident::BackendUnclassified
            )
        );
    }

    /// MO1 R30: a 16-bit still under an 8-bit managed description refuses
    /// with the existing `UnsupportedDecoderFormat` — no new media incident.
    #[test]
    fn still_depth_mismatch_is_unsupported_decoder_format() {
        crate::initialize_ffmpeg().expect("FFmpeg should initialize for generated media");
        let directory = crate::test_support::TempDirectory::new("decode-still-depth");
        let path = still_fixture(
            &directory,
            "deep.png",
            "color=c=red:size=64x36:rate=1:duration=1,format=rgb48le",
            "png",
        );
        let asset = probe_path(&path, AssetId(1)).expect("the 16-bit still should probe");
        assert_eq!(asset.kind, MediaKind::Image);

        // A valid 8-bit stamp passes classification, so the refusal is the
        // decoder's 16-bit depth, not the tagging.
        let description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Srgb,
            matrix: ColorMatrix::Rgb,
            range: ColorRange::Full,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::UserOverride,
        };
        let Err(error) =
            VideoDecoder::open_scaled_managed(&path, Rational::default(), None, &description, None)
        else {
            panic!("16-bit pixels under an 8-bit stamp must refuse");
        };
        assert_eq!(error.recovery_code(), Some("unsupported_decoder_format"));
        let MediaError::UnsupportedDecoderFormat {
            declared_bit_depth,
            decoder_bit_depth,
            ..
        } = error
        else {
            panic!("the refusal must stay UnsupportedDecoderFormat, got {error:?}");
        };
        assert_eq!(declared_bit_depth, Some(8));
        assert_eq!(decoder_bit_depth, Some(16));
    }
}
