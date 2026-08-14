use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, PxScale, PxScaleFont, ScaleFont};
use serde::{Deserialize, Serialize};

use crate::{ParamValue, TimeCode};

const INTER_BYTES: &[u8] = include_bytes!("../../openreel-app/assets/fonts/Inter-Variable.ttf");
const REFERENCE_SHORT_EDGE: u32 = 1_080;
const TITLE_SAFE_MARGIN_PERCENT: u32 = 8;
const CAPTION_MOTION_SCALE_PERCENT: i32 = 110;
const CAPTION_MOTION_Y_PERCENT: i32 = 15;
const MINIMUM_FONT_PIXELS: u32 = 8;

/// One half-open pixel rectangle in a rendered title frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TitlePixelBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl TitlePixelBounds {
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        other.left >= self.left
            && other.top >= self.top
            && other.right <= self.right
            && other.bottom <= self.bottom
    }
}

/// Deterministic delivery-aware title composition shared by QA and rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleLayout {
    pub lines: Vec<String>,
    pub font_pixels: u32,
    pub line_height_pixels: u32,
    pub safe_bounds: TitlePixelBounds,
    pub text_bounds: TitlePixelBounds,
    pub visual_bounds: TitlePixelBounds,
}

/// Stable title font-size tokens. Pixel sizes are interpreted by the media
/// renderer relative to a 1080-line frame so preview and export retain the
/// same composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleFontSizeDescriptor {
    pub token: u8,
    pub name: &'static str,
    pub pixels_at_1080p: u16,
}

/// Stable title color tokens resolved from the Cut Room design system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleColorDescriptor {
    pub token: u8,
    pub name: &'static str,
    pub rgba: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleParameterKind {
    Text { maximum_characters: usize },
    Integer { min: i64, max: i64 },
    Boolean,
    Position,
    CaptionPreset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleParameterDescriptor {
    pub name: &'static str,
    pub kind: TitleParameterKind,
}

pub const TITLE_FONT_SIZES: &[TitleFontSizeDescriptor] = &[
    TitleFontSizeDescriptor {
        token: 0,
        name: "Small",
        pixels_at_1080p: 40,
    },
    TitleFontSizeDescriptor {
        token: 1,
        name: "Standard",
        pixels_at_1080p: 64,
    },
    TitleFontSizeDescriptor {
        token: 2,
        name: "Display",
        pixels_at_1080p: 96,
    },
];

pub const TITLE_COLORS: &[TitleColorDescriptor] = &[
    TitleColorDescriptor {
        token: 0,
        name: "Primary",
        rgba: [0xE6, 0xEC, 0xF2, 0xFF],
    },
    TitleColorDescriptor {
        token: 1,
        name: "Secondary",
        rgba: [0xA5, 0xAF, 0xBB, 0xFF],
    },
    TitleColorDescriptor {
        token: 2,
        name: "Accent",
        rgba: [0x42, 0xC7, 0xC9, 0xFF],
    },
];

pub const TITLE_PARAMETER_DESCRIPTORS: &[TitleParameterDescriptor] = &[
    TitleParameterDescriptor {
        name: "text",
        kind: TitleParameterKind::Text {
            maximum_characters: 512,
        },
    },
    TitleParameterDescriptor {
        name: "font_size_token",
        kind: TitleParameterKind::Integer { min: 0, max: 2 },
    },
    TitleParameterDescriptor {
        name: "color_token",
        kind: TitleParameterKind::Integer { min: 0, max: 2 },
    },
    TitleParameterDescriptor {
        name: "position",
        kind: TitleParameterKind::Position,
    },
    TitleParameterDescriptor {
        name: "caption_preset",
        kind: TitleParameterKind::CaptionPreset,
    },
    TitleParameterDescriptor {
        name: "background_scrim",
        kind: TitleParameterKind::Boolean,
    },
    TitleParameterDescriptor {
        name: "fade_in_frames",
        kind: TitleParameterKind::Integer {
            min: 0,
            max: i64::MAX,
        },
    },
    TitleParameterDescriptor {
        name: "fade_out_frames",
        kind: TitleParameterKind::Integer {
            min: 0,
            max: i64::MAX,
        },
    },
];

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TitlePosition {
    LowerThird,
    #[default]
    Center,
    Top,
}

impl TitlePosition {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LowerThird => "lower_third",
            Self::Center => "center",
            Self::Top => "top",
        }
    }
}

impl std::str::FromStr for TitlePosition {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "lower_third" => Ok(Self::LowerThird),
            "center" => Ok(Self::Center),
            "top" => Ok(Self::Top),
            _ => Err(()),
        }
    }
}

/// Stable, renderer-independent caption compositions. A preset resolves to
/// ordinary title fields, so preview and export share exactly the same result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPreset {
    Clean,
    Social,
    Minimal,
}

impl CaptionPreset {
    pub const ALL: [Self; 3] = [Self::Clean, Self::Social, Self::Minimal];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Social => "social",
            Self::Minimal => "minimal",
        }
    }

    #[must_use]
    pub fn title(self, text: impl Into<String>) -> Title {
        let (font_size_token, color_token, position, background_scrim) = match self {
            Self::Clean => (0, 0, TitlePosition::LowerThird, true),
            Self::Social => (2, 0, TitlePosition::LowerThird, false),
            Self::Minimal => (0, 0, TitlePosition::LowerThird, false),
        };
        Title {
            text: text.into(),
            font_size_token,
            color_token,
            position,
            background_scrim,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
            caption_preset: Some(self),
        }
    }
}

#[cfg(test)]
mod caption_preset_tests {
    use super::*;

    #[test]
    fn social_captions_default_away_from_centered_subjects() {
        let title = CaptionPreset::Social.title("Readable social caption");
        assert_eq!(title.position, TitlePosition::LowerThird);
        assert_eq!(title.color_token, 0);
        assert!(!title.background_scrim);
    }
}

impl std::str::FromStr for CaptionPreset {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "clean" => Ok(Self::Clean),
            "social" => Ok(Self::Social),
            "minimal" => Ok(Self::Minimal),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Title {
    pub text: String,
    pub font_size_token: u8,
    pub color_token: u8,
    pub position: TitlePosition,
    pub background_scrim: bool,
    pub fade_in_frames: TimeCode,
    pub fade_out_frames: TimeCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub caption_preset: Option<CaptionPreset>,
}

impl Default for Title {
    fn default() -> Self {
        Self {
            text: "Title".to_owned(),
            font_size_token: 1,
            color_token: 0,
            position: TitlePosition::Center,
            background_scrim: true,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
            caption_preset: None,
        }
    }
}

#[must_use]
pub fn title_parameter_descriptor(name: &str) -> Option<TitleParameterDescriptor> {
    TITLE_PARAMETER_DESCRIPTORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.name == name)
}

#[must_use]
pub fn title_font_size(token: u8) -> Option<TitleFontSizeDescriptor> {
    TITLE_FONT_SIZES
        .iter()
        .copied()
        .find(|descriptor| descriptor.token == token)
}

#[must_use]
pub fn title_color(token: u8) -> Option<TitleColorDescriptor> {
    TITLE_COLORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.token == token)
}

/// Embedded title font bytes used by every `OpenReel` title renderer.
#[must_use]
pub const fn title_font_bytes() -> &'static [u8] {
    INTER_BYTES
}

/// Resolve wrapping, adaptive type size, and safe-area placement for a title.
///
/// Caption layouts reserve enough room for `OpenReel`'s largest built-in pop and
/// slide-up motion. Font tokens scale from the output's short edge, keeping a
/// vertical 1080x1920 delivery visually equivalent to a 1920x1080 delivery.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn title_layout(title: &Title, resolution: (u32, u32)) -> Option<TitleLayout> {
    let (width, height) = resolution;
    if width == 0 || height == 0 {
        return None;
    }
    let descriptor = title_font_size(title.font_size_token)?;
    let short_edge = width.min(height);
    let base_font_pixels = u32::from(descriptor.pixels_at_1080p)
        .saturating_mul(short_edge)
        .saturating_add(REFERENCE_SHORT_EDGE / 2)
        / REFERENCE_SHORT_EDGE;
    let base_font_pixels = base_font_pixels.max(MINIMUM_FONT_PIXELS);
    let safe_bounds = safe_bounds(resolution);
    let layout_bounds = if title.caption_preset.is_some() {
        caption_motion_base_bounds(safe_bounds, resolution)
    } else {
        safe_bounds
    };
    let font = inter_font();

    for font_pixels in (MINIMUM_FONT_PIXELS..=base_font_pixels).rev() {
        let scale = PxScale::from(font_pixels as f32);
        let scaled = font.as_scaled(scale);
        let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap())
            .ceil()
            .max(1.0) as u32;
        let horizontal_padding = if title.background_scrim {
            (font_pixels as f32 * 0.55).ceil() as i32
        } else {
            (font_pixels as f32 * 0.08).ceil() as i32
        };
        let vertical_padding = if title.background_scrim {
            (font_pixels as f32 * 0.28).ceil() as i32
        } else {
            (font_pixels as f32 * 0.08).ceil() as i32
        };
        let maximum_line_width = layout_bounds
            .right
            .saturating_sub(layout_bounds.left)
            .saturating_sub(horizontal_padding.saturating_mul(2))
            .max(1) as f32;
        let lines = if title.caption_preset.is_some() {
            wrap_title_text(&scaled, &title.text, maximum_line_width)
        } else {
            explicit_title_lines(&title.text)
        };
        let line_widths = lines
            .iter()
            .map(|line| title_line_width(&scaled, line))
            .collect::<Vec<_>>();
        let block_width = line_widths.iter().copied().fold(0.0_f32, f32::max);
        let line_count = u32::try_from(lines.len().max(1)).unwrap_or(u32::MAX);
        let block_height = line_height.saturating_mul(line_count);
        let visual_height = i32::try_from(block_height)
            .unwrap_or(i32::MAX)
            .saturating_add(vertical_padding.saturating_mul(2));
        if visual_height > layout_bounds.bottom.saturating_sub(layout_bounds.top) {
            continue;
        }

        let block_width_i32 = block_width.ceil() as i32;
        let block_height_i32 = i32::try_from(block_height).unwrap_or(i32::MAX);
        let desired_center_y = match title.position {
            TitlePosition::Top => height as f32 * 0.20,
            TitlePosition::Center => height as f32 * 0.50,
            TitlePosition::LowerThird => height as f32 * 0.72,
        };
        let minimum_top = layout_bounds.top.saturating_add(vertical_padding);
        let maximum_top = layout_bounds
            .bottom
            .saturating_sub(vertical_padding)
            .saturating_sub(block_height_i32);
        let text_top = (desired_center_y - block_height as f32 * 0.5)
            .round()
            .clamp(minimum_top as f32, maximum_top as f32) as i32;
        let text_left = ((width as f32 - block_width) * 0.5).floor() as i32;
        let text_bounds = TitlePixelBounds {
            left: text_left,
            top: text_top,
            right: text_left.saturating_add(block_width_i32),
            bottom: text_top.saturating_add(block_height_i32),
        };
        let visual_bounds = TitlePixelBounds {
            left: text_bounds.left.saturating_sub(horizontal_padding),
            top: text_bounds.top.saturating_sub(vertical_padding),
            right: text_bounds.right.saturating_add(horizontal_padding),
            bottom: text_bounds.bottom.saturating_add(vertical_padding),
        };
        if layout_bounds.contains(visual_bounds) {
            return Some(TitleLayout {
                lines,
                font_pixels,
                line_height_pixels: line_height,
                safe_bounds,
                text_bounds,
                visual_bounds,
            });
        }
    }
    None
}

fn inter_font() -> &'static FontArc {
    static FONT: OnceLock<FontArc> = OnceLock::new();
    FONT.get_or_init(|| {
        FontArc::try_from_slice(INTER_BYTES)
            .expect("the embedded Inter font must remain a valid OpenType font")
    })
}

fn safe_bounds(resolution: (u32, u32)) -> TitlePixelBounds {
    let (width, height) = resolution;
    let horizontal = width.saturating_mul(TITLE_SAFE_MARGIN_PERCENT) / 100;
    let vertical = height.saturating_mul(TITLE_SAFE_MARGIN_PERCENT) / 100;
    TitlePixelBounds {
        left: saturating_i32(i64::from(horizontal)),
        top: saturating_i32(i64::from(vertical)),
        right: saturating_i32(i64::from(width.saturating_sub(horizontal))),
        bottom: saturating_i32(i64::from(height.saturating_sub(vertical))),
    }
}

fn caption_motion_base_bounds(safe: TitlePixelBounds, resolution: (u32, u32)) -> TitlePixelBounds {
    let center_x = i64::from(resolution.0) / 2;
    let center_y = i64::from(resolution.1) / 2;
    let minimum_top = i64::from(safe.top).saturating_add(
        i64::from(resolution.1).saturating_mul(i64::from(CAPTION_MOTION_Y_PERCENT)) / 100,
    );
    let inverse = |value: i64, center: i64| {
        center.saturating_add(
            value
                .saturating_sub(center)
                .saturating_mul(100)
                .div_euclid(i64::from(CAPTION_MOTION_SCALE_PERCENT)),
        )
    };
    TitlePixelBounds {
        left: saturating_i32(inverse(i64::from(safe.left), center_x)),
        top: saturating_i32(inverse(minimum_top, center_y)),
        right: saturating_i32(inverse(i64::from(safe.right), center_x)),
        bottom: saturating_i32(inverse(i64::from(safe.bottom), center_y)),
    }
}

fn saturating_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value.is_negative() {
        i32::MIN
    } else {
        i32::MAX
    })
}

fn explicit_title_lines(text: &str) -> Vec<String> {
    let lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn wrap_title_text(font: &PxScaleFont<&FontArc>, text: &str, maximum_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if title_line_width(font, &candidate) <= maximum_width {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let mut pieces = wrap_long_word(font, word, maximum_width);
            current = pieces.pop().unwrap_or_default();
            lines.extend(pieces);
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_long_word(font: &PxScaleFont<&FontArc>, word: &str, maximum_width: f32) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for character in word.chars() {
        let mut candidate = piece.clone();
        candidate.push(character);
        if !piece.is_empty() && title_line_width(font, &candidate) > maximum_width {
            pieces.push(std::mem::take(&mut piece));
        }
        piece.push(character);
    }
    pieces.push(piece);
    pieces
}

fn title_line_width(font: &PxScaleFont<&FontArc>, text: &str) -> f32 {
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

#[must_use]
pub fn title_parameter_value(title: &Title, name: &str) -> Option<ParamValue> {
    match name {
        "text" => Some(ParamValue::Text(title.text.clone())),
        "font_size_token" => Some(ParamValue::Integer(i64::from(title.font_size_token))),
        "color_token" => Some(ParamValue::Integer(i64::from(title.color_token))),
        "position" => Some(ParamValue::Text(title.position.as_str().to_owned())),
        "caption_preset" => Some(ParamValue::Text(
            title
                .caption_preset
                .map_or("none", CaptionPreset::as_str)
                .to_owned(),
        )),
        "background_scrim" => Some(ParamValue::Boolean(title.background_scrim)),
        "fade_in_frames" => Some(ParamValue::Integer(title.fade_in_frames.0)),
        "fade_out_frames" => Some(ParamValue::Integer(title.fade_out_frames.0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_social_captions_keep_landscape_type_scale_and_wrap() {
        let title = CaptionPreset::Social
            .title("A finished vertical caption should stay comfortably inside the frame");
        let vertical = title_layout(&title, (1_080, 1_920)).unwrap();
        let landscape = title_layout(&title, (1_920, 1_080)).unwrap();

        assert_eq!(vertical.font_pixels, 96);
        assert_eq!(landscape.font_pixels, 96);
        assert!(vertical.lines.len() > 1);
        assert!(vertical.safe_bounds.contains(vertical.visual_bounds));
    }

    #[test]
    fn long_words_hard_wrap_without_escaping_the_safe_area() {
        let title = CaptionPreset::Social.title("W".repeat(128));
        let layout = title_layout(&title, (1_080, 1_920)).unwrap();

        assert!(layout.lines.len() > 1);
        assert_eq!(layout.lines.concat(), "W".repeat(128));
        assert!(layout.safe_bounds.contains(layout.visual_bounds));
    }

    #[test]
    fn ordinary_titles_preserve_authored_spacing_and_line_breaks() {
        let title = Title {
            text: "Keep   spacing\nand lines".to_owned(),
            ..Title::default()
        };
        let layout = title_layout(&title, (1_920, 1_080)).unwrap();

        assert_eq!(layout.lines, ["Keep   spacing", "and lines"]);
    }
}
