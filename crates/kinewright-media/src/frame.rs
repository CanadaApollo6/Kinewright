//! Media-owned working-frame storage for the managed SDR renderer.

use std::sync::Arc;

use half::f16;
use kinewright_core::{
    ColorMatrix, ColorRange, ColorSourceProfileAssumption, Effect, FrameTexture, MediaError,
};

use crate::color_pipeline::{
    PrimaryCorrection, decode_srgb, decode_transfer, expand_native_range, rgba64_normalization_max,
};

/// Scene-linear RGBA working pixels stored as IEEE-754 binary16 values.
///
/// Colour operations convert each value to f32 for arithmetic and only round
/// at this named storage boundary. The final public `FrameTexture` remains an
/// RGBA8 monitor image; this type never crosses that API boundary.
#[derive(Debug, Clone)]
pub(crate) struct WorkingFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Arc<Vec<f16>>,
}

impl WorkingFrame {
    pub(crate) fn from_rgba64_le(
        width: u32,
        height: u32,
        bytes: &[u8],
        description: &kinewright_core::ColorDescription,
        assumption: Option<ColorSourceProfileAssumption>,
    ) -> Result<Self, MediaError> {
        #[allow(clippy::cast_precision_loss)]
        let rgb_max = rgba64_normalization_max(description).map_err(|error| {
            MediaError::Backend(format!("managed source depth rejected: {error}"))
        })? as f32;
        let expected = pixel_count(width, height)
            .and_then(|count| count.checked_mul(8))
            .ok_or_else(|| MediaError::Backend("managed source frame is too large".to_owned()))?;
        if bytes.len() != expected {
            return Err(MediaError::Backend(format!(
                "managed RGBA64 source frame has {} bytes, expected {expected}",
                bytes.len()
            )));
        }

        let mut pixels = Vec::with_capacity(expected / 2);
        for rgba in bytes.chunks_exact(8) {
            let red = f32::from(u16::from_le_bytes([rgba[0], rgba[1]])) / rgb_max;
            let green = f32::from(u16::from_le_bytes([rgba[2], rgba[3]])) / rgb_max;
            let blue = f32::from(u16::from_le_bytes([rgba[4], rgba[5]])) / rgb_max;
            let alpha = f32::from(u16::from_le_bytes([rgba[6], rgba[7]])) / 65_535.0;
            let coded_rgb = if matches!(
                description.matrix,
                ColorMatrix::Rgb | ColorMatrix::Identity
            ) && matches!(description.range, ColorRange::Limited)
            {
                expand_native_range(
                    [red, green, blue],
                    &description.bit_depth,
                    &description.range,
                )
                .map_err(|error| {
                    MediaError::Backend(format!(
                        "managed source RGB range expansion failed (transfer={:?}, matrix={:?}, range={:?}, white_point={:?}, assumption={assumption:?}): {error}",
                        description.transfer,
                        description.matrix,
                        description.range,
                        description.white_point,
                    ))
                })?
            } else {
                [red, green, blue]
            };
            let decoded = coded_rgb
                .map(|value| decode_transfer(&description.transfer, value))
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    MediaError::Backend(format!(
                        "managed source colour decode failed (transfer={:?}, matrix={:?}, range={:?}, white_point={:?}, assumption={assumption:?}): {error}",
                        description.transfer,
                        description.matrix,
                        description.range,
                        description.white_point,
                    ))
                })?;
            pixels.extend(decoded.into_iter().map(f16::from_f32));
            pixels.push(f16::from_f32(alpha));
        }
        Ok(Self {
            width,
            height,
            pixels: Arc::new(pixels),
        })
    }

    /// Convert a display-coded RGBA8 title or compatibility frame into the
    /// same linear working representation used by managed video sources.
    pub(crate) fn from_display_frame(frame: &FrameTexture) -> Result<Self, MediaError> {
        let expected = pixel_count(frame.width, frame.height)
            .and_then(|count| count.checked_mul(4))
            .ok_or_else(|| MediaError::Backend("display source frame is too large".to_owned()))?;
        if frame.rgba.len() != expected {
            return Err(MediaError::Backend(
                "display source frame has invalid RGBA dimensions".to_owned(),
            ));
        }
        let mut pixels = Vec::with_capacity(expected);
        for rgba in frame.rgba.chunks_exact(4) {
            pixels.push(f16::from_f32(decode_srgb(f32::from(rgba[0]) / 255.0)));
            pixels.push(f16::from_f32(decode_srgb(f32::from(rgba[1]) / 255.0)));
            pixels.push(f16::from_f32(decode_srgb(f32::from(rgba[2]) / 255.0)));
            pixels.push(f16::from_f32(f32::from(rgba[3]) / 255.0));
        }
        Ok(Self {
            width: frame.width,
            height: frame.height,
            pixels: Arc::new(pixels),
        })
    }

    /// Apply every serialized CC1 node in vector order without an
    /// intermediate RGB clamp.
    #[allow(dead_code)]
    pub(crate) fn apply_primary_effects(&mut self, effects: &[Effect]) -> Result<(), MediaError> {
        let corrections = effects
            .iter()
            .filter(|effect| effect.name == "primary_correction")
            .map(PrimaryCorrection::from_effect)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                MediaError::Backend(format!("managed primary correction failed: {error}"))
            })?;
        if corrections.is_empty() {
            return Ok(());
        }
        let pixels = Arc::make_mut(&mut self.pixels);
        for rgba in pixels.chunks_exact_mut(4) {
            let mut rgb = [rgba[0].to_f32(), rgba[1].to_f32(), rgba[2].to_f32()];
            for correction in &corrections {
                rgb = correction.apply_checked(rgb).map_err(|error| {
                    MediaError::Backend(format!("managed primary correction failed: {error}"))
                })?;
            }
            rgba[0] = f16::from_f32(rgb[0]);
            rgba[1] = f16::from_f32(rgb[1]);
            rgba[2] = f16::from_f32(rgb[2]);
        }
        Ok(())
    }

    pub(crate) fn upload_bytes(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.pixels.len().saturating_mul(2)
    }
}

pub(crate) trait CachedFrame: Clone {
    fn byte_len(&self) -> usize;
}

impl CachedFrame for FrameTexture {
    fn byte_len(&self) -> usize {
        self.rgba.len()
    }
}

impl CachedFrame for WorkingFrame {
    fn byte_len(&self) -> usize {
        self.byte_len()
    }
}

fn pixel_count(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_pipeline::decode_bt709;
    use kinewright_core::{
        ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange,
        ColorTransfer, ColorWhitePoint,
    };

    fn rec709(depth: ColorBitDepth, range: ColorRange) -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range,
            white_point: ColorWhitePoint::D65,
            bit_depth: depth,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::UserOverride,
        }
    }

    fn rec709_rgb(depth: ColorBitDepth, range: ColorRange) -> ColorDescription {
        let mut description = rec709(depth, range);
        description.matrix = ColorMatrix::Rgb;
        description
    }

    fn srgb(depth: ColorBitDepth) -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Srgb,
            transfer: ColorTransfer::Srgb,
            matrix: ColorMatrix::Identity,
            range: ColorRange::Full,
            white_point: ColorWhitePoint::D65,
            bit_depth: depth,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::UserOverride,
        }
    }

    fn rgba64_bytes(rgb: &[[u16; 3]], alpha: &[u16]) -> Vec<u8> {
        assert_eq!(rgb.len(), alpha.len());
        let mut bytes = Vec::with_capacity(rgb.len() * 8);
        for (channels, alpha) in rgb.iter().zip(alpha) {
            for channel in channels {
                bytes.extend_from_slice(&channel.to_le_bytes());
            }
            bytes.extend_from_slice(&alpha.to_le_bytes());
        }
        bytes
    }

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual {actual} expected {expected} (tol {tolerance})"
        );
    }

    #[allow(clippy::cast_precision_loss)]
    #[test]
    fn full_8_bit_rgba64_ramp_uses_the_left_shifted_endpoint() {
        let rgb = (0_u16..=u16::from(u8::MAX))
            .map(|code| {
                let promoted = code << 8;
                [promoted; 3]
            })
            .collect::<Vec<_>>();
        let alpha = vec![u16::MAX; rgb.len()];
        let frame = WorkingFrame::from_rgba64_le(
            u32::try_from(rgb.len()).expect("ramp width"),
            1,
            &rgba64_bytes(&rgb, &alpha),
            &rec709(ColorBitDepth::Eight, ColorRange::Full),
            None,
        )
        .expect("8-bit full RGBA64 ramp");

        let mut previous = f32::NEG_INFINITY;
        for (code, rgba) in frame.pixels.chunks_exact(4).enumerate() {
            let expected = decode_bt709(code as f32 / f32::from(u8::MAX));
            assert_close(rgba[0].to_f32(), expected, 1.0e-3);
            assert_close(rgba[1].to_f32(), expected, 1.0e-3);
            assert_close(rgba[2].to_f32(), expected, 1.0e-3);
            assert!(rgba[0].to_f32() >= previous);
            assert_close(rgba[3].to_f32(), 1.0, 0.0);
            previous = rgba[0].to_f32();
        }
        assert_close(frame.pixels[0].to_f32(), 0.0, 0.0);
        assert_close(frame.pixels[frame.pixels.len() - 4].to_f32(), 1.0, 0.0);
    }

    #[test]
    fn full_10_bit_and_limited_10_bit_endpoints_use_their_effective_scales() {
        let full_10_rgb = [[0, 0, 0], [65_472, 65_472, 65_472]];
        let full_10 = WorkingFrame::from_rgba64_le(
            2,
            1,
            &rgba64_bytes(&full_10_rgb, &[0, u16::MAX]),
            &rec709(ColorBitDepth::Ten, ColorRange::Full),
            None,
        )
        .expect("10-bit full RGBA64 endpoints");
        assert_close(full_10.pixels[3].to_f32(), 0.0, 0.0);
        assert_close(full_10.pixels[4].to_f32(), 1.0, 1.0e-3);
        assert_close(full_10.pixels[7].to_f32(), 1.0, 0.0);

        // FFmpeg's direct limited YUV -> RGBA64 path emits the legal-white
        // endpoint as 65283 after fixed-point matrix/range rounding.  The
        // declared 10-bit source still uses the path's 8-bit nominal scale,
        // 65280, rather than the full-range 10-bit left-shift maximum 65472.
        let limited_10 = WorkingFrame::from_rgba64_le(
            3,
            1,
            &rgba64_bytes(
                &[
                    [0, 0, 0],
                    [33_387, 33_387, 33_387],
                    [65_283, 65_283, 65_283],
                ],
                &[0, u16::MAX, u16::MAX],
            ),
            &rec709(ColorBitDepth::Ten, ColorRange::Limited),
            None,
        )
        .expect("10-bit limited RGBA64 endpoints");
        assert_close(limited_10.pixels[3].to_f32(), 0.0, 0.0);
        assert_close(
            limited_10.pixels[4].to_f32(),
            decode_bt709(33_387.0 / 65_280.0),
            1.0e-3,
        );
        assert_close(
            limited_10.pixels[8].to_f32(),
            decode_bt709(65_283.0 / 65_280.0),
            1.0e-3,
        );
        assert_close(limited_10.pixels[11].to_f32(), 1.0, 0.0);
    }

    #[test]
    fn rec709_rgb_limited_range_is_expanded_after_swscale_rgb_packing() {
        // The configured swscale RGB path does not apply in_range=mpeg to
        // planar RGB. These are the observed RGBA64 values for source codes
        // 16, 128, and 235; the working-frame boundary must expand them once
        // using the declared source depth before BT.709 transfer decoding.
        let rgb8 = WorkingFrame::from_rgba64_le(
            3,
            1,
            &rgba64_bytes(
                &[
                    [4_094, 4_094, 4_094],
                    [32_767, 32_767, 32_767],
                    [60_159, 60_159, 60_159],
                ],
                &[u16::MAX; 3],
            ),
            &rec709_rgb(ColorBitDepth::Eight, ColorRange::Limited),
            None,
        )
        .expect("8-bit limited Rec.709 RGB");
        assert_close(rgb8.pixels[0].to_f32(), 0.0, 2.0e-3);
        assert_close(
            rgb8.pixels[4].to_f32(),
            decode_bt709((128.0 - 16.0) / 219.0),
            2.0e-3,
        );
        assert_close(rgb8.pixels[8].to_f32(), 1.0, 2.0e-3);

        let rgb10 = WorkingFrame::from_rgba64_le(
            3,
            1,
            &rgba64_bytes(
                &[
                    [4_100, 4_100, 4_100],
                    [32_800, 32_800, 32_800],
                    [60_218, 60_218, 60_218],
                ],
                &[u16::MAX; 3],
            ),
            &rec709_rgb(ColorBitDepth::Ten, ColorRange::Limited),
            None,
        )
        .expect("10-bit limited Rec.709 RGB");
        assert_close(rgb10.pixels[0].to_f32(), 0.0, 2.0e-3);
        assert_close(
            rgb10.pixels[4].to_f32(),
            decode_bt709((512.0 - 64.0) / 876.0),
            2.0e-3,
        );
        assert_close(rgb10.pixels[8].to_f32(), 1.0, 2.0e-3);
    }

    #[test]
    fn rec709_rgb_full_10_bit_uses_true_16_bit_rgba64_scale() {
        let frame = WorkingFrame::from_rgba64_le(
            2,
            1,
            &rgba64_bytes(&[[0, 0, 0], [u16::MAX, u16::MAX, u16::MAX]], &[u16::MAX; 2]),
            &rec709_rgb(ColorBitDepth::Ten, ColorRange::Full),
            None,
        )
        .expect("10-bit full Rec.709 RGB");
        assert_close(frame.pixels[0].to_f32(), 0.0, 0.0);
        assert_close(frame.pixels[4].to_f32(), 1.0, 1.0e-3);
    }

    #[test]
    fn srgb_rgb_8_and_10_bit_endpoints_and_alpha_are_preserved() {
        let rgb8 = WorkingFrame::from_rgba64_le(
            2,
            1,
            &rgba64_bytes(&[[0, 0, 0], [65_283, 65_283, 65_283]], &[0, u16::MAX]),
            &srgb(ColorBitDepth::Eight),
            None,
        )
        .expect("8-bit sRGB RGBA64 endpoints");
        assert_close(rgb8.pixels[3].to_f32(), 0.0, 0.0);
        assert_close(
            rgb8.pixels[4].to_f32(),
            decode_srgb(65_283.0 / 65_280.0),
            1.0e-3,
        );
        assert_close(rgb8.pixels[7].to_f32(), 1.0, 0.0);

        let rgb10 = WorkingFrame::from_rgba64_le(
            2,
            1,
            &rgba64_bytes(
                &[[0, 0, 0], [u16::MAX, u16::MAX, u16::MAX]],
                &[u16::MAX, u16::MAX],
            ),
            &srgb(ColorBitDepth::Ten),
            None,
        )
        .expect("10-bit sRGB RGBA64 endpoints");
        assert_close(rgb10.pixels[0].to_f32(), 0.0, 0.0);
        assert_close(rgb10.pixels[4].to_f32(), 1.0, 1.0e-3);
        assert_close(rgb10.pixels[3].to_f32(), 1.0, 0.0);
        assert_close(rgb10.pixels[7].to_f32(), 1.0, 0.0);
    }

    #[test]
    fn rgba64_frame_rejects_non_integer_source_depth() {
        let description = srgb(ColorBitDepth::Float16);
        let error = WorkingFrame::from_rgba64_le(
            1,
            1,
            &rgba64_bytes(&[[0, 0, 0]], &[u16::MAX]),
            &description,
            None,
        )
        .expect_err("float source depth must be rejected");
        assert!(
            error
                .to_string()
                .contains("unsupported source colour bit depth")
        );
    }
}
