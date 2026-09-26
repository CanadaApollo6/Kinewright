//! CC8 S1 numeric-kernel conformance suite.
//!
//! Expectations are independent vectors from `/tmp/cc8_oracle.py` (transcribed
//! from ST 2084 / ARIB STD-B67 / BT.2100, cross-checked against App. N), never
//! values the kernel printed. f64 reference anchors hold ±1e-6 per the design;
//! most assert far tighter. Grows one section per S1 increment.

use half::f16;
use kinewright_core::{
    BT709_TO_BT2020, BT709_TO_BT2020_F64, BT2020_TO_BT709, BT2020_TO_BT709_F64, Cc8KernelError,
    CompressDest, apply_matrix, display_to_scene, eetf_to_target, gamut_compress, hlg_gamma,
    hlg_inverse_oetf, hlg_oetf, hlg_output, pq_eotf, pq_oetf, reference, s_white, scene_to_display,
    scene_to_working,
};

/// Narrow an f64 test vector to f32 input. The rounding is the point of the
/// f32-vs-f64 agreement tests, so the truncation lint is allowed once, here.
#[allow(clippy::cast_possible_truncation)]
fn as_f32(x: f64) -> f32 {
    x as f32
}

fn assert_close(label: &str, actual: f64, expected: f64, tol: f64) {
    let diff = (actual - expected).abs();
    assert!(
        diff <= tol,
        "{label}: actual={actual} expected={expected} diff={diff} tol={tol}"
    );
}

// ---------------------------------------------------------------------------
// ST 2084 PQ: App. N anchors (q0, OETF at 203/100/1000/10000).
// ---------------------------------------------------------------------------

#[test]
fn pq_q0_is_c1_to_m2_not_zero() {
    // Oracle: 7.309559025783966e-07; App. N: 7.3e-7 (c1^m2; not 0).
    // q0 is defined exclusively as PQ_OETF(0) (§6 P2): the kernel calls the
    // same unchecked formula there, so asserting pq_oetf(0.0) pins q0.
    assert_close(
        "q0 f64",
        reference::pq_oetf(0.0).unwrap(),
        7.309_559_025_783_966e-07,
        1e-18,
    );
    assert!(
        reference::pq_oetf(0.0).unwrap() > 0.0,
        "q0 must be positive, not 0"
    );
    assert_close(
        "q0 f32",
        f64::from(pq_oetf(0.0).unwrap()),
        7.309_559_025_783_966e-07,
        1e-13,
    );
}

#[test]
fn pq_oetf_matches_independent_anchors() {
    for (nits, expected) in [
        (203.0, 0.580_688_881_041_610_9),
        (100.0, 0.508_078_421_517_399),
        (1000.0, 0.751_827_096_247_041),
        (10_000.0, 1.0),
    ] {
        let actual = reference::pq_oetf(nits).unwrap();
        assert_close("pq_oetf f64", actual, expected, 1e-12);
        // App. N rounded values, ±1e-6.
        assert_close(
            "pq_oetf App N",
            actual,
            (expected * 1e9).round() / 1e9,
            1e-6,
        );
    }
}

#[test]
fn pq_eotf_endpoints_exact() {
    assert_eq!(reference::pq_eotf(0.0).unwrap().to_bits(), 0.0f64.to_bits());
    assert_eq!(pq_eotf(0.0).unwrap().to_bits(), 0.0f32.to_bits());
    assert_close(
        "eotf(1) f64",
        reference::pq_eotf(1.0).unwrap(),
        10_000.0,
        1e-9,
    );
    assert_close(
        "eotf(1) f32",
        f64::from(pq_eotf(1.0).unwrap()),
        10_000.0,
        1e-2,
    );
}

#[test]
fn pq_roundtrip_is_identity() {
    for nits in [0.5, 1.0, 10.0, 100.0, 203.0, 1000.0, 4000.0, 10_000.0] {
        let there = reference::pq_oetf(nits).unwrap();
        let back = reference::pq_eotf(there).unwrap();
        assert_close("pq roundtrip", back / nits, 1.0, 1e-12);
    }
    // The c1^m2 foot maps back to ~0 (at most one rounding above the floor).
    let foot = reference::pq_eotf(reference::pq_oetf(0.0).unwrap()).unwrap();
    assert_close("pq foot", foot, 0.0, 1e-9);
}

#[test]
fn pq_f32_agrees_with_f64() {
    for nits in [0.0, 0.5, 10.0, 100.0, 203.0, 1000.0, 4000.0, 10_000.0] {
        let f32v = f64::from(pq_oetf(as_f32(nits)).unwrap());
        // The outer ^m2 (78.8) amplifies f32 rounding ~79x: 5e-6 abs is ~40
        // ulp here and ≈ 0.005 ten-bit codes. Honest f32 bound.
        assert_close(
            "pq_oetf f32~f64",
            f32v,
            reference::pq_oetf(nits).unwrap(),
            5e-6,
        );
    }
    for signal in [0.0, 0.25, 0.5, 0.75, 1.0, 1.1] {
        let f32v = f64::from(pq_eotf(as_f32(signal)).unwrap());
        let f64v = reference::pq_eotf(signal).unwrap();
        // Inherent to the standard's rational form in f32: c2−c3·p loses ~2
        // digits to cancellation, then ^(1/m1) amplifies ~6.3x. 5e-5
        // relative is 0.5 nit at peak — inside PB3's 2.0-nit peak budget.
        assert_close(
            "pq_eotf f32~f64",
            f32v / f64v.max(1.0),
            f64v / f64v.max(1.0),
            5e-5,
        );
    }
}

#[test]
fn pq_refuses_negative_nonfinite_and_pole() {
    assert!(matches!(
        pq_oetf(-1.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        pq_eotf(-0.5),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::pq_oetf(-1.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(matches!(
            pq_oetf(bad),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
        assert!(matches!(
            pq_eotf(bad),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
    }
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            reference::pq_oetf(bad),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
    }
    // Past the (c2/c3)^m2 ≈ 1.992 pole: refuse, never a fabricated finite.
    assert!(matches!(
        pq_eotf(2.0),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    assert!(matches!(
        reference::pq_eotf(2.5),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    // Just above 1.0 is fine (limited-range max ≈ 1.095) and huge.
    assert!(pq_eotf(1.1).unwrap() > 10_000.0);
}

// ---------------------------------------------------------------------------
// ARIB STD-B67 HLG OETF + inverse.
// ---------------------------------------------------------------------------

#[test]
fn hlg_oetf_anchors_and_seam() {
    // Lower branch is exact at the breakpoint: sqrt(3/12) = 0.5.
    assert_eq!(
        reference::hlg_oetf(1.0 / 12.0).unwrap().to_bits(),
        0.5f64.to_bits()
    );
    // Oracle: 0.9999999955365686; App. N: 0.999999996.
    assert_close(
        "hlg_oetf(1)",
        reference::hlg_oetf(1.0).unwrap(),
        0.999_999_995_536_568_6,
        1e-12,
    );
    // Seam continuity: both branches meet at 0.5.
    let below = reference::hlg_oetf(1.0 / 12.0 - 1e-9).unwrap();
    let above = reference::hlg_oetf(1.0 / 12.0 + 1e-9).unwrap();
    assert_close("hlg seam below", below, 0.5, 1e-7);
    assert_close("hlg seam above", above, 0.5, 1e-7);
    assert_close(
        "hlg_oetf f32(1)",
        f64::from(hlg_oetf(1.0).unwrap()),
        1.0,
        2e-7,
    );
}

#[test]
fn hlg_inverse_oetf_anchors() {
    // 0.5²/3 and 1/12 are the same real with one rounding each: identical.
    assert_eq!(
        reference::hlg_inverse_oetf(0.5).unwrap().to_bits(),
        (1.0f64 / 12.0).to_bits()
    );
    // Oracle: 0.26496255978640015; App. N scene at HLG 0.75: 0.26496256.
    assert_close(
        "inv oetf(0.75)",
        reference::hlg_inverse_oetf(0.75).unwrap(),
        0.264_962_559_786_400_15,
        1e-12,
    );
    // HLG signal of exactly 203 nits: oracle 0.7498773651026114.
    let scene_203 = (203.0f64 / 1000.0).powf(1.0 / 1.2);
    assert_close(
        "hlg signal of 203 nits",
        reference::hlg_oetf(scene_203).unwrap(),
        0.749_877_365_102_611_4,
        1e-12,
    );
    // Nits at nominal HLG 0.75: oracle 203.1521453536661.
    let nits = 1000.0 * reference::hlg_inverse_oetf(0.75).unwrap().powf(1.2);
    assert_close("nits at HLG 0.75", nits, 203.152_145_353_666_1, 1e-9);
}

#[test]
fn hlg_roundtrip_is_identity() {
    for signal in [0.0, 0.1, 0.5, 0.75, 1.0, 1.2] {
        let scene = reference::hlg_inverse_oetf(signal).unwrap();
        let back = reference::hlg_oetf(scene).unwrap();
        assert_close("hlg signal roundtrip", back, signal, 1e-12);
    }
    for scene in [0.0, 1.0 / 12.0, 0.26, 1.0, 3.0] {
        let signal = reference::hlg_oetf(scene).unwrap();
        let back = reference::hlg_inverse_oetf(signal).unwrap();
        assert_close("hlg scene roundtrip", back, scene, 1e-12 * scene.max(1.0));
    }
}

#[test]
fn hlg_sign_symmetry_and_f32_agreement() {
    for x in [0.0, 0.05, 0.5, 0.75, 1.0, 2.5] {
        assert_eq!(
            reference::hlg_oetf(-x).unwrap().to_bits(),
            (-reference::hlg_oetf(x).unwrap()).to_bits(),
            "oetf sign symmetry at {x}"
        );
        assert_eq!(
            reference::hlg_inverse_oetf(-x).unwrap().to_bits(),
            (-reference::hlg_inverse_oetf(x).unwrap()).to_bits(),
            "inverse oetf sign symmetry at {x}"
        );
        assert_close(
            "hlg_oetf f32~f64",
            f64::from(hlg_oetf(as_f32(x)).unwrap()),
            reference::hlg_oetf(x).unwrap(),
            1e-6,
        );
        // exp() branch: scene magnitudes reach thousands, so this is a
        // relative bound (~16 ulp); near zero it behaves as 2e-6 absolute.
        let expected = reference::hlg_inverse_oetf(x).unwrap();
        assert_close(
            "hlg inv oetf f32~f64",
            f64::from(hlg_inverse_oetf(as_f32(x)).unwrap()),
            expected,
            2e-6 * expected.abs().max(1.0),
        );
    }
}

#[test]
fn hlg_refuses_nonfinite_and_overflow() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(matches!(
            hlg_oetf(bad),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
        assert!(matches!(
            hlg_inverse_oetf(bad),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
    }
    // exp() overflow refuses rather than returning infinity.
    assert!(matches!(
        hlg_inverse_oetf(1e10),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    assert!(matches!(
        reference::hlg_inverse_oetf(1e10),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
}

// ---------------------------------------------------------------------------
// HLG gamma rule v1 (§3): log rule P ≤ 2000, power rule above.
// ---------------------------------------------------------------------------

#[test]
fn hlg_gamma_table_matches_independent_vectors() {
    for (peak, expected) in [
        (400.0, 1.032_865_196_357_744_2),
        (1000.0, 1.2),
        (2000.0, 1.326_432_598_178_872),
        (4000.0, 1.481_185_199_999_999_9),
        (10_000.0, 1.702_315_536_266_557),
    ] {
        let actual = reference::hlg_gamma(peak).unwrap();
        assert_close("gamma f64", actual, expected, 1e-12);
        // App. N rounded values, ±1e-6.
        assert_close("gamma App N", actual, (expected * 1e6).round() / 1e6, 1e-6);
        assert_close(
            "gamma f32~f64",
            f64::from(hlg_gamma(as_f32(peak)).unwrap()),
            expected,
            1e-6,
        );
    }
}

#[test]
fn hlg_gamma_log_rule_owns_2000() {
    // The power rule would give exactly 1.3332 at 2000; the log rule gives
    // 1.326432598178872 and owns the boundary.
    let at = reference::hlg_gamma(2000.0).unwrap();
    assert_close("gamma(2000)", at, 1.326_432_598_178_872, 1e-12);
    assert!(at < 1.33, "log rule must own P=2000, got {at}");
    // Just above, the power rule takes over with a visible upward step.
    let above = reference::hlg_gamma(2001.0).unwrap();
    assert!(above - at > 0.005, "power rule must start above 2000");
    assert_close(
        "gamma f32(2000)",
        f64::from(hlg_gamma(2000.0).unwrap()),
        at,
        1e-6,
    );
}

#[test]
fn hlg_gamma_refuses_bad_peaks() {
    for peak in [0.0, -100.0] {
        assert!(matches!(
            hlg_gamma(peak),
            Err(Cc8KernelError::OutOfDomain { .. })
        ));
    }
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(matches!(
            hlg_gamma(bad),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
    }
}

// ---------------------------------------------------------------------------
// C2: reference-scene normalization + rendering (§2).
// ---------------------------------------------------------------------------

fn assert_close_3(label: &str, actual: [f64; 3], expected: [f64; 3], tol: f64) {
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_close(&format!("{label}[{i}]"), *a, *e, tol);
    }
}

fn assert_bits_3(label: &str, actual: [f64; 3], expected: [f64; 3]) {
    let a = [
        actual[0].to_bits(),
        actual[1].to_bits(),
        actual[2].to_bits(),
    ];
    let e = [
        expected[0].to_bits(),
        expected[1].to_bits(),
        expected[2].to_bits(),
    ];
    assert_eq!(a, e, "{label}");
}

fn assert_bits_3_f32(label: &str, actual: [f32; 3], expected: [f32; 3]) {
    let a = [
        actual[0].to_bits(),
        actual[1].to_bits(),
        actual[2].to_bits(),
    ];
    let e = [
        expected[0].to_bits(),
        expected[1].to_bits(),
        expected[2].to_bits(),
    ];
    assert_eq!(a, e, "{label}");
}

fn f64_3(v: [f32; 3]) -> [f64; 3] {
    [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]
}

#[test]
fn s_white_anchors() {
    // Oracle: 0.26479718562407867; App. N: 0.26479719.
    let sw = reference::s_white(203.0, 1000.0, 1.2).unwrap();
    assert_close("s_white", sw, 0.264_797_185_624_078_67, 1e-12);
    assert_close("s_white App N", sw, 0.264_797_19, 1e-6);
    assert_close("w_peak", 1.0 / sw, 3.776_475_182_858_089_6, 1e-12);
    assert_close("w_peak App N", 1.0 / sw, 3.776_475, 1e-6);
    // White extreme W=100: oracle 0.14677992676220694; App. N 0.146780.
    let sw100 = reference::s_white(100.0, 1000.0, 1.2).unwrap();
    assert_close("s_white W=100", sw100, 0.146_779_926_762_206_94, 1e-12);
    assert_close("s_white W=100 App N", sw100, 0.146_780, 1e-6);
    // W=P anchors at exactly 1.0 (1.0^anything is 1.0).
    assert_eq!(
        reference::s_white(1000.0, 1000.0, 1.2).unwrap().to_bits(),
        1.0f64.to_bits()
    );
    // f32 vs the numpy f32 vector; App. N f32 row 0.26479718.
    assert_close(
        "s_white f32",
        f64::from(s_white(203.0, 1000.0, 1.2).unwrap()),
        0.264_797_180_891_037,
        3e-7,
    );
}

#[test]
fn hlg_reference_white_lands_on_working_one() {
    let sw = reference::s_white(203.0, 1000.0, 1.2).unwrap();
    // Exact-203 signal 0.749877365 decodes to s_white → working 1.0 (App. N
    // "exactly" is mathematical; float lands within 1e-9).
    let scene = reference::hlg_inverse_oetf(0.749_877_365).unwrap();
    let w = reference::scene_to_working([scene, 0.0, 0.0], sw).unwrap()[0];
    assert_close("HLG 203 working", w, 0.999_999_999_477_619_4, 1e-9);
    // Nominal 0.75 → working 1.000624531419893; App. N 1.000625.
    let scene75 = reference::hlg_inverse_oetf(0.75).unwrap();
    let w75 = reference::scene_to_working([scene75, 0.0, 0.0], sw).unwrap()[0];
    assert_close("HLG 0.75 working", w75, 1.000_624_531_419_893, 1e-9);
    assert_close("HLG 0.75 working App N", w75, 1.000_625, 1e-6);
    // And back: OETF(s_white) is the HLG signal of exactly 203 nits.
    assert_close(
        "OETF(s_white)",
        reference::hlg_oetf(sw).unwrap(),
        0.749_877_365_102_611_4,
        1e-12,
    );
    // f32 composition vs the numpy f32 vector; App. N f32 row 0.749877334.
    let sw32 = s_white(203.0, 1000.0, 1.2).unwrap();
    assert_close(
        "OETF(s_white) f32",
        f64::from(hlg_oetf(sw32).unwrap()),
        0.749_877_333_641_052_2,
        3e-7,
    );
    // Normalization roundtrips (normalize, then the trivial × s_white inverse
    // S3 inlines at render; no kernel entry point by budget design).
    let v = reference::scene_to_working([0.5, 0.25, 1.5], sw).unwrap();
    let back = [v[0] * sw, v[1] * sw, v[2] * sw];
    assert_close_3("normalize roundtrip", back, [0.5, 0.25, 1.5], 1e-15);
}

#[test]
fn forward_rendering_matches_vectors() {
    // Oracle forward vectors at P=1000, γ=1.2 (tol ≈ 7000 ulp: safe across
    // libms, lethal to wrong constants — KR+1e-4 shifts D by ~4e-4).
    let cases: [([f64; 3], [f64; 3]); 3] = [
        (
            [0.5, 0.25, 0.125],
            [
                395.142_864_255_787_4,
                197.571_432_127_893_7,
                98.785_716_063_946_85,
            ],
        ),
        ([1.0, 0.0, 0.0], [765.406_268_293_771_2, 0.0, 0.0]),
        ([0.5, -0.1, 0.0], [333.162_429_006_763_43, -100.0, 0.0]),
    ];
    for (scene, expected) in cases {
        let actual = reference::scene_to_display(scene, 1000.0, 1.2).unwrap();
        for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_close(&format!("forward[{i}]"), *a, *e, 1e-12 * e.abs().max(1.0));
        }
    }
    // Reference white renders to exactly 203 nits (working 1.0 anchor).
    let sw = reference::s_white(203.0, 1000.0, 1.2).unwrap();
    let white = reference::scene_to_display([sw, sw, sw], 1000.0, 1.2).unwrap();
    assert_close_3("white renders to 203", white, [203.0, 203.0, 203.0], 1e-9);
    // Achromatic form D = P·s^γ holds through the triplet path.
    for s in [0.01, 0.264_797_185_624_078_67, 1.0, 2.5] {
        let d = reference::scene_to_display([s, s, s], 1000.0, 1.2).unwrap();
        let expected = 1000.0 * s.powf(1.2);
        assert_close_3("achromatic", d, [expected, expected, expected], 1e-9);
    }
    // Kernel-resolved gamma (exactly 1.2 at P=1000) matches literal 1.2.
    let via_rule = reference::scene_to_display(
        [0.5, 0.25, 0.125],
        1000.0,
        reference::hlg_gamma(1000.0).unwrap(),
    )
    .unwrap();
    let literal = reference::scene_to_display([0.5, 0.25, 0.125], 1000.0, 1.2).unwrap();
    assert_bits_3("gamma rule vs literal", via_rule, literal);
}

#[test]
fn signed_rendering_never_feeds_powers_negatives() {
    // Negative channel passes linearly at peak scale; bit-exact -100.0.
    let d = reference::scene_to_display([0.5, -0.1, 0.0], 1000.0, 1.2).unwrap();
    assert_eq!(d[1].to_bits(), (-100.0f64).to_bits());
    assert_eq!(d[2].to_bits(), 0.0f64.to_bits());
    // All-negative triple: pure P·min(s,0), all exact in binary.
    let all = reference::scene_to_display([-0.5, -0.25, -0.125], 1000.0, 1.2).unwrap();
    assert_bits_3("all-negative forward", all, [-500.0, -250.0, -125.0]);
    // Zero-luma shortcut applies to the nonnegative part only: negatives
    // still pass through.
    let mixed = reference::scene_to_display([-0.5, 0.0, 0.0], 1000.0, 1.2).unwrap();
    assert_bits_3("signed zero-luma forward", mixed, [-500.0, 0.0, 0.0]);
    // Inverse mirrors: negatives pass at 1/P, bit-exact.
    let s = reference::display_to_scene([-500.0, 250.0, 0.0], 1000.0, 1.2).unwrap();
    assert_eq!(s[0].to_bits(), (-0.5f64).to_bits());
    let neg_only = reference::display_to_scene([-500.0, -250.0, -125.0], 1000.0, 1.2).unwrap();
    assert_bits_3("all-negative inverse", neg_only, [-0.5, -0.25, -0.125]);
}

#[test]
fn zero_luminance_is_exact_zero() {
    assert_bits_3(
        "forward zero f64",
        reference::scene_to_display([0.0, 0.0, 0.0], 1000.0, 1.2).unwrap(),
        [0.0, 0.0, 0.0],
    );
    assert_bits_3_f32(
        "forward zero f32",
        scene_to_display([0.0, 0.0, 0.0], 1000.0, 1.2).unwrap(),
        [0.0, 0.0, 0.0],
    );
    assert_bits_3(
        "inverse zero f64",
        reference::display_to_scene([0.0, 0.0, 0.0], 1000.0, 1.2).unwrap(),
        [0.0, 0.0, 0.0],
    );
    assert_bits_3_f32(
        "inverse zero f32",
        display_to_scene([0.0, 0.0, 0.0], 1000.0, 1.2).unwrap(),
        [0.0, 0.0, 0.0],
    );
}

#[test]
fn forward_inverse_identity() {
    let scenes = [
        [0.5, 0.25, 0.125],
        [1.0, 0.0, 0.0],
        [0.5, -0.1, 0.0],
        [1e-6, 1e-6, 1e-6],
        [3.776_475, 3.776_475, 3.776_475],
    ];
    for scene in scenes {
        let d = reference::scene_to_display(scene, 1000.0, 1.2).unwrap();
        let back = reference::display_to_scene(d, 1000.0, 1.2).unwrap();
        for (i, (b, e)) in back.iter().zip(scene.iter()).enumerate() {
            assert_close(&format!("identity[{i}]"), *b, *e, 1e-12 * e.abs().max(1e-9));
        }
        // And the other direction: inverse then forward.
        let there = reference::display_to_scene(d, 1000.0, 1.2).unwrap();
        let back2 = reference::scene_to_display(there, 1000.0, 1.2).unwrap();
        for (i, (b, e)) in back2.iter().zip(d.iter()).enumerate() {
            assert_close(
                &format!("identity2[{i}]"),
                *b,
                *e,
                1e-12 * e.abs().max(1e-9),
            );
        }
    }
    // Pure-negative roundtrip is bit-exact (linear passthrough both ways).
    let d = reference::scene_to_display([-0.5, -0.25, -0.125], 1000.0, 1.2).unwrap();
    let back = reference::display_to_scene(d, 1000.0, 1.2).unwrap();
    assert_bits_3("negative roundtrip", back, [-0.5, -0.25, -0.125]);
    // White extreme W=100: working of a 10k-nit input, oracle
    // 46.41588833612779; App. N 46.4159 (4dp).
    let sw100 = reference::s_white(100.0, 1000.0, 1.2).unwrap();
    let s = reference::display_to_scene([10_000.0, 10_000.0, 10_000.0], 1000.0, 1.2).unwrap();
    let w = reference::scene_to_working(s, sw100).unwrap()[0];
    assert_close("W=100 working of 10k", w, 46.415_888_336_127_79, 1e-9);
    assert_close("W=100 working of 10k App N", w, 46.415_9, 1e-4);
}

#[test]
fn rendering_f32_agrees_with_f64() {
    assert_close(
        "s_white f32~f64",
        f64::from(s_white(203.0, 1000.0, 1.2).unwrap()),
        reference::s_white(203.0, 1000.0, 1.2).unwrap(),
        2e-7,
    );
    for scene in [
        [0.5, 0.25, 0.125],
        [1.0, 0.0, 0.0],
        [0.5, -0.1, 0.0],
        [1e-6, 1e-6, 1e-6],
    ] {
        let disp32 = f64_3(
            scene_to_display(
                [as_f32(scene[0]), as_f32(scene[1]), as_f32(scene[2])],
                1000.0,
                1.2,
            )
            .unwrap(),
        );
        let disp64 = reference::scene_to_display(scene, 1000.0, 1.2).unwrap();
        for (i, (a, e)) in disp32.iter().zip(disp64.iter()).enumerate() {
            assert_close(&format!("fwd32[{i}]"), *a, *e, 1e-5 * e.abs().max(1.0));
        }
        let scene32 = f64_3(
            display_to_scene(
                [as_f32(disp64[0]), as_f32(disp64[1]), as_f32(disp64[2])],
                1000.0,
                1.2,
            )
            .unwrap(),
        );
        let scene64 = reference::display_to_scene(disp64, 1000.0, 1.2).unwrap();
        for (i, (a, e)) in scene32.iter().zip(scene64.iter()).enumerate() {
            assert_close(&format!("inv32[{i}]"), *a, *e, 1e-5 * e.abs().max(1.0));
        }
    }
}

#[test]
fn rendering_refusals() {
    // Non-positive white/peak/gamma/s_white refuse.
    assert!(matches!(
        s_white(0.0, 1000.0, 1.2),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        s_white(203.0, -1.0, 1.2),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        s_white(203.0, 1000.0, 0.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        scene_to_display([0.5, 0.5, 0.5], 1000.0, -0.5),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        scene_to_working([0.5, 0.5, 0.5], 0.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    // Any non-finite component/arg refuses.
    assert!(matches!(
        scene_to_display([f32::NAN, 0.0, 0.0], 1000.0, 1.2),
        Err(Cc8KernelError::NonFiniteInput { .. })
    ));
    assert!(matches!(
        display_to_scene([100.0, f32::INFINITY, 100.0], 1000.0, 1.2),
        Err(Cc8KernelError::NonFiniteInput { .. })
    ));
    assert!(matches!(
        s_white(f32::NAN, 1000.0, 1.2),
        Err(Cc8KernelError::NonFiniteInput { .. })
    ));
    // Overflow refuses rather than returning infinity (f32 and f64).
    assert!(matches!(
        scene_to_display([3e38, 0.0, 0.0], 1e4, 1.7),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    assert!(matches!(
        reference::scene_to_display([1e308, 0.0, 0.0], 1e4, 1.7),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    assert!(matches!(
        s_white(3e38, 1e-37, 1.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
}

#[test]
fn kernel_nits_domain_refusals() {
    // G1: nit params (P, W, Cs, Ct, SDR peak) ∈ [1, 100000] (R35). Edges
    // accepted, just-outside refused, both precisions.
    for peak in [1.0f32, 100_000.0] {
        assert!(hlg_gamma(peak).is_ok());
    }
    for peak in [0.999f32, 100_001.0] {
        assert!(matches!(
            hlg_gamma(peak),
            Err(Cc8KernelError::OutOfDomain { .. })
        ));
    }
    assert!(reference::hlg_gamma(1.0).is_ok());
    assert!(matches!(
        reference::hlg_gamma(100_001.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    // Peak/white/ceiling edges; sub-domain and over-domain refuse.
    assert!(s_white(1.0, 100_000.0, 1.0).is_ok());
    assert!(s_white(100_000.0, 1.0, 3.0).is_ok());
    assert!(matches!(
        s_white(0.999, 1000.0, 1.2),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::s_white(100.0, 100_001.0, 1.2),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(eetf_to_target(50.0, 1.0, 1.0).is_ok());
    assert!(matches!(
        eetf_to_target(50.0, 0.999, 1.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::eetf_to_target(50.0, 1000.0, 100_001.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(gamut_compress([50.0; 3], CompressDest::Sdr { target_peak: 1.0 }).is_ok());
    assert!(matches!(
        gamut_compress(
            [50.0; 3],
            CompressDest::Sdr {
                target_peak: 200_000.0
            }
        ),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::gamut_compress([50.0; 3], CompressDest::Sdr { target_peak: 0.5 }),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    // `scene_to_working`'s scale is scene-linear, not nits: small stays legal.
    assert!(scene_to_working([1.0, 1.0, 1.0], 0.5).is_ok());
}

#[test]
fn kernel_gamma_domain_refusals() {
    // G1: γ ∈ [1, 3] on every γ consumer (R35). Edges accepted,
    // just-outside and R1's subnormal-γ exponents refused, both precisions.
    for g in [1.0f32, 3.0] {
        assert!(s_white(100.0, 1000.0, g).is_ok());
        assert!(scene_to_display([0.5, 0.5, 0.5], 1000.0, g).is_ok());
        assert!(display_to_scene([100.0, 100.0, 100.0], 1000.0, g).is_ok());
        assert!(hlg_output([100.0, 100.0, 100.0], 1000.0, g).is_ok());
    }
    for g in [0.999f32, 3.001] {
        assert!(matches!(
            s_white(100.0, 1000.0, g),
            Err(Cc8KernelError::OutOfDomain { .. })
        ));
        assert!(matches!(
            scene_to_display([0.5, 0.5, 0.5], 1000.0, g),
            Err(Cc8KernelError::OutOfDomain { .. })
        ));
        assert!(matches!(
            display_to_scene([100.0, 100.0, 100.0], 1000.0, g),
            Err(Cc8KernelError::OutOfDomain { .. })
        ));
        assert!(matches!(
            hlg_output([100.0, 100.0, 100.0], 1000.0, g),
            Err(Cc8KernelError::OutOfDomain { .. })
        ));
        assert!(matches!(
            gamut_compress(
                [100.0, 100.0, 100.0],
                CompressDest::Hlg {
                    peak: 1000.0,
                    gamma: g
                }
            ),
            Err(Cc8KernelError::OutOfDomain { .. })
        ));
    }
    // R1's subnormal-γ exponents refuse (by domain now); the f32 peak sums
    // to 0.99999994, so an in-domain peak isolates the γ refusal as well.
    assert!(matches!(
        display_to_scene(
            [1.0, 1.0, 1.0],
            0.2627f32 + 0.6780 + 0.0593,
            f32::from_bits(1)
        ),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        display_to_scene([1.0, 1.0, 1.0], 1.0, f32::from_bits(1)),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::display_to_scene([1.0, 1.0, 1.0], 1.0, f64::from_bits(1)),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::scene_to_display([0.5, 0.5, 0.5], 1000.0, 0.5),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
}

#[test]
fn rendering_refuses_hidden_intermediate_overflow() {
    // R1 B3 under G1: Y/P and U overflows needed out-of-domain parameters,
    // so they now refuse by domain (f32 and f64). F2 exception: the
    // compressor's per-channel ratio may evaluate to +∞ (never NaN); it is
    // non-binding for t ≤ 1 and skipped, not refused (see the two
    // `compressor_skips_*`/`f64_true_overflowing_*` tests).
    assert!(matches!(
        display_to_scene([3e38, 3e38, 3e38], 1e-37, 1.2),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::display_to_scene([1e308, 1e308, 1e308], 1e-307, 1.2),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    // In-domain gain overflow still refuses with NonFiniteResult.
    assert!(matches!(
        scene_to_display([3e38, 0.0, 0.0], 1e4, 1.7),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    // The out-of-domain γ refusal propagates through hlg_output (f32, f64).
    assert!(matches!(
        hlg_output([1.0, 0.0, 0.0], 1000.0, 0.001),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::hlg_output([1.0, 0.0, 0.0], 1000.0, 0.001),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    // Infinite U needed γ < 1: refuses by domain before U (f32 and f64).
    assert!(matches!(
        gamut_compress(
            [1.0, 0.0, 0.0],
            CompressDest::Hlg {
                peak: 1000.0,
                gamma: 0.001
            }
        ),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::gamut_compress(
            [1.0, 0.0, 0.0],
            CompressDest::Hlg {
                peak: 1000.0,
                gamma: 0.001
            }
        ),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
}

#[test]
fn inverse_denominator_underflow_refuses_in_domain() {
    // R2 S1/N06 under G1: R2's P=1e30/1e200 + γ=0.5 denom overflows are
    // out of domain. In domain (finite Y ≥ 0, a = (γ−1)/γ ∈ [0, 2/3]) the
    // denominator D = P·(Y/P)^a ≤ max(P, (P·Y²)^(1/3)) — the P branch for
    // Y < P — < 4.9e27 f32 / 1.5e207 f64, so the finite-denom guard was
    // deleted as provably unreachable. Reachable in-domain is underflow to
    // a zero denominator, which still refuses NonFiniteResult, both
    // precisions.
    assert!(matches!(
        display_to_scene([1.0, 1.0, 1.0], 1e30, 0.5),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::display_to_scene([1.0, 1.0, 1.0], 1e200, 0.5),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        display_to_scene([1e-40, 0.0, 0.0], 1e5, 1.2),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    assert!(matches!(
        reference::display_to_scene([1e-319, 0.0, 0.0], 1e5, 1.2),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
}

#[test]
fn compressor_skips_inactive_infinite_ratio() {
    // R1 S1: SDR P=100, [1e-38,0,0] overflows the upper ratio to +inf, but
    // t ≤ +inf constrains nothing — skipped, pinned finite-correct, both
    // precisions (never min-ed, never refused).
    let x = 1e-38f32;
    let y = 0.2126f32 * x;
    assert!(((100.0 - y) / (x - y)).is_infinite());
    let got = gamut_compress([x, 0.0, 0.0], CompressDest::Sdr { target_peak: 100.0 }).unwrap();
    assert!(!got.y_clamped);
    assert!(!got.compressed);
    assert_close(
        "ratio-skip red",
        f64::from(got.value[0]),
        f64::from(x),
        1e-44,
    );
    assert_close("ratio-skip green", f64::from(got.value[1]), 0.0, 1e-44);
    assert_close("ratio-skip blue", f64::from(got.value[2]), 0.0, 1e-44);
    let got64 =
        reference::gamut_compress([1e-38, 0.0, 0.0], CompressDest::Sdr { target_peak: 100.0 })
            .unwrap();
    assert!(!got64.y_clamped);
    assert!(!got64.compressed);
    assert_close("ratio-skip red f64", got64.value[0], 1e-38, 1e-50);
}

#[test]
fn f64_true_overflowing_ratio_stays_correct() {
    // Verify nit (shipped from the verifier's probe): the f64 test above
    // uses 1e-38, whose ratio is FINITE in f64. 1e-310 (subnormal) makes
    // (100 − Y)/(x − Y) overflow to +∞ in f64, exercising the actual skip
    // branch: finite-correct, unflagged, never refused.
    let x = 1e-310f64;
    let y = 0.2126 * x;
    assert!(((100.0 - y) / (x - y)).is_infinite());
    let got =
        reference::gamut_compress([x, 0.0, 0.0], CompressDest::Sdr { target_peak: 100.0 }).unwrap();
    assert!(!got.compressed && !got.y_clamped);
    assert_close("f64 +inf skip red", got.value[0], x, 1e-320);
    assert_close("f64 +inf skip green", got.value[1], 0.0, 1e-320);
    assert_close("f64 +inf skip blue", got.value[2], 0.0, 1e-320);
}

// ---------------------------------------------------------------------------
// C3: EETF to target (§6, R14).
// ---------------------------------------------------------------------------

#[test]
fn eetf_anchors_narrow_span() {
    // (L, oracle f64, App. N) at Cs=1000, Ct=100.
    for (l, expected, appn) in [
        (10.0, 9.999_999_999_999_952, 10.0),
        (100.0, 69.454_403_035_658_91, 69.454_403),
        (203.0, 88.243_640_538_647_8, 88.243_641),
    ] {
        let out = reference::eetf_to_target(l, 1000.0, 100.0).unwrap();
        assert!(!out.clipped, "L={l} must not flag");
        assert_close("eetf narrow", out.value, expected, 1e-9);
        assert_close("eetf narrow App N", out.value, appn, 1e-6);
    }
    // L == Cs maps exactly to Ct, unflagged (nit-domain endpoint rule).
    let end = reference::eetf_to_target(1000.0, 1000.0, 100.0).unwrap();
    assert!(!end.clipped);
    assert_eq!(end.value.to_bits(), 100.0f64.to_bits());
    assert_close("eetf narrow App N endpoint", end.value, 100.0, 1e-6);
    // Exact 0→0, bit-identical, both precisions.
    assert_eq!(
        reference::eetf_to_target(0.0, 1000.0, 100.0)
            .unwrap()
            .value
            .to_bits(),
        0.0f64.to_bits()
    );
    assert_eq!(
        eetf_to_target(0.0, 1000.0, 100.0).unwrap().value.to_bits(),
        0.0f32.to_bits()
    );
    // Past Cs: clip to Ct exactly, flag set.
    let clip = reference::eetf_to_target(4000.0, 1000.0, 100.0).unwrap();
    assert!(clip.clipped, "L=4000 past Cs=1000 must flag");
    assert_eq!(clip.value.to_bits(), 100.0f64.to_bits());
    let clip32 = eetf_to_target(4000.0, 1000.0, 100.0).unwrap();
    assert!(clip32.clipped);
    assert_eq!(clip32.value.to_bits(), 100.0f32.to_bits());
}

#[test]
fn eetf_anchors_wide_span_and_identity() {
    for (l, expected, appn) in [
        (203.0, 63.306_209_002_170_73, 63.306_209),
        (1000.0, 91.070_396_016_548_29, 91.070_396),
        (4000.0, 99.437_355_791_646_78, 99.437_356),
    ] {
        let out = reference::eetf_to_target(l, 10_000.0, 100.0).unwrap();
        assert!(!out.clipped, "L={l} must not flag");
        assert_close("eetf wide", out.value, expected, 1e-9);
        assert_close("eetf wide App N", out.value, appn, 1e-6);
    }
    // L == Cs maps exactly to Ct, unflagged (nit-domain endpoint rule).
    let end = reference::eetf_to_target(10_000.0, 10_000.0, 100.0).unwrap();
    assert!(!end.clipped);
    assert_eq!(end.value.to_bits(), 100.0f64.to_bits());
    assert_close("eetf wide App N endpoint", end.value, 100.0, 1e-6);
    // Ct ≥ Cs returns L bit-identically: Ct > Cs and Ct == Cs.
    let id = reference::eetf_to_target(203.0, 100.0, 1000.0).unwrap();
    assert!(!id.clipped);
    assert_eq!(id.value.to_bits(), 203.0f64.to_bits());
    let id_eq = eetf_to_target(500.0, 1000.0, 1000.0).unwrap();
    assert!(!id_eq.clipped);
    assert_eq!(id_eq.value.to_bits(), 500.0f32.to_bits());
}

#[test]
fn eetf_domain_edges() {
    // Negatives pass unchanged in nits, never flagged, both precisions.
    for l in [-5.0, -0.5, -100.0] {
        let out = reference::eetf_to_target(l, 1000.0, 100.0).unwrap();
        assert!(!out.clipped);
        assert_eq!(out.value.to_bits(), l.to_bits(), "L={l}");
        let out32 = eetf_to_target(as_f32(l), 1000.0, 100.0).unwrap();
        assert!(!out32.clipped);
        assert_eq!(out32.value.to_bits(), as_f32(l).to_bits());
    }
    // Clip edge: just past Cs flags, just inside does not.
    assert!(
        reference::eetf_to_target(1000.0001, 1000.0, 100.0)
            .unwrap()
            .clipped
    );
    assert!(
        !reference::eetf_to_target(999.9999, 1000.0, 100.0)
            .unwrap()
            .clipped
    );
    // Below-knee near-identity (PQ roundtrip only): oracle 4.9999999999997575.
    assert_close(
        "eetf below knee",
        reference::eetf_to_target(5.0, 1000.0, 100.0).unwrap().value,
        4.999_999_999_999_757_5,
        1e-9,
    );
    for l in [1.0, 5.0, 10.0] {
        let v = reference::eetf_to_target(l, 1000.0, 100.0).unwrap().value;
        assert_close("eetf below-knee identity", v, l, 1e-9);
    }
    // Monotone through the knee region.
    let a = reference::eetf_to_target(100.0, 1000.0, 100.0)
        .unwrap()
        .value;
    let b = reference::eetf_to_target(203.0, 1000.0, 100.0)
        .unwrap()
        .value;
    let c = reference::eetf_to_target(1000.0, 1000.0, 100.0)
        .unwrap()
        .value;
    assert!(a < b && b < c, "EETF must be monotone: {a} < {b} < {c}");
}

#[test]
fn eetf_f32_agrees_with_f64() {
    for (l, cs) in [
        (0.0, 1000.0),
        (10.0, 1000.0),
        (100.0, 1000.0),
        (203.0, 1000.0),
        (1000.0, 1000.0),
        (203.0, 10_000.0),
        (4000.0, 10_000.0),
        (10_000.0, 10_000.0),
        (-5.0, 1000.0),
    ] {
        let v32 = eetf_to_target(as_f32(l), as_f32(cs), 100.0).unwrap();
        let v64 = reference::eetf_to_target(l, cs, 100.0).unwrap();
        assert_eq!(v32.clipped, v64.clipped, "flag at L={l}");
        assert_close(
            "eetf f32~f64",
            f64::from(v32.value),
            v64.value,
            1e-4 * v64.value.abs().max(1.0),
        );
    }
}

#[test]
fn eetf_refusals() {
    assert!(matches!(
        eetf_to_target(100.0, 0.0, 100.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        eetf_to_target(100.0, 1000.0, -100.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::eetf_to_target(100.0, -1.0, 100.0),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(matches!(
            eetf_to_target(bad, 1000.0, 100.0),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
        assert!(matches!(
            eetf_to_target(100.0, bad, 100.0),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
        assert!(matches!(
            eetf_to_target(100.0, 1000.0, bad),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
    }
    for bad in [f64::NAN, f64::INFINITY] {
        assert!(matches!(
            reference::eetf_to_target(bad, 1000.0, 100.0),
            Err(Cc8KernelError::NonFiniteInput { .. })
        ));
    }
}

#[test]
fn eetf_nit_domain_clip_regressions() {
    // R1 B1: f32 PQ(7606) == PQ(7605), but 10000 > 7606 must clip + flag.
    let out = eetf_to_target(10_000.0, 7606.0, 7605.0).unwrap();
    assert!(out.clipped, "L=10000 past Cs=7606 must flag");
    assert_eq!(out.value.to_bits(), 7605.0f32.to_bits());
    let out64 = reference::eetf_to_target(10_000.0, 7606.0, 7605.0).unwrap();
    assert!(out64.clipped);
    assert_eq!(out64.value.to_bits(), 7605.0f64.to_bits());
    // R1 B1 / R2 B1: next-representable past Cs flags, both precisions.
    let just_past = 1000f32.next_up();
    debug_assert!(just_past > 1000.0);
    let edge = eetf_to_target(just_past, 1000.0, 100.0).unwrap();
    assert!(edge.clipped, "L=1000.next_up past Cs must flag");
    assert_eq!(edge.value.to_bits(), 100.0f32.to_bits());
    let edge64 = reference::eetf_to_target(1000f64.next_up(), 1000.0, 100.0).unwrap();
    assert!(edge64.clipped);
    assert_eq!(edge64.value.to_bits(), 100.0f64.to_bits());
    // R2 B1: 400.0001 > 400 flags with value Ct.
    let b1 = eetf_to_target(400.0001, 400.0, 100.0).unwrap();
    assert!(b1.clipped, "L=400.0001 past Cs=400 must flag");
    assert_eq!(b1.value.to_bits(), 100.0f32.to_bits());
    let b1_64 = reference::eetf_to_target(400.0001, 400.0, 100.0).unwrap();
    assert!(b1_64.clipped);
    assert_eq!(b1_64.value.to_bits(), 100.0f64.to_bits());
    // R2 B2: distinct near-equal ceilings must not disable clipping.
    let ct = f32::from_bits(400f32.to_bits() - 4); // 399.9998779296875
    let b2 = eetf_to_target(10_000.0, 400.0, ct).unwrap();
    assert!(b2.clipped, "L=10000 past Cs=400 must flag");
    assert_eq!(b2.value.to_bits(), ct.to_bits());
    let ct64 = 399.999_877_929_687_5f64;
    let b2_64 = reference::eetf_to_target(10_000.0, 400.0, ct64).unwrap();
    assert!(b2_64.clipped);
    assert_eq!(b2_64.value.to_bits(), ct64.to_bits());
    // R2 S2, strict: L == Cs with rounded-PQ ceilings maps exactly to Ct.
    let end = eetf_to_target(400.0, 400.0, ct).unwrap();
    assert!(!end.clipped);
    assert_eq!(end.value.to_bits(), ct.to_bits());
    let end64 = reference::eetf_to_target(400.0, 400.0, ct64).unwrap();
    assert!(!end64.clipped);
    assert_eq!(end64.value.to_bits(), ct64.to_bits());
}

#[test]
fn eetf_adjacent_integer_ceiling_scan() {
    // Exhaustive Cs = 401..10000, Ct = Cs−1: L = Cs+1 flags to Ct,
    // L = Cs maps exactly to Ct unflagged. Both precisions.
    for cs in 401..=10_000 {
        let src = f64::from(cs);
        let tgt = f64::from(cs - 1);
        let over = eetf_to_target(as_f32(src) + 1.0, as_f32(src), as_f32(tgt)).unwrap();
        assert!(over.clipped, "Cs={cs}: L=Cs+1 must flag");
        assert_eq!(over.value.to_bits(), as_f32(tgt).to_bits(), "Cs={cs}");
        let at = eetf_to_target(as_f32(src), as_f32(src), as_f32(tgt)).unwrap();
        assert!(!at.clipped, "Cs={cs}: L=Cs must not flag");
        assert_eq!(at.value.to_bits(), as_f32(tgt).to_bits(), "Cs={cs}");
        let over64 = reference::eetf_to_target(src + 1.0, src, tgt).unwrap();
        assert!(over64.clipped, "f64 Cs={cs}: L=Cs+1 must flag");
        assert_eq!(over64.value.to_bits(), tgt.to_bits(), "f64 Cs={cs}");
        let at64 = reference::eetf_to_target(src, src, tgt).unwrap();
        assert!(!at64.clipped, "f64 Cs={cs}: L=Cs must not flag");
        assert_eq!(at64.value.to_bits(), tgt.to_bits(), "f64 Cs={cs}");
    }
}

#[test]
fn eetf_degenerate_knee_saturates_in_domain() {
    // R2 S2/N14 under G1: f64 PQ plateaus below Cs=400 (next_down rounds
    // equal), so m == 1 with Ct < Cs and the knee degenerates (dk == 0)
    // with x == 1 ≥ knee: saturate exactly to Ct, unflagged — with L ≠ Ct
    // so the Ct→L mutant is killed. R2's subnormal case now refuses.
    let cs = 400.0f64;
    let ct = cs.next_down();
    let l = ct.next_down();
    assert_eq!(
        reference::pq_oetf(ct).unwrap().to_bits(),
        reference::pq_oetf(cs).unwrap().to_bits()
    );
    assert_eq!(
        reference::pq_oetf(l).unwrap().to_bits(),
        reference::pq_oetf(cs).unwrap().to_bits()
    );
    let out = reference::eetf_to_target(l, cs, ct).unwrap();
    assert!(!out.clipped);
    assert_eq!(out.value.to_bits(), ct.to_bits());
    assert!(matches!(
        reference::eetf_to_target(f64::from_bits(1), f64::from_bits(4), f64::from_bits(2)),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
}

#[test]
fn eetf_interior_requires_widening_f32() {
    // R2 S2/N15: interior values go through the widened f64 core — the
    // narrowed mutant errs 0.0057 here and 1.1035 at the near endpoint.
    let got = eetf_to_target(3000.0, 4000.0, 1000.0).unwrap();
    assert!(!got.clipped);
    assert_close(
        "interior widened",
        f64::from(got.value),
        998.205_063_342_773_4,
        1e-4,
    );
    let l_near = 9764f32.next_down(); // 9763.9990234375
    let near = eetf_to_target(l_near, 9764.0, 9763.0).unwrap();
    assert!(!near.clipped);
    assert_eq!(near.value.to_bits(), 9763.0f32.to_bits());
}

#[test]
fn exact_identity_small_positive_ceilings() {
    // R2 S1/R13 under G1: the smallest in-domain ceilings Cs = Ct = 1 take
    // the exact-identity rule (bit-identical, unflagged; still kills R13),
    // both precisions; sub-domain ceilings refuse OutOfDomain.
    let out = eetf_to_target(203.0, 1.0, 1.0).unwrap();
    assert!(!out.clipped);
    assert_eq!(out.value.to_bits(), 203.0f32.to_bits());
    let out64 = reference::eetf_to_target(203.0, 1.0, 1.0).unwrap();
    assert!(!out64.clipped);
    assert_eq!(out64.value.to_bits(), 203.0f64.to_bits());
    assert!(matches!(
        eetf_to_target(203.0, f32::from_bits(1), f32::from_bits(1)),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::eetf_to_target(203.0, f64::from_bits(1), f64::from_bits(1)),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
}

// ---------------------------------------------------------------------------
// C4: gamut compressor + HLG delivery kernel (§6, R30).
// ---------------------------------------------------------------------------

#[test]
fn hlg_primaries_u_bounds_pre_post_fit() {
    // (primary, oracle U, App. N U, oracle pre-fit signal, App. N pre).
    for (prim, u, u_appn, pre, pre_appn) in [
        (
            [1000.0, 0.0, 0.0],
            800.282_546_629_577_4,
            800.283,
            1.040_707_984_183_471,
            1.040_708,
        ),
        (
            [0.0, 1000.0, 0.0],
            937.284_889_648_623_7,
            937.285,
            1.011_854_952_694_234,
            1.011_855,
        ),
        (
            [0.0, 0.0, 1000.0],
            624.466_457_290_609_7,
            624.466,
            1.085_829_229_257_491,
            1.085_829,
        ),
    ] {
        let hlg = CompressDest::Hlg {
            peak: 1000.0,
            gamma: 1.2,
        };
        let fit = reference::gamut_compress(prim, hlg).unwrap();
        assert!(!fit.y_clamped, "primary Y is in range");
        assert!(fit.compressed, "primary must report compression");
        // Fitted max channel lands ON U by construction: this asserts U.
        let max_c = fit.value[0].max(fit.value[1]).max(fit.value[2]);
        assert_close("U bound", max_c, u, 1e-9);
        assert_close("U bound App N", max_c, u_appn, 1e-3);
        for c in fit.value {
            assert!(
                c >= 0.0 && c <= u * (1.0 + 1e-12),
                "in-volume: {c} vs U={u}"
            );
        }
        // Pre-fit signal (unfitted inverse OOTF + OETF) exceeds 1: the fit
        // is load-bearing, not decorative.
        let s_unfit = reference::display_to_scene(prim, 1000.0, 1.2).unwrap();
        let pre_sig = reference::hlg_oetf(s_unfit[0].max(s_unfit[1]).max(s_unfit[2])).unwrap();
        assert_close("pre-fit signal", pre_sig, pre, 1e-9);
        assert_close("pre-fit App N", pre_sig, pre_appn, 1e-6);
        assert!(pre_sig > 1.0, "pre-fit must exceed 1: {pre_sig}");
        // Post-fit: all three land on OETF(1.0) = 0.999999996 (App. N).
        let out = reference::hlg_output(prim, 1000.0, 1.2).unwrap();
        let post = out.signal[0].max(out.signal[1]).max(out.signal[2]);
        assert_close("post-fit", post, 0.999_999_995_536_568_6, 1e-9);
        assert_close("post-fit App N", post, 0.999_999_996, 1e-8);
        assert!(post < 1.0, "post-OETF clamp must be residue-only: {post}");
        assert!(out.fit.compressed);
    }
    // Oracle fitted triples, pinned componentwise (R primary shown; G/B by
    // symmetry of the same code path plus their U/max assertions above).
    let hlg = CompressDest::Hlg {
        peak: 1000.0,
        gamma: 1.2,
    };
    let fit_r = reference::gamut_compress([1000.0, 0.0, 0.0], hlg).unwrap();
    assert_close_3(
        "R fitted",
        fit_r.value,
        [
            800.282_546_629_577_4,
            71.159_331_344_649_42,
            71.159_331_344_649_42,
        ],
        1e-9,
    );
}

#[test]
fn sdr_cube_compress() {
    let sdr = CompressDest::Sdr { target_peak: 100.0 };
    // Chromatic vector: oracle [100.0, 5.76135205339822, 41.100845033373886].
    let v = reference::gamut_compress([150.0, -10.0, 50.0], sdr).unwrap();
    assert!(!v.y_clamped);
    assert!(v.compressed);
    assert_close_3(
        "SDR fitted",
        v.value,
        [100.0, 5.761_352_053_398_22, 41.100_845_033_373_886],
        1e-9,
    );
    // Grey over-peak: Y resolves to Ct, then t = 0 → Ct exactly, both flags.
    let over = reference::gamut_compress([200.0, 200.0, 200.0], sdr).unwrap();
    assert!(over.y_clamped && over.compressed);
    assert_bits_3("grey over-peak", over.value, [100.0, 100.0, 100.0]);
    // Negative luma resolves to black exactly (Y flag, no compression).
    let neg = reference::gamut_compress([-10.0, -10.0, -10.0], sdr).unwrap();
    assert!(neg.y_clamped && !neg.compressed);
    assert_bits_3("negative luma", neg.value, [0.0, 0.0, 0.0]);
    // Neutral ramp and black pass through, no flags.
    let ramp = reference::gamut_compress([50.0, 50.0, 50.0], sdr).unwrap();
    assert!(!ramp.y_clamped && !ramp.compressed);
    assert_close_3("neutral ramp", ramp.value, [50.0, 50.0, 50.0], 1e-9);
    let black = reference::gamut_compress([0.0, 0.0, 0.0], sdr).unwrap();
    assert!(!black.y_clamped && !black.compressed);
    assert_bits_3("black", black.value, [0.0, 0.0, 0.0]);
}

#[test]
fn hlg_output_neutrals_and_volume() {
    // Achromatic D → OETF((D/P)^(1/γ)) exactly (fit is identity on neutrals).
    for (d, expected) in [
        (100.0, 0.629_620_321_882_465_1),
        (203.0, 0.749_877_365_102_611_4),
        (500.0, 0.893_272_209_305_090_1),
        (1000.0, 0.999_999_995_536_568_6),
    ] {
        let out = reference::hlg_output([d, d, d], 1000.0, 1.2).unwrap();
        assert!(!out.fit.y_clamped && !out.fit.compressed, "D={d}");
        assert_close_3(
            "neutral out",
            out.signal,
            [expected, expected, expected],
            1e-9,
        );
    }
    // Reference white through the FULL output path: 0.749877365 (App. N).
    let w = reference::hlg_output([203.0, 203.0, 203.0], 1000.0, 1.2).unwrap();
    assert_close("white out", w.signal[0], 0.749_877_365_102_611_4, 1e-12);
    // Super-white grey resolves through the fit (both flags), lands on
    // OETF(1.0) — the post-OETF clamp never engages (residue only).
    let over = reference::hlg_output([2000.0, 2000.0, 2000.0], 1000.0, 1.2).unwrap();
    assert!(over.fit.y_clamped && over.fit.compressed);
    assert_close_3(
        "super-white out",
        over.signal,
        [
            0.999_999_995_536_568_6,
            0.999_999_995_536_568_6,
            0.999_999_995_536_568_6,
        ],
        1e-9,
    );
    // Extreme HDR red stays in-volume and ≤ 1.0 past the clamp.
    let x = reference::hlg_output([10_000.0, 0.0, 0.0], 1000.0, 1.2).unwrap();
    for s in x.signal {
        assert!((0.0..=1.0).contains(&s), "clamped signal: {s}");
    }
}

#[test]
fn compress_f32_agrees_with_f64() {
    for prim in [[1000.0, 0.0, 0.0], [0.0, 1000.0, 0.0], [0.0, 0.0, 1000.0]] {
        let hlg32 = CompressDest::Hlg {
            peak: 1000.0,
            gamma: 1.2,
        };
        let hlg64 = CompressDest::Hlg {
            peak: 1000.0,
            gamma: 1.2,
        };
        let f32v =
            gamut_compress([as_f32(prim[0]), as_f32(prim[1]), as_f32(prim[2])], hlg32).unwrap();
        let f64v = reference::gamut_compress(prim, hlg64).unwrap();
        assert_eq!(f32v.y_clamped, f64v.y_clamped);
        assert_eq!(f32v.compressed, f64v.compressed);
        let a = f64_3(f32v.value);
        for (i, (x, e)) in a.iter().zip(f64v.value.iter()).enumerate() {
            assert_close(&format!("fit32[{i}]"), *x, *e, 1e-5 * e.abs().max(1.0));
        }
        let s32 = hlg_output(
            [as_f32(prim[0]), as_f32(prim[1]), as_f32(prim[2])],
            1000.0,
            1.2,
        )
        .unwrap();
        let s64 = reference::hlg_output(prim, 1000.0, 1.2).unwrap();
        assert_close_3("sig32", f64_3(s32.signal), s64.signal, 2e-6);
    }
    let sdr32 = CompressDest::Sdr { target_peak: 100.0 };
    let sdr64 = CompressDest::Sdr { target_peak: 100.0 };
    let f32v = gamut_compress([150.0, -10.0, 50.0], sdr32).unwrap();
    let f64v = reference::gamut_compress([150.0, -10.0, 50.0], sdr64).unwrap();
    assert_close_3("sdr32", f64_3(f32v.value), f64v.value, 1e-4);
}

#[test]
fn compress_refusals() {
    assert!(matches!(
        gamut_compress([50.0, 50.0, 50.0], CompressDest::Sdr { target_peak: 0.0 }),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        gamut_compress(
            [50.0, 50.0, 50.0],
            CompressDest::Hlg {
                peak: 1000.0,
                gamma: -1.0
            }
        ),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        reference::gamut_compress(
            [50.0, 50.0, 50.0],
            CompressDest::Hlg {
                peak: -2.0,
                gamma: 1.2
            }
        ),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
    assert!(matches!(
        gamut_compress(
            [f32::NAN, 0.0, 0.0],
            CompressDest::Sdr { target_peak: 100.0 }
        ),
        Err(Cc8KernelError::NonFiniteInput { .. })
    ));
    assert!(matches!(
        hlg_output([100.0, 100.0, 100.0], f32::INFINITY, 1.2),
        Err(Cc8KernelError::NonFiniteInput { .. })
    ));
    assert!(matches!(
        hlg_output([100.0, 100.0, 100.0], 0.0, 1.2),
        Err(Cc8KernelError::OutOfDomain { .. })
    ));
}

#[test]
fn post_oetf_clamp_catches_negative_roundoff() {
    // R2 S3/R18: P=400, RGB=[−80,4,400] leaves raw red HLG negative from
    // float residue; the post-OETF clamp catches it. Both precisions.
    let g = hlg_gamma(400.0).unwrap();
    let rgb = [-80.0, 4.0, 400.0];
    let fit = gamut_compress(
        rgb,
        CompressDest::Hlg {
            peak: 400.0,
            gamma: g,
        },
    )
    .unwrap();
    let s = display_to_scene(fit.value, 400.0, g).unwrap();
    let raw = hlg_oetf(s[0]).unwrap();
    assert!(raw < 0.0, "raw red must be negative residue: {raw}");
    let out = hlg_output(rgb, 400.0, g).unwrap();
    assert_eq!(out.signal[0].to_bits(), 0.0f32.to_bits());
    let g64 = reference::hlg_gamma(400.0).unwrap();
    let rgb64 = [-80.0, 4.0, 400.0];
    let fit64 = reference::gamut_compress(
        rgb64,
        CompressDest::Hlg {
            peak: 400.0,
            gamma: g64,
        },
    )
    .unwrap();
    let s64 = reference::display_to_scene(fit64.value, 400.0, g64).unwrap();
    let raw64 = reference::hlg_oetf(s64[0]).unwrap();
    assert!(raw64 < 0.0, "f64 raw red must be negative residue: {raw64}");
    let out64 = reference::hlg_output(rgb64, 400.0, g64).unwrap();
    assert_eq!(out64.signal[0].to_bits(), 0.0f64.to_bits());
}

// ---------------------------------------------------------------------------
// C5: primaries matrices (R13), precision budgets (R35), wrong-transform
// controls (§17, R15).
// ---------------------------------------------------------------------------

// --- test-only matrix derivation (mirrors /tmp/cc8_oracle.py op order) ---

fn det3(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn inv3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = det3(m);
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det,
        ],
    ]
}

fn mul3(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut c = [[0.0; 3]; 3];
    for r in 0..3 {
        for cc in 0..3 {
            c[r][cc] = a[r][0] * b[0][cc] + a[r][1] * b[1][cc] + a[r][2] * b[2][cc];
        }
    }
    c
}

/// RGB→XYZ of one primary set + D65 (textbook derivation, oracle order).
// Single-letter bindings mirror the matrix-notation derivation.
#[allow(clippy::many_single_char_names)]
fn derive_rgb_to_xyz(p: [[f64; 2]; 3]) -> [[f64; 3]; 3] {
    let w = [0.3127, 0.3290];
    let mut m = [[0.0; 3]; 3];
    for (c, prim) in p.iter().enumerate() {
        m[0][c] = prim[0] / prim[1];
        m[1][c] = 1.0;
        m[2][c] = (1.0 - prim[0] - prim[1]) / prim[1];
    }
    let wv = [w[0] / w[1], 1.0, (1.0 - w[0] - w[1]) / w[1]];
    let i = inv3(m);
    let s = [
        i[0][0] * wv[0] + i[0][1] * wv[1] + i[0][2] * wv[2],
        i[1][0] * wv[0] + i[1][1] * wv[1] + i[1][2] * wv[2],
        i[2][0] * wv[0] + i[2][1] * wv[1] + i[2][2] * wv[2],
    ];
    let mut xyz = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            xyz[r][c] = m[r][c] * s[c];
        }
    }
    xyz
}

fn derive_matrices() -> ([[f64; 3]; 3], [[f64; 3]; 3]) {
    let p709 = [[0.64, 0.33], [0.3, 0.6], [0.15, 0.06]];
    let p2020 = [[0.708, 0.292], [0.17, 0.797], [0.131, 0.046]];
    let x709 = derive_rgb_to_xyz(p709);
    let x2020 = derive_rgb_to_xyz(p2020);
    (mul3(inv3(x709), x2020), mul3(inv3(x2020), x709))
}

/// The archived 2026-08 attempt's pinned f32 (third independent source).
const ARCHIVE_2020_TO_709: [[f32; 3]; 3] = [
    [1.660_491, -0.587_641_1, -0.072_849_86],
    [-0.124_550_48, 1.132_899_9, -0.008_349_422],
    [-0.018_150_764, -0.100_578_9, 1.118_729_7],
];
const ARCHIVE_709_TO_2020: [[f32; 3]; 3] = [
    [0.627_403_9, 0.329_283_03, 0.043_313_067],
    [0.069_097_29, 0.919_540_4, 0.011_362_315],
    [0.016_391_44, 0.088_013_306, 0.895_595_25],
];

#[test]
fn matrix_transcription_pins_derivation() {
    let (d20_09, d09_20) = derive_matrices();
    for r in 0..3 {
        for c in 0..3 {
            // Pinned f64 == independent derivation, bit-identical (same IEEE
            // ops in the same order; catches any transcription typo).
            assert_eq!(
                BT2020_TO_BT709_F64[r][c].to_bits(),
                d20_09[r][c].to_bits(),
                "2020->709[{r}][{c}]"
            );
            assert_eq!(
                BT709_TO_BT2020_F64[r][c].to_bits(),
                d09_20[r][c].to_bits(),
                "709->2020[{r}][{c}]"
            );
            // Pinned f32 == correctly-rounded narrowing of pinned f64.
            assert_eq!(
                BT2020_TO_BT709[r][c].to_bits(),
                as_f32(BT2020_TO_BT709_F64[r][c]).to_bits()
            );
            assert_eq!(
                BT709_TO_BT2020[r][c].to_bits(),
                as_f32(BT709_TO_BT2020_F64[r][c]).to_bits()
            );
            // Pinned f32 == archive's pinned f32, bit-identical (reuse).
            assert_eq!(
                BT2020_TO_BT709[r][c].to_bits(),
                ARCHIVE_2020_TO_709[r][c].to_bits()
            );
            assert_eq!(
                BT709_TO_BT2020[r][c].to_bits(),
                ARCHIVE_709_TO_2020[r][c].to_bits()
            );
        }
    }
}

#[test]
fn matrix_roundtrip_preserves_negatives() {
    // Saturated 2020 primaries go out-of-709-triangle (negatives, never
    // clamped) and roundtrip back; saturated 709 primaries likewise.
    for prim in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
        let to709 = reference::apply_matrix(BT2020_TO_BT709_F64, prim).unwrap();
        assert!(
            to709.iter().any(|c| *c < 0.0),
            "2020 primary must leave the 709 triangle: {to709:?}"
        );
        let back = reference::apply_matrix(BT709_TO_BT2020_F64, to709).unwrap();
        for (i, (b, e)) in back.iter().zip(prim.iter()).enumerate() {
            assert_close(&format!("2020rt[{i}]"), *b, *e, 1e-9 * e.abs().max(1e-6));
        }
        let to2020 = reference::apply_matrix(BT709_TO_BT2020_F64, prim).unwrap();
        assert!(
            to2020.iter().all(|c| *c >= 0.0),
            "709 primary stays positive"
        );
        let back2 = reference::apply_matrix(BT2020_TO_BT709_F64, to2020).unwrap();
        for (i, (b, e)) in back2.iter().zip(prim.iter()).enumerate() {
            assert_close(&format!("709rt[{i}]"), *b, *e, 1e-9 * e.abs().max(1e-6));
        }
        // f32 production roundtrip, f32-appropriate bound.
        let p32 = [as_f32(prim[0]), as_f32(prim[1]), as_f32(prim[2])];
        let t32 = apply_matrix(BT2020_TO_BT709, p32).unwrap();
        let b32 = apply_matrix(BT709_TO_BT2020, t32).unwrap();
        for (i, (b, e)) in b32.iter().zip(p32.iter()).enumerate() {
            assert_close(
                &format!("rt32[{i}]"),
                f64::from(*b),
                f64::from(*e),
                // Floor 1e-7: zero components rebuild from O(0.1)
                // intermediates through two matrices (~4 ulp residue).
                1e-6 * f64::from(*e).abs().max(1e-1),
            );
        }
    }
    // Near-black and HDR magnitudes (incl. the W=100 10k-nit working level).
    for v in [[1e-6, 2e-6, 1e-7], [46.415_9, 10.0, 1.0], [0.7, 0.3, 0.5]] {
        let there = reference::apply_matrix(BT2020_TO_BT709_F64, v).unwrap();
        let back = reference::apply_matrix(BT709_TO_BT2020_F64, there).unwrap();
        for (i, (b, e)) in back.iter().zip(v.iter()).enumerate() {
            assert_close(&format!("mag[{i}]"), *b, *e, 1e-9 * e.abs().max(1e-6));
        }
    }
}

#[test]
fn matrix_f32_agrees_with_f64_and_refuses() {
    for v in [[1.0, 0.0, 0.0], [0.7, 0.3, 0.5], [46.415_9, 10.0, 1.0]] {
        let a32 = f64_3(
            apply_matrix(BT2020_TO_BT709, [as_f32(v[0]), as_f32(v[1]), as_f32(v[2])]).unwrap(),
        );
        let a64 = reference::apply_matrix(BT2020_TO_BT709_F64, v).unwrap();
        for (i, (a, e)) in a32.iter().zip(a64.iter()).enumerate() {
            assert_close(&format!("mx32[{i}]"), *a, *e, 1e-6 * e.abs().max(1e-3));
        }
    }
    assert!(matches!(
        apply_matrix(BT2020_TO_BT709, [f32::NAN, 0.0, 0.0]),
        Err(Cc8KernelError::NonFiniteInput { .. })
    ));
    assert!(matches!(
        apply_matrix(BT2020_TO_BT709, [3e38, 0.0, 0.0]),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
    assert!(matches!(
        reference::apply_matrix(BT2020_TO_BT709_F64, [1.5e308, 0.0, 0.0]),
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
}

#[test]
fn rendering_10k_peak_and_white_400() {
    // White extreme W=400: oracle 0.4659972203002852.
    assert_close(
        "s_white W=400",
        reference::s_white(400.0, 1000.0, 1.2).unwrap(),
        0.465_997_220_300_285_2,
        1e-12,
    );
    // 10 000-nit peak: kernel-resolved gamma, forward vector + identity.
    let g = reference::hlg_gamma(10_000.0).unwrap();
    let d = reference::scene_to_display([0.5, 0.25, 0.125], 10_000.0, g).unwrap();
    assert_close_3(
        "P=10k forward",
        d,
        [
            2_187.918_139_940_348,
            1_093.959_069_970_174,
            546.979_534_985_087,
        ],
        1e-9,
    );
    let back = reference::display_to_scene(d, 10_000.0, g).unwrap();
    assert_close_3("P=10k identity", back, [0.5, 0.25, 0.125], 1e-12);
    assert_close(
        "s_white P=10k",
        reference::s_white(203.0, 10_000.0, g).unwrap(),
        0.101_335_978_092_349_29,
        1e-12,
    );
}

// --- precision budgets PB1–PB4 (§14, R35) over f16 storage ---

/// f16 ULP of a magnitude: 2^(e−10) on normals, 2^-24 on subnormals
/// (below 2^-14), sign-insensitive. Binade-exact by construction.
#[allow(clippy::cast_possible_truncation)] // log2 range fits i32 by construction
fn f16_ulp_of(x: f64) -> f64 {
    debug_assert!(x != 0.0);
    let a = x.abs();
    if a < 2f64.powi(-14) {
        2f64.powi(-24)
    } else {
        2f64.powi((a.log2().floor() as i32) - 10)
    }
}

/// Triplet magnitude for CE4: ULP/relative limits use the max |channel|.
fn max_abs_3(w: [f64; 3]) -> f64 {
    w[0].abs().max(w[1].abs()).max(w[2].abs())
}

/// Production-shaped f32→f16 store (round-to-nearest via `half`).
fn f16_store(x: f32) -> f32 {
    f16::from_f32(x).to_f32()
}

/// Truncating f16 store (test-only wrong implementation for PB1).
fn truncating_f16_store_sub(x: f32) -> f32 {
    let bits = f16::from_f32(x).to_bits();
    // Round toward zero instead of to-nearest: strip a set low mantissa bit.
    let truncated = if x > 0.0 && bits & 1 == 1 {
        bits - 1
    } else {
        bits
    };
    f16::from_bits(truncated).to_f32()
}

#[test]
fn pb1_storage_boundary() {
    // App. N ULP rows, exact powers of two; subnormal/negative pins.
    assert_eq!(f16_ulp_of(1.0).to_bits(), 0.000_976_562_5f64.to_bits());
    assert_eq!(f16_ulp_of(3.776_5).to_bits(), 0.001_953_125f64.to_bits());
    assert_eq!(
        f16_ulp_of(2f64.powi(-20)).to_bits(),
        2f64.powi(-24).to_bits()
    );
    assert_eq!(
        f16_ulp_of(2f64.powi(-14)).to_bits(),
        2f64.powi(-24).to_bits()
    );
    assert_eq!(f16_ulp_of(-1.0).to_bits(), f16_ulp_of(1.0).to_bits());
    // Each f32→f16 store ≤ 0.5 ULP (1e-6 slack absorbs diff rounding; a
    // truncating store at 1.0 ULP still fails). Per-channel (CE4 keeps
    // per-value ULP for scalar stores); subnormals/negatives included.
    for x in [
        2f64.powi(-20),
        2f64.powi(-15),
        2f64.powi(-14),
        0.000_976_562_5,
        0.01,
        0.1,
        0.5,
        1.0,
        2.0,
        3.776_475,
        10.0,
        46.415_9,
        65_504.0,
        -0.01,
        -0.5,
        -3.776_475,
        -46.415_9,
    ] {
        let stored = f16_store(as_f32(x));
        let err = (f64::from(stored) - x).abs();
        assert!(pb1_store_ok(err, x), "PB1 store of {x}: err={err}");
    }
    // The truncating substitute violates 0.5 ULP (control: the bound bites).
    let bad = truncating_f16_store_sub(1.0006);
    let bad_err = (f64::from(bad) - 1.0006).abs();
    assert!(
        !pb1_store_ok(bad_err, 1.0006),
        "truncation must violate PB1: err={bad_err}"
    );
    // Stage-pair round trips through an f16 store, ≤ 2 ULP in the starting
    // domain (pipeline direction: decode→encode).
    let sw = reference::s_white(203.0, 1000.0, 1.2).unwrap();
    // (i) HLG signal → scene → f16 → signal.
    let scene = reference::hlg_inverse_oetf(0.75).unwrap();
    let sig2 = reference::hlg_oetf(f64::from(f16_store(as_f32(scene)))).unwrap();
    assert!(
        pb1_pair_ok((sig2 - 0.75).abs(), 0.75),
        "PB1b signal: {sig2}"
    );
    // (ii) scene → working → f16 → scene.
    for s in [0.01, 0.26, 1.0] {
        let w = reference::scene_to_working([s, 0.0, 0.0], sw).unwrap()[0];
        let s2 = f64::from(f16_store(as_f32(w))) * sw;
        assert!(pb1_pair_ok((s2 - s).abs(), s), "PB1b scene {s}: {s2}");
    }
    // (iii) 2020 → 709 → f16 → 2020 through the production f32 path.
    // CE4: triplet ULP is measured against the max |channel|.
    for v in [
        [0.7, 0.3, 0.5],
        [1.0, 0.0, 0.0],
        [1.0, 2f64.powi(-10), 2f64.powi(-10)],
        [3.776_475, 2f64.powi(-10), 0.02],
    ] {
        let v32 = [as_f32(v[0]), as_f32(v[1]), as_f32(v[2])];
        let t = apply_matrix(BT2020_TO_BT709, v32).unwrap();
        let t16 = [f16_store(t[0]), f16_store(t[1]), f16_store(t[2])];
        let b = apply_matrix(BT709_TO_BT2020, t16).unwrap();
        for (i, (bb, e)) in b.iter().zip(v.iter()).enumerate() {
            let err = (f64::from(*bb) - e).abs();
            assert!(
                pb1_pair_ok(err, max_abs_3(v)),
                "PB1b matrix {v:?}[{i}]: err={err}"
            );
        }
    }
    // Erratum visibility: the saturated small channel FAILS the old
    // per-channel reading (R2 measured 8.57 ULP of 2^-10).
    let sv = [as_f32(1.0), as_f32(2f64.powi(-10)), as_f32(2f64.powi(-10))];
    let st = apply_matrix(BT2020_TO_BT709, sv).unwrap();
    let sb = apply_matrix(
        BT709_TO_BT2020,
        [f16_store(st[0]), f16_store(st[1]), f16_store(st[2])],
    )
    .unwrap();
    let blue_err = (f64::from(sb[2]) - 2f64.powi(-10)).abs();
    assert!(
        !pb1_pair_ok(blue_err, 2f64.powi(-10)),
        "saturated blue must fail per-channel 2 ULP: {blue_err}"
    );
    assert!(pb1_pair_ok(blue_err, 1.0), "…and pass CE4's ULP(max)");
    // (iv) display → scene → f16 → display at reference white.
    let sc = reference::display_to_scene([203.0, 203.0, 203.0], 1000.0, 1.2).unwrap();
    let sc16 = [
        f64::from(f16_store(as_f32(sc[0]))),
        f64::from(f16_store(as_f32(sc[1]))),
        f64::from(f16_store(as_f32(sc[2]))),
    ];
    let d2 = reference::scene_to_display(sc16, 1000.0, 1.2).unwrap();
    assert!(
        pb1_pair_ok((d2[0] - 203.0).abs(), 203.0),
        "PB1b display: {}",
        d2[0]
    );
}

#[test]
fn pb2_working_domain_repeated_chain() {
    // Repeated render/inverse cycles through f16 stores (f32 production)
    // with a grade-like ×1.05 gain between stores, so the stored values
    // change every round. CE4: 4 ULP(max|w|), relative ‖err‖∞/‖w‖∞ ≤ 0.3%.
    for white in [100.0, 203.0, 400.0] {
        let sw = s_white(white, 1000.0, 1.2).unwrap();
        for w0 in [
            [0.9, 0.45, 1.1],
            [3.776_475, 0.02, 2.0],
            [46.415_9, 20.0, 5.0],
        ] {
            let mut w = w0;
            for _ in 0..3 {
                w = [f16_store(w[0]), f16_store(w[1]), f16_store(w[2])];
                let s = [w[0] * sw, w[1] * sw, w[2] * sw];
                let d = scene_to_display(s, 1000.0, 1.2).unwrap();
                let s2 = display_to_scene(d, 1000.0, 1.2).unwrap();
                w = [s2[0] / sw * 1.05, s2[1] / sw * 1.05, s2[2] / sw * 1.05];
            }
            w = [f16_store(w[0]), f16_store(w[1]), f16_store(w[2])];
            let grown = 1.05f64.powi(3);
            let e = [
                f64::from(w0[0]) * grown,
                f64::from(w0[1]) * grown,
                f64::from(w0[2]) * grown,
            ];
            let w64 = [f64::from(w[0]), f64::from(w[1]), f64::from(w[2])];
            let err_max = (w64[0] - e[0])
                .abs()
                .max((w64[1] - e[1]).abs())
                .max((w64[2] - e[2]).abs());
            assert!(
                ce4_rel_ok(err_max, max_abs_3(e)),
                "PB2 grade rel W={white}: {}",
                err_max / max_abs_3(e)
            );
            for (i, (a, ee)) in w64.iter().zip(e.iter()).enumerate() {
                let err = (a - ee).abs();
                assert!(
                    pb2_abs_ok(err, max_abs_3(e)),
                    "PB2 grade abs W={white}[{i}]: err={err}"
                );
            }
        }
    }
    // R2 B3: three alternating matrix pairs through f16 stores (f32).
    for start in [
        [1.0, 2f64.powi(-10), 2f64.powi(-10)],
        [3.776_475, 2f64.powi(-10), 0.02],
    ] {
        let mut v = [as_f32(start[0]), as_f32(start[1]), as_f32(start[2])];
        for _ in 0..3 {
            v = apply_matrix(BT2020_TO_BT709, v).unwrap();
            v = [f16_store(v[0]), f16_store(v[1]), f16_store(v[2])];
            v = apply_matrix(BT709_TO_BT2020, v).unwrap();
            v = [f16_store(v[0]), f16_store(v[1]), f16_store(v[2])];
        }
        let v64 = [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])];
        let err_max = (v64[0] - start[0])
            .abs()
            .max((v64[1] - start[1]).abs())
            .max((v64[2] - start[2]).abs());
        assert!(
            ce4_rel_ok(err_max, max_abs_3(start)),
            "PB2 matrix rel {start:?}: {}",
            err_max / max_abs_3(start)
        );
        for (i, (a, e)) in v64.iter().zip(start.iter()).enumerate() {
            let err = (a - e).abs();
            assert!(
                pb2_abs_ok(err, max_abs_3(start)),
                "PB2 matrix abs {start:?}[{i}]: err={err}"
            );
        }
    }
    // Erratum visibility: the saturated chain FAILS the old per-channel
    // reading (R2 measured 9 ULP / 0.88% on blue).
    let mut old = [1.0f32, 2f32.powi(-10), 2f32.powi(-10)];
    for _ in 0..3 {
        old = apply_matrix(BT2020_TO_BT709, old).unwrap();
        old = [f16_store(old[0]), f16_store(old[1]), f16_store(old[2])];
        old = apply_matrix(BT709_TO_BT2020, old).unwrap();
        old = [f16_store(old[0]), f16_store(old[1]), f16_store(old[2])];
    }
    let blue_err = (f64::from(old[2]) - 2f64.powi(-10)).abs();
    assert!(
        !ce4_rel_ok(blue_err, 2f64.powi(-10)),
        "saturated chain must fail per-channel 0.3%: {blue_err}"
    );
    // Domain edge w = 2^-10 exactly (scalar path: per-value ULP).
    let sw = s_white(203.0, 1000.0, 1.2).unwrap();
    let e = 2f32.powi(-10);
    let w = f16_store(e);
    let s = [w * sw, 0.0, 0.0];
    let d = scene_to_display(s, 1000.0, 1.2).unwrap();
    let s2 = display_to_scene(d, 1000.0, 1.2).unwrap();
    let w2 = f16_store(s2[0] / sw);
    let edge_err = (f64::from(w2) - f64::from(e)).abs();
    assert!(
        pb2_abs_ok(edge_err, f64::from(e)),
        "PB2 edge: err={edge_err}"
    );
}

#[test]
fn pb3_display_absolute() {
    // Production f32 chain through one f16 working store vs exact display:
    // white ±10% ≤ 1.0 nit, peak ≤ max(2.0, 0.2% × P) (CE3, CE5), sub-1 ≤ 0.05.
    for white in [100.0, 203.0, 400.0] {
        for peak in [400.0, 1000.0, 2000.0, 4000.0, 10_000.0] {
            let g = hlg_gamma(as_f32(peak)).unwrap();
            let sw = s_white(as_f32(white), as_f32(peak), g).unwrap();
            let g64 = reference::hlg_gamma(peak).unwrap();
            for (i, d) in [0.1, 0.5, 0.9, white * 0.9, white, white * 1.1, peak]
                .into_iter()
                .enumerate()
            {
                let s = (d / peak).powf(1.0 / g64);
                let sw64 = reference::s_white(white, peak, g64).unwrap();
                let stored = f16_store(as_f32(s / sw64));
                let back =
                    scene_to_display([stored * sw, stored * sw, stored * sw], as_f32(peak), g)
                        .unwrap();
                let err = (f64::from(back[0]) - d).abs();
                // Index 6 is the peak anchor however the values collide.
                let tol = if d < 1.0 {
                    pb3_sub1_tol()
                } else if i == 6 {
                    pb3_peak_tol(peak)
                } else {
                    pb3_white_tol()
                };
                assert!(
                    err <= tol,
                    "PB3 W={white} P={peak} D={d}: err={err} tol={tol}"
                );
            }
        }
    }
    // R1 B2, erratum visibility: W=100/P=10000 neutral (R1: 10003.46 nits)
    // passes CE5 (20 nits), still passes CE3's 10, and FAILS the pre-CE3 2.0.
    let g = hlg_gamma(10_000.0).unwrap();
    let sw = s_white(100.0, 10_000.0, g).unwrap();
    let stored = f16_store(1.0 / sw);
    let got = scene_to_display([stored * sw, stored * sw, stored * sw], 10_000.0, g).unwrap()[0];
    let err = (f64::from(got) - 10_000.0).abs();
    assert!(
        err <= pb3_peak_tol(10_000.0),
        "W=100/P=10000 must pass CE5 (20 nits): err={err}"
    );
    assert!(
        err <= 10.0,
        "one-store case still passes CE3 (10): err={err}"
    );
    assert!(
        err > 2.0,
        "W=100/P=10000 must fail the old 2.0-nit bound: err={err}"
    );
    assert_close("W=100/P=10000 value", f64::from(got), 10_003.462, 0.1);
    // R1 B2 / CE5: composed matrix pairs at P=7004/W=100, saturated green,
    // through CE9's conservative foreign-space storage model (PB1–PB3 only).
    // One pair errs 10.45 nits, three pairs 11.01: both pass CE5's 14.008
    // and FAIL the old 0.1% (7.004).
    for pairs in [1, 3] {
        let got = foreign_space_store([0.0, 7004.0, 0.0], 100.0, 7004.0, pairs);
        let err = (f64::from(got[1]) - 7004.0).abs();
        assert!(
            err <= pb3_peak_tol(7004.0),
            "P=7004 pairs={pairs} must pass CE5: err={err}"
        );
        assert!(
            err > 0.001 * 7004.0,
            "P=7004 pairs={pairs} must fail the old 0.1%: err={err}"
        );
    }
    // App. N ULP rows via kernel finite differences (central = slope).
    let sw = reference::s_white(203.0, 1000.0, 1.2).unwrap();
    let u1 = 2f64.powi(-10);
    // Central difference over a ±(sw·u) scene step = nits per working-ULP.
    let slope_at = |s: f64, h: f64| {
        let up = reference::scene_to_display([s + h, s + h, s + h], 1000.0, 1.2).unwrap()[0];
        let dn = reference::scene_to_display([s - h, s - h, s - h], 1000.0, 1.2).unwrap()[0];
        (up - dn) / 2.0
    };
    let ds = slope_at(sw, sw * u1);
    assert_close("nits/ULP white", ds, 0.237_890_625_000_000_02, 1e-6);
    assert_close("nits/ULP white App N", ds, 0.237_891, 1e-6);
    let wp = 1.0 / sw;
    let up = 2f64.powi(-9);
    let dp = slope_at(wp * sw, sw * up);
    assert_close("nits/ULP peak", dp, 0.620_618_403_806_434_4, 1e-6);
    assert_close("nits/ULP peak App N", dp, 0.620_618, 1e-6);
    // Forward differences (one actual ULP step, curvature included).
    let f1 = reference::scene_to_display([(1.0 + u1) * sw; 3], 1000.0, 1.2).unwrap()[0]
        - reference::scene_to_display([sw; 3], 1000.0, 1.2).unwrap()[0];
    assert_close("nits/ULP fwd white", f1, 0.237_913_850_459_165_13, 1e-9);
}

/// CE3/CE5 peak bound in nits: `max(2.0, 0.002·P)`. Shared predicate —
/// PB3/PB4 and the bound-controls below all call this (G4).
fn pb3_peak_tol(peak: f64) -> f64 {
    2.0f64.max(0.002 * peak)
}

/// PB3 white ±10% bound in nits. Shared predicate (G4).
fn pb3_white_tol() -> f64 {
    1.0
}

/// PB3 sub-1-nit bound in nits. Shared predicate (G4).
fn pb3_sub1_tol() -> f64 {
    0.05
}

/// PB1 store acceptance: one f32→f16 store errs ≤ 0.5 ULP of the value
/// (1e-6 slack absorbs the f64 difference's rounding; a truncating store
/// at up to 1 ULP still fails). Fixed limit (verify S1). Shared (G4).
fn pb1_store_ok(err: f64, x: f64) -> bool {
    err <= 0.5 * f16_ulp_of(x) * (1.0 + 1e-6)
}

/// PB1 stage-pair acceptance: ≤ 2 ULP of `max_ch` (CE4: the triplet's max
/// |channel|; scalar callers pass the value). The limit is fixed inside
/// the helper — no caller-chosen multiplier (verify S1). Shared (G4).
fn pb1_pair_ok(err: f64, max_ch: f64) -> bool {
    err <= 2.0 * f16_ulp_of(max_ch)
}

/// PB2 absolute acceptance: ≤ 4 ULP of `max_ch` (CE4 triplet max; scalar
/// callers pass the value). Fixed limit (verify S1). Shared (G4).
fn pb2_abs_ok(err: f64, max_ch: f64) -> bool {
    err <= 4.0 * f16_ulp_of(max_ch)
}

/// CE4 relative bound: ‖err‖∞/‖w‖∞ ≤ 0.003. Shared predicate (G4).
fn ce4_rel_ok(err_max: f64, wmax: f64) -> bool {
    err_max / wmax <= 0.003
}

/// PB4 10-bit major-code bound. Shared predicate (G4).
fn pb4_code_tol() -> f64 {
    2.0
}

/// PB4 10-bit mean-code bound over all channels: design value 0.5 (CE7;
/// R2's measured 0.08 no longer holds once minors count). Shared (G4).
fn pb4_mean_tol() -> f64 {
    0.5
}

/// PB4 8-bit code bound: design value 1 (R2's measured 0 fails 1-code
/// composed channels under CE7). Shared predicate (G4).
fn pb4_8bit_tol() -> f64 {
    1.0
}

#[test]
fn bound_predicates_reject_just_outside() {
    // G4/R2 S3: every PB acceptance predicate is pinned at just-inside /
    // just-outside values through the SAME helpers the PB tests call, so
    // loosening or tightening any bound fails a test.
    // CE3/CE5 peak: limits at P=1000/4000/10000 are 2/8/20.
    for (peak, limit) in [(1000.0, 2.0), (4000.0, 8.0), (10_000.0, 20.0)] {
        let tol = pb3_peak_tol(peak);
        assert_close("peak pin", tol, limit, 1e-9);
        assert!(limit * 0.9995 <= tol, "P={peak} just inside");
        assert!(limit * 1.0005 > tol, "P={peak} just outside");
    }
    // White / sub-1.
    assert_eq!(pb3_white_tol().to_bits(), 1.0f64.to_bits());
    assert!(0.9995 <= pb3_white_tol());
    assert!(1.0005 > pb3_white_tol());
    assert_close("sub1 pin", pb3_sub1_tol(), 0.05, 1e-12);
    assert!(0.0499 <= pb3_sub1_tol());
    assert!(0.0501 > pb3_sub1_tol());
    // PB1 store 0.5 ULP / pair 2 ULP and PB2 4 ULP, through the SAME fixed
    // helpers the PB tests call (verify S1): inclusive edge, just inside,
    // just outside, at max 1.0 (ULP 2^-10).
    let u = 2f64.powi(-10);
    assert!(pb1_store_ok(0.5 * u, 1.0));
    assert!(!pb1_store_ok(0.501 * u, 1.0));
    assert!(pb1_pair_ok(2.0 * u, 1.0));
    assert!(pb1_pair_ok(1.999 * u, 1.0));
    assert!(!pb1_pair_ok(2.001 * u, 1.0));
    assert!(pb2_abs_ok(4.0 * u, 1.0));
    assert!(pb2_abs_ok(3.999 * u, 1.0));
    assert!(!pb2_abs_ok(4.001 * u, 1.0));
    // PB1's pair limit is not PB2's: 3 ULP splits them.
    assert!(!pb1_pair_ok(3.0 * u, 1.0) && pb2_abs_ok(3.0 * u, 1.0));
    // CE4: the asymmetric triplet's ULP comes from its max, not its min.
    assert_eq!(
        max_abs_3([1.0, 2f64.powi(-10), 0.02]).to_bits(),
        1.0f64.to_bits()
    );
    assert!(pb1_pair_ok(1.5 * u, max_abs_3([1.0, u, 0.02])));
    assert!(!pb1_pair_ok(1.5 * u, u));
    // CE4 relative + crossed decoys (R2 B03L/B04L).
    assert!(ce4_rel_ok(0.002_999, 1.0));
    assert!(!ce4_rel_ok(0.003_001, 1.0));
    let b03 = 4.001 * 2f64.powi(-10); // high-in-binade 1.9: ULP rejects…
    assert!(!pb2_abs_ok(b03, 1.9));
    assert!(ce4_rel_ok(b03, 1.9)); // …while relative accepts.
    let b04 = 0.003_001; // magnitude 1.0: relative rejects…
    assert!(!ce4_rel_ok(b04, 1.0));
    assert!(pb2_abs_ok(b04, 1.0)); // …while ULP accepts.
    // PB4 codes.
    assert_eq!(pb4_code_tol().to_bits(), 2.0f64.to_bits());
    assert!(2.0 <= pb4_code_tol());
    assert!(3.0 > pb4_code_tol());
    assert_eq!(pb4_mean_tol().to_bits(), 0.5f64.to_bits());
    assert!(0.499 <= pb4_mean_tol());
    assert!(0.501 > pb4_mean_tol());
    assert_eq!(pb4_8bit_tol().to_bits(), 1.0f64.to_bits());
    assert!(1.0 <= pb4_8bit_tol());
    assert!(2.0 > pb4_8bit_tol());
    // CE7 display prong: 0.2% of the triplet max (0.8 nits at 400), `≤`
    // inclusive, magnitude of the max, never of the channel itself.
    assert!(ce7_display_ok(0.799_6, 400.0));
    assert!(ce7_display_ok(0.8, 400.0)); // inclusive edge.
    assert!(!ce7_display_ok(0.800_4, 400.0));
    assert!(ce7_display_ok(0.799_6, -400.0));
    // Relative to the max (400), not the channel (16): same error, two verdicts.
    assert!(ce7_display_ok(0.08, 400.0));
    assert!(!ce7_display_ok(0.08, 16.0));
    // CE7 channel acceptance: codes OR display, both prongs bite.
    assert!(pb4_10bit_channel_ok(2.0, 1e9, 400.0)); // codes alone.
    assert!(pb4_10bit_channel_ok(4.0, 0.799_6, 400.0)); // display alone.
    assert!(!pb4_10bit_channel_ok(3.0, 0.800_4, 400.0)); // neither.
    assert!(pb4_8bit_channel_ok(1.0, 1e9, 100.0));
    assert!(pb4_8bit_channel_ok(2.0, 0.199_9, 100.0));
    assert!(!pb4_8bit_channel_ok(2.0, 0.200_1, 100.0));
}

/// Anchor display → f64 scene → f32 2020 working values (unstored), with
/// the f32 HLG γ and scene white the render needs.
fn working_from_display(d: [f64; 3], white: f64, peak: f64) -> ([f32; 3], f32, f32) {
    let g = hlg_gamma(as_f32(peak)).unwrap();
    let sw = s_white(as_f32(white), as_f32(peak), g).unwrap();
    let g64 = reference::hlg_gamma(peak).unwrap();
    let sw64 = reference::s_white(white, peak, g64).unwrap();
    let s = reference::display_to_scene(d, peak, g64).unwrap();
    (s.map(|x| as_f32(x / sw64)), g, sw)
}

/// CE9 delivery model (storage model D): the source is stored once in f16
/// in 2020; each of `pairs` nodes converts 2020→709→2020 in f32 in-pass and
/// stores only in 2020 f16; then render. 709 values never cross a store.
fn matrix_chain_display(d: [f64; 3], white: f64, peak: f64, pairs: usize) -> [f32; 3] {
    let (w0, g, sw) = working_from_display(d, white, peak);
    let mut w = w0.map(f16_store);
    for _ in 0..pairs {
        let t = apply_matrix(BT2020_TO_BT709, w).unwrap();
        w = apply_matrix(BT709_TO_BT2020, t).unwrap().map(f16_store);
    }
    scene_to_display(w.map(|x| x * sw), as_f32(peak), g).unwrap()
}

/// CE9 CONTROL — the CE4/CE5 conservative storage model (PB1–PB3 only,
/// never a delivery path): f32 source, then `pairs` × (2020→709→f16 in 709
/// →2020→f16). The 709 store quantizes large opposite-sign components into
/// the 2020 minors; it must FAIL PB4 CE7 on verify2's witnesses.
fn foreign_space_store(d: [f64; 3], white: f64, peak: f64, pairs: usize) -> [f32; 3] {
    let (mut w, g, sw) = working_from_display(d, white, peak);
    for _ in 0..pairs {
        w = apply_matrix(BT2020_TO_BT709, w).unwrap().map(f16_store);
        w = apply_matrix(BT709_TO_BT2020, w).unwrap().map(f16_store);
    }
    scene_to_display(w.map(|x| x * sw), as_f32(peak), g).unwrap()
}

/// PB4 working chain: anchor display → f64 scene → f32 working → three
/// render/inverse pairs with f16 stores → final store → f32 display.
fn pb4_chain_display(d: [f64; 3], white: f64, peak: f64) -> [f32; 3] {
    let g = hlg_gamma(as_f32(peak)).unwrap();
    let sw = s_white(as_f32(white), as_f32(peak), g).unwrap();
    let g64 = reference::hlg_gamma(peak).unwrap();
    let sw64 = reference::s_white(white, peak, g64).unwrap();
    let scene = reference::display_to_scene(d, peak, g64).unwrap();
    let mut w = [
        as_f32(scene[0] / sw64),
        as_f32(scene[1] / sw64),
        as_f32(scene[2] / sw64),
    ];
    for _ in 0..3 {
        w = [f16_store(w[0]), f16_store(w[1]), f16_store(w[2])];
        let s = [w[0] * sw, w[1] * sw, w[2] * sw];
        let rd = scene_to_display(s, as_f32(peak), g).unwrap();
        let rs = display_to_scene(rd, as_f32(peak), g).unwrap();
        w = [rs[0] / sw, rs[1] / sw, rs[2] / sw];
    }
    w = [f16_store(w[0]), f16_store(w[1]), f16_store(w[2])];
    scene_to_display([w[0] * sw, w[1] * sw, w[2] * sw], as_f32(peak), g).unwrap()
}

/// CE7 display prong: display error within 0.2% of the triplet's
/// max-channel reference display value (CE5's composed budget, relative to
/// the channel that sets the storage quantum). Replaces CE6's 1% minor
/// cutoff. Shared predicate (G4).
fn ce7_display_ok(disp_err: f64, max_ch: f64) -> bool {
    disp_err <= 0.002 * max_ch.abs()
}

/// CE7 10-bit PB4 channel acceptance: ≤ 2 codes, or the display prong.
fn pb4_10bit_channel_ok(codes: f64, disp_err: f64, max_ch: f64) -> bool {
    codes <= pb4_code_tol() || ce7_display_ok(disp_err, max_ch)
}

/// CE7 8-bit SDR PB4 channel acceptance: 8-bit code bound, or the display
/// prong (SDR nits vs the SDR triplet max).
fn pb4_8bit_channel_ok(codes: f64, disp_err: f64, max_ch: f64) -> bool {
    codes <= pb4_8bit_tol() || ce7_display_ok(disp_err, max_ch)
}

/// PB4 code accumulators over every channel (CE7: the mean covers all).
/// `max10`/`max8` are raw code errors; `by_display10`/`by_display8` count
/// channels past the code bound that pass through the CE7 display prong.
#[derive(Default)]
struct Pb4Acc {
    max10: f64,
    sum10: f64,
    n10: u32,
    max8: f64,
    by_display10: u32,
    by_display8: u32,
}

/// One delivered channel vs the f64 reference: code error, display error
/// (nits of that leg) and the CE7 verdict.
#[derive(Clone, Copy, Debug)]
struct Ce7Ch {
    codes: f64,
    disp_err: f64,
    ok: bool,
}

/// PB4 HLG 10-bit leg, non-panicking: kernel f32 HLG output of `display`
/// vs the f64 reference of `d`; CE7 per channel against max|d|.
fn pb4_hlg_leg(display: [f32; 3], d: [f64; 3], peak: f64) -> [Ce7Ch; 3] {
    let quant10 = |s: f64| (876.0 * s + 64.0).round();
    let g = hlg_gamma(as_f32(peak)).unwrap();
    let g64 = reference::hlg_gamma(peak).unwrap();
    let m = max_abs_3(d);
    let got = hlg_output(display, as_f32(peak), g).unwrap().signal;
    let exp = reference::hlg_output(d, peak, g64).unwrap().signal;
    [0, 1, 2].map(|i| {
        let codes = (quant10(f64::from(got[i])) - quant10(exp[i])).abs();
        let disp_err = (f64::from(display[i]) - d[i]).abs();
        let ok = pb4_10bit_channel_ok(codes, disp_err, m);
        Ce7Ch {
            codes,
            disp_err,
            ok,
        }
    })
}

/// PB4 SDR 8-bit leg, non-panicking: EETF→100 → 709 → SDR compress of
/// `display` (f32 kernel) vs `d` (f64 reference); CE7 per channel, display
/// prong in SDR nits vs the reference SDR triplet max.
fn pb4_sdr_leg(display: [f32; 3], d: [f64; 3], peak: f64) -> [Ce7Ch; 3] {
    let quant8 = |x: f64| (219.0 * (x.max(0.0) / 100.0).powf(1.0 / 2.4) + 16.0).round();
    let tone = display.map(|x| eetf_to_target(x, as_f32(peak), 100.0).unwrap().value);
    let rec709 = apply_matrix(BT2020_TO_BT709, tone).unwrap();
    let sdr = gamut_compress(rec709, CompressDest::Sdr { target_peak: 100.0 })
        .unwrap()
        .value;
    let tone64 = d.map(|x| reference::eetf_to_target(x, peak, 100.0).unwrap().value);
    let rec709_64 = reference::apply_matrix(BT2020_TO_BT709_F64, tone64).unwrap();
    let sdr64 = reference::gamut_compress(rec709_64, CompressDest::Sdr { target_peak: 100.0 })
        .unwrap()
        .value;
    let ms = max_abs_3(sdr64);
    [0, 1, 2].map(|i| {
        let codes = (quant8(f64::from(sdr[i])) - quant8(sdr64[i])).abs();
        let disp_err = (f64::from(sdr[i]) - sdr64[i]).abs();
        let ok = pb4_8bit_channel_ok(codes, disp_err, ms);
        Ce7Ch {
            codes,
            disp_err,
            ok,
        }
    })
}

/// PB4 HLG 10-bit gate: every channel passes CE7 (test-only quantizer; S4
/// owns delivery packing); accumulates codes.
fn pb4_hlg_gate(acc: &mut Pb4Acc, display: [f32; 3], d: [f64; 3], white: f64, peak: f64) {
    for (i, c) in pb4_hlg_leg(display, d, peak).into_iter().enumerate() {
        assert!(c.ok, "PB4[{i}] d={d:?} W={white} P={peak}: {c:?}");
        if c.codes > pb4_code_tol() {
            acc.by_display10 += 1;
        }
        acc.max10 = acc.max10.max(c.codes);
        acc.sum10 += c.codes;
        acc.n10 += 1;
    }
}

/// PB4 SDR 8-bit gate: every channel passes CE7 (CE9: all peaks, all
/// channels); accumulates codes.
fn pb4_sdr_gate(acc: &mut Pb4Acc, display: [f32; 3], d: [f64; 3], white: f64, peak: f64) {
    for (i, c) in pb4_sdr_leg(display, d, peak).into_iter().enumerate() {
        assert!(c.ok, "PB4 SDR[{i}] d={d:?} W={white} P={peak}: {c:?}");
        if c.codes > pb4_8bit_tol() {
            acc.by_display8 += 1;
        }
        acc.max8 = acc.max8.max(c.codes);
    }
}

/// CE7 failures of both legs (HLG + SDR) without asserting — the control
/// chain's load-bearing count.
fn pb4_ce7_failures(display: [f32; 3], d: [f64; 3], peak: f64) -> usize {
    pb4_hlg_leg(display, d, peak)
        .into_iter()
        .chain(pb4_sdr_leg(display, d, peak))
        .filter(|c| !c.ok)
        .count()
}

/// PB4 per-anchor gate: HLG 10-bit + SDR 8-bit legs, CE7 per channel.
fn pb4_gate(acc: &mut Pb4Acc, display: [f32; 3], d: [f64; 3], white: f64, peak: f64) {
    pb4_hlg_gate(acc, display, d, white, peak);
    pb4_sdr_gate(acc, display, d, white, peak);
}

#[test]
fn pb4_final_codes() {
    // Delivery chains through f16 working stores vs direct f64 delivery.
    // CE7 per channel (inside the gate); the working-chain anchors also
    // pass codes outright (10-bit ≤ 2, 8-bit ≤ 1 codes).
    let mut acc = Pb4Acc::default();
    for white in [100.0, 203.0, 400.0] {
        for peak in [400.0, 1000.0, 2000.0, 2001.0, 4000.0, 10_000.0] {
            let g64 = reference::hlg_gamma(peak).unwrap();
            let sw64 = reference::s_white(white, peak, g64).unwrap();
            let grey = reference::scene_to_display([0.18 * sw64; 3], peak, g64).unwrap()[0];
            for d in [
                [0.0, 0.0, 0.0],
                [grey, grey, grey],
                [white, white, white],
                [peak, peak, peak],
                [peak, 0.0, 0.0],
                [0.0, peak, 0.0],
                [0.0, 0.0, peak],
                [0.5, 0.5, 0.5],
                [white * 0.9; 3],
                [white * 1.1; 3],
            ] {
                pb4_gate(&mut acc, pb4_chain_display(d, white, peak), d, white, peak);
            }
        }
    }
    assert!(acc.max10 <= pb4_code_tol(), "PB4 10-bit max: {}", acc.max10);
    assert!(acc.max8 <= pb4_8bit_tol(), "PB4 8-bit max: {}", acc.max8);
    assert_eq!(acc.by_display10 + acc.by_display8, 0, "no display prong");
    // CE7 prong visibility under CE9's model D: saturated green at
    // W=194/P=400 — the zero channels miss by codes (HLG toe) and pass only
    // through the CE7 display prong.
    pb4_gate(
        &mut acc,
        matrix_chain_display([0.0, 400.0, 0.0], 194.0, 400.0, 1),
        [0.0, 400.0, 0.0],
        194.0,
        400.0,
    );
    assert!(
        acc.by_display10 > 0,
        "D anchor zero channels must miss by codes"
    );
    let mean10 = acc.sum10 / f64::from(acc.n10);
    println!(
        "PB4 anchors: n10={} mean10={mean10} by_display10={}",
        acc.n10, acc.by_display10
    );
    assert!(mean10 <= pb4_mean_tol(), "PB4 10-bit mean: {mean10}");
    // HLG codes per working-ULP at white: oracle 0.1680400672866091.
    let sw = reference::s_white(203.0, 1000.0, 1.2).unwrap();
    let c_up = reference::hlg_oetf((1.0 + ulp_w1()) * sw).unwrap();
    let c_dn = reference::hlg_oetf((1.0 - ulp_w1()) * sw).unwrap();
    assert_close(
        "codes/ULP",
        (c_up - c_dn) / 2.0 * 876.0,
        0.168_040_067_286_609_1,
        1e-6,
    );
    assert_close(
        "codes/ULP App N",
        (c_up - c_dn) / 2.0 * 876.0,
        0.168_0,
        1e-4,
    );
    let fwd = (reference::hlg_oetf((1.0 + ulp_w1()) * sw).unwrap()
        - reference::hlg_oetf(sw).unwrap())
        * 876.0;
    assert_close("codes/ULP fwd", fwd, 0.167_950_006_847_549_02, 1e-9);
}

#[test]
fn closing_pb4_major_after_matrix_storage() {
    // Verify B1 (ported), re-derived under CE9. Through the foreign-space
    // CONTROL (P=400/W=203, [16, 400, 16], one pair) every channel is ≥ 4%
    // of the max (major under CE6's 1% cutoff), yet red/blue inherit green's
    // storage quantum and miss by 4 codes; CE7 passes them by display error
    // (≈0.035 nits ≤ 0.2% × 400). Under model D the same anchor needs no
    // prong at all (0 codes).
    let d = [16.0, 400.0, 16.0];
    let mut acc = Pb4Acc::default();
    let display = foreign_space_store(d, 203.0, 400.0, 1);
    pb4_gate(&mut acc, display, d, 203.0, 400.0);
    println!(
        "control display={display:?} max10={} by_display10={}",
        acc.max10, acc.by_display10
    );
    // Erratum visibility: the pure 2-code rule (CE6's major reading) fails.
    assert!(
        acc.max10 > pb4_code_tol(),
        "[16,400,16] must fail the pure 2-code rule: {}",
        acc.max10
    );
    assert!(acc.by_display10 >= 2, "red and blue pass only by display");
    assert!(
        d.iter().all(|&ch| ch >= 0.01 * 400.0),
        "all major under CE6"
    );
    let mut acc_d = Pb4Acc::default();
    pb4_gate(
        &mut acc_d,
        matrix_chain_display(d, 203.0, 400.0, 1),
        d,
        203.0,
        400.0,
    );
    assert_eq!(acc_d.by_display10 + acc_d.by_display8, 0, "model D: codes");
}

#[test]
fn pb4_foreign_space_store_control_fails_ce7() {
    // CE9 is load-bearing: verify2's three `verify2_gate_*` witnesses FAIL
    // PB4 CE7 through the foreign-space control (709 f16 store) and pass
    // through model D (2020-only stores).
    for (p, w, pairs, d) in [
        (1001.0, 100.0, 1, [20.02, 1001.0, 20.02]),
        (9332.0, 113.0, 3, [23.33, 9332.0, 23.33]),
        (9794.0, 159.0, 3, [0.0, 0.0, 9794.0]),
    ] {
        let bad = pb4_ce7_failures(foreign_space_store(d, w, p, pairs), d, p);
        assert!(bad > 0, "control must fail CE7: d={d:?} P={p} W={w}");
        let mut acc = Pb4Acc::default();
        pb4_gate(&mut acc, matrix_chain_display(d, w, p, pairs), d, w, p);
    }
}

#[test]
fn pb4_composed_coloured_anchor_scan() {
    // CE7 closing scan under CE9's model D: coloured anchors around and
    // above CE6's old 1% cutoff. One channel at P, the other two at f·P;
    // P × W × pairs × f × position = 324 anchors. HLG 10-bit and SDR 8-bit:
    // every channel passes CE7; 10-bit mean over all channels. The same
    // grid through the foreign-space control must FAIL CE7 somewhere.
    let mut acc = Pb4Acc::default();
    let (mut n, mut control_bad) = (0u32, 0usize);
    for peak in [400.0, 1000.0, 4000.0] {
        for white in [100.0, 203.0, 400.0] {
            for pairs in [1, 3] {
                for f in [0.005, 0.01, 0.02, 0.04, 0.1, 0.25] {
                    for i in 0..3 {
                        let mut d = [f * peak; 3];
                        d[i] = peak;
                        let display = matrix_chain_display(d, white, peak, pairs);
                        pb4_gate(&mut acc, display, d, white, peak);
                        let ctl = foreign_space_store(d, white, peak, pairs);
                        control_bad += pb4_ce7_failures(ctl, d, peak);
                        n += 1;
                    }
                }
            }
        }
    }
    // Prong visibility under D: saturated green at W=194/P=400 misses by
    // codes on the zero channels and passes by display.
    let d = [0.0, 400.0, 0.0];
    pb4_gate(
        &mut acc,
        matrix_chain_display(d, 194.0, 400.0, 1),
        d,
        194.0,
        400.0,
    );
    let mean10 = acc.sum10 / f64::from(acc.n10);
    println!(
        "CE7 scan: anchors={n} ch={} max10={} mean10={mean10} by_display10={} \
         max8={} by_display8={} control_ce7_failures={control_bad}",
        acc.n10, acc.max10, acc.by_display10, acc.max8, acc.by_display8
    );
    assert_eq!(n, 324);
    assert!(mean10 <= pb4_mean_tol(), "CE7 scan 10-bit mean: {mean10}");
    assert!(acc.max10 > pb4_code_tol(), "scan must exercise the prong");
    assert!(acc.by_display10 > 0);
    assert!(control_bad > 0, "foreign-space control must fail CE7");
    // Former CE8 witness: [20, 4000, 20], W=203, three pairs. Through the
    // control the SDR red channel fails CE7 (Linux: 39 codes, 1.60% of the
    // SDR max); through model D it passes.
    let d = [20.0, 4000.0, 20.0];
    let c = pb4_sdr_leg(foreign_space_store(d, 203.0, 4000.0, 3), d, 4000.0)[0];
    println!("control witness red: {c:?}");
    assert!(!c.ok, "control witness must fail CE7: {c:?}");
    assert!(c.codes > pb4_8bit_tol() && c.disp_err > 0.002 * 100.0);
    pb4_gate(
        &mut acc,
        matrix_chain_display(d, 203.0, 4000.0, 3),
        d,
        203.0,
        4000.0,
    );
}

#[test]
fn pb4_sdr_high_peak_scan() {
    // CE9 withdrew CE8: the SDR leg is CE7 at every peak. Former CE8
    // envelope grid (P 1500/2000/4000/10000 × minor 0.1/0.5/1% × W 203/400
    // × three pairs × major position = 72 anchors) under model D: HLG and
    // SDR every channel passes CE7; the control fails CE7 on the same grid.
    let mut acc = Pb4Acc::default();
    let (mut n, mut control_bad) = (0u32, 0usize);
    for peak in [1500.0, 2000.0, 4000.0, 10_000.0] {
        for f in [0.001, 0.005, 0.01] {
            for white in [203.0, 400.0] {
                for i in 0..3 {
                    let mut d = [f * peak; 3];
                    d[i] = peak;
                    let display = matrix_chain_display(d, white, peak, 3);
                    pb4_gate(&mut acc, display, d, white, peak);
                    let ctl = foreign_space_store(d, white, peak, 3);
                    control_bad += pb4_ce7_failures(ctl, d, peak);
                    n += 1;
                }
            }
        }
    }
    println!(
        "high-peak scan: anchors={n} max10={} max8={} control_ce7_failures={control_bad}",
        acc.max10, acc.max8
    );
    assert_eq!(n, 72);
    assert!(control_bad > 0, "foreign-space control must fail CE7");
}

fn ulp_w1() -> f64 {
    2f64.powi(-10)
}

// --- §17 wrong-transform controls (R15): one shared anchor/assertion harness
// per gate. The production function passes each gate; every substitute runs
// through the SAME gate and must fail it (gates return Result so rejection
// is asserted, not panicked through). Each gate cites the kernel test whose
// anchors it shares. ---

/// Gate assertion: `Ok(())` within tol, else `Err` naming the anchor.
fn gate_close(label: &str, actual: f64, expected: f64, tol: f64) -> Result<(), String> {
    let diff = (actual - expected).abs();
    if diff <= tol {
        Ok(())
    } else {
        Err(format!(
            "{label}: actual={actual} expected={expected} diff={diff} tol={tol}"
        ))
    }
}

/// EETF anchor gate (mirrors `eetf_anchors_narrow_span`, Cs=1000, Ct=100).
fn eetf_gate(eval: &dyn Fn(f64, f64, f64) -> f64) -> Result<(), String> {
    gate_close("eetf black", eval(0.0, 1000.0, 100.0), 0.0, 1e-12)?;
    gate_close("eetf Cs→Ct", eval(1000.0, 1000.0, 100.0), 100.0, 1e-9)?;
    gate_close(
        "eetf L=100",
        eval(100.0, 1000.0, 100.0),
        69.454_403_035_658_91,
        1e-9,
    )?;
    gate_close(
        "eetf L=203",
        eval(203.0, 1000.0, 100.0),
        88.243_640_538_647_8,
        1e-9,
    )
}

/// HLG-input anchor gate (mirrors `hlg_inverse_oetf_anchors` +
/// `hlg_reference_white_lands_on_working_one`).
fn hlg_input_gate(eval: &dyn Fn([f64; 3]) -> [f64; 3]) -> Result<(), String> {
    let sw = reference::s_white(203.0, 1000.0, 1.2).unwrap();
    let scene = eval([0.75, 0.75, 0.75])[0];
    gate_close("hlg scene at 0.75", scene, 0.264_962_559_786_400_15, 1e-12)?;
    let w = eval([0.749_877_365, 0.749_877_365, 0.749_877_365])[0] / sw;
    gate_close("hlg working 1.0", w, 0.999_999_999_477_619_4, 1e-9)
}

/// HLG-compressor anchor gate (mirrors `hlg_primaries_u_bounds_pre_post_fit`,
/// P=1000, γ=1.2): fitted max channel lands on U per primary.
fn hlg_compress_gate(eval: &dyn Fn([f64; 3], f64, f64) -> [f64; 3]) -> Result<(), String> {
    for (prim, u) in [
        ([1000.0, 0.0, 0.0], 800.282_546_629_577_4),
        ([0.0, 1000.0, 0.0], 937.284_889_648_623_7),
        ([0.0, 0.0, 1000.0], 624.466_457_290_609_7),
    ] {
        let fit = eval(prim, 1000.0, 1.2);
        let max_c = fit[0].max(fit[1]).max(fit[2]);
        gate_close(&format!("hlg U for {prim:?}"), max_c, u, 1e-9)?;
    }
    Ok(())
}

/// Extended (white-preserving) Reinhard as an EETF substitute: matches 0→0
/// and Cs→Ct, so it is a worthy adversary — and still wrong in the middle.
fn reinhard_eetf_sub(l: f64, cs: f64, ct: f64) -> f64 {
    if ct >= cs {
        return l;
    }
    l * (1.0 + l / (ct * ct)) / (1.0 + l / ct)
}

#[test]
fn control_reinhard_fails_eetf() {
    let prod = |l: f64, cs: f64, ct: f64| reference::eetf_to_target(l, cs, ct).unwrap().value;
    assert!(eetf_gate(&prod).is_ok());
    let err = eetf_gate(&reinhard_eetf_sub).unwrap_err();
    // Black and Cs→Ct pass (worthy adversary); the mid anchor rejects it.
    assert!(err.contains("eetf L=100"), "unexpected rejection: {err}");
}

/// §4 sRGB inverse OETF (test-only copy) as an HLG-input substitute.
fn srgb_gamma_decode_sub(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn control_srgb_gamma_fails_hlg_input() {
    let prod = |s: [f64; 3]| s.map(|x| reference::hlg_inverse_oetf(x).unwrap());
    assert!(hlg_input_gate(&prod).is_ok());
    let sub = |s: [f64; 3]| s.map(srgb_gamma_decode_sub);
    assert!(hlg_input_gate(&sub).is_err());
}

/// HLG compressor with 709 luma (test-only copy of the §6 equation).
fn hlg_compress_709_sub(rgb: [f64; 3], peak: f64, gamma: f64) -> [f64; 3] {
    let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    let u = peak * (y / peak).powf((gamma - 1.0) / gamma);
    let mut t = 1.0f64;
    for c in rgb {
        if c < y {
            t = t.min(y / (y - c));
        } else if c > y {
            t = t.min((u - y) / (c - y));
        }
    }
    [
        y + t * (rgb[0] - y),
        y + t * (rgb[1] - y),
        y + t * (rgb[2] - y),
    ]
}

#[test]
fn control_709_luma_fails_hlg_compressor() {
    let prod = |rgb: [f64; 3], p: f64, g: f64| {
        reference::gamut_compress(rgb, CompressDest::Hlg { peak: p, gamma: g })
            .unwrap()
            .value
    };
    assert!(hlg_compress_gate(&prod).is_ok());
    assert!(hlg_compress_gate(&hlg_compress_709_sub).is_err());
}

/// HLG input WITH an input-side OOTF (the archived attempt's forbidden light
/// model): inverse OETF then the full OOTF gain, as scene.
fn hlg_input_ootf_sub(signal: [f64; 3], peak: f64, gamma: f64) -> [f64; 3] {
    let s = [
        reference::hlg_inverse_oetf(signal[0]).unwrap(),
        reference::hlg_inverse_oetf(signal[1]).unwrap(),
        reference::hlg_inverse_oetf(signal[2]).unwrap(),
    ];
    let y = 0.2627 * s[0] + 0.6780 * s[1] + 0.0593 * s[2];
    let gain = peak * y.powf(gamma - 1.0);
    [gain * s[0], gain * s[1], gain * s[2]]
}

#[test]
fn control_input_ootf_fails_hlg_input() {
    let prod = |s: [f64; 3]| s.map(|x| reference::hlg_inverse_oetf(x).unwrap());
    assert!(hlg_input_gate(&prod).is_ok());
    let sub = |s: [f64; 3]| hlg_input_ootf_sub(s, 1000.0, 1.2);
    assert!(hlg_input_gate(&sub).is_err());
}
