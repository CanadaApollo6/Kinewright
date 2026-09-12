//! AU6 §2: the scenario authority for the five named audio workflows.
//!
//! This module is the **single** place an AU6 geometry constant, authored
//! turn range, source level, committed detector range, canonical operation,
//! export job or budget threshold is written down. Five scenarios times three
//! execution paths (media fixture, scripted agent, person builders) times two
//! crates that also read them (agent eval, app) is seven places a number could
//! drift, so AU6 §2.1 forbids restating one of these values as a literal
//! anywhere else in the workspace.
//!
//! It is **data and arithmetic only** (AU6 §2.1): no `Document` mutation, no
//! rendering, no filesystem, no clock, no RNG, no PCM. Two evaluations of any
//! function here produce identical values on both CI operating systems (AU6
//! §10.11). Every *stored* quantity is an integer in the unit its identifier
//! names (AU6 §10.1); the one place `f64` appears is inside
//! [`au6_c_analytic_mean_band_tenth_db`], whose result is an integer and whose
//! arithmetic is the analytic bound AU6 §2.5(ii) states.
//!
//! # What this module is not (AU6 §2.9)
//!
//! It is not a renderer, not a fixture and not a test helper: it holds no
//! `Analysis`, spawns no `Core`, reads no file and authors no PCM. It does not
//! re-implement `mix_levels`, `mix_window_levels`, `mix_noise_profile`,
//! `audio_qc`, `audio_repair`, the loudness meter, the limiter, or any
//! planner's arithmetic.
//!
//! # The three regression pins (AU6 §11.0.1)
//!
//! [`AU6_A_DUCK_KEYFRAMES`], [`AU6_C_LEARN_PROJECT_RANGE`] and
//! [`AU6_C_LEARNED_PROFILE_TENTH_DB`] are **committed output of a shipped
//! detector or planner**, transcribed once from a measured run at `11a6098`,
//! and labelled as regression pins in their doc comments. Their claim is
//! "the product still does what it did when this was measured", never "this
//! is what the authored material implies".
//!
//! # Restated constants carry their owner (AU6 §2.8)
//!
//! `crates/kinewright-core/Cargo.toml` has **no path dependency on
//! `kinewright-media`**, so the silence threshold, the noise-profile minimum
//! and the loudness gating block are restated here with owner, file and line,
//! and cross-checked from the media side by
//! `au6_restated_constants_agree_with_their_owners` (AU6 §11.2 item 20).

use std::{collections::BTreeMap, ops::Range};

use crate::{
    AssetId, AudioBus, AudioBusId, AutomationCurve, ClipId, DeliveryEncodeDepth, DeliveryProfile,
    Document, EBU_R128_PROGRAMME_TARGET, Effect, EffectId, ExportCancellation, ExportSettings,
    Keyframe, KeyframeInterpolation, LoudnessTarget, NOISE_PROFILE_BAND_COUNT,
    NOISE_PROFILE_PARAMETER_NAMES, Operation, ParamValue, STREAMING_PLATFORM_TARGET, SyncGroup,
    SyncGroupId, SyncGroupMember, TRACK_AUTOMATION_PARAMETERS, TimeCode, TrackId, TrackKind,
    effect_descriptor,
};

/// AU6 §2.3: the project frame rate. At 25 fps a project frame is exactly
/// [`AU6_SAMPLES_PER_FRAME`] samples and a 200 ms window exactly
/// [`AU6_FRAMES_PER_WINDOW`] frames, which is what lets the whole-programme
/// `mix_window_levels` vector be sliced by integer index (AU6 §2.3, A1).
pub const AU6_SOURCE_FPS: u32 = 25;
/// AU6 §2.3: the shared raster width, CC7's.
pub const AU6_SOURCE_WIDTH: u32 = 320;
/// AU6 §2.3: the shared raster height.
pub const AU6_SOURCE_HEIGHT: u32 = 180;
/// AU6 §2.3: the sample rate of every authored buffer.
pub const AU6_SAMPLE_RATE: u32 = 48_000;
/// AU6 §2.3: every buffer is interleaved stereo.
pub const AU6_CHANNELS: u16 = 2;
/// AU6 §2.3: samples per project frame, `48 000 / 25 = 1 920`.
pub const AU6_SAMPLES_PER_FRAME: u32 = AU6_SAMPLE_RATE / AU6_SOURCE_FPS;
/// AU6 §2.3: the programme length of (a), (b), (d) and (e), 12 s.
pub const AU6_PROGRAMME_FRAMES: u32 = 300;
/// AU6 §2.3: (c)'s programme length, 12.48 s — its eighth gap runs to 312 so
/// the click-free learn gap is 37 frames long (§2.4).
pub const AU6_C_PROGRAMME_FRAMES: u32 = 312;
/// AU6 §2.3 (A1): the encoding lanes render 8 s, not the whole programme.
pub const AU6_ENCODE_PROGRAMME_SECONDS: u32 = 8;
/// AU6 §2.3 (A1): `8 × 25 = 200` project frames.
pub const AU6_ENCODE_PROGRAMME_FRAMES: u32 = AU6_ENCODE_PROGRAMME_SECONDS * AU6_SOURCE_FPS;
/// AU6 §2.3: the `mix_window_levels` window.
pub const AU6_WINDOW_MILLISECONDS: u32 = 200;
/// AU6 §2.3: the `mix_window_levels` hop, equal to the window so the grid is
/// contiguous and non-overlapping.
pub const AU6_HOP_MILLISECONDS: u32 = 200;
/// AU6 §2.3: `200 ms × 25 fps / 1 000 = 5` project frames per window.
pub const AU6_FRAMES_PER_WINDOW: u32 = AU6_WINDOW_MILLISECONDS * AU6_SOURCE_FPS / 1_000;
/// AU6 §2.4: the raised-cosine fade at every turn edge, normative — without
/// it a turn boundary is a step, and the probe measured (b)'s compressor
/// **increasing** the spread on stepped material.
pub const AU6_TURN_EDGE_FADE_MS: u32 = 20;

/// `LOUDNESS_GATING_BLOCK_FRAMES`, restated with its owner:
/// `crates/kinewright-media/src/loudness.rs:27` (re-exported `lib.rs:101`).
/// 19 200 sample frames = 400 ms = 10 project frames at 25 fps, so AU6's
/// shortest `mix_levels` window (a 25-frame gap) is 2.5× the block and its
/// longest (a 50-frame turn) 5.0× (AU6 §2.3).
pub const AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED: u32 = 19_200;
/// [`AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED`] in project frames.
pub const AU6_LOUDNESS_GATING_BLOCK_PROJECT_FRAMES: u32 =
    AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED / AU6_SAMPLES_PER_FRAME;

/// Which of the two voices owns a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Au6Speaker {
    /// Speaker A: the 1 kHz-band carrier, band [`AU6_VOICE_A_BAND_INDEX`].
    A,
    /// Speaker B: the 2 kHz-band carrier, band [`AU6_VOICE_B_BAND_INDEX`].
    B,
}

/// One authored turn, in project frames, half-open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6Turn {
    pub speaker: Au6Speaker,
    pub start: TimeCode,
    pub end: TimeCode,
}

impl Au6Turn {
    /// The turn as a half-open project-frame range.
    #[must_use]
    pub const fn range(&self) -> Range<TimeCode> {
        self.start..self.end
    }

    /// The turn's length in project frames.
    #[must_use]
    pub const fn frames(&self) -> i64 {
        self.end.0 - self.start.0
    }
}

/// AU6 §2.3's turn table: A `0..50`, B `75..125`, A `150..200`, B `225..275`,
/// each 2.0 s, separated by 1.0 s gaps. The same four turns author (a), (b)
/// and (c); only (c)'s eighth gap differs ([`AU6_C_GAPS`]).
pub const AU6_TURNS: [Au6Turn; 4] = [
    Au6Turn {
        speaker: Au6Speaker::A,
        start: TimeCode(0),
        end: TimeCode(50),
    },
    Au6Turn {
        speaker: Au6Speaker::B,
        start: TimeCode(75),
        end: TimeCode(125),
    },
    Au6Turn {
        speaker: Au6Speaker::A,
        start: TimeCode(150),
        end: TimeCode(200),
    },
    Au6Turn {
        speaker: Au6Speaker::B,
        start: TimeCode(225),
        end: TimeCode(275),
    },
];

/// AU6 §2.3: the four 1 s gaps of (a) and (b); the last runs to the programme
/// end at 300.
pub const AU6_GAPS: [Range<TimeCode>; 4] = [
    TimeCode(50)..TimeCode(75),
    TimeCode(125)..TimeCode(150),
    TimeCode(200)..TimeCode(225),
    TimeCode(275)..TimeCode(300),
];

/// AU6 §2.3 (nit N12): (c)'s gaps — the eighth runs to **312**, 37 frames,
/// which is the click-free learn gap of §2.4.
pub const AU6_C_GAPS: [Range<TimeCode>; 4] = [
    TimeCode(50)..TimeCode(75),
    TimeCode(125)..TimeCode(150),
    TimeCode(200)..TimeCode(225),
    TimeCode(275)..TimeCode(312),
];

/// AU6 §2.3: the named measurement windows, each a `const`.
pub const AU6_WINDOW_A_FIRST_TURN: Range<TimeCode> = TimeCode(0)..TimeCode(50);
/// See [`AU6_WINDOW_A_FIRST_TURN`].
pub const AU6_WINDOW_B_FIRST_TURN: Range<TimeCode> = TimeCode(75)..TimeCode(125);
/// See [`AU6_WINDOW_A_FIRST_TURN`].
pub const AU6_WINDOW_A_SECOND_TURN: Range<TimeCode> = TimeCode(150)..TimeCode(200);
/// See [`AU6_WINDOW_A_FIRST_TURN`].
pub const AU6_WINDOW_B_SECOND_TURN: Range<TimeCode> = TimeCode(225)..TimeCode(275);
/// AU6 §2.3: the whole 12 s programme.
pub const AU6_WINDOW_PROGRAMME: Range<TimeCode> =
    TimeCode(0)..TimeCode(AU6_PROGRAMME_FRAMES as i64);
/// The whole (c) programme, 312 frames.
pub const AU6_C_WINDOW_PROGRAMME: Range<TimeCode> =
    TimeCode(0)..TimeCode(AU6_C_PROGRAMME_FRAMES as i64);

/// The turns one speaker owns, in programme order (AU6 §4(a)(3) measures
/// `mix_levels` over each speaker's own turns).
#[must_use]
pub fn au6_turns_of(speaker: Au6Speaker) -> Vec<Range<TimeCode>> {
    AU6_TURNS
        .iter()
        .filter(|turn| turn.speaker == speaker)
        .map(Au6Turn::range)
        .collect()
}

/// The index of the whole-programme `mix_window_levels` window that starts at
/// `frame`, on the aligned 200 ms grid (AU6 §2.3). Every authored boundary is
/// a multiple of [`AU6_FRAMES_PER_WINDOW`], so the division is exact there.
#[must_use]
pub fn au6_window_index(frame: TimeCode) -> usize {
    usize::try_from(frame.0 / i64::from(AU6_FRAMES_PER_WINDOW)).unwrap_or(0)
}

/// AU6 §2.3 (B9): `plan_audio_ducking`'s attack, pinned equal to today's
/// default `DEFAULT_DUCKING_ATTACK_MILLISECONDS`
/// (`crates/kinewright-agent/src/server.rs:9849-9859`) so a default change is
/// caught rather than silently absorbed.
pub const AU6_A_DUCK_ATTACK_MS: u32 = 150;
/// AU6 §2.3 (B9): the hold, pinned equal to `DEFAULT_DUCKING_HOLD_MILLISECONDS`.
pub const AU6_A_DUCK_HOLD_MS: u32 = 200;
/// AU6 §2.3 (B9): the release, pinned equal to
/// `DEFAULT_DUCKING_RELEASE_MILLISECONDS`.
pub const AU6_A_DUCK_RELEASE_MS: u32 = 400;
/// AU6 §2.3 (B9): the depth under the parked fader, pinned equal to
/// `DEFAULT_DUCKING_DEPTH_TENTH_DB`.
pub const AU6_A_DUCK_DEPTH_TENTH_DB: i32 = -120;
/// AU6 §2.8: the sixteen keys the plan commits at the pinned arguments.
pub const AU6_A_DUCK_KEYFRAME_COUNT: usize = 16;

/// AU6 §2.3 (A2): the milliseconds of every 1 s gap the ramps and the hold
/// occupy — `attack 150 + hold 200 + release 400 = 750`. A2's arithmetic: a
/// 1 s gap leaves `1 000 − 750 = 250 ms` of parked bed, which holds exactly
/// one whole 200 ms window; a 500 ms gap leaves none, which is why the 1 s
/// gap is **required**, not preferred.
pub const AU6_A_DUCK_RAMP_AND_HOLD_MS: u32 =
    AU6_A_DUCK_ATTACK_MS + AU6_A_DUCK_HOLD_MS + AU6_A_DUCK_RELEASE_MS;

/// A2's arithmetic on the **authored** ramp lengths: the milliseconds of a gap
/// of `gap_ms` over which the bed sits at its parked value, once the hold and
/// release at the gap's head and the next turn's attack at its tail are
/// subtracted. `250` for the 1 s gap, `0` for a 500 ms gap.
#[must_use]
pub const fn au6_duck_gap_parked_milliseconds(gap_ms: u32) -> u32 {
    gap_ms.saturating_sub(AU6_A_DUCK_RAMP_AND_HOLD_MS)
}

/// How many whole 200 ms windows fit inside
/// [`au6_duck_gap_parked_milliseconds`]: **1** for the 1 s gap, **0** for a
/// 500 ms gap (A2).
#[must_use]
pub const fn au6_duck_gap_whole_windows(gap_ms: u32) -> u32 {
    au6_duck_gap_parked_milliseconds(gap_ms) / AU6_WINDOW_MILLISECONDS
}

/// REGRESSION PIN, not authored geometry (AU6 §2.6, A12, §11.0.1). These are
/// the sixteen keys `plan_audio_ducking` **committed** on the probe machine at
/// `11a6098` (probe-1 §7.1, identical in probe-2 §1) at the pinned arguments
/// `attack 150 / hold 200 / release 400 / depth −120` on (a)'s base document:
/// four linear keys per detected window, at attack before its start, at its
/// start, at its end, and at release after its end. The detector opens each
/// window one to two frames after the authored turn start and closes it four
/// frames after the authored end ([`AU6_A_DUCK_DETECTED_WINDOWS`]), so these
/// keys are the silence detector's output, never the authored turns of
/// [`AU6_TURNS`]. The claim is "the planner still commits what it committed
/// when this was measured", never "this is what the turn table implies".
pub const AU6_A_DUCK_KEYFRAMES: [(i64, i64); AU6_A_DUCK_KEYFRAME_COUNT] = [
    (0, 0),
    (1, -120),
    (54, -120),
    (64, 0),
    (73, 0),
    (77, -120),
    (129, -120),
    (139, 0),
    (147, 0),
    (151, -120),
    (204, -120),
    (214, 0),
    (223, 0),
    (227, -120),
    (279, -120),
    (289, 0),
];

/// REGRESSION PIN (A12): the four dialogue windows `plan_audio_ducking`
/// detected on (a) at `11a6098` — `{1,54} {77,129} {151,204} {227,279}` —
/// published beside the authored turns `0..50 / 75..125 / 150..200 / 225..275`
/// so a reader can see the detector's offset rather than infer it.
pub const AU6_A_DUCK_DETECTED_WINDOWS: [(i64, i64); 4] =
    [(1, 54), (77, 129), (151, 204), (227, 279)];

/// B2: the number of keys of [`AU6_A_DUCK_KEYFRAMES`] with `at < 200`.
pub const AU6_E_DUCK_KEYFRAME_COUNT: usize = 10;

const fn duck_key(at: i64, value: i64) -> Keyframe {
    Keyframe {
        at: TimeCode(at),
        value,
        interpolation: KeyframeInterpolation::Linear,
    }
}

/// B2 (AU6 §2.6 (e), Appendix A row C13): (e)'s ducking curve — the ten keys
/// of [`AU6_A_DUCK_KEYFRAMES`] with `at < AU6_ENCODE_PROGRAMME_FRAMES`. **A
/// derivation from the pin, not a second pin.** The 8 s lanes exist by
/// document truncation (`ExportSettings` has no range), and core refuses a
/// fader key outside `0..duration` (`project_frame_in_range`,
/// `crates/kinewright-core/src/operation.rs:1469`), so the six keys at
/// 204–289 cannot land on a 200-frame document. The curve ends at
/// `(151, −120)`: the bed stays **ducked from the third turn's attack to
/// frame 199 with no release**, because that turn runs to the truncated
/// programme's end. Probe-2 regenerated its own ten-key curve over the
/// surviving turns rather than clipping the planner's
/// (`target/review/au6/probe/probe2-media-tests.rs:913-995`, `:357-424`);
/// the two differ only inside the ramps, by at most one frame, and probe-2's
/// is recorded as evidence in Appendix A row C13, never published here.
pub const AU6_E_DUCK_KEYFRAMES: [Keyframe; AU6_E_DUCK_KEYFRAME_COUNT] = [
    duck_key(0, 0),
    duck_key(1, -120),
    duck_key(54, -120),
    duck_key(64, 0),
    duck_key(73, 0),
    duck_key(77, -120),
    duck_key(129, -120),
    duck_key(139, 0),
    duck_key(147, 0),
    duck_key(151, -120),
];

/// The keys of [`AU6_A_DUCK_KEYFRAMES`] that fall inside a `frames`-long
/// document — every key with `at < frames` — as an `AutomationCurve` with
/// `KeyframeInterpolation::Linear`. [`AU6_E_DUCK_KEYFRAMES`] is this at 200,
/// which `au6_the_delivery_scenario_reuses_the_interview_document` asserts.
#[must_use]
pub fn au6_duck_curve_within(frames: u32) -> AutomationCurve {
    AutomationCurve {
        keyframes: AU6_A_DUCK_KEYFRAMES
            .iter()
            .filter(|(at, _)| *at < i64::from(frames))
            .map(|(at, value)| duck_key(*at, *value))
            .collect(),
    }
}

/// The `AutomationCurve` the (a) commit writes on the music track: the pinned
/// sixteen keys with `KeyframeInterpolation::Linear`.
#[must_use]
pub fn au6_a_duck_curve() -> AutomationCurve {
    au6_duck_curve_within(AU6_PROGRAMME_FRAMES)
}

/// The `AutomationCurve` the (e) commit writes on the music track of the
/// 200-frame base: [`AU6_E_DUCK_KEYFRAMES`] (B2).
#[must_use]
pub fn au6_e_duck_curve() -> AutomationCurve {
    AutomationCurve {
        keyframes: AU6_E_DUCK_KEYFRAMES.to_vec(),
    }
}

/// AU6 §2.3 (A16): the duck-depth **speech** population — the whole 200 ms
/// windows from the **second** window of each turn to its end, 36 windows, as
/// indices into the whole-programme `mix_window_levels` vector.
#[must_use]
pub fn au6_duck_speech_window_indices() -> Vec<usize> {
    AU6_TURNS
        .iter()
        .flat_map(|turn| (au6_window_index(turn.start) + 1)..au6_window_index(turn.end))
        .collect()
}

/// AU6 §2.3 (A16): the duck-depth **gap** population — the **fourth** whole
/// 200 ms window of each 1 s gap, the only window that lies wholly inside the
/// parked interval (`600–850 ms` into the gap: hold + release at the head,
/// the next turn's attack at the tail). Window 5 contains the attack ramp and
/// reads 1.80 dB under parked.
#[must_use]
pub fn au6_duck_gap_window_indices() -> [usize; 4] {
    let mut indices = [0_usize; 4];
    for (slot, gap) in indices.iter_mut().zip(AU6_GAPS.iter()) {
        *slot = au6_window_index(gap.start) + 3;
    }
    indices
}

/// AU6 §3.2 rule 1: the Schroeder-phase partial count of each voice carrier.
pub const AU6_VOICE_PARTIALS: u32 = 64;
/// AU6 §2.4: speaker A occupies the 1 kHz third-octave band,
/// `exact_band_center(17) = 1000 · 2^0` (`crates/kinewright-media/src/spectrum.rs:225`).
pub const AU6_VOICE_A_BAND_INDEX: usize = 17;
/// AU6 §2.4: speaker B occupies the 2 kHz band, three bands above A.
pub const AU6_VOICE_B_BAND_INDEX: usize = 20;
/// AU6 §3.2 rule 1: the syllabic envelope rate of (a)'s and (c)'s voices — a
/// raised-cosine burst train whose mean square is exactly 3/8.
pub const AU6_VOICE_ENVELOPE_HERTZ: u32 = 4;

/// AU6 §2.4 (a): speaker A's authored RMS; measured −2 602.
pub const AU6_A_VOICE_A_LEVEL_DBFS_HUNDREDTHS: i32 = -2_600;
/// AU6 §2.4 (a): speaker B's authored RMS; measured −3 200.
pub const AU6_A_VOICE_B_LEVEL_DBFS_HUNDREDTHS: i32 = -3_200;
/// AU6 §2.4 (a): the music bed's authored RMS, continuous over the whole
/// programme; measured −2 000.
pub const AU6_A_BED_LEVEL_DBFS_HUNDREDTHS: i32 = -2_000;
/// AU6 §2.4 (a): the bed's six partials, in millihertz — A3, C#4, E4, A4, C#5,
/// E5 — a steady chord with a quadratic phase spread and no envelope, so a
/// windowed delta on it is attributable (AU4's lesson).
pub const AU6_A_BED_PARTIALS_MILLIHERTZ: [i64; 6] =
    [220_000, 277_183, 329_628, 440_000, 554_365, 659_255];

/// AU6 §2.4 (A3/E3): the (a) speaker-B trim, **measured**, not derived from
/// the authored dBFS. The two speakers are 6.00 dB apart in RMS dBFS but only
/// **3.65 LU** apart in LUFS, because K-weighting lifts the 2 kHz band ≈ 3.1 dB
/// and the 1 kHz band ≈ 0.8 dB. **The nominal `+60` is the wrong model**
/// ([`AU6_INTERVIEW_NOMINAL_TRIM_TENTH_DB_WRONG_MODEL`]): it over-corrects by
/// 2.35 LU and measures 235 centi-LU of mismatch — worse than the un-trimmed
/// 365 is better than it — while `+37` measures **5**.
pub const AU6_INTERVIEW_A2_TRIM_TENTH_DB: i32 = 37;
/// The authored 6.00 dB dBFS difference as a trim, named as the wrong model
/// (A3/E3): it measures 235 against `+37`'s 5, and the un-trimmed 365.
pub const AU6_INTERVIEW_NOMINAL_TRIM_TENTH_DB_WRONG_MODEL: i32 = 60;
/// AU6 §4(a)(3): the un-trimmed voice mismatch, the failing direction.
pub const AU6_MEASURED_VOICE_MATCH_UNTRIMMED_LU_HUNDREDTHS: i64 = 365;
/// AU6 §4(a)(3): the mismatch at the nominal `+60`, worse than no trim.
pub const AU6_MEASURED_VOICE_MATCH_NOMINAL_TRIM_LU_HUNDREDTHS: i64 = 235;

/// AU6 §2.4 (b): speaker A, steady, authored RMS; measured −3 005.
pub const AU6_B_VOICE_A_LEVEL_DBFS_HUNDREDTHS: i32 = -3_000;
/// AU6 §2.4 (b): speaker B's ride **peak**; measured −1 800.
pub const AU6_B_VOICE_B_LEVEL_DBFS_HUNDREDTHS: i32 = -1_800;
/// AU6 §2.4 (A5/E5): speaker B rides in the log domain at **1 Hz** — not the
/// brief's 4 Hz "syllabic", which a 200 ms window integrates 0.8 of a cycle of
/// and averages away (bypass spread 377 at 4 Hz against 1 482 at 1 Hz).
pub const AU6_PODCAST_AM_HERTZ: u32 = 1;
/// AU6 §2.4 (A5): the ride's depth, 18 dB, trough at both ends.
pub const AU6_PODCAST_AM_DEPTH_TENTH_DB: i32 = 180;
/// AU6 §2.4 (A3): speaker A's trim in (b), measured (voice delta 1 bypassed,
/// 50 through c1m).
pub const AU6_PODCAST_A_TRIM_TENTH_DB: i32 = 72;
/// AU6 §2.4 (A3): speaker B's trim in (b), measured.
pub const AU6_PODCAST_B_TRIM_TENTH_DB: i32 = -72;

/// AU6 §2.4 (A6/E6): (b)'s two gated clips on track 2, both edges on envelope
/// maxima and strictly inside the second turn; **no gated clip starts at
/// frame 0**, because `plan_clip_fades` skips a clip whose head **or** tail
/// window is silent.
pub const AU6_B_GATED_CLIP_RANGES: [Range<TimeCode>; 2] =
    [TimeCode(90)..TimeCode(103), TimeCode(103)..TimeCode(120)];
/// The clips of (b)'s track 2, in timeline order: the two gated clips of
/// [`AU6_B_GATED_CLIP_RANGES`] sit between a head clip `0..90` and a tail clip
/// `120..300`, all from the Voice B asset with source equal to project frames.
pub const AU6_B_VOICE_B_CLIP_RANGES: [Range<TimeCode>; 4] = [
    TimeCode(0)..TimeCode(90),
    TimeCode(90)..TimeCode(103),
    TimeCode(103)..TimeCode(120),
    TimeCode(120)..TimeCode(300),
];
/// The clip ids of the two gated clips in (b)'s base document: track 1 holds
/// clip 1 and track 2 holds clips 2–5 in timeline order, so the gated clips
/// are **3** and **4** ([`AU6_B_CLIPS`]).
pub const AU6_B_GATED_CLIP_IDS: [ClipId; 2] = [ClipId(3), ClipId(4)];
/// AU6 §2.6 (b): `plan_clip_fades` proposes one frame in and one frame out on
/// each gated clip at `fade_milliseconds = 20`, `window_project_frames = 1`,
/// `threshold_dbfs_hundredths = −4 000`.
pub const AU6_B_FADE_FRAMES: TimeCode = TimeCode(1);

/// AU6 §2.4 (c): the voice's authored RMS; measured −2 390, the worst
/// authored-level error of the slice (10, against the 25 ceiling).
pub const AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS: i32 = -2_400;
/// AU6 §2.4 (c): the white-noise bed's authored RMS (N1/B3: re-derived
/// downward from the silence detector so the gaps are detectable).
pub const AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS: i32 = -4_200;
/// AU6 §2.4 (c): the hum's authored **total** RMS over its five partials.
pub const AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS: i32 = -4_500;
/// AU6 §2.4 (c): the mains fundamental — 60 Hz, the branch AU5's 50 Hz
/// mains fixture does not exercise.
pub const AU6_C_HUM_FUNDAMENTAL_HERTZ: u32 = 60;
/// AU6 §2.4 (c): the fixture authors **five** partials — 60 / 120 / 180 / 240
/// / 300 Hz — while `plan_dialogue_repair` notches three
/// ([`AU6_C_HUM_HARMONIC_COUNT`]), which is why §4(c)(2) gates the first three
/// harmonics only and records the fourth (A19).
pub const AU6_C_HUM_PARTIALS: u32 = 5;
/// AU6 §2.4 (c): each partial is **half the amplitude** of the one below it —
/// the "−6 dB per step" of §2.4 / §3.2 rule 5 is exactly −6.02 dB, AU5's
/// `hum_fixture` shape (`0.5^h`, which probe-2's `hum()` copied at
/// `target/review/au6/probe/probe2-media-tests.rs:797-811`). Stated as a
/// ratio because [`AU6_C_ANALYTIC_MEAN_BAND_TENTH_DB`] was evaluated on the
/// halving, and a literal −6.00 dB moves band 10 by one tenth dB.
pub const AU6_C_HUM_PARTIAL_AMPLITUDE_RATIO_HUNDREDTHS: i64 = 50;
/// AU6 §2.4 (c): the authored learn-gap RMS (noise + hum), measured −4 026.
pub const AU6_MEASURED_C_LEARN_GAP_DBFS_HUNDREDTHS: i64 = -4_026;
/// AU6 §2.4 (c): the other three gaps' RMS, measured −3 978.
pub const AU6_MEASURED_C_OTHER_GAP_DBFS_HUNDREDTHS: i64 = -3_978;

/// AU6 §2.4 (c): twelve clicks, under speech and in the three clicked gaps,
/// **never in the learn gap**.
pub const AU6_C_CLICK_COUNT: usize = 12;
/// AU6 §2.4 (c): the click frames.
pub const AU6_C_CLICK_FRAMES: [i64; AU6_C_CLICK_COUNT] =
    [10, 20, 30, 40, 60, 90, 100, 140, 180, 190, 210, 250];
/// AU6 §2.4 (A20): the click shape — **two** samples, `+A` then `−A`, both
/// channels, written after noise and hum. AU5's 8-frame sign-alternating click
/// is the wrong model here: at its ±0.9 the three clicked gaps read
/// −36.39 dBFS, 139 hundredths under the −35.00 threshold, failing the 250
/// floor; at ±0.5 they clear it at only 1.46×.
pub const AU6_CLICK_SAMPLES: u32 = 2;
/// AU6 §2.4 (A20): the click amplitude, `0.50` full scale.
pub const AU6_CLICK_AMPLITUDE_HUNDREDTHS: i32 = 50;
/// AU6 §2.4 (A20): the three clicked gaps' asset RMS with the 2-sample click,
/// 478 hundredths under the threshold.
/// The three clicked gaps ARE the three non-learn gaps, so this is the same
/// measurement as [`AU6_MEASURED_C_OTHER_GAP_DBFS_HUNDREDTHS`] under the name
/// §4.2 and Appendix A row E20 use for it; it derives rather than restates.
pub const AU6_MEASURED_C_CLICKED_GAP_DBFS_HUNDREDTHS: i64 =
    AU6_MEASURED_C_OTHER_GAP_DBFS_HUNDREDTHS;

/// `DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS`, restated with its owner:
/// `crates/kinewright-media/src/derived.rs:33`. The brief's noise + hum bed sat
/// at ≈ −29 dBFS, above it, so no authored gap would ever have been detected
/// as silence (N1/B3).
pub const AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS: i32 = -3_500;
/// `NOISE_PROFILE_MINIMUM_FRAMES`, restated with its owner:
/// `crates/kinewright-media/src/spectrum.rs:48-49` (`pub(crate)`) —
/// `SEGMENT + 9 · HOP = 4 096 + 9 · 2 048 = 22 528` sample frames, ten
/// windows.
pub const AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED: u64 = 22_528;
/// `NOISE_PROFILE_SEGMENT_FRAMES`, restated with its owner:
/// `crates/kinewright-media/src/spectrum.rs:37`.
pub const AU6_NOISE_PROFILE_SEGMENT_SAMPLE_FRAMES_RESTATED: u64 = 4_096;
/// `NOISE_PROFILE_PERCENT`, restated with its owner:
/// `crates/kinewright-media/src/spectrum.rs:52-54` — the profile reduces over
/// ten windows with the **20th** percentile, which is why the analytic bound
/// of §2.5(ii) is one-sided.
pub const AU6_NOISE_PROFILE_PERCENT_RESTATED: u32 = 20;
/// `MAIN_LOBE_BINS`, restated with its owner:
/// `crates/kinewright-media/src/spectrum.rs:70` — the Hann main lobe is four
/// bins wide, `±2 · bin_width` about a partial.
pub const AU6_HANN_MAIN_LOBE_BINS_RESTATED: u32 = 4;
/// AU6 §2.8: `ceil(22 528 × 25 / 48 000) = 12` project frames, the shortest
/// range a noise profile can be learned over.
pub const AU6_LEARN_MINIMUM_PROJECT_FRAMES: u64 = (AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED
    * AU6_SOURCE_FPS as u64)
    .div_ceil(AU6_SAMPLE_RATE as u64);

/// AU6 §2.4: (c)'s **authored** click-free gap, 37 frames, published beside
/// the committed range so the two are never confused.
pub const AU6_C_LEARN_AUTHORED_RANGE: Range<TimeCode> = TimeCode(275)..TimeCode(312);
/// REGRESSION PIN, not the authored range (AU6 §2.4, A12/E12, A19, §11.0.1).
/// `timeline_silences`' 10 ms RMS window closes early, so the authored
/// `275..312` commits as **`274..312`**: `plan_dialogue_repair` reported
/// `learn_range = { track: 1, start_frame: 274, end_frame: 312 }` and the four
/// detected spans were `[(60,76), (124,140), (210,226), (274,312)]` on the
/// probe machine at `11a6098` (probe-1 §3.2 / §7.4, probe-2 §2.2). It is
/// pinned **exactly**, because learning over `275..312` instead moves band 0
/// of the profile from −853 to −882 (29 tenth dB) and breaks the exact pin
/// of [`AU6_C_LEARNED_PROFILE_TENTH_DB`]. The claim is "the detector still
/// finds what it found", never "this is what the authored gap implies".
pub const AU6_C_LEARN_PROJECT_RANGE: Range<TimeCode> = TimeCode(274)..TimeCode(312);
/// REGRESSION PIN (A12): the four silence spans `timeline_silences` returned
/// over (c) at `11a6098`; the learn gap is the longest by 21 frames.
pub const AU6_C_DETECTED_SILENCES: [(i64, i64); 4] = [(60, 76), (124, 140), (210, 226), (274, 312)];
/// AU6 §2.4 (S15): the **source** range `capture_room_tone` decodes on the
/// dialogue asset — the authored click-free gap, `275..312`. After the split
/// at 175 the surviving right-hand clip carries `source_range 175..312` at
/// `timeline_start 175`, so the project→source mapping is the identity. This
/// is the capture that produced the recorded 44-frame / 1 466 ms room-tone
/// asset (Appendix A row E15); the detector's `274` of
/// [`AU6_C_LEARN_PROJECT_RANGE`] is the **repair** learn range, a project
/// range the planner finds itself.
pub const AU6_C_LEARN_SOURCE_RANGE: Range<TimeCode> = TimeCode(275)..TimeCode(312);
/// AU6 §2.6 (c) item 1: the interior gap the person and the agent both cut,
/// `160..175`, 15 project frames.
pub const AU6_C_GAP_RANGE: Range<TimeCode> = TimeCode(160)..TimeCode(175);
/// AU6 §2.6 (c) item 3 (E15): the captured room tone probes at the store's
/// own **30 fps** grid, not the project's 25.
pub const AU6_C_ROOM_TONE_FPS: u32 = 30;
/// AU6 §2.6 (c) item 3 (E15): 44 asset frames / 1 466 ms / 70 400 sample
/// frames — the capture of [`AU6_C_LEARN_SOURCE_RANGE`].
pub const AU6_C_ROOM_TONE_ASSET_FRAMES: i64 = 44;
/// AU6 §2.6 (c) item 3: `plan_room_tone_fill`'s one tile — 15 project frames
/// at 25 fps is 600 ms is **18** source frames at 30 fps, taken from the head
/// of the captured asset (`longest_coverable_project_tile`).
pub const AU6_C_FILL_TILE_SOURCE_RANGE: Range<TimeCode> = TimeCode(0)..TimeCode(18);
/// AU6 §2.6 (c) item 3: `plan_room_tone_fill`'s `maximum_tiles`.
pub const AU6_C_FILL_MAXIMUM_TILES: u32 = 64;
/// AU6 §2.6 (c) item 3: `plan_room_tone_fill`'s `minimum_gap_frames`.
pub const AU6_C_FILL_MINIMUM_GAP_FRAMES: TimeCode = TimeCode(1);

/// AU6 §2.4 (d): the master audio track's authored RMS.
pub const AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS: i32 = -2_000;
/// AU6 §2.4 (d): the level of the LCG noise added to each scratch track. A
/// scratch buffer is the master delayed by its angle offset, **halved**, plus
/// this noise, so its RMS is not this number; §3.2 rule 1 gates no (d) buffer
/// (Appendix A row S2 lists none).
pub const AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS: i32 = -3_400;
/// AU6 §2.4 (d): the two angles' sync offsets, authored and caller-supplied;
/// nothing derives or verifies them.
pub const AU6_D_ANGLE_OFFSETS_FRAMES: [i64; 2] = [0, 8];
/// AU6 §2.4 (d): the three cut frames.
pub const AU6_D_CUT_FRAMES: [i64; 3] = [75, 150, 225];
/// AU6 §2.4 (d): the sync group's id in the base document.
pub const AU6_D_SYNC_GROUP_ID: SyncGroupId = SyncGroupId(1);
/// AU6 §2.4 (d): the sync group's name.
pub const AU6_D_SYNC_GROUP_NAME: &str = "Interview";
/// AU6 §2.4 (d): the two angle names.
pub const AU6_D_ANGLE_NAMES: [&str; 2] = ["Angle 1", "Angle 2"];

/// AU6 §2.4 (d): the `SyncGroup` that is base-document state, not a
/// canonical operation, for the two angle assets in [`AU6_D_ANGLE_OFFSETS_FRAMES`]
/// order.
#[must_use]
pub fn au6_d_sync_group(angle_assets: [AssetId; 2]) -> SyncGroup {
    SyncGroup {
        id: AU6_D_SYNC_GROUP_ID,
        name: AU6_D_SYNC_GROUP_NAME.to_owned(),
        members: angle_assets
            .into_iter()
            .zip(AU6_D_ANGLE_OFFSETS_FRAMES)
            .zip(AU6_D_ANGLE_NAMES)
            .map(|((asset, offset), angle_name)| SyncGroupMember {
                asset,
                offset: TimeCode(offset),
                angle_name: angle_name.to_owned(),
            })
            .collect(),
    }
}

/// REGRESSION PIN, not an analytic expectation (AU6 §2.5(i), §11.0.1). These
/// are the values `mix_noise_profile` produced over
/// [`AU6_C_LEARN_PROJECT_RANGE`] at `Bus(Dialogue repair)` on the probe machine at
/// `11a6098` (probe-1 §3.3, re-learned three times in probe-2 §2.1, each with
/// a fresh `FfmpegMediaEngine` and a freshly synthesised fixture, with
/// `max_abs_delta_tenth_db = 0`). The claim is "the learner still does what it
/// did when this was measured", never "this is what the authored bed implies";
/// what the bed implies is the one-sided bound of
/// [`au6_c_analytic_mean_band_tenth_db`]. Asserted with an exact `assert_eq!`
/// (A19): there is no drift to budget.
pub const AU6_C_LEARNED_PROFILE_TENTH_DB: [i32; NOISE_PROFILE_BAND_COUNT] = [
    -853, -846, -809, -588, -497, -457, -530, -615, -502, -601, -580, -619, -643, -659, -645, -626,
    -618, -604, -593, -576, -566, -559, -546, -538, -530, -517, -505, -496, -484, -476, -463,
];

/// AU6 §2.5: the band 0 value a learn over the **authored** `275..312`
/// produces instead of the pin's −853 — one frame moves nine bands by up to
/// 29 tenth dB, which is why the learn range is pinned exactly.
pub const AU6_C_ONE_FRAME_OFFSET_BAND0_TENTH_DB: i32 = -882;
/// AU6 §2.5: `|−882 − (−853)|`, the worst band delta of a one-frame slip.
pub const AU6_C_ONE_FRAME_OFFSET_MAX_DELTA_TENTH_DB: i32 = 29;

/// AU6 §2.5(ii): the Hann-main-lobe analytic bound, band by band, in tenth dB
/// — the value [`au6_c_analytic_mean_band_tenth_db`] computes, transcribed
/// here from probe-2 §2.3 so the arithmetic is checked against an independent
/// evaluation rather than trusted.
pub const AU6_C_ANALYTIC_MEAN_BAND_TENTH_DB: [i32; NOISE_PROFILE_BAND_COUNT] = [
    -762, -752, -742, -511, -494, -484, -487, -542, -513, -565, -570, -602, -626, -632, -622, -612,
    -602, -592, -582, -572, -562, -552, -542, -532, -522, -512, -502, -492, -482, -471, -461,
];

/// AU6 §2.5(ii): the **point-mass** model — a partial's whole power in the one
/// band containing its frequency — named as the wrong model. It denies that a
/// windowed tone leaks: its worst excess is **225** tenth dB in band 4,
/// *adjacent* to 60 Hz, and it would need an allowance of 450 tenth dB,
/// making every hum-adjacent band vacuous.
pub const AU6_C_POINT_MASS_BAND_TENTH_DB_WRONG_MODEL: [i32; NOISE_PROFILE_BAND_COUNT] = [
    -762, -752, -742, -732, -722, -433, -702, -692, -492, -672, -550, -598, -625, -632, -622, -612,
    -602, -592, -582, -572, -562, -552, -542, -532, -522, -512, -502, -492, -482, -471, -461,
];
/// The point-mass model's worst excess, at band 4 (A19).
pub const AU6_C_POINT_MASS_WORST_EXCESS_TENTH_DB: i32 = 225;
/// The band the point-mass model's worst excess lands in.
pub const AU6_C_POINT_MASS_WORST_BAND: usize = 4;
/// The Hann-main-lobe bound's worst excess, at band 5 (A19).
pub const AU6_C_LEAKAGE_WORST_EXCESS_TENTH_DB: i32 = 27;
/// The band the main-lobe bound's worst excess lands in.
pub const AU6_C_LEAKAGE_WORST_BAND: usize = 5;
/// The main-lobe bound's most negative excess, at band 1 — the 20th
/// percentile's own downward bias on a noise-only band.
pub const AU6_C_LEAKAGE_MIN_EXCESS_TENTH_DB: i32 = -94;

/// AU6 §2.5: the three shape claims. Bands 4–6 (≈ 50–80 Hz, the 60 Hz
/// fundamental) sit 13–20 tenth dB **above** the broadband trend.
pub const AU6_C_PROFILE_HUM_BUMP_BANDS: Range<usize> = 4..7;
/// AU6 §2.5: bands 1–3 (20–40 Hz) fall away 30–40 tenth dB — nothing is
/// authored below 50 Hz.
pub const AU6_C_PROFILE_LOW_FALLOFF_BANDS: Range<usize> = 1..4;
/// AU6 §2.5: from this band up the pinned profile rises **monotonically**,
/// the white-noise third-octave slope (1.0 dB per band analytically, ≈ 0.4
/// read through the percentile). §2.5's prose says "above band 8", but the
/// pin itself falls from band 8 (−502) to band 9 (−601) and again over bands
/// 11–13 — the 120 / 180 / 240 Hz partials sit there — and is monotone only
/// from band 13 (≈ 400 Hz); `au6_every_budget_carries_the_declared_margin`
/// asserts both facts on the pin.
pub const AU6_C_PROFILE_MONOTONE_FROM_BAND: usize = 13;

/// Which hum-partial leakage model the analytic band level uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Au6HumLeakageModel {
    /// The Hann main lobe, four bins wide (AU6 §2.5(ii)).
    HannMainLobe,
    /// The whole partial in the one band containing it — the wrong model.
    PointMass,
}

/// One AU6 integer as `f64`. Every value this module converts is far inside
/// `2^53`, so the conversion is exact and the pedantic precision-loss lint is
/// answered here once.
#[allow(clippy::cast_precision_loss)]
const fn au6_as_f64(value: i64) -> f64 {
    value as f64
}

/// `exact_band_center(index) = 1000 · 2^((index − 17)/3)`, transcribed from
/// `crates/kinewright-media/src/spectrum.rs:225`.
fn au6_exact_band_center_hertz(band: usize) -> f64 {
    let index = i64::try_from(band).unwrap_or(0);
    1_000.0 * 2.0_f64.powf(au6_as_f64(index - 17) / 3.0)
}

/// `band_edges(index) = [f_c · 2^(−1/6), f_c · 2^(1/6))`, transcribed from
/// `spectrum.rs:230-236`.
fn au6_band_edges_hertz(band: usize) -> (f64, f64) {
    let centre = au6_exact_band_center_hertz(band);
    let ratio = 2.0_f64.powf(1.0 / 6.0);
    (centre / ratio, centre * ratio)
}

/// `profile_band_tenth_db` verbatim (`crates/kinewright-media/src/export.rs:1815-1834`):
/// `None` writes the neutral, hundredths become tenths by `div_euclid(10)`
/// (toward −∞), and the result is clamped into `-1200..=0`.
fn au6_profile_band_tenth_db(hundredths: Option<i32>) -> i32 {
    let neutral = i32::try_from(crate::PROFILE_BAND_NEUTRAL_TENTH_DB).unwrap_or(-1_200);
    match hundredths {
        None => neutral,
        Some(value) => value.div_euclid(10).clamp(neutral, 0),
    }
}

/// The mean-power band level of the authored (c) bed — white noise plus the
/// five hum partials — under one leakage model, in tenth dB after
/// `profile_band_tenth_db`.
///
/// White noise: `pseudo_random_amplitude` is uniform on `[−a, a)`, RMS `a/√3`,
/// flat one-sided PSD, so a band's mean power is `P_total · (high − low) /
/// nyquist` — exactly what `band_level_hundredths`' half-bin overlap sum
/// converges to, because the overlap weights over a band sum to
/// `(high − low) / bin_width`. Hum: each partial's one-sided power is
/// `A_k² / 2`, spread over the Hann main lobe (`±2 · bin_width = ±23.44 Hz`)
/// before the band integrator sees it, or dropped whole into one band under
/// the wrong model.
#[allow(clippy::cast_possible_truncation)]
fn au6_c_band_bound_tenth_db(band: usize, model: Au6HumLeakageModel) -> i32 {
    assert!(
        band < NOISE_PROFILE_BAND_COUNT,
        "band {band} is outside the 31 third-octave bands"
    );
    let nyquist = f64::from(AU6_SAMPLE_RATE) / 2.0;
    let bin_width = f64::from(AU6_SAMPLE_RATE)
        / au6_as_f64(
            i64::try_from(AU6_NOISE_PROFILE_SEGMENT_SAMPLE_FRAMES_RESTATED).unwrap_or(4_096),
        );
    let noise_power = 10.0_f64.powf(f64::from(AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS) / 1_000.0);
    let hum_power = 10.0_f64.powf(f64::from(AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS) / 1_000.0);
    // Partial amplitude weights: 1, 1/2, 1/4, … — the halving per step.
    let ratio = au6_as_f64(AU6_C_HUM_PARTIAL_AMPLITUDE_RATIO_HUNDREDTHS) / 100.0;
    let weights = (0..AU6_C_HUM_PARTIALS)
        .map(|partial| ratio.powi(i32::try_from(partial).unwrap_or(0)))
        .collect::<Vec<_>>();
    let sum_squares = weights.iter().map(|weight| weight * weight).sum::<f64>();
    let amplitude = (2.0 * hum_power / sum_squares).sqrt();

    let (low, high) = au6_band_edges_hertz(band);
    let high = high.min(nyquist);
    let mut power = if high > low {
        noise_power * (high - low) / nyquist
    } else {
        0.0
    };
    for (partial, weight) in weights.iter().enumerate() {
        let hertz = f64::from(AU6_C_HUM_FUNDAMENTAL_HERTZ)
            * au6_as_f64(i64::try_from(partial).unwrap_or(0) + 1);
        let partial_amplitude = amplitude * weight;
        let partial_power = partial_amplitude * partial_amplitude / 2.0;
        match model {
            Au6HumLeakageModel::HannMainLobe => {
                let half_lobe = f64::from(AU6_HANN_MAIN_LOBE_BINS_RESTATED) / 2.0 * bin_width;
                let overlap = (hertz + half_lobe).min(high) - (hertz - half_lobe).max(low);
                if overlap > 0.0 {
                    power += partial_power * (overlap / (2.0 * half_lobe)).min(1.0);
                }
            }
            Au6HumLeakageModel::PointMass => {
                if hertz >= low && hertz < high {
                    power += partial_power;
                }
            }
        }
    }
    let hundredths = if power < 1e-12 {
        None
    } else {
        Some((10.0 * (power / 0.5).log10() * 100.0).round() as i32)
    };
    au6_profile_band_tenth_db(hundredths)
}

/// AU6 §2.5(ii): the one-sided analytic bound — the **mean-power** band level
/// of the authored white noise plus the hum partials, each partial modelled as
/// a Hann main lobe four bins wide, computed with the same bin-count
/// integrator and half-bin overlap the runtime uses and then through
/// `profile_band_tenth_db` verbatim. Every learned band must be
/// `≤ this + AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB`; the inequality holds by
/// construction because a 20th percentile sits below the mean.
///
/// **Two inputs `au6_hum_pcm` must honour for this to reproduce probe-2's
/// array** ([`AU6_C_ANALYTIC_MEAN_BAND_TENTH_DB`], asserted exactly by
/// `au6_every_budget_carries_the_declared_margin`): the partials halve in
/// **amplitude** per step ([`AU6_C_HUM_PARTIAL_AMPLITUDE_RATIO_HUNDREDTHS`],
/// `0.5^h`, not a literal −6.00 dB, which moves band 10 by one tenth dB) and
/// [`AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS`] is the **total** hum RMS over all five
/// partials, not the fundamental's.
///
/// # Panics
///
/// Panics when `band` is not one of the 31 third-octave bands.
#[must_use]
pub fn au6_c_analytic_mean_band_tenth_db(band: usize) -> i32 {
    au6_c_band_bound_tenth_db(band, Au6HumLeakageModel::HannMainLobe)
}

/// AU6 §2.5(ii): the point-mass model, **the wrong model**, kept so the test
/// can show why it is wrong (worst excess 225 in a band adjacent to 60 Hz).
///
/// # Panics
///
/// Panics when `band` is not one of the 31 third-octave bands.
#[must_use]
pub fn au6_c_point_mass_band_tenth_db_wrong_model(band: usize) -> i32 {
    au6_c_band_bound_tenth_db(band, Au6HumLeakageModel::PointMass)
}

/// The five named audio workflows AU6 proves end to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Au6Scenario {
    /// (a) two-person interview with a music bed.
    Interview,
    /// (b) podcast with uneven voices.
    Podcast,
    /// (c) noisy location dialogue.
    LocationDialogue,
    /// (d) event / multicam with a master audio track.
    Multicam,
    /// (e) encoded delivery at two loudness targets, on (a)'s document.
    Delivery,
}

/// AU6 §10.9's scenario iteration order.
pub const AU6_SCENARIOS: [Au6Scenario; 5] = [
    Au6Scenario::Interview,
    Au6Scenario::Podcast,
    Au6Scenario::LocationDialogue,
    Au6Scenario::Multicam,
    Au6Scenario::Delivery,
];

/// Whether the person path can express a scenario's canonical document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Au6PersonPath {
    /// The app's operation builders can author it by hand.
    Expressible,
    /// It cannot be authored by hand, with the reason the code itself gives.
    NotApplicable { reason: &'static str },
}

/// What a track carries in a scenario document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Au6TrackRole {
    /// Speaker A's voice.
    VoiceA,
    /// Speaker B's voice.
    VoiceB,
    /// (a)'s steady six-partial chord.
    MusicBed,
    /// (c)'s voice + noise + hum + clicks.
    Dialogue,
    /// (a)/(e)'s V1: a **video-only** picture asset (`au6_picture_source`,
    /// `MediaKind::Video`), exactly as both probes built it (B1). No document
    /// carries an audio-carrying mux: `export.rs` does not filter by track
    /// kind, so any PCM on V1 would join the (a) mix and move every row.
    Picture,
    /// One of (d)'s two video-only angles.
    Angle,
    /// (d)'s camera scratch audio: the master delayed, halved, plus noise.
    Scratch,
    /// (d)'s recorder track, the one no operation touches.
    MasterAudio,
}

/// One track of a scenario's base document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6TrackSpec {
    pub track: TrackId,
    pub kind: TrackKind,
    pub role: Au6TrackRole,
    /// S11: which speaker's Schroeder carrier the track's voice is authored
    /// from — `Some(A)` for every Voice A track **and for (c)'s dialogue on
    /// all four turns**, `Some(B)` for every Voice B track ((b)'s ride is B's
    /// carrier), `None` for the bed, the picture, the angles, the scratch and
    /// the master.
    pub carrier: Option<Au6Speaker>,
    /// The authored RMS of the track's buffer in dBFS hundredths; `None` for a
    /// picture-only track. For [`Au6TrackRole::Scratch`] it is the additive
    /// noise component's level (§2.4 (d)), which §3.2 rule 1 does not gate.
    pub level_dbfs_hundredths: Option<i32>,
    /// `true` on every (d) track (AU6 §0.2 item 21), so `RippleDeleteClip`
    /// would move the master stem and the failing direction can fail.
    pub sync_lock: bool,
}

/// One bus of a scenario's canonical document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6BusSpec {
    pub bus: AudioBusId,
    pub name: &'static str,
    pub tracks: &'static [TrackId],
}

/// One clip of a scenario's **base** document, in timeline order per track.
/// Every base clip maps source to project frames one-to-one (the asset is
/// stamped onto the 25 fps grid, A11), so `source_range == start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6ClipSpec {
    pub clip: ClipId,
    pub track: TrackId,
    pub asset: AssetId,
    pub start: TimeCode,
    pub end: TimeCode,
}

impl Au6ClipSpec {
    /// The clip's project range, which is also its source range.
    #[must_use]
    pub const fn range(&self) -> Range<TimeCode> {
        self.start..self.end
    }
}

/// Everything AU6 pins about one scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6ScenarioSpec {
    pub scenario: Au6Scenario,
    /// `"a".."e"`.
    pub id: &'static str,
    pub title: &'static str,
    pub fps: u32,
    /// The document's length in project frames — every base clip ends here.
    pub frames: u32,
    /// The authored assets' length in source frames. Equal to `frames` except
    /// for (e), whose base is (a)'s **300-frame** assets under **200-frame**
    /// clips (B2: the 8 s lanes exist by document truncation).
    pub asset_frames: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// `Some(other)` when this scenario reuses another's assets and
    /// operations at this scenario's own `frames` — only (e) does, and it
    /// reuses (a).
    pub document_source: Option<Au6Scenario>,
    /// S18: a picture asset exists in this scenario's document.
    pub carries_picture: bool,
    /// S18 + A1: this scenario runs an encode and a delivery verification.
    /// (a) false, (b) false, (c) false, **(d) false** (its delivery leg is
    /// cut, A1), (e) true.
    pub delivers: bool,
    pub tracks: &'static [Au6TrackSpec],
    pub buses: &'static [Au6BusSpec],
    /// Empty for (d).
    pub turns: &'static [Au6Turn],
    /// The base document's clips, in track then timeline order.
    pub clips: &'static [Au6ClipSpec],
    pub person_path: Au6PersonPath,
}

const fn clip(clip: u64, track: u64, asset: u64, start: i64, end: i64) -> Au6ClipSpec {
    Au6ClipSpec {
        clip: ClipId(clip),
        track: TrackId(track),
        asset: AssetId(asset),
        start: TimeCode(start),
        end: TimeCode(end),
    }
}

const fn track(
    track: u64,
    kind: TrackKind,
    role: Au6TrackRole,
    carrier: Option<Au6Speaker>,
    level_dbfs_hundredths: Option<i32>,
    sync_lock: bool,
) -> Au6TrackSpec {
    Au6TrackSpec {
        track: TrackId(track),
        kind,
        role,
        carrier,
        level_dbfs_hundredths,
        sync_lock,
    }
}

/// S11: (c)'s single dialogue track is speaker **A**'s band-17 carrier on all
/// four turns of the A/B turn pattern; every (c) number — 2 041 → 3 183, the
/// 31-band pin, the −4 026 learn-gap RMS — was measured on it.
pub const AU6_C_VOICE_CARRIER: Au6Speaker = Au6Speaker::A;

// --- (a) / (e) -------------------------------------------------------------

/// (a)'s picture track.
pub const AU6_A_PICTURE_TRACK: TrackId = TrackId(1);
/// (a)'s Voice A track.
pub const AU6_A_VOICE_A_TRACK: TrackId = TrackId(2);
/// (a)'s Voice B track.
pub const AU6_A_VOICE_B_TRACK: TrackId = TrackId(3);
/// (a)'s music bed track, the one the duck rides.
pub const AU6_A_BED_TRACK: TrackId = TrackId(4);
/// (a)'s Dialogue bus.
pub const AU6_A_DIALOGUE_BUS: AudioBusId = AudioBusId(1);
/// (a)'s Music bus — **the** duck-depth measurement point (A13): with the
/// curve on the track, `Track(A3)` reads a depth of −1 for the bus-curve
/// owner while `Bus(Music)` reads the real depth for all three owners.
pub const AU6_A_MUSIC_BUS: AudioBusId = AudioBusId(2);
/// (a)'s Dialogue bus name.
pub const AU6_A_DIALOGUE_BUS_NAME: &str = "Dialogue";
/// (a)'s Music bus name.
pub const AU6_A_MUSIC_BUS_NAME: &str = "Music";

const AU6_A_TRACKS: [Au6TrackSpec; 4] = [
    track(
        1,
        TrackKind::Video,
        Au6TrackRole::Picture,
        None,
        None,
        false,
    ),
    track(
        2,
        TrackKind::Audio,
        Au6TrackRole::VoiceA,
        Some(Au6Speaker::A),
        Some(AU6_A_VOICE_A_LEVEL_DBFS_HUNDREDTHS),
        false,
    ),
    track(
        3,
        TrackKind::Audio,
        Au6TrackRole::VoiceB,
        Some(Au6Speaker::B),
        Some(AU6_A_VOICE_B_LEVEL_DBFS_HUNDREDTHS),
        false,
    ),
    track(
        4,
        TrackKind::Audio,
        Au6TrackRole::MusicBed,
        None,
        Some(AU6_A_BED_LEVEL_DBFS_HUNDREDTHS),
        false,
    ),
];
const AU6_A_DIALOGUE_BUS_TRACKS: [TrackId; 2] = [AU6_A_VOICE_A_TRACK, AU6_A_VOICE_B_TRACK];
const AU6_A_MUSIC_BUS_TRACKS: [TrackId; 1] = [AU6_A_BED_TRACK];
const AU6_A_BUSES: [Au6BusSpec; 2] = [
    Au6BusSpec {
        bus: AU6_A_DIALOGUE_BUS,
        name: AU6_A_DIALOGUE_BUS_NAME,
        tracks: &AU6_A_DIALOGUE_BUS_TRACKS,
    },
    Au6BusSpec {
        bus: AU6_A_MUSIC_BUS,
        name: AU6_A_MUSIC_BUS_NAME,
        tracks: &AU6_A_MUSIC_BUS_TRACKS,
    },
];
/// (a)'s base clips: one per track, each over the whole programme; asset ids
/// follow track order (1 the video-only picture, 2 Voice A, 3 Voice B, 4 the
/// bed).
pub const AU6_A_CLIPS: [Au6ClipSpec; 4] = [
    clip(1, 1, 1, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(2, 2, 2, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(3, 3, 3, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(4, 4, 4, 0, AU6_PROGRAMME_FRAMES as i64),
];
/// B2: (e)'s base clips — (a)'s layout on **200-frame** assets, the document
/// truncation by which the 8 s lanes exist (`ExportSettings` has no range).
pub const AU6_E_CLIPS: [Au6ClipSpec; 4] = [
    clip(1, 1, 1, 0, AU6_ENCODE_PROGRAMME_FRAMES as i64),
    clip(2, 2, 2, 0, AU6_ENCODE_PROGRAMME_FRAMES as i64),
    clip(3, 3, 3, 0, AU6_ENCODE_PROGRAMME_FRAMES as i64),
    clip(4, 4, 4, 0, AU6_ENCODE_PROGRAMME_FRAMES as i64),
];

// --- (b) -------------------------------------------------------------------

/// (b)'s Voice A track.
pub const AU6_B_VOICE_A_TRACK: TrackId = TrackId(1);
/// (b)'s Voice B track, the one that carries the gated clips.
pub const AU6_B_VOICE_B_TRACK: TrackId = TrackId(2);
/// (b)'s Voice A bus, no chain.
pub const AU6_B_VOICE_A_BUS: AudioBusId = AudioBusId(1);
/// (b)'s Voice B bus, the chain — gate (2) reads `Bus(Voice B)` because a
/// `Track` point is pre-bus-chain (N1/Q4/B5).
pub const AU6_B_VOICE_B_BUS: AudioBusId = AudioBusId(2);
/// (b)'s Voice A bus name.
pub const AU6_B_VOICE_A_BUS_NAME: &str = "Voice A";
/// (b)'s Voice B bus name.
pub const AU6_B_VOICE_B_BUS_NAME: &str = "Voice B";

const AU6_B_TRACKS: [Au6TrackSpec; 2] = [
    track(
        1,
        TrackKind::Audio,
        Au6TrackRole::VoiceA,
        Some(Au6Speaker::A),
        Some(AU6_B_VOICE_A_LEVEL_DBFS_HUNDREDTHS),
        false,
    ),
    track(
        2,
        TrackKind::Audio,
        Au6TrackRole::VoiceB,
        Some(Au6Speaker::B),
        Some(AU6_B_VOICE_B_LEVEL_DBFS_HUNDREDTHS),
        false,
    ),
];
const AU6_B_VOICE_A_BUS_TRACKS: [TrackId; 1] = [AU6_B_VOICE_A_TRACK];
const AU6_B_VOICE_B_BUS_TRACKS: [TrackId; 1] = [AU6_B_VOICE_B_TRACK];
const AU6_B_BUSES: [Au6BusSpec; 2] = [
    Au6BusSpec {
        bus: AU6_B_VOICE_A_BUS,
        name: AU6_B_VOICE_A_BUS_NAME,
        tracks: &AU6_B_VOICE_A_BUS_TRACKS,
    },
    Au6BusSpec {
        bus: AU6_B_VOICE_B_BUS,
        name: AU6_B_VOICE_B_BUS_NAME,
        tracks: &AU6_B_VOICE_B_BUS_TRACKS,
    },
];
/// (b)'s base clips: track 1 one clip; track 2 the four clips of
/// [`AU6_B_VOICE_B_CLIP_RANGES`], so the gated clips are ids 3 and 4.
pub const AU6_B_CLIPS: [Au6ClipSpec; 5] = [
    clip(1, 1, 1, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(2, 2, 2, 0, 90),
    clip(3, 2, 2, 90, 103),
    clip(4, 2, 2, 103, 120),
    clip(5, 2, 2, 120, AU6_PROGRAMME_FRAMES as i64),
];

// --- (c) -------------------------------------------------------------------

/// (c)'s one dialogue track.
pub const AU6_C_DIALOGUE_TRACK: TrackId = TrackId(1);
/// (c)'s dialogue asset.
pub const AU6_C_DIALOGUE_ASSET: AssetId = AssetId(1);
/// (c)'s base clip, the one the two splits cut.
pub const AU6_C_BASE_CLIP_ID: ClipId = ClipId(1);
/// (c)'s repair bus — `plan_dialogue_repair` names a fresh bus
/// `"Dialogue repair"` (`crates/kinewright-agent/src/server.rs`).
pub const AU6_C_REPAIR_BUS: AudioBusId = AudioBusId(1);
/// (c)'s repair bus name.
pub const AU6_C_REPAIR_BUS_NAME: &str = "Dialogue repair";
/// S17: the clip id core allocates for the right-hand half of the split at
/// 175 — the base document's next free id, `max + 1 = 2`.
pub const AU6_C_RIGHT_CLIP_ID: ClipId = ClipId(2);
/// S17: the clip id core allocates for the interior `160..175` piece — the
/// split at 160 comes second, so it takes `3` — which `DeleteClip` removes.
pub const AU6_C_MIDDLE_CLIP_ID: ClipId = ClipId(3);
/// After the fill commit the room-tone tile reuses the freed `3`: the clips
/// read `[(1,0), (3,160), (2,175)]` (AU6 §2.6 (c) item 3).
pub const AU6_C_FILL_CLIP_ID: ClipId = ClipId(3);

const AU6_C_TRACKS: [Au6TrackSpec; 1] = [track(
    1,
    TrackKind::Audio,
    Au6TrackRole::Dialogue,
    Some(AU6_C_VOICE_CARRIER),
    Some(AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS),
    false,
)];
const AU6_C_REPAIR_BUS_TRACKS: [TrackId; 1] = [AU6_C_DIALOGUE_TRACK];
const AU6_C_BUSES: [Au6BusSpec; 1] = [Au6BusSpec {
    bus: AU6_C_REPAIR_BUS,
    name: AU6_C_REPAIR_BUS_NAME,
    tracks: &AU6_C_REPAIR_BUS_TRACKS,
}];
/// (c)'s base clip: the whole 312-frame programme on track 1.
pub const AU6_C_CLIPS: [Au6ClipSpec; 1] = [clip(1, 1, 1, 0, AU6_C_PROGRAMME_FRAMES as i64)];

// --- (d) -------------------------------------------------------------------

/// (d)'s first angle track.
pub const AU6_D_ANGLE_1_TRACK: TrackId = TrackId(1);
/// (d)'s second angle track, above the first.
pub const AU6_D_ANGLE_2_TRACK: TrackId = TrackId(2);
/// (d)'s first scratch track, muted by the canonical batch.
pub const AU6_D_SCRATCH_1_TRACK: TrackId = TrackId(3);
/// (d)'s second scratch track, muted by the canonical batch.
pub const AU6_D_SCRATCH_2_TRACK: TrackId = TrackId(4);
/// (d)'s master audio track — **no operation touches it**.
pub const AU6_D_MASTER_TRACK: TrackId = TrackId(5);
/// (d)'s master audio clip.
pub const AU6_D_MASTER_CLIP_ID: ClipId = ClipId(5);
/// (d)'s two angle clips, the ones the six splits cut, in
/// [`AU6_D_ANGLE_OFFSETS_FRAMES`] order.
pub const AU6_D_ANGLE_CLIP_IDS: [ClipId; 2] = [ClipId(1), ClipId(2)];
/// (d)'s two angle assets, in the same order.
pub const AU6_D_ANGLE_ASSETS: [AssetId; 2] = [AssetId(1), AssetId(2)];

const AU6_D_TRACKS: [Au6TrackSpec; 5] = [
    track(1, TrackKind::Video, Au6TrackRole::Angle, None, None, true),
    track(2, TrackKind::Video, Au6TrackRole::Angle, None, None, true),
    track(
        3,
        TrackKind::Audio,
        Au6TrackRole::Scratch,
        None,
        Some(AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS),
        true,
    ),
    track(
        4,
        TrackKind::Audio,
        Au6TrackRole::Scratch,
        None,
        Some(AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS),
        true,
    ),
    track(
        5,
        TrackKind::Audio,
        Au6TrackRole::MasterAudio,
        None,
        Some(AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS),
        true,
    ),
];
/// (d)'s base clips: one per track over the whole programme; asset ids follow
/// track order (1–2 the video-only angles, 3–4 the scratch `.wav`s, 5 the
/// master).
pub const AU6_D_CLIPS: [Au6ClipSpec; 5] = [
    clip(1, 1, 1, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(2, 2, 2, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(3, 3, 3, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(4, 4, 4, 0, AU6_PROGRAMME_FRAMES as i64),
    clip(5, 5, 5, 0, AU6_PROGRAMME_FRAMES as i64),
];

/// The clip ids core allocates for (d)'s six splits, in operation order:
/// angle 1 at 225 / 150 / 75, then angle 2 at 225 / 150 / 75. The base
/// document's highest clip id is 5, so the right-hand halves take 6–11.
pub const AU6_D_SPLIT_CLIP_IDS: [ClipId; 6] = [
    ClipId(6),
    ClipId(7),
    ClipId(8),
    ClipId(9),
    ClipId(10),
    ClipId(11),
];
/// Which angle is visible in each of the four segments `0..75 / 75..150 /
/// 150..225 / 225..300`: 1, 2, 1, 2 — the alternation the three cuts exist
/// to produce.
pub const AU6_D_VISIBLE_ANGLE_PER_SEGMENT: [usize; 4] = [1, 2, 1, 2];
/// The four clips the canonical batch deletes so that exactly one angle
/// exists at every frame: angle 1's `75..150` (8) and `225..300` (6), angle
/// 2's `0..75` (2) and `150..225` (10). Three cuts make four segments per
/// angle, and exclusive alternation deletes one segment per pair.
pub const AU6_D_DELETED_CLIP_IDS: [ClipId; 4] = [ClipId(8), ClipId(6), ClipId(2), ClipId(10)];

// --- the specs -------------------------------------------------------------

/// The five specs, in [`AU6_SCENARIOS`] order.
pub const AU6_SCENARIO_SPECS: [Au6ScenarioSpec; 5] = [
    Au6ScenarioSpec {
        scenario: Au6Scenario::Interview,
        id: "a",
        title: "Two-person interview with a music bed",
        fps: AU6_SOURCE_FPS,
        frames: AU6_PROGRAMME_FRAMES,
        asset_frames: AU6_PROGRAMME_FRAMES,
        sample_rate: AU6_SAMPLE_RATE,
        channels: AU6_CHANNELS,
        document_source: None,
        carries_picture: true,
        delivers: false,
        tracks: &AU6_A_TRACKS,
        buses: &AU6_A_BUSES,
        turns: &AU6_TURNS,
        clips: &AU6_A_CLIPS,
        person_path: Au6PersonPath::Expressible,
    },
    Au6ScenarioSpec {
        scenario: Au6Scenario::Podcast,
        id: "b",
        title: "Podcast with uneven voices",
        fps: AU6_SOURCE_FPS,
        frames: AU6_PROGRAMME_FRAMES,
        asset_frames: AU6_PROGRAMME_FRAMES,
        sample_rate: AU6_SAMPLE_RATE,
        channels: AU6_CHANNELS,
        document_source: None,
        carries_picture: false,
        delivers: false,
        tracks: &AU6_B_TRACKS,
        buses: &AU6_B_BUSES,
        turns: &AU6_TURNS,
        clips: &AU6_B_CLIPS,
        person_path: Au6PersonPath::Expressible,
    },
    Au6ScenarioSpec {
        scenario: Au6Scenario::LocationDialogue,
        id: "c",
        title: "Noisy location dialogue",
        fps: AU6_SOURCE_FPS,
        frames: AU6_C_PROGRAMME_FRAMES,
        asset_frames: AU6_C_PROGRAMME_FRAMES,
        sample_rate: AU6_SAMPLE_RATE,
        channels: AU6_CHANNELS,
        document_source: None,
        carries_picture: false,
        delivers: false,
        tracks: &AU6_C_TRACKS,
        buses: &AU6_C_BUSES,
        turns: &AU6_TURNS,
        clips: &AU6_C_CLIPS,
        person_path: Au6PersonPath::Expressible,
    },
    Au6ScenarioSpec {
        scenario: Au6Scenario::Multicam,
        id: "d",
        title: "Event multicam with a master audio track",
        fps: AU6_SOURCE_FPS,
        frames: AU6_PROGRAMME_FRAMES,
        asset_frames: AU6_PROGRAMME_FRAMES,
        sample_rate: AU6_SAMPLE_RATE,
        channels: AU6_CHANNELS,
        document_source: None,
        carries_picture: true,
        delivers: false,
        tracks: &AU6_D_TRACKS,
        buses: &[],
        turns: &[],
        clips: &AU6_D_CLIPS,
        person_path: Au6PersonPath::Expressible,
    },
    Au6ScenarioSpec {
        scenario: Au6Scenario::Delivery,
        id: "e",
        title: "Encoded delivery at two loudness targets",
        fps: AU6_SOURCE_FPS,
        frames: AU6_ENCODE_PROGRAMME_FRAMES,
        asset_frames: AU6_PROGRAMME_FRAMES,
        sample_rate: AU6_SAMPLE_RATE,
        channels: AU6_CHANNELS,
        document_source: Some(Au6Scenario::Interview),
        carries_picture: true,
        delivers: true,
        tracks: &AU6_A_TRACKS,
        buses: &AU6_A_BUSES,
        turns: &AU6_TURNS,
        clips: &AU6_E_CLIPS,
        person_path: Au6PersonPath::Expressible,
    },
];

/// The turns of a scenario that lie inside its document, clipped to its
/// length — (e)'s third turn `150..200` ends exactly at its 200th frame and
/// its fourth is gone.
#[must_use]
pub fn au6_turns_within(scenario: Au6Scenario) -> Vec<Au6Turn> {
    let frames = TimeCode(i64::from(au6_spec(scenario).frames));
    au6_spec(scenario)
        .turns
        .iter()
        .filter(|turn| turn.start < frames)
        .map(|turn| Au6Turn {
            speaker: turn.speaker,
            start: turn.start,
            end: turn.end.min(frames),
        })
        .collect()
}

/// The spec for one scenario.
#[must_use]
pub const fn au6_spec(scenario: Au6Scenario) -> &'static Au6ScenarioSpec {
    match scenario {
        Au6Scenario::Interview => &AU6_SCENARIO_SPECS[0],
        Au6Scenario::Podcast => &AU6_SCENARIO_SPECS[1],
        Au6Scenario::LocationDialogue => &AU6_SCENARIO_SPECS[2],
        Au6Scenario::Multicam => &AU6_SCENARIO_SPECS[3],
        Au6Scenario::Delivery => &AU6_SCENARIO_SPECS[4],
    }
}

/// The `audio_parametric_eq` effect name (`effect.rs:2010`), the node that
/// carries (b)'s high-pass — `audio_eq` has no high-pass row.
pub const AU6_PARAMETRIC_EQ_EFFECT: &str = "audio_parametric_eq";
/// The `audio_compressor` effect name (`effect.rs:1892`).
pub const AU6_COMPRESSOR_EFFECT: &str = "audio_compressor";
/// The `audio_denoise` effect name (`effect.rs:2231`).
pub const AU6_DENOISE_EFFECT: &str = "audio_denoise";
/// The `audio_hum_removal` effect name (`effect.rs:2240`).
pub const AU6_HUM_REMOVAL_EFFECT: &str = "audio_hum_removal";
/// The `audio_declick` effect name (`effect.rs:2282`).
pub const AU6_DECLICK_EFFECT: &str = "audio_declick";

/// AU6 §2.6 (b), chain **c1m**: the high-pass corner, `high_pass_hertz`.
pub const AU6_PODCAST_HIGHPASS_HERTZ: i64 = 80;
/// c1m: `threshold_tenth_db`.
pub const AU6_PODCAST_COMPRESSOR_THRESHOLD_TENTH_DB: i64 = -400;
/// c1m: `ratio_hundredths`, 4:1.
pub const AU6_PODCAST_COMPRESSOR_RATIO_HUNDREDTHS: i64 = 400;
/// c1m: `attack_milliseconds`.
pub const AU6_PODCAST_COMPRESSOR_ATTACK_MS: i64 = 5;
/// c1m: `release_milliseconds`.
pub const AU6_PODCAST_COMPRESSOR_RELEASE_MS: i64 = 50;
/// c1m: `knee_tenth_db`.
pub const AU6_PODCAST_COMPRESSOR_KNEE_TENTH_DB: i64 = 60;
/// c1m: `makeup_gain_tenth_db`, **load-bearing** (A5): at makeup 0 the
/// compressed bus drops 12.5 LU below the untouched Voice A bus, the voice
/// delta reads 1 250 against the 150 ceiling and the mix LRA 1 274 against
/// the 300 ceiling — both gates fail. +13 dB is inside the descriptor's +24 dB
/// maximum, unlike the alternative c4m's +22 dB.
pub const AU6_PODCAST_COMPRESSOR_MAKEUP_TENTH_DB: i64 = 130;
/// c1m: `detector`, `1` = RMS, `0` = peak.
pub const AU6_PODCAST_COMPRESSOR_DETECTOR: i64 = 1;
/// c1m: `rms_window_milliseconds` — the descriptor neutral, written
/// explicitly because the app's `insert_audio_effect` writes every row.
pub const AU6_PODCAST_COMPRESSOR_RMS_WINDOW_MS: i64 = 10;
/// The effect id the EQ takes on a fresh Voice B chain (`max + 1` within the
/// chain, `crates/kinewright-app/src/mixer_pane_ui.rs:1962-1996`).
pub const AU6_PODCAST_EQ_EFFECT_ID: EffectId = EffectId(1);
/// The effect id the compressor takes, inserted after the EQ.
pub const AU6_PODCAST_COMPRESSOR_EFFECT_ID: EffectId = EffectId(2);

/// AU6 §2.6 (c) item 4: `audio_denoise`'s `reduction_tenth_db`,
/// `DEFAULT_REPAIR_REDUCTION_TENTH_DB` (`crates/kinewright-agent/src/server.rs:10219`).
pub const AU6_C_DENOISE_REDUCTION_TENTH_DB: i64 = 120;
/// `audio_denoise`'s `lookahead_milliseconds`, `REPAIR_DENOISE_LOOKAHEAD_MILLISECONDS`
/// (`server.rs:10226`) — the planner writes it even though it is the
/// descriptor's only value.
pub const AU6_C_DENOISE_LOOKAHEAD_MS: i64 = 12;
/// `audio_hum_removal`'s `fundamental_hertz` on 60 Hz material,
/// `REPAIR_ALTERNATE_HUM_FUNDAMENTAL_HERTZ` (`server.rs:10261`).
pub const AU6_C_HUM_FUNDAMENTAL_PARAMETER_HERTZ: i64 = 60;
/// `audio_hum_removal`'s `harmonic_count`, `REPAIR_HUM_HARMONIC_COUNT = 3`
/// (`server.rs:10228`, A22) — three notched of the five partials authored.
pub const AU6_C_HUM_HARMONIC_COUNT: i64 = 3;
/// `audio_hum_removal`'s `depth_tenth_db`, `REPAIR_HUM_DEPTH_TENTH_DB`
/// (`server.rs:10230`); the planner writes it, so the canonical node carries it.
pub const AU6_C_HUM_DEPTH_TENTH_DB: i64 = -300;
/// `audio_hum_removal`'s `notch_q_hundredths`, `REPAIR_HUM_NOTCH_Q_HUNDREDTHS`
/// (`server.rs:10232`). It **is** the descriptor neutral (`effect.rs:2268`,
/// `neutral: 1_200`) and it **is on the wire**: `plan_dialogue_repair` writes
/// it through `static_audio_effect` (`server.rs:9571`), core's
/// `upsert_audio_bus` stores the bus verbatim (`operation.rs:1796-1810`), and
/// the agent's own `au5_plan_dialogue_repair_builds_the_measured_chain_at_the_head_of_the_bus`
/// asserts the committed node carries it (`server.rs:23643`). The canonical
/// node therefore carries it too, or §5.1(4)'s document equality fails.
pub const AU6_C_HUM_NOTCH_Q_HUNDREDTHS: i64 = 1_200;
/// `audio_declick`'s `max_click_milliseconds`,
/// `REPAIR_DECLICK_MAX_CLICK_MILLISECONDS` (`server.rs:10235`).
pub const AU6_C_DECLICK_MAX_CLICK_MS: i64 = 1;
/// `audio_declick`'s `detector_threshold_tenth_db`,
/// `REPAIR_DECLICK_DETECTOR_THRESHOLD_TENTH_DB` (`server.rs:10238`).
pub const AU6_C_DECLICK_DETECTOR_THRESHOLD_TENTH_DB: i64 = 240;
/// `audio_declick`'s `lookahead_milliseconds`,
/// `REPAIR_DECLICK_LOOKAHEAD_MILLISECONDS` (`server.rs:10241`). A real
/// descriptor row with `min = max = neutral = 3` (`effect.rs:2304-2309`), and
/// **on the wire** for the same reason as [`AU6_C_HUM_NOTCH_Q_HUNDREDTHS`]:
/// the planner writes it (`server.rs:9590`) and the agent's own test asserts
/// the committed node carries it (`server.rs:23655`).
pub const AU6_C_DECLICK_LOOKAHEAD_MS: i64 = 3;
/// The planner's **reported** `chain_lookahead_milliseconds`, `12 + 3`
/// (`server.rs:9699`): a response field, never a payload (A22).
pub const AU6_C_REPAIR_CHAIN_LOOKAHEAD_MS_REPORTED: i64 = 15;
/// The effect ids `next_audio_effect_id` allocates for the repair prefix on
/// (c)'s base document (no effects anywhere): denoise 1, hum 2, declick 3.
pub const AU6_C_REPAIR_EFFECT_IDS: [EffectId; 3] = [EffectId(1), EffectId(2), EffectId(3)];
/// The effect id the declick-only node takes on the fourth (c) document.
pub const AU6_C_DECLICK_ONLY_EFFECT_ID: EffectId = EffectId(1);

/// A neutral bus: no gain, no curve, no effects, no sidechain.
fn bus(id: AudioBusId, name: &str, tracks: &[TrackId], effects: Vec<Effect>) -> AudioBus {
    AudioBus {
        id,
        name: name.to_owned(),
        tracks: tracks.to_vec(),
        gain_tenth_db: 0,
        gain_curve: None,
        effects,
        ducking_sidechain_tracks: Vec::new(),
    }
}

/// A `SetTrackMix` with only the gain set, the shape both trims and the
/// neutral entries take.
const fn track_mix(track: TrackId, gain_tenth_db: i32) -> Operation {
    Operation::SetTrackMix {
        track,
        gain_tenth_db,
        pan_percent: 0,
        mute: false,
        solo: false,
    }
}

/// An effect in the **planner's** shape: only the listed parameters
/// (`static_audio_effect`, `crates/kinewright-agent/src/server.rs:11194`).
fn sparse_effect(id: EffectId, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id,
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect::<BTreeMap<_, _>>(),
        keyframes: BTreeMap::new(),
    }
}

/// An effect in the **app's** shape: every descriptor row written explicitly
/// at its neutral except the 31 profile rows (`insert_audio_effect`,
/// `crates/kinewright-app/src/mixer_pane_ui.rs:1962-1996`), then the listed
/// overrides applied one control at a time.
///
/// # Panics
///
/// Panics when `name` is not a registered effect or an override names a
/// parameter the descriptor does not carry — both are programming errors in
/// this module's own tables.
fn descriptor_effect(id: EffectId, name: &str, overrides: &[(&str, i64)]) -> Effect {
    let descriptor = effect_descriptor(name)
        .unwrap_or_else(|| panic!("{name} is a registered audio effect descriptor"));
    let mut parameters = descriptor
        .parameters
        .iter()
        .filter(|parameter| !NOISE_PROFILE_PARAMETER_NAMES.contains(&parameter.name))
        .map(|parameter| {
            (
                parameter.name.to_owned(),
                ParamValue::Integer(parameter.neutral),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (parameter, value) in overrides {
        assert!(
            parameters.contains_key(*parameter),
            "{parameter} is not a {name} control"
        );
        parameters.insert((*parameter).to_owned(), ParamValue::Integer(*value));
    }
    Effect {
        id,
        name: name.to_owned(),
        parameters,
        keyframes: BTreeMap::new(),
    }
}

/// AU6 §2.6 (b): chain c1m's two nodes, in order — the high-pass EQ, then the
/// compressor — in the app's write-every-row shape, so the person path and
/// the agent's `upsert_audio_bus` land the identical document.
#[must_use]
pub fn au6_b_chain_effects() -> Vec<Effect> {
    vec![
        descriptor_effect(
            AU6_PODCAST_EQ_EFFECT_ID,
            AU6_PARAMETRIC_EQ_EFFECT,
            &[("high_pass_hertz", AU6_PODCAST_HIGHPASS_HERTZ)],
        ),
        descriptor_effect(
            AU6_PODCAST_COMPRESSOR_EFFECT_ID,
            AU6_COMPRESSOR_EFFECT,
            &[
                (
                    "threshold_tenth_db",
                    AU6_PODCAST_COMPRESSOR_THRESHOLD_TENTH_DB,
                ),
                ("ratio_hundredths", AU6_PODCAST_COMPRESSOR_RATIO_HUNDREDTHS),
                ("attack_milliseconds", AU6_PODCAST_COMPRESSOR_ATTACK_MS),
                ("release_milliseconds", AU6_PODCAST_COMPRESSOR_RELEASE_MS),
                ("knee_tenth_db", AU6_PODCAST_COMPRESSOR_KNEE_TENTH_DB),
                (
                    "makeup_gain_tenth_db",
                    AU6_PODCAST_COMPRESSOR_MAKEUP_TENTH_DB,
                ),
                ("detector", AU6_PODCAST_COMPRESSOR_DETECTOR),
                (
                    "rms_window_milliseconds",
                    AU6_PODCAST_COMPRESSOR_RMS_WINDOW_MS,
                ),
            ],
        ),
    ]
}

/// AU6 §2.6 (a): the five operations, in commit order — two trims, two
/// buses, and the duck ride as `SetTrackAutomation` on the bed track's
/// `gain_tenth_db` (the person's route once §6.2 ships; A13 measured all three
/// curve owners bit-identical at the master and the bus).
#[must_use]
pub fn au6_a_canonical_operations() -> Vec<Operation> {
    au6_interview_operations(au6_a_duck_curve())
}

/// B2: (e)'s operations — (a)'s five, with the duck curve truncated to the
/// 200-frame base ([`au6_e_duck_curve`]). Everything but the curve's tail is
/// byte-identical to (a)'s.
#[must_use]
pub fn au6_e_canonical_operations() -> Vec<Operation> {
    au6_interview_operations(au6_e_duck_curve())
}

/// (a)'s batch shape for one duck curve.
fn au6_interview_operations(curve: AutomationCurve) -> Vec<Operation> {
    let spec = au6_spec(Au6Scenario::Interview);
    vec![
        track_mix(AU6_A_VOICE_A_TRACK, 0),
        track_mix(AU6_A_VOICE_B_TRACK, AU6_INTERVIEW_A2_TRIM_TENTH_DB),
        Operation::UpsertAudioBus {
            bus: bus(
                spec.buses[0].bus,
                spec.buses[0].name,
                spec.buses[0].tracks,
                Vec::new(),
            ),
        },
        Operation::UpsertAudioBus {
            bus: bus(
                spec.buses[1].bus,
                spec.buses[1].name,
                spec.buses[1].tracks,
                Vec::new(),
            ),
        },
        Operation::SetTrackAutomation {
            track: AU6_A_BED_TRACK,
            parameter: TRACK_AUTOMATION_PARAMETERS[0].to_owned(),
            curve: Some(curve),
        },
    ]
}

/// AU6 §2.6 (b) items 5–6: the two `SetClipAudio` operations `plan_clip_fades`
/// proposes — one frame in, one frame out, gain untouched — on each of
/// [`AU6_B_GATED_CLIP_IDS`].
#[must_use]
pub fn au6_b_fade_operations() -> Vec<Operation> {
    AU6_B_GATED_CLIP_IDS
        .iter()
        .map(|clip| Operation::SetClipAudio {
            clip: *clip,
            gain_tenth_db: 0,
            fade_in_frames: AU6_B_FADE_FRAMES,
            fade_out_frames: AU6_B_FADE_FRAMES,
        })
        .collect()
}

/// AU6 §2.6 (b): the six operations, in commit order — two trims, the
/// chain-less Voice A bus, the Voice B bus carrying c1m, and the two fades.
/// `plan_audio_normalization`'s `UpsertAudioBus` is an **evidence call** and
/// is not here (A8/E8).
#[must_use]
pub fn au6_b_canonical_operations() -> Vec<Operation> {
    let spec = au6_spec(Au6Scenario::Podcast);
    let mut operations = vec![
        track_mix(AU6_B_VOICE_A_TRACK, AU6_PODCAST_A_TRIM_TENTH_DB),
        track_mix(AU6_B_VOICE_B_TRACK, AU6_PODCAST_B_TRIM_TENTH_DB),
        Operation::UpsertAudioBus {
            bus: bus(
                spec.buses[0].bus,
                spec.buses[0].name,
                spec.buses[0].tracks,
                Vec::new(),
            ),
        },
        Operation::UpsertAudioBus {
            bus: bus(
                spec.buses[1].bus,
                spec.buses[1].name,
                spec.buses[1].tracks,
                au6_b_chain_effects(),
            ),
        },
    ];
    operations.extend(au6_b_fade_operations());
    operations
}

/// AU6 §2.6 (c) item 1 (S17, A10/E10): the first commit. `SplitClip` at 175
/// **then** at 160 — late-to-early, because splitting at 160 first makes the
/// right half a *new* clip and the second split fails
/// `SplitOutsideClip { clip: ClipId(1), at: TimeCode(175) }` — then
/// `DeleteClip` of the interior piece, leaving the gap `160..175`.
///
/// `next_clip_id` is the base document's next free clip id, because §2.1
/// forbids this module from reading a `Document`: the split at 175 takes it
/// and the split at 160 takes the one after. On the canonical base document
/// it is [`AU6_C_RIGHT_CLIP_ID`].
#[must_use]
pub fn au6_c_gap_operations(next_clip_id: ClipId) -> Vec<Operation> {
    let middle = ClipId(next_clip_id.0 + 1);
    vec![
        Operation::SplitClip {
            clip: AU6_C_BASE_CLIP_ID,
            at: AU6_C_GAP_RANGE.end,
        },
        Operation::SplitClip {
            clip: AU6_C_BASE_CLIP_ID,
            at: AU6_C_GAP_RANGE.start,
        },
        Operation::DeleteClip { clip: middle },
    ]
}

/// AU6 §2.6 (c) item 3: the fill commit — the one `AddClip`
/// `plan_room_tone_fill` proposes for `160..175` from the captured room-tone
/// `asset`: one 18-source-frame tile at the gap's start
/// ([`AU6_C_FILL_TILE_SOURCE_RANGE`]).
#[must_use]
pub fn au6_c_fill_operations(asset: AssetId) -> Vec<Operation> {
    vec![Operation::AddClip {
        track: AU6_C_DIALOGUE_TRACK,
        asset,
        at: AU6_C_GAP_RANGE.start,
        source: AU6_C_FILL_TILE_SOURCE_RANGE,
    }]
}

/// The `audio_denoise` node `plan_dialogue_repair` commits: reduction,
/// lookahead, then the 31 learned rows in `NOISE_PROFILE_PARAMETER_NAMES`
/// order.
fn au6_c_denoise_effect() -> Effect {
    let mut parameters = vec![
        ("reduction_tenth_db", AU6_C_DENOISE_REDUCTION_TENTH_DB),
        ("lookahead_milliseconds", AU6_C_DENOISE_LOOKAHEAD_MS),
    ];
    for (name, band) in NOISE_PROFILE_PARAMETER_NAMES
        .iter()
        .zip(AU6_C_LEARNED_PROFILE_TENTH_DB.iter())
    {
        parameters.push((name, i64::from(*band)));
    }
    sparse_effect(AU6_C_REPAIR_EFFECT_IDS[0], AU6_DENOISE_EFFECT, &parameters)
}

/// The `audio_hum_removal` node `plan_dialogue_repair` commits.
fn au6_c_hum_effect() -> Effect {
    sparse_effect(
        AU6_C_REPAIR_EFFECT_IDS[1],
        AU6_HUM_REMOVAL_EFFECT,
        &[
            ("fundamental_hertz", AU6_C_HUM_FUNDAMENTAL_PARAMETER_HERTZ),
            ("harmonic_count", AU6_C_HUM_HARMONIC_COUNT),
            ("depth_tenth_db", AU6_C_HUM_DEPTH_TENTH_DB),
            ("notch_q_hundredths", AU6_C_HUM_NOTCH_Q_HUNDREDTHS),
        ],
    )
}

/// The `audio_declick` node at the planner's three constants, with the id the
/// caller's document allocates.
fn au6_c_declick_effect(id: EffectId) -> Effect {
    sparse_effect(
        id,
        AU6_DECLICK_EFFECT,
        &[
            ("max_click_milliseconds", AU6_C_DECLICK_MAX_CLICK_MS),
            (
                "detector_threshold_tenth_db",
                AU6_C_DECLICK_DETECTOR_THRESHOLD_TENTH_DB,
            ),
            ("lookahead_milliseconds", AU6_C_DECLICK_LOOKAHEAD_MS),
        ],
    )
}

/// AU6 §2.6 (c) item 4: the repair commit — `plan_dialogue_repair`'s single
/// `UpsertAudioBus`, whose effects are, in order, `audio_denoise` (31 profile
/// rows, reduction 120, lookahead 12), `audio_hum_removal` (60 Hz, three
/// harmonics, the planner's depth and Q) and `audio_declick` (the planner's
/// three constants), in the planner's sparse shape.
#[must_use]
pub fn au6_c_repair_operations() -> Vec<Operation> {
    vec![Operation::UpsertAudioBus {
        bus: bus(
            AU6_C_REPAIR_BUS,
            AU6_C_REPAIR_BUS_NAME,
            &AU6_C_REPAIR_BUS_TRACKS,
            vec![
                au6_c_denoise_effect(),
                au6_c_hum_effect(),
                au6_c_declick_effect(AU6_C_REPAIR_EFFECT_IDS[2]),
            ],
        ),
    }]
}

/// A21: the FOURTH (c) document, for §4(c)(3) only — `audio_declick` **alone**
/// on the Dialogue bus at the planner's constants. A fixture document built
/// from (c)'s base document, never a commit in §5.3's ledger: on the canonical
/// chain AU5's error-drop instrument reads 4.5 / −116.9 / 0.5 and admits no
/// floor.
#[must_use]
pub fn au6_c_declick_only_operations() -> Vec<Operation> {
    vec![Operation::UpsertAudioBus {
        bus: bus(
            AU6_C_REPAIR_BUS,
            AU6_C_REPAIR_BUS_NAME,
            &AU6_C_REPAIR_BUS_TRACKS,
            vec![au6_c_declick_effect(AU6_C_DECLICK_ONLY_EFFECT_ID)],
        ),
    }]
}

/// AU6 §5.3: (c)'s three **commits** in ledger order — gap, fill, repair —
/// given the room-tone asset `capture_room_tone` registered between the first
/// two (its own `AddAsset` is Action-applied and is not a commit). Applied to
/// (c)'s base document after that `AddAsset`, this is (c)'s complete canonical
/// document.
#[must_use]
pub fn au6_c_canonical_operations_with_room_tone(room_tone: AssetId) -> Vec<Operation> {
    let mut operations = au6_c_gap_operations(AU6_C_RIGHT_CLIP_ID);
    operations.extend(au6_c_fill_operations(room_tone));
    operations.extend(au6_c_repair_operations());
    operations
}

/// AU6 §2.6 (d): the twelve operations, in commit order — six `SplitClip`
/// late-to-early (225, 150, 75 on angle 1, then on angle 2), the four
/// `DeleteClip` of [`AU6_D_DELETED_CLIP_IDS`] so exactly one angle exists at
/// every frame, and the two scratch mutes. **No operation touches the master
/// track**; `RippleDeleteClip` is never used.
#[must_use]
pub fn au6_d_angle_cut_operations() -> Vec<Operation> {
    let mut operations = Vec::with_capacity(12);
    for clip in AU6_D_ANGLE_CLIP_IDS {
        for at in AU6_D_CUT_FRAMES.iter().rev() {
            operations.push(Operation::SplitClip {
                clip,
                at: TimeCode(*at),
            });
        }
    }
    for clip in AU6_D_DELETED_CLIP_IDS {
        operations.push(Operation::DeleteClip { clip });
    }
    for track in [AU6_D_SCRATCH_1_TRACK, AU6_D_SCRATCH_2_TRACK] {
        operations.push(Operation::SetTrackMix {
            track,
            gain_tenth_db: 0,
            pan_percent: 0,
            mute: true,
            solo: false,
        });
    }
    operations
}

/// AU6 §2.2's `au6_canonical_operations`: the operations in the order a
/// commit must apply them, and the **single** definition of "the canonical
/// document".
///
/// (c) binds a room-tone asset whose record is capture-derived and cannot be
/// pinned in a module that reads no file, so for [`Au6Scenario::LocationDialogue`]
/// this returns the two commits that need none of it — the gap and the repair
/// — and [`au6_c_canonical_operations_with_room_tone`] is the full three-commit
/// ledger, CC7's `cc7_lut_backed_canonical_operations` precedent. (e) is (a)'s
/// batch on the 200-frame base, with the duck curve truncated to it (B2).
#[must_use]
pub fn au6_canonical_operations(scenario: Au6Scenario) -> Vec<Operation> {
    match scenario {
        Au6Scenario::Interview => au6_a_canonical_operations(),
        Au6Scenario::Delivery => au6_e_canonical_operations(),
        Au6Scenario::Podcast => au6_b_canonical_operations(),
        Au6Scenario::LocationDialogue => {
            let mut operations = au6_c_gap_operations(AU6_C_RIGHT_CLIP_ID);
            operations.extend(au6_c_repair_operations());
            operations
        }
        Au6Scenario::Multicam => au6_d_angle_cut_operations(),
    }
}

/// AU6 §2.7: the streaming lane's profile.
pub const AU6_STREAMING_PROFILE: DeliveryProfile = DeliveryProfile::Youtube1080p;
/// AU6 §2.7: the source-master lane's profile.
pub const AU6_SOURCE_MASTER_PROFILE: DeliveryProfile = DeliveryProfile::SourceMaster;
/// AU6 §2.8: `−1 400 − (−2 300) = 900`, the difference the two deliveries
/// must separate by, exact.
pub const AU6_TARGET_SEPARATION_LU_HUNDREDTHS: i32 = STREAMING_PLATFORM_TARGET
    .integrated_lufs_hundredths
    - EBU_R128_PROGRAMME_TARGET.integrated_lufs_hundredths;

/// One canonical (document, job) pair of AU6 §2.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6ExportJob {
    /// `"e_source_master"` or `"e_streaming"`.
    pub id: &'static str,
    pub profile: DeliveryProfile,
    /// Equal to `profile.loudness_target()`.
    pub target: LoudnessTarget,
    /// Always `true`.
    pub normalize: bool,
    /// Always `Eight`.
    pub depth: DeliveryEncodeDepth,
}

/// AU6 §2.7's two rows. (d)'s job is cut (A1).
pub const AU6_EXPORT_JOBS: [Au6ExportJob; 2] = [
    Au6ExportJob {
        id: "e_source_master",
        profile: AU6_SOURCE_MASTER_PROFILE,
        target: EBU_R128_PROGRAMME_TARGET,
        normalize: true,
        depth: DeliveryEncodeDepth::Eight,
    },
    Au6ExportJob {
        id: "e_streaming",
        profile: AU6_STREAMING_PROFILE,
        target: STREAMING_PLATFORM_TARGET,
        normalize: true,
        depth: DeliveryEncodeDepth::Eight,
    },
];

/// The profile's **own** settings for a job — the raster the profile declares
/// (1920×1080 for `e_streaming`), normalization set to the job's target. This
/// is the struct B10's four-field difference set is stated on.
#[must_use]
pub fn au6_profile_export_settings(job: &Au6ExportJob, document: &Document) -> ExportSettings {
    let mut settings =
        job.profile
            .export_settings(document, job.depth, ExportCancellation::default());
    settings.loudness_normalization = job.normalize.then_some(job.target);
    settings
}

/// AU6 §2.7 (A17): the settings the two lanes **render** —
/// [`au6_profile_export_settings`] with `resolution` overridden to
/// `document.resolution`, AU3's `lane_settings` shape
/// (`crates/kinewright-media/tests/au3_fixtures.rs:225-234`). Every audio
/// measurement is raster-invariant to the last centi-unit and the aspect
/// raster costs 8.7× the wall clock, so delivery at the profile's own raster
/// is a hand-run lane.
#[must_use]
pub fn au6_export_settings(job: &Au6ExportJob, document: &Document) -> ExportSettings {
    let mut settings = au6_profile_export_settings(job, document);
    settings.resolution = document.resolution;
    settings
}

/// (a)(1): dialogue over bed, floor. Budget 400 | measured **812** (Dialogue
/// −2 158, Music −2 970) | 2.03×. Failing direction: the un-ducked document
/// at −388.
pub const AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS: i32 = 400;
/// (a)(2): duck depth at `Bus(Music)`, floor. Budget 400 | measured **1 187**
/// at window 4 (1 011 at window 5) | 2.97×. Failing direction: the un-ducked
/// bed at −14.
pub const AU6_DUCK_DEPTH_MIN_HUNDREDTHS: i32 = 400;
/// (a)(3) and (b)(1): voices matched, ceiling. Budget 150 | measured **5** in
/// (a) at `+37` and **50** in (b) through c1m | 30× / 3.0×. Failing
/// directions: un-trimmed 365 (a), 1 442 (b); the nominal `+60` trim 235.
pub const AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS: i32 = 150;
/// (b)(2): spread reduction at `Bus(Voice B)`, floor. Budget 450 | measured
/// **978** (1 482 bypassed → 504) | 2.17×. Failing direction: bypass, 0.
pub const AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS: i32 = 450;
/// (b)(3): programme LRA via `audio_qc`, ceiling. Budget 300 | measured **50**
/// | 6.0×. Failing direction: the makeup-less chain at 1 274.
pub const AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS: i32 = 300;
/// (c)(1): repair SNR gain at `Bus(Dialogue repair)`, floor. Budget 500 | measured
/// **1 142** (2 041 → 3 183) | 2.28×. Failing direction: the neutral profile,
/// 132.
pub const AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS: i32 = 500;
/// (c)(2): the 60 Hz drop in `hum_60_excess_db_hundredths`, floor. Budget
/// 2 400 | measured **4 970** (9 163 → 4 193) | 2.07×. **The field is
/// hundredths** (`crates/kinewright-core/src/audio_repair.rs:117`): N2 Q4's
/// 250 was a tenth-dB figure under a hundredths name and would have been a
/// 19.9× margin (A19). Failing direction: no hum node, 1 tenth dB.
pub const AU6_HUM_DROP_MIN_DB_HUNDREDTHS: i32 = 2_400;
/// (c)(2): each of the **first three** harmonics' drop in
/// `hum_60_harmonic_excess_db_hundredths`, floor. Budget 500 | measured
/// **1 096 / 1 706 / 2 272** at 60 / 120 / 180 Hz | worst 2.19×. The fourth
/// harmonic (240 Hz) is un-notched and its excess **rises** 103; it is
/// recorded, never gated (A19).
pub const AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS: i32 = 500;
/// (c)(3) row 14 (N3): the de-click error drop, node **alone** against the
/// click-free programme, floor. Budget 120 | measured **258** (258.8 on the
/// instrument, nine fresh-engine runs, drift 0.000) | 2.16×. Coincides
/// numerically with two non-budget parameters in the same unit
/// (`reduction_tenth_db 120`, `|depth| 120`); 100 would collide with the
/// targets' tolerance. Failing direction: no declick node, 0.0.
pub const AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB: i32 = 120;
/// (c)(4): speech level retained at `Bus(Dialogue repair)` over the four turns,
/// ceiling. Budget 300 | measured **101** (−2 418 → −2 519) | 2.97×. **The
/// field is hundredths** (`MixWindowLevelReport.windows`); N2 Q4's `10 → 30`
/// was the tenth-dB restatement of the same physical budget.
pub const AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS: i32 = 300;
/// (c)(6): the leakage allowance over the Hann-main-lobe bound, ceiling.
/// Budget 60 | measured worst excess **27** at band 5 | 2.22×. The point-mass
/// model would need 450 (A19).
pub const AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB: i32 = 60;
/// (c)(0): every authored (c) gap's asset RMS below the −35.00 dBFS detector
/// threshold, floor. Budget 250 | measured **526** (learn gap −4 026) |
/// 2.10×. Failing direction: the brief's 10 dB-SNR bed at ≈ −3 400, above the
/// threshold (A4/E4).
pub const AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS: i32 = 250;
/// (d)(3): the mix against the master-alone document. Budget 50 | measured
/// **0** (−2 670 both) | infinite (measured exactly zero); the bound is the
/// failing direction, the unmuted scratch at 600.
pub const AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS: i32 = 50;
/// (e)(1): `|measured − target|` on each written file, ceiling. Budget 25 |
/// measured **0** on both 8 s lanes, worst across all lanes **1** | 25×.
/// Chosen against the measurement, never against the profile's own ±100
/// tolerance (AU3 §0 E55). Failing direction: un-normalized, 122 / 778.
pub const AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS: i32 = 25;
/// (e)(2): `ceiling − measured` true peak, floor. Budget 100 | thinnest
/// measurement the streaming lane's **204** | 2.04×; source master keeps
/// 1 104. Numerically equal to the targets' 100 LU tolerance in a different
/// unit (dBTP hundredths) — recorded, not a substitution.
pub const AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS: i32 = 100;
/// (e)(3): `|(streaming − source_master) − 900|`, ceiling. Budget 25 |
/// measured **0** at 8 s (1 at 12 s) | 25×. Failing direction: two exports at
/// one profile, 0 separation.
pub const AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS: i32 = 25;
/// (b)(5): `plan_audio_normalization`'s `predicted.integrated` against its
/// target, ceiling. Budget 25 | measured **1** at both targets (−2 299 /
/// −1 399) | 25×.
pub const AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS: i32 = 25;
/// §3.2 rule 1 (S3): every authored level against its analytic derivation,
/// ceiling. Budget 25 | worst measured **10** ((c)'s voice, −2 390) | 2.5×.
pub const AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS: i32 = 25;
/// §3.2 rule 2 (S3): the two voices' peak bands apart, floor. Budget 1 |
/// measured **3** (bands 17 and 20) | 3.0×.
pub const AU6_VOICE_BAND_SEPARATION_BANDS: i32 = 1;
/// A1: the media lane's wall clock, **recorded, never gated**. Budget 180 s |
/// projected ≈ 90 s with (d)'s leg cut | 2.0×.
pub const AU6_MEDIA_LANE_BUDGET_SECONDS: i32 = 180;
/// A1: the agent lane's wall clock, **recorded, never gated**. Budget 120 s |
/// projected ≈ 60 s | 2.0×.
pub const AU6_AGENT_LANE_BUDGET_SECONDS: i32 = 120;

// --- The measured column, one constant per §4.1 row ---------------------

/// §4.1 row 1, measured (probe-1 §1.2).
pub const AU6_MEASURED_DIALOGUE_OVER_BED_LU_HUNDREDTHS: i64 = 812;
/// §4.1 row 2, measured at window 4 (probe-2 §1).
pub const AU6_MEASURED_DUCK_DEPTH_HUNDREDTHS: i64 = 1_187;
/// §4.1 row 3, measured at `+37` (probe-1 §1.4).
pub const AU6_MEASURED_INTERVIEW_VOICE_MATCH_LU_HUNDREDTHS: i64 = 5;
/// §4.1 row 4, measured through c1m (probe-1 §2.2).
pub const AU6_MEASURED_PODCAST_VOICE_MATCH_LU_HUNDREDTHS: i64 = 50;
/// §4.1 row 5, measured (probe-1 §2.1–2.2).
pub const AU6_MEASURED_PODCAST_SPREAD_REDUCTION_HUNDREDTHS: i64 = 978;
/// §4.1 row 6, measured (probe-1 §2.2).
pub const AU6_MEASURED_PODCAST_LRA_LU_HUNDREDTHS: i64 = 50;
/// §4.1 row 7, measured at both targets (probe-1 §7.2).
pub const AU6_MEASURED_NORMALIZATION_PREDICTION_LU_HUNDREDTHS: i64 = 1;
/// §4.1 row 8, clipped runs on every scenario and variant (probe-1 §1.5, §2.2).
pub const AU6_MEASURED_CLIPPED_RUNS: i64 = 0;
/// §4.1 row 9, measured (probe-1 §3.1).
pub const AU6_MEASURED_LEARN_GAP_BELOW_SILENCE_HUNDREDTHS: i64 = 526;
/// §4.1 row 10, measured (probe-1 §3.4, probe-2 §5).
pub const AU6_MEASURED_REPAIR_SNR_GAIN_HUNDREDTHS: i64 = 1_142;
/// §4.1 row 11, measured in hundredths (probe-2 §5).
pub const AU6_MEASURED_HUM_DROP_DB_HUNDREDTHS: i64 = 4_970;
/// §4.1 row 12, the worst of the first three harmonics (probe-2 §5).
pub const AU6_MEASURED_HUM_HARMONIC_DROPS_DB_HUNDREDTHS: [i64; 3] = [1_096, 1_706, 2_272];
/// §4.1 row 12's fourth harmonic, which **rises** 103 and is not claimed.
pub const AU6_MEASURED_FOURTH_HARMONIC_RISE_DB_HUNDREDTHS: i64 = 103;
/// §4.1 row 13, clicks after repair on the canonical chain.
pub const AU6_MEASURED_CLICKS_AFTER_REPAIR: i64 = 0;
/// §4.1 row 14, measured (probe-3 §0.1), the instrument's 258.8 in whole
/// tenth dB.
pub const AU6_MEASURED_DECLICK_ERROR_DROP_TENTH_DB: i64 = 258;
/// §4.1 row 15, measured (probe-1 §3.4).
pub const AU6_MEASURED_REPAIR_SPEECH_LOSS_DB_HUNDREDTHS: i64 = 101;
/// §4.1 row 16, the pin's drift over three fresh engines (probe-2 §2.1).
pub const AU6_MEASURED_PROFILE_DRIFT_TENTH_DB: i64 = 0;
/// §4.1 row 18, the room-tone seam (probe-1 §7.5), exactly zero.
pub const AU6_MEASURED_ROOM_TONE_SEAM_SAMPLES: i64 = 0;
/// §4.1 row 19, master-stem samples that differ across the cuts (probe-1
/// §4.5, probe-2 §4.1); the stems are 1 152 000 samples long.
pub const AU6_MEASURED_MASTER_STEM_DIFFERING_SAMPLES: i64 = 0;
/// §4.1 row 19's stem length in **interleaved** samples,
/// `300 frames × 1 920 × 2 channels = 1 152 000`.
pub const AU6_MASTER_STEM_SAMPLES: i64 =
    AU6_PROGRAMME_FRAMES as i64 * AU6_SAMPLES_PER_FRAME as i64 * AU6_CHANNELS as i64;
/// §4.1 row 19's failing direction: a master split at 150 plus a ripple of
/// the tail diverges at interleaved sample `150 × 1 920 × 2 = 576 000` (S15:
/// the channel count is part of the arithmetic; `150 × 1 920` alone is
/// 288 000 sample **frames**).
pub const AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE: i64 =
    AU6_D_CUT_FRAMES[1] * AU6_SAMPLES_PER_FRAME as i64 * AU6_CHANNELS as i64;
/// §4.1 row 20, measured (probe-2 §4.2).
pub const AU6_MEASURED_MASTER_PASSTHROUGH_LU_HUNDREDTHS: i64 = 0;
/// §4.1 row 21: both 8 s lanes read 0; the worst across every lane is the
/// 12 s source master's **1**, which is the row's measured column so the
/// ratio is computable.
pub const AU6_MEASURED_DELIVERY_DEVIATION_WORST_LU_HUNDREDTHS: i64 = 1;
/// §4.1 row 22, the streaming lane's true-peak margin (probe-1 §5, probe-2 §3).
pub const AU6_MEASURED_DELIVERY_TRUE_PEAK_MARGIN_HUNDREDTHS: i64 = 204;
/// The source-master lane's true-peak margin, recorded.
pub const AU6_MEASURED_SOURCE_MASTER_TRUE_PEAK_MARGIN_HUNDREDTHS: i64 = 1_104;
/// S1: the separation term at 12 s (0 at 8 s), the measured column.
pub const AU6_MEASURED_TARGET_SEPARATION_ERROR_LU_HUNDREDTHS: i64 = 1;
/// S2: the worst authored-level error, (c)'s voice.
pub const AU6_MEASURED_AUTHORED_LEVEL_ERROR_HUNDREDTHS: i64 = 10;
/// S3: bands 17 and 20 are three apart.
pub const AU6_MEASURED_VOICE_BAND_SEPARATION_BANDS: i64 = 3;

/// How AU6 §4.1 checks one budget row. Mirrors `Cc7BudgetKind` with `Floor`
/// / `Ceiling` replacing `RatioAtLeastTwo` (AU6 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Au6BudgetKind {
    /// `measured / budget ≥ 2`.
    Floor,
    /// `budget / measured ≥ 2`, measured strictly positive.
    Ceiling,
    /// The measurement is the budget: an exact count, not a bound.
    Exact,
    /// Measured exactly zero: the margin is "infinite (measured exactly
    /// zero)" and the bound is the failing-direction fixture (§4.1 note 3).
    MeasuredZero,
    /// A value pinned between two populations, CC7 §4.1 note 5's bracket
    /// form. AU6 has none (§4.1 note 4).
    TwoSided,
    /// A measurement inside a budget that does **not** clear the 2× bar and is
    /// recorded rather than asserted — the cut-order fallback of §12. AU6
    /// ships none.
    RecordedMargin,
}

/// One row of AU6 §4.1's `budget | measured | margin` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6Budget {
    pub term: &'static str,
    /// `""` for the five rows that carry no constant (S6: rows 8, 13, 16, 18
    /// and 19).
    pub constant: &'static str,
    pub budget: i64,
    pub measured: i64,
    pub kind: Au6BudgetKind,
}

const fn row(
    term: &'static str,
    constant: &'static str,
    budget: i64,
    measured: i64,
    kind: Au6BudgetKind,
) -> Au6Budget {
    Au6Budget {
        term,
        constant,
        budget,
        measured,
        kind,
    }
}

/// AU6 §4.1's 22 rows, one per row of the table (S2, S6): sixteen rows carry
/// their own constant, one (row 4) reuses row 3's, and five (rows 8, 13, 16,
/// 18, 19) carry none.
pub const AU6_BUDGETS: [Au6Budget; 22] = [
    row(
        "dialogue over bed",
        "AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS",
        AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS as i64,
        AU6_MEASURED_DIALOGUE_OVER_BED_LU_HUNDREDTHS,
        Au6BudgetKind::Floor,
    ),
    row(
        "duck depth",
        "AU6_DUCK_DEPTH_MIN_HUNDREDTHS",
        AU6_DUCK_DEPTH_MIN_HUNDREDTHS as i64,
        AU6_MEASURED_DUCK_DEPTH_HUNDREDTHS,
        Au6BudgetKind::Floor,
    ),
    row(
        "voices matched, (a)",
        "AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS",
        AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS as i64,
        AU6_MEASURED_INTERVIEW_VOICE_MATCH_LU_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "voices matched, (b)",
        "AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS",
        AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS as i64,
        AU6_MEASURED_PODCAST_VOICE_MATCH_LU_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "dynamics reduced",
        "AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS",
        AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS as i64,
        AU6_MEASURED_PODCAST_SPREAD_REDUCTION_HUNDREDTHS,
        Au6BudgetKind::Floor,
    ),
    row(
        "programme LRA",
        "AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS",
        AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS as i64,
        AU6_MEASURED_PODCAST_LRA_LU_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "normalization prediction",
        "AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS",
        AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS as i64,
        AU6_MEASURED_NORMALIZATION_PREDICTION_LU_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "clipping, every scenario",
        "",
        0,
        AU6_MEASURED_CLIPPED_RUNS,
        Au6BudgetKind::Exact,
    ),
    row(
        "gap under the detector",
        "AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS",
        AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS as i64,
        AU6_MEASURED_LEARN_GAP_BELOW_SILENCE_HUNDREDTHS,
        Au6BudgetKind::Floor,
    ),
    row(
        "repair SNR gain",
        "AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS",
        AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS as i64,
        AU6_MEASURED_REPAIR_SNR_GAIN_HUNDREDTHS,
        Au6BudgetKind::Floor,
    ),
    row(
        "60 Hz drop",
        "AU6_HUM_DROP_MIN_DB_HUNDREDTHS",
        AU6_HUM_DROP_MIN_DB_HUNDREDTHS as i64,
        AU6_MEASURED_HUM_DROP_DB_HUNDREDTHS,
        Au6BudgetKind::Floor,
    ),
    row(
        "per-harmonic drop, first three",
        "AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS",
        AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS as i64,
        AU6_MEASURED_HUM_HARMONIC_DROPS_DB_HUNDREDTHS[0],
        Au6BudgetKind::Floor,
    ),
    row(
        "clicks after repair",
        "",
        0,
        AU6_MEASURED_CLICKS_AFTER_REPAIR,
        Au6BudgetKind::Exact,
    ),
    row(
        "de-click error drop, node alone against click-free",
        "AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB",
        AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB as i64,
        AU6_MEASURED_DECLICK_ERROR_DROP_TENTH_DB,
        Au6BudgetKind::Floor,
    ),
    row(
        "speech level retained",
        "AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS",
        AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS as i64,
        AU6_MEASURED_REPAIR_SPEECH_LOSS_DB_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "profile band pin",
        "",
        0,
        AU6_MEASURED_PROFILE_DRIFT_TENTH_DB,
        Au6BudgetKind::MeasuredZero,
    ),
    row(
        "profile leakage allowance",
        "AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB",
        AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB as i64,
        AU6_C_LEAKAGE_WORST_EXCESS_TENTH_DB as i64,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "room-tone seam",
        "",
        0,
        AU6_MEASURED_ROOM_TONE_SEAM_SAMPLES,
        Au6BudgetKind::MeasuredZero,
    ),
    row(
        "master stem identity",
        "",
        0,
        AU6_MEASURED_MASTER_STEM_DIFFERING_SAMPLES,
        Au6BudgetKind::MeasuredZero,
    ),
    row(
        "master passthrough",
        "AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS",
        AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS as i64,
        AU6_MEASURED_MASTER_PASSTHROUGH_LU_HUNDREDTHS,
        Au6BudgetKind::MeasuredZero,
    ),
    row(
        "delivery deviation, both lanes",
        "AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS",
        AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS as i64,
        AU6_MEASURED_DELIVERY_DEVIATION_WORST_LU_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "true-peak margin, thinnest lane",
        "AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS",
        AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS as i64,
        AU6_MEASURED_DELIVERY_TRUE_PEAK_MARGIN_HUNDREDTHS,
        Au6BudgetKind::Floor,
    ),
];

/// AU6 §4.1's three rows outside [`AU6_BUDGETS`], because they gate the
/// **sources** rather than the mix (S3); each carries a §11.3 `thresholds`
/// key and a §4.1-shaped manifest row.
pub const AU6_SOURCE_BUDGETS: [Au6Budget; 3] = [
    row(
        "target separation",
        "AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS",
        AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS as i64,
        AU6_MEASURED_TARGET_SEPARATION_ERROR_LU_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "authored level",
        "AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS",
        AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS as i64,
        AU6_MEASURED_AUTHORED_LEVEL_ERROR_HUNDREDTHS,
        Au6BudgetKind::Ceiling,
    ),
    row(
        "voice band separation",
        "AU6_VOICE_BAND_SEPARATION_BANDS",
        AU6_VOICE_BAND_SEPARATION_BANDS as i64,
        AU6_MEASURED_VOICE_BAND_SEPARATION_BANDS,
        Au6BudgetKind::Floor,
    ),
];

/// The unit a threshold constant is stated in, for §2.8's within-unit
/// distinctness rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Au6Unit {
    /// Loudness units, hundredths.
    LuHundredths,
    /// A level or level difference in the dBFS domain, hundredths.
    DbHundredths,
    /// True peak in dBTP, hundredths.
    DbtpHundredths,
    /// Tenths of a decibel.
    TenthDb,
    /// Third-octave bands.
    Bands,
    /// Wall-clock seconds.
    Seconds,
}

/// One AU3 or AU5 budget an AU6 budget must not be silently substitutable
/// for (§2.8), restated here with its owner so the restatement can be checked
/// rather than trusted.
///
/// **R25.** These twelve values used to be twelve `const`s inside
/// `tests/au6_core.rs` with their owners named only in a doc comment, and
/// nothing compared them to those owners: `kinewright-core` cannot see
/// `kinewright-media/tests/au3_fixtures.rs`. If AU3 moved
/// `FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS`, the 147-line distinctness test went
/// on passing while no longer testing what it said it tested — the one place
/// in the slice where the evidence could rot in silence. Publishing the table
/// here gives it two consumers that cannot disagree: the distinctness test in
/// core, and `au6_neighbour_budgets_agree_with_their_owners` in the media
/// lane, which **can** see the owning files and pins each value against the
/// line that declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au6Neighbour {
    /// The owner's constant name, verbatim, as it is declared.
    pub constant: &'static str,
    /// The repo-relative file that declares it, or `""` when the owner is a
    /// struct field rather than a `const` and is compared by value instead.
    pub owner: &'static str,
    /// The unit the value is in; §2.8's distinctness rule is within-unit.
    pub unit: Au6Unit,
    pub value: i64,
}

/// Every neighbour of §2.8, with its owner (R25).
///
/// `DECLICK_ERROR_DROP_BUDGET_TENTH_DB` carries no owner **file**: it is a
/// function-local `const` inside `crates/kinewright-media/src/audio.rs`
/// (`:11004`), so it is reachable by neither name nor a cheap text pin — the
/// file is eleven thousand lines and embedding it would cost more than the
/// check is worth. It is the one entry in this table whose restatement is
/// trusted, and the media lane says so out loud rather than implying all
/// twelve are guarded.
pub const AU6_NEIGHBOUR_BUDGETS: [Au6Neighbour; 12] = [
    neighbour(
        "FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS",
        "crates/kinewright-media/tests/au3_fixtures.rs",
        Au6Unit::LuHundredths,
        100,
    ),
    neighbour(
        "FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS",
        "crates/kinewright-media/tests/au3_fixtures.rs",
        Au6Unit::DbtpHundredths,
        50,
    ),
    neighbour(
        "VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS",
        "crates/kinewright-media/tests/au3_fixtures.rs",
        Au6Unit::DbtpHundredths,
        80,
    ),
    neighbour(
        "DENOISE_FLOOR_DROP_BUDGET_TENTH_DB",
        "crates/kinewright-media/tests/au5_fixtures.rs",
        Au6Unit::TenthDb,
        70,
    ),
    neighbour(
        "DENOISE_TONE_LOSS_BUDGET_TENTH_DB",
        "crates/kinewright-media/tests/au5_fixtures.rs",
        Au6Unit::TenthDb,
        10,
    ),
    neighbour(
        "DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB",
        "crates/kinewright-media/tests/au5_fixtures.rs",
        Au6Unit::TenthDb,
        15,
    ),
    neighbour(
        "DENOISE_PROFILE_NEIGHBOUR_BUDGET_TENTH_DB",
        "crates/kinewright-media/tests/au5_fixtures.rs",
        Au6Unit::TenthDb,
        70,
    ),
    neighbour(
        "HUM_DROP_BUDGET_TENTH_DB",
        "crates/kinewright-media/tests/au5_fixtures.rs",
        Au6Unit::TenthDb,
        140,
    ),
    neighbour(
        "HUM_TONE_LOSS_BUDGET_TENTH_DB",
        "crates/kinewright-media/tests/au5_fixtures.rs",
        Au6Unit::TenthDb,
        15,
    ),
    neighbour(
        "AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS",
        "crates/kinewright-media/tests/au5_fixtures.rs",
        Au6Unit::DbHundredths,
        600,
    ),
    neighbour(
        "DECLICK_ERROR_DROP_BUDGET_TENTH_DB",
        "",
        Au6Unit::TenthDb,
        300,
    ),
    neighbour(
        "LoudnessTarget.tolerance_lu_hundredths",
        "",
        Au6Unit::LuHundredths,
        100,
    ),
];

const fn neighbour(
    constant: &'static str,
    owner: &'static str,
    unit: Au6Unit,
    value: i64,
) -> Au6Neighbour {
    Au6Neighbour {
        constant,
        owner,
        unit,
        value,
    }
}

/// Every §2.8 threshold constant by name, value and unit — the manifest's
/// `thresholds` block (§11.3) and the distinctness test's subject (§2.8).
pub const AU6_THRESHOLD_CONSTANTS: [(&str, i64, Au6Unit); 21] = [
    (
        "AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS",
        AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS as i64,
        Au6Unit::LuHundredths,
    ),
    (
        "AU6_DUCK_DEPTH_MIN_HUNDREDTHS",
        AU6_DUCK_DEPTH_MIN_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS",
        AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS as i64,
        Au6Unit::LuHundredths,
    ),
    (
        "AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS",
        AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS",
        AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS as i64,
        Au6Unit::LuHundredths,
    ),
    (
        "AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS",
        AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_HUM_DROP_MIN_DB_HUNDREDTHS",
        AU6_HUM_DROP_MIN_DB_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS",
        AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB",
        AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB as i64,
        Au6Unit::TenthDb,
    ),
    (
        "AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS",
        AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB",
        AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB as i64,
        Au6Unit::TenthDb,
    ),
    (
        "AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS",
        AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS",
        AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS as i64,
        Au6Unit::LuHundredths,
    ),
    (
        "AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS",
        AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS as i64,
        Au6Unit::LuHundredths,
    ),
    (
        "AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS",
        AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS as i64,
        Au6Unit::DbtpHundredths,
    ),
    (
        "AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS",
        AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS as i64,
        Au6Unit::LuHundredths,
    ),
    (
        "AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS",
        AU6_NORMALIZATION_PREDICTION_MAX_LU_HUNDREDTHS as i64,
        Au6Unit::LuHundredths,
    ),
    (
        "AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS",
        AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS as i64,
        Au6Unit::DbHundredths,
    ),
    (
        "AU6_VOICE_BAND_SEPARATION_BANDS",
        AU6_VOICE_BAND_SEPARATION_BANDS as i64,
        Au6Unit::Bands,
    ),
    (
        "AU6_MEDIA_LANE_BUDGET_SECONDS",
        AU6_MEDIA_LANE_BUDGET_SECONDS as i64,
        Au6Unit::Seconds,
    ),
    (
        "AU6_AGENT_LANE_BUDGET_SECONDS",
        AU6_AGENT_LANE_BUDGET_SECONDS as i64,
        Au6Unit::Seconds,
    ),
];

/// AU6 §8.4: one clause each, in the roadmap row's two words — balance and
/// intelligibility — keyed by base task id. `a5a` and `a5b` carry the same
/// sentence deliberately: the same judgement about two files, answered twice
/// without being told which target each is.
pub const AU6_QUESTIONS: [(&str, &str); 6] = [
    (
        "a1",
        "Is the dialogue clearly balanced above the music bed?",
    ),
    ("a2", "Are both voices at one comfortable level?"),
    ("a3", "Is the dialogue intelligible?"),
    (
        "a4",
        "Is the programme audio at one level across the angle changes?",
    ),
    ("a5a", "Is this delivery at a comfortable listening level?"),
    ("a5b", "Is this delivery at a comfortable listening level?"),
];

/// AU6 §7.2: the six eval task ids, in order.
pub const AU6_TASK_IDS: [&str; 6] = ["a1", "a2", "a3", "a4", "a5a", "a5b"];
