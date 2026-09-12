//! AU5 §3.11: the synthetic-corruption fixtures and their pinned budgets.
//!
//! Exit-gate clause 1 — *"Repair is measured on synthetic corruptions with
//! pinned SNR gains"* — is closed by these lanes together with `audio.rs`'s
//! de-click lane (§3.11(c), which needs the node's own output buffer and so
//! lives beside `process_buffer_static`; AU5 §0 R62) and its latency and
//! identity pins.
//!
//! Every fixture is **exact `wav_f32` bytes** through `GeneratedMedia::from_bytes`
//! — no lavfi, because the provisioned `FFmpeg`'s `sine` emits −18 dBFS
//! (au3_fixtures.rs:127-131) — at 48 kHz stereo, summed from the **mono**
//! promoted helpers and interleaved here.
//!
//! Following AU3 §5.8 and CC6 §6.3, every lane **prints its measurement** and
//! asserts a margin against a single named constant rather than pinning a
//! value. There is no `cfg`-conditioned tolerance anywhere in this file, and
//! there must never be one.

use std::sync::Arc;

use kinewright_core::{
    Analysis, AudioBus, AudioBusId, AudioMix, AudioRepairRequest, Clip, ClipContent, ClipId,
    Document, Effect, EffectId, MediaAsset, MixNoiseProfileRequest, MixSpectrumPoint,
    MixSpectrumRequest, MixWindowRequest, NOISE_PROFILE_PARAMETER_NAMES, ParamValue, QaSeverity,
    Rational, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::{FfmpegMediaEngine, hum_removal_magnitude_db};

#[path = "../src/test_support.rs"]
pub mod test_support;
use test_support::{GeneratedMedia, pseudo_random_amplitude, tone, wav_f32};

/// CC6 rule 11.0.5: a budget no measurement approaches proves nothing.
const FIXTURE_MINIMUM_MARGIN: f64 = 2.0;

/// §3.11(a): the noise-floor drop the broadband lane must clear, in tenth dB.
///
/// **AU5 §0 R67 re-baselined this from 90 — and the *fixture window* with it.**
/// The gate's depth **is** set by `reduction_tenth_db`. With `f_k` the learned
/// per-bin RMS magnitude, `|X_k|` Rayleigh with `E|X|² = f²` and `a` the ratio
/// of the subtracted floor to the true one, rule 37's gain has expectation
///
/// ```text
/// E[g] = g_floor * P(r < a/(1 - g_floor))
///      + integral from a/(1 - g_floor) to infinity of (1 - a/r) * 2r e^(-r^2) dr
/// ```
///
/// Rule 37's 50 ms smoother holds the per-bin gain near that expectation, so
/// the steady-state drop is the analytic one, and the ≈ 12 dB this lane used to
/// read was a **measurement-window transient** — the smoother releasing from the
/// 1 kHz tone that ends on the tail window's left edge. Measuring from 2.6 s
/// restores the steady state.
///
/// **The shipped lane's `a` is 0.866, not 1** (AU5 §0 R67, R75). At `a = 1` the
/// integral gives `E[g] = 0.1561 → −16.13 dB` at `reduction_tenth_db = 200` and
/// `0.0954 → −20.41 dB` at 400. `a = 1` is what the **pre-R75** lane measured,
/// and only by a coincidence worth naming once: rule 44's conversion was
/// `+1.25 dB` high and §3.7 rule 61(ii)'s 20th-percentile learn reads ≈ 1.25 dB
/// *low*, so the two cancelled and the subtracted floor happened to equal the
/// true one. R75 removed the conversion error and left the percentile bias, so
/// the floor the runtime subtracts is now `10^(−1.25/20) = 0.866` of the true
/// per-bin RMS. **Re-evaluating** — not shifting — the integral at that `a`
/// gives `E[g] = 0.1901 →` **−14.42 dB** at reduction 200, against this lane's
/// measured **14.41 dB**: 0.01 dB between a first-principles Rayleigh integral
/// and the shipped pipeline. (At 400 the same `a` gives `0.1390 → −17.14 dB`.)
///
/// 70, then, and not 90: 90 would leave 1.60× against 14.41 dB, under the house
/// floor. The lane's headroom above the `FIXTURE_MINIMUM_MARGIN` gate is
/// **0.41 dB** — the assert trips at a measured 14.0 dB — which is deliberate
/// and is the thinnest margin in AU5 (hum 2.20, de-click 2.32, SNR 2.44, learn
/// 7.50). It is thin because the budget is pinned to an analytic steady state
/// rather than padded around a measurement, so anything that shallows the gate
/// by half a decibel is a **red lane rather than a silent drift**: a further
/// change to rule 44's conversion, a different percentile in rule 61(ii), or a
/// smoother long enough to keep the tone's release inside the tail window would
/// each move it. Dropping to 65 would buy 2.22× and give that up.
const DENOISE_FLOOR_DROP_BUDGET_TENTH_DB: f64 = 70.0;

/// §3.11(a): the 1 kHz tone loss the broadband lane may not exceed, tenth dB.
///
/// The tone's bins sit far above the learned floor, so `g ≈ 1` there and the
/// loss is a fraction of a decibel. Unchanged from the contract.
const DENOISE_TONE_LOSS_BUDGET_TENTH_DB: f64 = 10.0;

/// §3.11(a′): how close `mix_noise_profile` reads a band of known level.
const DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB: f64 = 15.0;

/// §3.11(a′): the **deepest** the gate may leave the learned band, tenth dB.
///
/// **AU5 §0 R78 replaced the contract's centre-and-budget with a derived
/// bracket.** The contract expected `L − 40 dB` at `reduction_tenth_db = 400`,
/// which the gain law cannot produce here: rule 37's gain is `1 − a·f_k/|X_k|`
/// under a `g_floor` clamp, and on material whose bins sit **above** the floor
/// the profile spreads evenly over the band the clamp never binds, so
/// `reduction_tenth_db` does not enter at all. At a 512-point runtime window and
/// 48 kHz the 1 kHz third-octave band holds just **two** one-sided bins, and
/// this fixture is a constant-envelope chirp, so if all of the band's power sat
/// in one of them then `|X| = √2·f_k` and `g = 1 − 1/√2 = 0.2929`, i.e.
/// `20·log10(0.2929) =` **−10.67 dB**. That is a *bound*, not a centre: the
/// 512-point Hann main lobe is four bins wide against a band only two bins
/// across, so part of the chirp's energy always leaks into neighbouring bins
/// whose floors come from bands 16 and 18, the real concentration is lower and
/// the residual is shallower. A residual **deeper** than the bound means the
/// profile is over-subtracting — the failure AU5 §0 R75's `sum w^2` correction
/// removed 1.25 dB of, and the arm that would have caught it. Measured
/// −7.35 dB, clearing the bound by 3.3 dB.
const DENOISE_PROFILE_GATE_BOUND_TENTH_DB: f64 = -106.7;

/// §3.11(a′): the **shallowest** the gate may leave the learned band, tenth dB.
///
/// The other half of R78's bracket: a gate that removes under 3 dB from a band
/// whose own measured level it has just learned is not gating, and the lane must
/// say so rather than passing on a number that only looks plausible. Measured
/// −7.35 dB, clearing this by 4.35 dB. Together the two bounds fail on both real
/// regressions — over-subtraction and a dead gate — which a ±3 dB window around
/// a fitted centre does not.
const DENOISE_PROFILE_GATE_FLOOR_TENTH_DB: f64 = -30.0;

/// §3.11(a′): how far a band neighbouring the learned one may move, tenth dB.
///
/// **AU5 §0 R69 re-baselined this from 30, and strengthened what it asserts.**
/// The profile is learned from this same fixture, so a neighbouring band's
/// measured content — Hann leakage from the chirp, about 25 dB under the band
/// itself — is *part of the learned floor* and the gate legitimately takes it
/// down too, by 5.6 dB. "Under 3.0 dB" was only ever reachable against
/// neighbours carrying content the profile had not learned. What the lane pins
/// instead is the statement that actually makes this a band gate: **every
/// energetic neighbour moves strictly less than the learned band does**, under
/// a 7.0 dB ceiling.
const DENOISE_PROFILE_NEIGHBOUR_BUDGET_TENTH_DB: f64 = 70.0;

/// §3.11(b): the hum drop the cascade must clear, in tenth dB.
///
/// 14 dB against an expected ≈ 30 dB — the **exactly-on-centre** case. A ±0.5 Hz
/// mains drift in a 4.17 Hz bandwidth costs several dB, so this proves the
/// design, not the field (AU5 §0 R20).
const HUM_DROP_BUDGET_TENTH_DB: f64 = 140.0;

/// §3.11(b): the 300 Hz loss the cascade may not exceed, in tenth dB.
///
/// **AU5 §0 R68 re-baselined this from 5.** R20 corrected the brief's 0.05 dB to
/// 0.13 by summing per-section estimates; the design's own analytic response —
/// `hum_removal_magnitude_db` at 300 Hz, which the lane's analytic arm matches
/// to 0.03 dB — is **0.56 dB**, because an RBJ peaking section's skirt at Q 12
/// falls more slowly than the estimate assumed. 1.5 dB keeps the ≥ 2× margin
/// against what the design really does, and the analytic arm is what proves the
/// number is the design and not a bug.
const HUM_TONE_LOSS_BUDGET_TENTH_DB: f64 = 15.0;

/// §3.11(b): how far the measured tone response may sit from
/// `hum_removal_magnitude_db`, the AU2 "compare against the design" rule.
const HUM_ANALYTIC_BUDGET_DB: f64 = 0.1;

/// §3.11(d): the SNR gain the Part A repair lane must clear, in hundredths.
///
/// Unchanged from the contract at 600, against a measured **1 461 hundredths**
/// — margin **2.44×**.
///
/// The figure to compare it with is `DENOISE_FLOOR_DROP_BUDGET_TENTH_DB`'s
/// analytic steady state, `E[g](a = 0.866) = 0.1901 →` **−14.42 dB**, not the
/// ≈ 11.7 dB AU5 §0 R67 withdrew as fitted. The 10th percentile of 10 ms
/// windows falls slightly *further* than the 400 ms mean-level drop the tail
/// lane reads — **14.61 dB against 14.41 dB**, 0.20 dB apart — because the
/// residual after spectral subtraction is fluctuating rather than stationary,
/// so a short-window percentile finds deeper troughs than a long-window mean.
///
/// The two lanes are **not interchangeable** and neither may be re-baselined
/// from the other's evidence: this one runs `range: None` over the whole 3 s
/// fixture, tone third included, and reads a *difference of two percentile
/// SNRs*; the tail lane reads a mean level over 400 ms of noise-only material.
/// That they land 0.20 dB apart is corroboration, not a shared measurement.
const AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS: f64 = 600.0;

const RATE: u32 = 48_000;

/// The fixture raster: 30 fps, which is what `probe_path` gives an audio-only
/// asset (`Rational::default()`), so the project and source grids agree and
/// `clip_duration` maps one to one.
fn fixture_fps() -> Rational {
    Rational::new(30, 1).expect("30 fps is valid")
}

/// One project range in **seconds**, on the fixture raster.
fn secs(range: std::ops::Range<f64>) -> std::ops::Range<i64> {
    #[allow(clippy::cast_possible_truncation)]
    let frame = |seconds: f64| (seconds * 30.0).round() as i64;
    frame(range.start)..frame(range.end)
}

/// Interleave one mono buffer to stereo, which every fixture does itself
/// because the promoted helpers are mono and their signatures do not change
/// (AU5 §0 R45).
fn to_stereo(mono: &[f32]) -> Vec<f32> {
    let mut stereo = Vec::with_capacity(mono.len() * 2);
    for sample in mono {
        stereo.push(*sample);
        stereo.push(*sample);
    }
    stereo
}

fn fixture_media(label: &str, stereo: &[f32]) -> GeneratedMedia {
    GeneratedMedia::from_bytes(label, "wav", &wav_f32(stereo, RATE, 2))
}

fn effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: std::collections::BTreeMap::new(),
    }
}

/// One audio track routed to one bus carrying `effects`.
fn fixture_document(asset: MediaAsset, effects: Vec<Effect>) -> Arc<Document> {
    let fps = fixture_fps();
    let duration = asset.duration;
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: AudioMix {
            tracks: Vec::new(),
            buses: vec![AudioBus {
                id: AudioBusId(1),
                name: "Repair".to_owned(),
                tracks: vec![TrackId(1)],
                gain_tenth_db: 0,
                effects,
                ducking_sidechain_tracks: Vec::new(),
                gain_curve: None,
            }],
            ..AudioMix::default()
        },
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..duration,
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
            }],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps,
        resolution: (320, 180),
        duration,
    };
    document.validate().expect("the fixture document is valid");
    Arc::new(document)
}

/// AU5 §2.1: an `audio_denoise` carrying one learned profile.
fn denoise_with_profile(id: u64, reduction: i64, bands: &[i32; 31]) -> Effect {
    let mut parameters = vec![
        ("reduction_tenth_db".to_owned(), reduction),
        ("floor_offset_tenth_db".to_owned(), 0),
        ("smoothing_milliseconds".to_owned(), 50),
        ("lookahead_milliseconds".to_owned(), 12),
    ];
    for (name, band) in NOISE_PROFILE_PARAMETER_NAMES.iter().zip(bands) {
        parameters.push(((*name).to_owned(), i64::from(*band)));
    }
    Effect {
        id: EffectId(id),
        name: "audio_denoise".to_owned(),
        parameters: parameters
            .into_iter()
            .map(|(name, value)| (name, ParamValue::Integer(value)))
            .collect(),
        keyframes: std::collections::BTreeMap::new(),
    }
}

fn engine() -> FfmpegMediaEngine {
    kinewright_media::initialize_ffmpeg().expect("FFmpeg initializes");
    test_support::test_engine("KINEWRIGHT_AU5_DATA_DIR")
}

/// The mean RMS level of one mix point over one project range, in dB.
///
/// Read through `Analysis::mix_window_levels` — AU5 §3.8's own accessor — as
/// the mean of the windows' **powers**, so a fluctuating gate's residual is
/// weighted the way an RMS is rather than the way a mean of decibels would be.
fn mean_level_db(
    engine: &FfmpegMediaEngine,
    document: &Document,
    range: std::ops::Range<i64>,
) -> f64 {
    let report = engine
        .mix_window_levels(
            document,
            &MixWindowRequest {
                range: Some(TimeCode(range.start)..TimeCode(range.end)),
                point: MixSpectrumPoint::Bus(AudioBusId(1)),
                window_milliseconds: 100,
                hop_milliseconds: 100,
            },
        )
        .expect("the window levels measure");
    let powers: Vec<f64> = report
        .windows
        .iter()
        .filter_map(|level| level.map(|level| 10.0_f64.powf(f64::from(level) / 1_000.0)))
        .collect();
    assert!(!powers.is_empty(), "the range carried no energetic window");
    #[allow(clippy::cast_precision_loss)]
    let mean = powers.iter().sum::<f64>() / powers.len() as f64;
    10.0 * mean.log10()
}

/// One third-octave band's level over one project range, in dB.
fn band_level_db(
    engine: &FfmpegMediaEngine,
    document: &Document,
    range: std::ops::Range<i64>,
    band: usize,
) -> f64 {
    let report = engine
        .mix_spectrum(
            document,
            &MixSpectrumRequest {
                range: Some(TimeCode(range.start)..TimeCode(range.end)),
                point: MixSpectrumPoint::Bus(AudioBusId(1)),
            },
        )
        .expect("the spectrum measures");
    report.bands[band]
        .level_dbfs_hundredths
        .map_or(SILENCE_FLOOR_DB, |level| f64::from(level) / 100.0)
}

/// `10*log10(SILENCE_POWER)`: what a band under the silence threshold reads as.
const SILENCE_FLOOR_DB: f64 = -120.0;

fn learn_profile(
    engine: &FfmpegMediaEngine,
    document: &Document,
    range: std::ops::Range<i64>,
) -> [i32; 31] {
    engine
        .mix_noise_profile(
            document,
            &MixNoiseProfileRequest {
                range: Some(TimeCode(range.start)..TimeCode(range.end)),
                point: MixSpectrumPoint::Bus(AudioBusId(1)),
            },
        )
        .expect("the profile learns")
        .bands
}

/// §3.11(a): 3 s — `[0, 1 s)` noise only, `[1, 2.5 s)` tone + noise,
/// `[2.5, 3 s)` noise only.
fn broadband_fixture() -> Vec<f32> {
    let mut mono = pseudo_random_amplitude(144_000, 0.010);
    let carrier = tone(1_000.0, 0.200, RATE, 72_000);
    for (index, sample) in carrier.iter().enumerate() {
        mono[48_000 + index] += *sample;
    }
    to_stereo(&mono)
}

/// AU5 §7 A7 / §3.11(a): the noise-floor drop and the 1 kHz tone loss.
#[test]
fn au5_the_broadband_lane_drops_its_floor_and_keeps_its_tone() {
    let engine = engine();
    let media = fixture_media("au5-broadband", &broadband_fixture());
    let asset = engine.probe(media.path()).expect("the fixture probes");

    let bare = fixture_document(asset.clone(), Vec::new());
    let profile = learn_profile(&engine, &bare, secs(0.0..1.0));
    let treated = fixture_document(asset, vec![denoise_with_profile(1, 200, &profile)]);

    let before_floor = mean_level_db(&engine, &bare, secs(2.6..3.0));
    let after_floor = mean_level_db(&engine, &treated, secs(2.6..3.0));
    let drop_tenth_db = (before_floor - after_floor) * 10.0;

    let before_tone = band_level_db(&engine, &bare, secs(1.2..2.3), 17);
    let after_tone = band_level_db(&engine, &treated, secs(1.2..2.3), 17);
    let loss_tenth_db = (before_tone - after_tone) * 10.0;

    let margin_drop = drop_tenth_db / DENOISE_FLOOR_DROP_BUDGET_TENTH_DB;
    let margin_tone = DENOISE_TONE_LOSS_BUDGET_TENTH_DB / loss_tenth_db.abs().max(f64::EPSILON);
    println!(
        "AU5_DENOISE measured_drop_tenth_db={drop_tenth_db:.1} \
         measured_tone_loss_tenth_db={loss_tenth_db:.2} margin_drop={margin_drop:.2} \
         margin_tone={margin_tone:.2}"
    );

    assert!(
        drop_tenth_db >= DENOISE_FLOOR_DROP_BUDGET_TENTH_DB,
        "the floor dropped only {drop_tenth_db} tenth dB"
    );
    assert!(
        loss_tenth_db <= DENOISE_TONE_LOSS_BUDGET_TENTH_DB,
        "the 1 kHz tone lost {loss_tenth_db} tenth dB"
    );
    assert!(
        margin_drop >= FIXTURE_MINIMUM_MARGIN,
        "the drop margin is only {margin_drop:.2}x"
    );
    assert!(
        margin_tone >= FIXTURE_MINIMUM_MARGIN,
        "the tone margin is only {margin_tone:.2}x"
    );
}

/// §3.11(a′): 64 phase-randomised sinusoids inside the 1 kHz third-octave band,
/// so the band's level `L` is **analytic**, not measured.
///
/// The band is `[1000·2^(−1/6), 1000·2^(1/6))` = `[891.4, 1122.5)` Hz. Total
/// mean-square power is `64·A²/2`, and rule 43's scale reads
/// `10·log10(P / 0.5)`, so `A = 1.25e-3` gives exactly `−40.00 dBFS`.
fn band_limited_fixture() -> (Vec<f32>, f64) {
    const PARTIALS: usize = 64;
    const AMPLITUDE: f32 = 1.25e-3;
    let frames = 96_000;
    let low = 1_000.0 * 2.0_f64.powf(-1.0 / 6.0);
    let high = 1_000.0 * 2.0_f64.powf(1.0 / 6.0);
    let mut mono = vec![0.0_f32; frames];
    // A fixed integer phase spread, so the fixture is identical on every OS.
    for partial in 0..PARTIALS {
        #[allow(clippy::cast_precision_loss)]
        let position = (partial as f64 + 0.5) / PARTIALS as f64;
        let hertz = low + (high - low) * position;
        #[allow(clippy::cast_precision_loss)]
        let phase_offset = ((partial * partial) as f64 / (2 * PARTIALS) as f64).fract();
        let samples = tone(hertz, AMPLITUDE, RATE, frames + RATE as usize);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let shift = (phase_offset * f64::from(RATE) / hertz) as usize;
        for (index, slot) in mono.iter_mut().enumerate() {
            *slot += samples[index + shift];
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let power = PARTIALS as f64 * f64::from(AMPLITUDE) * f64::from(AMPLITUDE) / 2.0;
    let level = 10.0 * (power / 0.5).log10();
    (to_stereo(&mono), level)
}

/// AU5 §7 A4 / §3.11(a′): the profile's wire unit, pinned against an analytic
/// band level, and the gate it implies.
#[test]
fn au5_the_profile_unit_pin_learns_and_gates_a_known_band() {
    let engine = engine();
    let (samples, expected_db) = band_limited_fixture();
    let media = fixture_media("au5-band-limited", &samples);
    let asset = engine.probe(media.path()).expect("the fixture probes");
    let bare = fixture_document(asset.clone(), Vec::new());

    let profile = learn_profile(&engine, &bare, secs(0.0..1.0));
    let learned_tenth_db = f64::from(profile[17]);
    let expected_tenth_db = expected_db * 10.0;
    let learn_error = (learned_tenth_db - expected_tenth_db).abs();

    let treated = fixture_document(asset, vec![denoise_with_profile(1, 400, &profile)]);
    let before = band_level_db(&engine, &bare, secs(0.0..2.0), 17);
    let after = band_level_db(&engine, &treated, secs(0.0..2.0), 17);
    let gated_tenth_db = (after - before) * 10.0;

    let margin = DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB / learn_error.max(f64::EPSILON);
    let deep_clearance = gated_tenth_db - DENOISE_PROFILE_GATE_BOUND_TENTH_DB;
    let shallow_clearance = DENOISE_PROFILE_GATE_FLOOR_TENTH_DB - gated_tenth_db;
    println!(
        "AU5_DENOISE_UNIT learned_tenth_db={learned_tenth_db:.1} \
         expected_tenth_db={expected_tenth_db:.1} gated_tenth_db={gated_tenth_db:.1} \
         margin={margin:.2} gate_bound_tenth_db={DENOISE_PROFILE_GATE_BOUND_TENTH_DB:.1} \
         gate_clearance_deep_tenth_db={deep_clearance:.1} \
         gate_clearance_shallow_tenth_db={shallow_clearance:.1}"
    );

    assert!(
        learn_error <= DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB,
        "the profile read {learned_tenth_db} tenth dB against an analytic {expected_tenth_db}"
    );
    assert!(
        margin >= FIXTURE_MINIMUM_MARGIN,
        "the unit margin is {margin:.2}x"
    );
    assert!(
        gated_tenth_db >= DENOISE_PROFILE_GATE_BOUND_TENTH_DB,
        "the gate left {gated_tenth_db} tenth dB, deeper than the two-bin concentration bound \
         {DENOISE_PROFILE_GATE_BOUND_TENTH_DB}: the profile is over-subtracting"
    );
    assert!(
        gated_tenth_db <= DENOISE_PROFILE_GATE_FLOOR_TENTH_DB,
        "the gate left {gated_tenth_db} tenth dB, shallower than \
         {DENOISE_PROFILE_GATE_FLOOR_TENTH_DB}: it is not gating"
    );

    let mut checked = 0_usize;
    for band in [15_usize, 16, 18, 19] {
        let before = band_level_db(&engine, &bare, secs(0.0..2.0), band);
        if before <= SILENCE_FLOOR_DB + 20.0 {
            continue;
        }
        checked += 1;
        let after = band_level_db(&engine, &treated, secs(0.0..2.0), band);
        let moved = ((after - before) * 10.0).abs();
        assert!(
            moved <= DENOISE_PROFILE_NEIGHBOUR_BUDGET_TENTH_DB,
            "band {band} moved {moved} tenth dB"
        );
        assert!(
            moved < gated_tenth_db.abs(),
            "band {band} moved {moved} tenth dB, no less than the learned band's \
             {}",
            gated_tenth_db.abs()
        );
    }
    assert!(
        checked >= 2,
        "only {checked} neighbouring bands carried anything"
    );
}

/// §3.11(b): 2 s of 300 Hz at 0.200 with 50 / 100 / 150 Hz harmonics at −6 dB
/// each. Hum RMS `= sqrt((0.05² + 0.025² + 0.0125²)/2) = 0.04051` = −27.85 dBFS.
fn hum_fixture() -> Vec<f32> {
    let mut mono = tone(300.0, 0.200, RATE, 96_000);
    for (hertz, amplitude) in [(50.0, 0.0500_f32), (100.0, 0.0250), (150.0, 0.0125)] {
        for (index, sample) in tone(hertz, amplitude, RATE, 96_000).iter().enumerate() {
            mono[index] += *sample;
        }
    }
    to_stereo(&mono)
}

/// AU5 §7 A8 / §3.11(b): the hum drop, the 300 Hz loss, and the analytic arm.
#[test]
fn au5_the_hum_lane_notches_its_harmonics_and_matches_the_design() {
    let engine = engine();
    let media = fixture_media("au5-hum", &hum_fixture());
    let asset = engine.probe(media.path()).expect("the fixture probes");
    let bare = fixture_document(asset.clone(), Vec::new());
    let node = effect(
        1,
        "audio_hum_removal",
        &[
            ("fundamental_hertz", 50),
            ("harmonic_count", 3),
            ("depth_tenth_db", -300),
            ("notch_q_hundredths", 1_200),
        ],
    );
    let treated = fixture_document(asset, vec![node.clone()]);

    // Bands 4 / 7 / 9 carry 50 / 100 / 150 Hz and band 12 carries 300.
    let mut before_power = 0.0_f64;
    let mut after_power = 0.0_f64;
    let mut worst_analytic = 0.0_f64;
    for (band, hertz) in [(4_usize, 50.0_f64), (7, 100.0), (9, 150.0)] {
        let before = band_level_db(&engine, &bare, secs(0.0..2.0), band);
        let after = band_level_db(&engine, &treated, secs(0.0..2.0), band);
        before_power += 10.0_f64.powf(before / 10.0);
        after_power += 10.0_f64.powf(after / 10.0);
        let analytic = hum_removal_magnitude_db(&node, TimeCode::ZERO, hertz, RATE);
        worst_analytic = worst_analytic.max(((after - before) - analytic).abs());
    }
    let drop_tenth_db = 10.0 * (before_power / after_power).log10() * 10.0;

    let before_tone = band_level_db(&engine, &bare, secs(0.0..2.0), 12);
    let after_tone = band_level_db(&engine, &treated, secs(0.0..2.0), 12);
    let loss_tenth_db = (before_tone - after_tone) * 10.0;
    let analytic_300 = hum_removal_magnitude_db(&node, TimeCode::ZERO, 300.0, RATE);
    worst_analytic = worst_analytic.max(((after_tone - before_tone) - analytic_300).abs());

    let margin_drop = drop_tenth_db / HUM_DROP_BUDGET_TENTH_DB;
    let margin_tone = HUM_TONE_LOSS_BUDGET_TENTH_DB / loss_tenth_db.abs().max(f64::EPSILON);
    println!(
        "AU5_HUM measured_drop_tenth_db={drop_tenth_db:.1} \
         measured_tone_loss_tenth_db={loss_tenth_db:.2} analytic_delta_db={worst_analytic:.4} \
         margin_drop={margin_drop:.2} margin_tone={margin_tone:.2}"
    );

    assert!(
        drop_tenth_db >= HUM_DROP_BUDGET_TENTH_DB,
        "the hum dropped only {drop_tenth_db} tenth dB"
    );
    assert!(
        loss_tenth_db <= HUM_TONE_LOSS_BUDGET_TENTH_DB,
        "the 300 Hz tone lost {loss_tenth_db} tenth dB"
    );
    assert!(
        worst_analytic <= HUM_ANALYTIC_BUDGET_DB,
        "the measured response differs from the design by {worst_analytic} dB"
    );
    assert!(
        margin_drop >= FIXTURE_MINIMUM_MARGIN,
        "the hum drop margin is only {margin_drop:.2}x"
    );
    assert!(
        margin_tone >= FIXTURE_MINIMUM_MARGIN,
        "the hum tone margin is only {margin_tone:.2}x"
    );
}

/// AU5 §7 A11 / §3.11(d): what makes Part A a complete exit gate on clause 1.
///
/// Fixture (a) routed through a bus carrying one `audio_denoise`;
/// `Analysis::audio_repair` on the bus point before and after the profile is
/// written and the reduction set to 200.
#[test]
fn au5_the_repair_inspector_measures_a_pinned_snr_gain() {
    let engine = engine();
    let media = fixture_media("au5-snr", &broadband_fixture());
    let asset = engine.probe(media.path()).expect("the fixture probes");
    let bare = fixture_document(asset.clone(), Vec::new());
    let profile = learn_profile(&engine, &bare, secs(0.0..1.0));
    let treated = fixture_document(asset, vec![denoise_with_profile(1, 200, &profile)]);

    let request = AudioRepairRequest {
        range: None,
        point: MixSpectrumPoint::Bus(AudioBusId(1)),
    };
    let before = engine
        .audio_repair(&bare, &request)
        .expect("the inspector measures");
    let after = engine
        .audio_repair(&treated, &request)
        .expect("the inspector measures");

    assert!(before.evidence_only);
    assert_eq!(before.window_milliseconds, 10);
    assert!(before.windows >= 10, "{} energetic windows", before.windows);
    let before_snr = before.snr_db_hundredths.expect("enough windows");
    let after_snr = after.snr_db_hundredths.expect("enough windows");
    let gain = f64::from(after_snr - before_snr);
    let margin = gain / AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS;
    println!(
        "AU5_REPAIR_SNR before_hundredths={before_snr} after_hundredths={after_snr} \
         gain_hundredths={gain:.0} margin={margin:.2}"
    );
    assert!(
        gain >= AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS,
        "the SNR gained only {gain} hundredths"
    );
    assert!(
        margin >= FIXTURE_MINIMUM_MARGIN,
        "the SNR gain margin is only {margin:.2}x"
    );
}

/// AU5 §7 A10 / §3.9: the inspector's hum and click arms.
///
/// A 50 Hz fundamental over a broadband floor reads a large `hum_50` and a
/// near-zero `hum_60`, with **four** per-harmonic values reported for each;
/// and `click_density_per_minute` comes from the same `detect_clicks` the node
/// uses, so a clean fixture reads zero.
#[test]
fn au5_the_repair_inspector_separates_fifty_from_sixty_hertz() {
    let engine = engine();
    let mut mono = pseudo_random_amplitude(96_000, 0.010);
    for (index, sample) in tone(50.0, 0.1000, RATE, 96_000).iter().enumerate() {
        mono[index] += *sample;
    }
    let media = fixture_media("au5-mains", &to_stereo(&mono));
    let asset = engine.probe(media.path()).expect("the fixture probes");
    let document = fixture_document(asset, Vec::new());

    let report = engine
        .audio_repair(
            &document,
            &AudioRepairRequest {
                range: None,
                point: MixSpectrumPoint::Bus(AudioBusId(1)),
            },
        )
        .expect("the inspector measures");
    let hum_50 = report.hum_50_excess_db_hundredths.expect("one whole block");
    let hum_60 = report.hum_60_excess_db_hundredths.expect("one whole block");
    println!(
        "AU5_HUM_INSPECTOR hum_50_hundredths={hum_50} hum_60_hundredths={hum_60} \
         harmonics_50={:?} harmonics_60={:?} clicks={} density={}",
        report.hum_50_harmonic_excess_db_hundredths,
        report.hum_60_harmonic_excess_db_hundredths,
        report.click_count,
        report.click_density_per_minute,
    );
    assert_eq!(report.hum_50_harmonic_excess_db_hundredths.len(), 4);
    assert_eq!(report.hum_60_harmonic_excess_db_hundredths.len(), 4);
    assert!(
        hum_50 > 1_500,
        "a 50 Hz fundamental over a floor must read a large excess, not {hum_50}"
    );
    assert!(
        hum_60 < 300,
        "a fixture with no 60 Hz content must read a near-zero excess, not {hum_60}"
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == "mains_hum_present"),
        "core must raise `mains_hum_present`: {:?}",
        report.findings
    );
    assert_eq!(report.click_count, 0, "the mains fixture carries no click");
}

/// AU5 §7 A10 / §2.4 rule 22: below ten energetic windows the three percentile
/// fields are `None` and the report says so through a `low_window_count`
/// finding rather than inventing a number.
///
/// The hum fields are `None` on the same range for a different reason — it is
/// under one whole `HUM_GOERTZEL_BLOCK_FRAMES` block (AU5 §0 R64) — so this arm
/// also pins the honest-`None` floor that erratum moved from 100 ms to 500 ms.
#[test]
fn au5_the_repair_inspector_refuses_to_invent_a_percentile() {
    let engine = engine();
    let media = fixture_media("au5-short", &broadband_fixture());
    let asset = engine.probe(media.path()).expect("the fixture probes");
    let document = fixture_document(asset, Vec::new());

    let report = engine
        .audio_repair(
            &document,
            &AudioRepairRequest {
                range: Some(TimeCode(0)..TimeCode(2)),
                point: MixSpectrumPoint::Bus(AudioBusId(1)),
            },
        )
        .expect("a short range is measured, not refused");
    println!(
        "AU5_REPAIR_SHORT windows={} snr={:?} floor={:?} signal={:?} hum_50={:?} findings={:?}",
        report.windows,
        report.snr_db_hundredths,
        report.noise_floor_dbfs_hundredths,
        report.signal_dbfs_hundredths,
        report.hum_50_excess_db_hundredths,
        report
            .findings
            .iter()
            .map(|finding| finding.code.clone())
            .collect::<Vec<_>>(),
    );
    assert!(report.windows < 10, "{} energetic windows", report.windows);
    assert!(report.noise_floor_dbfs_hundredths.is_none());
    assert!(report.signal_dbfs_hundredths.is_none());
    assert!(report.snr_db_hundredths.is_none());
    // Under one Goertzel block, so no hum number is fabricated either.
    assert!(report.hum_50_excess_db_hundredths.is_none());
    assert!(report.hum_60_excess_db_hundredths.is_none());
    assert!(report.hum_50_harmonic_excess_db_hundredths.is_empty());
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.code == "low_window_count")
        .expect("core must raise `low_window_count`");
    assert_eq!(finding.severity, QaSeverity::Info);
    assert!(
        !report
            .findings
            .iter()
            .any(|finding| finding.code == "low_signal_to_noise"),
        "an unmeasured SNR must not also read as a bad one"
    );
}
