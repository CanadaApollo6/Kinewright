//! AU6 §3: the source generators for the five named audio workflows.
//!
//! **This module is test support.** It shells out to the provisioned `FFmpeg`
//! CLI through [`crate::test_support::run_ffmpeg`], which **panics** when the
//! binary is missing and when it reports a nonzero exit
//! (`test_support.rs:294-313`), so nothing in production may reach for it. It
//! is `pub` rather than `cfg(test)` because the agent's `tests/mcp_server.rs`
//! and the eval binary both need it and a `cfg(test)` module is invisible
//! across a crate boundary.
//!
//! It is the **one** generator: the media fixtures, the agent end-to-end
//! tests, and `audio-workflow-v7`'s fixture builders all call it, so a level,
//! a turn range or a click frame cannot drift between the three claims made
//! about it.
//!
//! # Rule 11.0.1's source-content exemption
//!
//! Every buffer here is authored **sample by sample in Rust** from
//! [`kinewright_core::au6_scenarios`]' tables and written as exact `wav_f32`
//! bytes through [`GeneratedMedia::from_bytes`] — never `lavfi`, because the
//! provisioned `FFmpeg`'s `sine` emits −18 dBFS and `amix` renormalises
//! (`tests/au5_fixtures.rs:9-12`). AU6 §11.0.1 forbids obtaining an *expected
//! value* by calling the product's meters and explicitly permits *authoring
//! source content* with `test_support::tone` and `pseudo_random_amplitude`,
//! which is all this module does: nothing here is ever compared against the
//! output of a function it called to build the buffer.
//!
//! # The recipe (AU6 §3.2), and why it is the probes' arithmetic verbatim
//!
//! Every (c) number in the contract — the four detected silence spans, the
//! learn range `274..312`, the 31-band regression pin, SNR 2 041 → 3 183, the
//! hum table, the speech loss 101, the de-click drop 258.8 — was measured on
//! the buffers probe-1, probe-2 and probe-3 synthesised
//! (`target/review/au6/probe/probe2-media-tests.rs:72-172`, `:787-831`), and
//! the profile pin is an **exact** `assert_eq!`. The synthesis below is
//! therefore that code reproduced step for step — the same partial spacing,
//! the same Schroeder phase offsets applied as integer sample shifts, the same
//! `f32` summation order (`voice += noise + hum`, then the clicks assigned
//! last) — and not a re-derivation of it. A change to any line of it is a
//! change to every (c) measurement.
//!
//! Mono is authored first and interleaved by [`au6_to_stereo`], AU5's shape
//! (`tests/au5_fixtures.rs:201-208`).

use std::{ops::Range, path::PathBuf};

use kinewright_core::{
    Au6Scenario, Au6Speaker, Au6TrackRole, Au6Turn, MediaAsset, Rational, TimeCode, TrackId,
    TrackKind,
    au6_scenarios::{
        AU6_A_BED_LEVEL_DBFS_HUNDREDTHS, AU6_A_BED_PARTIALS_MILLIHERTZ, AU6_C_CLICK_FRAMES,
        AU6_C_HUM_FUNDAMENTAL_HERTZ, AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS,
        AU6_C_HUM_PARTIAL_AMPLITUDE_RATIO_HUNDREDTHS, AU6_C_HUM_PARTIALS,
        AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS, AU6_C_PROGRAMME_FRAMES, AU6_C_VOICE_CARRIER,
        AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS, AU6_CHANNELS, AU6_CLICK_AMPLITUDE_HUNDREDTHS,
        AU6_CLICK_SAMPLES, AU6_D_ANGLE_NAMES, AU6_D_ANGLE_OFFSETS_FRAMES,
        AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS, AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS,
        AU6_ENCODE_PROGRAMME_FRAMES, AU6_PODCAST_AM_DEPTH_TENTH_DB, AU6_PODCAST_AM_HERTZ,
        AU6_SAMPLE_RATE, AU6_SAMPLES_PER_FRAME, AU6_SOURCE_FPS, AU6_SOURCE_HEIGHT,
        AU6_SOURCE_WIDTH, AU6_TURN_EDGE_FADE_MS, AU6_TURNS, AU6_VOICE_A_BAND_INDEX,
        AU6_VOICE_B_BAND_INDEX, AU6_VOICE_ENVELOPE_HERTZ, AU6_VOICE_PARTIALS, au6_spec,
        au6_turns_of,
    },
};

use crate::{
    cc7_sources::cc7_bt709_limited_source_codes,
    test_support::{GeneratedMedia, pseudo_random_amplitude, tone, wav_f32},
};

/// Sample frames per project frame, `48 000 / 25 = 1 920`, as a `usize`.
const SPF: usize = AU6_SAMPLES_PER_FRAME as usize;

/// One project-frame count as a mono sample count.
const fn samples_for_frames(frames: u32) -> usize {
    frames as usize * SPF
}

/// A dBFS-hundredths level as decibels.
fn level_db(level_dbfs_hundredths: i32) -> f64 {
    f64::from(level_dbfs_hundredths) / 100.0
}

/// Interleave one mono buffer to stereo, AU5's shape
/// (`tests/au5_fixtures.rs:201-208`): every AU6 asset is stereo with identical
/// channels, so a stereo RMS equals the mono RMS.
#[must_use]
pub fn au6_to_stereo(mono: &[f32]) -> Vec<f32> {
    let mut stereo = Vec::with_capacity(mono.len() * usize::from(AU6_CHANNELS));
    for sample in mono {
        stereo.push(*sample);
        stereo.push(*sample);
    }
    stereo
}

/// The left channel of an interleaved stereo buffer — the inverse of
/// [`au6_to_stereo`] for the identical-channel buffers this module writes.
#[must_use]
pub fn au6_to_mono(stereo: &[f32]) -> Vec<f32> {
    stereo.iter().step_by(2).copied().collect()
}

/// `exact_band_center(index) = 1000 · 2^((index − 17)/3)`
/// (`crates/kinewright-media/src/spectrum.rs:225`), for the speaker's band:
/// **1 000 Hz** for A (band 17), **2 000 Hz** for B (band 20).
#[must_use]
pub fn au6_band_center_hertz(speaker: Au6Speaker) -> f64 {
    let index = match speaker {
        Au6Speaker::A => AU6_VOICE_A_BAND_INDEX,
        Au6Speaker::B => AU6_VOICE_B_BAND_INDEX,
    };
    #[allow(clippy::cast_precision_loss)]
    let offset = index as f64 - 17.0;
    1_000.0 * 2.0_f64.powf(offset / 3.0)
}

/// A band-limited multi-partial **Schroeder-phase** carrier
/// (`tests/au5_fixtures.rs:461-494`), `partials` unit-amplitude sinusoids
/// across the third-octave band `[f_c · 2^(−1/6), f_c · 2^(1/6))`, each
/// scaled to `amplitude`, `frames` samples long.
///
/// The phases `phi_p = pi p² / P` are applied as **integer sample shifts** of
/// a one-second-longer tone, exactly as the probes did: a linear phase ramp
/// across equally spaced partials is a pure delay that realigns into a pulse
/// train and swings the per-window band level by 25 dB; the quadratic ramp
/// makes the sum a constant-envelope chirp so every window carries the same
/// power.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn schroeder_carrier(
    center_hertz: f64,
    partials: usize,
    amplitude: f32,
    frames: usize,
) -> Vec<f32> {
    let low = center_hertz * 2.0_f64.powf(-1.0 / 6.0);
    let high = center_hertz * 2.0_f64.powf(1.0 / 6.0);
    let mut mono = vec![0.0_f32; frames];
    for partial in 0..partials {
        let position = (partial as f64 + 0.5) / partials as f64;
        let hertz = low + (high - low) * position;
        let phase_offset = ((partial * partial) as f64 / (2 * partials) as f64).fract();
        let samples = tone(
            hertz,
            amplitude,
            AU6_SAMPLE_RATE,
            frames + AU6_SAMPLE_RATE as usize,
        );
        let shift = (phase_offset * f64::from(AU6_SAMPLE_RATE) / hertz) as usize;
        for (index, slot) in mono.iter_mut().enumerate() {
            *slot += samples[index + shift];
        }
    }
    mono
}

/// The partial amplitude that puts a `partials`-partial carrier at `level_db`
/// **RMS** after multiplication by an envelope of mean square `envelope_ms`:
/// `A = sqrt(2·10^(L/10) / (P·envelope_ms))` (AU6 §3.2 rule 1).
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn carrier_amplitude(level_db: f64, partials: usize, envelope_ms: f64) -> f32 {
    let target_power = 10.0_f64.powf(level_db / 10.0);
    (2.0 * target_power / (partials as f64 * envelope_ms)).sqrt() as f32
}

/// The 4 Hz raised-cosine syllabic burst train, starting and ending at zero;
/// its mean square over whole cycles is exactly 3/8.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn syllabic_envelope(frames: usize, hertz: f64) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            let t = n as f64 / f64::from(AU6_SAMPLE_RATE);
            (0.5 - 0.5 * (2.0 * std::f64::consts::PI * hertz * t).cos()) as f32
        })
        .collect()
}

/// The mean square of the 4 Hz raised-cosine envelope, `3/8`.
const SYLLABIC_MEAN_SQUARE: f64 = 0.375;

/// The 20 ms raised-cosine fade at both ends of a turn
/// (`AU6_TURN_EDGE_FADE_MS`), so a turn boundary is not a step the dynamics
/// processors have to recover from. It is applied to the **steady** and
/// **ride** turns, whose envelopes do not vanish at the edges; the syllabic
/// envelope is itself zero at both ends of every turn (an integer number of
/// 4 Hz cycles), which is how all three probes authored it and what every (c)
/// number was measured on.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn apply_edge_fades(buffer: &mut [f32], fade_samples: usize) {
    let len = buffer.len();
    for i in 0..fade_samples.min(len / 2) {
        let gain =
            (0.5 - 0.5 * (std::f64::consts::PI * i as f64 / fade_samples as f64).cos()) as f32;
        buffer[i] *= gain;
        buffer[len - 1 - i] *= gain;
    }
}

/// `AU6_TURN_EDGE_FADE_MS` in samples.
const fn edge_fade_samples() -> usize {
    (AU6_SAMPLE_RATE * AU6_TURN_EDGE_FADE_MS / 1_000) as usize
}

/// The log-domain level ride: 0 dB at the peak, `−depth_db` at the trough,
/// starting and ending at the trough.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn ride_envelope(frames: usize, hertz: f64, depth_db: f64) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            let t = n as f64 / f64::from(AU6_SAMPLE_RATE);
            let phase = (2.0 * std::f64::consts::PI * hertz * t).cos();
            let level = -depth_db * 0.5 * (1.0 + phase);
            10.0_f64.powf(level / 20.0) as f32
        })
        .collect()
}

/// How one turn's carrier is shaped in time.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TurnShape {
    /// The 4 Hz raised-cosine burst train ((a), (c), (d)'s master).
    Syllabic,
    /// Unmodulated, with the 20 ms edge fades ((b)'s Voice A).
    Steady,
    /// The log-domain ride at `am_hertz` over `depth_db`, edge fades ((b)'s
    /// Voice B).
    Ride { am_hertz: f64, depth_db: f64 },
}

/// Paint `turns` of enveloped carrier into a silent `frames`-long mono buffer;
/// outside a turn the sample is exactly `0.0`.
fn utterances(
    speaker: Au6Speaker,
    turns: &[Range<TimeCode>],
    level_dbfs_hundredths: i32,
    frames: u32,
    shape: TurnShape,
) -> Vec<f32> {
    let partials = AU6_VOICE_PARTIALS as usize;
    let center = au6_band_center_hertz(speaker);
    let envelope_ms = match shape {
        TurnShape::Syllabic => SYLLABIC_MEAN_SQUARE,
        TurnShape::Steady => 1.0,
        TurnShape::Ride { am_hertz, depth_db } => {
            // The envelope's mean square, computed once over one cycle.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cycle = (f64::from(AU6_SAMPLE_RATE) / am_hertz) as usize;
            #[allow(clippy::cast_precision_loss)]
            let mean = ride_envelope(cycle, am_hertz, depth_db)
                .iter()
                .map(|e| f64::from(*e) * f64::from(*e))
                .sum::<f64>()
                / cycle as f64;
            mean
        }
    };
    let amplitude = carrier_amplitude(level_db(level_dbfs_hundredths), partials, envelope_ms);
    let mut mono = vec![0.0_f32; samples_for_frames(frames)];
    for turn in turns {
        let start = usize::try_from(turn.start.0).expect("a turn starts at or after frame 0") * SPF;
        let len =
            usize::try_from(turn.end.0 - turn.start.0).expect("a turn has a positive length") * SPF;
        assert!(
            start + len <= mono.len(),
            "turn {turn:?} runs past the {frames}-frame buffer"
        );
        let carrier = schroeder_carrier(center, partials, amplitude, len);
        match shape {
            TurnShape::Syllabic => {
                let envelope = syllabic_envelope(len, f64::from(AU6_VOICE_ENVELOPE_HERTZ));
                for (i, sample) in carrier.iter().enumerate() {
                    mono[start + i] += *sample * envelope[i];
                }
            }
            TurnShape::Steady => {
                let mut turn_buffer = carrier;
                apply_edge_fades(&mut turn_buffer, edge_fade_samples());
                for (i, sample) in turn_buffer.iter().enumerate() {
                    mono[start + i] += *sample;
                }
            }
            TurnShape::Ride { am_hertz, depth_db } => {
                let envelope = ride_envelope(len, am_hertz, depth_db);
                let mut turn_buffer: Vec<f32> = carrier
                    .iter()
                    .enumerate()
                    .map(|(i, sample)| *sample * envelope[i])
                    .collect();
                apply_edge_fades(&mut turn_buffer, edge_fade_samples());
                for (i, sample) in turn_buffer.iter().enumerate() {
                    mono[start + i] += *sample;
                }
            }
        }
    }
    mono
}

/// AU6 §3.2 rule 1: `speaker`'s 64-partial Schroeder carrier under the 4 Hz
/// raised-cosine syllabic envelope on **the speaker's own turns** of
/// [`AU6_TURNS`], at `level_dbfs_hundredths` RMS over a turn, `frames` long.
/// Outside a turn the sample is exactly `0.0`, so a gap on a voice track is
/// digital silence. Mono.
#[must_use]
pub fn au6_voice_pcm(speaker: Au6Speaker, level_dbfs_hundredths: i32, frames: u32) -> Vec<f32> {
    au6_voice_on_turns_pcm(
        speaker,
        &au6_turns_of(speaker),
        level_dbfs_hundredths,
        frames,
    )
}

/// [`au6_voice_pcm`] on an explicit turn list: (c)'s dialogue is speaker A's
/// carrier on **all four** turns (N4 S11), and (d)'s master is A on A's turns
/// plus B on B's. Mono.
#[must_use]
pub fn au6_voice_on_turns_pcm(
    speaker: Au6Speaker,
    turns: &[Range<TimeCode>],
    level_dbfs_hundredths: i32,
    frames: u32,
) -> Vec<f32> {
    utterances(
        speaker,
        turns,
        level_dbfs_hundredths,
        frames,
        TurnShape::Syllabic,
    )
}

/// (b)'s Voice A (N4 S11): speaker A's carrier on A's turns, **steady** — no
/// syllabic envelope, the 20 ms edge fades only — at `level_dbfs_hundredths`
/// RMS over a turn. Mono.
#[must_use]
pub fn au6_steady_voice_pcm(
    speaker: Au6Speaker,
    level_dbfs_hundredths: i32,
    frames: u32,
) -> Vec<f32> {
    utterances(
        speaker,
        &au6_turns_of(speaker),
        level_dbfs_hundredths,
        frames,
        TurnShape::Steady,
    )
}

/// AU6 §3.2 rule 2: (b)'s speaker B — B's carrier on B's turns under a
/// **log-domain level ride** at `am_hertz` over `depth_tenth_db`, trough at
/// both ends, with the 20 ms edge fades; `level_dbfs_hundredths` is the RMS
/// over a turn with the ride's mean square factored in, i.e. the authored
/// **ride peak** level of §2.4. Not "syllabic": at 4 Hz a 200 ms window
/// averages the modulation away (§0.2 item 5). Mono.
#[must_use]
pub fn au6_ride_pcm(
    level_dbfs_hundredths: i32,
    am_hertz: u32,
    depth_tenth_db: i32,
    frames: u32,
) -> Vec<f32> {
    utterances(
        Au6Speaker::B,
        &au6_turns_of(Au6Speaker::B),
        level_dbfs_hundredths,
        frames,
        TurnShape::Ride {
            am_hertz: f64::from(am_hertz),
            depth_db: f64::from(depth_tenth_db) / 10.0,
        },
    )
}

/// AU6 §3.2 rule 3: the steady six-partial chord
/// ([`AU6_A_BED_PARTIALS_MILLIHERTZ`]) with a quadratic phase spread,
/// continuous over the whole buffer, at `level_dbfs_hundredths` RMS. Constant
/// material makes a windowed delta attributable (AU4's lesson). Mono.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn au6_chord_bed_pcm(level_dbfs_hundredths: i32, frames: u32) -> Vec<f32> {
    let partials = AU6_A_BED_PARTIALS_MILLIHERTZ.map(|millihertz| millihertz as f64 / 1_000.0);
    let count = partials.len();
    let amplitude =
        (2.0 * 10.0_f64.powf(level_db(level_dbfs_hundredths) / 10.0) / count as f64).sqrt() as f32;
    let total = samples_for_frames(frames);
    let mut mono = vec![0.0_f32; total];
    for (index, hertz) in partials.iter().enumerate() {
        // A fixed integer phase spread so the crest never stacks.
        let phase_offset = ((index * index) as f64 / (2 * count) as f64).fract();
        let samples = tone(
            *hertz,
            amplitude,
            AU6_SAMPLE_RATE,
            total + AU6_SAMPLE_RATE as usize,
        );
        let shift = (phase_offset * f64::from(AU6_SAMPLE_RATE) / hertz) as usize;
        for (i, slot) in mono.iter_mut().enumerate() {
            *slot += samples[i + shift];
        }
    }
    mono
}

/// The `pseudo_random_amplitude` amplitude that puts the seeded xorshift64
/// buffer — uniform on `[−a, a)`, RMS exactly `a/√3` — at `level_dbfs_hundredths`.
#[allow(clippy::cast_possible_truncation)]
fn noise_amplitude(level_dbfs_hundredths: i32) -> f32 {
    (10.0_f64.powf(level_db(level_dbfs_hundredths) / 20.0) * 3.0_f64.sqrt()) as f32
}

/// AU6 §3.2 rule 4: `test_support::pseudo_random_amplitude` at
/// `level_dbfs_hundredths` RMS — the seeded xorshift64, no `rand`, no clock,
/// no OS entropy. Mono, `frames` project frames long.
#[must_use]
pub fn au6_noise_pcm(level_dbfs_hundredths: i32, frames: u32) -> Vec<f32> {
    pseudo_random_amplitude(
        samples_for_frames(frames),
        noise_amplitude(level_dbfs_hundredths),
    )
}

/// AU6 §3.2 rule 5: `fundamental_hertz` plus `harmonics − 1` partials, each
/// **half the amplitude** of the one below it
/// (`AU6_C_HUM_PARTIAL_AMPLITUDE_RATIO_HUNDREDTHS`, AU5's `hum_fixture` shape
/// and probe-2's `hum()`), totalling `level_dbfs_hundredths` RMS. Mono.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
pub fn au6_hum_pcm(
    level_dbfs_hundredths: i32,
    fundamental_hertz: f64,
    harmonics: usize,
    frames: u32,
) -> Vec<f32> {
    let ratio = AU6_C_HUM_PARTIAL_AMPLITUDE_RATIO_HUNDREDTHS as f64 / 100.0;
    let weights: Vec<f64> = (0..harmonics).map(|h| ratio.powi(h as i32)).collect();
    let sum_squares: f64 = weights.iter().map(|w| w * w).sum();
    let amplitude =
        (2.0 * 10.0_f64.powf(level_db(level_dbfs_hundredths) / 10.0) / sum_squares).sqrt();
    let total = samples_for_frames(frames);
    let mut mono = vec![0.0_f32; total];
    for (index, weight) in weights.iter().enumerate() {
        let hertz = fundamental_hertz * (index as f64 + 1.0);
        let samples = tone(hertz, (amplitude * weight) as f32, AU6_SAMPLE_RATE, total);
        for (i, sample) in samples.iter().enumerate() {
            mono[i] += *sample;
        }
    }
    mono
}

/// AU6 §3.2 rule 6 (A20): write one click at each of `frames`, **assigned** —
/// not added — after noise and hum: `AU6_CLICK_SAMPLES = 2` samples at the
/// first sample of the frame, `+AU6_CLICK_AMPLITUDE_HUNDREDTHS / 100` then
/// `−…`, i.e. `+0.5` then `−0.5`. Mono; the stereo interleave puts the same
/// two samples on both channels.
///
/// AU5's 8-frame sign-alternating click is the wrong model here: at its ±0.9
/// the three clicked gaps read −36.39 dBFS, 139 hundredths *under* the −35.00
/// threshold, failing rule 3's 250 floor; at ±0.5 they clear it at only
/// 1.46×. The 2-sample click's `d2` is `4 × 0.5 = 2.0` against a 20 ms
/// reference of ≈ 7e-3 RMS at the −42 dBFS floor, an 18× clearance, and
/// `click_count` reads 12 exactly on every probe run.
///
/// # Panics
///
/// Panics when a click frame is negative.
pub fn au6_clicks_into(mono: &mut [f32], frames: &[i64]) {
    let amplitude = f64::from(AU6_CLICK_AMPLITUDE_HUNDREDTHS) / 100.0;
    #[allow(clippy::cast_possible_truncation)]
    let amplitude = amplitude as f32;
    for frame in frames {
        let index = usize::try_from(*frame).expect("a click frame is not negative") * SPF;
        for sample in 0..AU6_CLICK_SAMPLES as usize {
            let sign = if sample % 2 == 0 { 1.0 } else { -1.0 };
            mono[index + sample] = amplitude * sign;
        }
    }
}

/// AU6 §3.2 rule 7: a camera's scratch take of `master` — the master delayed
/// by `offset_frames`, **halved**, plus `pseudo_random_amplitude` noise at
/// `noise_level_dbfs_hundredths`. Mono in, mono out, the master's length.
///
/// # Panics
///
/// Panics when `offset_frames` is negative.
#[must_use]
pub fn au6_scratch_pcm(
    master: &[f32],
    offset_frames: i64,
    noise_level_dbfs_hundredths: i32,
) -> Vec<f32> {
    let shift = usize::try_from(offset_frames).expect("an angle offset is not negative") * SPF;
    let mut scratch = vec![0.0_f32; master.len()];
    for i in shift..master.len() {
        scratch[i] = master[i - shift] * 0.5;
    }
    let noise = pseudo_random_amplitude(master.len(), noise_amplitude(noise_level_dbfs_hundredths));
    for (slot, sample) in scratch.iter_mut().zip(&noise) {
        *slot += *sample;
    }
    scratch
}

/// The four turns of [`AU6_TURNS`] as project ranges, the turn list (c)'s
/// single voice speaks on.
fn all_turns() -> Vec<Range<TimeCode>> {
    AU6_TURNS.iter().map(Au6Turn::range).collect()
}

/// (c)'s **clean** voice — speaker A's carrier on all four turns at
/// `AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS`, no noise, no hum, no click —
/// interleaved stereo. Probe-3's `clean` reference, kept for the in-chain
/// de-click contribution the manifest records as evidence (A21).
#[must_use]
pub fn au6_c_voice_only_track() -> Vec<f32> {
    au6_to_stereo(&au6_c_voice_only_mono())
}

fn au6_c_voice_only_mono() -> Vec<f32> {
    au6_voice_on_turns_pcm(
        AU6_C_VOICE_CARRIER,
        &all_turns(),
        AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS,
        AU6_C_PROGRAMME_FRAMES,
    )
}

/// (c)'s A1 buffer **before** [`au6_clicks_into`] — voice + noise + hum,
/// summed as `voice += noise + hum` in that `f32` order — as mono.
fn au6_c_click_free_mono() -> Vec<f32> {
    let mut mono = au6_c_voice_only_mono();
    let noise = au6_noise_pcm(AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS, AU6_C_PROGRAMME_FRAMES);
    let hum = au6_hum_pcm(
        AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS,
        f64::from(AU6_C_HUM_FUNDAMENTAL_HERTZ),
        AU6_C_HUM_PARTIALS as usize,
        AU6_C_PROGRAMME_FRAMES,
    );
    for i in 0..mono.len() {
        mono[i] += noise[i] + hum[i];
    }
    mono
}

/// A21: (c)'s A1 buffer BEFORE `au6_clicks_into` — voice + noise + hum,
/// interleaved stereo — so `au6_scenario_tracks(LocationDialogue)` equals this
/// plus the clicks by construction (§3.2 rule 8). AU5's de-click instrument
/// measures against exactly this buffer.
#[must_use]
pub fn au6_c_click_free_track() -> Vec<f32> {
    au6_to_stereo(&au6_c_click_free_mono())
}

/// (c)'s degraded A1 buffer: [`au6_c_click_free_track`] with the twelve
/// clicks of [`AU6_C_CLICK_FRAMES`] written last.
fn au6_c_degraded_mono() -> Vec<f32> {
    let mut mono = au6_c_click_free_mono();
    au6_clicks_into(&mut mono, &AU6_C_CLICK_FRAMES);
    mono
}

/// (d)'s master audio: speaker A's syllabic carrier on A's turns plus speaker
/// B's on B's, each at `AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS` RMS over a turn —
/// one voice across all four turn slots, probe-1's construction. Mono.
#[must_use]
pub fn au6_d_master_pcm(frames: u32) -> Vec<f32> {
    let mut mono = au6_voice_pcm(Au6Speaker::A, AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS, frames);
    for (slot, sample) in mono.iter_mut().zip(au6_voice_pcm(
        Au6Speaker::B,
        AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS,
        frames,
    )) {
        *slot += sample;
    }
    mono
}

/// The scenario whose buffers `scenario` authors: itself, or the scenario it
/// reuses ((e) reuses (a)'s assets at their own 300 frames, B2).
fn source_scenario(scenario: Au6Scenario) -> Au6Scenario {
    au6_spec(scenario).document_source.unwrap_or(scenario)
}

/// AU6 §3.1's `au6_scenario_tracks`: one **interleaved stereo** buffer per
/// audio track of the scenario's base document, in track order, tagged with
/// its role. Picture and angle tracks carry no buffer and are absent. Every
/// buffer is `asset_frames` long — (e) returns (a)'s 300-frame buffers
/// byte-identically, because its 8 s document is a clip truncation, not a
/// shorter asset (B2).
///
/// # Panics
///
/// Panics when an audio track is missing its authored level or carrier, or
/// when `scenario` authors no buffer for a requested role.
#[must_use]
pub fn au6_scenario_tracks(scenario: Au6Scenario) -> Vec<(Au6TrackRole, Vec<f32>)> {
    let spec = au6_spec(source_scenario(scenario));
    let frames = spec.asset_frames;
    let mut tracks = Vec::new();
    let mut master: Option<Vec<f32>> = None;
    let mut scratch_index = 0_usize;
    for track in spec.tracks {
        if track.kind != TrackKind::Audio {
            continue;
        }
        let level = track
            .level_dbfs_hundredths
            .expect("every audio track carries an authored level");
        let mono = match (spec.scenario, track.role) {
            (Au6Scenario::Interview, Au6TrackRole::VoiceA | Au6TrackRole::VoiceB) => au6_voice_pcm(
                track.carrier.expect("a voice track names its carrier"),
                level,
                frames,
            ),
            (Au6Scenario::Interview, Au6TrackRole::MusicBed) => au6_chord_bed_pcm(level, frames),
            (Au6Scenario::Podcast, Au6TrackRole::VoiceA) => au6_steady_voice_pcm(
                track.carrier.expect("a voice track names its carrier"),
                level,
                frames,
            ),
            (Au6Scenario::Podcast, Au6TrackRole::VoiceB) => au6_ride_pcm(
                level,
                AU6_PODCAST_AM_HERTZ,
                AU6_PODCAST_AM_DEPTH_TENTH_DB,
                frames,
            ),
            (Au6Scenario::LocationDialogue, Au6TrackRole::Dialogue) => au6_c_degraded_mono(),
            (Au6Scenario::Multicam, Au6TrackRole::Scratch) => {
                let master = master.get_or_insert_with(|| au6_d_master_pcm(frames));
                let offset = AU6_D_ANGLE_OFFSETS_FRAMES[scratch_index];
                scratch_index += 1;
                au6_scratch_pcm(master, offset, AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS)
            }
            (Au6Scenario::Multicam, Au6TrackRole::MasterAudio) => {
                master.take().unwrap_or_else(|| au6_d_master_pcm(frames))
            }
            (scenario, role) => panic!("{scenario:?} authors no {role:?} track"),
        };
        tracks.push((track.role, au6_to_stereo(&mono)));
    }
    tracks
}

/// The `.wav` label one track's asset is written under.
fn track_label(scenario: Au6Scenario, role: Au6TrackRole, track: TrackId) -> String {
    let role = match role {
        Au6TrackRole::Picture => "picture",
        Au6TrackRole::VoiceA => "voice-a",
        Au6TrackRole::VoiceB => "voice-b",
        Au6TrackRole::MusicBed => "bed",
        Au6TrackRole::Dialogue => "dialogue",
        Au6TrackRole::Angle => "angle",
        Au6TrackRole::Scratch => "scratch",
        Au6TrackRole::MasterAudio => "master",
    };
    format!("au6-{}-{role}-{}", au6_spec(scenario).id, track.0)
}

/// One interleaved stereo buffer as an exact IEEE-float `.wav`.
fn wav_media(label: &str, stereo: &[f32]) -> GeneratedMedia {
    GeneratedMedia::from_bytes(
        label,
        "wav",
        &wav_f32(stereo, AU6_SAMPLE_RATE, AU6_CHANNELS),
    )
}

/// AU6 §3.1's `au6_source`: the generated file for the **first** track of
/// `scenario` carrying `role` — a `.wav` for an audio role, the video-only
/// `.mkv` for `Picture` and `Angle`. (d) has two `Scratch` and two `Angle`
/// tracks; [`au6_source_for_track`] names one by track id.
///
/// # Panics
///
/// Panics when the scenario has no track of that role, or as
/// [`crate::test_support::run_ffmpeg`] does.
#[must_use]
pub fn au6_source(scenario: Au6Scenario, role: Au6TrackRole) -> GeneratedMedia {
    let track = au6_spec(scenario)
        .tracks
        .iter()
        .find(|track| track.role == role)
        .unwrap_or_else(|| panic!("{scenario:?} has no {role:?} track"))
        .track;
    au6_source_for_track(scenario, track)
}

/// [`au6_source`] by track id.
///
/// # Panics
///
/// Panics when the scenario has no such track, or as
/// [`crate::test_support::run_ffmpeg`] does.
#[must_use]
pub fn au6_source_for_track(scenario: Au6Scenario, track: TrackId) -> GeneratedMedia {
    let spec = au6_spec(scenario);
    let entry = spec
        .tracks
        .iter()
        .find(|entry| entry.track == track)
        .unwrap_or_else(|| panic!("{scenario:?} has no track {}", track.0));
    match entry.role {
        Au6TrackRole::Picture => au6_picture_source(spec.asset_frames),
        Au6TrackRole::Angle => {
            let angle = spec
                .tracks
                .iter()
                .filter(|candidate| candidate.role == Au6TrackRole::Angle)
                .position(|candidate| candidate.track == track)
                .expect("the angle track is one of the angle tracks");
            au6_angle_source(angle + 1)
        }
        role => {
            let audio_index = spec
                .tracks
                .iter()
                .filter(|candidate| candidate.kind == TrackKind::Audio)
                .position(|candidate| candidate.track == track)
                .expect("the audio track is one of the audio tracks");
            let (tagged, stereo) = au6_scenario_tracks(scenario)
                .into_iter()
                .nth(audio_index)
                .expect("au6_scenario_tracks returns one buffer per audio track");
            assert_eq!(tagged, role);
            wav_media(&track_label(scenario, role, track), &stereo)
        }
    }
}

/// AU6 §3.1's `au6_scenario_sources`: one generated file per base-document
/// asset, **in track order** (every scenario's asset ids follow its track ids
/// one to one), so `sources[i]` is the asset of `spec.tracks[i]`.
///
/// # Panics
///
/// Panics as [`crate::test_support::run_ffmpeg`] does.
#[must_use]
pub fn au6_scenario_sources(scenario: Au6Scenario) -> Vec<GeneratedMedia> {
    let spec = au6_spec(scenario);
    let buffers = au6_scenario_tracks(scenario);
    let mut audio = buffers.into_iter();
    let mut angle = 0_usize;
    spec.tracks
        .iter()
        .map(|track| match track.role {
            Au6TrackRole::Picture => au6_picture_source(spec.asset_frames),
            Au6TrackRole::Angle => {
                angle += 1;
                au6_angle_source(angle)
            }
            role => {
                let (tagged, stereo) = audio
                    .next()
                    .expect("au6_scenario_tracks returns one buffer per audio track");
                assert_eq!(tagged, role);
                wav_media(&track_label(scenario, role, track.track), &stereo)
            }
        })
        .collect()
}

/// The display code of the flat picture (a)/(e) carry on V1.
const PICTURE_DISPLAY_CODE: u8 = 118;
/// The display codes of (d)'s two angles: distinguishable flat greys.
const ANGLE_DISPLAY_CODES: [u8; 2] = [64, 172];

/// A temp file that is removed when it goes out of scope, **including on a
/// panic** — `RawFrames`' shape (`cc7_sources.rs:449-487`).
///
/// `run_ffmpeg` panics on a missing binary and on a nonzero exit, so a plain
/// `remove_file` after the call is unreachable on exactly the path that
/// leaks: at 8 s × 25 fps of yuv444p 320×180 the `.yuv` is ≈ 34.6 MB, and a
/// 300-frame one ≈ 51.8 MB.
struct RawInput {
    path: PathBuf,
}

impl RawInput {
    /// Write `bytes` to a uniquely named `.{extension}` beside the other temp
    /// media.
    fn write(label: &str, extension: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "{label}-{}-{}.{extension}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the system clock follows the Unix epoch")
                .as_nanos()
        ));
        std::fs::write(&path, bytes).expect("the raw AU6 source should write");
        Self { path }
    }

    /// The path, as `FFmpeg`'s `-i` argument.
    fn input(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for RawInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// `frames` frames of one flat BT.709 limited-range grey as `yuv444p` planes
/// — the picture carries no claim, so a flat raster through CC7's own
/// independently transcribed forward matrix is all it needs to be.
fn flat_planes(display_code: u8, frames: u32) -> Vec<u8> {
    let count = (AU6_SOURCE_WIDTH * AU6_SOURCE_HEIGHT) as usize;
    let codes = cc7_bt709_limited_source_codes([display_code; 3]);
    let mut frame = Vec::with_capacity(count * 3);
    for code in codes {
        frame.extend(std::iter::repeat_n(code, count));
    }
    frame.repeat(frames as usize)
}

/// The placeholder for the raw `.yuv` input in the two recipes below.
pub const AU6_RECIPE_YUV_INPUT: &str = "<temp.yuv>";
/// The placeholder for the raw `.wav` input in the mux recipe.
pub const AU6_RECIPE_WAV_INPUT: &str = "<temp.wav>";

/// AU6 §3.3: `cc7_source`'s recipe unchanged — **no second input, no
/// `-c:a`** — the argument vector of every video-only source, with the raw
/// input as [`AU6_RECIPE_YUV_INPUT`]. Recorded verbatim in the manifest.
pub const AU6_VIDEO_ONLY_RECIPE: [&str; 28] = [
    "-f",
    "rawvideo",
    "-pix_fmt",
    "yuv444p",
    "-s",
    "320x180",
    "-r",
    "25",
    "-i",
    AU6_RECIPE_YUV_INPUT,
    "-vf",
    "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
    "-c:v",
    "ffv1",
    "-level",
    "3",
    "-g",
    "1",
    "-pix_fmt",
    "yuv444p",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-colorspace",
    "bt709",
    "-color_range",
    "tv",
];

/// AU6 §3.3: the one audio-carrying mux — CC7's recipe with a second input
/// and `pcm_s16le` appended (E16). `pcm_s16le` is canonical because it is
/// what AU3's `lane_media` already writes; `pcm_f32le` and `pcm_s24le` were
/// measured as working alternatives, the first being the one to pick if a
/// later slice needs bit-exactness with the authored buffer.
pub const AU6_MUX_RECIPE: [&str; 33] = [
    "-f",
    "rawvideo",
    "-pix_fmt",
    "yuv444p",
    "-s",
    "320x180",
    "-r",
    "25",
    "-i",
    AU6_RECIPE_YUV_INPUT,
    "-i",
    AU6_RECIPE_WAV_INPUT,
    "-vf",
    "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
    "-c:v",
    "ffv1",
    "-level",
    "3",
    "-g",
    "1",
    "-pix_fmt",
    "yuv444p",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-colorspace",
    "bt709",
    "-color_range",
    "tv",
    "-c:a",
    "pcm_s16le",
    "-shortest",
];

const _: () = assert!(
    AU6_SOURCE_WIDTH == 320 && AU6_SOURCE_HEIGHT == 180 && AU6_SOURCE_FPS == 25,
    "the recipes' `-s 320x180 -r 25` restate AU6 §2.3's raster and rate"
);

/// A recipe with its placeholders bound to real paths.
fn bind_recipe(recipe: &[&str], yuv: &str, wav: Option<&str>) -> Vec<String> {
    recipe
        .iter()
        .map(|argument| match *argument {
            AU6_RECIPE_YUV_INPUT => yuv.to_owned(),
            AU6_RECIPE_WAV_INPUT => wav
                .expect("the video-only recipe names no wav input")
                .to_owned(),
            other => other.to_owned(),
        })
        .collect()
}

/// One flat raster through [`AU6_VIDEO_ONLY_RECIPE`].
fn video_only_source(label: &str, display_code: u8, frames: u32) -> GeneratedMedia {
    let raw = RawInput::write(label, "yuv", &flat_planes(display_code, frames));
    let arguments = bind_recipe(&AU6_VIDEO_ONLY_RECIPE, &raw.input(), None);
    let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    GeneratedMedia::ffmpeg(label, &arguments, "mkv")
    // `raw` drops here, removing the `.yuv` — on a `run_ffmpeg` panic too.
}

/// AU6 §3.1's `au6_picture_source`: the **video-only** picture (a) carries on
/// V1 and (e) reuses (B1), `frames` long at 25 fps, `MediaKind::Video` — no
/// PCM at all, because `export.rs` does not filter the mix by track kind and
/// any audio in V1's asset would join the (a) mix and move every row.
///
/// # Panics
///
/// Panics as [`crate::test_support::run_ffmpeg`] does.
#[must_use]
pub fn au6_picture_source(frames: u32) -> GeneratedMedia {
    video_only_source("au6-picture", PICTURE_DISPLAY_CODE, frames)
}

/// AU6 §3.1's `au6_angle_source`: (d)'s **video-only** angle `1` or `2`
/// (`AU6_D_ANGLE_NAMES` order), 300 frames, `MediaKind::Video` (B6) — the
/// scratch audio is a separate `.wav` on A1/A2, because a picture asset that
/// carried it would contribute it to the mix regardless of the mute.
///
/// # Panics
///
/// Panics when `angle` is not 1 or 2, or as
/// [`crate::test_support::run_ffmpeg`] does.
#[must_use]
pub fn au6_angle_source(angle: usize) -> GeneratedMedia {
    assert!(
        (1..=AU6_D_ANGLE_NAMES.len()).contains(&angle),
        "(d) has angles 1 and 2, not {angle}"
    );
    video_only_source(
        &format!("au6-d-angle-{angle}"),
        ANGLE_DISPLAY_CODES[angle - 1],
        au6_spec(Au6Scenario::Multicam).asset_frames,
    )
}

/// AU6 §3.1's `au6_muxed_source`: the **one** audio-carrying source — the
/// flat picture with (a)'s chord bed as `pcm_s16le` through
/// [`AU6_MUX_RECIPE`], `AU6_ENCODE_PROGRAMME_FRAMES` long. It exists so the
/// programme's first audio-in-picture mux is proven once (§3.2 rule 7, E16)
/// and is available to a later slice; **it appears in no AU6 document** (B1).
///
/// # Panics
///
/// Panics as [`crate::test_support::run_ffmpeg`] does.
#[must_use]
pub fn au6_muxed_source() -> GeneratedMedia {
    let frames = AU6_ENCODE_PROGRAMME_FRAMES;
    let raw = RawInput::write("au6-mux", "yuv", &flat_planes(PICTURE_DISPLAY_CODE, frames));
    let bed = au6_to_stereo(&au6_chord_bed_pcm(AU6_A_BED_LEVEL_DBFS_HUNDREDTHS, frames));
    let wav = RawInput::write(
        "au6-mux",
        "wav",
        &wav_f32(&bed, AU6_SAMPLE_RATE, AU6_CHANNELS),
    );
    let arguments = bind_recipe(&AU6_MUX_RECIPE, &raw.input(), Some(&wav.input()));
    let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    GeneratedMedia::ffmpeg("au6-mux", &arguments, "mkv")
    // Both guards drop here, on success and on a `run_ffmpeg` panic alike.
}

/// The RMS of `samples` in dBFS hundredths, `round(2000 · log10(rms))` — the
/// analytic side of §3.2 rule 1, computed on the authored buffer, never on a
/// meter. Mono and identical-channel stereo read the same.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn au6_analytic_level_dbfs_hundredths(samples: &[f32]) -> i32 {
    let rms = crate::test_support::rms(samples);
    (2_000.0 * rms.log10()).round() as i32
}

/// A11/E11: an audio-only WAV probes at `Rational::default()` = 30 fps, so
/// every asset is re-stamped onto the project grid or `validate()` refuses
/// with `IncorrectDocumentDuration`. Sets `fps` and `duration`; nothing else
/// moves.
#[must_use]
pub fn au6_stamp_on_project_grid(
    mut asset: MediaAsset,
    fps: Rational,
    frames: TimeCode,
) -> MediaAsset {
    asset.fps = fps;
    asset.duration = frames;
    asset
}
