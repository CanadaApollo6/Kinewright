use std::{path::Path, sync::Arc};

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    AssetId, ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance,
    ColorRange, ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, ContentLightLevel,
    FrameTexture, HdrStaticMetadata, MasteringDisplayChromaticity, MasteringDisplayMetadata,
    MediaAsset, MediaError, MediaKind, Rational, RgbaImage, TimeCode,
    cc8_hdr::{
        CC8_AV_CONTENT_LIGHT_METADATA_BYTES, CC8_AV_MASTERING_DISPLAY_METADATA_BYTES,
        CC8_MASTERING_DISPLAY_CHROMATICITY_INCREMENTS_PER_UNIT,
        CC8_MASTERING_DISPLAY_LUMINANCE_TEN_THOUSANDTHS_PER_NIT, cc8_st2086_units,
    },
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

    let kind = match (video.is_some(), audio.is_some()) {
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

    let (fps, resolution, duration, color_description) = if let Some(stream) = video {
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
        // FFmpeg may leave the opened decoder's pixel format unresolved until
        // it sees a packet. Decode one frame through the safe ffmpeg-next API
        // on a short-lived second input so probing can still report the source
        // component depth without consuming the input used for duration data.
        let (negotiated_pixel_format, frame_hdr_static_metadata) =
            negotiated_frame_facts(path, stream.index());
        let color_description = color_description_from_decoder(
            &decoder,
            negotiated_pixel_format,
            hdr_static_metadata_for_stream(&stream, frame_hdr_static_metadata),
        );
        let stream_duration = timestamp_to_grid_ceil(stream.duration(), stream.time_base(), rate);
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
        (rate, Some(resolution), duration, color_description)
    } else {
        let rate = Rational::default();
        (
            rate,
            None,
            TimeCode(duration_us_to_frames(input.duration(), rate)),
            ColorDescription::unknown(),
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
    })
}

fn color_description_from_decoder(
    decoder: &ffmpeg::codec::decoder::Video,
    negotiated_pixel_format: Option<ffmpeg::format::Pixel>,
    hdr_static_metadata: HdrStaticMetadata,
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
        // FFmpeg exposes primaries, transfer, matrix, and range here, but not
        // a white point. Keep this unknown rather than mixing an inferred
        // value into a description whose provenance is stream metadata.
        white_point: ColorWhitePoint::Unknown,
        bit_depth,
        confidence_basis_points,
        provenance,
        // CC8 §2.4, read where the container or the coded stream carried it.
        // It deliberately does **not** feed `color_metadata_coverage`: the
        // confidence CC1 defined is coverage of the four colorimetry fields
        // plus the depth, and a file that also declares a mastering display is
        // not better described in that sense. Moving that number would move a
        // CC1 figure §9.1 fixture 6 holds unchanged.
        hdr_static_metadata,
    }
}

/// CC8 §2.4's static HDR metadata, read from the two places a source can
/// declare it.
///
/// §2.4: "Mastering-display primaries and MaxCLL/MaxFALL are read on probe
/// **where the container carries them**, stored on the source description with
/// provenance, and reported."
///
/// There are two such places, and they are distinguished by provenance rather
/// than merged:
///
/// * the container's own boxes (MP4 `mdcv`/`clli`, Matroska's colour element),
///   which `FFmpeg` exposes as **coded stream side data** —
///   [`ColorProvenance::ContainerMetadata`]; and
/// * an SEI message inside the bitstream, which reaches the **decoded frame** —
///   [`ColorProvenance::StreamMetadata`].
///
/// The container is read first and wins, per block, because a remux that
/// rewrote the boxes is the more recent statement about the file; a block the
/// container did not carry is then taken from the frame. Neither is invented,
/// and a source that declares neither keeps [`HdrStaticMetadata::unknown`].
fn hdr_static_metadata_for_stream(
    stream: &ffmpeg::format::stream::Stream<'_>,
    frame_metadata: HdrStaticMetadata,
) -> HdrStaticMetadata {
    let mut container = HdrStaticMetadata::unknown();
    for side_data in stream.side_data() {
        match side_data.kind() {
            ffmpeg::codec::packet::side_data::Type::MasteringDisplayMetadata => {
                container.mastering_display =
                    parse_mastering_display(side_data.data(), ColorProvenance::ContainerMetadata);
            }
            ffmpeg::codec::packet::side_data::Type::ContentLightLevel => {
                container.content_light_level =
                    parse_content_light_level(side_data.data(), ColorProvenance::ContainerMetadata);
            }
            _ => {}
        }
    }
    merge_hdr_static_metadata(container, frame_metadata)
}

/// Combine the container's declaration with the bitstream's, per block.
///
/// Split out of [`hdr_static_metadata_for_stream`] so the rule is testable
/// without a container that carries the boxes. **The pinned build's libx264
/// writes its mastering display and content light level as an SEI only**, and
/// neither the MP4 nor the Matroska muxer promotes that to a container box on
/// a stream copy, so no fixture in this repository can produce container-level
/// side data with the encoders CC8 ships (§0.2 Q2 keeps libx265, whose
/// `hdr-opt` route §0.3(a) measured, out of the delivered set). The parsers and
/// this merge are therefore exercised by `decode::tests` on constructed
/// buffers, and CC8 §9.1 fixture 11's media half exercises the SEI path
/// end to end on real media.
fn merge_hdr_static_metadata(
    container: HdrStaticMetadata,
    frame: HdrStaticMetadata,
) -> HdrStaticMetadata {
    HdrStaticMetadata {
        mastering_display: container.mastering_display.or(frame.mastering_display),
        content_light_level: container.content_light_level.or(frame.content_light_level),
    }
}

/// CC8 §2.4's two blocks as one decoded frame declares them.
fn hdr_static_metadata_from_frame(frame: &ffmpeg::frame::Video) -> HdrStaticMetadata {
    HdrStaticMetadata {
        mastering_display: frame
            .side_data(ffmpeg::frame::side_data::Type::MasteringDisplayMetadata)
            .and_then(|side_data| {
                parse_mastering_display(side_data.data(), ColorProvenance::StreamMetadata)
            }),
        content_light_level: frame
            .side_data(ffmpeg::frame::side_data::Type::ContentLightLevel)
            .and_then(|side_data| {
                parse_content_light_level(side_data.data(), ColorProvenance::StreamMetadata)
            }),
    }
}

/// Decode libavutil's `AVMasteringDisplayMetadata` from a side-data buffer.
///
/// The buffer **is** the C struct, so this reads its twenty-two `int` fields at
/// their fixed offsets: `display_primaries[3][2]` and `white_point[2]` as
/// `AVRational` numerator/denominator pairs, then `min_luminance`,
/// `max_luminance`, `has_primaries` and `has_luminance`. The workspace forbids
/// `unsafe`, so the fields are read as native-endian integers out of the byte
/// slice rather than through a pointer cast; the exact length check against
/// [`CC8_AV_MASTERING_DISPLAY_METADATA_BYTES`] is what makes that sound, and a
/// buffer of any other length is reported as `Unknown` rather than decoded on a
/// guess.
///
/// `None` — no declaration this build can honestly record — is returned when:
/// the length is not the struct's; either `has_*` flag is clear, because CC8
/// stores a mastering display or nothing rather than half of one and §2.4
/// forbids inventing the other half; or a declared rational cannot be written
/// in ST 2086's own units exactly ([`cc8_st2086_units`]).
fn parse_mastering_display(
    bytes: &[u8],
    provenance: ColorProvenance,
) -> Option<MasteringDisplayMetadata> {
    if bytes.len() != CC8_AV_MASTERING_DISPLAY_METADATA_BYTES {
        return None;
    }
    let field = |index: usize| -> i64 {
        let start = index * 4;
        let mut buffer = [0_u8; 4];
        buffer.copy_from_slice(&bytes[start..start + 4]);
        i64::from(i32::from_ne_bytes(buffer))
    };
    // `has_primaries` and `has_luminance`, the struct's last two ints.
    if field(20) == 0 || field(21) == 0 {
        return None;
    }
    let chromaticity = |pair: usize| -> Option<MasteringDisplayChromaticity> {
        Some(MasteringDisplayChromaticity {
            x_increments: cc8_st2086_units(
                field(pair * 4),
                field(pair * 4 + 1),
                CC8_MASTERING_DISPLAY_CHROMATICITY_INCREMENTS_PER_UNIT,
            )?,
            y_increments: cc8_st2086_units(
                field(pair * 4 + 2),
                field(pair * 4 + 3),
                CC8_MASTERING_DISPLAY_CHROMATICITY_INCREMENTS_PER_UNIT,
            )?,
        })
    };
    Some(MasteringDisplayMetadata {
        // `display_primaries` is documented "(r, g, b order)".
        red: chromaticity(0)?,
        green: chromaticity(1)?,
        blue: chromaticity(2)?,
        white_point: chromaticity(3)?,
        min_luminance_ten_thousandths_nits: cc8_st2086_units(
            field(16),
            field(17),
            CC8_MASTERING_DISPLAY_LUMINANCE_TEN_THOUSANDTHS_PER_NIT,
        )?,
        max_luminance_ten_thousandths_nits: cc8_st2086_units(
            field(18),
            field(19),
            CC8_MASTERING_DISPLAY_LUMINANCE_TEN_THOUSANDTHS_PER_NIT,
        )?,
        provenance,
    })
}

/// Decode libavutil's `AVContentLightMetadata` — `MaxCLL`, `MaxFALL` — from a
/// side-data buffer, on [`parse_mastering_display`]'s terms.
fn parse_content_light_level(
    bytes: &[u8],
    provenance: ColorProvenance,
) -> Option<ContentLightLevel> {
    if bytes.len() != CC8_AV_CONTENT_LIGHT_METADATA_BYTES {
        return None;
    }
    let field = |index: usize| -> u32 {
        let start = index * 4;
        let mut buffer = [0_u8; 4];
        buffer.copy_from_slice(&bytes[start..start + 4]);
        u32::from_ne_bytes(buffer)
    };
    Some(ContentLightLevel {
        max_cll_nits: field(0),
        max_fall_nits: field(1),
        provenance,
    })
}

/// Return the facts only a decoded frame can supply: the negotiated pixel
/// format, and CC8 §2.4's static HDR metadata as the **bitstream** declared it.
///
/// Some codecs leave `AVCodecContext::pix_fmt` at `AV_PIX_FMT_NONE` until the
/// first decoded frame. Probe-time color metadata uses the negotiated frame
/// format when available and preserves `Unknown` when the stream never yields
/// one.
///
/// The frame is where an SEI-carried mastering display or content light level
/// arrives, so §2.4's second source is read from the same single decoded frame
/// this function already produces rather than from a second decode pass. A
/// stream that never yields a frame reports no metadata, exactly as it reports
/// no pixel format.
fn negotiated_frame_facts(
    path: &Path,
    stream_index: usize,
) -> (Option<ffmpeg::format::Pixel>, HdrStaticMetadata) {
    let unknown = (None, HdrStaticMetadata::unknown());
    let Ok(mut input) = media_input(path) else {
        return unknown;
    };
    let decoder = {
        let Some(stream) = input
            .streams()
            .find(|stream| stream.index() == stream_index)
        else {
            return unknown;
        };
        let Ok(context) = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        else {
            return unknown;
        };
        context.decoder().video().ok()
    };
    let Some(mut decoder) = decoder else {
        return unknown;
    };

    let mut frame = ffmpeg::frame::Video::empty();
    let facts = |frame: &ffmpeg::frame::Video| {
        (Some(frame.format()), hdr_static_metadata_from_frame(frame))
    };
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            return unknown;
        }
        match decoder.receive_frame(&mut frame) {
            Ok(()) => return facts(&frame),
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
            Err(_) => return unknown,
        }
    }

    if decoder.send_eof().is_err() {
        return unknown;
    }
    match decoder.receive_frame(&mut frame) {
        Ok(()) => facts(&frame),
        Err(_) => unknown,
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
            // FFmpeg names AV_PIX_FMT_GRAY8 `gray` (not `gray8`). Keep the
            // descriptor names explicit: this is intentionally not a broad
            // string parser, since packed and float formats must not be
            // mistaken for integer component depths.
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
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
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
    Ok(VideoPacketTiming { variable, duration })
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

/// The `buffer` source filter's `colorspace` spelling for one source matrix.
///
/// CC1 §3.1 forbids an unspecified matrix here ("no unspecified/default matrix,
/// range, transfer, or colour-space selection is allowed"), so every arm is an
/// explicit name from `buffersrc`'s own `colorspace` option table.
///
/// CC8 §10 step 4 adds the `bt2020_ncl` arm. It is unreachable from an SDR
/// source: `classify_source_with_assumption` admits only `bt709`, `rgb`, and
/// `identity` matrices for `rec709_video` / `srgb_full`, and
/// [`VideoDecoder::open_scaled_managed`] classifies before building the graph,
/// so a description that reaches this arm has already been classified as one of
/// CC8 §2.1's two HDR profiles. Every SDR description takes the same arm it took
/// before.
pub(crate) fn managed_filter_matrix(description: &ColorDescription) -> &'static str {
    match description.matrix {
        ColorMatrix::Rgb | ColorMatrix::Identity => "gbr",
        ColorMatrix::Bt2020Ncl => "bt2020nc",
        _ => "bt709",
    }
}

/// The `scale` filter's `in_color_matrix` / `out_color_matrix` spelling for the
/// same source matrix.
///
/// It is a **different vocabulary from [`managed_filter_matrix`]'s** and that is
/// a property of `FFmpeg`, not a choice: `buffersrc`'s `colorspace` option
/// enumerates `AVColorSpace` names (`bt2020nc`), while `vf_scale`'s
/// `in_color_matrix` enumerates its own shorter set (`bt2020`, which is
/// `AVCOL_SPC_BT2020_NCL = 9`). Writing one spelling into the other option
/// silently leaves the matrix unset, so the two names are derived here from the
/// single `ColorMatrix` and never copied between call sites.
pub(crate) fn managed_scale_color_matrix(description: &ColorDescription) -> &'static str {
    match description.matrix {
        ColorMatrix::Bt2020Ncl => "bt2020",
        _ => "bt709",
    }
}

fn managed_filter_is_rgb(description: &ColorDescription) -> bool {
    matches!(description.matrix, ColorMatrix::Rgb | ColorMatrix::Identity)
}

pub(crate) fn managed_filter_range(description: &ColorDescription) -> &'static str {
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
    let scale_matrix = managed_scale_color_matrix(description);
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
        // CC8 §10 step 4: `bt709` becomes `{scale_matrix}`, which is `bt709` for
        // every CC1 source and `bt2020` only for a classified CC8 §2.1 HDR
        // profile — so the SDR lane's string is byte-identical to the one it has
        // always produced. Both directions are named because CC1 §3.1 forbids
        // leaving either unspecified; the output side is RGBA64, so
        // `out_color_matrix` selects no coefficients, and naming it the lane's
        // own matrix keeps the graph from claiming a matrix it is not on.
        format!(
            "w={scaled_width}:h={scaled_height}:flags=bicubic:in_color_matrix={scale_matrix}:out_color_matrix={scale_matrix}:in_range={source_range}:out_range=jpeg"
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
        managed_source: Option<&ManagedSource>,
    ) -> Result<Self, MediaError>;
}

impl DecoderFrame for FrameTexture {
    fn from_rgba_frame(
        rgba: &ffmpeg::frame::Video,
        width: u32,
        height: u32,
        rotation: VideoRotation,
        managed_source: Option<&ManagedSource>,
    ) -> Result<Self, MediaError> {
        if managed_source.is_some() {
            return Err(MediaError::Backend(
                "legacy RGBA8 decoder output cannot carry managed source metadata".to_owned(),
            ));
        }
        let pixels = read_plane(rgba, width, height, 4)?;
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
        managed_source: Option<&ManagedSource>,
    ) -> Result<Self, MediaError> {
        let source = managed_source.ok_or_else(|| {
            MediaError::Backend(
                "managed RGBA64 decoder output is missing source colour metadata".to_owned(),
            )
        })?;
        let pixels = read_plane(rgba, width, height, 8)?;
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
        // CC8 §10 step 4: the classification is still required — §1 forbids
        // treating an unclassifiable source as Rec.709 — but a classified HDR
        // profile is no longer refused. `managed_filter_graph` now carries the
        // BT.2020 NCL source matrix into the YCbCr -> RGB conversion and
        // `WorkingFrame::from_rgba64_le` runs §3.3's HDR transfer and primaries
        // stages, so the frame that reaches the compositor is working BT.709 D65
        // linear light for both HDR and SDR sources.
        let _source =
            classify_source_with_assumption(description, assumption).map_err(|error| {
                MediaError::Backend(format!(
                    "managed source profile rejected for {} (assumption={assumption:?}): {error}",
                    path.display()
                ))
            })?;
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

    // Decoder setup owns the single managed/legacy converter decision and the
    // associated path/format diagnostics; keeping it together avoids losing
    // the typed managed-depth error at an intermediate helper boundary.
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
        let rotation = stream_rotation(&stream).map_err(|error| {
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

    // -----------------------------------------------------------------
    // CC8 §2.4: the declared static HDR metadata parsers (§10 step 9).
    // -----------------------------------------------------------------

    /// One `AVMasteringDisplayMetadata` buffer, built field by field.
    ///
    /// The layout is libavutil's: `display_primaries[3][2]` and
    /// `white_point[2]` as `AVRational` numerator/denominator pairs, then
    /// `min_luminance`, `max_luminance`, `has_primaries`, `has_luminance` —
    /// twenty-two native-endian `int`s. Building the buffer here rather than
    /// casting a struct is the same discipline the parser itself works under:
    /// the workspace forbids `unsafe`.
    fn mastering_display_bytes(fields: [i32; 22]) -> Vec<u8> {
        fields
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect()
    }

    /// §0.3(a)'s measured mastering display, in its declared rationals.
    fn declared_mastering_fields() -> [i32; 22] {
        [
            34_000, 50_000, 16_000, 50_000, // red x, y
            13_250, 50_000, 34_500, 50_000, // green x, y
            7_500, 50_000, 3_000, 50_000, // blue x, y
            15_635, 50_000, 16_450, 50_000, // white point x, y
            1, 10_000, // min luminance
            10_000_000, 10_000, // max luminance
            1, 1, // has_primaries, has_luminance
        ]
    }

    #[test]
    fn cc8_mastering_display_side_data_parses_into_st2086_units() {
        let parsed = parse_mastering_display(
            &mastering_display_bytes(declared_mastering_fields()),
            ColorProvenance::ContainerMetadata,
        )
        .expect("a well-formed buffer must parse");
        assert_eq!(parsed.red.x_increments, 34_000);
        assert_eq!(parsed.red.y_increments, 16_000);
        assert_eq!(parsed.green.x_increments, 13_250);
        assert_eq!(parsed.green.y_increments, 34_500);
        assert_eq!(parsed.blue.x_increments, 7_500);
        assert_eq!(parsed.blue.y_increments, 3_000);
        assert_eq!(parsed.white_point.x_increments, 15_635);
        assert_eq!(parsed.white_point.y_increments, 16_450);
        assert_eq!(parsed.min_luminance_ten_thousandths_nits, 1);
        assert_eq!(parsed.max_luminance_ten_thousandths_nits, 10_000_000);
        assert_eq!(parsed.provenance, ColorProvenance::ContainerMetadata);

        // A denominator that is not the standard one still parses when the
        // conversion is exact: the value is the same number, written twice.
        let mut halved = declared_mastering_fields();
        halved[0] = 17_000;
        halved[1] = 25_000;
        let parsed = parse_mastering_display(
            &mastering_display_bytes(halved),
            ColorProvenance::ContainerMetadata,
        )
        .expect("an exactly representable rational must parse");
        assert_eq!(parsed.red.x_increments, 34_000);
    }

    /// Every direction in which §2.4's "never invented" rule refuses to record
    /// a declaration.
    #[test]
    fn cc8_mastering_display_side_data_refuses_rather_than_guesses() {
        let complete = mastering_display_bytes(declared_mastering_fields());
        // The exact-length guard: this build reads a C struct out of a byte
        // slice, so a buffer of any other length is not that struct.
        for length in [0, complete.len() - 4, complete.len() + 4] {
            let mut truncated = complete.clone();
            truncated.resize(length, 0);
            assert_eq!(
                parse_mastering_display(&truncated, ColorProvenance::ContainerMetadata),
                None,
                "a {length}-byte buffer must not be decoded as a mastering display",
            );
        }

        // Either `has_*` flag clear: CC8 stores a mastering display or nothing,
        // never half of one with the other half invented.
        for index in [20, 21] {
            let mut fields = declared_mastering_fields();
            fields[index] = 0;
            assert_eq!(
                parse_mastering_display(
                    &mastering_display_bytes(fields),
                    ColorProvenance::ContainerMetadata,
                ),
                None,
            );
        }

        // A rational that cannot be written in ST 2086's units exactly
        // (`34000/3` increments is not an integer), a negative numerator, and a
        // zero denominator.
        for (index, value) in [(1, 3), (0, -1), (1, 0)] {
            let mut fields = declared_mastering_fields();
            fields[index] = value;
            assert_eq!(
                parse_mastering_display(
                    &mastering_display_bytes(fields),
                    ColorProvenance::ContainerMetadata,
                ),
                None,
                "field {index} = {value} must refuse rather than round",
            );
        }
    }

    #[test]
    fn cc8_content_light_level_side_data_parses_and_guards_its_length() {
        let bytes: Vec<u8> = [1_000_u32, 400]
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        let parsed = parse_content_light_level(&bytes, ColorProvenance::StreamMetadata)
            .expect("a well-formed buffer must parse");
        assert_eq!(parsed.max_cll_nits, 1_000);
        assert_eq!(parsed.max_fall_nits, 400);
        assert_eq!(parsed.provenance, ColorProvenance::StreamMetadata);

        for length in [0, 4, 12] {
            let mut wrong = bytes.clone();
            wrong.resize(length, 0);
            assert_eq!(
                parse_content_light_level(&wrong, ColorProvenance::StreamMetadata),
                None,
            );
        }
    }

    /// CC8 §2.4's two declaration sites, and which one wins per block.
    #[test]
    fn cc8_container_metadata_wins_per_block_over_the_bitstreams() {
        let container = HdrStaticMetadata {
            mastering_display: parse_mastering_display(
                &mastering_display_bytes(declared_mastering_fields()),
                ColorProvenance::ContainerMetadata,
            ),
            content_light_level: None,
        };
        let frame = HdrStaticMetadata {
            mastering_display: parse_mastering_display(
                &mastering_display_bytes(declared_mastering_fields()),
                ColorProvenance::StreamMetadata,
            ),
            content_light_level: Some(ContentLightLevel {
                max_cll_nits: 1_000,
                max_fall_nits: 400,
                provenance: ColorProvenance::StreamMetadata,
            }),
        };

        let merged = merge_hdr_static_metadata(container.clone(), frame.clone());
        assert_eq!(
            merged
                .mastering_display
                .as_ref()
                .map(|display| &display.provenance),
            Some(&ColorProvenance::ContainerMetadata),
            "a container declaration is the more recent statement about the file",
        );
        assert_eq!(
            merged
                .content_light_level
                .as_ref()
                .map(|light| &light.provenance),
            Some(&ColorProvenance::StreamMetadata),
            "a block the container did not carry is taken from the bitstream",
        );

        // Neither side declaring anything stays §2.4's Unknown.
        assert!(
            merge_hdr_static_metadata(HdrStaticMetadata::unknown(), HdrStaticMetadata::unknown())
                .is_unknown()
        );
        assert_eq!(
            merge_hdr_static_metadata(HdrStaticMetadata::unknown(), frame.clone()),
            frame,
        );
    }
}
