//! AU6 §11.2 items 1–12: the core-owned scenario-authority fixtures.
//!
//! Every expected value here is written out from AU6 §2's tables or derived
//! arithmetically in this file (rule 11.0.1). No expected value is obtained by
//! calling `mix_levels`, `mix_window_levels`, `mix_noise_profile`, `audio_qc`,
//! `audio_repair`, `verify_delivery_audio`, the loudness meter, the limiter,
//! or any planner — none of which core can reach. The three regression pins
//! (`AU6_A_DUCK_KEYFRAMES`, `AU6_C_LEARN_PROJECT_RANGE`,
//! `AU6_C_LEARNED_PROFILE_TENTH_DB`) are read as pins, never re-derived.
//!
//! Fixtures that need a rendered stem, an encoder or a decoded file are the
//! media crate's half of AU6 and are not duplicated here.

use std::{collections::BTreeSet, ops::Range, path::PathBuf};

use kinewright_core::{
    AssetId, AudioMix, BatchError, Clip, ClipContent, ClipId, ColorDescription, Document,
    EBU_R128_PROGRAMME_TARGET, Keyframe, MediaAsset, MediaKind, MediaSourceFingerprint,
    NOISE_PROFILE_BAND_COUNT, NOISE_PROFILE_PARAMETER_NAMES, OpError, Operation, ParamValue,
    Rational, STREAMING_PLATFORM_TARGET, TRACK_AUTOMATION_PARAMETERS, TimeCode, Track, TrackId,
    TrackKind, apply_batch,
    au6_scenarios::{
        AU6_A_BED_TRACK, AU6_A_DUCK_ATTACK_MS, AU6_A_DUCK_DETECTED_WINDOWS, AU6_A_DUCK_HOLD_MS,
        AU6_A_DUCK_KEYFRAME_COUNT, AU6_A_DUCK_KEYFRAMES, AU6_A_DUCK_RAMP_AND_HOLD_MS,
        AU6_A_DUCK_RELEASE_MS, AU6_A_VOICE_A_TRACK, AU6_A_VOICE_B_TRACK,
        AU6_AGENT_LANE_BUDGET_SECONDS, AU6_B_FADE_FRAMES, AU6_B_GATED_CLIP_IDS,
        AU6_B_GATED_CLIP_RANGES, AU6_B_VOICE_B_BUS, AU6_B_VOICE_B_CLIP_RANGES, AU6_BUDGETS,
        AU6_C_ANALYTIC_MEAN_BAND_TENTH_DB, AU6_C_BASE_CLIP_ID, AU6_C_DECLICK_ONLY_EFFECT_ID,
        AU6_C_DETECTED_SILENCES, AU6_C_DIALOGUE_ASSET, AU6_C_DIALOGUE_TRACK, AU6_C_FILL_CLIP_ID,
        AU6_C_FILL_TILE_SOURCE_RANGE, AU6_C_GAP_RANGE, AU6_C_GAPS,
        AU6_C_LEAKAGE_MIN_EXCESS_TENTH_DB, AU6_C_LEAKAGE_WORST_BAND,
        AU6_C_LEAKAGE_WORST_EXCESS_TENTH_DB, AU6_C_LEARN_AUTHORED_RANGE, AU6_C_LEARN_PROJECT_RANGE,
        AU6_C_LEARN_SOURCE_RANGE, AU6_C_LEARNED_PROFILE_TENTH_DB, AU6_C_MIDDLE_CLIP_ID,
        AU6_C_ONE_FRAME_OFFSET_BAND0_TENTH_DB, AU6_C_ONE_FRAME_OFFSET_MAX_DELTA_TENTH_DB,
        AU6_C_POINT_MASS_BAND_TENTH_DB_WRONG_MODEL, AU6_C_POINT_MASS_WORST_BAND,
        AU6_C_POINT_MASS_WORST_EXCESS_TENTH_DB, AU6_C_PROFILE_HUM_BUMP_BANDS,
        AU6_C_PROFILE_LOW_FALLOFF_BANDS, AU6_C_PROFILE_MONOTONE_FROM_BAND, AU6_C_PROGRAMME_FRAMES,
        AU6_C_REPAIR_BUS, AU6_C_REPAIR_BUS_NAME, AU6_C_REPAIR_EFFECT_IDS, AU6_C_RIGHT_CLIP_ID,
        AU6_C_ROOM_TONE_ASSET_FRAMES, AU6_C_ROOM_TONE_FPS, AU6_C_VOICE_CARRIER, AU6_CHANNELS,
        AU6_D_ANGLE_1_TRACK, AU6_D_ANGLE_2_TRACK, AU6_D_ANGLE_ASSETS, AU6_D_ANGLE_CLIP_IDS,
        AU6_D_CUT_FRAMES, AU6_D_DELETED_CLIP_IDS, AU6_D_MASTER_CLIP_ID, AU6_D_MASTER_TRACK,
        AU6_D_SCRATCH_1_TRACK, AU6_D_SCRATCH_2_TRACK, AU6_D_SPLIT_CLIP_IDS,
        AU6_D_VISIBLE_ANGLE_PER_SEGMENT, AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS,
        AU6_E_DUCK_KEYFRAME_COUNT, AU6_E_DUCK_KEYFRAMES, AU6_ENCODE_PROGRAMME_FRAMES,
        AU6_ENCODE_PROGRAMME_SECONDS, AU6_EXPORT_JOBS, AU6_FRAMES_PER_WINDOW, AU6_GAPS,
        AU6_HOP_MILLISECONDS, AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS,
        AU6_LEARN_MINIMUM_PROJECT_FRAMES, AU6_LOUDNESS_GATING_BLOCK_PROJECT_FRAMES,
        AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED, AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS,
        AU6_MASTER_STEM_SAMPLES, AU6_MEASURED_HUM_HARMONIC_DROPS_DB_HUNDREDTHS,
        AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE, AU6_MEDIA_LANE_BUDGET_SECONDS,
        AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED, AU6_PODCAST_COMPRESSOR_MAKEUP_TENTH_DB,
        AU6_PODCAST_HIGHPASS_HERTZ, AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB, AU6_PROGRAMME_FRAMES,
        AU6_QUESTIONS, AU6_SAMPLE_RATE, AU6_SAMPLES_PER_FRAME, AU6_SCENARIO_SPECS, AU6_SCENARIOS,
        AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS, AU6_SOURCE_BUDGETS, AU6_SOURCE_FPS,
        AU6_SOURCE_HEIGHT, AU6_SOURCE_WIDTH, AU6_STREAMING_PROFILE,
        AU6_TARGET_SEPARATION_LU_HUNDREDTHS, AU6_TASK_IDS, AU6_THRESHOLD_CONSTANTS, AU6_TURNS,
        AU6_WINDOW_A_FIRST_TURN, AU6_WINDOW_A_SECOND_TURN, AU6_WINDOW_B_FIRST_TURN,
        AU6_WINDOW_B_SECOND_TURN, AU6_WINDOW_MILLISECONDS, AU6_WINDOW_PROGRAMME, Au6Budget,
        Au6BudgetKind, Au6PersonPath, Au6Scenario, Au6Speaker, Au6TrackRole, Au6Unit,
        au6_c_analytic_mean_band_tenth_db, au6_c_canonical_operations_with_room_tone,
        au6_c_declick_only_operations, au6_c_fill_operations, au6_c_gap_operations,
        au6_c_point_mass_band_tenth_db_wrong_model, au6_c_repair_operations,
        au6_canonical_operations, au6_d_sync_group, au6_duck_gap_parked_milliseconds,
        au6_duck_gap_whole_windows, au6_duck_gap_window_indices, au6_duck_speech_window_indices,
        au6_export_settings, au6_profile_export_settings, au6_spec, au6_turns_of, au6_turns_within,
        au6_window_index,
    },
    effect_descriptor, longest_coverable_project_tile,
};

/// The module's own source, read so item 12 can check the pins' doc comments.
const AU6_SCENARIOS_SOURCE: &str = include_str!("../src/au6_scenarios.rs");

/// One margin ratio for printing. Every AU6 quantity is far inside `2^53`, so
/// the conversion is exact and the precision-loss lint is answered once.
#[allow(clippy::cast_precision_loss)]
fn ratio(numerator: i64, denominator: i64) -> f64 {
    numerator as f64 / denominator as f64
}

// ---------------------------------------------------------------------------
// Neighbouring constants AU6 asserts distinctness from (§2.8).
//
// Each lives in a crate `kinewright-core` cannot see, so it is restated here
// with its owner named. A transcription with a named owner is a boundary.
// ---------------------------------------------------------------------------

/// `FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS`, `kinewright-media/tests/au3_fixtures.rs:38`, LU.
const NEIGHBOUR_AU3_FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS: i64 = 100;
/// `FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS`, `au3_fixtures.rs:47`, dBTP.
const NEIGHBOUR_AU3_FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS: i64 = 50;
/// `VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS`, `au3_fixtures.rs:56`, dBTP.
const NEIGHBOUR_AU3_VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS: i64 = 80;
/// `DENOISE_FLOOR_DROP_BUDGET_TENTH_DB`, `kinewright-media/tests/au5_fixtures.rs:81`.
const NEIGHBOUR_AU5_DENOISE_FLOOR_DROP_BUDGET_TENTH_DB: i64 = 70;
/// `DENOISE_TONE_LOSS_BUDGET_TENTH_DB`, `au5_fixtures.rs:87`.
const NEIGHBOUR_AU5_DENOISE_TONE_LOSS_BUDGET_TENTH_DB: i64 = 10;
/// `DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB`, `au5_fixtures.rs:90`.
const NEIGHBOUR_AU5_DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB: i64 = 15;
/// `DENOISE_PROFILE_NEIGHBOUR_BUDGET_TENTH_DB`, `au5_fixtures.rs:134`.
const NEIGHBOUR_AU5_DENOISE_PROFILE_NEIGHBOUR_BUDGET_TENTH_DB: i64 = 70;
/// `HUM_DROP_BUDGET_TENTH_DB`, `au5_fixtures.rs:141`.
const NEIGHBOUR_AU5_HUM_DROP_BUDGET_TENTH_DB: i64 = 140;
/// `HUM_TONE_LOSS_BUDGET_TENTH_DB`, `au5_fixtures.rs:152`.
const NEIGHBOUR_AU5_HUM_TONE_LOSS_BUDGET_TENTH_DB: i64 = 15;
/// `AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS`, `au5_fixtures.rs:176`, dB.
const NEIGHBOUR_AU5_AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS: i64 = 600;
/// `DECLICK_ERROR_DROP_BUDGET_TENTH_DB`, `kinewright-media/src/audio.rs:11004`.
const NEIGHBOUR_AU5_DECLICK_ERROR_DROP_BUDGET_TENTH_DB: i64 = 300;
/// `LoudnessTarget.tolerance_lu_hundredths` on both targets, LU.
const NEIGHBOUR_TARGET_TOLERANCE_LU_HUNDREDTHS: i64 = 100;

// ---------------------------------------------------------------------------
// Base documents, built off `au6_spec` with no §2 literal restated.
// ---------------------------------------------------------------------------

fn fps(numerator: u32) -> Rational {
    Rational::new(numerator, 1).expect("a positive integer frame rate")
}

fn asset(id: AssetId, kind: MediaKind, rate: u32, frames: i64) -> MediaAsset {
    MediaAsset {
        id,
        path: PathBuf::from(format!("au6-asset-{}", id.0)),
        name: format!("au6 asset {}", id.0),
        duration: TimeCode(frames),
        fps: fps(rate),
        kind,
        resolution: match kind {
            MediaKind::Video | MediaKind::AudioVideo => Some((AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT)),
            MediaKind::Audio => None,
        },
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: ColorDescription::default(),
    }
}

/// The room-tone asset `capture_room_tone` registers: the store's own 30 fps
/// grid, 44 frames (Appendix A row E15).
fn room_tone_asset(id: AssetId) -> MediaAsset {
    asset(
        id,
        MediaKind::Audio,
        AU6_C_ROOM_TONE_FPS,
        AU6_C_ROOM_TONE_ASSET_FRAMES,
    )
}

fn media_clip(id: ClipId, asset: AssetId, range: Range<TimeCode>) -> Clip {
    Clip {
        id,
        asset,
        source_range: range.clone(),
        content: ClipContent::Media,
        timeline_start: range.start,
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
    }
}

/// A scenario's base document, read entirely off its spec.
fn base_document(scenario: Au6Scenario) -> Document {
    let spec = au6_spec(scenario);
    let tracks = spec
        .tracks
        .iter()
        .map(|track| Track {
            id: track.track,
            kind: track.kind,
            sync_lock: track.sync_lock,
            clips: spec
                .clips
                .iter()
                .filter(|clip| clip.track == track.track)
                .map(|clip| media_clip(clip.clip, clip.asset, clip.range()))
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut media_pool = Vec::new();
    for clip in spec.clips {
        if media_pool
            .iter()
            .any(|entry: &MediaAsset| entry.id == clip.asset)
        {
            continue;
        }
        let kind = match spec
            .tracks
            .iter()
            .find(|track| track.track == clip.track)
            .map(|track| track.kind)
        {
            Some(TrackKind::Video) => MediaKind::Video,
            Some(TrackKind::Audio) | None => MediaKind::Audio,
        };
        media_pool.push(asset(
            clip.asset,
            kind,
            spec.fps,
            i64::from(spec.asset_frames),
        ));
    }
    let mut document = Document {
        tracks,
        media_pool,
        fps: fps(spec.fps),
        resolution: (AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT),
        duration: TimeCode(i64::from(spec.frames)),
        ..Document::default()
    };
    if scenario == Au6Scenario::Multicam {
        document
            .catalog
            .sync_groups
            .push(au6_d_sync_group(AU6_D_ANGLE_ASSETS));
    }
    document
        .validate()
        .unwrap_or_else(|error| panic!("{scenario:?}: the base document is valid: {error}"));
    document
}

/// AU6 §2.6 / §6.1 step 6: one operation at a time through the real
/// `apply_batch`, with `Document::validate()` after each (A11).
fn apply_in_order(document: &mut Document, operations: &[Operation]) -> Result<(), String> {
    for (index, operation) in operations.iter().enumerate() {
        apply_batch(document, std::slice::from_ref(operation))
            .map_err(|error| format!("operation {}: {error}", index + 1))?;
        document
            .validate()
            .map_err(|error| format!("after operation {}: {error}", index + 1))?;
    }
    Ok(())
}

/// A scenario's canonical document.
fn canonical_document(scenario: Au6Scenario) -> Document {
    let mut document = base_document(scenario);
    apply_in_order(&mut document, &au6_canonical_operations(scenario))
        .unwrap_or_else(|error| panic!("{scenario:?}: core must accept the batch: {error}"));
    document
}

/// (c)'s complete ledger: gap → the Action's `AddAsset` → fill → repair.
fn location_dialogue_ledger(room_tone: AssetId) -> Vec<Operation> {
    let mut ledger = au6_c_gap_operations(AU6_C_RIGHT_CLIP_ID);
    ledger.push(Operation::AddAsset {
        asset: room_tone_asset(room_tone),
    });
    ledger.extend(au6_c_fill_operations(room_tone));
    ledger.extend(au6_c_repair_operations());
    ledger
}

fn clip_layout(document: &Document, track: TrackId) -> Vec<(u64, i64, i64)> {
    document
        .tracks
        .iter()
        .find(|entry| entry.id == track)
        .expect("the track exists")
        .clips
        .iter()
        .map(|clip| {
            (
                clip.id.0,
                clip.timeline_start.0,
                clip.timeline_start.0
                    + document
                        .clip_duration(clip)
                        .expect("every base clip maps onto the project grid")
                        .0,
            )
        })
        .collect()
}

fn assert_descriptor_controls(effect: &kinewright_core::Effect, context: &str) {
    let descriptor = effect_descriptor(&effect.name)
        .unwrap_or_else(|| panic!("{context}: {} is a registered effect", effect.name));
    for (name, value) in &effect.parameters {
        let ParamValue::Integer(value) = value else {
            panic!("{context}: {name} is an integer control");
        };
        let parameter = descriptor
            .parameter(name)
            .unwrap_or_else(|| panic!("{context}: {name} is not a {} control", effect.name));
        assert!(
            (parameter.min..=parameter.max).contains(value),
            "{context}: {name} = {value} is outside {}..={}",
            parameter.min,
            parameter.max
        );
    }
}

fn integer_parameter(effect: &kinewright_core::Effect, name: &str) -> Option<i64> {
    match effect.parameters.get(name) {
        Some(ParamValue::Integer(value)) => Some(*value),
        _ => None,
    }
}

// ===========================================================================
// §11.2.1 — key `geometry`.
// ===========================================================================

/// §2.3's fps, raster, rate, channels, lengths, the eight ranges,
/// `AU6_SAMPLES_PER_FRAME == 1 920`, 200 ms = 5 frames.
///
/// *Fails:* at 24 fps a frame is 2 000 samples and a 200 ms window is 4.8
/// frames; at 30000/1001 a frame is not a whole number of samples. **§11.2
/// item 1's own 30 fps example is false** — 200 ms is exactly 6 frames of
/// 1 600 samples there — and is asserted false so the erratum is checked.
#[test]
#[allow(clippy::too_many_lines)]
fn au6_scenario_geometry_is_the_contract_table() {
    assert_eq!(AU6_SOURCE_FPS, 25);
    assert_eq!((AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT), (320, 180));
    assert_eq!(AU6_SAMPLE_RATE, 48_000);
    assert_eq!(AU6_CHANNELS, 2);
    assert_eq!(AU6_PROGRAMME_FRAMES, 300);
    assert_eq!(AU6_C_PROGRAMME_FRAMES, 312);
    assert_eq!(AU6_ENCODE_PROGRAMME_SECONDS, 8);
    assert_eq!(
        AU6_ENCODE_PROGRAMME_FRAMES,
        AU6_ENCODE_PROGRAMME_SECONDS * AU6_SOURCE_FPS
    );
    assert_eq!(AU6_ENCODE_PROGRAMME_FRAMES, 200);
    assert_eq!(AU6_SAMPLES_PER_FRAME, AU6_SAMPLE_RATE / AU6_SOURCE_FPS);
    assert_eq!(AU6_SAMPLES_PER_FRAME, 1_920);
    assert_eq!(
        AU6_SAMPLE_RATE % AU6_SOURCE_FPS,
        0,
        "a project frame is a whole number of samples"
    );
    assert_eq!(AU6_WINDOW_MILLISECONDS, 200);
    assert_eq!(AU6_HOP_MILLISECONDS, AU6_WINDOW_MILLISECONDS);
    assert_eq!(AU6_FRAMES_PER_WINDOW, 5);
    let window_samples = AU6_SAMPLE_RATE * AU6_WINDOW_MILLISECONDS / 1_000;
    assert_eq!(window_samples, 9_600);
    assert_eq!(
        window_samples,
        AU6_FRAMES_PER_WINDOW * AU6_SAMPLES_PER_FRAME
    );
    assert_eq!(
        window_samples % AU6_SAMPLES_PER_FRAME,
        0,
        "a 200 ms window is a whole number of 25 fps frames"
    );

    // The turn table: four 2.0 s turns separated by 1.0 s gaps, A B A B.
    assert_eq!(AU6_TURNS.len(), 4);
    let expected: [(Au6Speaker, i64, i64); 4] = [
        (Au6Speaker::A, 0, 50),
        (Au6Speaker::B, 75, 125),
        (Au6Speaker::A, 150, 200),
        (Au6Speaker::B, 225, 275),
    ];
    for (turn, (speaker, start, end)) in AU6_TURNS.iter().zip(expected) {
        assert_eq!(
            (turn.speaker, turn.start.0, turn.end.0),
            (speaker, start, end)
        );
        assert_eq!(turn.frames(), 50, "every turn is 2.0 s");
        assert_eq!(turn.frames() * 1_000 / i64::from(AU6_SOURCE_FPS), 2_000);
    }
    for gap in &AU6_GAPS {
        assert_eq!(gap.end.0 - gap.start.0, 25, "every (a)/(b) gap is 1.0 s");
    }
    // The eight ranges tile the programme contiguously, turn / gap alternating.
    let mut cursor = 0_i64;
    for (turn, gap) in AU6_TURNS.iter().zip(AU6_GAPS.iter()) {
        assert_eq!(turn.start.0, cursor);
        assert_eq!(gap.start, turn.end);
        cursor = gap.end.0;
    }
    assert_eq!(cursor, i64::from(AU6_PROGRAMME_FRAMES));
    // (c)'s eighth gap runs to 312, stated once; the other seven are the same.
    assert_eq!(AU6_C_GAPS[..3], AU6_GAPS[..3]);
    assert_eq!(AU6_C_GAPS[3].start, AU6_GAPS[3].start);
    assert_eq!(AU6_C_GAPS[3].end.0, i64::from(AU6_C_PROGRAMME_FRAMES));
    assert_eq!(AU6_C_GAPS[3].end.0 - AU6_C_GAPS[3].start.0, 37);
    assert_eq!(AU6_C_LEARN_AUTHORED_RANGE, AU6_C_GAPS[3]);
    // Every authored boundary is a multiple of five frames, so the
    // whole-programme window vector is sliced by integer index.
    let mut boundaries = AU6_TURNS
        .iter()
        .flat_map(|turn| [turn.start.0, turn.end.0])
        .chain(AU6_GAPS.iter().flat_map(|gap| [gap.start.0, gap.end.0]))
        .chain(AU6_C_GAPS.iter().flat_map(|gap| [gap.start.0, gap.end.0]))
        .chain([AU6_C_GAP_RANGE.start.0, AU6_C_GAP_RANGE.end.0])
        .chain(AU6_D_CUT_FRAMES)
        .collect::<Vec<_>>();
    boundaries.sort_unstable();
    boundaries.dedup();
    assert_eq!(
        boundaries,
        vec![0, 50, 75, 125, 150, 160, 175, 200, 225, 275, 300, 312]
    );
    // (c)'s programme end is NOT on the grid: 312 = 62 windows + 2 frames,
    // whatever §2.3 says (erratum R12). No (c) gate slices the whole-programme
    // vector past frame 300: the speech-loss gate reads the four turn ranges.
    assert_eq!(
        i64::from(AU6_C_PROGRAMME_FRAMES) % i64::from(AU6_FRAMES_PER_WINDOW),
        2
    );
    let boundaries = boundaries
        .into_iter()
        .filter(|boundary| *boundary != i64::from(AU6_C_PROGRAMME_FRAMES));
    for boundary in boundaries {
        assert_eq!(
            boundary % i64::from(AU6_FRAMES_PER_WINDOW),
            0,
            "{boundary} is not on the 200 ms grid"
        );
        assert_eq!(
            i64::try_from(au6_window_index(TimeCode(boundary))).unwrap()
                * i64::from(AU6_FRAMES_PER_WINDOW),
            boundary
        );
    }
    // The named windows are the turns and the programme.
    assert_eq!(AU6_WINDOW_A_FIRST_TURN, AU6_TURNS[0].range());
    assert_eq!(AU6_WINDOW_B_FIRST_TURN, AU6_TURNS[1].range());
    assert_eq!(AU6_WINDOW_A_SECOND_TURN, AU6_TURNS[2].range());
    assert_eq!(AU6_WINDOW_B_SECOND_TURN, AU6_TURNS[3].range());
    assert_eq!(
        AU6_WINDOW_PROGRAMME,
        TimeCode(0)..TimeCode(i64::from(AU6_PROGRAMME_FRAMES))
    );
    assert_eq!(
        au6_turns_of(Au6Speaker::A),
        vec![AU6_WINDOW_A_FIRST_TURN, AU6_WINDOW_A_SECOND_TURN]
    );
    assert_eq!(
        au6_turns_of(Au6Speaker::B),
        vec![AU6_WINDOW_B_FIRST_TURN, AU6_WINDOW_B_SECOND_TURN]
    );

    // The specs carry the geometry, and (e) is the 200-frame truncation.
    assert_eq!(AU6_SCENARIOS.len(), 5);
    assert_eq!(AU6_SCENARIO_SPECS.len(), AU6_SCENARIOS.len());
    for (index, scenario) in AU6_SCENARIOS.into_iter().enumerate() {
        let spec = au6_spec(scenario);
        assert_eq!(spec.scenario, scenario);
        assert_eq!(AU6_SCENARIO_SPECS[index].scenario, scenario);
        assert_eq!(spec.fps, AU6_SOURCE_FPS);
        assert_eq!(spec.sample_rate, AU6_SAMPLE_RATE);
        assert_eq!(spec.channels, AU6_CHANNELS);
        let expected_frames = match scenario {
            Au6Scenario::LocationDialogue => AU6_C_PROGRAMME_FRAMES,
            Au6Scenario::Delivery => AU6_ENCODE_PROGRAMME_FRAMES,
            _ => AU6_PROGRAMME_FRAMES,
        };
        assert_eq!(spec.frames, expected_frames, "{scenario:?}");
        assert_eq!(
            spec.turns.is_empty(),
            scenario == Au6Scenario::Multicam,
            "{scenario:?}: only (d) has no turns"
        );
        for clip in spec.clips {
            assert!(clip.end.0 <= i64::from(spec.frames), "{scenario:?}");
            assert!(
                spec.tracks.iter().any(|track| track.track == clip.track),
                "{scenario:?}: clip {} names a track the spec has",
                clip.clip.0
            );
        }
        // S11: every voice track names its carrier; nothing else does.
        for track in spec.tracks {
            let expected_carrier = match track.role {
                Au6TrackRole::VoiceA | Au6TrackRole::Dialogue => Some(Au6Speaker::A),
                Au6TrackRole::VoiceB => Some(Au6Speaker::B),
                Au6TrackRole::MusicBed
                | Au6TrackRole::Picture
                | Au6TrackRole::Angle
                | Au6TrackRole::Scratch
                | Au6TrackRole::MasterAudio => None,
            };
            assert_eq!(track.carrier, expected_carrier, "{scenario:?} {track:?}");
            assert_eq!(
                track.level_dbfs_hundredths.is_none(),
                matches!(track.role, Au6TrackRole::Picture | Au6TrackRole::Angle),
                "{scenario:?}: only a picture-only track has no authored level"
            );
            assert_eq!(
                track.kind == TrackKind::Video,
                matches!(track.role, Au6TrackRole::Picture | Au6TrackRole::Angle),
                "{scenario:?} {track:?}"
            );
        }
    }
    assert_eq!(AU6_C_VOICE_CARRIER, Au6Speaker::A);
    // (e)'s third turn ends exactly at its last frame and its fourth is gone.
    let delivery_turns = au6_turns_within(Au6Scenario::Delivery);
    assert_eq!(delivery_turns.len(), 3);
    assert_eq!(
        delivery_turns[2].end.0,
        i64::from(AU6_ENCODE_PROGRAMME_FRAMES)
    );
    assert_eq!(au6_turns_within(Au6Scenario::Interview), AU6_TURNS.to_vec());
    // B1: (a)'s and (e)'s V1 is a video-only picture; (d)'s V1/V2 are angles.
    assert_eq!(
        au6_spec(Au6Scenario::Interview).tracks[0].role,
        Au6TrackRole::Picture
    );
    assert_eq!(
        au6_spec(Au6Scenario::Delivery).tracks[0].role,
        Au6TrackRole::Picture
    );
    assert!(au6_spec(Au6Scenario::Interview).carries_picture);
    assert!(au6_spec(Au6Scenario::Delivery).carries_picture);
    assert!(au6_spec(Au6Scenario::Multicam).carries_picture);
    assert!(!au6_spec(Au6Scenario::Podcast).carries_picture);
    assert!(!au6_spec(Au6Scenario::LocationDialogue).carries_picture);
    let delivering = AU6_SCENARIOS
        .into_iter()
        .filter(|scenario| au6_spec(*scenario).delivers)
        .collect::<Vec<_>>();
    assert_eq!(
        delivering,
        vec![Au6Scenario::Delivery],
        "(d)'s leg is cut (A1)"
    );

    // Failing directions.
    let samples_at_24 = AU6_SAMPLE_RATE / 24;
    assert_eq!(samples_at_24, 2_000);
    assert_ne!(
        window_samples % samples_at_24,
        0,
        "at 24 fps a 200 ms window is 4.8 frames"
    );
    assert_ne!(
        AU6_SAMPLE_RATE * 1_001 % 30_000,
        0,
        "at 30000/1001 a frame is not a whole number of samples"
    );
    // The contract's own 30 fps example: a frame IS 1 600 samples, but a
    // 200 ms window IS six whole frames there, so it is not the failing
    // direction §11.2 item 1 says it is (erratum).
    let samples_at_30 = AU6_SAMPLE_RATE / 30;
    assert_eq!(samples_at_30, 1_600);
    assert_eq!(window_samples % samples_at_30, 0);
    assert_eq!(window_samples / samples_at_30, 6);
    // What 30 fps does break: the A2 ramp lengths stop being whole frames.
    assert_ne!(AU6_A_DUCK_ATTACK_MS * 30 % 1_000, 0);
    assert_eq!(AU6_A_DUCK_HOLD_MS * 30 % 1_000, 0);
    assert_eq!(AU6_A_DUCK_RELEASE_MS * 30 % 1_000, 0);
}

// ===========================================================================
// §11.2.2 — key `geometry.gating`.
// ===========================================================================

/// Every `mix_levels` window is ≥ 10 frames at 25 fps, with the 2.5× / 5.0×
/// margins recorded. *Fails:* a 5-frame window is under the block.
#[test]
fn au6_every_measured_window_clears_one_gating_block() {
    assert_eq!(AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED, 19_200);
    assert_eq!(
        AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED * 1_000 / AU6_SAMPLE_RATE,
        400,
        "the gating block is 400 ms"
    );
    assert_eq!(AU6_LOUDNESS_GATING_BLOCK_PROJECT_FRAMES, 10);
    assert_eq!(
        AU6_LOUDNESS_GATING_BLOCK_PROJECT_FRAMES * AU6_SAMPLES_PER_FRAME,
        AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED,
        "ten whole project frames"
    );
    let block = i64::from(AU6_LOUDNESS_GATING_BLOCK_PROJECT_FRAMES);
    let mut shortest = i64::MAX;
    let mut longest = 0_i64;
    for window in AU6_TURNS
        .iter()
        .map(kinewright_core::au6_scenarios::Au6Turn::range)
        .chain(AU6_GAPS.iter().cloned())
    {
        let frames = window.end.0 - window.start.0;
        assert!(
            frames >= 2 * block,
            "{window:?}: {frames} frames is under 2x the gating block"
        );
        shortest = shortest.min(frames);
        longest = longest.max(frames);
    }
    assert_eq!(shortest, 25, "the shortest measured window is a 1 s gap");
    assert_eq!(longest, 50, "the longest measured window is a 2 s turn");
    // The margins, in tenths: 2.5x and 5.0x.
    assert_eq!(shortest * 10 / block, 25);
    assert_eq!(longest * 10 / block, 50);
    println!(
        "AU6_GATING shortest_frames={shortest} longest_frames={longest} block_frames={block} margin_shortest={:.1}x margin_longest={:.1}x",
        ratio(shortest, block),
        ratio(longest, block)
    );
    // Failing direction: one 200 ms window is under the block, which is why
    // `mix_window_levels` is not BS.1770 and `mix_levels` never reads it.
    assert!(i64::from(AU6_FRAMES_PER_WINDOW) < block);
}

// ===========================================================================
// §11.2.3 — key `geometry.duck`.
// ===========================================================================

/// A2's arithmetic, in A16's one derivation (S2): `attack 150 + hold 200 +
/// release 400 = 750 ms`, so a 1 s gap parks the bed for 250 ms — exactly one
/// whole 200 ms window, the **fourth** of the gap — and a 500 ms gap parks it
/// for none. Asserted by window index, never as a frame count.
///
/// *Fails:* the 500 ms case leaves zero windows; window 5 of a gap is not
/// inside the parked interval the pinned keys describe.
#[test]
#[allow(clippy::too_many_lines)]
fn au6_the_duck_gap_leaves_exactly_one_unducked_window() {
    assert_eq!(
        (
            AU6_A_DUCK_ATTACK_MS,
            AU6_A_DUCK_HOLD_MS,
            AU6_A_DUCK_RELEASE_MS
        ),
        (150, 200, 400)
    );
    assert_eq!(AU6_A_DUCK_RAMP_AND_HOLD_MS, 750);
    assert_eq!(au6_duck_gap_parked_milliseconds(1_000), 250);
    assert_eq!(au6_duck_gap_whole_windows(1_000), 1);
    assert_eq!(au6_duck_gap_parked_milliseconds(500), 0);
    assert_eq!(au6_duck_gap_whole_windows(500), 0, "the brief's 500 ms gap");
    // A16: hold + release sit at the gap's head and the next turn's attack at
    // its tail, so the parked interval runs 600–850 ms into the gap; the
    // 600–800 ms window is window 4 (0-based index 3) and lies wholly inside.
    let head = AU6_A_DUCK_HOLD_MS + AU6_A_DUCK_RELEASE_MS;
    let tail = 1_000 - AU6_A_DUCK_ATTACK_MS;
    assert_eq!((head, tail), (600, 850));
    let window_index_of_ms = |ms: u32| ms / AU6_WINDOW_MILLISECONDS;
    let inside = (0..5)
        .filter(|window| {
            window * AU6_WINDOW_MILLISECONDS >= head
                && (window + 1) * AU6_WINDOW_MILLISECONDS <= tail
        })
        .collect::<Vec<_>>();
    assert_eq!(
        inside,
        vec![3],
        "only window 4 lies wholly inside 600..850 ms"
    );
    assert_eq!(window_index_of_ms(head), 3);
    // Window 5 (700 ms of parked bed, then the attack) is not inside.
    assert!(5 * AU6_WINDOW_MILLISECONDS > tail);

    // The populations as indices into the whole-programme vector.
    let gap_windows = au6_duck_gap_window_indices();
    let expected_gaps = AU6_GAPS
        .iter()
        .map(|gap| au6_window_index(gap.start) + 3)
        .collect::<Vec<_>>();
    assert_eq!(gap_windows.to_vec(), expected_gaps);
    assert_eq!(gap_windows, [13, 28, 43, 58]);
    let speech = au6_duck_speech_window_indices();
    assert_eq!(speech.len(), 36, "nine whole windows per turn, four turns");
    for turn in &AU6_TURNS {
        let first = au6_window_index(turn.start);
        let last = au6_window_index(turn.end);
        assert!(
            !speech.contains(&first),
            "the first window of a turn is an attack ramp"
        );
        assert!(speech.contains(&(first + 1)));
        assert!(speech.contains(&(last - 1)));
        assert!(
            !speech.contains(&last),
            "the window after a turn is a gap window"
        );
    }
    for index in &gap_windows {
        assert!(!speech.contains(index));
    }

    // The pinned keys agree with the derivation: inside each interior gap the
    // parked span (release end .. next attack start) contains window 4 whole
    // and does not contain window 5.
    assert_eq!(AU6_A_DUCK_KEYFRAMES.len(), AU6_A_DUCK_KEYFRAME_COUNT);
    let keys = AU6_A_DUCK_KEYFRAMES;
    for (gap_index, gap_window) in gap_windows.iter().enumerate().take(3) {
        let release_end = keys[4 * gap_index + 3].0;
        let next_attack_start = keys[4 * gap_index + 4].0;
        let window_start = i64::try_from(*gap_window).unwrap() * i64::from(AU6_FRAMES_PER_WINDOW);
        let window_end = window_start + i64::from(AU6_FRAMES_PER_WINDOW);
        assert!(
            release_end <= window_start && window_end <= next_attack_start,
            "gap {gap_index}: window {gap_window} ({window_start}..{window_end}) must sit inside the parked span {release_end}..{next_attack_start}"
        );
        let fifth_end = window_end + i64::from(AU6_FRAMES_PER_WINDOW);
        assert!(
            fifth_end > next_attack_start,
            "gap {gap_index}: window 5 runs into the next turn's attack"
        );
        // The ramps the keys draw are the pinned arguments on the 25 fps grid.
        assert_eq!(
            release_end - keys[4 * gap_index + 2].0,
            i64::from(AU6_A_DUCK_RELEASE_MS * AU6_SOURCE_FPS / 1_000),
            "release is 10 frames"
        );
        assert_eq!(
            keys[4 * gap_index + 5].0 - next_attack_start,
            (i64::from(AU6_A_DUCK_ATTACK_MS * AU6_SOURCE_FPS) + 999) / 1_000,
            "attack is ceil(3.75) = 4 frames"
        );
    }
    // The tail gap has no following turn: the last key is the release's end.
    assert_eq!(keys[15], (289, 0));
    // The detected windows the keys were built from, published beside the
    // authored turns: each opens at or after the authored start and closes
    // four frames after the authored end.
    for ((open, close), turn) in AU6_A_DUCK_DETECTED_WINDOWS.iter().zip(AU6_TURNS.iter()) {
        assert!(
            *open >= turn.start.0 && *open <= turn.start.0 + 2,
            "{open} vs {turn:?}"
        );
        assert_eq!(*close, turn.end.0 + 4, "{turn:?}");
    }
}

// ===========================================================================
// §11.2.4 — key `learn`.
// ===========================================================================

/// `AU6_C_LEARN_PROJECT_RANGE`'s length ≥ `AU6_LEARN_MINIMUM_PROJECT_FRAMES
/// = 12`, and 12 is re-derived from `ceil(22 528 · 25 / 48 000)`.
///
/// *Fails:* an 11-frame gap holds fewer than 22 528 sample frames.
#[test]
fn au6_the_learn_gap_clears_the_profile_minimum() {
    assert_eq!(AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED, 22_528);
    assert_eq!(AU6_LEARN_MINIMUM_PROJECT_FRAMES, 12);
    // `ceil(22 528 × 25 / 48 000) = ceil(11.73)`, derived a second time.
    let numerator = AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED * u64::from(AU6_SOURCE_FPS);
    let floor = numerator / u64::from(AU6_SAMPLE_RATE);
    assert_eq!(floor, 11);
    assert_ne!(numerator % u64::from(AU6_SAMPLE_RATE), 0);
    assert_eq!(floor + 1, AU6_LEARN_MINIMUM_PROJECT_FRAMES);

    let learn = &AU6_C_LEARN_PROJECT_RANGE;
    assert_eq!((learn.start.0, learn.end.0), (274, 312));
    let learn_frames = learn.end.0 - learn.start.0;
    assert_eq!(learn_frames, 38);
    assert!(u64::try_from(learn_frames).unwrap() >= AU6_LEARN_MINIMUM_PROJECT_FRAMES);
    // The authored gap, published beside it, also clears it.
    let authored_frames = AU6_C_LEARN_AUTHORED_RANGE.end.0 - AU6_C_LEARN_AUTHORED_RANGE.start.0;
    assert_eq!(authored_frames, 37);
    assert_eq!(
        AU6_C_LEARN_PROJECT_RANGE.start.0,
        AU6_C_LEARN_AUTHORED_RANGE.start.0 - 1,
        "the detector closes early by one frame (A12)"
    );
    assert_eq!(
        AU6_C_LEARN_PROJECT_RANGE.end,
        AU6_C_LEARN_AUTHORED_RANGE.end
    );
    // The capture's source range is the authored gap (identity mapping).
    assert_eq!(AU6_C_LEARN_SOURCE_RANGE, AU6_C_LEARN_AUTHORED_RANGE);
    // The pinned detector spans: the learn gap is the longest.
    let mut spans = AU6_C_DETECTED_SILENCES.to_vec();
    spans.sort_by_key(|(start, end)| end - start);
    let (longest_start, longest_end) = spans[3];
    assert_eq!((longest_start, longest_end), (learn.start.0, learn.end.0));
    let next_longest = spans[2].1 - spans[2].0;
    assert_eq!(next_longest, 16);
    assert!(
        learn_frames - next_longest >= 21,
        "the learn gap is the longest by at least 21 frames (exactly {})",
        learn_frames - next_longest
    );
    for (start, end) in AU6_C_DETECTED_SILENCES {
        assert!(u64::try_from(end - start).unwrap() >= AU6_LEARN_MINIMUM_PROJECT_FRAMES);
    }
    // In sample frames: 12 frames clear the minimum, 11 do not.
    let samples = |frames: u64| frames * u64::from(AU6_SAMPLES_PER_FRAME);
    assert!(samples(12) >= AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED);
    assert!(
        samples(11) < AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED,
        "an 11-frame gap is {} sample frames short",
        AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED - samples(11)
    );
    assert!(
        samples(u64::try_from(learn_frames).unwrap())
            >= AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED
    );
    // No click sits in the learn gap.
    for frame in kinewright_core::au6_scenarios::AU6_C_CLICK_FRAMES {
        assert!(
            !(learn.start.0..learn.end.0).contains(&frame),
            "click at {frame} is inside the learn gap"
        );
        assert!(
            !(AU6_C_LEARN_AUTHORED_RANGE.start.0..AU6_C_LEARN_AUTHORED_RANGE.end.0)
                .contains(&frame)
        );
    }
    assert_eq!(AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS, -3_500);
    assert_eq!(AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS, 250);
}

// ===========================================================================
// §11.2.5 — key `canonical_documents`.
// ===========================================================================

/// Each batch through `apply_batch` one operation at a time with
/// `validate()` after each (A11), and the document each lands.
#[test]
#[allow(clippy::too_many_lines)]
fn au6_canonical_operations_are_accepted_by_core_in_order() {
    // (a): two trims, two buses, the ride on the bed track.
    let interview = canonical_document(Au6Scenario::Interview);
    let mix = &interview.audio_mix;
    assert!(
        !mix.tracks
            .iter()
            .any(|entry| entry.track == AU6_A_VOICE_A_TRACK),
        "a neutral SetTrackMix stores no entry: the neutrals the planner did not move are absent"
    );
    let voice_b = mix
        .tracks
        .iter()
        .find(|entry| entry.track == AU6_A_VOICE_B_TRACK)
        .unwrap();
    assert_eq!(voice_b.gain_tenth_db, 37);
    let bed = mix
        .tracks
        .iter()
        .find(|entry| entry.track == AU6_A_BED_TRACK)
        .unwrap();
    let curve = bed.gain_curve.as_ref().expect("the bed carries the ride");
    assert_eq!(curve.keyframes.len(), AU6_A_DUCK_KEYFRAME_COUNT);
    for (keyframe, (at, value)) in curve.keyframes.iter().zip(AU6_A_DUCK_KEYFRAMES) {
        assert_eq!((keyframe.at.0, keyframe.value), (at, value));
    }
    assert!(bed.pan_curve.is_none());
    assert_eq!(mix.buses.len(), 2);
    assert_eq!(mix.buses[0].name, "Dialogue");
    assert_eq!(
        mix.buses[0].tracks,
        vec![AU6_A_VOICE_A_TRACK, AU6_A_VOICE_B_TRACK]
    );
    assert_eq!(mix.buses[1].name, "Music");
    assert_eq!(mix.buses[1].tracks, vec![AU6_A_BED_TRACK]);
    assert!(
        mix.buses
            .iter()
            .all(|bus| bus.effects.is_empty() && bus.gain_curve.is_none())
    );
    match &au6_canonical_operations(Au6Scenario::Interview)[4] {
        Operation::SetTrackAutomation {
            track,
            parameter,
            curve,
        } => {
            assert_eq!(*track, AU6_A_BED_TRACK);
            assert_eq!(parameter, TRACK_AUTOMATION_PARAMETERS[0]);
            assert_eq!(parameter, "gain_tenth_db");
            assert!(curve.is_some());
        }
        other => panic!("(a)'s fifth operation is the ride, not {other:?}"),
    }

    // (b): two trims, a chain-less bus, the c1m bus, two fades.
    let podcast = canonical_document(Au6Scenario::Podcast);
    let trims = podcast
        .audio_mix
        .tracks
        .iter()
        .map(|entry| (entry.track.0, entry.gain_tenth_db))
        .collect::<Vec<_>>();
    assert_eq!(trims, vec![(1, 72), (2, -72)]);
    let chain = podcast
        .audio_mix
        .buses
        .iter()
        .find(|bus| bus.id == AU6_B_VOICE_B_BUS)
        .expect("the Voice B bus");
    assert_eq!(chain.name, "Voice B");
    assert!(
        podcast.audio_mix.buses[0].effects.is_empty(),
        "Voice A has no chain"
    );
    let names = chain
        .effects
        .iter()
        .map(|effect| effect.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["audio_parametric_eq", "audio_compressor"]);
    for effect in &chain.effects {
        assert_descriptor_controls(effect, "(b)");
        // The app's shape: every descriptor row is written explicitly.
        let descriptor = effect_descriptor(&effect.name).unwrap();
        for parameter in descriptor.parameters {
            assert!(
                effect.parameters.contains_key(parameter.name),
                "(b) {}: {} is written explicitly, as insert_audio_effect writes it",
                effect.name,
                parameter.name
            );
        }
        assert!(effect.keyframes.is_empty());
    }
    assert_eq!(
        integer_parameter(&chain.effects[0], "high_pass_hertz"),
        Some(AU6_PODCAST_HIGHPASS_HERTZ)
    );
    let compressor = &chain.effects[1];
    let c1m: [(&str, i64); 8] = [
        ("threshold_tenth_db", -400),
        ("ratio_hundredths", 400),
        ("attack_milliseconds", 5),
        ("release_milliseconds", 50),
        ("knee_tenth_db", 60),
        (
            "makeup_gain_tenth_db",
            AU6_PODCAST_COMPRESSOR_MAKEUP_TENTH_DB,
        ),
        ("detector", 1),
        ("rms_window_milliseconds", 10),
    ];
    for (name, value) in c1m {
        assert_eq!(
            integer_parameter(compressor, name),
            Some(value),
            "c1m {name}"
        );
    }
    assert_eq!(AU6_PODCAST_COMPRESSOR_MAKEUP_TENTH_DB, 130);
    for (clip, range) in AU6_B_GATED_CLIP_IDS
        .iter()
        .zip(AU6_B_GATED_CLIP_RANGES.iter())
    {
        let stored = podcast.clip(*clip).expect("a gated clip");
        assert_eq!(stored.timeline_start, range.start);
        assert_eq!(stored.audio_fade_in_frames, AU6_B_FADE_FRAMES);
        assert_eq!(stored.audio_fade_out_frames, AU6_B_FADE_FRAMES);
        assert_eq!(stored.audio_gain_tenth_db, 0);
        assert!(range.start.0 > 0, "no gated clip starts at frame 0 (A6)");
    }
    for clip in [ClipId(1), ClipId(2), ClipId(5)] {
        let stored = podcast.clip(clip).unwrap();
        assert_eq!(
            stored.audio_fade_in_frames,
            TimeCode::ZERO,
            "clip {}",
            clip.0
        );
    }
    assert_eq!(
        clip_layout(&podcast, TrackId(2)),
        AU6_B_VOICE_B_CLIP_RANGES
            .iter()
            .zip(2_u64..)
            .map(|(range, id)| (id, range.start.0, range.end.0))
            .collect::<Vec<_>>()
    );

    // (c): the two asset-free commits alone are accepted …
    let dialogue = canonical_document(Au6Scenario::LocationDialogue);
    assert_eq!(
        dialogue.track_gaps(AU6_C_DIALOGUE_TRACK),
        Some(vec![AU6_C_GAP_RANGE]),
        "without the fill the interior gap is open"
    );
    // … and the full ledger lands the repaired, gap-free document.
    let room_tone = AssetId(2);
    let mut repaired = base_document(Au6Scenario::LocationDialogue);
    apply_in_order(&mut repaired, &location_dialogue_ledger(room_tone))
        .expect("(c)'s four advances apply in order");
    assert_eq!(
        repaired.track_gaps(AU6_C_DIALOGUE_TRACK),
        Some(Vec::new()),
        "N15: Some(empty) is the passing reading; None would mean the track is missing"
    );
    assert_eq!(
        clip_layout(&repaired, AU6_C_DIALOGUE_TRACK),
        vec![(1, 0, 160), (3, 160, 175), (2, 175, 312)]
    );
    let bus = &repaired.audio_mix.buses[0];
    assert_eq!(
        (bus.id, bus.name.as_str()),
        (AU6_C_REPAIR_BUS, AU6_C_REPAIR_BUS_NAME)
    );
    assert_eq!(bus.tracks, vec![AU6_C_DIALOGUE_TRACK]);
    let names = bus
        .effects
        .iter()
        .map(|effect| effect.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["audio_denoise", "audio_hum_removal", "audio_declick"]
    );
    for (effect, id) in bus.effects.iter().zip(AU6_C_REPAIR_EFFECT_IDS) {
        assert_eq!(effect.id, id);
        assert_descriptor_controls(effect, "(c)");
    }
    let denoise = &bus.effects[0];
    assert_eq!(denoise.parameters.len(), 2 + NOISE_PROFILE_BAND_COUNT);
    assert_eq!(integer_parameter(denoise, "reduction_tenth_db"), Some(120));
    assert_eq!(
        integer_parameter(denoise, "lookahead_milliseconds"),
        Some(12)
    );
    for (name, band) in NOISE_PROFILE_PARAMETER_NAMES
        .iter()
        .zip(AU6_C_LEARNED_PROFILE_TENTH_DB)
    {
        assert_eq!(
            integer_parameter(denoise, name),
            Some(i64::from(band)),
            "{name}"
        );
    }
    let hum = &bus.effects[1];
    assert_eq!(
        hum.parameters
            .iter()
            .map(|(name, value)| (
                name.as_str(),
                integer_parameter(hum, name).unwrap_or_else(|| panic!("{value:?}"))
            ))
            .collect::<Vec<_>>(),
        vec![
            ("depth_tenth_db", -300),
            ("fundamental_hertz", 60),
            ("harmonic_count", 3),
            ("notch_q_hundredths", 1_200),
        ],
        "the planner's hum node, verbatim (server.rs:9565-9573, asserted at :23643)"
    );
    let declick = &bus.effects[2];
    assert_eq!(
        declick
            .parameters
            .keys()
            .map(|name| (name.as_str(), integer_parameter(declick, name).unwrap()))
            .collect::<Vec<_>>(),
        vec![
            ("detector_threshold_tenth_db", 240),
            ("lookahead_milliseconds", 3),
            ("max_click_milliseconds", 1),
        ],
        "the planner's declick node, verbatim (server.rs:9580-9594, asserted at :23655)"
    );
    // The fourth (c) document: declick alone, from the base document.
    let mut declick_only = base_document(Au6Scenario::LocationDialogue);
    apply_in_order(&mut declick_only, &au6_c_declick_only_operations()).expect("declick alone");
    let alone = &declick_only.audio_mix.buses[0];
    assert_eq!(alone.effects.len(), 1);
    assert_eq!(alone.effects[0].id, AU6_C_DECLICK_ONLY_EFFECT_ID);
    assert_eq!(alone.effects[0].parameters, declick.parameters);

    // (d): six splits, four deletes, two mutes; the master untouched.
    let multicam = canonical_document(Au6Scenario::Multicam);
    let operations = au6_canonical_operations(Au6Scenario::Multicam);
    assert_eq!(operations.len(), 12);
    assert!(
        operations[..6]
            .iter()
            .all(|operation| matches!(operation, Operation::SplitClip { .. }))
    );
    assert!(
        operations[6..10]
            .iter()
            .all(|operation| matches!(operation, Operation::DeleteClip { .. }))
    );
    assert!(
        operations[10..]
            .iter()
            .all(|operation| matches!(operation, Operation::SetTrackMix { mute: true, .. }))
    );
    assert!(
        !operations
            .iter()
            .any(|operation| matches!(operation, Operation::RippleDeleteClip { .. })),
        "the cuts must not move the master audio"
    );
    assert_eq!(
        clip_layout(&multicam, AU6_D_ANGLE_1_TRACK),
        vec![(1, 0, 75), (7, 150, 225)]
    );
    assert_eq!(
        clip_layout(&multicam, AU6_D_ANGLE_2_TRACK),
        vec![(11, 75, 150), (9, 225, 300)]
    );
    // Exactly one angle exists at every frame, alternating 1 2 1 2.
    let segments = [0_i64, 75, 150, 225, 300];
    for frame in 0..i64::from(AU6_PROGRAMME_FRAMES) {
        let covering = [AU6_D_ANGLE_1_TRACK, AU6_D_ANGLE_2_TRACK]
            .into_iter()
            .enumerate()
            .filter(|(_, track)| {
                clip_layout(&multicam, *track)
                    .iter()
                    .any(|(_, start, end)| (*start..*end).contains(&frame))
            })
            .map(|(index, _)| index + 1)
            .collect::<Vec<_>>();
        let segment = segments.iter().position(|edge| frame < *edge).unwrap() - 1;
        assert_eq!(
            covering,
            vec![AU6_D_VISIBLE_ANGLE_PER_SEGMENT[segment]],
            "frame {frame}: exactly one angle, the alternating one"
        );
    }
    let mut cuts = clip_layout(&multicam, AU6_D_ANGLE_1_TRACK)
        .iter()
        .chain(clip_layout(&multicam, AU6_D_ANGLE_2_TRACK).iter())
        .flat_map(|(_, start, end)| [*start, *end])
        .filter(|edge| *edge != 0 && *edge != i64::from(AU6_PROGRAMME_FRAMES))
        .collect::<Vec<_>>();
    cuts.sort_unstable();
    cuts.dedup();
    assert_eq!(
        cuts,
        AU6_D_CUT_FRAMES.to_vec(),
        "the cuts sit at the authored frames"
    );
    let master = multicam
        .clip(AU6_D_MASTER_CLIP_ID)
        .expect("the master clip");
    let untouched = base_document(Au6Scenario::Multicam);
    assert_eq!(
        master,
        untouched.clip(AU6_D_MASTER_CLIP_ID).unwrap(),
        "no operation touches A3"
    );
    assert_eq!(clip_layout(&multicam, AU6_D_MASTER_TRACK).len(), 1);
    assert_eq!(master.source_range, TimeCode(0)..TimeCode(300));
    assert!(master.effects.is_empty() && master.transition_in.is_none());
    assert_eq!((master.audio_gain_tenth_db, master.speed_percent), (0, 100));
    let muted = multicam
        .audio_mix
        .tracks
        .iter()
        .filter(|entry| entry.mute)
        .map(|entry| entry.track)
        .collect::<Vec<_>>();
    assert_eq!(muted, vec![AU6_D_SCRATCH_1_TRACK, AU6_D_SCRATCH_2_TRACK]);
    assert!(
        !multicam
            .audio_mix
            .tracks
            .iter()
            .any(|entry| entry.track == AU6_D_MASTER_TRACK)
    );
    assert_eq!(
        multicam.catalog.sync_groups.len(),
        1,
        "the sync group is base-document state"
    );
    assert!(
        multicam.tracks.iter().all(|track| track.sync_lock),
        "every (d) track is sync-locked"
    );

    // (e): accepted on the 200-frame base; item 7 compares it with (a).
    let delivery = canonical_document(Au6Scenario::Delivery);
    assert_eq!(
        delivery.duration,
        TimeCode(i64::from(AU6_ENCODE_PROGRAMME_FRAMES))
    );

    // Person-path bookkeeping: five of five.
    let expressible = AU6_SCENARIOS
        .into_iter()
        .filter(|scenario| au6_spec(*scenario).person_path == Au6PersonPath::Expressible)
        .count();
    assert_eq!(expressible, 5);
    let ids = AU6_SCENARIOS
        .into_iter()
        .map(|scenario| au6_spec(scenario).id)
        .collect::<Vec<_>>();
    assert_eq!(ids, ["a", "b", "c", "d", "e"]);
}

/// §11.2.5's failing direction (A10/E10): (c)'s and (d)'s splits in
/// **ascending** order are rejected with `SplitOutsideClip`, because the
/// first split makes the right half a new clip.
#[test]
fn au6_ascending_splits_are_rejected_by_core() {
    let mut dialogue = base_document(Au6Scenario::LocationDialogue);
    let ascending = [
        Operation::SplitClip {
            clip: AU6_C_BASE_CLIP_ID,
            at: AU6_C_GAP_RANGE.start,
        },
        Operation::SplitClip {
            clip: AU6_C_BASE_CLIP_ID,
            at: AU6_C_GAP_RANGE.end,
        },
    ];
    let error = apply_batch(&mut dialogue, &ascending).expect_err("ascending splits are refused");
    assert_eq!(
        error,
        BatchError::OperationFailed {
            op_number: 2,
            error: OpError::SplitOutsideClip {
                clip: AU6_C_BASE_CLIP_ID,
                at: AU6_C_GAP_RANGE.end,
            },
        }
    );

    let mut multicam = base_document(Au6Scenario::Multicam);
    let ascending = AU6_D_CUT_FRAMES
        .iter()
        .map(|at| Operation::SplitClip {
            clip: AU6_D_ANGLE_CLIP_IDS[0],
            at: TimeCode(*at),
        })
        .collect::<Vec<_>>();
    let error = apply_batch(&mut multicam, &ascending).expect_err("ascending splits are refused");
    assert_eq!(
        error,
        BatchError::OperationFailed {
            op_number: 2,
            error: OpError::SplitOutsideClip {
                clip: AU6_D_ANGLE_CLIP_IDS[0],
                at: TimeCode(AU6_D_CUT_FRAMES[1]),
            },
        }
    );
    // The canonical late-to-early order is what the module emits.
    let canonical = au6_canonical_operations(Au6Scenario::Multicam);
    let cut_frames = canonical
        .iter()
        .take(3)
        .map(|operation| match operation {
            Operation::SplitClip { at, .. } => at.0,
            other => panic!("{other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(cut_frames, vec![225, 150, 75]);
    let dialogue_cuts = au6_canonical_operations(Au6Scenario::LocationDialogue)
        .iter()
        .take(2)
        .map(|operation| match operation {
            Operation::SplitClip { at, .. } => at.0,
            other => panic!("{other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(dialogue_cuts, vec![175, 160]);
}

// ===========================================================================
// §11.2.6 — key `canonical_documents.clip_ids`.
// ===========================================================================

/// `AU6_C_RIGHT_CLIP_ID` and `AU6_C_MIDDLE_CLIP_ID` are what core allocates
/// from the base document's next free id (S17), and the fill's one tile is
/// the 18-source-frame head of the 44-frame capture.
///
/// *Fails:* a different base id shifts them, and the pinned gap operations
/// then delete a clip that does not exist.
#[test]
#[allow(clippy::too_many_lines)]
fn au6_core_allocates_the_pinned_gap_clip_ids() {
    assert_eq!(AU6_C_RIGHT_CLIP_ID, ClipId(2));
    assert_eq!(AU6_C_MIDDLE_CLIP_ID, ClipId(3));
    assert_eq!(
        AU6_C_FILL_CLIP_ID, AU6_C_MIDDLE_CLIP_ID,
        "the fill reuses the freed id"
    );

    let mut document = base_document(Au6Scenario::LocationDialogue);
    let highest = document.tracks[0]
        .clips
        .iter()
        .map(|clip| clip.id.0)
        .max()
        .unwrap();
    assert_eq!(
        ClipId(highest + 1),
        AU6_C_RIGHT_CLIP_ID,
        "core allocates max + 1"
    );
    apply_in_order(&mut document, &au6_c_gap_operations(AU6_C_RIGHT_CLIP_ID)).expect("the gap");
    assert_eq!(
        clip_layout(&document, AU6_C_DIALOGUE_TRACK),
        vec![(1, 0, 160), (2, 175, 312)]
    );
    assert_eq!(
        document.track_gaps(AU6_C_DIALOGUE_TRACK),
        Some(vec![AU6_C_GAP_RANGE])
    );
    let right = document
        .clip(AU6_C_RIGHT_CLIP_ID)
        .expect("the right-hand clip survives");
    assert_eq!(right.timeline_start, AU6_C_GAP_RANGE.end);
    assert_eq!(right.source_range, TimeCode(175)..TimeCode(312));
    assert_eq!(right.asset, AU6_C_DIALOGUE_ASSET);
    // S15: the project→source mapping on the surviving clip is the identity,
    // so the capture's source range is its project range.
    assert_eq!(right.source_range.start, right.timeline_start);
    assert!(right.source_range.start <= AU6_C_LEARN_SOURCE_RANGE.start);
    assert!(AU6_C_LEARN_SOURCE_RANGE.end <= right.source_range.end);
    assert!(
        document.clip(AU6_C_MIDDLE_CLIP_ID).is_none(),
        "the interior piece is deleted"
    );

    // The fill: one tile of 18 source frames at 30 fps covers 15 project frames.
    let room_tone = AssetId(2);
    let (covered, source) = longest_coverable_project_tile(
        TimeCode(AU6_C_ROOM_TONE_ASSET_FRAMES),
        fps(AU6_C_ROOM_TONE_FPS),
        fps(AU6_SOURCE_FPS),
        AU6_SAMPLE_RATE,
        TimeCode(AU6_C_GAP_RANGE.end.0 - AU6_C_GAP_RANGE.start.0),
    )
    .expect("the tile is representable");
    assert_eq!(covered, TimeCode(15));
    assert_eq!(source, AU6_C_FILL_TILE_SOURCE_RANGE);
    assert_eq!(source, TimeCode(0)..TimeCode(18));
    assert_eq!(18 * 1_000 / i64::from(AU6_C_ROOM_TONE_FPS), 600, "600 ms");
    assert_eq!(15 * 1_000 / i64::from(AU6_SOURCE_FPS), 600);
    let mut remainder = vec![Operation::AddAsset {
        asset: room_tone_asset(room_tone),
    }];
    remainder.extend(au6_c_fill_operations(room_tone));
    apply_in_order(&mut document, &remainder).expect("the fill");
    assert_eq!(
        clip_layout(&document, AU6_C_DIALOGUE_TRACK),
        vec![(1, 0, 160), (3, 160, 175), (2, 175, 312)]
    );
    assert_eq!(document.track_gaps(AU6_C_DIALOGUE_TRACK), Some(Vec::new()));
    let fill = document.clip(AU6_C_FILL_CLIP_ID).unwrap();
    assert_eq!(fill.asset, room_tone);
    assert_eq!(fill.source_range, AU6_C_FILL_TILE_SOURCE_RANGE);
    assert_eq!(
        au6_c_canonical_operations_with_room_tone(room_tone).len(),
        3 + 1 + 1,
        "gap (3) + fill (1) + repair (1)"
    );

    // Failing direction: a base whose one clip is id 5 allocates 6 and 7, so
    // the pinned ids are wrong and the pinned batch deletes a missing clip.
    let mut shifted = base_document(Au6Scenario::LocationDialogue);
    shifted.tracks[0].clips[0].id = ClipId(5);
    shifted.validate().unwrap();
    let mut with_pinned = shifted.clone();
    let error = apply_batch(&mut with_pinned, &au6_c_gap_operations(AU6_C_RIGHT_CLIP_ID))
        .expect_err("the pinned ids do not fit a shifted base");
    assert!(
        matches!(error, BatchError::OperationFailed { .. }),
        "{error}"
    );
    let mut renumbered = shifted;
    let mut shifted_operations = au6_c_gap_operations(ClipId(6));
    for operation in &mut shifted_operations {
        if let Operation::SplitClip { clip, .. } = operation {
            *clip = ClipId(5);
        }
    }
    apply_in_order(&mut renumbered, &shifted_operations).expect("the shifted batch applies");
    let ids = renumbered.tracks[0]
        .clips
        .iter()
        .map(|clip| clip.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![ClipId(5), ClipId(6)]);
    assert_ne!(
        ids[1], AU6_C_RIGHT_CLIP_ID,
        "a different base id shifts the allocation"
    );

    // (d)'s allocation follows the same rule: six splits take 6..=11.
    let mut multicam = base_document(Au6Scenario::Multicam);
    apply_in_order(
        &mut multicam,
        &au6_canonical_operations(Au6Scenario::Multicam)[..6],
    )
    .unwrap();
    let mut allocated = multicam
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter().map(|clip| clip.id))
        .filter(|clip| clip.0 > 5)
        .collect::<Vec<_>>();
    allocated.sort_unstable();
    assert_eq!(allocated, AU6_D_SPLIT_CLIP_IDS.to_vec());
    for deleted in AU6_D_DELETED_CLIP_IDS {
        assert!(
            multicam.clip(deleted).is_some(),
            "clip {} exists before its delete",
            deleted.0
        );
    }
}

// ===========================================================================
// §11.2.7 — key `canonical_documents.delivery`.
// ===========================================================================

/// (e)'s operations are (a)'s, with the duck curve truncated to the 200-frame
/// base (B2), and (e)'s document is (a)'s truncated to 200 frames.
///
/// *Fails:* (a)'s full sixteen-key curve is **refused** on the 200-frame
/// base, and a divergent batch lands a different document.
#[test]
fn au6_the_delivery_scenario_reuses_the_interview_document() {
    let spec = au6_spec(Au6Scenario::Delivery);
    assert_eq!(spec.document_source, Some(Au6Scenario::Interview));
    assert_eq!(spec.tracks, au6_spec(Au6Scenario::Interview).tracks);
    assert_eq!(spec.buses, au6_spec(Au6Scenario::Interview).buses);
    assert_eq!(spec.frames, AU6_ENCODE_PROGRAMME_FRAMES);
    assert!(spec.delivers && spec.carries_picture);

    let interview = au6_canonical_operations(Au6Scenario::Interview);
    let delivery = au6_canonical_operations(Au6Scenario::Delivery);
    assert_eq!(interview.len(), delivery.len());
    assert_eq!(
        interview[..4],
        delivery[..4],
        "everything but the ride is identical"
    );
    let (
        Operation::SetTrackAutomation {
            curve: Some(full), ..
        },
        Operation::SetTrackAutomation {
            curve: Some(truncated),
            ..
        },
    ) = (&interview[4], &delivery[4])
    else {
        panic!("both batches end in the ride");
    };
    let expected = full
        .keyframes
        .iter()
        .filter(|keyframe| keyframe.at.0 < i64::from(AU6_ENCODE_PROGRAMME_FRAMES))
        .copied()
        .collect::<Vec<Keyframe>>();
    assert_eq!(truncated.keyframes, expected);
    assert_eq!(truncated.keyframes.len(), AU6_E_DUCK_KEYFRAME_COUNT);
    assert_eq!(AU6_E_DUCK_KEYFRAME_COUNT, 10);
    assert_eq!(
        truncated.keyframes,
        AU6_E_DUCK_KEYFRAMES.to_vec(),
        "the published constant is the `at < 200` derivation"
    );
    assert!(
        AU6_E_DUCK_KEYFRAMES
            .iter()
            .all(|keyframe| keyframe.at.0 < i64::from(AU6_ENCODE_PROGRAMME_FRAMES))
    );
    assert_eq!(
        truncated
            .keyframes
            .last()
            .map(|keyframe| (keyframe.at.0, keyframe.value)),
        Some((151, -120)),
        "the ride ends ducked, as probe-2's rebuilt curve did"
    );

    // (e)'s document is (a)'s truncated: the duration, every clip and the
    // curve — never the assets, which stay (a)'s 300-frame buffers (B2).
    let mut truncated_interview = canonical_document(Au6Scenario::Interview);
    let end = TimeCode(i64::from(AU6_ENCODE_PROGRAMME_FRAMES));
    truncated_interview.duration = end;
    assert_eq!(spec.asset_frames, AU6_PROGRAMME_FRAMES);
    assert!(
        truncated_interview
            .media_pool
            .iter()
            .all(|media| media.duration == TimeCode(i64::from(AU6_PROGRAMME_FRAMES)))
    );
    for track in &mut truncated_interview.tracks {
        for clip in &mut track.clips {
            clip.source_range.end = clip.source_range.end.min(end);
        }
    }
    for entry in &mut truncated_interview.audio_mix.tracks {
        if let Some(curve) = &mut entry.gain_curve {
            curve.keyframes.retain(|keyframe| keyframe.at < end);
        }
    }
    truncated_interview
        .validate()
        .expect("the truncation is valid");
    let delivery_document = canonical_document(Au6Scenario::Delivery);
    assert_eq!(delivery_document, truncated_interview);
    assert_eq!(delivery_document.duration, end);

    // Failing direction 1: the full curve does not fit the 200-frame base.
    let mut too_long = base_document(Au6Scenario::Delivery);
    let error = apply_in_order(&mut too_long, &interview)
        .expect_err("a key at frame 204 is outside a 200-frame document");
    assert!(error.starts_with("operation 5"), "{error}");
    // Failing direction 2: a divergent batch lands a different document.
    let mut divergent = base_document(Au6Scenario::Delivery);
    let mut batch = delivery.clone();
    batch.push(Operation::SetTrackMix {
        track: AU6_A_VOICE_A_TRACK,
        gain_tenth_db: 1,
        pan_percent: 0,
        mute: false,
        solo: false,
    });
    apply_in_order(&mut divergent, &batch).unwrap();
    assert_ne!(divergent, delivery_document);
}

// ===========================================================================
// §11.2.8 — key `export_jobs`.
// ===========================================================================

/// §2.7's two rows: a field-by-field comparison **excluding `cancellation`**
/// on the un-overridden profile settings (S1), whose difference set is
/// exactly `resolution`, `video_bitrate`, `audio_bitrate`,
/// `loudness_normalization`; the rendered settings have both `resolution`s
/// overridden to the document's; `delivery_color` identical; neither target
/// carries an LRA maximum.
///
/// *Fails:* a whole-struct `assert_eq!` never holds, because
/// `ExportCancellation`'s `PartialEq` is `Arc::ptr_eq`.
#[test]
#[allow(clippy::too_many_lines)]
fn au6_export_jobs_differ_only_in_the_job() {
    assert_eq!(AU6_EXPORT_JOBS.len(), 2);
    let [source_master, streaming] = &AU6_EXPORT_JOBS;
    assert_eq!(source_master.id, "e_source_master");
    assert_eq!(streaming.id, "e_streaming");
    assert_eq!(streaming.profile, AU6_STREAMING_PROFILE);
    for job in &AU6_EXPORT_JOBS {
        assert_eq!(job.target, job.profile.loudness_target(), "{}", job.id);
        assert!(job.normalize, "{}", job.id);
        assert_eq!(job.depth, kinewright_core::DeliveryEncodeDepth::Eight);
        assert_eq!(job.target.tolerance_lu_hundredths, 100);
        assert_eq!(job.target.true_peak_ceiling_dbtp_hundredths, -100);
        assert!(
            job.target.loudness_range_max_lu_hundredths.is_none(),
            "{}: neither target has an LRA maximum",
            job.id
        );
    }
    assert_eq!(source_master.target, EBU_R128_PROGRAMME_TARGET);
    assert_eq!(streaming.target, STREAMING_PLATFORM_TARGET);
    assert_eq!(source_master.target.integrated_lufs_hundredths, -2_300);
    assert_eq!(streaming.target.integrated_lufs_hundredths, -1_400);
    assert_eq!(AU6_TARGET_SEPARATION_LU_HUNDREDTHS, 900);
    assert_eq!(
        streaming.target.integrated_lufs_hundredths
            - source_master.target.integrated_lufs_hundredths,
        AU6_TARGET_SEPARATION_LU_HUNDREDTHS
    );

    let document = canonical_document(Au6Scenario::Delivery);
    let profile_master = au6_profile_export_settings(source_master, &document);
    let profile_streaming = au6_profile_export_settings(streaming, &document);
    // Identical fields.
    assert_eq!(profile_master.fps, profile_streaming.fps);
    assert_eq!(
        profile_master.delivery_color,
        profile_streaming.delivery_color
    );
    assert_eq!(profile_master.video_codec, profile_streaming.video_codec);
    assert_eq!(profile_master.audio_codec, profile_streaming.audio_codec);
    // The four that differ.
    assert_eq!(profile_master.resolution, document.resolution);
    assert_eq!(
        profile_streaming.resolution,
        (1_920, 1_080),
        "the profile's declared raster"
    );
    assert_ne!(profile_master.resolution, profile_streaming.resolution);
    assert_ne!(
        profile_master.video_bitrate,
        profile_streaming.video_bitrate
    );
    assert_ne!(
        profile_master.audio_bitrate,
        profile_streaming.audio_bitrate
    );
    assert_eq!(
        profile_master.loudness_normalization,
        Some(EBU_R128_PROGRAMME_TARGET)
    );
    assert_eq!(
        profile_streaming.loudness_normalization,
        Some(STREAMING_PLATFORM_TARGET)
    );
    assert_ne!(
        profile_master.loudness_normalization,
        profile_streaming.loudness_normalization
    );

    // The rendered settings: the raster override, and only it, differs.
    let rendered_master = au6_export_settings(source_master, &document);
    let rendered_streaming = au6_export_settings(streaming, &document);
    assert_eq!(rendered_master.resolution, document.resolution);
    assert_eq!(rendered_streaming.resolution, document.resolution);
    assert_eq!(
        rendered_master.resolution,
        (AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT)
    );
    assert_eq!(rendered_master.video_bitrate, profile_master.video_bitrate);
    assert_eq!(
        rendered_streaming.video_bitrate,
        profile_streaming.video_bitrate
    );
    assert_eq!(
        rendered_streaming.audio_bitrate,
        profile_streaming.audio_bitrate
    );
    assert_eq!(
        rendered_streaming.loudness_normalization,
        profile_streaming.loudness_normalization
    );
    assert_eq!(
        rendered_streaming.delivery_color,
        profile_streaming.delivery_color
    );
    assert_ne!(
        rendered_master.video_bitrate,
        rendered_streaming.video_bitrate
    );
    assert_ne!(
        rendered_master.audio_bitrate,
        rendered_streaming.audio_bitrate
    );
    assert_ne!(
        rendered_master.loudness_normalization,
        rendered_streaming.loudness_normalization
    );

    // Failing direction: a whole-struct comparison never holds, even for the
    // same job built twice, because the cancellation token is compared by
    // pointer identity.
    let again = au6_export_settings(source_master, &document);
    assert_ne!(
        rendered_master, again,
        "ExportCancellation's PartialEq is Arc::ptr_eq"
    );
    assert_ne!(rendered_master.cancellation, again.cancellation);
    assert_eq!(
        rendered_master.cancellation,
        rendered_master.cancellation.clone()
    );
}

// ===========================================================================
// §11.2.9 — key `thresholds.distinctness`.
// ===========================================================================

/// No AU6 budget equals a neighbouring AU3 or AU5 budget, or a target's own
/// tolerance, **within its unit** (§2.8). Cross-unit coincidences are
/// recorded rather than asserted away, CC7's precedent.
///
/// *Fails:* a deliberately equal pair in one unit trips the same predicate.
#[test]
#[allow(clippy::too_many_lines)]
fn au6_budgets_are_distinct_from_every_neighbouring_constant() {
    let neighbours: [(&str, Au6Unit, i64); 12] = [
        (
            "FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS",
            Au6Unit::LuHundredths,
            NEIGHBOUR_AU3_FIXTURE_LOUDNESS_BUDGET_HUNDREDTHS,
        ),
        (
            "FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS",
            Au6Unit::DbtpHundredths,
            NEIGHBOUR_AU3_FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS,
        ),
        (
            "VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS",
            Au6Unit::DbtpHundredths,
            NEIGHBOUR_AU3_VERIFY_TONE_TRUE_PEAK_BUDGET_HUNDREDTHS,
        ),
        (
            "DENOISE_FLOOR_DROP_BUDGET_TENTH_DB",
            Au6Unit::TenthDb,
            NEIGHBOUR_AU5_DENOISE_FLOOR_DROP_BUDGET_TENTH_DB,
        ),
        (
            "DENOISE_TONE_LOSS_BUDGET_TENTH_DB",
            Au6Unit::TenthDb,
            NEIGHBOUR_AU5_DENOISE_TONE_LOSS_BUDGET_TENTH_DB,
        ),
        (
            "DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB",
            Au6Unit::TenthDb,
            NEIGHBOUR_AU5_DENOISE_PROFILE_LEARN_BUDGET_TENTH_DB,
        ),
        (
            "DENOISE_PROFILE_NEIGHBOUR_BUDGET_TENTH_DB",
            Au6Unit::TenthDb,
            NEIGHBOUR_AU5_DENOISE_PROFILE_NEIGHBOUR_BUDGET_TENTH_DB,
        ),
        (
            "HUM_DROP_BUDGET_TENTH_DB",
            Au6Unit::TenthDb,
            NEIGHBOUR_AU5_HUM_DROP_BUDGET_TENTH_DB,
        ),
        (
            "HUM_TONE_LOSS_BUDGET_TENTH_DB",
            Au6Unit::TenthDb,
            NEIGHBOUR_AU5_HUM_TONE_LOSS_BUDGET_TENTH_DB,
        ),
        (
            "AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS",
            Au6Unit::DbHundredths,
            NEIGHBOUR_AU5_AUDIO_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS,
        ),
        (
            "DECLICK_ERROR_DROP_BUDGET_TENTH_DB",
            Au6Unit::TenthDb,
            NEIGHBOUR_AU5_DECLICK_ERROR_DROP_BUDGET_TENTH_DB,
        ),
        (
            "LoudnessTarget.tolerance_lu_hundredths",
            Au6Unit::LuHundredths,
            NEIGHBOUR_TARGET_TOLERANCE_LU_HUNDREDTHS,
        ),
    ];
    assert_eq!(
        NEIGHBOUR_TARGET_TOLERANCE_LU_HUNDREDTHS,
        i64::from(EBU_R128_PROGRAMME_TARGET.tolerance_lu_hundredths)
    );
    assert_eq!(
        NEIGHBOUR_TARGET_TOLERANCE_LU_HUNDREDTHS,
        i64::from(STREAMING_PLATFORM_TARGET.tolerance_lu_hundredths)
    );

    let mut compared = 0;
    for (name, value, unit) in AU6_THRESHOLD_CONSTANTS {
        assert!(value != 0, "{name} is zero");
        assert!(
            !name.contains("WINDOWS") && !name.contains("MACOS") && !name.contains("LINUX"),
            "{name}: never a per-OS constant"
        );
        for (neighbour, neighbour_unit, neighbour_value) in neighbours {
            if unit != neighbour_unit {
                continue;
            }
            compared += 1;
            assert_ne!(
                value, neighbour_value,
                "{name} must not equal {neighbour}: a budget equal to its neighbour in the same unit can be silently substituted for it"
            );
        }
        // §2.8 / §11.2.9: no LU budget equals a target's own tolerance, which
        // measures nothing (AU3 §0 E55).
        if unit == Au6Unit::LuHundredths {
            assert_ne!(value, NEIGHBOUR_TARGET_TOLERANCE_LU_HUNDREDTHS, "{name}");
        }
    }
    assert!(
        compared >= 30,
        "the same-unit comparison must not be vacuous: {compared} pairs"
    );
    // One constant per name.
    let names = AU6_THRESHOLD_CONSTANTS
        .iter()
        .map(|(name, _, _)| *name)
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), AU6_THRESHOLD_CONSTANTS.len());
    // Every budget row's constant is in the table at the row's budget.
    for row in AU6_BUDGETS.iter().chain(AU6_SOURCE_BUDGETS.iter()) {
        if row.constant.is_empty() {
            continue;
        }
        let (_, value, _) = AU6_THRESHOLD_CONSTANTS
            .iter()
            .find(|(name, _, _)| *name == row.constant)
            .unwrap_or_else(|| panic!("{} names a constant the table lacks", row.term));
        assert_eq!(*value, row.budget, "{}", row.term);
    }

    // The cross-unit coincidences, recorded rather than asserted away: the
    // true-peak margin is 100 **dBTP** hundredths and the targets' tolerance
    // is 100 **LU** hundredths; the master passthrough is 50 LU and AU3's AAC
    // overshoot 50 dBTP. Neither can stand in for the other.
    assert_eq!(
        i64::from(AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS),
        NEIGHBOUR_TARGET_TOLERANCE_LU_HUNDREDTHS
    );
    assert_eq!(
        i64::from(AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS),
        NEIGHBOUR_AU3_FIXTURE_AAC_OVERSHOOT_BUDGET_HUNDREDTHS
    );
    let true_peak_unit = AU6_THRESHOLD_CONSTANTS
        .iter()
        .find(|(name, _, _)| *name == "AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS")
        .map(|(_, _, unit)| *unit);
    assert_eq!(true_peak_unit, Some(Au6Unit::DbtpHundredths));

    // Failing direction: the predicate trips on a deliberately equal pair.
    let equal_pair = [("AU6_X_LU_HUNDREDTHS", Au6Unit::LuHundredths, 100_i64)];
    let tripped = equal_pair.iter().any(|(_, unit, value)| {
        neighbours
            .iter()
            .any(|(_, neighbour_unit, neighbour_value)| {
                unit == neighbour_unit && value == neighbour_value
            })
    });
    assert!(tripped, "a budget of 100 LU would be caught");
}

// ===========================================================================
// §11.2.10 — key `budgets`.
// ===========================================================================

/// §4.1's 22 rows by `Au6BudgetKind`, plus the three source rows: floors
/// `measured / budget ≥ 2`, ceilings `budget / measured ≥ 2`, exact rows
/// equal, measured-zero rows exactly zero. The leakage row's measured 27 is
/// re-derived from the pin and the analytic bound, and the analytic bound
/// itself is checked against probe-2's independent evaluation.
///
/// *Fails:* a floor whose measurement equals its budget.
#[test]
#[allow(clippy::too_many_lines)]
fn au6_every_budget_carries_the_declared_margin() {
    fn check(row: &Au6Budget) {
        let margin = match row.kind {
            Au6BudgetKind::Floor => {
                assert!(row.budget > 0, "{}: a floor is positive", row.term);
                assert!(
                    row.measured >= 2 * row.budget,
                    "{} ({}): measured {} over budget {} is below the 2x bar",
                    row.term,
                    row.constant,
                    row.measured,
                    row.budget
                );
                ratio(row.measured, row.budget)
            }
            Au6BudgetKind::Ceiling => {
                assert!(
                    row.measured > 0,
                    "{}: a ceiling row measured zero belongs in MeasuredZero",
                    row.term
                );
                assert!(
                    row.budget >= 2 * row.measured,
                    "{} ({}): budget {} over measured {} is below the 2x bar",
                    row.term,
                    row.constant,
                    row.budget,
                    row.measured
                );
                ratio(row.budget, row.measured)
            }
            Au6BudgetKind::Exact => {
                assert_eq!(
                    row.measured, row.budget,
                    "{}: an exact term measures its budget",
                    row.term
                );
                f64::INFINITY
            }
            Au6BudgetKind::MeasuredZero => {
                assert_eq!(
                    row.measured, 0,
                    "{}: a MeasuredZero row measures exactly zero",
                    row.term
                );
                f64::INFINITY
            }
            Au6BudgetKind::TwoSided | Au6BudgetKind::RecordedMargin => {
                panic!("{}: AU6 ships no {:?} row", row.term, row.kind)
            }
        };
        let rendered = if margin.is_infinite() {
            "infinite (measured exactly zero)".to_owned()
        } else {
            format!("{margin:.3}x")
        };
        println!(
            "AU6_BUDGET term={:?} constant={:?} budget={} measured={} kind={:?} margin={rendered}",
            row.term, row.constant, row.budget, row.measured, row.kind
        );
    }

    assert_eq!(AU6_BUDGETS.len(), 22);
    assert_eq!(AU6_SOURCE_BUDGETS.len(), 3);
    for row in AU6_BUDGETS.iter().chain(AU6_SOURCE_BUDGETS.iter()) {
        check(row);
    }
    // S6: one row reuses a constant, five carry none.
    let named = AU6_BUDGETS
        .iter()
        .filter(|row| !row.constant.is_empty())
        .collect::<Vec<_>>();
    let distinct = named
        .iter()
        .map(|row| row.constant)
        .collect::<BTreeSet<_>>();
    assert_eq!(named.len(), 17);
    assert_eq!(distinct.len(), 16, "row 4 reuses row 3's constant");
    assert_eq!(AU6_BUDGETS.len() - named.len(), 5);
    let unnamed = AU6_BUDGETS
        .iter()
        .enumerate()
        .filter(|(_, row)| row.constant.is_empty())
        .map(|(index, _)| index + 1)
        .collect::<Vec<_>>();
    assert_eq!(unnamed, vec![8, 13, 16, 18, 19]);
    assert!(
        AU6_SOURCE_BUDGETS
            .iter()
            .all(|row| !row.constant.is_empty())
    );
    assert_eq!(
        AU6_SOURCE_BUDGETS.map(|row| row.constant),
        [
            "AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS",
            "AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS",
            "AU6_VOICE_BAND_SEPARATION_BANDS",
        ],
        "S7: the three source rows"
    );
    // Every base document starts with a neutral mix, so the canonical batch
    // is the only mix state the canonical document carries.
    assert!(AudioMix::default().is_empty());
    for scenario in AU6_SCENARIOS {
        assert!(base_document(scenario).audio_mix.is_empty(), "{scenario:?}");
    }
    let kinds = |kind: Au6BudgetKind| AU6_BUDGETS.iter().filter(|row| row.kind == kind).count();
    assert_eq!(kinds(Au6BudgetKind::Floor), 9);
    assert_eq!(kinds(Au6BudgetKind::Ceiling), 7);
    assert_eq!(kinds(Au6BudgetKind::Exact), 2);
    assert_eq!(kinds(Au6BudgetKind::MeasuredZero), 4);
    assert_eq!(kinds(Au6BudgetKind::TwoSided), 0);
    assert_eq!(kinds(Au6BudgetKind::RecordedMargin), 0);
    // Row 12 is the worst of the first three harmonics; the fourth rises.
    let harmonic = &AU6_BUDGETS[11];
    assert_eq!(
        harmonic.measured,
        *AU6_MEASURED_HUM_HARMONIC_DROPS_DB_HUNDREDTHS
            .iter()
            .min()
            .unwrap()
    );
    assert!(
        AU6_MEASURED_HUM_HARMONIC_DROPS_DB_HUNDREDTHS
            .iter()
            .all(|drop| *drop >= 2 * harmonic.budget)
    );
    // The recorded-not-gated lanes keep the same 2x shape.
    assert_eq!(
        (AU6_MEDIA_LANE_BUDGET_SECONDS, AU6_AGENT_LANE_BUDGET_SECONDS),
        (180, 120)
    );
    // S15: the (d) arithmetic carries the channel count.
    assert_eq!(AU6_MASTER_STEM_SAMPLES, 1_152_000);
    assert_eq!(AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE, 576_000);
    assert_eq!(
        AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE,
        AU6_D_CUT_FRAMES[1] * i64::from(AU6_SAMPLES_PER_FRAME) * i64::from(AU6_CHANNELS)
    );
    assert_ne!(
        AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE,
        AU6_D_CUT_FRAMES[1] * i64::from(AU6_SAMPLES_PER_FRAME)
    );

    // Row 17: the leakage excess, re-derived from the pin and the bound.
    let bound = (0..NOISE_PROFILE_BAND_COUNT)
        .map(au6_c_analytic_mean_band_tenth_db)
        .collect::<Vec<_>>();
    assert_eq!(
        bound,
        AU6_C_ANALYTIC_MEAN_BAND_TENTH_DB.to_vec(),
        "probe-2's independent evaluation"
    );
    let excess = AU6_C_LEARNED_PROFILE_TENTH_DB
        .iter()
        .zip(&bound)
        .map(|(learned, bound)| learned - bound)
        .collect::<Vec<_>>();
    let worst = *excess.iter().max().unwrap();
    let worst_band = excess.iter().position(|value| *value == worst).unwrap();
    assert_eq!(
        (worst, worst_band),
        (
            AU6_C_LEAKAGE_WORST_EXCESS_TENTH_DB,
            AU6_C_LEAKAGE_WORST_BAND
        )
    );
    assert_eq!(
        *excess.iter().min().unwrap(),
        AU6_C_LEAKAGE_MIN_EXCESS_TENTH_DB
    );
    assert_eq!(
        excess
            .iter()
            .position(|value| *value == AU6_C_LEAKAGE_MIN_EXCESS_TENTH_DB),
        Some(1)
    );
    for (band, value) in excess.iter().enumerate() {
        assert!(
            *value <= AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB,
            "band {band}: excess {value} over the allowance"
        );
    }
    assert_eq!(AU6_BUDGETS[16].measured, i64::from(worst));
    println!(
        "AU6_PROFILE_BOUND worst_excess_tenth_db={worst} worst_band={worst_band} min_excess={} allowance={}",
        excess.iter().min().unwrap(),
        AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB
    );
    // The point-mass model is the wrong model: worst excess 225 in band 4.
    let point_mass = (0..NOISE_PROFILE_BAND_COUNT)
        .map(au6_c_point_mass_band_tenth_db_wrong_model)
        .collect::<Vec<_>>();
    assert_eq!(
        point_mass,
        AU6_C_POINT_MASS_BAND_TENTH_DB_WRONG_MODEL.to_vec()
    );
    let wrong_excess = AU6_C_LEARNED_PROFILE_TENTH_DB
        .iter()
        .zip(&point_mass)
        .map(|(learned, bound)| learned - bound)
        .collect::<Vec<_>>();
    let wrong_worst = *wrong_excess.iter().max().unwrap();
    assert_eq!(wrong_worst, AU6_C_POINT_MASS_WORST_EXCESS_TENTH_DB);
    assert_eq!(
        wrong_excess.iter().position(|value| *value == wrong_worst),
        Some(AU6_C_POINT_MASS_WORST_BAND)
    );
    assert!(
        wrong_worst > AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB,
        "the wrong model fails the allowance"
    );
    assert!(
        2 * wrong_worst >= 450,
        "the wrong model would need a 45 dB allowance"
    );
    // The noise-only bands are identical under both models; only the
    // hum-adjacent bands 3–10 move.
    // The 240 Hz and 300 Hz partials' main lobes leak into bands 11 and 12,
    // so the two models agree only from band 13 up.
    for band in 13..NOISE_PROFILE_BAND_COUNT {
        assert_eq!(bound[band], point_mass[band], "band {band}");
    }
    assert!((3..=12).any(|band| bound[band] != point_mass[band]));
    assert_eq!((bound[11], point_mass[11]), (-602, -598));
    // The shape claims on the pin.
    let pin = AU6_C_LEARNED_PROFILE_TENTH_DB;
    for band in AU6_C_PROFILE_HUM_BUMP_BANDS {
        assert!(
            pin[band] > pin[3] && pin[band] > pin[7],
            "band {band} sits above its flanks"
        );
    }
    for band in AU6_C_PROFILE_LOW_FALLOFF_BANDS {
        assert!(
            pin[band] + 30 < pin[4],
            "band {band} falls away from the 60 Hz bump"
        );
    }
    for band in AU6_C_PROFILE_MONOTONE_FROM_BAND + 1..NOISE_PROFILE_BAND_COUNT {
        assert!(
            pin[band] > pin[band - 1],
            "band {band}: the profile rises monotonically from band {AU6_C_PROFILE_MONOTONE_FROM_BAND}"
        );
    }
    assert!(
        pin[9] < pin[8],
        "§2.5's 'above band 8' is not what the pin shows: band 9 falls (erratum)"
    );
    assert!(pin[13] < pin[12] && pin[12] < pin[11]);
    assert_eq!(AU6_C_PROFILE_MONOTONE_FROM_BAND, 13);
    // The one-frame slip that breaks the pin.
    assert_eq!(
        (pin[0] - AU6_C_ONE_FRAME_OFFSET_BAND0_TENTH_DB).abs(),
        AU6_C_ONE_FRAME_OFFSET_MAX_DELTA_TENTH_DB
    );

    // Failing direction: a floor whose measurement equals its budget.
    let flat = Au6Budget {
        term: "flat floor",
        constant: "",
        budget: 400,
        measured: 400,
        kind: Au6BudgetKind::Floor,
    };
    assert!(
        std::panic::catch_unwind(|| check(&flat)).is_err(),
        "a 1x floor must fail"
    );
    let vacuous_ceiling = Au6Budget {
        term: "ceiling at zero",
        constant: "",
        budget: 25,
        measured: 0,
        kind: Au6BudgetKind::Ceiling,
    };
    assert!(
        std::panic::catch_unwind(|| check(&vacuous_ceiling)).is_err(),
        "a ceiling measured at zero belongs in MeasuredZero"
    );
}

// ===========================================================================
// §11.2.11 — key `review.questions`.
// ===========================================================================

/// One `?`, no `" and "`, and no leak needle in any entry; `a5a` and `a5b`
/// carry the same sentence.
///
/// *Fails:* the brief's original compound questions.
#[test]
fn au6_the_questions_are_one_clause_each() {
    /// `CC7_MACHINE_PROVENANCE_NEEDLES`, `kinewright-eval.rs:6348-6350`, reused verbatim.
    const PROVENANCE_NEEDLES: [&str; 7] = [
        "-sample-", "agent", "person", "model", "harness", "passed", "assert",
    ];
    fn single_clause(question: &str) -> bool {
        question.matches('?').count() == 1 && question.ends_with('?') && !question.contains(" and ")
    }

    assert_eq!(AU6_QUESTIONS.len(), 6);
    assert_eq!(AU6_QUESTIONS.map(|(id, _)| id), AU6_TASK_IDS);
    // Parameter-name and value needles derived from the canonical operations.
    let mut needles = PROVENANCE_NEEDLES
        .iter()
        .map(|needle| (*needle).to_owned())
        .collect::<BTreeSet<_>>();
    for scenario in AU6_SCENARIOS {
        for operation in au6_canonical_operations(scenario) {
            let rendered = format!("{operation:?}");
            match &operation {
                Operation::SetTrackMix { .. } => {
                    needles.extend(
                        ["gain_tenth_db", "pan_percent", "mute", "solo"].map(str::to_owned),
                    );
                }
                Operation::UpsertAudioBus { bus } => {
                    for effect in &bus.effects {
                        needles.insert(effect.name.clone());
                        needles.extend(effect.parameters.keys().cloned());
                    }
                }
                Operation::SetTrackAutomation { parameter, .. } => {
                    needles.insert(parameter.clone());
                }
                Operation::SetClipAudio { .. } => {
                    needles.extend(["fade_in_frames", "fade_out_frames"].map(str::to_owned));
                }
                _ => {}
            }
            // Every run of three or more digits in the payload.
            let mut digits = String::new();
            for character in rendered.chars().chain(std::iter::once(' ')) {
                if character.is_ascii_digit() {
                    digits.push(character);
                } else {
                    if digits.len() >= 3 {
                        needles.insert(digits.clone());
                    }
                    digits.clear();
                }
            }
        }
    }
    assert!(
        needles.len() > 40,
        "the needle set is not vacuous: {}",
        needles.len()
    );
    for (id, question) in AU6_QUESTIONS {
        assert!(single_clause(question), "{id}: {question:?}");
        for needle in &needles {
            assert!(
                !question.to_lowercase().contains(needle.as_str()),
                "{id}: {question:?} contains the needle {needle:?}"
            );
        }
        assert!(!question.chars().any(|character| character.is_ascii_digit()));
    }
    assert_eq!(
        AU6_QUESTIONS[4].1, AU6_QUESTIONS[5].1,
        "a5a and a5b ask the same question"
    );
    // The row's two words.
    assert!(AU6_QUESTIONS[0].1.contains("balanced"));
    assert!(AU6_QUESTIONS[2].1.contains("intelligible"));

    // Failing direction: the brief's compound questions (design-brief.md:89-93).
    let brief: [&str; 4] = [
        "Is the dialogue clearly above the bed, and does the bed move without pumping?",
        "Do the two voices sit at one comfortable level, and is every word intelligible?",
        "Is the programme audio continuous and at one level across every angle change?",
        "Are both deliveries at a comfortable level with no audible limiting?",
    ];
    assert!(!single_clause(brief[0]));
    assert!(!single_clause(brief[1]));
    assert!(!single_clause(brief[2]));
    assert!(
        single_clause(brief[3]),
        "the brief's (e) was one clause; it changed because it named a parameter"
    );
    assert!(brief[3].contains("limiting"));
    assert!(!AU6_QUESTIONS[4].1.contains("limiting"));
    assert!(!single_clause("Is it good? Is it loud?"));
}

// ===========================================================================
// §11.2.12 — key `pins`.
// ===========================================================================

/// The three §11.0.1 pins each carry a doc comment containing the words
/// "regression pin" and the run they were transcribed from.
///
/// *Fails:* a pin whose doc comment claims a derivation.
#[test]
fn au6_the_regression_pins_are_labelled_as_pins() {
    /// The contiguous `///` block immediately above `pub const NAME`.
    fn doc_comment_of(source: &str, name: &str) -> String {
        let lines = source.lines().collect::<Vec<_>>();
        let declaration = lines
            .iter()
            .position(|line| line.starts_with(&format!("pub const {name}:")))
            .unwrap_or_else(|| panic!("{name} is declared as a pub const"));
        let mut block = Vec::new();
        for line in lines[..declaration].iter().rev() {
            let trimmed = line.trim_start();
            if let Some(text) = trimmed.strip_prefix("///") {
                block.push(text.trim());
            } else {
                break;
            }
        }
        block.reverse();
        block.join(" ")
    }
    fn is_labelled_pin(doc: &str) -> bool {
        let lower = doc.to_lowercase();
        lower.contains("regression pin") && doc.contains("11a6098")
    }

    for pin in [
        "AU6_A_DUCK_KEYFRAMES",
        "AU6_C_LEARN_PROJECT_RANGE",
        "AU6_C_LEARNED_PROFILE_TENTH_DB",
        "AU6_A_DUCK_DETECTED_WINDOWS",
        "AU6_C_DETECTED_SILENCES",
    ] {
        let doc = doc_comment_of(AU6_SCENARIOS_SOURCE, pin);
        assert!(
            is_labelled_pin(&doc),
            "{pin} must be labelled a regression pin naming its run: {doc:?}"
        );
        assert!(
            !doc.to_lowercase().contains("derived analytically"),
            "{pin} must not claim a derivation"
        );
    }
    // The analytic bound is labelled the opposite way.
    let bound = doc_comment_of(AU6_SCENARIOS_SOURCE, "AU6_C_ANALYTIC_MEAN_BAND_TENTH_DB");
    assert!(!bound.to_lowercase().contains("regression pin"));
    assert!(bound.to_lowercase().contains("analytic"));
    // The three pins really are committed output in shape.
    assert_eq!(AU6_A_DUCK_KEYFRAMES.len(), 16);
    assert!(
        AU6_A_DUCK_KEYFRAMES
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0)
    );
    assert_eq!(
        AU6_C_LEARN_PROJECT_RANGE.start.0,
        AU6_C_DETECTED_SILENCES[3].0
    );
    assert_eq!(
        AU6_C_LEARNED_PROFILE_TENTH_DB.len(),
        NOISE_PROFILE_BAND_COUNT
    );
    assert!(
        AU6_C_LEARNED_PROFILE_TENTH_DB
            .iter()
            .all(|band| (-1_200..=0).contains(band))
    );

    // Failing direction: a doc comment that claims a derivation is not a pin.
    let derived = "/// Derived analytically from the authored turn table at 25 fps.\npub const AU6_FAKE: [i64; 1] = [0];\n";
    let doc = doc_comment_of(derived, "AU6_FAKE");
    assert!(!is_labelled_pin(&doc));
    let unlabelled_run =
        "/// REGRESSION PIN of the planner's output.\npub const AU6_FAKE: [i64; 1] = [0];\n";
    assert!(
        !is_labelled_pin(&doc_comment_of(unlabelled_run, "AU6_FAKE")),
        "a pin must name the run it was transcribed from"
    );
}
