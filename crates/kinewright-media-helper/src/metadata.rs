use ffmpeg_next as ffmpeg;
use kinewright_core::{
    ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange,
    ColorTransfer, ColorWhitePoint, MediaError, Rational,
};
const COLOR_COVERAGE_PER_FIELD_BASIS_POINTS: u16 = 2000;
const AV_TIME_BASE: i64 = 1_000_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoRotation {
    None,
    Clockwise90,
    HalfTurn,
    Clockwise270,
}
impl VideoRotation {
    pub(crate) fn display_dimensions(self, w: u32, h: u32) -> (u32, u32) {
        match self {
            Self::Clockwise90 | Self::Clockwise270 => (h, w),
            _ => (w, h),
        }
    }
}
pub(crate) fn color_description_from_decoder(
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

#[allow(
    clippy::too_many_lines,
    reason = "preserve the existing pixel-format interpretation table"
)]
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

pub(crate) fn duration_us_to_frames(duration: i64, fps: Rational) -> i64 {
    if duration <= 0 {
        return 0;
    }
    mul_div_ceil(
        duration,
        i64::from(fps.numerator()),
        AV_TIME_BASE.saturating_mul(i64::from(fps.denominator())),
    )
}

pub(crate) fn timestamp_to_grid_ceil(
    timestamp: i64,
    time_base: ffmpeg::Rational,
    fps: Rational,
) -> i64 {
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

pub(crate) fn stream_rotation(stream: &ffmpeg::Stream<'_>) -> Result<VideoRotation, MediaError> {
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

pub(crate) fn is_still_image_codec(id: ffmpeg::codec::Id) -> bool {
    matches!(id, ffmpeg::codec::Id::PNG | ffmpeg::codec::Id::MJPEG)
}
