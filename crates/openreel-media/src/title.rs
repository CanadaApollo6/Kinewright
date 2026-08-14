use std::sync::Arc;

use ab_glyph::{Font, FontArc, OutlinedGlyph, PxScale, PxScaleFont, ScaleFont, point};
use openreel_core::{FrameTexture, MediaError, Title, title_color, title_font_bytes, title_layout};

const SCRIM_COLOR: [u8; 4] = [0x05, 0x07, 0x0A, 184];
const CAPTION_OUTLINE_COLOR: [u8; 4] = [0x05, 0x07, 0x0A, 224];

pub(crate) struct TitleRasterizer {
    font: FontArc,
}

impl TitleRasterizer {
    pub(crate) fn new() -> Self {
        let font = FontArc::try_from_slice(title_font_bytes())
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
        let color = title_color(title.color_token)
            .ok_or_else(|| MediaError::Backend("title color token is invalid".to_owned()))?;
        let layout = title_layout(title, resolution)
            .ok_or_else(|| MediaError::Backend("title cannot fit its safe area".to_owned()))?;
        let px = layout.font_pixels as f32;
        let scale = PxScale::from(px);
        let scaled = self.font.as_scaled(scale);
        let line_height = layout.line_height_pixels as f32;
        let line_widths = layout
            .lines
            .iter()
            .map(|line| line_width(&scaled, line.as_str()))
            .collect::<Vec<_>>();
        let block_top = layout.text_bounds.top as f32;
        let pixel_count = usize::try_from(width)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::try_from(height).unwrap_or(usize::MAX));
        let mut rgba = vec![0_u8; pixel_count.saturating_mul(4)];

        if title.background_scrim && !title.text.is_empty() {
            fill_rect(
                &mut rgba,
                resolution,
                (
                    layout.visual_bounds.left,
                    layout.visual_bounds.top,
                    layout.visual_bounds.right,
                    layout.visual_bounds.bottom,
                ),
                SCRIM_COLOR,
            );
        }

        for (line_index, line) in layout.lines.iter().enumerate() {
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
                    if title.caption_preset.is_some() && !title.background_scrim {
                        let radius =
                            i32::try_from((layout.font_pixels / 32).clamp(2, 4)).unwrap_or(2);
                        draw_glyph(
                            &mut rgba,
                            resolution,
                            &outlined,
                            CAPTION_OUTLINE_COLOR,
                            &[
                                (-radius, -radius),
                                (0, -radius),
                                (radius, -radius),
                                (-radius, 0),
                                (radius, 0),
                                (-radius, radius),
                                (0, radius),
                                (radius, radius),
                            ],
                        );
                    }
                    draw_glyph(&mut rgba, resolution, &outlined, color.rgba, &[(0, 0)]);
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

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn draw_glyph(
    rgba: &mut [u8],
    resolution: (u32, u32),
    glyph: &OutlinedGlyph,
    color: [u8; 4],
    offsets: &[(i32, i32)],
) {
    let bounds = glyph.px_bounds();
    for &(offset_x, offset_y) in offsets {
        glyph.draw(|x, y, coverage| {
            let pixel_x = bounds.min.x.floor() as i32 + x as i32 + offset_x;
            let pixel_y = bounds.min.y.floor() as i32 + y as i32 + offset_y;
            if pixel_x >= 0
                && pixel_y >= 0
                && pixel_x < resolution.0 as i32
                && pixel_y < resolution.1 as i32
            {
                let offset = (usize::try_from(pixel_y).unwrap()
                    * usize::try_from(resolution.0).unwrap()
                    + usize::try_from(pixel_x).unwrap())
                    * 4;
                let alpha = (coverage * f32::from(color[3])).round() as u8;
                blend_pixel(
                    &mut rgba[offset..offset + 4],
                    [color[0], color[1], color[2], alpha],
                );
            }
        });
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
    use openreel_core::CaptionPreset;

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

    #[test]
    fn vertical_social_caption_pixels_stay_inside_shared_safe_bounds() {
        let rasterizer = TitleRasterizer::new();
        let title = CaptionPreset::Social
            .title("This finished vertical caption wraps inside the delivery safe area");
        let resolution = (1_080, 1_920);
        let layout = title_layout(&title, resolution).unwrap();
        let frame = rasterizer.rasterize(&title, resolution).unwrap();
        let mut rendered = openreel_core::TitlePixelBounds {
            left: i32::MAX,
            top: i32::MAX,
            right: i32::MIN,
            bottom: i32::MIN,
        };
        for (index, pixel) in frame.rgba.chunks_exact(4).enumerate() {
            if pixel[3] == 0 {
                continue;
            }
            let x = i32::try_from(index % usize::try_from(resolution.0).unwrap()).unwrap();
            let y = i32::try_from(index / usize::try_from(resolution.0).unwrap()).unwrap();
            rendered.left = rendered.left.min(x);
            rendered.top = rendered.top.min(y);
            rendered.right = rendered.right.max(x.saturating_add(1));
            rendered.bottom = rendered.bottom.max(y.saturating_add(1));
        }

        assert!(layout.lines.len() > 1);
        assert!(layout.safe_bounds.contains(rendered));
    }
}
