/// A compositor control populated by an effect parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectUniform {
    Brightness,
    Contrast,
    Saturation,
    Opacity,
    Scale,
    OffsetX,
    OffsetY,
    CropLeft,
    CropRight,
    CropTop,
    CropBottom,
    ReframeAspect,
    ReframeFocusX,
    ReframeFocusY,
    ReframeFocusXBasisPoints,
    ReframeFocusYBasisPoints,
    Exposure,
    Temperature,
    Tint,
    PrimaryExposure,
    PrimaryTemperature,
    PrimaryTint,
    PrimaryContrast,
    PrimaryPivot,
    Blacks,
    Shadows,
    Highlights,
    Whites,
    PrimarySaturation,
    LutPreset,
    LutIntensity,
    ExternalLutIntensity,
    MaskShape,
    MaskCenterX,
    MaskCenterY,
    MaskWidth,
    MaskHeight,
    MaskFeather,
    MaskInvert,
    KeyRed,
    KeyGreen,
    KeyBlue,
    KeyThreshold,
    KeySoftness,
    KeySpill,
    AudioGain,
    EqLowGain,
    EqMidGain,
    EqHighGain,
    CompressorThreshold,
    CompressorRatio,
    CompressorAttack,
    CompressorRelease,
    CompressorMakeup,
    LimiterCeiling,
    DuckThreshold,
    DuckReduction,
    DuckAttack,
    DuckRelease,
}

/// The accepted integer domain and neutral value for one effect parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectParameterDescriptor {
    pub name: &'static str,
    pub min: i64,
    pub max: i64,
    pub neutral: i64,
    pub uniform: EffectUniform,
}

/// The complete public contract for one built-in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectDescriptor {
    pub name: &'static str,
    pub parameters: &'static [EffectParameterDescriptor],
}

impl EffectDescriptor {
    #[must_use]
    pub fn parameter(self, name: &str) -> Option<&'static EffectParameterDescriptor> {
        self.parameters
            .iter()
            .find(|parameter| parameter.name == name)
    }
}

/// Built-in effect metadata used by validation, rendering, and agent documentation.
pub const EFFECT_DESCRIPTORS: &[EffectDescriptor] = &[
    EffectDescriptor {
        name: "brightness",
        parameters: &[EffectParameterDescriptor {
            name: "percent",
            min: -100,
            max: 100,
            neutral: 0,
            uniform: EffectUniform::Brightness,
        }],
    },
    EffectDescriptor {
        name: "contrast",
        parameters: &[EffectParameterDescriptor {
            name: "percent",
            min: -100,
            max: 100,
            neutral: 0,
            uniform: EffectUniform::Contrast,
        }],
    },
    EffectDescriptor {
        name: "saturation",
        parameters: &[EffectParameterDescriptor {
            name: "percent",
            min: -100,
            max: 100,
            neutral: 0,
            uniform: EffectUniform::Saturation,
        }],
    },
    EffectDescriptor {
        name: "opacity",
        parameters: &[EffectParameterDescriptor {
            name: "percent",
            min: 0,
            max: 100,
            neutral: 100,
            uniform: EffectUniform::Opacity,
        }],
    },
    EffectDescriptor {
        name: "transform",
        parameters: &[
            EffectParameterDescriptor {
                name: "scale_percent",
                min: 1,
                max: 400,
                neutral: 100,
                uniform: EffectUniform::Scale,
            },
            EffectParameterDescriptor {
                name: "x_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::OffsetX,
            },
            EffectParameterDescriptor {
                name: "y_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::OffsetY,
            },
        ],
    },
    EffectDescriptor {
        name: "crop",
        parameters: &[
            EffectParameterDescriptor {
                name: "left_percent",
                min: 0,
                max: 45,
                neutral: 0,
                uniform: EffectUniform::CropLeft,
            },
            EffectParameterDescriptor {
                name: "right_percent",
                min: 0,
                max: 45,
                neutral: 0,
                uniform: EffectUniform::CropRight,
            },
            EffectParameterDescriptor {
                name: "top_percent",
                min: 0,
                max: 45,
                neutral: 0,
                uniform: EffectUniform::CropTop,
            },
            EffectParameterDescriptor {
                name: "bottom_percent",
                min: 0,
                max: 45,
                neutral: 0,
                uniform: EffectUniform::CropBottom,
            },
        ],
    },
    EffectDescriptor {
        name: "reframe",
        parameters: &[
            EffectParameterDescriptor {
                name: "target_aspect_basis_points",
                min: 1_000,
                max: 40_000,
                neutral: 17_778,
                uniform: EffectUniform::ReframeAspect,
            },
            EffectParameterDescriptor {
                name: "focus_x_percent",
                min: 0,
                max: 100,
                neutral: 50,
                uniform: EffectUniform::ReframeFocusX,
            },
            EffectParameterDescriptor {
                name: "focus_y_percent",
                min: 0,
                max: 100,
                neutral: 50,
                uniform: EffectUniform::ReframeFocusY,
            },
            EffectParameterDescriptor {
                name: "focus_x_basis_points",
                min: 0,
                max: 10_000,
                neutral: 5_000,
                uniform: EffectUniform::ReframeFocusXBasisPoints,
            },
            EffectParameterDescriptor {
                name: "focus_y_basis_points",
                min: 0,
                max: 10_000,
                neutral: 5_000,
                uniform: EffectUniform::ReframeFocusYBasisPoints,
            },
        ],
    },
    EffectDescriptor {
        name: "color_grade",
        parameters: &[
            EffectParameterDescriptor {
                name: "exposure_milli_stops",
                min: -5_000,
                max: 5_000,
                neutral: 0,
                uniform: EffectUniform::Exposure,
            },
            EffectParameterDescriptor {
                name: "temperature_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::Temperature,
            },
            EffectParameterDescriptor {
                name: "tint_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::Tint,
            },
        ],
    },
    EffectDescriptor {
        name: "primary_correction",
        parameters: &[
            EffectParameterDescriptor {
                name: "exposure_milli_stops",
                min: -5_000,
                max: 5_000,
                neutral: 0,
                uniform: EffectUniform::PrimaryExposure,
            },
            EffectParameterDescriptor {
                name: "temperature_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::PrimaryTemperature,
            },
            EffectParameterDescriptor {
                name: "tint_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::PrimaryTint,
            },
            EffectParameterDescriptor {
                name: "contrast_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::PrimaryContrast,
            },
            EffectParameterDescriptor {
                name: "contrast_pivot_basis_points",
                min: 0,
                max: 10_000,
                neutral: 5_000,
                uniform: EffectUniform::PrimaryPivot,
            },
            EffectParameterDescriptor {
                name: "blacks_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::Blacks,
            },
            EffectParameterDescriptor {
                name: "shadows_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::Shadows,
            },
            EffectParameterDescriptor {
                name: "highlights_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::Highlights,
            },
            EffectParameterDescriptor {
                name: "whites_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::Whites,
            },
            EffectParameterDescriptor {
                name: "saturation_percent",
                min: -100,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::PrimarySaturation,
            },
        ],
    },
    EffectDescriptor {
        name: "look_lut",
        parameters: &[
            EffectParameterDescriptor {
                name: "preset_token",
                min: 0,
                max: 4,
                neutral: 0,
                uniform: EffectUniform::LutPreset,
            },
            EffectParameterDescriptor {
                name: "intensity_percent",
                min: 0,
                max: 100,
                neutral: 100,
                uniform: EffectUniform::LutIntensity,
            },
        ],
    },
    EffectDescriptor {
        name: "cube_lut",
        parameters: &[EffectParameterDescriptor {
            name: "intensity_percent",
            min: 0,
            max: 100,
            neutral: 100,
            uniform: EffectUniform::ExternalLutIntensity,
        }],
    },
    EffectDescriptor {
        name: "mask",
        parameters: &[
            EffectParameterDescriptor {
                name: "shape_token",
                min: 1,
                max: 2,
                neutral: 1,
                uniform: EffectUniform::MaskShape,
            },
            EffectParameterDescriptor {
                name: "center_x_percent",
                min: 0,
                max: 100,
                neutral: 50,
                uniform: EffectUniform::MaskCenterX,
            },
            EffectParameterDescriptor {
                name: "center_y_percent",
                min: 0,
                max: 100,
                neutral: 50,
                uniform: EffectUniform::MaskCenterY,
            },
            EffectParameterDescriptor {
                name: "width_percent",
                min: 1,
                max: 200,
                neutral: 100,
                uniform: EffectUniform::MaskWidth,
            },
            EffectParameterDescriptor {
                name: "height_percent",
                min: 1,
                max: 200,
                neutral: 100,
                uniform: EffectUniform::MaskHeight,
            },
            EffectParameterDescriptor {
                name: "feather_percent",
                min: 0,
                max: 100,
                neutral: 0,
                uniform: EffectUniform::MaskFeather,
            },
            EffectParameterDescriptor {
                name: "invert",
                min: 0,
                max: 1,
                neutral: 0,
                uniform: EffectUniform::MaskInvert,
            },
        ],
    },
    EffectDescriptor {
        name: "chroma_key",
        parameters: &[
            EffectParameterDescriptor {
                name: "key_red",
                min: 0,
                max: 255,
                neutral: 0,
                uniform: EffectUniform::KeyRed,
            },
            EffectParameterDescriptor {
                name: "key_green",
                min: 0,
                max: 255,
                neutral: 255,
                uniform: EffectUniform::KeyGreen,
            },
            EffectParameterDescriptor {
                name: "key_blue",
                min: 0,
                max: 255,
                neutral: 0,
                uniform: EffectUniform::KeyBlue,
            },
            EffectParameterDescriptor {
                name: "threshold_percent",
                min: 0,
                max: 100,
                neutral: 15,
                uniform: EffectUniform::KeyThreshold,
            },
            EffectParameterDescriptor {
                name: "softness_percent",
                min: 0,
                max: 100,
                neutral: 10,
                uniform: EffectUniform::KeySoftness,
            },
            EffectParameterDescriptor {
                name: "spill_percent",
                min: 0,
                max: 100,
                neutral: 50,
                uniform: EffectUniform::KeySpill,
            },
        ],
    },
    EffectDescriptor {
        name: "audio_gain",
        parameters: &[EffectParameterDescriptor {
            name: "gain_tenth_db",
            min: -600,
            max: 120,
            neutral: 0,
            uniform: EffectUniform::AudioGain,
        }],
    },
    EffectDescriptor {
        name: "audio_eq",
        parameters: &[
            EffectParameterDescriptor {
                name: "low_gain_tenth_db",
                min: -240,
                max: 240,
                neutral: 0,
                uniform: EffectUniform::EqLowGain,
            },
            EffectParameterDescriptor {
                name: "mid_gain_tenth_db",
                min: -240,
                max: 240,
                neutral: 0,
                uniform: EffectUniform::EqMidGain,
            },
            EffectParameterDescriptor {
                name: "high_gain_tenth_db",
                min: -240,
                max: 240,
                neutral: 0,
                uniform: EffectUniform::EqHighGain,
            },
        ],
    },
    EffectDescriptor {
        name: "audio_compressor",
        parameters: &[
            EffectParameterDescriptor {
                name: "threshold_tenth_db",
                min: -600,
                max: 0,
                neutral: 0,
                uniform: EffectUniform::CompressorThreshold,
            },
            EffectParameterDescriptor {
                name: "ratio_hundredths",
                min: 100,
                max: 2_000,
                neutral: 100,
                uniform: EffectUniform::CompressorRatio,
            },
            EffectParameterDescriptor {
                name: "attack_milliseconds",
                min: 1,
                max: 1_000,
                neutral: 10,
                uniform: EffectUniform::CompressorAttack,
            },
            EffectParameterDescriptor {
                name: "release_milliseconds",
                min: 10,
                max: 5_000,
                neutral: 250,
                uniform: EffectUniform::CompressorRelease,
            },
            EffectParameterDescriptor {
                name: "makeup_gain_tenth_db",
                min: -120,
                max: 240,
                neutral: 0,
                uniform: EffectUniform::CompressorMakeup,
            },
        ],
    },
    EffectDescriptor {
        name: "audio_ducking",
        parameters: &[
            EffectParameterDescriptor {
                name: "threshold_tenth_db",
                min: -600,
                max: 0,
                neutral: -300,
                uniform: EffectUniform::DuckThreshold,
            },
            EffectParameterDescriptor {
                name: "reduction_tenth_db",
                min: 0,
                max: 600,
                neutral: 120,
                uniform: EffectUniform::DuckReduction,
            },
            EffectParameterDescriptor {
                name: "attack_milliseconds",
                min: 1,
                max: 1_000,
                neutral: 20,
                uniform: EffectUniform::DuckAttack,
            },
            EffectParameterDescriptor {
                name: "release_milliseconds",
                min: 10,
                max: 5_000,
                neutral: 300,
                uniform: EffectUniform::DuckRelease,
            },
        ],
    },
    EffectDescriptor {
        name: "audio_limiter",
        parameters: &[EffectParameterDescriptor {
            name: "ceiling_tenth_db",
            min: -120,
            max: 0,
            neutral: -10,
            uniform: EffectUniform::LimiterCeiling,
        }],
    },
];

/// Built-in effects whose historical compositor semantics operate in the
/// display-coded compatibility path rather than the managed CC1 working
/// space. They remain loadable for old projects, but must be surfaced as
/// legacy colour semantics and are not offered for new insertion.
pub const LEGACY_DISPLAY_EFFECT_NAMES: &[&str] = &["brightness", "contrast", "saturation"];

/// Built-in LUT effects that execute after the managed primary correction.
/// They remain supported compatibility stages, but are outside the CC1
/// managed-primary conformance claim and must be reported as such.
pub const POST_PRIMARY_LUT_EFFECT_NAMES: &[&str] = &["look_lut", "cube_lut"];

/// Colour compatibility stage occupied by an effect outside the CC1 managed
/// primary correction.
///
/// Only colour-transform stages are classified here. Alpha/keying operations
/// such as `chroma_key` produce coverage rather than a display-coded colour
/// transform: they are outside colour compatibility staging entirely, are
/// compatible with the managed working space, and therefore return `None`
/// from [`effect_compatibility_stage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectCompatibilityStage {
    /// Historical display-coded controls retained for project compatibility.
    LegacyDisplayCoded,
    /// Built-in looks and imported `.cube` LUTs applied after the primary.
    PostPrimaryLut,
}

impl EffectCompatibilityStage {
    /// Stable QA/delivery issue code for the compatibility stage.
    #[must_use]
    pub const fn issue_code(self) -> &'static str {
        match self {
            Self::LegacyDisplayCoded => "legacy_colour_semantics",
            Self::PostPrimaryLut => "legacy_lut_stage",
        }
    }

    /// Human-facing inspector warning for the compatibility stage.
    #[must_use]
    pub const fn inspector_warning(self) -> &'static str {
        match self {
            Self::LegacyDisplayCoded => {
                "Legacy display semantics · compatibility path; not managed SDR primary"
            }
            Self::PostPrimaryLut => {
                "Post-primary compatibility LUT · outside managed SDR conformance"
            }
        }
    }
}

#[must_use]
pub fn is_audio_effect(name: &str) -> bool {
    matches!(
        name,
        "audio_gain" | "audio_eq" | "audio_compressor" | "audio_ducking" | "audio_limiter"
    )
}

/// Whether an effect is a legacy display-coded colour effect.
#[must_use]
pub fn is_legacy_display_effect(name: &str) -> bool {
    effect_compatibility_stage(name) == Some(EffectCompatibilityStage::LegacyDisplayCoded)
}

/// Classify colour effects that remain supported outside the CC1 managed
/// primary correction.
///
/// [`LEGACY_DISPLAY_EFFECT_NAMES`] and [`POST_PRIMARY_LUT_EFFECT_NAMES`] are
/// the single source of truth so QA, delivery conformance, the inspector, and
/// the compositor cannot drift apart. Effects that are absent from both lists
/// (including alpha-only operations such as `chroma_key`) are managed-path
/// compatible and are not reported as compatibility stages.
#[must_use]
pub fn effect_compatibility_stage(name: &str) -> Option<EffectCompatibilityStage> {
    if LEGACY_DISPLAY_EFFECT_NAMES.contains(&name) {
        return Some(EffectCompatibilityStage::LegacyDisplayCoded);
    }
    if POST_PRIMARY_LUT_EFFECT_NAMES.contains(&name) {
        return Some(EffectCompatibilityStage::PostPrimaryLut);
    }
    None
}

#[must_use]
pub fn effect_descriptor(name: &str) -> Option<EffectDescriptor> {
    EFFECT_DESCRIPTORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.name == name)
}
