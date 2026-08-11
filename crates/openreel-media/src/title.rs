use std::sync::Arc;

use ab_glyph::{Font, FontArc, PxScale, PxScaleFont, ScaleFont, point};
use openreel_core::{FrameTexture, MediaError, Title, TitlePosition, title_color, title_font_size};

const INTER_BYTES: &[u8] = include_bytes!("../../openreel-app/assets/fonts/Inter-Variable.ttf");
const REFERENCE_HEIGHT: f32 = 1080.0;
const SCRIM_COLOR: [u8; 4] = [0x05, 0x07, 0x0A, 184];

pub(crate) struct TitleRasterizer {
    font: FontArc,
}

impl TitleRasterizer {
    pub(crate) fn new() -> Self {
        let font = FontArc::try_from_slice(INTER_BYTES)
            .expect("the embedded Inter font must remain a valid OpenType font");
        Self { font }
    }

    /// Rasterize a declarative title to a transparent full-frame RGBA layer.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]
    pub(crate) fn rasterize(
        &self,
        title: &Title,
        resolution: (u32, u32),
    ) -> Result<FrameTexture, MediaError> {
        let (width, height) = resolution;
        if width == 0 || height == 0 {
            return Err(MediaError::Backend(
                "title output resolution must be non-zero".to_owned(),
            ));
        }
        let size = title_font_size(title.font_size_token)
            .ok_or_else(|| MediaError::Backend("title font-size token is invalid".to_owned()))?;
        let color = title_color(title.color_token)
            .ok_or_else(|| MediaError::Backend("title color token is invalid".to_owned()))?;
        let px = (f32::from(size.pixels_at_1080p) * height as f32 / REFERENCE_HEIGHT)
            .round()
            .max(8.0);
        let scale = PxScale::from(px);
        let scaled = self.font.as_scaled(scale);
        let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap()).ceil();
        let lines = title.text.split('\n').collect::<Vec<_>>();
        let line_widths = lines
            .iter()
            .map(|line| line_width(&scaled, line))
            .collect::<Vec<_>>();
        let block_width = line_widths.iter().copied().fold(0.0_f32, f32::max);
        let block_height = line_height * lines.len().max(1) as f32;
        let center_y = match title.position {
            TitlePosition::Top => height as f32 * 0.18,
            TitlePosition::Center => height as f32 * 0.50,
            TitlePosition::LowerThird => height as f32 * 0.78,
        };
        let block_left = (width as f32 - block_width) * 0.5;
        let block_top = center_y - block_height * 0.5;
        let pixel_count = usize::try_from(width)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::try_from(height).unwrap_or(usize::MAX));
        let mut rgba = vec![0_u8; pixel_count.saturating_mul(4)];

        if title.background_scrim && !title.text.is_empty() {
            let horizontal_padding = (px * 0.55).round() as i32;
            let vertical_padding = (px * 0.28).round() as i32;
            fill_rect(
                &mut rgba,
                resolution,
                (
                    block_left.floor() as i32 - horizontal_padding,
                    block_top.floor() as i32 - vertical_padding,
                    (block_left + block_width).ceil() as i32 + horizontal_padding,
                    (block_top + block_height).ceil() as i32 + vertical_padding,
                ),
                SCRIM_COLOR,
            );
        }

        for (line_index, line) in lines.iter().enumerate() {
            let mut cursor_x = (width as f32 - line_widths[line_index]) * 0.5;
            let baseline = block_top + line_index as f32 * line_height + scaled.ascent();
            let mut previous = None;
            for character in line.chars() {
                let id = self.font.glyph_id(character);
                if let Some(previous) = previous {
                    cursor_x += scaled.kern(previous, id);
                }
                let glyph = id.with_scale_and_position(scale, point(cursor_x, baseline));
                if let Some(outlined) = self.font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|x, y, coverage| {
                        let pixel_x = bounds.min.x.floor() as i32 + x as i32;
                        let pixel_y = bounds.min.y.floor() as i32 + y as i32;
                        if pixel_x >= 0
                            && pixel_y >= 0
                            && pixel_x < width as i32
                            && pixel_y < height as i32
                        {
                            let offset = (usize::try_from(pixel_y).unwrap()
                                * usize::try_from(width).unwrap()
                                + usize::try_from(pixel_x).unwrap())
                                * 4;
                            let alpha = (coverage * f32::from(color.rgba[3])).round() as u8;
                            blend_pixel(
                                &mut rgba[offset..offset + 4],
                                [color.rgba[0], color.rgba[1], color.rgba[2], alpha],
                            );
                        }
                    });
                }
                cursor_x += scaled.h_advance(id);
                previous = Some(id);
            }
        }

        Ok(FrameTexture {
            width,
            height,
            rgba: Arc::new(rgba),
        })
    }
}

fn line_width(font: &PxScaleFont<&FontArc>, text: &str) -> f32 {
    let mut width = 0.0;
    let mut previous = None;
    for character in text.chars() {
        let id = font.glyph_id(character);
        if let Some(previous) = previous {
            width += font.kern(previous, id);
        }
        width += font.h_advance(id);
        previous = Some(id);
    }
    width
}

#[allow(clippy::cast_possible_wrap)]
fn fill_rect(rgba: &mut [u8], resolution: (u32, u32), rect: (i32, i32, i32, i32), color: [u8; 4]) {
    let width = resolution.0 as i32;
    let height = resolution.1 as i32;
    let left = rect.0.clamp(0, width);
    let top = rect.1.clamp(0, height);
    let right = rect.2.clamp(left, width);
    let bottom = rect.3.clamp(top, height);
    for y in top..bottom {
        for x in left..right {
            let offset = (usize::try_from(y).unwrap() * usize::try_from(width).unwrap()
                + usize::try_from(x).unwrap())
                * 4;
            rgba[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

fn blend_pixel(destination: &mut [u8], source: [u8; 4]) {
    let source_alpha = u32::from(source[3]);
    let destination_alpha = u32::from(destination[3]);
    let inverse = 255 - source_alpha;
    let output_alpha = source_alpha + (destination_alpha * inverse + 127) / 255;
    if output_alpha == 0 {
        destination.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for channel in 0..3 {
        let source_premultiplied = u32::from(source[channel]) * source_alpha;
        let destination_premultiplied =
            u32::from(destination[channel]) * destination_alpha * inverse / 255;
        destination[channel] = u8::try_from(
            (source_premultiplied + destination_premultiplied + output_alpha / 2) / output_alpha,
        )
        .unwrap_or(u8::MAX);
    }
    destination[3] = u8::try_from(output_alpha).unwrap_or(u8::MAX);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_title_rasterizes_to_identical_pixels() {
        let rasterizer = TitleRasterizer::new();
        let title = Title {
            text: "Deterministic\nTitle".to_owned(),
            ..Title::default()
        };
        let first = rasterizer.rasterize(&title, (320, 180)).unwrap();
        let second = rasterizer.rasterize(&title, (320, 180)).unwrap();
        assert_eq!(first.rgba, second.rgba);
        assert!(first.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }
}
