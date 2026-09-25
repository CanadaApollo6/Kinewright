//! CC8 S1 numeric-kernel conformance suite.
//!
//! Expectations are independent vectors from `/tmp/cc8_oracle.py` (transcribed
//! from ST 2084 / ARIB STD-B67 / BT.2100, cross-checked against App. N), never
//! values the kernel printed. f64 reference anchors hold ±1e-6 per the design;
//! most assert far tighter. Grows one section per S1 increment.

use kinewright_core::{
    Cc8KernelError, CompressDest, display_to_scene, eetf_to_target, gamut_compress, hlg_gamma,
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
        Err(Cc8KernelError::NonFiniteResult { .. })
    ));
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
        (1000.0, 100.000_000_000_005_24, 100.0),
    ] {
        let out = reference::eetf_to_target(l, 1000.0, 100.0).unwrap();
        assert!(!out.clipped, "L={l} must not flag");
        assert_close("eetf narrow", out.value, expected, 1e-9);
        assert_close("eetf narrow App N", out.value, appn, 1e-6);
    }
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
        (10_000.0, 100.000_000_000_005_24, 100.0),
    ] {
        let out = reference::eetf_to_target(l, 10_000.0, 100.0).unwrap();
        assert!(!out.clipped, "L={l} must not flag");
        assert_close("eetf wide", out.value, expected, 1e-9);
        assert_close("eetf wide App N", out.value, appn, 1e-6);
    }
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
