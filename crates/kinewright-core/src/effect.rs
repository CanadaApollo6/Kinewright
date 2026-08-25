use crate::{Effect, EffectId, LutAssetId, ParamValue};

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

/// The most LUT nodes one layer may carry, counting `technical_lut` and
/// `creative_look` together (CC4 §3.1).
///
/// Tighter than [`COLOR_NODE_LIMIT_PER_LAYER`] because each LUT node needs a
/// texture atlas slot. Exceeding it is a typed error, never a silent drop.
pub const LUT_NODE_LIMIT_PER_LAYER: usize = 4;

/// The asset-reference control shared by both CC4 LUT node kinds (CC4 §3.3).
///
/// An ordinary `ParamValue::Integer` whose value is a `LutAssetId`. `0` means
/// *unbound*, which makes the node inactive; a valid document never stores it.
pub const LUT_ASSET_ID_PARAMETER: &str = "lut_asset_id";

/// The strength control shared by both CC4 LUT node kinds (CC4 §5).
pub const LUT_MIX_PARAMETER: &str = "mix_basis_points";

/// The input-encoding control shared by both CC4 LUT node kinds (CC4 §3.4).
pub const LUT_INPUT_ENCODING_PARAMETER: &str = "input_encoding_token";

/// Full look strength in basis points, and the pinned `technical_lut` value.
pub const LUT_MIX_BASIS_POINTS_MAX: i64 = 10_000;

/// The highest `input_encoding_token`: `0` display709, `1` linear, `2`
/// grade709 (CC4 §3.4).
pub const LUT_INPUT_ENCODING_TOKEN_MAX: i64 = 2;

/// [`crate::LUT_ASSET_ID_MAX`] as the descriptor's inclusive integer maximum.
///
/// The two constants are asserted equal by the CC4 descriptor fixture, so the
/// `u64` model bound and the `i64` descriptor bound cannot drift apart.
const LUT_ASSET_ID_DESCRIPTOR_MAX: i64 = 9_007_199_254_740_991;

/// Position of `lut_asset_id` in both LUT parameter tables.
const LUT_ASSET_ID_INDEX: usize = 0;
/// Position of `mix_basis_points` in both LUT parameter tables.
const LUT_MIX_INDEX: usize = 1;
/// Position of `input_encoding_token` in both LUT parameter tables.
const LUT_INPUT_ENCODING_INDEX: usize = 2;
/// Position of `bypass` in both LUT parameter tables.
const LUT_BYPASS_INDEX: usize = 3;

const LUT_ASSET_ID_DESCRIPTOR: EffectParameterDescriptor = EffectParameterDescriptor {
    name: LUT_ASSET_ID_PARAMETER,
    min: 0,
    max: LUT_ASSET_ID_DESCRIPTOR_MAX,
    neutral: 0,
    uniform: EffectUniform::ColorNode,
};

const LUT_INPUT_ENCODING_DESCRIPTOR: EffectParameterDescriptor = EffectParameterDescriptor {
    name: LUT_INPUT_ENCODING_PARAMETER,
    min: 0,
    max: LUT_INPUT_ENCODING_TOKEN_MAX,
    neutral: 0,
    uniform: EffectUniform::ColorNode,
};

/// The four `technical_lut` controls, in CC4 §5.1 table order.
///
/// `mix_basis_points` is pinned by its bounds rather than by a special case: a
/// partially applied technical normalization is not a meaningful state.
const TECHNICAL_LUT_PARAMETERS: [EffectParameterDescriptor; 4] = [
    LUT_ASSET_ID_DESCRIPTOR,
    EffectParameterDescriptor {
        name: LUT_MIX_PARAMETER,
        min: LUT_MIX_BASIS_POINTS_MAX,
        max: LUT_MIX_BASIS_POINTS_MAX,
        neutral: LUT_MIX_BASIS_POINTS_MAX,
        uniform: EffectUniform::ColorNode,
    },
    LUT_INPUT_ENCODING_DESCRIPTOR,
    COLOR_NODE_BYPASS_DESCRIPTOR,
];

/// The four `creative_look` controls, in CC4 §5.2 table order.
///
/// The neutral of `mix_basis_points` is full strength, not zero: a look node
/// created with only `lut_asset_id` set must show the look.
const CREATIVE_LOOK_PARAMETERS: [EffectParameterDescriptor; 4] = [
    LUT_ASSET_ID_DESCRIPTOR,
    EffectParameterDescriptor {
        name: LUT_MIX_PARAMETER,
        min: 0,
        max: LUT_MIX_BASIS_POINTS_MAX,
        neutral: LUT_MIX_BASIS_POINTS_MAX,
        uniform: EffectUniform::ColorNode,
    },
    LUT_INPUT_ENCODING_DESCRIPTOR,
    COLOR_NODE_BYPASS_DESCRIPTOR,
];

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

/// The ten CC1 `primary_correction` controls, before the CC5 matte
/// parameters are appended (CC1 §4).
const PRIMARY_CORRECTION_PARAMETERS: [EffectParameterDescriptor; 10] = [
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
];

// ---------------------------------------------------------------------------
// CC5 §2.2 matte parameters
// ---------------------------------------------------------------------------

/// The most geometric windows one matte may carry (CC5 §2.2).
pub const MATTE_WINDOW_LIMIT: usize = 4;
/// [`MATTE_WINDOW_LIMIT`] as the `matte_window_count` descriptor maximum.
const MATTE_WINDOW_LIMIT_I64: i64 = 4;
const _: () = assert!(MATTE_WINDOW_LIMIT == 4 && MATTE_WINDOW_LIMIT_I64 == 4);

/// Matte controls that belong to the node rather than to one window
/// (CC5 §2.2, first table).
pub const MATTE_CONTROL_PARAMETER_COUNT: usize = 15;

/// Controls one geometric window owns (CC5 §2.2, second table).
pub const MATTE_WINDOW_PARAMETER_COUNT: usize = 8;

/// Every parameter a matte-carrying node gains: 15 node controls plus
/// `4 × 8` window controls (CC5 §2.2).
pub const MATTE_PARAMETER_COUNT: usize =
    MATTE_CONTROL_PARAMETER_COUNT + MATTE_WINDOW_LIMIT * MATTE_WINDOW_PARAMETER_COUNT;

/// The `matte_hue_width_centidegrees` value that disables the hue leg
/// entirely, achromatic pixels included (CC5 §2.4).
///
/// It is simultaneously that control's maximum and its neutral: 180° is a
/// half-width covering the whole wheel, so the boundary of the range is the
/// switch rather than a special-cased flag.
pub const MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES: i64 = 18_000;

/// Full coverage in `matte_mix_basis_points`, and that control's neutral.
pub const MATTE_MIX_BASIS_POINTS_MAX: i64 = 10_000;

/// Inclusive minimum of a window centre, in basis points of the frame extent
/// (CC5 §2.2). Off-frame centres are legal so a tracked window may leave and
/// re-enter the picture.
pub const MATTE_WINDOW_CENTER_MIN_BASIS_POINTS: i64 = -10_000;
/// Inclusive maximum of a window centre (CC5 §2.2).
pub const MATTE_WINDOW_CENTER_MAX_BASIS_POINTS: i64 = 20_000;
/// Inclusive minimum of a window half-extent (CC5 §2.2). A zero half-extent is
/// not authorable, which is why keyframes can never resolve a degenerate
/// window.
pub const MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS: i64 = 1;
/// Inclusive maximum of a window half-extent (CC5 §2.2).
pub const MATTE_WINDOW_HALF_EXTENT_MAX_BASIS_POINTS: i64 = 10_000;
/// Inclusive rotation limit in hundredths of a degree (CC5 §2.2). A window is
/// symmetric under 180°, so `±18000` covers every orientation.
pub const MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES: i64 = 18_000;

/// The saturation band token reported by [`MatteParams::degenerate_bands`].
pub const MATTE_SATURATION_BAND: &str = "saturation";
/// The luma band token reported by [`MatteParams::degenerate_bands`].
pub const MATTE_LUMA_BAND: &str = "luma";

const fn matte_control(
    name: &'static str,
    min: i64,
    max: i64,
    neutral: i64,
) -> EffectParameterDescriptor {
    EffectParameterDescriptor {
        name,
        min,
        max,
        neutral,
        uniform: EffectUniform::ColorNode,
    }
}

/// Expand the CC5 §2.2 parameter patterns.
///
/// `@controls` emits the fifteen node-owned controls in table order;
/// `@window {j}` emits window `j`'s eight controls with their names built by
/// `concat!`, so the 32 window parameters are generated rather than
/// transcribed while every entry stays a plain `&'static str` in a `const`
/// table that agents and the compositor can read without allocating. This is
/// how CC3 generates its 133 curve parameters.
macro_rules! matte_parameter_table {
    (@controls) => {
        [
            matte_control("matte_enabled", 0, 1, 0),
            matte_control("matte_window_count", 0, MATTE_WINDOW_LIMIT_I64, 0),
            matte_control("matte_combine_token", 0, 1, 0),
            matte_control("matte_invert", 0, 1, 0),
            matte_control(
                "matte_mix_basis_points",
                0,
                MATTE_MIX_BASIS_POINTS_MAX,
                MATTE_MIX_BASIS_POINTS_MAX,
            ),
            matte_control("matte_qualifier_enabled", 0, 1, 0),
            matte_control("matte_hue_center_centidegrees", 0, 35_999, 0),
            matte_control(
                "matte_hue_width_centidegrees",
                0,
                MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES,
                MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES,
            ),
            matte_control("matte_hue_softness_centidegrees", 0, 18_000, 0),
            matte_control("matte_saturation_low_basis_points", 0, 10_000, 0),
            matte_control("matte_saturation_high_basis_points", 0, 10_000, 10_000),
            matte_control("matte_saturation_softness_basis_points", 0, 10_000, 0),
            matte_control("matte_luma_low_basis_points", 0, 10_000, 0),
            matte_control("matte_luma_high_basis_points", 0, 10_000, 10_000),
            matte_control("matte_luma_softness_basis_points", 0, 10_000, 0),
        ]
    };
    (@window $index:literal) => {
        [
            matte_control(concat!("matte_window", $index, "_shape_token"), 1, 2, 1),
            matte_control(
                concat!("matte_window", $index, "_center_x_basis_points"),
                MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
                5_000,
            ),
            matte_control(
                concat!("matte_window", $index, "_center_y_basis_points"),
                MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
                5_000,
            ),
            matte_control(
                concat!("matte_window", $index, "_half_width_basis_points"),
                MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS,
                MATTE_WINDOW_HALF_EXTENT_MAX_BASIS_POINTS,
                2_500,
            ),
            matte_control(
                concat!("matte_window", $index, "_half_height_basis_points"),
                MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS,
                MATTE_WINDOW_HALF_EXTENT_MAX_BASIS_POINTS,
                2_500,
            ),
            matte_control(
                concat!("matte_window", $index, "_rotation_centidegrees"),
                -MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES,
                MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES,
                0,
            ),
            matte_control(
                concat!("matte_window", $index, "_feather_basis_points"),
                0,
                10_000,
                0,
            ),
            matte_control(concat!("matte_window", $index, "_invert"), 0, 1, 0),
        ]
    };
}

const MATTE_CONTROL_PARAMETERS: [EffectParameterDescriptor; MATTE_CONTROL_PARAMETER_COUNT] =
    matte_parameter_table!(@controls);

const MATTE_WINDOW_PARAMETERS: [[EffectParameterDescriptor; MATTE_WINDOW_PARAMETER_COUNT];
    MATTE_WINDOW_LIMIT] = [
    matte_parameter_table!(@window 0),
    matte_parameter_table!(@window 1),
    matte_parameter_table!(@window 2),
    matte_parameter_table!(@window 3),
];

/// The 47 matte descriptors: the fifteen node controls, then window `0..=3`.
const MATTE_PARAMETERS: [EffectParameterDescriptor; MATTE_PARAMETER_COUNT] = {
    let mut parameters = [MATTE_CONTROL_PARAMETERS[0]; MATTE_PARAMETER_COUNT];
    let mut index = 0;
    while index < MATTE_CONTROL_PARAMETER_COUNT {
        parameters[index] = MATTE_CONTROL_PARAMETERS[index];
        index += 1;
    }
    let mut window = 0;
    while window < MATTE_WINDOW_LIMIT {
        let mut control = 0;
        while control < MATTE_WINDOW_PARAMETER_COUNT {
            parameters
                [MATTE_CONTROL_PARAMETER_COUNT + window * MATTE_WINDOW_PARAMETER_COUNT + control] =
                MATTE_WINDOW_PARAMETERS[window][control];
            control += 1;
        }
        window += 1;
    }
    parameters
};

const MATTE_PARAMETER_NAMES: [&str; MATTE_PARAMETER_COUNT] = {
    let mut names = [""; MATTE_PARAMETER_COUNT];
    let mut index = 0;
    while index < MATTE_PARAMETER_COUNT {
        names[index] = MATTE_PARAMETERS[index].name;
        index += 1;
    }
    names
};

const MATTE_WINDOW_PARAMETER_NAMES: [[&str; MATTE_WINDOW_PARAMETER_COUNT]; MATTE_WINDOW_LIMIT] = {
    let mut names = [[""; MATTE_WINDOW_PARAMETER_COUNT]; MATTE_WINDOW_LIMIT];
    let mut window = 0;
    while window < MATTE_WINDOW_LIMIT {
        let mut control = 0;
        while control < MATTE_WINDOW_PARAMETER_COUNT {
            names[window][control] = MATTE_WINDOW_PARAMETERS[window][control].name;
            control += 1;
        }
        window += 1;
    }
    names
};

const MATTE_ENABLED_INDEX: usize = 0;
const MATTE_WINDOW_COUNT_INDEX: usize = 1;
const MATTE_COMBINE_TOKEN_INDEX: usize = 2;
const MATTE_INVERT_INDEX: usize = 3;
const MATTE_MIX_INDEX: usize = 4;
const MATTE_QUALIFIER_ENABLED_INDEX: usize = 5;
const MATTE_HUE_CENTER_INDEX: usize = 6;
const MATTE_HUE_WIDTH_INDEX: usize = 7;
const MATTE_HUE_SOFTNESS_INDEX: usize = 8;
const MATTE_SATURATION_LOW_INDEX: usize = 9;
const MATTE_SATURATION_HIGH_INDEX: usize = 10;
const MATTE_SATURATION_SOFTNESS_INDEX: usize = 11;
const MATTE_LUMA_LOW_INDEX: usize = 12;
const MATTE_LUMA_HIGH_INDEX: usize = 13;
const MATTE_LUMA_SOFTNESS_INDEX: usize = 14;

const MATTE_WINDOW_SHAPE_INDEX: usize = 0;
const MATTE_WINDOW_CENTER_X_INDEX: usize = 1;
const MATTE_WINDOW_CENTER_Y_INDEX: usize = 2;
const MATTE_WINDOW_HALF_WIDTH_INDEX: usize = 3;
const MATTE_WINDOW_HALF_HEIGHT_INDEX: usize = 4;
const MATTE_WINDOW_ROTATION_INDEX: usize = 5;
const MATTE_WINDOW_FEATHER_INDEX: usize = 6;
const MATTE_WINDOW_INVERT_INDEX: usize = 7;

/// The 47 matte descriptors every matte-capable kind carries (CC5 §2.2).
///
/// Appended verbatim to each of the four matte-capable descriptors, so the
/// bounds and neutrals cannot drift between kinds.
#[must_use]
pub const fn matte_parameters() -> &'static [EffectParameterDescriptor; MATTE_PARAMETER_COUNT] {
    &MATTE_PARAMETERS
}

/// The 47 matte parameter names, node controls first, then window `0..=3`.
///
/// A matte reset writes exactly these names to their neutrals.
#[must_use]
pub const fn matte_parameter_names() -> &'static [&'static str; MATTE_PARAMETER_COUNT] {
    &MATTE_PARAMETER_NAMES
}

/// The eight parameter names window `j` owns, or `None` past window 3.
#[must_use]
pub const fn matte_window_parameter_names(
    window: usize,
) -> Option<&'static [&'static str; MATTE_WINDOW_PARAMETER_COUNT]> {
    if window >= MATTE_WINDOW_LIMIT {
        return None;
    }
    Some(&MATTE_WINDOW_PARAMETER_NAMES[window])
}

/// The eight descriptors window `j` owns, or `None` past window 3.
#[must_use]
pub const fn matte_window_parameters(
    window: usize,
) -> Option<&'static [EffectParameterDescriptor; MATTE_WINDOW_PARAMETER_COUNT]> {
    if window >= MATTE_WINDOW_LIMIT {
        return None;
    }
    Some(&MATTE_WINDOW_PARAMETERS[window])
}

/// Whether `name` is one of the 47 CC5 matte parameters.
///
/// Name-based rather than kind-based: `technical_lut` carries none of them, so
/// naming one there is the ordinary [`crate::OpError::UnknownEffectParam`]
/// rejection (CC5 §2.1).
#[must_use]
pub fn is_matte_parameter(name: &str) -> bool {
    MATTE_PARAMETER_NAMES.contains(&name)
}

/// Whether a node kind may carry a matte (CC5 §2.1).
#[must_use]
pub const fn matte_capable(kind: ColorNodeKind) -> bool {
    kind.supports_matte()
}

/// Whether an effect name is a matte-capable managed colour node (CC5 §2.1).
#[must_use]
pub fn is_matte_capable_color_node(name: &str) -> bool {
    ColorNodeKind::from_effect_name(name).is_some_and(ColorNodeKind::supports_matte)
}

/// Whether a matte parameter accepts `Hold` keyframes only (CC5 §5.1).
///
/// Tokens and counts switch discontinuously: interpolating between union and
/// intersection, or between one window and three, resolves states nobody
/// authored. Every other matte control — the window geometry, the mix, and
/// every qualifier scalar — is fully keyframable with any interpolation.
#[must_use]
pub fn is_hold_only_matte_parameter(name: &str) -> bool {
    if matches!(
        name,
        "matte_enabled"
            | "matte_window_count"
            | "matte_combine_token"
            | "matte_invert"
            | "matte_qualifier_enabled"
    ) {
        return true;
    }
    MATTE_WINDOW_PARAMETER_NAMES.iter().any(|window| {
        window[MATTE_WINDOW_SHAPE_INDEX] == name || window[MATTE_WINDOW_INVERT_INDEX] == name
    })
}

/// Append the 47 matte parameters to one kind's own control table (CC5 §2.2).
///
/// `N` is checked against the base table's length at compile time, so a kind
/// whose control table grows cannot silently drop a matte parameter.
const fn with_matte_parameters<const N: usize>(
    base: &[EffectParameterDescriptor],
) -> [EffectParameterDescriptor; N] {
    assert!(
        N == base.len() + MATTE_PARAMETER_COUNT,
        "a matte-capable descriptor is its own controls plus the 47 matte parameters"
    );
    let mut parameters = [COLOR_NODE_BYPASS_DESCRIPTOR; N];
    let mut index = 0;
    while index < base.len() {
        parameters[index] = base[index];
        index += 1;
    }
    let mut matte = 0;
    while matte < MATTE_PARAMETER_COUNT {
        parameters[base.len() + matte] = MATTE_PARAMETERS[matte];
        matte += 1;
    }
    parameters
}

/// One geometric window resolved to its stored integers (CC5 §2.2, §2.3).
///
/// Callers pass a *keyframe-evaluated* effect ([`Effect::evaluated_at`]); this
/// type performs no automation evaluation of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatteWindowParams {
    /// `1` rect, `2` ellipse — the `mask.shape_token` vocabulary, reused.
    pub shape_token: i64,
    /// Centre `x` in basis points of the frame width.
    pub center_x_bp: i64,
    /// Centre `y` in basis points of the frame height.
    pub center_y_bp: i64,
    /// Half width in basis points of the frame width.
    pub half_width_bp: i64,
    /// Half height in basis points of the frame height.
    pub half_height_bp: i64,
    /// Rotation in hundredths of a degree, clockwise as the viewer sees it.
    pub rotation_cd: i64,
    /// Symmetric feather band in basis points of the normalized field.
    pub feather_bp: i64,
    /// `1` complements this window only, before the combine.
    pub invert: i64,
}

impl MatteWindowParams {
    /// The window every matte-capable node resolves when nothing is stored: a
    /// centred rect covering the middle half of the frame, unfeathered.
    pub const NEUTRAL: Self = Self {
        shape_token: 1,
        center_x_bp: 5_000,
        center_y_bp: 5_000,
        half_width_bp: 2_500,
        half_height_bp: 2_500,
        rotation_cd: 0,
        feather_bp: 0,
        invert: 0,
    };

    fn resolve(effect: &Effect, window: usize) -> Self {
        let Some(table) = matte_window_parameters(window) else {
            return Self::NEUTRAL;
        };
        let stored = |index: usize| {
            let descriptor = table[index];
            stored_integer(effect, descriptor.name, descriptor.neutral)
                .clamp(descriptor.min, descriptor.max)
        };
        Self {
            shape_token: stored(MATTE_WINDOW_SHAPE_INDEX),
            center_x_bp: stored(MATTE_WINDOW_CENTER_X_INDEX),
            center_y_bp: stored(MATTE_WINDOW_CENTER_Y_INDEX),
            half_width_bp: stored(MATTE_WINDOW_HALF_WIDTH_INDEX),
            half_height_bp: stored(MATTE_WINDOW_HALF_HEIGHT_INDEX),
            rotation_cd: stored(MATTE_WINDOW_ROTATION_INDEX),
            feather_bp: stored(MATTE_WINDOW_FEATHER_INDEX),
            invert: stored(MATTE_WINDOW_INVERT_INDEX),
        }
    }

    /// Whether the stored shape token selects the ellipse (CC5 §2.3).
    #[must_use]
    pub const fn is_ellipse(self) -> bool {
        self.shape_token == 2
    }

    /// Whether this window's own coverage is complemented before the combine.
    #[must_use]
    pub const fn is_inverted(self) -> bool {
        self.invert >= 1
    }
}

/// The HSL qualifier resolved to its stored integers (CC5 §2.2, §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatteQualifierParams {
    /// `matte_qualifier_enabled`, `0` or `1`.
    pub enabled: i64,
    /// Hue band centre in hundredths of a degree, `0..=35999`.
    pub hue_center_cd: i64,
    /// Hue half-width. [`MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES`] disables the
    /// hue leg entirely, achromatic pixels included (CC5 §2.4).
    pub hue_width_cd: i64,
    /// Hue shoulder width beyond the half-width.
    pub hue_softness_cd: i64,
    /// Saturation band low edge in basis points.
    pub sat_low_bp: i64,
    /// Saturation band high edge in basis points.
    pub sat_high_bp: i64,
    /// Saturation band shoulder width in basis points.
    pub sat_softness_bp: i64,
    /// Luma band low edge in basis points.
    pub luma_low_bp: i64,
    /// Luma band high edge in basis points.
    pub luma_high_bp: i64,
    /// Luma band shoulder width in basis points.
    pub luma_softness_bp: i64,
}

impl MatteQualifierParams {
    /// The all-neutral qualifier: disabled, hue leg off, both bands full.
    pub const NEUTRAL: Self = Self {
        enabled: 0,
        hue_center_cd: 0,
        hue_width_cd: MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES,
        hue_softness_cd: 0,
        sat_low_bp: 0,
        sat_high_bp: 10_000,
        sat_softness_bp: 0,
        luma_low_bp: 0,
        luma_high_bp: 10_000,
        luma_softness_bp: 0,
    };

    /// Whether the qualifier leg participates in the coverage.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        self.enabled >= 1
    }

    /// Whether the hue leg is disabled by a 180° half-width (CC5 §2.4).
    #[must_use]
    pub const fn hue_leg_disabled(self) -> bool {
        self.hue_width_cd >= MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES
    }

    /// Whether the saturation band's low edge is above its high edge.
    #[must_use]
    pub const fn saturation_band_inverted(self) -> bool {
        self.sat_low_bp > self.sat_high_bp
    }

    /// Whether the luma band's low edge is above its high edge.
    #[must_use]
    pub const fn luma_band_inverted(self) -> bool {
        self.luma_low_bp > self.luma_high_bp
    }
}

/// Every matte control of one node resolved to its stored integer (CC5 §2.2).
///
/// Callers pass a *keyframe-evaluated* effect ([`Effect::evaluated_at`]); this
/// type performs no automation evaluation of its own. Omitted parameters
/// resolve to their descriptor neutrals and out-of-range values are clamped
/// defensively, so a rendered frame can never fail — neither case is reachable
/// through the edit path, which rejects both atomically.
///
/// Every §2.6 question is answered on these integers, never on floats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatteParams {
    /// `matte_enabled`, the master switch. `0` makes the node byte-identical
    /// to its CC4 self.
    pub enabled: i64,
    /// Active windows, `0..=4`. Windows at index `>= window_count` are stored
    /// but never rendered.
    pub window_count: usize,
    /// `0` union, `1` intersection.
    pub combine_token: i64,
    /// `1` complements the combined coverage before the mix.
    pub invert: i64,
    /// `matte_mix_basis_points`, `0..=10000`: scales the final coverage.
    pub mix_bp: i64,
    /// The HSL qualifier leg.
    pub qualifier: MatteQualifierParams,
    /// All four windows, resolved whether or not they are active.
    pub windows: [MatteWindowParams; MATTE_WINDOW_LIMIT],
}

impl MatteParams {
    /// The matte a node carries when it stores no `matte_*` parameter: an
    /// inactive matte, which is what makes a pre-CC5 project render
    /// bit-identically (CC5 §8).
    pub const NEUTRAL: Self = Self {
        enabled: 0,
        window_count: 0,
        combine_token: 0,
        invert: 0,
        mix_bp: MATTE_MIX_BASIS_POINTS_MAX,
        qualifier: MatteQualifierParams::NEUTRAL,
        windows: [MatteWindowParams::NEUTRAL; MATTE_WINDOW_LIMIT],
    };

    /// Resolve a keyframe-evaluated matte-capable node.
    ///
    /// An effect whose kind carries no matte — `technical_lut`, or any effect
    /// outside the managed stack — resolves to [`Self::NEUTRAL`], so a
    /// hand-edited file cannot make a technical input transform partial.
    #[must_use]
    pub fn from_effect(effect: &Effect) -> Self {
        if !is_matte_capable_color_node(&effect.name) {
            return Self::NEUTRAL;
        }
        let stored = |index: usize| {
            let descriptor = MATTE_CONTROL_PARAMETERS[index];
            stored_integer(effect, descriptor.name, descriptor.neutral)
                .clamp(descriptor.min, descriptor.max)
        };
        let window_count = usize::try_from(stored(MATTE_WINDOW_COUNT_INDEX)).unwrap_or(0);
        Self {
            enabled: stored(MATTE_ENABLED_INDEX),
            window_count: window_count.min(MATTE_WINDOW_LIMIT),
            combine_token: stored(MATTE_COMBINE_TOKEN_INDEX),
            invert: stored(MATTE_INVERT_INDEX),
            mix_bp: stored(MATTE_MIX_INDEX),
            qualifier: MatteQualifierParams {
                enabled: stored(MATTE_QUALIFIER_ENABLED_INDEX),
                hue_center_cd: stored(MATTE_HUE_CENTER_INDEX),
                hue_width_cd: stored(MATTE_HUE_WIDTH_INDEX),
                hue_softness_cd: stored(MATTE_HUE_SOFTNESS_INDEX),
                sat_low_bp: stored(MATTE_SATURATION_LOW_INDEX),
                sat_high_bp: stored(MATTE_SATURATION_HIGH_INDEX),
                sat_softness_bp: stored(MATTE_SATURATION_SOFTNESS_INDEX),
                luma_low_bp: stored(MATTE_LUMA_LOW_INDEX),
                luma_high_bp: stored(MATTE_LUMA_HIGH_INDEX),
                luma_softness_bp: stored(MATTE_LUMA_SOFTNESS_INDEX),
            },
            windows: [
                MatteWindowParams::resolve(effect, 0),
                MatteWindowParams::resolve(effect, 1),
                MatteWindowParams::resolve(effect, 2),
                MatteWindowParams::resolve(effect, 3),
            ],
        }
    }

    /// Whether the master switch is on.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled >= 1
    }

    /// Whether the combined coverage is complemented before the mix.
    #[must_use]
    pub const fn is_inverted(&self) -> bool {
        self.invert >= 1
    }

    /// Whether the windows combine by intersection rather than union.
    #[must_use]
    pub const fn intersects(&self) -> bool {
        self.combine_token >= 1
    }

    /// The active windows, in index order (CC5 §2.3).
    pub fn active_windows(&self) -> std::slice::Iter<'_, MatteWindowParams> {
        self.windows[..self.window_count].iter()
    }

    /// One stored window, active or not, or `None` past window 3.
    #[must_use]
    pub const fn window(&self, index: usize) -> Option<&MatteWindowParams> {
        if index >= MATTE_WINDOW_LIMIT {
            return None;
        }
        Some(&self.windows[index])
    }

    /// Whether the matte is inactive, so no matte block is written and the
    /// node is byte-identical to its CC4 self (CC5 §2.6, rule 1).
    ///
    /// True when the master switch is off, or when the matte is enabled but
    /// selects everything at full strength: no window, no qualifier, no
    /// invert, and full mix.
    #[must_use]
    pub const fn is_inactive_matte(&self) -> bool {
        if !self.is_enabled() {
            return true;
        }
        self.window_count == 0
            && !self.qualifier.is_enabled()
            && self.invert == 0
            && self.mix_bp == MATTE_MIX_BASIS_POINTS_MAX
    }

    /// Whether the node carries a matte the renderer must evaluate.
    #[must_use]
    pub const fn has_matte(&self) -> bool {
        !self.is_inactive_matte()
    }

    /// Whether the matte makes the whole node the exact identity (CC5 §2.6,
    /// rule 2), reported as [`ColorNodeInactiveReason::MatteExcluded`].
    ///
    /// Either the mix is zero, or an empty matte is inverted, which is `m = 0`
    /// at every pixel.
    #[must_use]
    pub const fn node_excluded_by_matte(&self) -> bool {
        if !self.is_enabled() {
            return false;
        }
        self.mix_bp == 0
            || (self.window_count == 0 && !self.qualifier.is_enabled() && self.invert >= 1)
    }

    /// The qualifier bands whose low edge resolved above their high edge
    /// (CC5 §2.6).
    ///
    /// Such a band evaluates to `0` — no clamping, no reordering, no error —
    /// and QA reports it as `matte_band_inverted_by_automation`. The test is
    /// arithmetic: whether the qualifier is enabled at all is the reporter's
    /// question, not this accessor's.
    #[must_use]
    pub fn degenerate_bands(&self) -> Vec<&'static str> {
        let mut bands = Vec::new();
        if self.qualifier.saturation_band_inverted() {
            bands.push(MATTE_SATURATION_BAND);
        }
        if self.qualifier.luma_band_inverted() {
            bands.push(MATTE_LUMA_BAND);
        }
        bands
    }

    /// The evaluated mix as a coverage scale in `0.0..=1.0`.
    ///
    /// `0..=10000` is exactly representable in `f32`, so the conversion is
    /// exact and the endpoints are precisely `0.0` and `1.0`.
    #[must_use]
    pub fn mix(&self) -> f32 {
        let clamped = self.mix_bp.clamp(0, MATTE_MIX_BASIS_POINTS_MAX);
        f32::from(u16::try_from(clamped).unwrap_or_default()) / 10_000.0
    }
}

/// Parameters in the `primary_correction` descriptor: ten CC1 controls plus
/// the 47 CC5 matte parameters (CC5 §2.2).
pub const PRIMARY_CORRECTION_DESCRIPTOR_PARAMETER_COUNT: usize = 10 + MATTE_PARAMETER_COUNT;
/// Parameters in the `color_wheels` descriptor: twelve CC3 controls, `bypass`,
/// and the 47 CC5 matte parameters (CC5 §2.2).
pub const COLOR_WHEELS_DESCRIPTOR_PARAMETER_COUNT: usize = 13 + MATTE_PARAMETER_COUNT;
/// Parameters in the `color_curves` descriptor: the 133 CC3 curve parameters
/// plus the 47 CC5 matte parameters (CC5 §2.2).
pub const COLOR_CURVES_DESCRIPTOR_PARAMETER_COUNT: usize =
    COLOR_CURVES_PARAMETER_COUNT + MATTE_PARAMETER_COUNT;
/// Parameters in the `creative_look` descriptor: four CC4 controls plus the 47
/// CC5 matte parameters (CC5 §2.2). `technical_lut` keeps its four: a
/// partially applied source normalization is not a meaningful state.
pub const CREATIVE_LOOK_DESCRIPTOR_PARAMETER_COUNT: usize = 4 + MATTE_PARAMETER_COUNT;

const PRIMARY_CORRECTION_DESCRIPTOR_PARAMETERS: [EffectParameterDescriptor;
    PRIMARY_CORRECTION_DESCRIPTOR_PARAMETER_COUNT] =
    with_matte_parameters(&PRIMARY_CORRECTION_PARAMETERS);
const COLOR_WHEELS_DESCRIPTOR_PARAMETERS: [EffectParameterDescriptor;
    COLOR_WHEELS_DESCRIPTOR_PARAMETER_COUNT] = with_matte_parameters(&COLOR_WHEELS_PARAMETERS);
const COLOR_CURVES_DESCRIPTOR_PARAMETERS: [EffectParameterDescriptor;
    COLOR_CURVES_DESCRIPTOR_PARAMETER_COUNT] = with_matte_parameters(&COLOR_CURVES_PARAMETERS);
const CREATIVE_LOOK_DESCRIPTOR_PARAMETERS: [EffectParameterDescriptor;
    CREATIVE_LOOK_DESCRIPTOR_PARAMETER_COUNT] = with_matte_parameters(&CREATIVE_LOOK_PARAMETERS);

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
        parameters: &PRIMARY_CORRECTION_DESCRIPTOR_PARAMETERS,
    },
    EffectDescriptor {
        name: "color_wheels",
        parameters: &COLOR_WHEELS_DESCRIPTOR_PARAMETERS,
    },
    EffectDescriptor {
        name: "color_curves",
        parameters: &COLOR_CURVES_DESCRIPTOR_PARAMETERS,
    },
    EffectDescriptor {
        name: "technical_lut",
        parameters: &TECHNICAL_LUT_PARAMETERS,
    },
    EffectDescriptor {
        name: "creative_look",
        parameters: &CREATIVE_LOOK_DESCRIPTOR_PARAMETERS,
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

/// The managed colour node kinds, in CC4 §3.2 stage order.
///
/// These five effect names form one ordered node stack executed in
/// `clip.effects` vector order. Within a stage there is no fixed inter-kind
/// precedence (CC3 §3.1); *across* stages the vector order must be
/// non-decreasing in stage rank, which [`color_stage_order_violation`]
/// enforces.
///
/// CC4 grew this list from the three CC3 correction kinds to five by adding
/// `technical_lut` at the front and `creative_look` at the back, so the array
/// order is the stage order rather than the historical CC3 order.
pub const MANAGED_COLOR_NODE_NAMES: [&str; 5] = [
    "technical_lut",
    "primary_correction",
    "color_wheels",
    "color_curves",
    "creative_look",
];

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
    /// The CC4 technical input transform: a LUT that normalizes the source.
    TechnicalLut,
    /// The CC1 managed SDR primary correction.
    Primary,
    /// The CC3 ASC CDL slope/offset/power wheels.
    Wheels,
    /// The CC3 master/red/green/blue curves.
    Curves,
    /// The CC4 creative look: a LUT applied after every correction.
    CreativeLook,
}

impl ColorNodeKind {
    /// Every node kind, in [`MANAGED_COLOR_NODE_NAMES`] (stage) order.
    pub const ALL: [Self; 5] = [
        Self::TechnicalLut,
        Self::Primary,
        Self::Wheels,
        Self::Curves,
        Self::CreativeLook,
    ];

    /// The canonical effect name for this node kind.
    #[must_use]
    pub const fn effect_name(self) -> &'static str {
        match self {
            Self::TechnicalLut => "technical_lut",
            Self::Primary => "primary_correction",
            Self::Wheels => "color_wheels",
            Self::Curves => "color_curves",
            Self::CreativeLook => "creative_look",
        }
    }

    /// Classify a registered effect name.
    #[must_use]
    pub fn from_effect_name(name: &str) -> Option<Self> {
        match name {
            "technical_lut" => Some(Self::TechnicalLut),
            "primary_correction" => Some(Self::Primary),
            "color_wheels" => Some(Self::Wheels),
            "color_curves" => Some(Self::Curves),
            "creative_look" => Some(Self::CreativeLook),
            _ => None,
        }
    }

    /// The node-kind tag written into the ordered grade storage buffer
    /// (CC3 §3.2, CC4 §4.2): `1` primary, `2` wheels, `3` curves,
    /// `4` technical LUT, `5` creative look.
    #[must_use]
    pub const fn storage_buffer_tag(self) -> u32 {
        match self {
            Self::Primary => 1,
            Self::Wheels => 2,
            Self::Curves => 3,
            Self::TechnicalLut => 4,
            Self::CreativeLook => 5,
        }
    }

    /// The ordering stage this kind occupies (CC4 §3.1, §3.2).
    #[must_use]
    pub const fn stage(self) -> ColorStage {
        match self {
            Self::TechnicalLut => ColorStage::Input,
            Self::Primary | Self::Wheels | Self::Curves => ColorStage::Correction,
            Self::CreativeLook => ColorStage::Look,
        }
    }

    /// The stable manifest/inspector role token for this kind (CC4 §3.1):
    /// `technical`, `correction`, or `creative`.
    #[must_use]
    pub const fn role(self) -> &'static str {
        match self {
            Self::TechnicalLut => "technical",
            Self::Primary | Self::Wheels | Self::Curves => "correction",
            Self::CreativeLook => "creative",
        }
    }

    /// Whether this kind evaluates a project LUT asset.
    #[must_use]
    pub const fn is_lut(self) -> bool {
        matches!(self, Self::TechnicalLut | Self::CreativeLook)
    }

    /// Whether this kind may carry a CC5 matte (CC5 §2.1).
    ///
    /// Every kind but `technical_lut`: a technical input transform normalizes
    /// the *whole* source, and a partially applied source normalization is not
    /// a meaningful state — the same argument that pins its
    /// `mix_basis_points`. Its descriptor therefore carries no `matte_*`
    /// parameter at all, so naming one is the ordinary
    /// [`crate::OpError::UnknownEffectParam`] rejection.
    #[must_use]
    pub const fn supports_matte(self) -> bool {
        !matches!(self, Self::TechnicalLut)
    }

    /// The descriptor parameter table for a LUT node kind (CC4 §5).
    const fn lut_parameters(self) -> Option<&'static [EffectParameterDescriptor; 4]> {
        match self {
            Self::TechnicalLut => Some(&TECHNICAL_LUT_PARAMETERS),
            Self::CreativeLook => Some(&CREATIVE_LOOK_PARAMETERS),
            Self::Primary | Self::Wheels | Self::Curves => None,
        }
    }
}

/// Where one managed colour node sits in the CC4 §3.2 stage order.
///
/// The subsequence of managed nodes in `clip.effects` must have non-decreasing
/// stage rank. A vector order that contradicts it is *rejected*, never
/// silently reordered, so the stored order stays the execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ColorStage {
    /// Input transforms: `technical_lut`.
    Input = 0,
    /// Corrections: `primary_correction`, `color_wheels`, `color_curves`.
    Correction = 1,
    /// Creative looks: `creative_look`.
    Look = 2,
}

impl ColorStage {
    /// Every stage, in ascending rank order.
    pub const ALL: [Self; 3] = [Self::Input, Self::Correction, Self::Look];

    /// The stage rank compared by the CC4 §3.2 ordering rule.
    #[must_use]
    pub const fn rank(self) -> u8 {
        self as u8
    }

    /// Stable manifest/inspector token: `input`, `correction`, or `look`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Correction => "correction",
            Self::Look => "look",
        }
    }
}

/// Whether an effect name is one of the two CC4 LUT node kinds.
#[must_use]
pub fn is_lut_color_node(name: &str) -> bool {
    ColorNodeKind::from_effect_name(name).is_some_and(ColorNodeKind::is_lut)
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
    /// structurally identity, or a LUT node's evaluated mix is zero.
    Neutral,
    /// A LUT node's evaluated `lut_asset_id` is `0` (CC4 §3.6).
    ///
    /// Unreachable in a valid document: `validate_document` requires every
    /// referenced id to exist. The state exists so a resolved node can never
    /// index a missing asset.
    Unbound,
    /// An enabled matte resolves to `m = 0` at every pixel (CC5 §2.6): its
    /// `matte_mix_basis_points` is `0`, or an empty matte is inverted.
    ///
    /// The node is the exact identity, so it is not written to the grade
    /// buffer and is skipped by the CPU reference: a zero-mix matte is
    /// losslessly identical to removing the node, bit for bit.
    MatteExcluded,
}

impl ColorNodeInactiveReason {
    /// Stable manifest/inspector token for the reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bypassed => "bypassed",
            Self::Neutral => "neutral",
            Self::Unbound => "unbound",
            Self::MatteExcluded => "matte_excluded",
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
/// neutral short-circuit, so it is reported active unless CC5 §2.6's matte
/// rule excludes it, and its CC1 rendering is unchanged.
#[must_use]
pub fn color_node_inactive_reason(effect: &Effect) -> Option<ColorNodeInactiveReason> {
    let kind = classify_color_node(effect)?;
    let reason = match kind {
        ColorNodeKind::Primary => None,
        ColorNodeKind::Wheels => ColorWheelsParams::from_effect(effect).inactive_reason(),
        ColorNodeKind::Curves => ResolvedCurves::from_effect(effect).inactive_reason(),
        ColorNodeKind::TechnicalLut | ColorNodeKind::CreativeLook => {
            LutNodeParams::from_effect(effect).inactive_reason()
        }
    };
    if reason.is_some() {
        return reason;
    }
    // CC5 §2.6 rule 2, tested last so a bypassed, neutral, or unbound node
    // keeps reporting the reason it already had: a matte cannot make an
    // already-identity node report a different cause.
    if kind.supports_matte() && MatteParams::from_effect(effect).node_excluded_by_matte() {
        return Some(ColorNodeInactiveReason::MatteExcluded);
    }
    None
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

/// How many LUT nodes one effect stack carries (CC4 §3.1).
///
/// Counts `technical_lut` and `creative_look` together, active or not: a
/// bypassed LUT node keeps its place in `clip.effects` and still counts
/// against [`LUT_NODE_LIMIT_PER_LAYER`].
#[must_use]
pub fn lut_node_count(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| is_lut_color_node(&effect.name))
        .count()
}

/// Every `technical_lut` / `creative_look` control resolved to its stored
/// integer (CC4 §5).
///
/// Callers pass a *keyframe-evaluated* effect ([`Effect::evaluated_at`]); this
/// type performs no automation evaluation of its own. Omitted parameters
/// resolve to their descriptor neutrals and out-of-range values are clamped
/// defensively, so a rendered frame can never fail — neither case is reachable
/// through the edit path, which rejects both atomically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LutNodeParams {
    /// The referenced project asset. [`LutAssetId`]`(0)` is *unbound*, which
    /// makes the node inactive.
    pub lut_asset_id: LutAssetId,
    /// Look strength in basis points, `0..=10000`. Always `10000` on a
    /// `technical_lut`, whose descriptor pins it.
    pub mix_basis_points: i64,
    /// `0` display709, `1` linear, `2` grade709 (CC4 §3.4).
    pub input_encoding_token: i64,
    /// The stored `bypass` token, `0` or `1`.
    pub bypass_token: i64,
}

impl LutNodeParams {
    /// Resolve a keyframe-evaluated LUT node.
    ///
    /// The bounds come from the node's own descriptor table, so a
    /// `technical_lut` always resolves full strength while a `creative_look`
    /// keeps its authored mix. An effect that is not a LUT node resolves
    /// against the `creative_look` table and is reported unbound.
    #[must_use]
    pub fn from_effect(effect: &Effect) -> Self {
        let parameters = ColorNodeKind::from_effect_name(&effect.name)
            .and_then(ColorNodeKind::lut_parameters)
            .unwrap_or(&CREATIVE_LOOK_PARAMETERS);
        let resolve = |index: usize| {
            let descriptor = parameters[index];
            stored_integer(effect, descriptor.name, descriptor.neutral)
                .clamp(descriptor.min, descriptor.max)
        };
        let asset_id = resolve(LUT_ASSET_ID_INDEX);
        Self {
            lut_asset_id: LutAssetId(u64::try_from(asset_id).unwrap_or_default()),
            mix_basis_points: resolve(LUT_MIX_INDEX),
            input_encoding_token: resolve(LUT_INPUT_ENCODING_INDEX),
            bypass_token: resolve(LUT_BYPASS_INDEX),
        }
    }

    /// Whether the evaluated `bypass` control is `>= 1`.
    #[must_use]
    pub const fn bypass(&self) -> bool {
        self.bypass_token >= 1
    }

    /// Whether the node references no asset (CC4 §3.3).
    #[must_use]
    pub const fn is_unbound(&self) -> bool {
        self.lut_asset_id.0 == 0
    }

    /// Why this node is the identity for the evaluated frame, if it is
    /// (CC4 §3.6).
    ///
    /// Tested on the stored integers, never on floats, so bypass, `mix = 0`,
    /// and an unbound reference are losslessly identical to removing the node.
    #[must_use]
    pub const fn inactive_reason(&self) -> Option<ColorNodeInactiveReason> {
        if self.bypass() {
            Some(ColorNodeInactiveReason::Bypassed)
        } else if self.mix_basis_points == 0 {
            Some(ColorNodeInactiveReason::Neutral)
        } else if self.is_unbound() {
            Some(ColorNodeInactiveReason::Unbound)
        } else {
            None
        }
    }

    /// Whether the renderer must evaluate this node.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.inactive_reason().is_none()
    }

    /// The evaluated mix as a linear-light blend factor in `0.0..=1.0`.
    ///
    /// `0..=10000` is exactly representable in `f32`, so the conversion is
    /// exact and the endpoints are precisely `0.0` and `1.0`.
    #[must_use]
    pub fn mix(&self) -> f32 {
        let clamped = self.mix_basis_points.clamp(0, LUT_MIX_BASIS_POINTS_MAX);
        f32::from(u16::try_from(clamped).unwrap_or_default()) / 10_000.0
    }
}

/// Whether a LUT node could be evaluated on *any* frame of its clip
/// (CC4 §3.6, §6).
///
/// [`LutNodeParams::is_active`] answers the question for one already-resolved
/// frame. Export preflight and availability reporting ask a different
/// question: a node whose `bypass` or `mix_basis_points` is keyframed is the
/// identity on some frames and a real look on others, so an asset it
/// references still has to be there.
///
/// The answer is deliberately conservative — it over-approximates activity by
/// testing each control's candidate values independently, including the static
/// fallback even when the control is automated. A node is reported inactive
/// only when *no* stored value of `bypass`, `mix_basis_points`, or
/// `lut_asset_id` could make it evaluate, which is exactly the case where
/// removing the node is provably lossless.
#[must_use]
pub fn lut_node_may_be_active(effect: &Effect) -> bool {
    let Some(parameters) =
        ColorNodeKind::from_effect_name(&effect.name).and_then(ColorNodeKind::lut_parameters)
    else {
        return false;
    };
    let candidates = |index: usize| {
        let descriptor = parameters[index];
        let mut values = vec![stored_integer(effect, descriptor.name, descriptor.neutral)];
        if let Some(curve) = effect.keyframes.get(descriptor.name) {
            values.extend(curve.keyframes.iter().map(|keyframe| keyframe.value));
        }
        values
            .into_iter()
            .map(|value| value.clamp(descriptor.min, descriptor.max))
            .collect::<Vec<_>>()
    };
    candidates(LUT_BYPASS_INDEX)
        .iter()
        .any(|bypass| *bypass < 1)
        && candidates(LUT_MIX_INDEX).iter().any(|mix| *mix != 0)
        && candidates(LUT_ASSET_ID_INDEX)
            .iter()
            .any(|lut_asset_id| *lut_asset_id != 0)
}

/// The first place a managed colour node appears before an earlier stage
/// (CC4 §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorStageOrderViolation {
    /// Position of the offending node in the caller's effect slice.
    pub index: usize,
    /// The offending node.
    pub effect: EffectId,
    /// The offending node's kind.
    pub kind: ColorNodeKind,
    /// The offending node's stage.
    pub color_stage: ColorStage,
    /// Position of the managed node immediately before it.
    pub previous_index: usize,
    /// The managed node immediately before it.
    pub previous_effect: EffectId,
    /// That node's kind.
    pub previous_kind: ColorNodeKind,
    /// That node's stage, whose rank is greater than `color_stage`'s.
    pub previous_color_stage: ColorStage,
}

/// The first CC4 §3.2 stage-order violation in one effect stack, if any.
///
/// Only the managed colour nodes form the constrained subsequence; crops,
/// masks, keys, reframes, and the legacy LUT stages are unconstrained and keep
/// their positions. A `None` result means the stack's managed subsequence has
/// non-decreasing stage rank, which every pre-CC4 project satisfies trivially
/// because all of its nodes are corrections.
#[must_use]
pub fn color_stage_order_violation(effects: &[Effect]) -> Option<ColorStageOrderViolation> {
    color_stage_order_violation_over(effects.iter().enumerate().filter_map(|(index, effect)| {
        classify_color_node(effect).map(|kind| (index, effect.id, kind))
    }))
}

/// The CC4 §3.2 scan over an already-classified managed-node subsequence.
///
/// The edit path uses this to test the vector an insertion or conversion
/// *would* produce without cloning any effect, so both the operation
/// precondition and the document invariant are the same rule.
pub(crate) fn color_stage_order_violation_over<I>(nodes: I) -> Option<ColorStageOrderViolation>
where
    I: IntoIterator<Item = (usize, EffectId, ColorNodeKind)>,
{
    let mut previous: Option<(usize, EffectId, ColorNodeKind)> = None;
    for (index, effect, kind) in nodes {
        if let Some((previous_index, previous_effect, previous_kind)) = previous
            && kind.stage().rank() < previous_kind.stage().rank()
        {
            return Some(ColorStageOrderViolation {
                index,
                effect,
                kind,
                color_stage: kind.stage(),
                previous_index,
                previous_effect,
                previous_kind,
                previous_color_stage: previous_kind.stage(),
            });
        }
        previous = Some((index, effect, kind));
    }
    None
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
