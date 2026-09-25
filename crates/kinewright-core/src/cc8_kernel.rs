//! CC8 S1 numeric kernel (design §§2–3, 6, 14): pure colour maths, no I/O.
//! f32 = production path for S3; [`reference`] = f64 conformance path (App. N).
//! Fail-closed: non-finite/overflow/out-of-domain refuse, never clamp (R35).

use thiserror::Error;

/// Typed S1 kernel failure (R35). S2 maps this into `OpError`/incidents.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum Cc8KernelError {
    /// NaN/infinite argument (`function`: entry point, `value`: the input).
    #[error("cc8 kernel: {function}: non-finite input {value}")]
    NonFiniteInput { function: &'static str, value: f64 },
    /// NaN/infinite result — overflow or past a pole; handle, don't clamp.
    #[error("cc8 kernel: {function}: non-finite result (overflow), refusing")]
    NonFiniteResult { function: &'static str },
    /// Finite but out-of-domain argument (`reason`: which rule was violated).
    #[error("cc8 kernel: {function}: out of domain ({reason})")]
    OutOfDomain {
        function: &'static str,
        reason: &'static str,
    },
}

/// Reject non-finite input (R35: fail closed, never clamp).
fn finite_input<T>(function: &'static str, value: T) -> Result<T, Cc8KernelError>
where
    T: Copy + Into<f64>,
{
    if value.into().is_finite() {
        Ok(value)
    } else {
        Err(Cc8KernelError::NonFiniteInput {
            function,
            value: value.into(),
        })
    }
}

/// Reject a non-finite result: overflow refuses rather than clamping (R35).
fn finite_result<T>(function: &'static str, value: T) -> Result<T, Cc8KernelError>
where
    T: Copy + Into<f64>,
{
    if value.into().is_finite() {
        Ok(value)
    } else {
        Err(Cc8KernelError::NonFiniteResult { function })
    }
}

// ========================================================================
// ST 2084 (PQ); constants are exact binary fractions in both precisions.

const PQ_M1_F32: f32 = 2610.0 / 16384.0;
const PQ_M2_F32: f32 = (2523.0 / 4096.0) * 128.0;
const PQ_C1_F32: f32 = 3424.0 / 4096.0;
const PQ_C2_F32: f32 = (2413.0 / 4096.0) * 32.0;
const PQ_C3_F32: f32 = (2392.0 / 4096.0) * 32.0;
const PQ_M1_F64: f64 = 2610.0 / 16384.0;
const PQ_M2_F64: f64 = (2523.0 / 4096.0) * 128.0;
const PQ_C1_F64: f64 = 3424.0 / 4096.0;
const PQ_C2_F64: f64 = (2413.0 / 4096.0) * 32.0;
const PQ_C3_F64: f64 = (2392.0 / 4096.0) * 32.0;

/// ST 2084 peak, cd/m².
const PQ_PEAK_NITS_F32: f32 = 10_000.0;
const PQ_PEAK_NITS_F64: f64 = 10_000.0;

/// ST 2084 inverse EOTF ("OETF" in §6 vocabulary): absolute nits → PQ signal.
///
/// Domain L ≥ 0 only: negatives are never fed to PQ (EETF passes them through
/// in nits, §6 P3), so they refuse here rather than taking a sign extension.
///
/// # Errors
/// Non-finite → `NonFiniteInput`; negative → `OutOfDomain`; overflow → `NonFiniteResult`.
pub fn pq_oetf(nits: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "pq_oetf";
    let nits = finite_input(FUNCTION, nits)?;
    if nits < 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "PQ input must be >= 0 nits",
        });
    }
    finite_result(FUNCTION, pq_oetf_unchecked(nits))
}

fn pq_oetf_unchecked(nits: f32) -> f32 {
    let y = (nits / PQ_PEAK_NITS_F32).powf(PQ_M1_F32);
    ((PQ_C1_F32 + PQ_C2_F32 * y) / (1.0 + PQ_C3_F32 * y)).powf(PQ_M2_F32)
}

/// ST 2084 EOTF: PQ signal → absolute nits. Exact 0 → 0.
///
/// # Errors
/// Non-finite → `NonFiniteInput`; negative → `OutOfDomain`; at/past pole → `NonFiniteResult`.
pub fn pq_eotf(signal: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "pq_eotf";
    let signal = finite_input(FUNCTION, signal)?;
    if signal < 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "PQ input must be >= 0",
        });
    }
    match pq_eotf_unchecked(signal) {
        Some(nits) => finite_result(FUNCTION, nits),
        None => Err(Cc8KernelError::NonFiniteResult { function: FUNCTION }),
    }
}

/// Raw ST 2084 EOTF; `None` at/past the pole, where the standard's rational
/// form is singular and any finite answer would be a fabrication.
fn pq_eotf_unchecked(signal: f32) -> Option<f32> {
    let p = signal.powf(1.0 / PQ_M2_F32);
    let denominator = PQ_C2_F32 - PQ_C3_F32 * p;
    if denominator <= 0.0 {
        return None;
    }
    Some(PQ_PEAK_NITS_F32 * ((p - PQ_C1_F32).max(0.0) / denominator).powf(1.0 / PQ_M1_F32))
}

// ========================================================================
// ARIB STD-B67 (HLG); a/b/c are the standard's rounded decimals.

const HLG_A_F32: f32 = 0.178_832_77;
const HLG_B_F32: f32 = 0.284_668_92;
const HLG_C_F32: f32 = 0.559_910_7;
const HLG_A_F64: f64 = 0.178_832_77;
const HLG_B_F64: f64 = 0.284_668_92;
const HLG_C_F64: f64 = 0.559_910_73;

/// HLG scene breakpoint 1/12; its image is the signal breakpoint 0.5.
const HLG_SCENE_BREAKPOINT_F32: f32 = 1.0 / 12.0;
const HLG_SIGNAL_BREAKPOINT_F32: f32 = 0.5;
const HLG_SCENE_BREAKPOINT_F64: f64 = 1.0 / 12.0;
const HLG_SIGNAL_BREAKPOINT_F64: f64 = 0.5;

/// ARIB STD-B67 OETF: scene-linear → HLG signal, sign-preserving so negative
/// working values keep their sign (negatives are never fed to powers, §2 P3).
///
/// # Errors
/// Non-finite → `NonFiniteInput`; overflow → `NonFiniteResult`.
pub fn hlg_oetf(scene: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "hlg_oetf";
    let scene = finite_input(FUNCTION, scene)?;
    finite_result(FUNCTION, hlg_oetf_unchecked(scene))
}

fn hlg_oetf_unchecked(scene: f32) -> f32 {
    let magnitude = scene.abs();
    let encoded = if magnitude <= HLG_SCENE_BREAKPOINT_F32 {
        (3.0 * magnitude).sqrt()
    } else {
        HLG_A_F32 * (12.0 * magnitude - HLG_B_F32).ln() + HLG_C_F32
    };
    encoded.copysign(scene)
}

/// ARIB STD-B67 inverse OETF: HLG signal → scene-linear (the whole of HLG
/// input decoding — no OOTF at input, §2 R28), sign-preserving.
///
/// # Errors
/// Non-finite → `NonFiniteInput`; overflow → `NonFiniteResult`.
pub fn hlg_inverse_oetf(signal: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "hlg_inverse_oetf";
    let signal = finite_input(FUNCTION, signal)?;
    finite_result(FUNCTION, hlg_inverse_oetf_unchecked(signal))
}

fn hlg_inverse_oetf_unchecked(signal: f32) -> f32 {
    let magnitude = signal.abs();
    let decoded = if magnitude <= HLG_SIGNAL_BREAKPOINT_F32 {
        magnitude * magnitude / 3.0
    } else {
        (((magnitude - HLG_C_F32) / HLG_A_F32).exp() + HLG_B_F32) / 12.0
    };
    decoded.copysign(signal)
}

// ========================================================================
// HLG system gamma rule v1 (§3).

/// HLG system gamma at peak `peak_nits` (BT.2100-3 Table 5 Note 5f), rule v1:
/// P ≤ 2000 takes the log rule, P > 2000 the power rule — the log rule owns
/// the boundary at exactly 2000.
///
/// # Errors
/// Non-finite → `NonFiniteInput`; peak ≤ 0 → `OutOfDomain`.
pub fn hlg_gamma(peak_nits: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "hlg_gamma";
    let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
    finite_result(FUNCTION, hlg_gamma_unchecked(peak))
}

fn hlg_gamma_unchecked(peak_nits: f32) -> f32 {
    if peak_nits <= 2000.0 {
        1.2 + 0.42 * (peak_nits / 1000.0).log10()
    } else {
        1.2 * 1.111_f32.powf((peak_nits / 1000.0).log2())
    }
}

// ========================================================================
// Scene normalization + rendering (§2); α = 1, β = 0; gamma arrives resolved.

const BT2020_KR_F32: f32 = 0.2627;
const BT2020_KG_F32: f32 = 0.6780;
const BT2020_KB_F32: f32 = 0.0593;
const BT2020_KR_F64: f64 = 0.2627;
const BT2020_KG_F64: f64 = 0.6780;
const BT2020_KB_F64: f64 = 0.0593;

/// Triplet [`finite_input`]: any non-finite component refuses (R35).
fn finite_input_3<T>(function: &'static str, value: [T; 3]) -> Result<[T; 3], Cc8KernelError>
where
    T: Copy + Into<f64>,
{
    for component in value {
        if !component.into().is_finite() {
            return Err(Cc8KernelError::NonFiniteInput {
                function,
                value: component.into(),
            });
        }
    }
    Ok(value)
}

/// Triplet [`finite_result`]: overflow refuses rather than clamping (R35).
fn finite_result_3<T>(function: &'static str, value: [T; 3]) -> Result<[T; 3], Cc8KernelError>
where
    T: Copy + Into<f64>,
{
    for component in value {
        if !component.into().is_finite() {
            return Err(Cc8KernelError::NonFiniteResult { function });
        }
    }
    Ok(value)
}

/// Require a finite positive scale (white/peak/Ct/γ); else refuse (R35).
fn positive<T>(function: &'static str, reason: &'static str, value: T) -> Result<T, Cc8KernelError>
where
    T: Copy + Into<f64>,
{
    let v = value.into();
    if !v.is_finite() {
        return Err(Cc8KernelError::NonFiniteInput { function, value: v });
    }
    if v <= 0.0 {
        return Err(Cc8KernelError::OutOfDomain { function, reason });
    }
    Ok(value)
}

/// Reference-scene white `s_white = (W/P)^(1/γ)` (§2). Working = `scene/s_white`.
///
/// # Errors
/// Non-finite args → `NonFiniteInput`; W/P/γ ≤ 0 → `OutOfDomain`.
pub fn s_white(white_nits: f32, peak_nits: f32, gamma: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "s_white";
    let white = positive(FUNCTION, "white must be > 0", white_nits)?;
    let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
    let gamma = positive(FUNCTION, "gamma must be > 0", gamma)?;
    finite_result(FUNCTION, (white / peak).powf(1.0 / gamma))
}

/// Normalize scene to working: `w = s/s_white` per component (§2).
///
/// # Errors
/// Non-finite args → `NonFiniteInput`; `s_white` ≤ 0 → `OutOfDomain`.
pub fn scene_to_working(scene: [f32; 3], ref_white: f32) -> Result<[f32; 3], Cc8KernelError> {
    const FUNCTION: &str = "scene_to_working";
    let scene = finite_input_3(FUNCTION, scene)?;
    let sw = positive(FUNCTION, "s_white must be > 0", ref_white)?;
    finite_result_3(FUNCTION, [scene[0] / sw, scene[1] / sw, scene[2] / sw])
}

/// Forward rendering, scene → display nits (§2): `D_c = P·Y_s^(γ−1)·s_c`
/// with 2020 luma of the nonnegative part, plus P3 signed handling
/// `D = F(max(s,0)) + P·min(s,0)`. `Y_s` = 0 → F part exactly 0.
///
/// # Errors
/// Non-finite args → `NonFiniteInput`; peak/γ ≤ 0 → `OutOfDomain`.
pub fn scene_to_display(
    scene: [f32; 3],
    peak_nits: f32,
    gamma: f32,
) -> Result<[f32; 3], Cc8KernelError> {
    const FUNCTION: &str = "scene_to_display";
    let scene = finite_input_3(FUNCTION, scene)?;
    let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
    let gamma = positive(FUNCTION, "gamma must be > 0", gamma)?;
    let nonneg = [scene[0].max(0.0), scene[1].max(0.0), scene[2].max(0.0)];
    let luma = BT2020_KR_F32 * nonneg[0] + BT2020_KG_F32 * nonneg[1] + BT2020_KB_F32 * nonneg[2];
    // Positive luma takes the power path; zero luma is exactly zero (never
    // feed 0 to a possibly-negative power). Luma of nonnegatives is ≥ 0.
    let gain = if luma > 0.0 {
        peak * luma.powf(gamma - 1.0)
    } else {
        0.0
    };
    finite_result_3(
        FUNCTION,
        [
            gain * nonneg[0] + peak * scene[0].min(0.0),
            gain * nonneg[1] + peak * scene[1].min(0.0),
            gain * nonneg[2] + peak * scene[2].min(0.0),
        ],
    )
}

/// Inverse rendering, display nits → scene (§2, `display_to_scene` v1):
/// `Y_s = (Y_d/P)^(1/γ)`, `s_c = D_c/(P·Y_s^(γ−1))`, plus P3 signed
/// handling `s = F⁻¹(max(D,0)) + min(D,0)/P`. `Y_d` = 0 → F⁻¹ part exactly 0.
///
/// # Errors
/// Non-finite args → `NonFiniteInput`;
/// peak/γ ≤ 0 → `OutOfDomain`.
pub fn display_to_scene(
    display: [f32; 3],
    peak_nits: f32,
    gamma: f32,
) -> Result<[f32; 3], Cc8KernelError> {
    const FUNCTION: &str = "display_to_scene";
    let display = finite_input_3(FUNCTION, display)?;
    let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
    let gamma = positive(FUNCTION, "gamma must be > 0", gamma)?;
    let nonneg = [
        display[0].max(0.0),
        display[1].max(0.0),
        display[2].max(0.0),
    ];
    let luma = BT2020_KR_F32 * nonneg[0] + BT2020_KG_F32 * nonneg[1] + BT2020_KB_F32 * nonneg[2];
    if luma > 0.0 {
        let scene_luma = (luma / peak).powf(1.0 / gamma);
        let denom = peak * scene_luma.powf(gamma - 1.0);
        finite_result_3(
            FUNCTION,
            [
                nonneg[0] / denom + display[0].min(0.0) / peak,
                nonneg[1] / denom + display[1].min(0.0) / peak,
                nonneg[2] / denom + display[2].min(0.0) / peak,
            ],
        )
    } else {
        finite_result_3(
            FUNCTION,
            [
                display[0].min(0.0) / peak,
                display[1].min(0.0) / peak,
                display[2].min(0.0) / peak,
            ],
        )
    }
}

// ========================================================================
// EETF to target (§6): PQ-normalized Hermite, source span, clip+flag past Cs.

/// `eetf_to_target`: `value` = mapped nits (or input unchanged), `clipped` = past Cs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EetfOutput<T> {
    pub value: T,
    pub clipped: bool,
}

/// §6 Hermite core in f64, shared by both precisions (the f32 entry widens:
/// near-equal ceilings round equal in f32 PQ). `None` = degenerate source
/// span or knee — the caller saturates to Ct and never divides by ≤ 0.
/// `q0` IS `PQ_OETF(0)` (§6 P2).
// Single-letter bindings below are the §6 symbols (x, m, H) verbatim.
#[allow(clippy::many_single_char_names)]
fn eetf_hermite_64(l: f64, cs: f64, ct: f64) -> Option<f64> {
    let q0 = reference::pq_oetf_unchecked(0.0);
    let span = reference::pq_oetf_unchecked(cs) - q0;
    if !span.is_finite() || span <= 0.0 {
        return None;
    }
    let x = (reference::pq_oetf_unchecked(l) - q0) / span;
    let m = (reference::pq_oetf_unchecked(ct) - q0) / span;
    let knee = 1.5 * m - 0.5;
    let dk = 1.0 - knee;
    let h = if x < knee {
        x
    } else if !dk.is_finite() || dk <= 0.0 {
        return None; // m ≥ 1 by rounding: at/above knee saturates to Ct
    } else {
        let t = (x - knee) / dk;
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * knee + (t3 - 2.0 * t2 + t) * dk + (-2.0 * t3 + 3.0 * t2) * m
    };
    reference::pq_eotf_unchecked(q0 + span * h)
}

/// EETF to target peak (§6). Identity/clip/endpoint decide in nits before
/// any PQ arithmetic: exact 0→0; Ct ≥ Cs identity; L > Cs clips with flag;
/// L == Cs maps exactly to Ct, unflagged.
///
/// # Errors
/// Non-finite args → `NonFiniteInput`; Cs/Ct ≤ 0 → `OutOfDomain`.
pub fn eetf_to_target(
    nits: f32,
    source_ceiling_nits: f32,
    target_peak_nits: f32,
) -> Result<EetfOutput<f32>, Cc8KernelError> {
    const FUNCTION: &str = "eetf_to_target";
    let l = finite_input(FUNCTION, nits)?;
    let cs = positive(FUNCTION, "source ceiling must be > 0", source_ceiling_nits)?;
    let ct = positive(FUNCTION, "target peak must be > 0", target_peak_nits)?;
    if l < 0.0 {
        return Ok(EetfOutput {
            value: l,
            clipped: false,
        }); // never fed to PQ
    }
    if l <= 0.0 {
        return Ok(EetfOutput {
            value: 0.0,
            clipped: false,
        }); // exact 0→0
    }
    if ct >= cs {
        return Ok(EetfOutput {
            value: l,
            clipped: false,
        }); // identity
    }
    if l > cs {
        return Ok(EetfOutput {
            value: ct,
            clipped: true,
        }); // past Cs
    }
    // Exact endpoint (bit equality is the spec): source maps to target.
    #[allow(clippy::float_cmp)]
    if l == cs {
        return Ok(EetfOutput {
            value: ct,
            clipped: false,
        }); // source→target endpoint
    }
    // L < Cs: §6 Hermite via the f64 core (local widen); degenerate spans
    // saturate to Ct rather than dividing.
    match eetf_hermite_64(f64::from(l), f64::from(cs), f64::from(ct)) {
        Some(v) => {
            #[allow(clippy::cast_possible_truncation)]
            let narrowed = v as f32;
            Ok(EetfOutput {
                value: finite_result(FUNCTION, narrowed)?,
                clipped: false,
            })
        }
        None => Ok(EetfOutput {
            value: ct,
            clipped: false,
        }),
    }
}

// ========================================================================
// 2020↔709 primaries matrices (§5, R13): derived from the two primary sets
// + D65 (test-only derivation pins the transcription), f32 narrowed.

/// Rec.2020 → BT.709 linear matrix, row-major. Negatives are out-of-triangle
/// colours: never clamped (R12/R13).
pub const BT2020_TO_BT709: [[f32; 3]; 3] = [
    [1.660_491, -0.587_641_1, -0.072_849_86],
    [-0.124_550_48, 1.132_899_9, -0.008_349_422],
    [-0.018_150_764, -0.100_578_9, 1.118_729_7],
];

/// BT.709 → Rec.2020 linear matrix, row-major: the delivery direction.
pub const BT709_TO_BT2020: [[f32; 3]; 3] = [
    [0.627_403_9, 0.329_283_03, 0.043_313_067],
    [0.069_097_29, 0.919_540_4, 0.011_362_315],
    [0.016_391_44, 0.088_013_306, 0.895_595_25],
];

/// f64 transcription of [`BT2020_TO_BT709`] (full-precision derivation).
pub const BT2020_TO_BT709_F64: [[f64; 3]; 3] = [
    [
        1.660_491_002_108_434_7,
        -0.587_641_138_788_549_5,
        -0.072_849_863_319_884_86,
    ],
    [
        -0.124_550_474_521_590_52,
        1.132_899_897_125_959_8,
        -0.008_349_422_604_369_487,
    ],
    [
        -0.018_150_763_354_905_22,
        -0.100_578_898_008_007_36,
        1.118_729_661_362_912_5,
    ],
];

/// f64 transcription of [`BT709_TO_BT2020`] (full-precision derivation).
pub const BT709_TO_BT2020_F64: [[f64; 3]; 3] = [
    [
        0.627_403_895_934_699,
        0.329_283_038_377_883_8,
        0.043_313_065_687_417_22,
    ],
    [
        0.069_097_289_358_231_99,
        0.919_540_395_075_459,
        0.011_362_315_566_309_157,
    ],
    [
        0.016_391_438_875_150_228,
        0.088_013_307_877_225_78,
        0.895_595_253_247_624,
    ],
];

/// Primaries matrix multiply, no clamp ever (R13).
/// # Errors
/// Non-finite → `NonFiniteInput`.
pub fn apply_matrix(m: [[f32; 3]; 3], rgb: [f32; 3]) -> Result<[f32; 3], Cc8KernelError> {
    const FUNCTION: &str = "apply_matrix";
    let rgb = finite_input_3(FUNCTION, rgb)?;
    finite_result_3(
        FUNCTION,
        [
            m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
            m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
            m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
        ],
    )
}

// Gamut compressor + HLG delivery kernel (§6, R30).

const BT709_KR_F32: f32 = 0.2126;
const BT709_KG_F32: f32 = 0.7152;
const BT709_KB_F32: f32 = 0.0722;
const BT709_KR_F64: f64 = 0.2126;
const BT709_KG_F64: f64 = 0.7152;
const BT709_KB_F64: f64 = 0.0722;

/// Volume for [`gamut_compress`]: `Sdr` (709, `U = Ct`) or `Hlg` (2020, `U` rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompressDest<T> {
    Sdr { target_peak: T },
    Hlg { peak: T, gamma: T },
}

/// [`gamut_compress`]: `value` = fitted RGB, `y_clamped`, `compressed` (`t < 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamutOutput<T> {
    pub value: [T; 3],
    pub y_clamped: bool,
    pub compressed: bool,
}

/// [`hlg_output`]: `signal` = clamped HLG triple, `fit` = fit report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HlgOutput<T> {
    pub signal: [T; 3],
    pub fit: GamutOutput<T>,
}

/// Gamut compression in destination RGB (§6): resolve Y into range, `Y = 0`
/// → black, else affine `c′ = Y + t(c−Y)` with destination luma/U.
///
/// # Errors
/// Non-finite → `NonFiniteInput`; peak/Ct/γ ≤ 0 → `OutOfDomain`.
pub fn gamut_compress(
    rgb: [f32; 3],
    dest: CompressDest<f32>,
) -> Result<GamutOutput<f32>, Cc8KernelError> {
    const FUNCTION: &str = "gamut_compress";
    let rgb = finite_input_3(FUNCTION, rgb)?;
    let (kr, kg, kb, ymax, hlg) = match dest {
        CompressDest::Sdr { target_peak } => {
            let ct = positive(FUNCTION, "target peak must be > 0", target_peak)?;
            (BT709_KR_F32, BT709_KG_F32, BT709_KB_F32, ct, None)
        }
        CompressDest::Hlg { peak, gamma } => {
            let p = positive(FUNCTION, "peak must be > 0", peak)?;
            let g = positive(FUNCTION, "gamma must be > 0", gamma)?;
            (BT2020_KR_F32, BT2020_KG_F32, BT2020_KB_F32, p, Some((p, g)))
        }
    };
    let y = kr * rgb[0] + kg * rgb[1] + kb * rgb[2];
    let (y, y_clamped) = if y < 0.0 {
        (0.0, true)
    } else if y > ymax {
        (ymax, true)
    } else {
        (y, false)
    };
    if y <= 0.0 {
        return Ok(GamutOutput {
            value: [0.0, 0.0, 0.0],
            y_clamped,
            compressed: false,
        });
    }
    let upper = match hlg {
        Some((p, g)) => p * (y / p).powf((g - 1.0) / g),
        None => ymax,
    };
    let mut t = 1.0f32;
    for c in rgb {
        if c < y {
            t = t.min(y / (y - c));
        } else if c > y {
            t = t.min((upper - y) / (c - y));
        }
    }
    let value = finite_result_3(
        FUNCTION,
        [
            y + t * (rgb[0] - y),
            y + t * (rgb[1] - y),
            y + t * (rgb[2] - y),
        ],
    )?;
    Ok(GamutOutput {
        value,
        y_clamped,
        compressed: t < 1.0,
    })
}

/// HLG delivery kernel (§6): volume fit → inverse OOTF + OETF at the target
/// peak → post-OETF clamp to `[0, 1]` (float residue only).
///
/// # Errors
/// Non-finite → `NonFiniteInput`; peak/γ ≤ 0 → `OutOfDomain`.
pub fn hlg_output(
    display: [f32; 3],
    target_peak: f32,
    gamma: f32,
) -> Result<HlgOutput<f32>, Cc8KernelError> {
    let fit = gamut_compress(
        display,
        CompressDest::Hlg {
            peak: target_peak,
            gamma,
        },
    )?;
    let scene = display_to_scene(fit.value, target_peak, gamma)?;
    let signal = [
        hlg_oetf_unchecked(scene[0]).clamp(0.0, 1.0),
        hlg_oetf_unchecked(scene[1]).clamp(0.0, 1.0),
        hlg_oetf_unchecked(scene[2]).clamp(0.0, 1.0),
    ];
    Ok(HlgOutput { signal, fit })
}

// f64 conformance reference (App. N path; f32 agreement is itself a test).

/// f64 conformance reference for every f32 production entry point above.
/// Same domains, same refusals, same branch ownership.
pub mod reference {
    use super::{
        BT709_KB_F64, BT709_KG_F64, BT709_KR_F64, BT2020_KB_F64, BT2020_KG_F64, BT2020_KR_F64,
        Cc8KernelError, CompressDest, EetfOutput, GamutOutput, HLG_A_F64, HLG_B_F64, HLG_C_F64,
        HLG_SCENE_BREAKPOINT_F64, HLG_SIGNAL_BREAKPOINT_F64, HlgOutput, PQ_C1_F64, PQ_C2_F64,
        PQ_C3_F64, PQ_M1_F64, PQ_M2_F64, PQ_PEAK_NITS_F64, finite_input, finite_input_3,
        finite_result, finite_result_3, positive,
    };

    /// f64 [`super::pq_oetf`].
    ///
    /// # Errors
    /// Same as [`super::pq_oetf`].
    pub fn pq_oetf(nits: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::pq_oetf";
        let nits = finite_input(FUNCTION, nits)?;
        if nits < 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "PQ input must be >= 0 nits",
            });
        }
        finite_result(FUNCTION, pq_oetf_unchecked(nits))
    }

    pub(crate) fn pq_oetf_unchecked(nits: f64) -> f64 {
        let y = (nits / PQ_PEAK_NITS_F64).powf(PQ_M1_F64);
        ((PQ_C1_F64 + PQ_C2_F64 * y) / (1.0 + PQ_C3_F64 * y)).powf(PQ_M2_F64)
    }

    /// f64 [`super::pq_eotf`].
    ///
    /// # Errors
    /// Same as [`super::pq_eotf`].
    pub fn pq_eotf(signal: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::pq_eotf";
        let signal = finite_input(FUNCTION, signal)?;
        if signal < 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "PQ input must be >= 0",
            });
        }
        match pq_eotf_unchecked(signal) {
            Some(nits) => finite_result(FUNCTION, nits),
            None => Err(Cc8KernelError::NonFiniteResult { function: FUNCTION }),
        }
    }

    pub(crate) fn pq_eotf_unchecked(signal: f64) -> Option<f64> {
        let p = signal.powf(1.0 / PQ_M2_F64);
        let denominator = PQ_C2_F64 - PQ_C3_F64 * p;
        if denominator <= 0.0 {
            return None;
        }
        Some(PQ_PEAK_NITS_F64 * ((p - PQ_C1_F64).max(0.0) / denominator).powf(1.0 / PQ_M1_F64))
    }

    /// f64 [`super::hlg_oetf`].
    ///
    /// # Errors
    /// Same as [`super::hlg_oetf`].
    pub fn hlg_oetf(scene: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::hlg_oetf";
        let scene = finite_input(FUNCTION, scene)?;
        finite_result(FUNCTION, hlg_oetf_unchecked(scene))
    }

    fn hlg_oetf_unchecked(scene: f64) -> f64 {
        let magnitude = scene.abs();
        let encoded = if magnitude <= HLG_SCENE_BREAKPOINT_F64 {
            (3.0 * magnitude).sqrt()
        } else {
            HLG_A_F64 * (12.0 * magnitude - HLG_B_F64).ln() + HLG_C_F64
        };
        encoded.copysign(scene)
    }

    /// f64 [`super::hlg_inverse_oetf`].
    ///
    /// # Errors
    /// Same as [`super::hlg_inverse_oetf`].
    pub fn hlg_inverse_oetf(signal: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::hlg_inverse_oetf";
        let signal = finite_input(FUNCTION, signal)?;
        finite_result(FUNCTION, hlg_inverse_oetf_unchecked(signal))
    }

    fn hlg_inverse_oetf_unchecked(signal: f64) -> f64 {
        let magnitude = signal.abs();
        let decoded = if magnitude <= HLG_SIGNAL_BREAKPOINT_F64 {
            magnitude * magnitude / 3.0
        } else {
            (((magnitude - HLG_C_F64) / HLG_A_F64).exp() + HLG_B_F64) / 12.0
        };
        decoded.copysign(signal)
    }

    /// f64 [`super::hlg_gamma`].
    ///
    /// # Errors
    /// Same as [`super::hlg_gamma`].
    pub fn hlg_gamma(peak_nits: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::hlg_gamma";
        let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
        finite_result(FUNCTION, hlg_gamma_unchecked(peak))
    }

    fn hlg_gamma_unchecked(peak_nits: f64) -> f64 {
        if peak_nits <= 2000.0 {
            1.2 + 0.42 * (peak_nits / 1000.0).log10()
        } else {
            1.2 * 1.111_f64.powf((peak_nits / 1000.0).log2())
        }
    }

    /// f64 [`super::s_white`].
    ///
    /// # Errors
    /// Same as [`super::s_white`].
    pub fn s_white(white_nits: f64, peak_nits: f64, gamma: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::s_white";
        let white = positive(FUNCTION, "white must be > 0", white_nits)?;
        let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
        let gamma = positive(FUNCTION, "gamma must be > 0", gamma)?;
        finite_result(FUNCTION, (white / peak).powf(1.0 / gamma))
    }

    /// f64 [`super::scene_to_working`].
    ///
    /// # Errors
    /// Same as [`super::scene_to_working`].
    pub fn scene_to_working(scene: [f64; 3], ref_white: f64) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::scene_to_working";
        let scene = finite_input_3(FUNCTION, scene)?;
        let sw = positive(FUNCTION, "s_white must be > 0", ref_white)?;
        finite_result_3(FUNCTION, [scene[0] / sw, scene[1] / sw, scene[2] / sw])
    }

    /// f64 [`super::scene_to_display`].
    ///
    /// # Errors
    /// Same as [`super::scene_to_display`].
    pub fn scene_to_display(
        scene: [f64; 3],
        peak_nits: f64,
        gamma: f64,
    ) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::scene_to_display";
        let scene = finite_input_3(FUNCTION, scene)?;
        let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
        let gamma = positive(FUNCTION, "gamma must be > 0", gamma)?;
        let nonneg = [scene[0].max(0.0), scene[1].max(0.0), scene[2].max(0.0)];
        let luma =
            BT2020_KR_F64 * nonneg[0] + BT2020_KG_F64 * nonneg[1] + BT2020_KB_F64 * nonneg[2];
        let gain = if luma > 0.0 {
            peak * luma.powf(gamma - 1.0)
        } else {
            0.0
        };
        finite_result_3(
            FUNCTION,
            [
                gain * nonneg[0] + peak * scene[0].min(0.0),
                gain * nonneg[1] + peak * scene[1].min(0.0),
                gain * nonneg[2] + peak * scene[2].min(0.0),
            ],
        )
    }

    /// f64 [`super::display_to_scene`].
    ///
    /// # Errors
    /// Same as [`super::display_to_scene`].
    pub fn display_to_scene(
        display: [f64; 3],
        peak_nits: f64,
        gamma: f64,
    ) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::display_to_scene";
        let display = finite_input_3(FUNCTION, display)?;
        let peak = positive(FUNCTION, "peak must be > 0", peak_nits)?;
        let gamma = positive(FUNCTION, "gamma must be > 0", gamma)?;
        let nonneg = [
            display[0].max(0.0),
            display[1].max(0.0),
            display[2].max(0.0),
        ];
        let luma =
            BT2020_KR_F64 * nonneg[0] + BT2020_KG_F64 * nonneg[1] + BT2020_KB_F64 * nonneg[2];
        if luma > 0.0 {
            let scene_luma = (luma / peak).powf(1.0 / gamma);
            let denom = peak * scene_luma.powf(gamma - 1.0);
            finite_result_3(
                FUNCTION,
                [
                    nonneg[0] / denom + display[0].min(0.0) / peak,
                    nonneg[1] / denom + display[1].min(0.0) / peak,
                    nonneg[2] / denom + display[2].min(0.0) / peak,
                ],
            )
        } else {
            finite_result_3(
                FUNCTION,
                [
                    display[0].min(0.0) / peak,
                    display[1].min(0.0) / peak,
                    display[2].min(0.0) / peak,
                ],
            )
        }
    }

    /// f64 [`super::eetf_to_target`].
    ///
    /// # Errors
    /// Same as [`super::eetf_to_target`].
    pub fn eetf_to_target(
        nits: f64,
        source_ceiling_nits: f64,
        target_peak_nits: f64,
    ) -> Result<EetfOutput<f64>, Cc8KernelError> {
        const FUNCTION: &str = "reference::eetf_to_target";
        let l = finite_input(FUNCTION, nits)?;
        let cs = positive(FUNCTION, "source ceiling must be > 0", source_ceiling_nits)?;
        let ct = positive(FUNCTION, "target peak must be > 0", target_peak_nits)?;
        if l < 0.0 {
            return Ok(EetfOutput {
                value: l,
                clipped: false,
            });
        }
        if l <= 0.0 {
            return Ok(EetfOutput {
                value: 0.0,
                clipped: false,
            });
        }
        if ct >= cs {
            return Ok(EetfOutput {
                value: l,
                clipped: false,
            });
        }
        if l > cs {
            return Ok(EetfOutput {
                value: ct,
                clipped: true,
            });
        }
        // Exact endpoint (bit equality is the spec): source maps to target.
        #[allow(clippy::float_cmp)]
        if l == cs {
            return Ok(EetfOutput {
                value: ct,
                clipped: false,
            });
        }
        match super::eetf_hermite_64(l, cs, ct) {
            Some(v) => Ok(EetfOutput {
                value: finite_result(FUNCTION, v)?,
                clipped: false,
            }),
            None => Ok(EetfOutput {
                value: ct,
                clipped: false,
            }),
        }
    }

    /// f64 [`super::gamut_compress`].
    /// # Errors
    /// Same as [`super::gamut_compress`].
    pub fn gamut_compress(
        rgb: [f64; 3],
        dest: CompressDest<f64>,
    ) -> Result<GamutOutput<f64>, Cc8KernelError> {
        const FUNCTION: &str = "reference::gamut_compress";
        let rgb = finite_input_3(FUNCTION, rgb)?;
        let (kr, kg, kb, ymax, hlg) = match dest {
            CompressDest::Sdr { target_peak } => {
                let ct = positive(FUNCTION, "target peak must be > 0", target_peak)?;
                (BT709_KR_F64, BT709_KG_F64, BT709_KB_F64, ct, None)
            }
            CompressDest::Hlg { peak, gamma } => {
                let p = positive(FUNCTION, "peak must be > 0", peak)?;
                let g = positive(FUNCTION, "gamma must be > 0", gamma)?;
                (BT2020_KR_F64, BT2020_KG_F64, BT2020_KB_F64, p, Some((p, g)))
            }
        };
        let y = kr * rgb[0] + kg * rgb[1] + kb * rgb[2];
        let (y, y_clamped) = if y < 0.0 {
            (0.0, true)
        } else if y > ymax {
            (ymax, true)
        } else {
            (y, false)
        };
        if y <= 0.0 {
            return Ok(GamutOutput {
                value: [0.0, 0.0, 0.0],
                y_clamped,
                compressed: false,
            });
        }
        let upper = match hlg {
            Some((p, g)) => p * (y / p).powf((g - 1.0) / g),
            None => ymax,
        };
        let mut t = 1.0f64;
        for c in rgb {
            if c < y {
                t = t.min(y / (y - c));
            } else if c > y {
                t = t.min((upper - y) / (c - y));
            }
        }
        let value = finite_result_3(
            FUNCTION,
            [
                y + t * (rgb[0] - y),
                y + t * (rgb[1] - y),
                y + t * (rgb[2] - y),
            ],
        )?;
        Ok(GamutOutput {
            value,
            y_clamped,
            compressed: t < 1.0,
        })
    }

    /// f64 [`super::hlg_output`].
    /// # Errors
    /// Same as [`super::hlg_output`].
    pub fn hlg_output(
        display: [f64; 3],
        target_peak: f64,
        gamma: f64,
    ) -> Result<HlgOutput<f64>, Cc8KernelError> {
        let fit = gamut_compress(
            display,
            CompressDest::Hlg {
                peak: target_peak,
                gamma,
            },
        )?;
        let scene = display_to_scene(fit.value, target_peak, gamma)?;
        let signal = [
            hlg_oetf_unchecked(scene[0]).clamp(0.0, 1.0),
            hlg_oetf_unchecked(scene[1]).clamp(0.0, 1.0),
            hlg_oetf_unchecked(scene[2]).clamp(0.0, 1.0),
        ];
        Ok(HlgOutput { signal, fit })
    }

    /// f64 [`super::apply_matrix`].
    /// # Errors
    /// Same as [`super::apply_matrix`].
    pub fn apply_matrix(m: [[f64; 3]; 3], rgb: [f64; 3]) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::apply_matrix";
        let rgb = finite_input_3(FUNCTION, rgb)?;
        finite_result_3(
            FUNCTION,
            [
                m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
                m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
                m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
            ],
        )
    }
}
