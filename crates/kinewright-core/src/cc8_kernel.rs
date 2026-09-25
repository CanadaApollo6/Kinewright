//! CC8 stage S1 numeric kernel: pure colour maths, no I/O.
//!
//! Design `docs/CC8-FLEXIBLE-COLOUR-AND-YOUTUBE-HDR.md`, §§2–3, 6, 14.
//! The f32 entry points are the production path the S3 renderer calls; the
//! [`reference`] module is the f64 conformance path (App. N anchors, ±1e-6).
//! S1 fails closed: every entry point refuses non-finite input, non-finite
//! results (overflow), and out-of-domain arguments with [`Cc8KernelError`]
//! instead of clamping. S2 maps this error into `OpError`/incidents.

use thiserror::Error;

/// Typed S1 kernel failure: non-finite input/result, out-of-domain argument,
/// or unsupported version. Failing closed — never a quiet clamp (R35).
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum Cc8KernelError {
    /// A floating-point argument was NaN or infinite.
    #[error("cc8 kernel: {function}: non-finite input {value}")]
    NonFiniteInput {
        /// Calling entry point, for incident attribution.
        function: &'static str,
        /// The offending value, widened for reporting.
        value: f64,
    },
    /// The correctly-rounded result would be NaN or infinite (overflow, or
    /// past a pole such as ST 2084's); the caller must handle it, not clamp.
    #[error("cc8 kernel: {function}: non-finite result (overflow), refusing")]
    NonFiniteResult {
        /// Calling entry point, for incident attribution.
        function: &'static str,
    },
    /// A finite argument outside the function's domain (negative nits/signal
    /// into PQ, non-positive peak/white, …).
    #[error("cc8 kernel: {function}: out of domain ({reason})")]
    OutOfDomain {
        /// Calling entry point, for incident attribution.
        function: &'static str,
        /// Which domain rule was violated.
        reason: &'static str,
    },
    /// A versioned rule (HLG gamma, `display_to_scene`) was asked for a rule
    /// version other than the pinned one.
    #[error("cc8 kernel: {function}: unsupported version {version}")]
    UnsupportedVersion {
        /// Calling entry point, for incident attribution.
        function: &'static str,
        /// The requested rule version.
        version: u16,
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

// ---------------------------------------------------------------------------
// ST 2084 (PQ). Constants are exact binary fractions in both precisions.
// ---------------------------------------------------------------------------

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
///
/// [`Cc8KernelError::NonFiniteInput`] for NaN/infinite nits,
/// [`Cc8KernelError::OutOfDomain`] for negative nits, or
/// [`Cc8KernelError::NonFiniteResult`] on overflow.
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

/// `q0`, defined exclusively as `PQ_OETF(0)` (§6 P2): `c1^m2 ≈ 7.31e-7`,
/// not 0. Infallible: 0.0 is finite and its image is finite.
#[must_use]
pub fn pq_q0() -> f32 {
    pq_oetf_unchecked(0.0)
}

fn pq_oetf_unchecked(nits: f32) -> f32 {
    let y = (nits / PQ_PEAK_NITS_F32).powf(PQ_M1_F32);
    ((PQ_C1_F32 + PQ_C2_F32 * y) / (1.0 + PQ_C3_F32 * y)).powf(PQ_M2_F32)
}

/// ST 2084 EOTF: PQ signal → absolute nits. Exact 0 → 0.
///
/// # Errors
///
/// [`Cc8KernelError::NonFiniteInput`] for NaN/infinite signals,
/// [`Cc8KernelError::OutOfDomain`] for negative signals, or
/// [`Cc8KernelError::NonFiniteResult`] at/past the rational-form pole
/// (`E′ = (c2/c3)^m2 ≈ 1.992`).
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

// ---------------------------------------------------------------------------
// ARIB STD-B67 (HLG). a/b/c are the standard's rounded decimals.
// ---------------------------------------------------------------------------

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
///
/// [`Cc8KernelError::NonFiniteInput`] for NaN/infinite input or
/// [`Cc8KernelError::NonFiniteResult`] on overflow.
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
///
/// [`Cc8KernelError::NonFiniteInput`] for NaN/infinite input or
/// [`Cc8KernelError::NonFiniteResult`] on overflow.
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

// ---------------------------------------------------------------------------
// HLG system gamma rule, `gamma_rule_version = 1` (§3).
// ---------------------------------------------------------------------------

/// The pinned HLG gamma rule version. Only this version exists.
pub const HLG_GAMMA_RULE_VERSION: u16 = 1;

/// HLG system gamma at peak `peak_nits` (BT.2100-3 Table 5 Note 5f):
/// P ≤ 2000 takes the log rule, P > 2000 the power rule — the log rule owns
/// the boundary at exactly 2000.
///
/// # Errors
///
/// [`Cc8KernelError::UnsupportedVersion`] for any `rule_version` other than
/// [`HLG_GAMMA_RULE_VERSION`], [`Cc8KernelError::NonFiniteInput`] for
/// NaN/infinite peaks, [`Cc8KernelError::OutOfDomain`] for peaks ≤ 0, or
/// [`Cc8KernelError::NonFiniteResult`] on overflow.
pub fn hlg_gamma(rule_version: u16, peak_nits: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "hlg_gamma";
    if rule_version != HLG_GAMMA_RULE_VERSION {
        return Err(Cc8KernelError::UnsupportedVersion {
            function: FUNCTION,
            version: rule_version,
        });
    }
    let peak = finite_input(FUNCTION, peak_nits)?;
    if peak <= 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "peak must be > 0 nits",
        });
    }
    finite_result(FUNCTION, hlg_gamma_unchecked(peak))
}

fn hlg_gamma_unchecked(peak_nits: f32) -> f32 {
    if peak_nits <= 2000.0 {
        1.2 + 0.42 * (peak_nits / 1000.0).log10()
    } else {
        1.2 * 1.111_f32.powf((peak_nits / 1000.0).log2())
    }
}

// ---------------------------------------------------------------------------
// Reference-scene normalization + scene/display rendering (§2).
// α = 1, β = 0 pinned. Gamma arrives resolved (locks pin it, §2); the S3
// caller resolves it once per frame via `hlg_gamma`, not per pixel.
// ---------------------------------------------------------------------------

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

/// Reference-scene white `s_white = (W/P)^(1/γ)` (§2). Working = `scene/s_white`.
///
/// # Errors
/// Non-finite args → `NonFiniteInput`; W/P/γ ≤ 0 → `OutOfDomain`.
pub fn s_white(white_nits: f32, peak_nits: f32, gamma: f32) -> Result<f32, Cc8KernelError> {
    const FUNCTION: &str = "s_white";
    let white = finite_input(FUNCTION, white_nits)?;
    let peak = finite_input(FUNCTION, peak_nits)?;
    let gamma = finite_input(FUNCTION, gamma)?;
    if white <= 0.0 || peak <= 0.0 || gamma <= 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "white/peak/gamma must be > 0",
        });
    }
    finite_result(FUNCTION, (white / peak).powf(1.0 / gamma))
}

/// Normalize scene to working: `w = s/s_white` per component (§2).
///
/// # Errors
/// Non-finite args → `NonFiniteInput`; `s_white` ≤ 0 → `OutOfDomain`.
pub fn scene_to_working(scene: [f32; 3], ref_white: f32) -> Result<[f32; 3], Cc8KernelError> {
    const FUNCTION: &str = "scene_to_working";
    let scene = finite_input_3(FUNCTION, scene)?;
    let sw = finite_input(FUNCTION, ref_white)?;
    if sw <= 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "s_white must be > 0",
        });
    }
    finite_result_3(FUNCTION, [scene[0] / sw, scene[1] / sw, scene[2] / sw])
}

/// Denormalize working to scene: `s = w·s_white` per component (§2).
///
/// # Errors
/// Non-finite args → `NonFiniteInput`; `s_white` ≤ 0 → `OutOfDomain`.
pub fn working_to_scene(working: [f32; 3], ref_white: f32) -> Result<[f32; 3], Cc8KernelError> {
    const FUNCTION: &str = "working_to_scene";
    let working = finite_input_3(FUNCTION, working)?;
    let sw = finite_input(FUNCTION, ref_white)?;
    if sw <= 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "s_white must be > 0",
        });
    }
    finite_result_3(
        FUNCTION,
        [working[0] * sw, working[1] * sw, working[2] * sw],
    )
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
    let peak = finite_input(FUNCTION, peak_nits)?;
    let gamma = finite_input(FUNCTION, gamma)?;
    if peak <= 0.0 || gamma <= 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "peak/gamma must be > 0",
        });
    }
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

/// The pinned `display_to_scene` rule version. Only this version exists.
pub const DISPLAY_TO_SCENE_VERSION: u16 = 1;

/// Inverse rendering, display nits → scene (§2 `display_to_scene` v1):
/// `Y_s = (Y_d/P)^(1/γ)`, `s_c = D_c/(P·Y_s^(γ−1))`, plus P3 signed
/// handling `s = F⁻¹(max(D,0)) + min(D,0)/P`. `Y_d` = 0 → F⁻¹ part exactly 0.
///
/// # Errors
/// Other versions → `UnsupportedVersion`; non-finite args → `NonFiniteInput`;
/// peak/γ ≤ 0 → `OutOfDomain`.
pub fn display_to_scene(
    rule_version: u16,
    display: [f32; 3],
    peak_nits: f32,
    gamma: f32,
) -> Result<[f32; 3], Cc8KernelError> {
    const FUNCTION: &str = "display_to_scene";
    if rule_version != DISPLAY_TO_SCENE_VERSION {
        return Err(Cc8KernelError::UnsupportedVersion {
            function: FUNCTION,
            version: rule_version,
        });
    }
    let display = finite_input_3(FUNCTION, display)?;
    let peak = finite_input(FUNCTION, peak_nits)?;
    let gamma = finite_input(FUNCTION, gamma)?;
    if peak <= 0.0 || gamma <= 0.0 {
        return Err(Cc8KernelError::OutOfDomain {
            function: FUNCTION,
            reason: "peak/gamma must be > 0",
        });
    }
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

// ---------------------------------------------------------------------------
// f64 conformance reference: the App. N path. Tested ±1e-6 against the
// independent vectors; f32 production agreement with this module is itself
// a test (R35).
// ---------------------------------------------------------------------------

/// f64 conformance reference for every f32 production entry point above.
/// Same domains, same refusals, same branch ownership.
pub mod reference {
    use super::{
        BT2020_KB_F64, BT2020_KG_F64, BT2020_KR_F64, Cc8KernelError, DISPLAY_TO_SCENE_VERSION,
        HLG_A_F64, HLG_B_F64, HLG_C_F64, HLG_GAMMA_RULE_VERSION, HLG_SCENE_BREAKPOINT_F64,
        HLG_SIGNAL_BREAKPOINT_F64, PQ_C1_F64, PQ_C2_F64, PQ_C3_F64, PQ_M1_F64, PQ_M2_F64,
        PQ_PEAK_NITS_F64, finite_input, finite_input_3, finite_result, finite_result_3,
    };

    /// f64 [`super::pq_oetf`].
    ///
    /// # Errors
    ///
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

    /// f64 [`super::pq_q0`].
    #[must_use]
    pub fn pq_q0() -> f64 {
        pq_oetf_unchecked(0.0)
    }

    fn pq_oetf_unchecked(nits: f64) -> f64 {
        let y = (nits / PQ_PEAK_NITS_F64).powf(PQ_M1_F64);
        ((PQ_C1_F64 + PQ_C2_F64 * y) / (1.0 + PQ_C3_F64 * y)).powf(PQ_M2_F64)
    }

    /// f64 [`super::pq_eotf`].
    ///
    /// # Errors
    ///
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

    fn pq_eotf_unchecked(signal: f64) -> Option<f64> {
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
    ///
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
    ///
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
    ///
    /// Same as [`super::hlg_gamma`].
    pub fn hlg_gamma(rule_version: u16, peak_nits: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::hlg_gamma";
        if rule_version != HLG_GAMMA_RULE_VERSION {
            return Err(Cc8KernelError::UnsupportedVersion {
                function: FUNCTION,
                version: rule_version,
            });
        }
        let peak = finite_input(FUNCTION, peak_nits)?;
        if peak <= 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "peak must be > 0 nits",
            });
        }
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
    ///
    /// Same as [`super::s_white`].
    pub fn s_white(white_nits: f64, peak_nits: f64, gamma: f64) -> Result<f64, Cc8KernelError> {
        const FUNCTION: &str = "reference::s_white";
        let white = finite_input(FUNCTION, white_nits)?;
        let peak = finite_input(FUNCTION, peak_nits)?;
        let gamma = finite_input(FUNCTION, gamma)?;
        if white <= 0.0 || peak <= 0.0 || gamma <= 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "white/peak/gamma must be > 0",
            });
        }
        finite_result(FUNCTION, (white / peak).powf(1.0 / gamma))
    }

    /// f64 [`super::scene_to_working`].
    ///
    /// # Errors
    ///
    /// Same as [`super::scene_to_working`].
    pub fn scene_to_working(scene: [f64; 3], ref_white: f64) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::scene_to_working";
        let scene = finite_input_3(FUNCTION, scene)?;
        let sw = finite_input(FUNCTION, ref_white)?;
        if sw <= 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "s_white must be > 0",
            });
        }
        finite_result_3(FUNCTION, [scene[0] / sw, scene[1] / sw, scene[2] / sw])
    }

    /// f64 [`super::working_to_scene`].
    ///
    /// # Errors
    ///
    /// Same as [`super::working_to_scene`].
    pub fn working_to_scene(working: [f64; 3], ref_white: f64) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::working_to_scene";
        let working = finite_input_3(FUNCTION, working)?;
        let sw = finite_input(FUNCTION, ref_white)?;
        if sw <= 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "s_white must be > 0",
            });
        }
        finite_result_3(
            FUNCTION,
            [working[0] * sw, working[1] * sw, working[2] * sw],
        )
    }

    /// f64 [`super::scene_to_display`].
    ///
    /// # Errors
    ///
    /// Same as [`super::scene_to_display`].
    pub fn scene_to_display(
        scene: [f64; 3],
        peak_nits: f64,
        gamma: f64,
    ) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::scene_to_display";
        let scene = finite_input_3(FUNCTION, scene)?;
        let peak = finite_input(FUNCTION, peak_nits)?;
        let gamma = finite_input(FUNCTION, gamma)?;
        if peak <= 0.0 || gamma <= 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "peak/gamma must be > 0",
            });
        }
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
    ///
    /// Same as [`super::display_to_scene`].
    pub fn display_to_scene(
        rule_version: u16,
        display: [f64; 3],
        peak_nits: f64,
        gamma: f64,
    ) -> Result<[f64; 3], Cc8KernelError> {
        const FUNCTION: &str = "reference::display_to_scene";
        if rule_version != DISPLAY_TO_SCENE_VERSION {
            return Err(Cc8KernelError::UnsupportedVersion {
                function: FUNCTION,
                version: rule_version,
            });
        }
        let display = finite_input_3(FUNCTION, display)?;
        let peak = finite_input(FUNCTION, peak_nits)?;
        let gamma = finite_input(FUNCTION, gamma)?;
        if peak <= 0.0 || gamma <= 0.0 {
            return Err(Cc8KernelError::OutOfDomain {
                function: FUNCTION,
                reason: "peak/gamma must be > 0",
            });
        }
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
}
