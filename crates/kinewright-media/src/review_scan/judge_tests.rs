//! Unit witnesses of the pure decision rules (JA-1 S5a commit 5, fix
//! round 2): onset, per-word separation, freeze runs, the video lag of an
//! fps-converted cut and the source grid's coding.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "synthetic series of a few hundred values"
)]
use super::super::grid::Stats;
use super::super::picture::{FrameRecord, FrameState, PictureScan};
use super::super::plan::Clock;
use super::*;

fn level(own_db: f64, other_db: f64, out_db: f64) -> HopLevels {
    let power = |value: f64| 10_f64.powf(value / 10.0);
    let own = power(own_db);
    HopLevels {
        own,
        all: own + power(other_db),
        out: power(out_db),
    }
}

/// A quiet unstressed word: two short syllables at −30 dBFS in a long box,
/// under a steady music bed 10 dB below them.
fn quiet_word(present: bool) -> Vec<HopLevels> {
    (0..30)
        .map(|hop| {
            let own_db = if (4..7).contains(&hop) || (16..19).contains(&hop) {
                -30.0
            } else {
                -62.0
            };
            let own = 10_f64.powf(own_db / 10.0);
            let bed = 10_f64.powf(-40.0 / 10.0);
            let out = if present { own + bed } else { bed };
            HopLevels {
                own,
                all: own + bed,
                out,
            }
        })
        .collect()
}

/// R10: separation is weighted by the word's own power, so the hops where a
/// quiet word sounds decide it, not the gaps inside its box. The plain mean
/// over the box (the old rule) leaves this word Ambiguous; the weighted
/// rule decides it present, and decides its deletion missing.
#[test]
fn ja1_s5a_fix2_a_quiet_word_under_a_bed_is_decided() {
    let config = RetainedSpeechConfig::default();
    assert_eq!(
        judge_levels(&quiet_word(true), &config),
        WordCompare::Present
    );
    assert_eq!(
        judge_levels(&quiet_word(false), &config),
        WordCompare::Missing
    );
    // The old, unweighted separation of the same box.
    let (own, all) = quiet_word(true)
        .iter()
        .fold((0.0, 0.0), |(own, all), level| {
            (own + level.own, all + level.all)
        });
    let plain = 10.0 * (all / (all - own)).log10();
    assert!(plain < f64::from(config.separation_min_db), "{plain}");
}

/// R10: padding before the sound is not the onset.
#[test]
fn ja1_s5a_fix2_the_onset_starts_at_the_sound() {
    let mut levels = vec![level(-70.0, -80.0, -70.0); 5];
    levels.extend(vec![level(-20.0, -80.0, -20.0); 20]);
    assert_eq!(acoustic_onset(&levels), Some(5));
    assert_eq!(acoustic_onset(&[HopLevels::default(); 3]), None);
}

/// R3: an onset that cannot be judged leaves the word Unknown; one shift
/// finding it is enough; every shift missing it clips it.
#[test]
fn ja1_s5a_fix2_an_undecided_onset_is_not_a_pass() {
    use WordCompare::{Ambiguous, Missing, Present, Unmeasurable};
    let hops = 10..15;
    assert_eq!(
        onset_verdict(&[Missing, Present, Missing], hops.clone()),
        WordVerdict::Present
    );
    assert_eq!(
        onset_verdict(&[Missing, Missing, Missing], hops.clone()),
        WordVerdict::ClippedStart(hops.clone())
    );
    assert_eq!(
        onset_verdict(&[Missing, Ambiguous, Missing], hops.clone()),
        WordVerdict::Ambiguous
    );
    assert_eq!(
        onset_verdict(&[Unmeasurable, Missing, Unmeasurable], hops.clone()),
        WordVerdict::Unmeasurable
    );
    assert_eq!(onset_verdict(&[], hops), WordVerdict::Unmeasurable);
}

fn record(src_diff: Option<f32>, out_diff: Option<f32>, source_frame: i64) -> FrameRecord {
    FrameRecord {
        state: FrameState::Compared,
        part: 0,
        source_frame,
        out: Stats::default(),
        out_diff,
        src: Stats::default(),
        src_diff,
        mad: 0.0,
        corr: None,
    }
}

/// R7: still source frames between two short holds do not join them into
/// one freeze; a real freeze across a short still stays one.
#[test]
fn ja1_s5a_fix2_a_still_source_gap_ends_a_freeze_run() {
    let config = TechnicalScanConfig::default();
    let moving = |out: f32| record(Some(0.05), Some(out), 0);
    let still = record(Some(0.0), Some(0.0), 0);
    // Two held frames, a long still, two held frames: no freeze.
    let mut frames = vec![moving(0.05), moving(0.0), moving(0.0)];
    frames.extend(vec![still; 6]);
    frames.extend([moving(0.0), moving(0.0), moving(0.05)]);
    assert!(freeze_runs(&frames, |_| true, &config).is_empty());
    // Two held, two still, two held: one freeze of four moving frames.
    let mut frames = vec![moving(0.05), moving(0.0), moving(0.0)];
    frames.extend(vec![still; 2]);
    frames.extend([moving(0.0), moving(0.0), moving(0.05)]);
    assert_eq!(freeze_runs(&frames, |_| true, &config), vec![1..7]);
}

fn noise(seed: u64) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x % 10_001) as f32 / 10_000.0
}

/// R8: a 30 fps source on a 60 fps output repeats every source frame. The
/// lag is measured on the new source frames only, so the cadence does not
/// make every second lag a rival.
#[test]
fn ja1_s5a_fix2_an_fps_converted_cut_has_a_video_lag() {
    let config = TechnicalScanConfig::default();
    let clock = Clock::new(kinewright_core::Rational::new(60, 1).unwrap());
    let frames: Vec<FrameRecord> = (0..240_u64)
        .map(|index| {
            // Near-steady motion: counted on every output frame, the 0/m
            // cadence correlates at every even lag and makes rivals.
            let motion = 0.03 + 0.004 * noise(index / 2);
            let diff = if index % 2 == 0 { motion } else { 0.0 };
            let out = diff + 0.000_1 * noise(index + 1_000);
            record(Some(diff), Some(out), i64::try_from(index / 2).unwrap())
        })
        .collect();
    let picture = PictureScan {
        frames,
        truncated: false,
        size: (128, 72),
        color: None,
        time_base: (1, 60),
        decoder: String::new(),
        first_pts: Some(0),
        off_grid: Vec::new(),
        off_grid_count: 0,
        reasons: Vec::new(),
        source_seeks: 0,
        video_streams: 1,
    };
    let lag = video_window_lag(clock, &picture, &(0..4_000_000), &config);
    assert!(lag.is_some_and(|lag| lag.abs() < 0.5), "{lag:?}");
}

/// R7: the source grid is coded like the export's delivery (BT.709 OETF on
/// the scene-linear working frame), so mid grey reads as its code value,
/// not its linear light.
#[test]
fn ja1_s5a_fix2_the_source_grid_is_coded_like_the_export() {
    let (width, height) = (128_u32, 72_u32);
    let grey = half::f16::from_f32(0.18);
    let pixels: Vec<half::f16> = (0..width * height)
        .flat_map(|_| [grey, grey, grey, half::f16::ONE])
        .collect();
    let grid = super::super::grid::from_working(width, height, &pixels).unwrap();
    let code = crate::color_pipeline::encode_bt709(grey.to_f32());
    assert!((code - 0.409).abs() < 0.002, "{code}");
    assert!(
        grid.iter().all(|cell| (cell - code).abs() < 1e-3),
        "{grid:?}"
    );
}
