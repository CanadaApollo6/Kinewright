use crate::{Effect, ParamValue};

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
    /// Consumed by the ordered colour-node storage buffer, never by the
    /// `LayerParams` uniform block.
    ///
    /// Every CC3 (`color_wheels`, `color_curves`) parameter carries this
    /// uniform. A renderer must route these parameters through the ordered
    /// node stack described in `docs/CC3-CURVES-AND-WHEELS.md` §3.2 and must
    /// never flatten them into the per-layer uniform block.
    ColorNode,
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

/// Inclusive minimum of a `color_wheels` lift control, in basis points of the
/// `grade709` range (CC3 §4.1).
pub const COLOR_WHEEL_LIFT_MIN_BASIS_POINTS: i64 = -2_000;
/// Inclusive maximum of a `color_wheels` lift control (CC3 §4.1).
pub const COLOR_WHEEL_LIFT_MAX_BASIS_POINTS: i64 = 2_000;
/// Inclusive minimum of a `color_wheels` gamma control, in thousandths of an
/// exponent (CC3 §4.1).
pub const COLOR_WHEEL_GAMMA_MIN_THOUSANDTHS: i64 = 100;
/// Inclusive maximum of a `color_wheels` gamma control (CC3 §4.1).
pub const COLOR_WHEEL_GAMMA_MAX_THOUSANDTHS: i64 = 4_000;
/// Inclusive minimum of a `color_wheels` gain control, in thousandths of a
/// slope (CC3 §4.1). Zero slope is a legal boundary state, not an error.
pub const COLOR_WHEEL_GAIN_MIN_THOUSANDTHS: i64 = 0;
/// Inclusive maximum of a `color_wheels` gain control (CC3 §4.1).
pub const COLOR_WHEEL_GAIN_MAX_THOUSANDTHS: i64 = 4_000;
/// Neutral gamma and gain value: `1000` is an exponent/slope of exactly 1.
pub const COLOR_WHEEL_UNITY_THOUSANDTHS: i64 = 1_000;

/// Fewest points a CC3 curve may declare (CC3 §2.3).
pub const COLOR_CURVE_MIN_POINTS: usize = 2;
/// Most points a CC3 curve may declare (CC3 §2.3).
pub const COLOR_CURVE_MAX_POINTS: usize = 16;
/// Inclusive minimum of a curve coordinate, in basis points of the `grade709`
/// range (CC3 §2.3). Points below black keep over-range material shapeable.
pub const COLOR_CURVE_COORDINATE_MIN: i64 = -2_000;
/// Inclusive maximum of a curve coordinate (CC3 §2.3).
pub const COLOR_CURVE_COORDINATE_MAX: i64 = 12_000;
/// Display white in curve coordinates: `10000` basis points of `grade709`.
pub const COLOR_CURVE_WHITE_BASIS_POINTS: i32 = 10_000;
const COLOR_CURVE_MIN_POINTS_I64: i64 = 2;
const COLOR_CURVE_MAX_POINTS_I64: i64 = 16;

/// Parameters owned by one curve: `point_count` plus 16 `(x, y)` pairs.
pub const COLOR_CURVE_PARAMETER_COUNT: usize = 1 + 2 * COLOR_CURVE_MAX_POINTS;
/// Parameters in the `color_curves` descriptor: four curves plus `bypass`.
pub const COLOR_CURVES_PARAMETER_COUNT: usize = 4 * COLOR_CURVE_PARAMETER_COUNT + 1;
/// The most managed colour nodes one layer may carry (CC3 §3.1). Exceeding it
/// is a typed error, never a silent truncation.
pub const COLOR_NODE_LIMIT_PER_LAYER: usize = 16;

/// The canonical per-node bypass control shared by both CC3 nodes (CC3 §5).
///
/// It is an integer token, not a `ParamValue::Boolean`, exactly like
/// `mask.invert`: descriptor validation accepts only `ParamValue::Integer`.
pub const COLOR_NODE_BYPASS_PARAMETER: &str = "bypass";

const COLOR_NODE_BYPASS_DESCRIPTOR: EffectParameterDescriptor = EffectParameterDescriptor {
    name: COLOR_NODE_BYPASS_PARAMETER,
    min: 0,
    max: 1,
    neutral: 0,
    uniform: EffectUniform::ColorNode,
};

/// The twelve `color_wheels` controls plus `bypass`, in CC3 §4.1 table order.
const COLOR_WHEELS_PARAMETERS: [EffectParameterDescriptor; 13] = [
    color_wheel_control("lift_master_basis_points", ColorWheelControl::Lift),
    color_wheel_control("lift_red_basis_points", ColorWheelControl::Lift),
    color_wheel_control("lift_green_basis_points", ColorWheelControl::Lift),
    color_wheel_control("lift_blue_basis_points", ColorWheelControl::Lift),
    color_wheel_control("gamma_master_thousandths", ColorWheelControl::Gamma),
    color_wheel_control("gamma_red_thousandths", ColorWheelControl::Gamma),
    color_wheel_control("gamma_green_thousandths", ColorWheelControl::Gamma),
    color_wheel_control("gamma_blue_thousandths", ColorWheelControl::Gamma),
    color_wheel_control("gain_master_thousandths", ColorWheelControl::Gain),
    color_wheel_control("gain_red_thousandths", ColorWheelControl::Gain),
    color_wheel_control("gain_green_thousandths", ColorWheelControl::Gain),
    color_wheel_control("gain_blue_thousandths", ColorWheelControl::Gain),
    COLOR_NODE_BYPASS_DESCRIPTOR,
];

/// Which of the three ASC CDL control families a wheel parameter belongs to.
///
/// Lift is additive in basis points; gamma and gain are multiplicative in
/// thousandths (CC3 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColorWheelControl {
    /// `lift_*_basis_points`: an offset added before the power step.
    Lift,
    /// `gamma_*_thousandths`: the power exponent.
    Gamma,
    /// `gain_*_thousandths`: the slope.
    Gain,
}

impl ColorWheelControl {
    /// Every control family, in CC3 §4.1 table order.
    pub const ALL: [Self; 3] = [Self::Lift, Self::Gamma, Self::Gain];

    /// Inclusive `(min, max, neutral)` triple for this control family.
    #[must_use]
    pub const fn bounds(self) -> (i64, i64, i64) {
        match self {
            Self::Lift => (
                COLOR_WHEEL_LIFT_MIN_BASIS_POINTS,
                COLOR_WHEEL_LIFT_MAX_BASIS_POINTS,
                0,
            ),
            Self::Gamma => (
                COLOR_WHEEL_GAMMA_MIN_THOUSANDTHS,
                COLOR_WHEEL_GAMMA_MAX_THOUSANDTHS,
                COLOR_WHEEL_UNITY_THOUSANDTHS,
            ),
            Self::Gain => (
                COLOR_WHEEL_GAIN_MIN_THOUSANDTHS,
                COLOR_WHEEL_GAIN_MAX_THOUSANDTHS,
                COLOR_WHEEL_UNITY_THOUSANDTHS,
            ),
        }
    }

    /// The `color_wheels` parameter name for one control and channel.
    #[must_use]
    pub const fn parameter_name(self, channel: ColorWheelChannel) -> &'static str {
        match (self, channel) {
            (Self::Lift, ColorWheelChannel::Master) => "lift_master_basis_points",
            (Self::Lift, ColorWheelChannel::Red) => "lift_red_basis_points",
            (Self::Lift, ColorWheelChannel::Green) => "lift_green_basis_points",
            (Self::Lift, ColorWheelChannel::Blue) => "lift_blue_basis_points",
            (Self::Gamma, ColorWheelChannel::Master) => "gamma_master_thousandths",
            (Self::Gamma, ColorWheelChannel::Red) => "gamma_red_thousandths",
            (Self::Gamma, ColorWheelChannel::Green) => "gamma_green_thousandths",
            (Self::Gamma, ColorWheelChannel::Blue) => "gamma_blue_thousandths",
            (Self::Gain, ColorWheelChannel::Master) => "gain_master_thousandths",
            (Self::Gain, ColorWheelChannel::Red) => "gain_red_thousandths",
            (Self::Gain, ColorWheelChannel::Green) => "gain_green_thousandths",
            (Self::Gain, ColorWheelChannel::Blue) => "gain_blue_thousandths",
        }
    }
}

/// The master ring or one colour channel of a `color_wheels` control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColorWheelChannel {
    Master,
    Red,
    Green,
    Blue,
}

impl ColorWheelChannel {
    /// Every channel, master first, in CC3 §4.1 table order.
    pub const ALL: [Self; 4] = [Self::Master, Self::Red, Self::Green, Self::Blue];
}

const fn color_wheel_control(
    name: &'static str,
    control: ColorWheelControl,
) -> EffectParameterDescriptor {
    let (min, max, neutral) = control.bounds();
    EffectParameterDescriptor {
        name,
        min,
        max,
        neutral,
        uniform: EffectUniform::ColorNode,
    }
}

/// Expand one curve's 33 integer parameters from the CC3 §4.2 patterns.
///
/// The names are built with `concat!` so the 133-entry `color_curves` table is
/// generated rather than transcribed, while every entry stays a plain
/// `&'static str` in a `const` table that agents and the compositor can read
/// without allocating.
macro_rules! color_curve_parameter_table {
    ($curve:literal) => {
        color_curve_parameter_table!(
            @points $curve,
            (0, 0),
            (1, 10_000),
            (2, 10_000),
            (3, 10_000),
            (4, 10_000),
            (5, 10_000),
            (6, 10_000),
            (7, 10_000),
            (8, 10_000),
            (9, 10_000),
            (10, 10_000),
            (11, 10_000),
            (12, 10_000),
            (13, 10_000),
            (14, 10_000),
            (15, 10_000),
        )
    };
    (@points $curve:literal, $(($index:literal, $neutral:literal)),+ $(,)?) => {
        [
            EffectParameterDescriptor {
                name: concat!($curve, "_point_count"),
                min: COLOR_CURVE_MIN_POINTS_I64,
                max: COLOR_CURVE_MAX_POINTS_I64,
                neutral: COLOR_CURVE_MIN_POINTS_I64,
                uniform: EffectUniform::ColorNode,
            },
            $(
                EffectParameterDescriptor {
                    name: concat!($curve, "_x", $index),
                    min: COLOR_CURVE_COORDINATE_MIN,
                    max: COLOR_CURVE_COORDINATE_MAX,
                    neutral: $neutral,
                    uniform: EffectUniform::ColorNode,
                },
                EffectParameterDescriptor {
                    name: concat!($curve, "_y", $index),
                    min: COLOR_CURVE_COORDINATE_MIN,
                    max: COLOR_CURVE_COORDINATE_MAX,
                    neutral: $neutral,
                    uniform: EffectUniform::ColorNode,
                },
            )+
        ]
    };
}

const MASTER_CURVE_PARAMETERS: [EffectParameterDescriptor; COLOR_CURVE_PARAMETER_COUNT] =
    color_curve_parameter_table!("master");
const RED_CURVE_PARAMETERS: [EffectParameterDescriptor; COLOR_CURVE_PARAMETER_COUNT] =
    color_curve_parameter_table!("red");
const GREEN_CURVE_PARAMETERS: [EffectParameterDescriptor; COLOR_CURVE_PARAMETER_COUNT] =
    color_curve_parameter_table!("green");
const BLUE_CURVE_PARAMETERS: [EffectParameterDescriptor; COLOR_CURVE_PARAMETER_COUNT] =
    color_curve_parameter_table!("blue");

const fn curve_parameter_names(
    parameters: &[EffectParameterDescriptor; COLOR_CURVE_PARAMETER_COUNT],
) -> [&'static str; COLOR_CURVE_PARAMETER_COUNT] {
    let mut names = [""; COLOR_CURVE_PARAMETER_COUNT];
    let mut index = 0;
    while index < COLOR_CURVE_PARAMETER_COUNT {
        names[index] = parameters[index].name;
        index += 1;
    }
    names
}

const MASTER_CURVE_PARAMETER_NAMES: [&str; COLOR_CURVE_PARAMETER_COUNT] =
    curve_parameter_names(&MASTER_CURVE_PARAMETERS);
const RED_CURVE_PARAMETER_NAMES: [&str; COLOR_CURVE_PARAMETER_COUNT] =
    curve_parameter_names(&RED_CURVE_PARAMETERS);
const GREEN_CURVE_PARAMETER_NAMES: [&str; COLOR_CURVE_PARAMETER_COUNT] =
    curve_parameter_names(&GREEN_CURVE_PARAMETERS);
const BLUE_CURVE_PARAMETER_NAMES: [&str; COLOR_CURVE_PARAMETER_COUNT] =
    curve_parameter_names(&BLUE_CURVE_PARAMETERS);

/// The 133 `color_curves` parameters: master, red, green, blue, then `bypass`.
const COLOR_CURVES_PARAMETERS: [EffectParameterDescriptor; COLOR_CURVES_PARAMETER_COUNT] = {
    let mut parameters = [COLOR_NODE_BYPASS_DESCRIPTOR; COLOR_CURVES_PARAMETER_COUNT];
    let mut index = 0;
    while index < COLOR_CURVE_PARAMETER_COUNT {
        parameters[index] = MASTER_CURVE_PARAMETERS[index];
        parameters[COLOR_CURVE_PARAMETER_COUNT + index] = RED_CURVE_PARAMETERS[index];
        parameters[2 * COLOR_CURVE_PARAMETER_COUNT + index] = GREEN_CURVE_PARAMETERS[index];
        parameters[3 * COLOR_CURVE_PARAMETER_COUNT + index] = BLUE_CURVE_PARAMETERS[index];
        index += 1;
    }
    parameters
};

/// One of the four curves inside a `color_curves` node (CC3 §2.3).
///
/// Per-channel curves run first; the `master` curve is then applied identically
/// to all three channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColorCurveChannel {
    Master,
    Red,
    Green,
    Blue,
}

impl ColorCurveChannel {
    /// Every curve, in CC3 §4.2 table order.
    pub const ALL: [Self; 4] = [Self::Master, Self::Red, Self::Green, Self::Blue];

    /// The curve's serialized name prefix (`master`, `red`, `green`, `blue`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Red => "red",
            Self::Green => "green",
            Self::Blue => "blue",
        }
    }

    /// Parse a serialized curve prefix.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "master" => Some(Self::Master),
            "red" => Some(Self::Red),
            "green" => Some(Self::Green),
            "blue" => Some(Self::Blue),
            _ => None,
        }
    }

    /// The curve's `{curve}_point_count` parameter name.
    #[must_use]
    pub const fn point_count_parameter(self) -> &'static str {
        self.parameter_names()[0]
    }

    /// The curve's `{curve}_x{index}` parameter name, or `None` past point 15.
    #[must_use]
    pub const fn x_parameter(self, index: usize) -> Option<&'static str> {
        if index >= COLOR_CURVE_MAX_POINTS {
            return None;
        }
        Some(self.parameter_names()[1 + index * 2])
    }

    /// The curve's `{curve}_y{index}` parameter name, or `None` past point 15.
    #[must_use]
    pub const fn y_parameter(self, index: usize) -> Option<&'static str> {
        if index >= COLOR_CURVE_MAX_POINTS {
            return None;
        }
        Some(self.parameter_names()[2 + index * 2])
    }

    /// Every parameter this curve owns: `point_count`, then `x{j}`/`y{j}`.
    #[must_use]
    pub const fn parameter_names(self) -> &'static [&'static str; COLOR_CURVE_PARAMETER_COUNT] {
        match self {
            Self::Master => &MASTER_CURVE_PARAMETER_NAMES,
            Self::Red => &RED_CURVE_PARAMETER_NAMES,
            Self::Green => &GREEN_CURVE_PARAMETER_NAMES,
            Self::Blue => &BLUE_CURVE_PARAMETER_NAMES,
        }
    }

    /// Every descriptor this curve owns, in parameter-name order.
    #[must_use]
    pub const fn parameters(
        self,
    ) -> &'static [EffectParameterDescriptor; COLOR_CURVE_PARAMETER_COUNT] {
        match self {
            Self::Master => &MASTER_CURVE_PARAMETERS,
            Self::Red => &RED_CURVE_PARAMETERS,
            Self::Green => &GREEN_CURVE_PARAMETERS,
            Self::Blue => &BLUE_CURVE_PARAMETERS,
        }
    }

    /// The curve owning one `color_curves` parameter, if any.
    ///
    /// `bypass` belongs to the node rather than to a curve and returns `None`.
    #[must_use]
    pub fn owning(parameter: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|curve| curve.parameter_names().contains(&parameter))
    }
}

/// The 33 parameter names owned by one curve (CC3 §4.2).
///
/// A curve reset writes exactly these names to their neutrals.
#[must_use]
pub const fn color_curve_parameter_names(
    curve: ColorCurveChannel,
) -> &'static [&'static str; COLOR_CURVE_PARAMETER_COUNT] {
    curve.parameter_names()
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
        name: "color_wheels",
        parameters: &COLOR_WHEELS_PARAMETERS,
    },
    EffectDescriptor {
        name: "color_curves",
        parameters: &COLOR_CURVES_PARAMETERS,
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

/// The managed colour-correction node kinds, in the order CC3 §3.1 lists them.
///
/// These three effect names form one ordered node stack executed in
/// `clip.effects` vector order. There is no fixed inter-kind precedence.
pub const MANAGED_COLOR_NODE_NAMES: [&str; 3] =
    ["primary_correction", "color_wheels", "color_curves"];

/// Whether an effect name is a managed colour-correction node (CC3 §3.1).
///
/// Managed nodes are inside the CC1 conformance claim, so they must never be
/// reported through [`effect_compatibility_stage`] as a legacy or LUT stage.
#[must_use]
pub fn is_managed_color_node(name: &str) -> bool {
    MANAGED_COLOR_NODE_NAMES.contains(&name)
}

/// One kind of managed colour node in the ordered CC1/CC3 stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColorNodeKind {
    /// The CC1 managed SDR primary correction.
    Primary,
    /// The CC3 ASC CDL slope/offset/power wheels.
    Wheels,
    /// The CC3 master/red/green/blue curves.
    Curves,
}

impl ColorNodeKind {
    /// Every node kind, in `MANAGED_COLOR_NODE_NAMES` order.
    pub const ALL: [Self; 3] = [Self::Primary, Self::Wheels, Self::Curves];

    /// The canonical effect name for this node kind.
    #[must_use]
    pub const fn effect_name(self) -> &'static str {
        match self {
            Self::Primary => "primary_correction",
            Self::Wheels => "color_wheels",
            Self::Curves => "color_curves",
        }
    }

    /// Classify a registered effect name.
    #[must_use]
    pub fn from_effect_name(name: &str) -> Option<Self> {
        match name {
            "primary_correction" => Some(Self::Primary),
            "color_wheels" => Some(Self::Wheels),
            "color_curves" => Some(Self::Curves),
            _ => None,
        }
    }

    /// The node-kind tag written into the ordered grade storage buffer
    /// (CC3 §3.2): `1` primary, `2` wheels, `3` curves.
    #[must_use]
    pub const fn storage_buffer_tag(self) -> u32 {
        match self {
            Self::Primary => 1,
            Self::Wheels => 2,
            Self::Curves => 3,
        }
    }
}

/// Why a managed colour node is the exact identity for a rendered frame.
///
/// Reported verbatim by manifests and the inspector as `bypassed` or `neutral`
/// (CC3 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColorNodeInactiveReason {
    /// The evaluated `bypass` control is `>= 1`.
    Bypassed,
    /// Every stored control equals its descriptor neutral, or every curve is
    /// structurally identity.
    Neutral,
}

impl ColorNodeInactiveReason {
    /// Stable manifest/inspector token for the reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bypassed => "bypassed",
            Self::Neutral => "neutral",
        }
    }
}

/// The four values of one `color_wheels` control family (CC3 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorWheelControlSet {
    pub master: i64,
    pub red: i64,
    pub green: i64,
    pub blue: i64,
}

impl ColorWheelControlSet {
    /// Every channel at one value.
    #[must_use]
    pub const fn uniform(value: i64) -> Self {
        Self {
            master: value,
            red: value,
            green: value,
            blue: value,
        }
    }

    /// One channel's stored integer.
    #[must_use]
    pub const fn channel(self, channel: ColorWheelChannel) -> i64 {
        match channel {
            ColorWheelChannel::Master => self.master,
            ColorWheelChannel::Red => self.red,
            ColorWheelChannel::Green => self.green,
            ColorWheelChannel::Blue => self.blue,
        }
    }
}

/// Every `color_wheels` control resolved to its stored integer (CC3 §4.1).
///
/// Callers pass a *keyframe-evaluated* effect ([`Effect::evaluated_at`]); this
/// type performs no automation evaluation of its own. Omitted parameters
/// resolve to their descriptor neutrals, so a node created with only the
/// controls a colourist touched behaves exactly like a fully populated one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorWheelsParams {
    /// `lift_*_basis_points`, added before the power step.
    pub lift_basis_points: ColorWheelControlSet,
    /// `gamma_*_thousandths`, the power exponent.
    pub gamma_thousandths: ColorWheelControlSet,
    /// `gain_*_thousandths`, the slope.
    pub gain_thousandths: ColorWheelControlSet,
    /// The stored `bypass` token, `0` or `1`.
    pub bypass_token: i64,
}

impl ColorWheelsParams {
    /// The all-neutral node, which is the exact identity.
    pub const NEUTRAL: Self = Self {
        lift_basis_points: ColorWheelControlSet::uniform(0),
        gamma_thousandths: ColorWheelControlSet::uniform(COLOR_WHEEL_UNITY_THOUSANDTHS),
        gain_thousandths: ColorWheelControlSet::uniform(COLOR_WHEEL_UNITY_THOUSANDTHS),
        bypass_token: 0,
    };

    /// Resolve a keyframe-evaluated `color_wheels` effect.
    ///
    /// An absent or non-integer parameter resolves to its neutral, and a value
    /// outside the descriptor range is clamped into it, so a rendered frame
    /// can never fail. Neither case can be reached through the edit path,
    /// which rejects both atomically.
    #[must_use]
    pub fn from_effect(effect: &Effect) -> Self {
        let control = |control: ColorWheelControl, channel: ColorWheelChannel| {
            let name = control.parameter_name(channel);
            let (min, max, neutral) = control.bounds();
            stored_integer(effect, name, neutral).clamp(min, max)
        };
        let set = |family: ColorWheelControl| ColorWheelControlSet {
            master: control(family, ColorWheelChannel::Master),
            red: control(family, ColorWheelChannel::Red),
            green: control(family, ColorWheelChannel::Green),
            blue: control(family, ColorWheelChannel::Blue),
        };
        Self {
            lift_basis_points: set(ColorWheelControl::Lift),
            gamma_thousandths: set(ColorWheelControl::Gamma),
            gain_thousandths: set(ColorWheelControl::Gain),
            bypass_token: stored_integer(effect, COLOR_NODE_BYPASS_PARAMETER, 0).clamp(0, 1),
        }
    }

    /// One control's stored integer.
    #[must_use]
    pub const fn control(self, control: ColorWheelControl, channel: ColorWheelChannel) -> i64 {
        match control {
            ColorWheelControl::Lift => self.lift_basis_points.channel(channel),
            ColorWheelControl::Gamma => self.gamma_thousandths.channel(channel),
            ColorWheelControl::Gain => self.gain_thousandths.channel(channel),
        }
    }

    /// Whether all twelve controls equal their descriptor neutrals (CC3 §3.3).
    ///
    /// Neutrality is tested on the stored integers, never on floats, so the
    /// identity gate is bit-identical rather than tolerance-bounded.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        self.lift_basis_points == Self::NEUTRAL.lift_basis_points
            && self.gamma_thousandths == Self::NEUTRAL.gamma_thousandths
            && self.gain_thousandths == Self::NEUTRAL.gain_thousandths
    }

    /// Whether the evaluated `bypass` control is `>= 1`.
    #[must_use]
    pub const fn bypass(&self) -> bool {
        self.bypass_token >= 1
    }

    /// Why this node is the identity for the evaluated frame, if it is.
    #[must_use]
    pub fn inactive_reason(&self) -> Option<ColorNodeInactiveReason> {
        if self.bypass() {
            Some(ColorNodeInactiveReason::Bypassed)
        } else if self.is_neutral() {
            Some(ColorNodeInactiveReason::Neutral)
        } else {
            None
        }
    }

    /// Whether the renderer must evaluate this node.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.inactive_reason().is_none()
    }
}

/// One curve's resolved point list in curve basis points (CC3 §2.3, §3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurvePoints {
    /// The active points, always at least two and strictly increasing in `x`.
    pub points: Vec<(i32, i32)>,
    /// The `{curve}_point_count` the node declared before truncation.
    pub declared_point_count: usize,
    /// Whether §3.4 truncation dropped points whose `x` was not strictly
    /// increasing after keyframe evaluation.
    pub truncated: bool,
}

impl CurvePoints {
    /// The structural identity curve, `(0, 0)` and `(10000, 10000)`.
    #[must_use]
    pub fn identity() -> Self {
        Self {
            points: vec![
                (0, 0),
                (
                    COLOR_CURVE_WHITE_BASIS_POINTS,
                    COLOR_CURVE_WHITE_BASIS_POINTS,
                ),
            ],
            declared_point_count: COLOR_CURVE_MIN_POINTS,
            truncated: false,
        }
    }

    /// Resolve one curve of a keyframe-evaluated `color_curves` effect.
    ///
    /// Points at index `>= point_count` are ignored, including their
    /// deliberately colliding neutral `(10000, 10000)`. The active prefix is
    /// then truncated to its longest strictly-increasing-`x` run (CC3 §3.4);
    /// a run shorter than two points resolves to the identity curve. No
    /// clamping, no reordering, and no error: rendering a legal document must
    /// never fail.
    #[must_use]
    pub fn from_effect(effect: &Effect, curve: ColorCurveChannel) -> Self {
        let declared = usize::try_from(
            stored_integer(
                effect,
                curve.point_count_parameter(),
                COLOR_CURVE_MIN_POINTS_I64,
            )
            .clamp(COLOR_CURVE_MIN_POINTS_I64, COLOR_CURVE_MAX_POINTS_I64),
        )
        .unwrap_or(COLOR_CURVE_MIN_POINTS);
        let mut points = Vec::with_capacity(declared);
        for index in 0..declared {
            let (Some(x_name), Some(y_name)) = (curve.x_parameter(index), curve.y_parameter(index))
            else {
                break;
            };
            let neutral = point_coordinate_neutral(index);
            let x = stored_integer(effect, x_name, neutral);
            let y = stored_integer(effect, y_name, neutral);
            points.push((clamp_coordinate(x), clamp_coordinate(y)));
        }
        let prefix = strictly_increasing_prefix(&points);
        if prefix < COLOR_CURVE_MIN_POINTS {
            return Self {
                truncated: true,
                declared_point_count: declared,
                ..Self::identity()
            };
        }
        let truncated = prefix < points.len();
        points.truncate(prefix);
        Self {
            points,
            declared_point_count: declared,
            truncated,
        }
    }

    /// Whether this curve is *structurally* identity: exactly two points,
    /// `(0, 0)` and `(10000, 10000)` (CC3 §2.3).
    ///
    /// Only structural identity triggers the §3.3 bit-identity short-circuit.
    /// A collinear 16-point curve is mathematically identity but is still
    /// evaluated.
    #[must_use]
    pub fn is_structural_identity(&self) -> bool {
        self.points.as_slice()
            == [
                (0, 0),
                (
                    COLOR_CURVE_WHITE_BASIS_POINTS,
                    COLOR_CURVE_WHITE_BASIS_POINTS,
                ),
            ]
    }
}

/// Every curve of a `color_curves` node resolved for one rendered frame.
///
/// Callers pass a *keyframe-evaluated* effect ([`Effect::evaluated_at`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCurves {
    pub master: CurvePoints,
    pub red: CurvePoints,
    pub green: CurvePoints,
    pub blue: CurvePoints,
    /// The stored `bypass` token, `0` or `1`.
    pub bypass_token: i64,
}

impl ResolvedCurves {
    /// Resolve all four curves of a keyframe-evaluated `color_curves` effect.
    #[must_use]
    pub fn from_effect(effect: &Effect) -> Self {
        Self {
            master: CurvePoints::from_effect(effect, ColorCurveChannel::Master),
            red: CurvePoints::from_effect(effect, ColorCurveChannel::Red),
            green: CurvePoints::from_effect(effect, ColorCurveChannel::Green),
            blue: CurvePoints::from_effect(effect, ColorCurveChannel::Blue),
            bypass_token: stored_integer(effect, COLOR_NODE_BYPASS_PARAMETER, 0).clamp(0, 1),
        }
    }

    /// One resolved curve.
    #[must_use]
    pub const fn curve(&self, curve: ColorCurveChannel) -> &CurvePoints {
        match curve {
            ColorCurveChannel::Master => &self.master,
            ColorCurveChannel::Red => &self.red,
            ColorCurveChannel::Green => &self.green,
            ColorCurveChannel::Blue => &self.blue,
        }
    }

    /// Whether all four curves are structurally identity (CC3 §3.3).
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        ColorCurveChannel::ALL
            .into_iter()
            .all(|curve| self.curve(curve).is_structural_identity())
    }

    /// Whether the evaluated `bypass` control is `>= 1`.
    #[must_use]
    pub const fn bypass(&self) -> bool {
        self.bypass_token >= 1
    }

    /// The curves the §3.4 truncation rule shortened, in `ALL` order.
    #[must_use]
    pub fn truncated_curves(&self) -> Vec<ColorCurveChannel> {
        ColorCurveChannel::ALL
            .into_iter()
            .filter(|curve| self.curve(*curve).truncated)
            .collect()
    }

    /// Whether any curve was truncated by automation (CC3 §3.4).
    #[must_use]
    pub fn truncated(&self) -> bool {
        ColorCurveChannel::ALL
            .into_iter()
            .any(|curve| self.curve(curve).truncated)
    }

    /// Why this node is the identity for the evaluated frame, if it is.
    #[must_use]
    pub fn inactive_reason(&self) -> Option<ColorNodeInactiveReason> {
        if self.bypass() {
            Some(ColorNodeInactiveReason::Bypassed)
        } else if self.is_neutral() {
            Some(ColorNodeInactiveReason::Neutral)
        } else {
            None
        }
    }

    /// Whether the renderer must evaluate this node.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.inactive_reason().is_none()
    }
}

/// Classify one effect as a managed colour node.
///
/// Returns `None` for every other effect, including the legacy display-coded
/// controls and the post-primary LUT stages.
#[must_use]
pub fn classify_color_node(effect: &Effect) -> Option<ColorNodeKind> {
    ColorNodeKind::from_effect_name(&effect.name)
}

/// Why a managed colour node is inactive for one rendered frame, if it is.
///
/// The caller passes a *keyframe-evaluated* effect ([`Effect::evaluated_at`]);
/// CC3 §3.3 resolves keyframes first, then tests inactivity on the stored
/// integers. `primary_correction` has neither a `bypass` control nor a CC1
/// neutral short-circuit, so it is always reported active and its CC1
/// rendering is unchanged.
#[must_use]
pub fn color_node_inactive_reason(effect: &Effect) -> Option<ColorNodeInactiveReason> {
    match classify_color_node(effect)? {
        ColorNodeKind::Primary => None,
        ColorNodeKind::Wheels => ColorWheelsParams::from_effect(effect).inactive_reason(),
        ColorNodeKind::Curves => ResolvedCurves::from_effect(effect).inactive_reason(),
    }
}

/// The managed colour nodes a renderer must evaluate, with their positions in
/// `clip.effects`.
///
/// The caller passes *keyframe-evaluated* effects; inactive nodes (CC3 §3.3)
/// are omitted because an inactive node is the exact identity and must not be
/// written to the GPU buffer or evaluated by the CPU reference. The returned
/// indices are positions in the caller's slice, so the ordered stack keeps its
/// serialized order with no reordering, flattening, or merging.
#[must_use]
pub fn active_color_nodes(effects: &[Effect]) -> Vec<(usize, ColorNodeKind)> {
    effects
        .iter()
        .enumerate()
        .filter_map(|(index, effect)| {
            let kind = classify_color_node(effect)?;
            color_node_inactive_reason(effect)
                .is_none()
                .then_some((index, kind))
        })
        .collect()
}

/// How many managed colour nodes one effect stack carries (CC3 §3.1).
///
/// Counts every managed node, active or not: a bypassed node keeps its place
/// in `clip.effects` and still occupies one of the sixteen slots.
#[must_use]
pub fn managed_color_node_count(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| is_managed_color_node(&effect.name))
        .count()
}

/// The first violation of the strictly-increasing `x` rule (CC3 §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorCurveOrderViolation {
    /// The curve that violates the rule.
    pub curve: ColorCurveChannel,
    /// The index of the offending point inside the active prefix.
    pub index: usize,
    /// The previous point's `x`.
    pub previous_x: i64,
    /// The offending point's `x`.
    pub x: i64,
}

/// The first strictly-increasing-`x` violation in a `color_curves` node's
/// *stored static* parameters, if any (CC3 §2.3).
///
/// Keyframes are deliberately ignored: `AddEffect` and `SetEffectParam`
/// validate static values, while resolved curves that still cross after
/// keyframe evaluation are handled by the §3.4 truncation rule instead of
/// failing a render. Points at index `>= point_count` are not examined, so
/// their colliding neutrals are legal.
#[must_use]
pub fn color_curve_order_violation(effect: &Effect) -> Option<ColorCurveOrderViolation> {
    if ColorNodeKind::from_effect_name(&effect.name) != Some(ColorNodeKind::Curves) {
        return None;
    }
    ColorCurveChannel::ALL
        .into_iter()
        .find_map(|curve| curve_order_violation(effect, curve))
}

fn curve_order_violation(
    effect: &Effect,
    curve: ColorCurveChannel,
) -> Option<ColorCurveOrderViolation> {
    let declared = stored_integer(
        effect,
        curve.point_count_parameter(),
        COLOR_CURVE_MIN_POINTS_I64,
    )
    .clamp(COLOR_CURVE_MIN_POINTS_I64, COLOR_CURVE_MAX_POINTS_I64);
    let declared = usize::try_from(declared).unwrap_or(COLOR_CURVE_MIN_POINTS);
    let mut previous_x = None;
    for index in 0..declared {
        let Some(x_name) = curve.x_parameter(index) else {
            break;
        };
        let x = stored_integer(effect, x_name, point_coordinate_neutral(index));
        if let Some(previous_x) = previous_x
            && x <= previous_x
        {
            return Some(ColorCurveOrderViolation {
                curve,
                index,
                previous_x,
                x,
            });
        }
        previous_x = Some(x);
    }
    None
}

/// The descriptor neutral of point `index`: `0` for point 0, else `10000`.
///
/// The deliberate collision at `(10000, 10000)` for every later point is what
/// lets an omitted point resolve to a legal neutral (CC3 §4.2).
const fn point_coordinate_neutral(index: usize) -> i64 {
    if index == 0 {
        0
    } else {
        COLOR_CURVE_WHITE_BASIS_POINTS as i64
    }
}

/// Read one stored integer parameter, falling back to its neutral.
///
/// Automation is *not* consulted: callers pass an already-evaluated effect.
fn stored_integer(effect: &Effect, name: &str, neutral: i64) -> i64 {
    match effect.parameters.get(name) {
        Some(ParamValue::Integer(value)) => *value,
        Some(ParamValue::Boolean(_) | ParamValue::Text(_)) | None => neutral,
    }
}

fn clamp_coordinate(value: i64) -> i32 {
    let clamped = value.clamp(COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_COORDINATE_MAX);
    i32::try_from(clamped).unwrap_or(COLOR_CURVE_WHITE_BASIS_POINTS)
}

/// Length of the longest prefix whose `x` coordinates strictly increase.
fn strictly_increasing_prefix(points: &[(i32, i32)]) -> usize {
    let mut prefix = 0;
    for (index, point) in points.iter().enumerate() {
        if index > 0 && point.0 <= points[index - 1].0 {
            break;
        }
        prefix = index + 1;
    }
    prefix
}

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
