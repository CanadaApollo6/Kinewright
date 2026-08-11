use serde::{Deserialize, Serialize};

use crate::{ParamValue, TimeCode};

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

#[must_use]
pub fn title_parameter_value(title: &Title, name: &str) -> Option<ParamValue> {
    match name {
        "text" => Some(ParamValue::Text(title.text.clone())),
        "font_size_token" => Some(ParamValue::Integer(i64::from(title.font_size_token))),
        "color_token" => Some(ParamValue::Integer(i64::from(title.color_token))),
        "position" => Some(ParamValue::Text(title.position.as_str().to_owned())),
        "background_scrim" => Some(ParamValue::Boolean(title.background_scrim)),
        "fade_in_frames" => Some(ParamValue::Integer(title.fade_in_frames.0)),
        "fade_out_frames" => Some(ParamValue::Integer(title.fade_out_frames.0)),
        _ => None,
    }
}
