//! Coarse luma grids, normalised to 0..1, from decoded frames of any depth
//! and range and from RGBA source frames.
use ffmpeg_next as ffmpeg;
use kinewright_core::MediaError;

use super::LUMA_GRID;

pub(super) const CELLS: usize = LUMA_GRID.0 * LUMA_GRID.1;

/// How to read normalised luma from one sample of the first plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LumaLayout {
    /// Bytes per sample: 1 or 2 (little endian).
    pub bytes: usize,
    /// Right shift of a 2-byte sample to its code (P010 stores 10 bits high).
    pub shift: u32,
    pub bits: u32,
}

/// The luma plane layout of a YUV format, if it has one.
pub(super) fn layout(format: ffmpeg::format::Pixel) -> Option<LumaLayout> {
    use ffmpeg::format::Pixel;
    let eight = LumaLayout {
        bytes: 1,
        shift: 0,
        bits: 8,
    };
    let wide = |bits: u32, shift: u32| LumaLayout {
        bytes: 2,
        shift,
        bits,
    };
    Some(match format {
        Pixel::YUV420P
        | Pixel::YUVJ420P
        | Pixel::YUV422P
        | Pixel::YUVJ422P
        | Pixel::YUV444P
        | Pixel::YUVJ444P
        | Pixel::YUV440P
        | Pixel::YUVJ440P
        | Pixel::YUV411P
        | Pixel::YUV410P
        | Pixel::NV12
        | Pixel::NV21
        | Pixel::NV16
        | Pixel::GRAY8 => eight,
        Pixel::YUV420P10LE | Pixel::YUV422P10LE | Pixel::YUV444P10LE | Pixel::GRAY10LE => {
            wide(10, 0)
        }
        Pixel::YUV420P12LE | Pixel::YUV422P12LE | Pixel::YUV444P12LE | Pixel::GRAY12LE => {
            wide(12, 0)
        }
        Pixel::YUV420P16LE | Pixel::YUV422P16LE | Pixel::YUV444P16LE | Pixel::GRAY16LE => {
            wide(16, 0)
        }
        // Semi-planar high depth keeps its code in the high bits.
        Pixel::P010LE => wide(10, 6),
        Pixel::P016LE => wide(16, 0),
        _ => return None,
    })
}

fn full_range(frame: &ffmpeg::frame::Video) -> bool {
    use ffmpeg::format::Pixel;
    use ffmpeg::util::color::Range;
    match frame.color_range() {
        Range::JPEG => true,
        Range::MPEG => false,
        Range::Unspecified => matches!(
            frame.format(),
            Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P | Pixel::YUVJ440P
        ),
    }
}

/// Box averages of `sample(x, y)` over the grid.
fn boxed(width: usize, height: usize, mut sample: impl FnMut(usize, usize) -> f32) -> Vec<f32> {
    let (columns, rows) = LUMA_GRID;
    let mut sums = vec![(0.0_f64, 0_u32); CELLS];
    for y in 0..height {
        let row = y * rows / height;
        for x in 0..width {
            let cell = &mut sums[row * columns + x * columns / width];
            cell.0 += f64::from(sample(x, y));
            cell.1 += 1;
        }
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a mean of 0..1 samples fits f32"
    )]
    sums.into_iter()
        .map(|(sum, count)| {
            if count == 0 {
                0.0
            } else {
                (sum / f64::from(count)) as f32
            }
        })
        .collect()
}

/// Luma grid of a decoded frame. YUV formats read the luma plane at their
/// own depth and range; other formats are converted to RGBA first.
pub(super) fn from_frame(
    frame: &ffmpeg::frame::Video,
    rgba: &mut RgbaScaler,
) -> Result<Vec<f32>, MediaError> {
    let width = usize::try_from(frame.width()).unwrap_or(0);
    let height = usize::try_from(frame.height()).unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(MediaError::Backend("review frame has no picture".into()));
    }
    let Some(layout) = layout(frame.format()) else {
        return rgba.grid(frame);
    };
    let stride = frame.stride(0);
    let data = frame.data(0);
    if data.len() < stride * (height - 1) + width * layout.bytes {
        return Err(MediaError::Backend("review frame plane is short".into()));
    }
    let scale = f32::from(1_u16 << (layout.bits.min(16) - 8).min(8));
    let (black, span) = if full_range(frame) {
        let max = 2_f32.powi(i32::try_from(layout.bits).unwrap_or(8)) - 1.0;
        (0.0, max)
    } else {
        (16.0 * scale, 219.0 * scale)
    };
    Ok(boxed(width, height, |x, y| {
        let at = y * stride + x * layout.bytes;
        let code = if layout.bytes == 1 {
            f32::from(data[at])
        } else {
            f32::from(u16::from_le_bytes([data[at], data[at + 1]]) >> layout.shift)
        };
        ((code - black) / span).clamp(0.0, 1.0)
    }))
}

/// BT.709 luma of 8-bit gamma-encoded RGB.
fn rgb_luma(red: u8, green: u8, blue: u8) -> f32 {
    (0.2126 * f32::from(red) + 0.7152 * f32::from(green) + 0.0722 * f32::from(blue)) / 255.0
}

/// Luma grid of a tightly packed RGBA picture.
pub(super) fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<f32>> {
    let width = usize::try_from(width).ok()?;
    let height = usize::try_from(height).ok()?;
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return None;
    }
    Some(boxed(width, height, |x, y| {
        let at = (y * width + x) * 4;
        rgb_luma(rgba[at], rgba[at + 1], rgba[at + 2])
    }))
}

/// Converts frames without a luma plane to small RGBA pictures.
#[derive(Default)]
pub(super) struct RgbaScaler {
    context: Option<(
        ffmpeg::format::Pixel,
        u32,
        u32,
        ffmpeg::software::scaling::Context,
    )>,
}

impl RgbaScaler {
    fn grid(&mut self, frame: &ffmpeg::frame::Video) -> Result<Vec<f32>, MediaError> {
        let backend = |error: ffmpeg::Error| MediaError::Backend(error.to_string());
        let key = (frame.format(), frame.width(), frame.height());
        let width = frame.width().clamp(1, 128);
        let height = (u64::from(frame.height()) * u64::from(width)
            / u64::from(frame.width().max(1)))
        .clamp(1, 128);
        let height = u32::try_from(height).unwrap_or(1);
        if self
            .context
            .as_ref()
            .is_none_or(|(format, w, h, _)| (*format, *w, *h) != key)
        {
            let context = ffmpeg::software::scaling::Context::get(
                key.0,
                key.1,
                key.2,
                ffmpeg::format::Pixel::RGBA,
                width,
                height,
                ffmpeg::software::scaling::Flags::AREA,
            )
            .map_err(backend)?;
            self.context = Some((key.0, key.1, key.2, context));
        }
        let (_, _, _, context) = self.context.as_mut().expect("scaler set above");
        let mut scaled = ffmpeg::frame::Video::empty();
        context.run(frame, &mut scaled).map_err(backend)?;
        let stride = scaled.stride(0);
        let row_bytes = usize::try_from(width).unwrap_or(0) * 4;
        let mut packed = Vec::with_capacity(row_bytes * usize::try_from(height).unwrap_or(0));
        for row in 0..usize::try_from(height).unwrap_or(0) {
            packed.extend_from_slice(&scaled.data(0)[row * stride..row * stride + row_bytes]);
        }
        from_rgba(width, height, &packed)
            .ok_or_else(|| MediaError::Backend("review frame conversion was empty".into()))
    }
}

/// Per-frame comparison statistics over unmasked cells.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Stats {
    pub mean: f32,
    pub max: f32,
    pub std: f32,
}

pub(super) fn stats(cells: &[f32], masked: &[bool]) -> Stats {
    let mut count = 0_u32;
    let mut sum = 0.0_f64;
    let mut max = 0.0_f32;
    for (index, &value) in cells.iter().enumerate() {
        if masked.get(index).copied().unwrap_or(false) {
            continue;
        }
        count += 1;
        sum += f64::from(value);
        max = max.max(value);
    }
    if count == 0 {
        return Stats::default();
    }
    let mean = sum / f64::from(count);
    let variance = cells
        .iter()
        .enumerate()
        .filter(|(index, _)| !masked.get(*index).copied().unwrap_or(false))
        .map(|(_, &value)| (f64::from(value) - mean).powi(2))
        .sum::<f64>()
        / f64::from(count);
    #[allow(clippy::cast_possible_truncation, reason = "0..1 statistics")]
    Stats {
        mean: mean as f32,
        max,
        std: variance.sqrt() as f32,
    }
}

/// Mean absolute difference of two grids over unmasked cells.
pub(super) fn difference(a: &[f32], b: &[f32], masked: &[bool]) -> f32 {
    let mut count = 0_u32;
    let mut sum = 0.0_f32;
    for (index, (x, y)) in a.iter().zip(b).enumerate() {
        if masked.get(index).copied().unwrap_or(false) {
            continue;
        }
        count += 1;
        sum += (x - y).abs();
    }
    if count == 0 {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss, reason = "at most 576 cells")]
        let count = count as f32;
        sum / count
    }
}

/// Pearson correlation over unmasked cells; `None` when either is flat.
pub(super) fn correlation(a: &[f32], b: &[f32], masked: &[bool], flat: f32) -> Option<f32> {
    let pairs: Vec<(f64, f64)> = a
        .iter()
        .zip(b)
        .enumerate()
        .filter(|(index, _)| !masked.get(*index).copied().unwrap_or(false))
        .map(|(_, (x, y))| (f64::from(*x), f64::from(*y)))
        .collect();
    #[allow(clippy::cast_precision_loss, reason = "at most 576 cells")]
    let count = pairs.len() as f64;
    if pairs.len() < 2 {
        return None;
    }
    let (mean_a, mean_b) = pairs.iter().fold((0.0, 0.0), |(sa, sb), (x, y)| {
        (sa + x / count, sb + y / count)
    });
    let (mut covariance, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
    for (x, y) in &pairs {
        covariance += (x - mean_a) * (y - mean_b);
        var_a += (x - mean_a).powi(2);
        var_b += (y - mean_b).powi(2);
    }
    let flat = f64::from(flat).powi(2) * count;
    if var_a < flat || var_b < flat {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, reason = "a correlation in -1..1")]
    Some((covariance / (var_a * var_b).sqrt()) as f32)
}
