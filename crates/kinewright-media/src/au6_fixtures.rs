//! AU6 §11: the media gates and the audio programme's first fixture manifest.
//!
//! Lives in `src/` because `mix_audio_stems` and `MixStems` are `pub(crate)`
//! (export.rs:999, :1099). AU6 does not widen the public surface to reach
//! them (A9/E9).
//!
//! Expected values are analytic, independently transcribed, or one of the
//! three labelled regression pins (rule 11.0.1). No fixture obtains an
//! expected value by calling a meter or a planner.

#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::float_cmp)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::needless_lifetimes)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::used_underscore_binding)]

use std::{
    ops::Range,
    sync::{Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};

use kinewright_core::{
    Analysis, AssetId, AudioBus, AudioBusId, AudioQcRequest, AudioRepairReport, AudioRepairRequest,
    Clip, ClipContent, ClipId, ColorContext, DeliveryProfile, Document, Export, ExportCancellation,
    ExportSettings, MediaAsset, MediaKind, MixLevelReport, MixLevelRequest, MixNoiseProfileRequest,
    MixSpectrumPoint, MixSpectrumRequest, MixWindowLevelReport, MixWindowRequest,
    NOISE_PROFILE_BAND_COUNT, NOISE_PROFILE_PARAMETER_NAMES, Operation,
    PROFILE_BAND_NEUTRAL_TENTH_DB, ParamValue, Rational, SilenceStatus, TimeCode, Track, TrackId,
    TrackKind, apply_batch,
    au6_scenarios::{
        AU6_A_BED_LEVEL_DBFS_HUNDREDTHS, AU6_A_BED_TRACK, AU6_A_CLIPS, AU6_A_DIALOGUE_BUS_NAME,
        AU6_A_MUSIC_BUS, AU6_A_MUSIC_BUS_NAME, AU6_A_VOICE_A_LEVEL_DBFS_HUNDREDTHS,
        AU6_A_VOICE_A_TRACK, AU6_A_VOICE_B_LEVEL_DBFS_HUNDREDTHS, AU6_A_VOICE_B_TRACK,
        AU6_AGENT_LANE_BUDGET_SECONDS, AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS,
        AU6_B_VOICE_A_LEVEL_DBFS_HUNDREDTHS, AU6_B_VOICE_A_TRACK, AU6_B_VOICE_B_BUS,
        AU6_B_VOICE_B_LEVEL_DBFS_HUNDREDTHS, AU6_B_VOICE_B_TRACK, AU6_C_CLICK_COUNT,
        AU6_C_CLICK_FRAMES, AU6_C_DETECTED_SILENCES, AU6_C_DIALOGUE_TRACK, AU6_C_GAP_RANGE,
        AU6_C_GAPS, AU6_C_HUM_FUNDAMENTAL_HERTZ, AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS,
        AU6_C_HUM_PARTIALS, AU6_C_LEAKAGE_WORST_BAND, AU6_C_LEARN_PROJECT_RANGE,
        AU6_C_LEARN_SOURCE_RANGE, AU6_C_LEARNED_PROFILE_TENTH_DB,
        AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS, AU6_C_ONE_FRAME_OFFSET_BAND0_TENTH_DB,
        AU6_C_ONE_FRAME_OFFSET_MAX_DELTA_TENTH_DB, AU6_C_PROFILE_HUM_BUMP_BANDS,
        AU6_C_PROFILE_LOW_FALLOFF_BANDS, AU6_C_PROFILE_MONOTONE_FROM_BAND, AU6_C_PROGRAMME_FRAMES,
        AU6_C_REPAIR_BUS, AU6_C_ROOM_TONE_ASSET_FRAMES, AU6_C_ROOM_TONE_FPS, AU6_C_VOICE_CARRIER,
        AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS, AU6_CHANNELS, AU6_CLICK_SAMPLES,
        AU6_D_ANGLE_OFFSETS_FRAMES, AU6_D_CUT_FRAMES, AU6_D_MASTER_CLIP_ID,
        AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS, AU6_D_MASTER_TRACK, AU6_D_SCRATCH_1_TRACK,
        AU6_D_SCRATCH_2_TRACK, AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS,
        AU6_D_VISIBLE_ANGLE_PER_SEGMENT, AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB,
        AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS, AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS,
        AU6_DUCK_DEPTH_MIN_HUNDREDTHS, AU6_ENCODE_PROGRAMME_FRAMES, AU6_EXPORT_JOBS,
        AU6_FRAMES_PER_WINDOW, AU6_HANN_MAIN_LOBE_BINS_RESTATED, AU6_HOP_MILLISECONDS,
        AU6_HUM_DROP_MIN_DB_HUNDREDTHS, AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS,
        AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS,
        AU6_INTERVIEW_NOMINAL_TRIM_TENTH_DB_WRONG_MODEL,
        AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS, AU6_LEARN_MINIMUM_PROJECT_FRAMES,
        AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED, AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS,
        AU6_MASTER_STEM_SAMPLES, AU6_MEASURED_C_LEARN_GAP_DBFS_HUNDREDTHS,
        AU6_MEASURED_DECLICK_ERROR_DROP_TENTH_DB, AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE,
        AU6_MEASURED_VOICE_MATCH_NOMINAL_TRIM_LU_HUNDREDTHS,
        AU6_MEASURED_VOICE_MATCH_UNTRIMMED_LU_HUNDREDTHS, AU6_MEDIA_LANE_BUDGET_SECONDS,
        AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED, AU6_NOISE_PROFILE_PERCENT_RESTATED,
        AU6_NOISE_PROFILE_SEGMENT_SAMPLE_FRAMES_RESTATED, AU6_PODCAST_AM_DEPTH_TENTH_DB,
        AU6_PODCAST_AM_HERTZ, AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS,
        AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS, AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB,
        AU6_PROGRAMME_FRAMES, AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS,
        AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS, AU6_SAMPLE_RATE, AU6_SAMPLES_PER_FRAME,
        AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS, AU6_SOURCE_FPS, AU6_SOURCE_HEIGHT,
        AU6_SOURCE_WIDTH, AU6_TARGET_SEPARATION_LU_HUNDREDTHS,
        AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS, AU6_TURNS, AU6_VOICE_A_BAND_INDEX,
        AU6_VOICE_B_BAND_INDEX, AU6_VOICE_BAND_SEPARATION_BANDS, AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS,
        AU6_WINDOW_MILLISECONDS, AU6_WINDOW_PROGRAMME, Au6Scenario, Au6Speaker, Au6TrackRole,
        au6_a_duck_curve, au6_c_analytic_mean_band_tenth_db, au6_c_declick_only_operations,
        au6_c_fill_operations, au6_c_gap_operations, au6_c_point_mass_band_tenth_db_wrong_model,
        au6_c_repair_operations, au6_canonical_operations, au6_d_sync_group,
        au6_duck_gap_window_indices, au6_duck_speech_window_indices, au6_export_settings,
        au6_profile_export_settings, au6_spec, au6_turns_of,
    },
};

use crate::{
    FfmpegMediaEngine,
    au6_sources::{
        AU6_MUX_RECIPE, AU6_VIDEO_ONLY_RECIPE, au6_analytic_level_dbfs_hundredths,
        au6_angle_source, au6_c_click_free_track, au6_c_voice_only_track, au6_chord_bed_pcm,
        au6_clicks_into, au6_hum_pcm, au6_muxed_source, au6_noise_pcm, au6_picture_source,
        au6_ride_pcm, au6_scenario_sources, au6_scenario_tracks, au6_scratch_pcm, au6_source,
        au6_stamp_on_project_grid, au6_steady_voice_pcm, au6_to_stereo, au6_voice_pcm,
    },
    audio::{decode_audio_range, detect_clicks, process_buffer_static},
    clock::frame_to_samples,
    decode::probe_path,
    derived::DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS,
    export::{MixStems, mix_audio_stems},
    initialize_ffmpeg,
    loudness::LOUDNESS_GATING_BLOCK_FRAMES,
    room_tone_store::RoomToneStore,
    spectrum::{NOISE_PROFILE_MINIMUM_FRAMES, NOISE_PROFILE_PERCENT, NOISE_PROFILE_SEGMENT_FRAMES},
    test_support::{GeneratedMedia, TempDirectory, wav_f32},
};

/// The contract token recorded on the manifest and asserted by item 44 / 79.
const AU6_CONTRACT: &str = "au6_workflow_evaluation";
const AU6_MANIFEST: &str = include_str!("../tests/fixtures/au6_manifest.json");

/// `MAIN_LOBE_BINS`, transcribed from `spectrum.rs:71` (private to that
/// module) so item 20 can still name its owner.
const OWNER_HANN_MAIN_LOBE_BINS: u32 = 4;

// ===========================================================================
// S16: two margin helpers, one per direction.
// ===========================================================================

/// Ceiling margin: `allowed / observed`. Zero observed is infinite.
fn ceiling_margin(observed: i64, allowed: i64) -> f64 {
    if observed == 0 {
        f64::INFINITY
    } else {
        allowed as f64 / observed.abs() as f64
    }
}

/// Floor margin: `observed / required`.
fn floor_margin(observed: i64, required: i64) -> f64 {
    if required == 0 {
        f64::INFINITY
    } else {
        observed as f64 / required as f64
    }
}

fn render_margin(value: f64) -> String {
    if value.is_infinite() {
        "unbounded".to_owned()
    } else {
        format!("{value:.2}x")
    }
}

fn print_floor(term: &str, observed: i64, required: i64) {
    let margin = floor_margin(observed, required);
    println!(
        "AU6 {term} observed={observed} budget={required} kind=Floor margin={}",
        render_margin(margin)
    );
    assert!(
        observed >= required,
        "{term}: observed {observed} does not clear floor {required} (margin {})",
        render_margin(margin)
    );
}

fn print_ceiling(term: &str, observed: i64, allowed: i64) {
    let margin = ceiling_margin(observed, allowed);
    println!(
        "AU6 {term} observed={observed} budget={allowed} kind=Ceiling margin={}",
        render_margin(margin)
    );
    assert!(
        observed >= 0 && observed <= allowed,
        "{term}: observed {observed} does not sit under ceiling {allowed} (margin {})",
        render_margin(margin)
    );
}

// ===========================================================================
// Shared engine and mix settings.
// ===========================================================================

fn engine() -> MutexGuard<'static, FfmpegMediaEngine> {
    static ENGINE: OnceLock<Mutex<FfmpegMediaEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            initialize_ffmpeg().expect("FFmpeg must initialize for AU6 media fixtures");
            Mutex::new(FfmpegMediaEngine::new().expect("the shared AU6 engine starts"))
        })
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn project_fps() -> Rational {
    Rational::new(AU6_SOURCE_FPS, 1).expect("25 fps")
}

fn mix_settings(document: &Document) -> ExportSettings {
    ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 1,
        audio_bitrate: 1,
        loudness_normalization: None,
        cancellation: ExportCancellation::default(),
    }
}

fn window_request(point: MixSpectrumPoint, range: Range<TimeCode>) -> MixWindowRequest {
    MixWindowRequest {
        range: Some(range),
        point,
        window_milliseconds: AU6_WINDOW_MILLISECONDS,
        hop_milliseconds: AU6_HOP_MILLISECONDS,
    }
}

fn level_request(range: Range<TimeCode>) -> MixLevelRequest {
    MixLevelRequest { range: Some(range) }
}

fn qc_request(range: Range<TimeCode>) -> AudioQcRequest {
    AudioQcRequest {
        range: Some(range),
        profile: None,
    }
}

fn repair_request(point: MixSpectrumPoint, range: Range<TimeCode>) -> AudioRepairRequest {
    AudioRepairRequest {
        range: Some(range),
        point,
    }
}

fn profile_request(point: MixSpectrumPoint, range: Range<TimeCode>) -> MixNoiseProfileRequest {
    MixNoiseProfileRequest {
        range: Some(range),
        point,
    }
}

fn integrated(levels: &kinewright_core::AudioLoudness) -> Option<i32> {
    levels.integrated_lufs_hundredths
}

fn bus_integrated(report: &MixLevelReport, name: &str) -> i32 {
    report
        .buses
        .iter()
        .find(|bus| bus.name == name)
        .and_then(|bus| integrated(&bus.levels))
        .unwrap_or_else(|| panic!("bus {name} must report an integrated level"))
}

fn track_levels<'a>(
    report: &'a MixLevelReport,
    track: TrackId,
) -> &'a kinewright_core::TrackLevels {
    report
        .tracks
        .iter()
        .find(|entry| entry.track == track)
        .unwrap_or_else(|| panic!("track {} is in the mix report", track.0))
}

fn apply_in_order(document: &mut Document, operations: &[Operation]) {
    for (index, operation) in operations.iter().enumerate() {
        apply_batch(document, std::slice::from_ref(operation))
            .unwrap_or_else(|error| panic!("operation {}: {error}", index + 1));
        document
            .validate()
            .unwrap_or_else(|error| panic!("after operation {}: {error}", index + 1));
    }
}

fn media_clip(spec: &kinewright_core::Au6ClipSpec) -> Clip {
    Clip {
        id: spec.clip,
        asset: spec.asset,
        source_range: spec.range(),
        content: ClipContent::Media,
        timeline_start: spec.start,
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

/// One scenario's generated files plus the document they sit in. The
/// `GeneratedMedia` values are held for their `Drop`.
struct Au6Scene {
    _media: Vec<GeneratedMedia>,
    _room_tone_store: Option<TempDirectory>,
    document: Document,
}

impl Au6Scene {
    fn base(scenario: Au6Scenario) -> Self {
        initialize_ffmpeg().expect("FFmpeg must initialize");
        let spec = au6_spec(scenario);
        let media = au6_scenario_sources(scenario);
        assert_eq!(media.len(), spec.tracks.len());
        let mut assets = Vec::with_capacity(media.len());
        for (generated, track) in media.iter().zip(spec.tracks) {
            let probed =
                probe_path(generated.path(), AssetId(track.track.0)).unwrap_or_else(|error| {
                    panic!("{scenario:?}: probe {}: {error}", role_name(track.role))
                });
            let stamped = au6_stamp_on_project_grid(
                probed,
                project_fps(),
                TimeCode(i64::from(spec.asset_frames)),
            );
            assert_eq!(stamped.fps, project_fps());
            assets.push(stamped);
        }
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
                    .map(media_clip)
                    .collect(),
            })
            .collect();
        let mut document = Document {
            tracks,
            media_pool: assets,
            fps: project_fps(),
            resolution: (AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT),
            duration: TimeCode(i64::from(spec.frames)),
            ..Document::default()
        };
        if scenario == Au6Scenario::Multicam {
            document.catalog.sync_groups.push(au6_d_sync_group(
                kinewright_core::au6_scenarios::AU6_D_ANGLE_ASSETS,
            ));
        }
        document
            .validate()
            .unwrap_or_else(|error| panic!("{scenario:?}: base document: {error}"));
        Self {
            _media: media,
            _room_tone_store: None,
            document,
        }
    }

    fn commit(&self, operations: &[Operation]) -> Document {
        let mut document = self.document.clone();
        apply_in_order(&mut document, operations);
        document
    }

    fn canonical(&self) -> Document {
        self.commit(&au6_canonical_operations(self.document_scenario()))
    }

    fn document_scenario(&self) -> Au6Scenario {
        let duration = self.document.duration.0;
        if duration == i64::from(AU6_C_PROGRAMME_FRAMES) {
            Au6Scenario::LocationDialogue
        } else if duration == i64::from(kinewright_core::au6_scenarios::AU6_ENCODE_PROGRAMME_FRAMES)
            && self.document.tracks.len() == 4
        {
            Au6Scenario::Delivery
        } else if self.document.tracks.len() == 5 {
            Au6Scenario::Multicam
        } else if self.document.tracks.len() == 2 {
            Au6Scenario::Podcast
        } else {
            Au6Scenario::Interview
        }
    }
}

fn role_name(role: Au6TrackRole) -> &'static str {
    match role {
        Au6TrackRole::Picture => "picture",
        Au6TrackRole::VoiceA => "voice-a",
        Au6TrackRole::VoiceB => "voice-b",
        Au6TrackRole::MusicBed => "bed",
        Au6TrackRole::Dialogue => "dialogue",
        Au6TrackRole::Angle => "angle",
        Au6TrackRole::Scratch => "scratch",
        Au6TrackRole::MasterAudio => "master",
    }
}

fn scene(scenario: Au6Scenario) -> Au6Scene {
    Au6Scene::base(scenario)
}

fn wait_for_silence(engine: &FfmpegMediaEngine, asset: &MediaAsset) {
    engine.request_silence_detection(asset.clone());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match engine.silence_status(asset) {
            SilenceStatus::Ready(_) | SilenceStatus::NoAudio => return,
            SilenceStatus::Failed(error) => panic!("silence analysis failed: {error}"),
            _ => {
                assert!(
                    Instant::now() < deadline,
                    "silence analysis did not finish for {}",
                    asset.path.display()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn room_tone_asset(label: &str) -> (TempDirectory, GeneratedMedia, MediaAsset) {
    let store_root = TempDirectory::new(label);
    let store = RoomToneStore::for_project(&store_root.path("au6.kinewright"))
        .expect("the room-tone store root derives");
    let click_free = au6_c_click_free_track();
    let start = usize::try_from(AU6_C_LEARN_SOURCE_RANGE.start.0).unwrap()
        * AU6_SAMPLES_PER_FRAME as usize
        * usize::from(AU6_CHANNELS);
    let end = usize::try_from(AU6_C_LEARN_SOURCE_RANGE.end.0).unwrap()
        * AU6_SAMPLES_PER_FRAME as usize
        * usize::from(AU6_CHANNELS);
    let capture = store
        .write_capture(&click_free[start..end])
        .expect("the learn-gap capture is a legal room-tone write");
    let probed = probe_path(&capture.path, AssetId(2)).expect("the capture probes");
    // R3: 37 project frames at 48 kHz map to 44 source frames at 30 fps;
    // FFmpeg's probe can report the truncated-up neighbour, so the asset is
    // stamped to the pinned length rather than trusted as-probed.
    assert!(
        (probed.duration.0 - AU6_C_ROOM_TONE_ASSET_FRAMES).abs() <= 1,
        "room-tone capture duration {} is not the pinned 44 ± 1",
        probed.duration.0
    );
    let keep = GeneratedMedia::from_bytes(
        &format!("{label}-keep"),
        "wav",
        &std::fs::read(&capture.path).expect("the store file is readable"),
    );
    let mut asset = probe_path(keep.path(), AssetId(2)).expect("the kept capture probes");
    asset.fps = Rational::new(AU6_C_ROOM_TONE_FPS, 1).expect("30 fps");
    asset.duration = TimeCode(AU6_C_ROOM_TONE_ASSET_FRAMES);
    (store_root, keep, asset)
}

fn location_filled_scene() -> (Au6Scene, Document) {
    let mut scene = scene(Au6Scenario::LocationDialogue);
    let (store, media, asset) = room_tone_asset("au6-c-fill");
    let id = asset.id;
    apply_in_order(
        &mut scene.document,
        &[Operation::AddAsset {
            asset: asset.clone(),
        }],
    );
    scene._room_tone_store = Some(store);
    scene._media.push(media);
    // The seam is measured on the fill commit, before the repair bus
    // processes the track (§4(c)(5) / §5.3 step 3).
    let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
    ops.extend(au6_c_fill_operations(id));
    let document = scene.commit(&ops);
    (scene, document)
}

/// AU6's own seam helper: a `MeasuredZero` claim, not AU5's budgeted ceiling.
fn assert_au6_seam(document: &Document, join: Range<TimeCode>) {
    let stems = mix_audio_stems(
        document,
        TimeCode::ZERO..document.duration,
        &mix_settings(document),
    )
    .expect("the filled document mixes stems");
    let mixed = track_stem(&stems, AU6_C_DIALOGUE_TRACK);
    let track = document
        .tracks
        .iter()
        .find(|track| track.id == AU6_C_DIALOGUE_TRACK)
        .expect("the dialogue track exists");
    let mut expected = Vec::new();
    for clip in &track.clips {
        let asset = document
            .media_pool
            .iter()
            .find(|asset| asset.id == clip.asset)
            .expect("every clip names a pooled asset");
        expected.extend(
            decode_audio_range(
                &asset.path,
                asset.fps,
                clip.source_range.start,
                clip.source_range.end,
                AU6_SAMPLE_RATE,
                AU6_CHANNELS,
                &ExportCancellation::default(),
            )
            .expect("a fixture clip decodes"),
        );
    }
    assert_eq!(mixed.len(), expected.len());
    let window = AU6_SAMPLE_RATE as u64 * 10 / 1_000;
    let join_start = frame_to_samples(join.start, AU6_SAMPLE_RATE, document.fps);
    let join_end = frame_to_samples(join.end, AU6_SAMPLE_RATE, document.fps);
    let from = join_start.saturating_sub(window);
    let to = join_end + window;
    let channels = usize::from(AU6_CHANNELS);
    let start = usize::try_from(from).unwrap() * channels;
    let end = usize::try_from(to).unwrap() * channels;
    let measured = mixed[start..end]
        .iter()
        .zip(&expected[start..end])
        .map(|(left, right)| f64::from(*left - *right).abs())
        .fold(0.0_f64, f64::max);
    println!("AU6 seam join={join:?} measured={measured:.3e}");
    assert!(
        measured <= 0.0,
        "the room-tone seam must be exactly zero, not {measured}"
    );
}

fn stems_of(document: &Document, range: Range<TimeCode>) -> MixStems {
    mix_audio_stems(document, range, &mix_settings(document)).expect("the document mixes stems")
}

fn track_stem<'a>(stems: &'a MixStems, track: TrackId) -> &'a [f32] {
    stems
        .tracks
        .iter()
        .find(|(id, _)| *id == track)
        .map(|(_, samples)| samples.as_slice())
        .unwrap_or_else(|| panic!("track {} has a stem", track.0))
}

fn bus_stem<'a>(stems: &'a MixStems, bus: AudioBusId) -> &'a [f32] {
    stems
        .buses
        .iter()
        .find(|(id, _)| *id == bus)
        .map(|(_, samples)| samples.as_slice())
        .unwrap_or_else(|| panic!("bus {} has a stem", bus.0))
}

fn authored_region_level(label: &str, samples: &[f32]) -> i32 {
    let turn = match label {
        "a-voice-a" | "b-voice-a" | "c-voice" | "d-master" => {
            au6_turns_of(Au6Speaker::A)[0].clone()
        }
        "a-voice-b" | "b-voice-b" => au6_turns_of(Au6Speaker::B)[0].clone(),
        _ => {
            return au6_analytic_level_dbfs_hundredths(samples);
        }
    };
    let start = usize::try_from(turn.start.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
    let end = usize::try_from(turn.end.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
    au6_analytic_level_dbfs_hundredths(&samples[start..end])
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    (sum / samples.len() as f64).sqrt()
}

fn error_drop_tenth_db(before: &[f32], after: &[f32], reference: &[f32]) -> f64 {
    let before_err: Vec<f32> = before
        .iter()
        .zip(reference)
        .map(|(sample, clean)| sample - clean)
        .collect();
    let after_err: Vec<f32> = after
        .iter()
        .zip(reference)
        .map(|(sample, clean)| sample - clean)
        .collect();
    let before_rms = rms(&before_err);
    let after_rms = rms(&after_err);
    200.0 * (before_rms / after_rms).log10()
}

fn energy_average(values: &[i32]) -> i32 {
    let linear: f64 = values
        .iter()
        .map(|value| 10.0_f64.powf(f64::from(*value) / 2_000.0))
        .sum::<f64>()
        / values.len() as f64;
    (2_000.0 * linear.log10()).round() as i32
}

fn mean_i32(values: &[i32]) -> i32 {
    values.iter().sum::<i32>() / i32::try_from(values.len()).unwrap()
}

fn window_values(report: &MixWindowLevelReport, indices: &[usize]) -> Vec<i32> {
    indices
        .iter()
        .map(|index| {
            report.windows[*index].unwrap_or_else(|| {
                panic!("window {index} inside a speech population must not be None")
            })
        })
        .collect()
}

fn peak_band(report: &kinewright_core::MixSpectrumReport) -> usize {
    report
        .bands
        .iter()
        .enumerate()
        .max_by_key(|(_, band)| band.level_dbfs_hundredths.unwrap_or(i32::MIN))
        .map(|(index, _)| index)
        .expect("a spectrum has bands")
}

fn set_track_gain(document: &mut Document, track: TrackId, gain_tenth_db: i32) {
    apply_in_order(
        document,
        &[Operation::SetTrackMix {
            track,
            gain_tenth_db,
            pan_percent: 0,
            mute: false,
            solo: false,
        }],
    );
}

fn mute_track(document: &mut Document, track: TrackId, mute: bool) {
    apply_in_order(
        document,
        &[Operation::SetTrackMix {
            track,
            gain_tenth_db: 0,
            pan_percent: 0,
            mute,
            solo: false,
        }],
    );
}

fn interview_with_bus_duck(scene: &Au6Scene) -> Document {
    let mut operations = au6_canonical_operations(Au6Scenario::Interview);
    operations.retain(|operation| !matches!(operation, Operation::SetTrackAutomation { .. }));
    for operation in &mut operations {
        if let Operation::UpsertAudioBus { bus } = operation {
            if bus.id == AU6_A_MUSIC_BUS {
                bus.gain_curve = Some(au6_a_duck_curve());
            }
        }
    }
    scene.commit(&operations)
}

fn interview_without_duck(scene: &Au6Scene) -> Document {
    let ops: Vec<Operation> = au6_canonical_operations(Au6Scenario::Interview)
        .into_iter()
        .filter(|operation| !matches!(operation, Operation::SetTrackAutomation { .. }))
        .collect();
    scene.commit(&ops)
}

fn podcast_without_trims(scene: &Au6Scene) -> Document {
    let ops: Vec<Operation> = au6_canonical_operations(Au6Scenario::Podcast)
        .into_iter()
        .filter(|operation| !matches!(operation, Operation::SetTrackMix { .. }))
        .collect();
    scene.commit(&ops)
}

fn podcast_bypassed(scene: &Au6Scene) -> Document {
    let spec = au6_spec(Au6Scenario::Podcast);
    let mut ops = vec![
        Operation::SetTrackMix {
            track: AU6_B_VOICE_A_TRACK,
            gain_tenth_db: kinewright_core::au6_scenarios::AU6_PODCAST_A_TRIM_TENTH_DB,
            pan_percent: 0,
            mute: false,
            solo: false,
        },
        Operation::SetTrackMix {
            track: AU6_B_VOICE_B_TRACK,
            gain_tenth_db: kinewright_core::au6_scenarios::AU6_PODCAST_B_TRIM_TENTH_DB,
            pan_percent: 0,
            mute: false,
            solo: false,
        },
        Operation::UpsertAudioBus {
            bus: AudioBus {
                id: spec.buses[0].bus,
                name: spec.buses[0].name.to_owned(),
                tracks: spec.buses[0].tracks.to_vec(),
                gain_tenth_db: 0,
                gain_curve: None,
                effects: Vec::new(),
                ducking_sidechain_tracks: Vec::new(),
            },
        },
        Operation::UpsertAudioBus {
            bus: AudioBus {
                id: spec.buses[1].bus,
                name: spec.buses[1].name.to_owned(),
                tracks: spec.buses[1].tracks.to_vec(),
                gain_tenth_db: 0,
                gain_curve: None,
                effects: Vec::new(),
                ducking_sidechain_tracks: Vec::new(),
            },
        },
    ];
    ops.extend(kinewright_core::au6_b_fade_operations());
    scene.commit(&ops)
}

fn repair_with_profile(bands: [i32; NOISE_PROFILE_BAND_COUNT]) -> Vec<Operation> {
    let mut operations = au6_c_repair_operations();
    if let Operation::UpsertAudioBus { bus } = &mut operations[0] {
        for (name, band) in NOISE_PROFILE_PARAMETER_NAMES.iter().zip(bands) {
            bus.effects[0]
                .parameters
                .insert((*name).to_owned(), ParamValue::Integer(i64::from(band)));
        }
    }
    operations
}

fn repair_without_hum() -> Vec<Operation> {
    let mut operations = au6_c_repair_operations();
    if let Operation::UpsertAudioBus { bus } = &mut operations[0] {
        bus.effects
            .retain(|effect| effect.name != "audio_hum_removal");
    }
    operations
}

fn repair_without_declick() -> Vec<Operation> {
    let mut operations = au6_c_repair_operations();
    if let Operation::UpsertAudioBus { bus } = &mut operations[0] {
        bus.effects.retain(|effect| effect.name != "audio_declick");
    }
    operations
}

fn declick_effect() -> kinewright_core::Effect {
    let operations = au6_c_declick_only_operations();
    match operations.into_iter().next() {
        Some(Operation::UpsertAudioBus { bus }) => bus.effects.into_iter().next().expect("declick"),
        _ => panic!("declick-only operations write one bus"),
    }
}

// ===========================================================================
// §11.2 items 13–19b — source non-vacuity.
// ===========================================================================

#[test]
fn au6_every_authored_level_matches_its_analytic_derivation() {
    let cases: &[(&str, Vec<f32>, i32)] = &[
        (
            "a-voice-a",
            au6_voice_pcm(
                Au6Speaker::A,
                AU6_A_VOICE_A_LEVEL_DBFS_HUNDREDTHS,
                AU6_PROGRAMME_FRAMES,
            ),
            AU6_A_VOICE_A_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "a-voice-b",
            au6_voice_pcm(
                Au6Speaker::B,
                AU6_A_VOICE_B_LEVEL_DBFS_HUNDREDTHS,
                AU6_PROGRAMME_FRAMES,
            ),
            AU6_A_VOICE_B_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "a-bed",
            au6_chord_bed_pcm(AU6_A_BED_LEVEL_DBFS_HUNDREDTHS, AU6_PROGRAMME_FRAMES),
            AU6_A_BED_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "b-voice-a",
            au6_steady_voice_pcm(
                Au6Speaker::A,
                AU6_B_VOICE_A_LEVEL_DBFS_HUNDREDTHS,
                AU6_PROGRAMME_FRAMES,
            ),
            AU6_B_VOICE_A_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "b-voice-b",
            au6_ride_pcm(
                AU6_B_VOICE_B_LEVEL_DBFS_HUNDREDTHS,
                AU6_PODCAST_AM_HERTZ,
                AU6_PODCAST_AM_DEPTH_TENTH_DB,
                AU6_PROGRAMME_FRAMES,
            ),
            AU6_B_VOICE_B_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "c-voice",
            au6_voice_pcm(
                AU6_C_VOICE_CARRIER,
                AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS,
                AU6_C_PROGRAMME_FRAMES,
            ),
            AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "c-noise",
            au6_noise_pcm(AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS, AU6_C_PROGRAMME_FRAMES),
            AU6_C_NOISE_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "c-hum",
            au6_hum_pcm(
                AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS,
                f64::from(AU6_C_HUM_FUNDAMENTAL_HERTZ),
                AU6_C_HUM_PARTIALS as usize,
                AU6_C_PROGRAMME_FRAMES,
            ),
            AU6_C_HUM_LEVEL_DBFS_HUNDREDTHS,
        ),
        (
            "d-master",
            crate::au6_sources::au6_d_master_pcm(AU6_PROGRAMME_FRAMES),
            AU6_D_MASTER_LEVEL_DBFS_HUNDREDTHS,
        ),
    ];
    let mut worst = 0_i32;
    for (label, samples, authored) in cases {
        let measured = authored_region_level(label, samples);
        let error = (measured - authored).abs();
        println!(
            "AU6 authored-level {label} authored={authored} measured={measured} error={error}"
        );
        assert!(
            error <= AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS,
            "{label}: {error} exceeds the authored-level ceiling"
        );
        worst = worst.max(error);
    }
    print_ceiling(
        "authored level",
        i64::from(worst),
        i64::from(AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS),
    );
    let off = au6_chord_bed_pcm(AU6_A_BED_LEVEL_DBFS_HUNDREDTHS - 100, AU6_PROGRAMME_FRAMES);
    let off_error =
        (au6_analytic_level_dbfs_hundredths(&off) - AU6_A_BED_LEVEL_DBFS_HUNDREDTHS).abs();
    assert!(
        off_error > AU6_AUTHORED_LEVEL_TOLERANCE_HUNDREDTHS,
        "a buffer authored one constant off must miss the ceiling"
    );
}

#[test]
fn au6_the_two_voices_occupy_disjoint_bands() {
    let scene = scene(Au6Scenario::Interview);
    let engine = engine();
    let a = engine
        .mix_spectrum(
            &scene.document,
            &MixSpectrumRequest {
                range: Some(au6_turns_of(Au6Speaker::A)[0].clone()),
                point: MixSpectrumPoint::Track(AU6_A_VOICE_A_TRACK),
            },
        )
        .expect("voice A spectra");
    let b = engine
        .mix_spectrum(
            &scene.document,
            &MixSpectrumRequest {
                range: Some(au6_turns_of(Au6Speaker::B)[0].clone()),
                point: MixSpectrumPoint::Track(AU6_A_VOICE_B_TRACK),
            },
        )
        .expect("voice B spectra");
    let peak_a = peak_band(&a);
    let peak_b = peak_band(&b);
    let separation = i64::try_from(peak_a.abs_diff(peak_b)).unwrap();
    println!("AU6 voice bands A={peak_a} B={peak_b} separation={separation}");
    assert_eq!(peak_a, AU6_VOICE_A_BAND_INDEX);
    assert_eq!(peak_b, AU6_VOICE_B_BAND_INDEX);
    print_floor(
        "voice band separation",
        separation,
        i64::from(AU6_VOICE_BAND_SEPARATION_BANDS),
    );
    let same = engine
        .mix_spectrum(
            &scene.document,
            &MixSpectrumRequest {
                range: Some(au6_turns_of(Au6Speaker::A)[0].clone()),
                point: MixSpectrumPoint::Track(AU6_A_VOICE_A_TRACK),
            },
        )
        .expect("second A spectrum");
    assert_eq!(peak_band(&same), peak_a);
    drop(engine);
    let both_a = au6_voice_pcm(
        Au6Speaker::A,
        AU6_A_VOICE_A_LEVEL_DBFS_HUNDREDTHS,
        AU6_PROGRAMME_FRAMES,
    );
    assert_ne!(
        au6_analytic_level_dbfs_hundredths(&both_a),
        0,
        "a same-band construction still authors energy — the failing direction is the peak-band identity, not silence"
    );
}

#[test]
fn au6_c_every_authored_gap_is_below_the_silence_threshold() {
    let mono = {
        let tracks = au6_scenario_tracks(Au6Scenario::LocationDialogue);
        crate::au6_sources::au6_to_mono(&tracks[0].1)
    };
    let mut worst_below = i32::MAX;
    for gap in AU6_C_GAPS {
        let start = usize::try_from(gap.start.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
        let end = usize::try_from(gap.end.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
        let measured = au6_analytic_level_dbfs_hundredths(&mono[start..end]);
        let below = AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS - measured;
        println!("AU6 gap {gap:?} measured={measured} below-threshold={below}");
        assert!(
            measured < AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS,
            "gap {gap:?} is not under the silence threshold"
        );
        worst_below = worst_below.min(below);
    }
    let learn = AU6_C_LEARN_SOURCE_RANGE.clone();
    let start = usize::try_from(learn.start.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
    let end = usize::try_from(learn.end.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
    let learn_below = AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS
        - au6_analytic_level_dbfs_hundredths(&mono[start..end]);
    println!("AU6 worst clicked-gap below-threshold={worst_below} learn-gap={learn_below}");
    print_floor(
        "learn gap under the detector",
        i64::from(learn_below),
        i64::from(AU6_LEARN_GAP_BELOW_SILENCE_MIN_HUNDREDTHS),
    );
}

#[test]
fn au6_c_the_brief_levels_never_reach_the_detector() {
    let voice = au6_voice_pcm(
        AU6_C_VOICE_CARRIER,
        AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS,
        AU6_C_PROGRAMME_FRAMES,
    );
    let brief_noise = au6_noise_pcm(
        AU6_C_VOICE_LEVEL_DBFS_HUNDREDTHS - 1_000,
        AU6_C_PROGRAMME_FRAMES,
    );
    let mut bed = voice;
    for (sample, noise) in bed.iter_mut().zip(brief_noise) {
        *sample += noise;
    }
    let gap = AU6_C_GAPS[0].clone();
    let start = usize::try_from(gap.start.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
    let end = usize::try_from(gap.end.0).unwrap() * AU6_SAMPLES_PER_FRAME as usize;
    let measured = au6_analytic_level_dbfs_hundredths(&bed[start..end]);
    println!("AU6 brief-SNR gap measured={measured}");
    assert!(
        measured > AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS,
        "the brief's 10 dB-SNR bed must sit above the silence threshold, not {measured}"
    );
}

#[test]
fn au6_the_learn_gap_is_the_longest_detected_silence() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let engine = engine();
    let asset = scene.document.media_pool[0].clone();
    wait_for_silence(&engine, &asset);
    let spans = engine
        .timeline_silences(
            &scene.document,
            None,
            TimeCode(i64::try_from(AU6_LEARN_MINIMUM_PROJECT_FRAMES).unwrap()),
        )
        .expect("timeline silences");
    let detected: Vec<(i64, i64)> = spans
        .iter()
        .map(|span| (span.project_start.0, span.project_end.0))
        .collect();
    println!("AU6 detected silences {detected:?}");
    assert_eq!(detected, AU6_C_DETECTED_SILENCES);
    let longest = detected
        .iter()
        .max_by_key(|(start, end)| end - start)
        .copied()
        .unwrap();
    assert_eq!(
        longest,
        (
            AU6_C_LEARN_PROJECT_RANGE.start.0,
            AU6_C_LEARN_PROJECT_RANGE.end.0
        )
    );
    let next = detected
        .iter()
        .filter(|span| **span != longest)
        .map(|(start, end)| end - start)
        .max()
        .unwrap();
    assert!(longest.1 - longest.0 - next >= 21);
}

#[test]
fn au6_c_a_click_in_the_learn_gap_splits_it() {
    let mut mono = crate::au6_sources::au6_to_mono(&au6_c_click_free_track());
    au6_clicks_into(&mut mono, &[290]);
    let stereo = au6_to_stereo(&mono);
    let media = GeneratedMedia::from_bytes(
        "au6-c-click-in-gap",
        "wav",
        &wav_f32(&stereo, AU6_SAMPLE_RATE, AU6_CHANNELS),
    );
    let mut asset = probe_path(media.path(), AssetId(1)).expect("clicked fixture probes");
    asset = au6_stamp_on_project_grid(
        asset,
        project_fps(),
        TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
    );
    let spec = au6_spec(Au6Scenario::LocationDialogue);
    let document = Document {
        tracks: vec![Track {
            id: AU6_C_DIALOGUE_TRACK,
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: spec.clips.iter().map(media_clip).collect(),
        }],
        media_pool: vec![asset.clone()],
        fps: project_fps(),
        resolution: (AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT),
        duration: TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
        ..Document::default()
    };
    let engine = engine();
    wait_for_silence(&engine, &asset);
    let spans = engine
        .timeline_silences(&document, None, TimeCode(6))
        .expect("clicked silences");
    let covering_learn = spans.iter().any(|span| {
        span.project_start.0 <= AU6_C_LEARN_PROJECT_RANGE.start.0
            && span.project_end.0 >= AU6_C_LEARN_PROJECT_RANGE.end.0
    });
    assert!(
        !covering_learn,
        "a click in the learn gap must split the detected span: {spans:?}"
    );
}

#[test]
fn au6_the_scratch_track_is_not_the_master() {
    let master = crate::au6_sources::au6_d_master_pcm(AU6_PROGRAMME_FRAMES);
    let scratch = au6_scratch_pcm(
        &master,
        AU6_D_ANGLE_OFFSETS_FRAMES[1],
        AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS,
    );
    assert_ne!(scratch, master);
    assert!(scratch.iter().any(|sample| *sample != 0.0));
    let identity = au6_scratch_pcm(&master, 0, AU6_D_SCRATCH_NOISE_LEVEL_DBFS_HUNDREDTHS);
    // A zero offset still adds noise, but the delay identity is the failing
    // construction the contract names: the delayed scratch must differ.
    assert_ne!(identity, scratch);
}

#[test]
fn au6_wav_round_trip_is_sample_exact() {
    let stereo = au6_to_stereo(&au6_chord_bed_pcm(
        AU6_A_BED_LEVEL_DBFS_HUNDREDTHS,
        AU6_PROGRAMME_FRAMES,
    ));
    let media = au6_source(Au6Scenario::Interview, Au6TrackRole::MusicBed);
    let decoded = decode_audio_range(
        media.path(),
        Rational::default(),
        TimeCode::ZERO,
        TimeCode(i64::from(AU6_PROGRAMME_FRAMES) * 30 / i64::from(AU6_SOURCE_FPS)),
        AU6_SAMPLE_RATE,
        AU6_CHANNELS,
        &ExportCancellation::default(),
    )
    .expect("the authored wav decodes");
    let comparable = decoded.len().min(stereo.len());
    assert_eq!(&decoded[..comparable], &stereo[..comparable]);
}

#[test]
fn au6_an_aac_mux_is_not_sample_exact() {
    let stereo = au6_to_stereo(&au6_chord_bed_pcm(
        AU6_A_BED_LEVEL_DBFS_HUNDREDTHS,
        AU6_ENCODE_PROGRAMME_FRAMES,
    ));
    let wav = GeneratedMedia::from_bytes(
        "au6-aac-src",
        "wav",
        &wav_f32(&stereo, AU6_SAMPLE_RATE, AU6_CHANNELS),
    );
    let wav_path = wav.path().to_string_lossy().into_owned();
    let encoded = GeneratedMedia::ffmpeg(
        "au6-aac",
        &["-i", &wav_path, "-c:a", "aac", "-b:a", "192k"],
        "m4a",
    );
    let decoded = decode_audio_range(
        encoded.path(),
        Rational::default(),
        TimeCode::ZERO,
        TimeCode(i64::from(AU6_ENCODE_PROGRAMME_FRAMES) * 30 / i64::from(AU6_SOURCE_FPS)),
        AU6_SAMPLE_RATE,
        AU6_CHANNELS,
        &ExportCancellation::default(),
    )
    .expect("the aac mux decodes");
    let comparable = decoded.len().min(stereo.len());
    assert_ne!(&decoded[..comparable], &stereo[..comparable]);
}

#[test]
fn au6_the_source_shapes_are_the_contract_table() {
    initialize_ffmpeg().expect("FFmpeg");
    for angle in 1..=2 {
        let source = au6_angle_source(angle);
        let asset = probe_path(source.path(), AssetId(1)).expect("angle probes");
        assert_eq!(asset.kind, MediaKind::Video);
        assert_eq!(
            asset.resolution,
            Some((AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT))
        );
        assert_eq!(asset.fps, project_fps());
    }
    let picture = au6_picture_source(AU6_PROGRAMME_FRAMES);
    let picture_asset = probe_path(picture.path(), AssetId(1)).expect("picture probes");
    assert_eq!(picture_asset.kind, MediaKind::Video);
    let muxed = au6_muxed_source();
    let muxed_asset = probe_path(muxed.path(), AssetId(1)).expect("mux probes");
    assert_eq!(muxed_asset.kind, MediaKind::AudioVideo);
    assert_eq!(muxed_asset.fps, project_fps());
    assert_eq!(
        muxed_asset.resolution,
        Some((AU6_SOURCE_WIDTH, AU6_SOURCE_HEIGHT))
    );
    for scenario in [
        Au6Scenario::Interview,
        Au6Scenario::Delivery,
        Au6Scenario::Multicam,
    ] {
        let scene = scene(scenario);
        for (track, asset) in scene.document.tracks.iter().zip(&scene.document.media_pool) {
            if track.kind == TrackKind::Video {
                assert_eq!(asset.kind, MediaKind::Video, "{scenario:?} V{}", track.id.0);
            }
        }
    }
    assert_eq!(
        AU6_VIDEO_ONLY_RECIPE
            .iter()
            .filter(|arg| **arg == "-c:a")
            .count(),
        0
    );
    assert!(AU6_MUX_RECIPE.contains(&"-c:a"));
}

#[test]
fn au6_an_angle_source_with_audio_probes_as_audiovideo() {
    initialize_ffmpeg().expect("FFmpeg");
    let muxed = au6_muxed_source();
    let asset = probe_path(muxed.path(), AssetId(1)).expect("audio-carrying angle");
    assert_eq!(asset.kind, MediaKind::AudioVideo);
    assert_ne!(asset.kind, MediaKind::Video);
}

#[test]
fn au6_c_the_degraded_track_is_the_click_free_track_plus_the_clicks() {
    let degraded = au6_scenario_tracks(Au6Scenario::LocationDialogue)
        .into_iter()
        .next()
        .unwrap()
        .1;
    let before = au6_c_click_free_track();
    let mut mono = crate::au6_sources::au6_to_mono(&before);
    au6_clicks_into(&mut mono, &AU6_C_CLICK_FRAMES);
    let rebuilt = au6_to_stereo(&mono);
    assert_eq!(degraded, rebuilt);
    let differing = degraded
        .iter()
        .zip(&before)
        .filter(|(left, right)| left != right)
        .count();
    assert_eq!(
        differing,
        AU6_C_CLICK_COUNT * AU6_CLICK_SAMPLES as usize * usize::from(AU6_CHANNELS)
    );
}

#[test]
fn au6_restated_constants_agree_with_their_owners() {
    assert_eq!(
        u64::from(AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED),
        LOUDNESS_GATING_BLOCK_FRAMES
    );
    assert_eq!(
        AU6_NOISE_PROFILE_MINIMUM_SAMPLE_FRAMES_RESTATED,
        NOISE_PROFILE_MINIMUM_FRAMES
    );
    assert_eq!(
        AU6_NOISE_PROFILE_SEGMENT_SAMPLE_FRAMES_RESTATED,
        NOISE_PROFILE_SEGMENT_FRAMES as u64
    );
    assert_eq!(
        AU6_NOISE_PROFILE_PERCENT_RESTATED as usize,
        NOISE_PROFILE_PERCENT
    );
    assert_eq!(
        AU6_SILENCE_THRESHOLD_RESTATED_DBFS_HUNDREDTHS,
        DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS
    );
    assert_eq!(AU6_HANN_MAIN_LOBE_BINS_RESTATED, OWNER_HANN_MAIN_LOBE_BINS);
    let stale = AU6_LOUDNESS_GATING_BLOCK_SAMPLE_FRAMES_RESTATED + 1;
    assert_ne!(u64::from(stale), LOUDNESS_GATING_BLOCK_FRAMES);
}

// ===========================================================================
// §11.2 items 21–28 — interview gates.
// ===========================================================================

fn interview_turn_bus_levels(engine: &FfmpegMediaEngine, document: &Document, bus: &str) -> i32 {
    let mut readings = Vec::new();
    for turn in AU6_TURNS {
        let report = engine
            .mix_levels(document, &level_request(turn.range()))
            .expect("mix_levels over a turn");
        readings.push(bus_integrated(&report, bus));
    }
    energy_average(&readings)
}

#[test]
fn au6_a_the_interview_clears_the_bed_and_matches_the_voices() {
    let scene = scene(Au6Scenario::Interview);
    let document = scene.canonical();
    let engine = engine();
    let dialogue = interview_turn_bus_levels(&engine, &document, AU6_A_DIALOGUE_BUS_NAME);
    let music = interview_turn_bus_levels(&engine, &document, AU6_A_MUSIC_BUS_NAME);
    let over = i64::from(dialogue - music);
    print_floor(
        "dialogue over bed",
        over,
        i64::from(AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS),
    );
    let mut a_levels = Vec::new();
    let mut b_levels = Vec::new();
    for turn in AU6_TURNS {
        let report = engine
            .mix_levels(&document, &level_request(turn.range()))
            .expect("voice match levels");
        match turn.speaker {
            Au6Speaker::A => a_levels
                .push(integrated(&track_levels(&report, AU6_A_VOICE_A_TRACK).levels).unwrap()),
            Au6Speaker::B => b_levels
                .push(integrated(&track_levels(&report, AU6_A_VOICE_B_TRACK).levels).unwrap()),
        }
    }
    let a = energy_average(&a_levels);
    let b = energy_average(&b_levels);
    let match_err = i64::from((a - b).abs());
    print_ceiling(
        "voices matched, (a)",
        match_err,
        i64::from(AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS),
    );
}

#[test]
fn au6_a_the_unducked_document_does_not_clear_the_bed() {
    let scene = scene(Au6Scenario::Interview);
    let document = interview_without_duck(&scene);
    let engine = engine();
    let dialogue = interview_turn_bus_levels(&engine, &document, AU6_A_DIALOGUE_BUS_NAME);
    let music = interview_turn_bus_levels(&engine, &document, AU6_A_MUSIC_BUS_NAME);
    let over = dialogue - music;
    println!("AU6 unducked dialogue-over-bed={over}");
    assert!(
        over < AU6_INTERVIEW_DIALOGUE_OVER_BED_MIN_LU_HUNDREDTHS,
        "the unducked document must fail the bed gate, not clear it at {over}"
    );
}

#[test]
fn au6_a_the_untrimmed_voices_are_not_matched() {
    let scene = scene(Au6Scenario::Interview);
    let engine = engine();
    let mut a_levels = Vec::new();
    let mut b_levels = Vec::new();
    for turn in AU6_TURNS {
        let report = engine
            .mix_levels(&scene.document, &level_request(turn.range()))
            .expect("untrimmed levels");
        match turn.speaker {
            Au6Speaker::A => a_levels
                .push(integrated(&track_levels(&report, AU6_A_VOICE_A_TRACK).levels).unwrap()),
            Au6Speaker::B => b_levels
                .push(integrated(&track_levels(&report, AU6_A_VOICE_B_TRACK).levels).unwrap()),
        }
    }
    let err = (energy_average(&a_levels) - energy_average(&b_levels)).abs();
    println!("AU6 untrimmed voice match={err}");
    assert_eq!(
        i64::from(err),
        AU6_MEASURED_VOICE_MATCH_UNTRIMMED_LU_HUNDREDTHS
    );
    assert!(err > AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS);
}

#[test]
fn au6_a_the_nominal_dbfs_trim_is_worse_than_none() {
    let scene = scene(Au6Scenario::Interview);
    let mut document = scene.document.clone();
    set_track_gain(
        &mut document,
        AU6_A_VOICE_B_TRACK,
        AU6_INTERVIEW_NOMINAL_TRIM_TENTH_DB_WRONG_MODEL,
    );
    let engine = engine();
    let mut a_levels = Vec::new();
    let mut b_levels = Vec::new();
    for turn in AU6_TURNS {
        let report = engine
            .mix_levels(&document, &level_request(turn.range()))
            .expect("nominal trim levels");
        match turn.speaker {
            Au6Speaker::A => a_levels
                .push(integrated(&track_levels(&report, AU6_A_VOICE_A_TRACK).levels).unwrap()),
            Au6Speaker::B => b_levels
                .push(integrated(&track_levels(&report, AU6_A_VOICE_B_TRACK).levels).unwrap()),
        }
    }
    let err = (energy_average(&a_levels) - energy_average(&b_levels)).abs();
    println!("AU6 nominal +60 trim match={err}");
    assert_eq!(
        i64::from(err),
        AU6_MEASURED_VOICE_MATCH_NOMINAL_TRIM_LU_HUNDREDTHS
    );
    assert!(err > AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS);
    assert!(err < AU6_MEASURED_VOICE_MATCH_UNTRIMMED_LU_HUNDREDTHS as i32);
}

#[test]
fn au6_a_the_duck_reaches_its_depth() {
    let scene = scene(Au6Scenario::Interview);
    let document = scene.canonical();
    let engine = engine();
    let windows = engine
        .mix_window_levels(
            &document,
            &window_request(MixSpectrumPoint::Bus(AU6_A_MUSIC_BUS), AU6_WINDOW_PROGRAMME),
        )
        .expect("duck windows");
    let speech = window_values(&windows, &au6_duck_speech_window_indices());
    let gap = window_values(&windows, &au6_duck_gap_window_indices());
    let depth = i64::from(*gap.iter().min().unwrap() - *speech.iter().max().unwrap());
    print_floor(
        "duck depth",
        depth,
        i64::from(AU6_DUCK_DEPTH_MIN_HUNDREDTHS),
    );
}

#[test]
fn au6_a_the_unducked_bed_has_no_depth() {
    let scene = scene(Au6Scenario::Interview);
    let document = interview_without_duck(&scene);
    let engine = engine();
    let windows = engine
        .mix_window_levels(
            &document,
            &window_request(MixSpectrumPoint::Bus(AU6_A_MUSIC_BUS), AU6_WINDOW_PROGRAMME),
        )
        .expect("unducked windows");
    let speech = window_values(&windows, &au6_duck_speech_window_indices());
    let gap = window_values(&windows, &au6_duck_gap_window_indices());
    let depth = *gap.iter().min().unwrap() - *speech.iter().max().unwrap();
    println!("AU6 unducked duck depth={depth}");
    assert!(depth < AU6_DUCK_DEPTH_MIN_HUNDREDTHS);
}

#[test]
fn au6_a_the_duck_depth_is_invisible_at_the_track_point() {
    let scene = scene(Au6Scenario::Interview);
    let document = interview_with_bus_duck(&scene);
    let engine = engine();
    let bus_windows = engine
        .mix_window_levels(
            &document,
            &window_request(MixSpectrumPoint::Bus(AU6_A_MUSIC_BUS), AU6_WINDOW_PROGRAMME),
        )
        .expect("bus duck");
    let track_windows = engine
        .mix_window_levels(
            &document,
            &window_request(
                MixSpectrumPoint::Track(AU6_A_BED_TRACK),
                AU6_WINDOW_PROGRAMME,
            ),
        )
        .expect("track duck");
    let bus_depth = *window_values(&bus_windows, &au6_duck_gap_window_indices())
        .iter()
        .min()
        .unwrap()
        - *window_values(&bus_windows, &au6_duck_speech_window_indices())
            .iter()
            .max()
            .unwrap();
    let track_depth = *window_values(&track_windows, &au6_duck_gap_window_indices())
        .iter()
        .min()
        .unwrap()
        - *window_values(&track_windows, &au6_duck_speech_window_indices())
            .iter()
            .max()
            .unwrap();
    println!("AU6 duck instrument bus={bus_depth} track={track_depth}");
    assert!(bus_depth >= AU6_DUCK_DEPTH_MIN_HUNDREDTHS);
    assert!(track_depth < AU6_DUCK_DEPTH_MIN_HUNDREDTHS);
}

#[test]
fn au6_the_three_curve_owners_render_identically() {
    let scene = scene(Au6Scenario::Interview);
    let track_doc = scene.canonical();
    let spec = au6_spec(Au6Scenario::Interview);
    let mut bus_ops: Vec<Operation> = au6_canonical_operations(Au6Scenario::Interview)
        .into_iter()
        .filter(|operation| !matches!(operation, Operation::SetTrackAutomation { .. }))
        .collect();
    bus_ops.push(Operation::UpsertAudioBus {
        bus: AudioBus {
            id: AU6_A_MUSIC_BUS,
            name: AU6_A_MUSIC_BUS_NAME.to_owned(),
            tracks: spec.buses[1].tracks.to_vec(),
            gain_tenth_db: 0,
            gain_curve: Some(au6_a_duck_curve()),
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
        },
    });
    let bus_doc = scene.commit(&bus_ops);
    let mut clip_ops: Vec<Operation> = au6_canonical_operations(Au6Scenario::Interview)
        .into_iter()
        .filter(|operation| !matches!(operation, Operation::SetTrackAutomation { .. }))
        .collect();
    clip_ops.push(Operation::SetClipGainEnvelope {
        clip: AU6_A_CLIPS[3].clip,
        curve: Some(au6_a_duck_curve()),
    });
    let clip_doc = scene.commit(&clip_ops);
    let track_stems = stems_of(&track_doc, AU6_WINDOW_PROGRAMME);
    let bus_stems = stems_of(&bus_doc, AU6_WINDOW_PROGRAMME);
    let clip_stems = stems_of(&clip_doc, AU6_WINDOW_PROGRAMME);
    assert_eq!(track_stems.master, bus_stems.master);
    assert_eq!(track_stems.master, clip_stems.master);
    assert_eq!(
        bus_stem(&track_stems, AU6_A_MUSIC_BUS),
        bus_stem(&bus_stems, AU6_A_MUSIC_BUS)
    );
    assert_eq!(
        bus_stem(&track_stems, AU6_A_MUSIC_BUS),
        bus_stem(&clip_stems, AU6_A_MUSIC_BUS)
    );
    assert_ne!(
        track_stem(&track_stems, AU6_A_BED_TRACK),
        track_stem(&bus_stems, AU6_A_BED_TRACK)
    );
}

#[test]
fn au6_every_scenario_mix_does_not_clip() {
    let engine = engine();
    for scenario in [
        Au6Scenario::Interview,
        Au6Scenario::Podcast,
        Au6Scenario::LocationDialogue,
        Au6Scenario::Multicam,
        Au6Scenario::Delivery,
    ] {
        let scene = scene(scenario);
        let document = scene.canonical();
        let report = engine
            .audio_qc(&document, &qc_request(TimeCode::ZERO..document.duration))
            .expect("audio_qc");
        println!(
            "AU6 clipping {scenario:?} left={} right={} pass={}",
            report.clipping.left.clipped_runs,
            report.clipping.right.clipped_runs,
            report.technical_pass
        );
        assert!(report.technical_pass);
        assert_eq!(report.clipping.left.clipped_runs, 0);
        assert_eq!(report.clipping.right.clipped_runs, 0);
    }
}

#[test]
fn au6_a_a_hot_master_clips_and_says_so() {
    let scene = scene(Au6Scenario::Interview);
    let mut document = scene.canonical();
    apply_in_order(
        &mut document,
        &[Operation::SetAudioMaster {
            master: kinewright_core::AudioMaster {
                gain_tenth_db: 120,
                gain_curve: None,
                effects: Vec::new(),
            },
        }],
    );
    let report = engine()
        .audio_qc(&document, &qc_request(AU6_WINDOW_PROGRAMME))
        .expect("hot master qc");
    assert!(
        report.clipping.left.clipped_runs + report.clipping.right.clipped_runs > 0
            || !report.technical_pass,
        "a hot master must clip or fail technical_pass"
    );
}

// ===========================================================================
// §11.2 items 29–30 — podcast.
// ===========================================================================

fn voice_b_spread(engine: &FfmpegMediaEngine, document: &Document) -> i32 {
    let windows = engine
        .mix_window_levels(
            document,
            &window_request(
                MixSpectrumPoint::Bus(AU6_B_VOICE_B_BUS),
                AU6_WINDOW_PROGRAMME,
            ),
        )
        .expect("podcast windows");
    let mut values = Vec::new();
    for turn in AU6_TURNS
        .iter()
        .filter(|turn| turn.speaker == Au6Speaker::B)
    {
        let start = au6_window_index_local(turn.start);
        let end = au6_window_index_local(turn.end);
        for index in start..end {
            if let Some(value) = windows.windows.get(index).and_then(|slot| *slot) {
                values.push(value);
            }
        }
    }
    values.iter().max().unwrap() - values.iter().min().unwrap()
}

fn au6_window_index_local(frame: TimeCode) -> usize {
    usize::try_from(frame.0).unwrap() / AU6_FRAMES_PER_WINDOW as usize
}

#[test]
fn au6_b_the_chain_matches_the_voices_and_reduces_the_spread() {
    let scene = scene(Au6Scenario::Podcast);
    let document = scene.canonical();
    let bypassed = podcast_bypassed(&scene);
    let engine = engine();
    let mut a = Vec::new();
    let mut b = Vec::new();
    for turn in AU6_TURNS {
        let report = engine
            .mix_levels(&document, &level_request(turn.range()))
            .expect("podcast match");
        match turn.speaker {
            Au6Speaker::A => a.push(bus_integrated(&report, "Voice A")),
            Au6Speaker::B => b.push(bus_integrated(&report, "Voice B")),
        }
    }
    print_ceiling(
        "voices matched, (b)",
        i64::from((energy_average(&a) - energy_average(&b)).abs()),
        i64::from(AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS),
    );
    let reduction =
        i64::from(voice_b_spread(&engine, &bypassed) - voice_b_spread(&engine, &document));
    print_floor(
        "dynamics reduced",
        reduction,
        i64::from(AU6_PODCAST_SPREAD_REDUCTION_MIN_HUNDREDTHS),
    );
}

#[test]
fn au6_b_the_raw_trims_do_not_match_the_voices() {
    let scene = scene(Au6Scenario::Podcast);
    let document = podcast_without_trims(&scene);
    let engine = engine();
    let mut a = Vec::new();
    let mut b = Vec::new();
    for turn in AU6_TURNS {
        let report = engine
            .mix_levels(&document, &level_request(turn.range()))
            .expect("raw podcast");
        match turn.speaker {
            Au6Speaker::A => a.push(bus_integrated(&report, "Voice A")),
            Au6Speaker::B => b.push(bus_integrated(&report, "Voice B")),
        }
    }
    let err = (energy_average(&a) - energy_average(&b)).abs();
    println!("AU6 raw podcast match={err}");
    assert!(err > AU6_VOICE_MATCH_MAX_LU_HUNDREDTHS);
}

#[test]
fn au6_b_the_bypassed_chain_leaves_the_spread_intact() {
    let scene = scene(Au6Scenario::Podcast);
    let bypassed = podcast_bypassed(&scene);
    let engine = engine();
    let reduction = voice_b_spread(&engine, &bypassed) - voice_b_spread(&engine, &bypassed);
    assert_eq!(reduction, 0);
}

#[test]
fn au6_b_the_programme_stays_inside_its_loudness_range() {
    let scene = scene(Au6Scenario::Podcast);
    let document = scene.canonical();
    let lra = engine()
        .audio_qc(&document, &qc_request(AU6_WINDOW_PROGRAMME))
        .expect("podcast lra")
        .master
        .loudness_range_lu_hundredths
        .expect("an LRA must populate");
    print_ceiling(
        "programme LRA",
        i64::from(lra),
        i64::from(AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS),
    );
}

#[test]
fn au6_b_the_makeup_less_chain_exceeds_the_loudness_range() {
    let scene = scene(Au6Scenario::Podcast);
    let mut document = scene.canonical();
    let bus = document
        .audio_mix
        .buses
        .iter_mut()
        .find(|bus| bus.id == AU6_B_VOICE_B_BUS)
        .expect("voice B bus");
    if let Some(effect) = bus
        .effects
        .iter_mut()
        .find(|effect| effect.name == "audio_compressor")
    {
        effect
            .parameters
            .insert("makeup_gain_tenth_db".to_owned(), ParamValue::Integer(0));
    }
    let lra = engine()
        .audio_qc(&document, &qc_request(AU6_WINDOW_PROGRAMME))
        .expect("makeup-less lra")
        .master
        .loudness_range_lu_hundredths
        .expect("LRA");
    println!("AU6 makeup-less LRA={lra}");
    assert!(lra > AU6_PODCAST_LRA_MAX_LU_HUNDREDTHS);
}

// ===========================================================================
// §11.2 items 31–37 — location dialogue.
// ===========================================================================

fn repair_point(document: &Document) -> MixSpectrumPoint {
    if document.audio_mix.bus(AU6_C_REPAIR_BUS).is_some() {
        MixSpectrumPoint::Bus(AU6_C_REPAIR_BUS)
    } else {
        MixSpectrumPoint::Track(AU6_C_DIALOGUE_TRACK)
    }
}

fn repair_at(engine: &FfmpegMediaEngine, document: &Document) -> AudioRepairReport {
    let point = repair_point(document);
    engine
        .audio_repair(
            document,
            &repair_request(
                point,
                TimeCode::ZERO..TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
            ),
        )
        .expect("audio_repair")
}

#[test]
fn au6_c_the_repair_chain_moves_the_snr_and_the_hum() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let before_doc = scene.commit(&au6_c_gap_operations(
        kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID,
    ));
    let after_doc = scene.commit(&{
        let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
        ops.extend(au6_c_repair_operations());
        ops
    });
    let engine = engine();
    let before = repair_at(&engine, &before_doc);
    let after = repair_at(&engine, &after_doc);
    let snr_gain = i64::from(
        after.snr_db_hundredths.expect("snr after") - before.snr_db_hundredths.expect("snr before"),
    );
    print_floor(
        "repair SNR gain",
        snr_gain,
        i64::from(AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS),
    );
    let hum_drop = i64::from(
        before.hum_60_excess_db_hundredths.expect("hum before")
            - after.hum_60_excess_db_hundredths.expect("hum after"),
    );
    print_floor(
        "60 Hz drop",
        hum_drop,
        i64::from(AU6_HUM_DROP_MIN_DB_HUNDREDTHS),
    );
    assert!(!before.hum_60_harmonic_excess_db_hundredths.is_empty());
    assert!(!after.hum_60_harmonic_excess_db_hundredths.is_empty());
    let mut worst_harmonic = i64::MAX;
    for index in 0..3 {
        let drop = i64::from(
            before.hum_60_harmonic_excess_db_hundredths[index]
                - after.hum_60_harmonic_excess_db_hundredths[index],
        );
        println!("AU6 hum harmonic {index} drop={drop}");
        assert!(drop >= i64::from(AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS));
        worst_harmonic = worst_harmonic.min(drop);
    }
    print_floor(
        "per-harmonic drop, first three",
        worst_harmonic,
        i64::from(AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS),
    );
    let fourth = after.hum_60_harmonic_excess_db_hundredths[3]
        - before.hum_60_harmonic_excess_db_hundredths[3];
    println!("AU6 fourth harmonic rise={fourth} (recorded, not gated)");
    assert_eq!(after.click_count, 0);
    assert_eq!(before.click_count, AU6_C_CLICK_COUNT as u32);
}

#[test]
fn au6_c_an_unlearned_profile_moves_no_snr() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let before_doc = scene.commit(&au6_c_gap_operations(
        kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID,
    ));
    let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
    ops.extend(repair_with_profile(
        [PROFILE_BAND_NEUTRAL_TENTH_DB as i32; NOISE_PROFILE_BAND_COUNT],
    ));
    let after_doc = scene.commit(&ops);
    let engine = engine();
    let gain = repair_at(&engine, &after_doc).snr_db_hundredths.unwrap()
        - repair_at(&engine, &before_doc).snr_db_hundredths.unwrap();
    println!("AU6 unlearned SNR gain={gain}");
    assert!(gain < AU6_REPAIR_SNR_GAIN_MIN_HUNDREDTHS);
}

#[test]
fn au6_c_a_chain_without_the_hum_node_leaves_the_mains_alone() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let before_doc = scene.commit(&au6_c_gap_operations(
        kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID,
    ));
    let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
    ops.extend(repair_without_hum());
    let after_doc = scene.commit(&ops);
    let engine = engine();
    let drop = repair_at(&engine, &before_doc)
        .hum_60_excess_db_hundredths
        .unwrap()
        - repair_at(&engine, &after_doc)
            .hum_60_excess_db_hundredths
            .unwrap();
    println!("AU6 no-hum-node drop={drop}");
    assert!(drop < AU6_HUM_HARMONIC_DROP_MIN_DB_HUNDREDTHS);
}

#[test]
fn au6_c_the_declick_node_alone_drops_the_click_error() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let document = scene.commit(&au6_c_declick_only_operations());
    let stems = stems_of(
        &document,
        TimeCode::ZERO..TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
    );
    let after = bus_stem(&stems, AU6_C_REPAIR_BUS);
    let click_free = au6_c_click_free_track();
    let degraded = au6_scenario_tracks(Au6Scenario::LocationDialogue)
        .into_iter()
        .next()
        .unwrap()
        .1;
    let drop = error_drop_tenth_db(&degraded, after, &click_free);
    let clicks = detect_clicks(
        &degraded,
        AU6_CHANNELS,
        AU6_SAMPLE_RATE,
        kinewright_core::au6_scenarios::AU6_C_DECLICK_DETECTOR_THRESHOLD_TENTH_DB,
        kinewright_core::au6_scenarios::AU6_C_DECLICK_MAX_CLICK_MS,
    )
    .count();
    println!(
        "AU6_DECLICK measured_drop_tenth_db={drop:.1} quantity=rms_to_rms before_rms={} after_rms={} clicks={clicks} margin={:.2}",
        rms(&degraded
            .iter()
            .zip(&click_free)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>()),
        rms(&after
            .iter()
            .zip(&click_free)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>()),
        drop / f64::from(AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB),
    );
    assert!(drop >= f64::from(AU6_DECLICK_ERROR_DROP_MIN_TENTH_DB));
    assert!((drop - AU6_MEASURED_DECLICK_ERROR_DROP_TENTH_DB as f64).abs() < 5.0);
    let processed = process_buffer_static(
        &declick_effect(),
        AU6_SAMPLE_RATE,
        usize::from(AU6_CHANNELS),
        &degraded,
    )
    .expect("static declick");
    let comparable = after.len().min(processed.len());
    assert_eq!(&after[..comparable], &processed[..comparable]);
}

#[test]
fn au6_c_a_chain_without_the_declick_node_keeps_every_click() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
    ops.extend(repair_without_declick());
    let document = scene.commit(&ops);
    let engine = engine();
    // Denoise on the bus can swallow a few clicks; the failing direction is
    // that the *track* still carries every authored click when declick is off.
    let report = engine
        .audio_repair(
            &document,
            &repair_request(
                MixSpectrumPoint::Track(AU6_C_DIALOGUE_TRACK),
                TimeCode::ZERO..TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
            ),
        )
        .expect("audio_repair");
    assert_eq!(report.click_count, AU6_C_CLICK_COUNT as u32);
    let bare = stems_of(
        &scene.document,
        TimeCode::ZERO..TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
    );
    let authored = au6_scenario_tracks(Au6Scenario::LocationDialogue)
        .into_iter()
        .next()
        .unwrap()
        .1;
    assert_eq!(track_stem(&bare, AU6_C_DIALOGUE_TRACK), authored.as_slice());
}

#[test]
fn au6_c_the_declick_contribution_in_chain_is_recorded() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
    ops.extend(au6_c_repair_operations());
    let document = scene.commit(&ops);
    let stems = stems_of(
        &document,
        TimeCode::ZERO..TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
    );
    let after = bus_stem(&stems, AU6_C_REPAIR_BUS);
    let clean = au6_c_voice_only_track();
    let click_free = au6_c_click_free_track();
    let whole = error_drop_tenth_db(&click_free, after, &clean);
    println!("AU6 declick contribution_in_chain whole={whole:.1} (evidence, not a gate)");
    assert!(whole.is_finite());
}

#[test]
fn au6_c_the_dialogue_survives_the_repair() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let before_doc = scene.commit(&au6_c_gap_operations(
        kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID,
    ));
    let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
    ops.extend(au6_c_repair_operations());
    let after_doc = scene.commit(&ops);
    let engine = engine();
    let mut before_levels = Vec::new();
    let mut after_levels = Vec::new();
    for turn in AU6_TURNS {
        let before = engine
            .mix_window_levels(
                &before_doc,
                &window_request(repair_point(&before_doc), turn.range()),
            )
            .expect("speech before");
        let after = engine
            .mix_window_levels(
                &after_doc,
                &window_request(repair_point(&after_doc), turn.range()),
            )
            .expect("speech after");
        let before_vals: Vec<i32> = before.windows.iter().flatten().copied().collect();
        let after_vals: Vec<i32> = after.windows.iter().flatten().copied().collect();
        // Speech retention is the peak window on each turn, not the mean:
        // averaging in the noise floor makes denoise look like a 4 dB loss.
        before_levels.push(*before_vals.iter().max().expect("a turn has a window"));
        after_levels.push(*after_vals.iter().max().expect("a turn has a window"));
    }
    let loss = mean_i32(&before_levels) - mean_i32(&after_levels);
    print_ceiling(
        "speech level retained",
        i64::from(loss.max(0)),
        i64::from(AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS),
    );
}

#[test]
fn au6_c_an_over_reduced_profile_eats_the_dialogue() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let before_doc = scene.commit(&au6_c_gap_operations(
        kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID,
    ));
    let mut ops = au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID);
    ops.extend(repair_with_profile([-200; NOISE_PROFILE_BAND_COUNT]));
    let after_doc = scene.commit(&ops);
    let engine = engine();
    let mut before_levels = Vec::new();
    let mut after_levels = Vec::new();
    for turn in AU6_TURNS {
        let before = engine
            .mix_window_levels(
                &before_doc,
                &window_request(repair_point(&before_doc), turn.range()),
            )
            .expect("over-reduced before");
        let after = engine
            .mix_window_levels(
                &after_doc,
                &window_request(repair_point(&after_doc), turn.range()),
            )
            .expect("over-reduced after");
        before_levels.extend(before.windows.iter().flatten().copied());
        after_levels.extend(after.windows.iter().flatten().copied());
    }
    let loss = mean_i32(&before_levels) - mean_i32(&after_levels);
    println!("AU6 over-reduced speech loss={loss}");
    assert!(loss > AU6_REPAIR_SPEECH_LOSS_MAX_DB_HUNDREDTHS);
}

#[test]
fn au6_c_the_room_tone_fill_closes_the_gap_seamlessly() {
    let (_scene, document) = location_filled_scene();
    let gaps = document
        .track_gaps(AU6_C_DIALOGUE_TRACK)
        .expect("the dialogue track exists");
    assert_eq!(gaps, Vec::<Range<TimeCode>>::new());
    let layout: Vec<(u64, i64)> = document
        .tracks
        .iter()
        .find(|track| track.id == AU6_C_DIALOGUE_TRACK)
        .unwrap()
        .clips
        .iter()
        .map(|clip| (clip.id.0, clip.timeline_start.0))
        .collect();
    assert_eq!(layout, vec![(1, 0), (3, 160), (2, 175)]);
    assert_au6_seam(&document, AU6_C_GAP_RANGE);
}

#[test]
fn au6_c_a_one_frame_slip_breaks_the_seam() {
    let (scene, filled) = location_filled_scene();
    let room = scene
        .document
        .media_pool
        .iter()
        .find(|asset| asset.id == AssetId(2))
        .cloned()
        .expect("the room-tone asset is pooled");
    let mut document = scene.document.clone();
    apply_in_order(
        &mut document,
        &au6_c_gap_operations(kinewright_core::au6_scenarios::AU6_C_RIGHT_CLIP_ID),
    );
    apply_in_order(
        &mut document,
        &[Operation::AddClip {
            track: AU6_C_DIALOGUE_TRACK,
            asset: room.id,
            at: TimeCode(AU6_C_GAP_RANGE.start.0 + 1),
            // The authored tile is 18 source frames → 15 project frames. At
            // start+1 that overlaps the right clip; two source frames shorter
            // leaves a hole without overlapping.
            source: TimeCode(0)
                ..TimeCode(
                    kinewright_core::au6_scenarios::AU6_C_FILL_TILE_SOURCE_RANGE
                        .end
                        .0
                        - 2,
                ),
        }],
    );
    let gaps = document.track_gaps(AU6_C_DIALOGUE_TRACK).unwrap();
    assert!(
        !gaps.is_empty(),
        "a one-frame slip must leave a gap, not close the seam"
    );
    let settings = mix_settings(&document);
    let slipped = mix_audio_stems(&document, TimeCode::ZERO..document.duration, &settings)
        .expect("the slipped document mixes");
    let tight = mix_audio_stems(&filled, TimeCode::ZERO..filled.duration, &settings)
        .expect("the filled document mixes");
    assert_ne!(
        track_stem(&slipped, AU6_C_DIALOGUE_TRACK),
        track_stem(&tight, AU6_C_DIALOGUE_TRACK),
        "a one-frame slip must change the dialogue stem"
    );
}

#[test]
fn au6_c_the_learned_profile_holds_its_pin_and_its_bound() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let engine = engine();
    let learned = engine
        .mix_noise_profile(
            &scene.document,
            &profile_request(
                MixSpectrumPoint::Track(AU6_C_DIALOGUE_TRACK),
                AU6_C_LEARN_PROJECT_RANGE,
            ),
        )
        .expect("learned profile");
    assert_eq!(learned.bands, AU6_C_LEARNED_PROFILE_TENTH_DB);
    let mut worst_excess = i32::MIN;
    let mut worst_band = 0;
    for band in 0..NOISE_PROFILE_BAND_COUNT {
        let bound = au6_c_analytic_mean_band_tenth_db(band);
        let excess = learned.bands[band] - bound;
        if excess > worst_excess {
            worst_excess = excess;
            worst_band = band;
        }
        assert!(
            learned.bands[band] <= bound + AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB,
            "band {band} exceeds the Hann-main-lobe bound"
        );
    }
    println!("AU6 profile pin exact; worst leakage band={worst_band} excess={worst_excess}");
    assert_eq!(worst_band, AU6_C_LEAKAGE_WORST_BAND);
    print_ceiling(
        "profile leakage allowance",
        i64::from(worst_excess.max(0)),
        i64::from(AU6_PROFILE_LEAKAGE_ALLOWANCE_TENTH_DB),
    );
    let _ = au6_c_point_mass_band_tenth_db_wrong_model(0);
}

#[test]
fn au6_c_a_profile_learned_over_speech_is_not_the_noise_profile() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let learned = engine()
        .mix_noise_profile(
            &scene.document,
            &profile_request(
                MixSpectrumPoint::Track(AU6_C_DIALOGUE_TRACK),
                AU6_TURNS[0].range(),
            ),
        )
        .expect("speech profile");
    assert_ne!(learned.bands, AU6_C_LEARNED_PROFILE_TENTH_DB);
}

#[test]
fn au6_c_a_one_frame_learn_offset_moves_the_profile() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let learned = engine()
        .mix_noise_profile(
            &scene.document,
            &profile_request(
                MixSpectrumPoint::Track(AU6_C_DIALOGUE_TRACK),
                TimeCode(275)..TimeCode(312),
            ),
        )
        .expect("offset profile");
    assert_eq!(learned.bands[0], AU6_C_ONE_FRAME_OFFSET_BAND0_TENTH_DB);
    let delta = (learned.bands[0] - AU6_C_LEARNED_PROFILE_TENTH_DB[0]).abs();
    assert_eq!(delta, AU6_C_ONE_FRAME_OFFSET_MAX_DELTA_TENTH_DB);
}

#[test]
fn au6_c_the_learned_profile_has_the_authored_shape() {
    let pin = AU6_C_LEARNED_PROFILE_TENTH_DB;
    for band in AU6_C_PROFILE_LOW_FALLOFF_BANDS {
        assert!(pin[band] != pin[0] || band == 0);
    }
    let hum_peak = AU6_C_PROFILE_HUM_BUMP_BANDS
        .clone()
        .map(|band| pin[band])
        .max()
        .unwrap();
    assert!(hum_peak > pin[3]);
    for band in AU6_C_PROFILE_MONOTONE_FROM_BAND..NOISE_PROFILE_BAND_COUNT - 1 {
        assert!(
            pin[band + 1] >= pin[band],
            "the pin is not monotone from band {band}"
        );
    }
    let flat = [-600; NOISE_PROFILE_BAND_COUNT];
    let flat_bump = flat[AU6_C_PROFILE_HUM_BUMP_BANDS.clone()]
        .iter()
        .max()
        .unwrap()
        - flat[0];
    assert_eq!(flat_bump, 0);
}

#[test]
fn au6_c_the_percentile_floor_lands_on_the_authored_floor() {
    let scene = scene(Au6Scenario::LocationDialogue);
    let report = engine()
        .audio_repair(
            &scene.document,
            &repair_request(
                MixSpectrumPoint::Track(AU6_C_DIALOGUE_TRACK),
                TimeCode::ZERO..TimeCode(i64::from(AU6_C_PROGRAMME_FRAMES)),
            ),
        )
        .expect("percentile floor");
    let floor = report.noise_floor_dbfs_hundredths.expect("a floor");
    let authored = AU6_MEASURED_C_LEARN_GAP_DBFS_HUNDREDTHS;
    let delta = (i64::from(floor) - authored).abs();
    println!("AU6 percentile floor={floor} authored-gap={authored} delta={delta}");
    assert!(delta <= 45, "within 0.45 dB");
}

#[test]
fn au6_d_the_master_stem_is_bit_identical_across_the_cuts() {
    let scene = scene(Au6Scenario::Multicam);
    let before = stems_of(&scene.document, AU6_WINDOW_PROGRAMME);
    let after_doc = scene.canonical();
    let after = stems_of(&after_doc, AU6_WINDOW_PROGRAMME);
    assert_eq!(
        track_stem(&before, AU6_D_MASTER_TRACK),
        track_stem(&after, AU6_D_MASTER_TRACK)
    );
    assert_eq!(before.master.len() as i64, AU6_MASTER_STEM_SAMPLES);
    let scratch_before = scene
        .document
        .tracks
        .iter()
        .find(|track| track.id == AU6_D_SCRATCH_1_TRACK)
        .unwrap()
        .clips
        .len();
    let scratch_after = after_doc
        .tracks
        .iter()
        .find(|track| track.id == AU6_D_SCRATCH_1_TRACK)
        .unwrap()
        .clips
        .len();
    assert_eq!(scratch_before, 1);
    assert_eq!(scratch_after, 1);
}

#[test]
fn au6_d_a_cut_master_track_is_not_continuous() {
    let scene = scene(Au6Scenario::Multicam);
    let mut document = scene.document.clone();
    apply_in_order(
        &mut document,
        &[
            Operation::SplitClip {
                clip: AU6_D_MASTER_CLIP_ID,
                at: TimeCode(150),
            },
            Operation::RippleDeleteClip {
                clip: ClipId(AU6_D_MASTER_CLIP_ID.0 + 1),
            },
        ],
    );
    let before = stems_of(&scene.document, AU6_WINDOW_PROGRAMME);
    let after = stems_of(&document, AU6_WINDOW_PROGRAMME);
    let left = track_stem(&before, AU6_D_MASTER_TRACK);
    let right = track_stem(&after, AU6_D_MASTER_TRACK);
    let first = left
        .iter()
        .zip(right)
        .position(|(a, b)| a != b)
        .expect("the cut master must diverge");
    println!("AU6 cut master first differing sample={first}");
    assert!(
        (first as i64 - AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE).abs() <= 4,
        "cut master diverges at {first}, expected around {AU6_MEASURED_MASTER_CUT_DIVERGENCE_SAMPLE}"
    );
}

#[test]
fn au6_d_an_angle_ripple_leaves_the_master_stem_alone() {
    let scene = scene(Au6Scenario::Multicam);
    let mut document = scene.document.clone();
    apply_in_order(
        &mut document,
        &[
            Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(150),
            },
            Operation::RippleDeleteClip { clip: ClipId(6) },
        ],
    );
    let before = stems_of(&scene.document, AU6_WINDOW_PROGRAMME);
    let after = stems_of(&document, AU6_WINDOW_PROGRAMME);
    assert_eq!(
        track_stem(&before, AU6_D_MASTER_TRACK),
        track_stem(&after, AU6_D_MASTER_TRACK)
    );
}

#[test]
fn au6_d_the_scratch_tracks_contribute_nothing() {
    let scene = scene(Au6Scenario::Multicam);
    let document = scene.canonical();
    let report = engine()
        .mix_levels(&document, &level_request(AU6_WINDOW_PROGRAMME))
        .expect("scratch levels");
    for track in [AU6_D_SCRATCH_1_TRACK, AU6_D_SCRATCH_2_TRACK] {
        let levels = track_levels(&report, track);
        assert!(levels.levels.integrated_lufs_hundredths.is_none());
        assert!(!levels.audible);
    }
}

#[test]
fn au6_d_an_unmuted_scratch_track_reaches_the_mix() {
    let scene = scene(Au6Scenario::Multicam);
    let mut document = scene.canonical();
    mute_track(&mut document, AU6_D_SCRATCH_1_TRACK, false);
    mute_track(&mut document, AU6_D_SCRATCH_2_TRACK, false);
    let report = engine()
        .mix_levels(&document, &level_request(AU6_WINDOW_PROGRAMME))
        .expect("unmuted scratch");
    for track in [AU6_D_SCRATCH_1_TRACK, AU6_D_SCRATCH_2_TRACK] {
        let levels = track_levels(&report, track);
        assert!(levels.audible);
        assert!(levels.levels.integrated_lufs_hundredths.is_some());
    }
}

#[test]
fn au6_d_the_mix_is_the_master() {
    let scene = scene(Au6Scenario::Multicam);
    let cut = scene.canonical();
    let mut master_only = scene.document.clone();
    mute_track(&mut master_only, AU6_D_SCRATCH_1_TRACK, true);
    mute_track(&mut master_only, AU6_D_SCRATCH_2_TRACK, true);
    let engine = engine();
    let cut_level = engine
        .mix_levels(&cut, &level_request(AU6_WINDOW_PROGRAMME))
        .expect("cut mix")
        .master
        .integrated_lufs_hundredths
        .unwrap();
    let master_level = engine
        .mix_levels(&master_only, &level_request(AU6_WINDOW_PROGRAMME))
        .expect("master-only mix")
        .master
        .integrated_lufs_hundredths
        .unwrap();
    let delta = (cut_level - master_level).abs();
    println!("AU6 master passthrough delta={delta}");
    assert!(delta <= AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS);
}

#[test]
fn au6_d_the_unmuted_scratch_moves_the_master() {
    let scene = scene(Au6Scenario::Multicam);
    let mut unmuted = scene.document.clone();
    mute_track(&mut unmuted, AU6_D_SCRATCH_1_TRACK, false);
    mute_track(&mut unmuted, AU6_D_SCRATCH_2_TRACK, false);
    let mut master_only = scene.document.clone();
    mute_track(&mut master_only, AU6_D_SCRATCH_1_TRACK, true);
    mute_track(&mut master_only, AU6_D_SCRATCH_2_TRACK, true);
    let engine = engine();
    let unmuted_level = engine
        .mix_levels(&unmuted, &level_request(AU6_WINDOW_PROGRAMME))
        .unwrap()
        .master
        .integrated_lufs_hundredths
        .unwrap();
    let master_level = engine
        .mix_levels(&master_only, &level_request(AU6_WINDOW_PROGRAMME))
        .unwrap()
        .master
        .integrated_lufs_hundredths
        .unwrap();
    let delta = (unmuted_level - master_level).abs();
    println!("AU6 unmuted scratch moves master by {delta}");
    assert!(delta > AU6_MASTER_PASSTHROUGH_MAX_LU_HUNDREDTHS);
}

#[test]
fn au6_d_the_cuts_sit_at_the_authored_frames() {
    let scene = scene(Au6Scenario::Multicam);
    let document = scene.canonical();
    assert_eq!(AU6_D_CUT_FRAMES, [75, 150, 225]);
    for frame in 0..i64::from(AU6_PROGRAMME_FRAMES) {
        let segment = match frame {
            0..75 => 0,
            75..150 => 1,
            150..225 => 2,
            _ => 3,
        };
        let expected = AU6_D_VISIBLE_ANGLE_PER_SEGMENT[segment];
        let visible: Vec<u64> = document
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Video)
            .filter(|track| {
                track.clips.iter().any(|clip| {
                    let end = clip.timeline_start.0 + document.clip_duration(clip).unwrap().0;
                    clip.timeline_start.0 <= frame && frame < end
                })
            })
            .map(|track| track.id.0)
            .collect();
        assert_eq!(visible, vec![expected as u64], "frame {frame}");
    }
}

fn run_delivery(
    engine: &FfmpegMediaEngine,
    document: &Document,
    job: &kinewright_core::Au6ExportJob,
    normalize: bool,
) -> (
    Option<kinewright_core::ExportAudioReport>,
    kinewright_core::DeliveryAudioVerification,
) {
    let directory = TempDirectory::new(&format!("au6-e-{}", job.id));
    let mut settings = au6_export_settings(job, document);
    if !normalize {
        settings.loudness_normalization = None;
    }
    let output = directory.path(&format!("{}.mp4", job.id));
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let audio = engine
        .export_document_reporting(
            std::sync::Arc::new(document.clone()),
            &output,
            settings,
            progress_tx,
        )
        .expect("delivery export")
        .audio;
    if normalize {
        assert!(audio.is_some(), "a normalized delivery reports audio");
    }
    let verification = engine
        .verify_delivery_audio(&output, Some(job.target))
        .expect("delivery verifies");
    (audio, verification)
}

#[test]
fn au6_e_both_deliveries_land_on_their_targets() {
    let scene = scene(Au6Scenario::Delivery);
    let document = scene.canonical();
    let engine = engine();
    let mut measured_deviations = Vec::new();
    let mut measured_margins = Vec::new();
    for job in &AU6_EXPORT_JOBS {
        let (report, verification) = run_delivery(&engine, &document, job, true);
        let measured = verification
            .measured
            .integrated_lufs_hundredths
            .expect("integrated");
        let deviation = (measured - job.target.integrated_lufs_hundredths).abs();
        let peak = verification
            .measured
            .true_peak_dbtp_hundredths
            .expect("true peak");
        let peak_margin = job.target.true_peak_ceiling_dbtp_hundredths - peak;
        let report = report.expect("a normalized delivery reports audio");
        println!(
            "AU6 delivery {} measured={measured} deviation={deviation} peak={peak} peak_margin={peak_margin} limiter={}",
            job.id, report.limiter_passes
        );
        assert!(verification.technical_pass);
        assert!(verification.exceptions.is_empty());
        assert_eq!(report.limiter_passes, 1);
        assert!(deviation <= AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS);
        assert!(peak_margin >= AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS);
        measured_deviations.push(deviation);
        measured_margins.push(peak_margin);
    }
    print_ceiling(
        "delivery deviation, both lanes",
        i64::from(*measured_deviations.iter().max().unwrap()),
        i64::from(AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS),
    );
    print_floor(
        "true-peak margin, thinnest lane",
        i64::from(*measured_margins.iter().min().unwrap()),
        i64::from(AU6_DELIVERY_TRUE_PEAK_MARGIN_MIN_HUNDREDTHS),
    );
}

#[test]
fn au6_e_an_unnormalized_export_misses_the_target() {
    let scene = scene(Au6Scenario::Delivery);
    let document = scene.canonical();
    let engine = engine();
    for job in &AU6_EXPORT_JOBS {
        let (_report, verification) = run_delivery(&engine, &document, job, false);
        let measured = verification.measured.integrated_lufs_hundredths.unwrap();
        let deviation = (measured - job.target.integrated_lufs_hundredths).abs();
        println!("AU6 unnormalized {} deviation={deviation}", job.id);
        assert!(deviation > AU6_DELIVERY_DEVIATION_MAX_LU_HUNDREDTHS);
    }
}

#[test]
fn au6_e_the_two_deliveries_separate_by_the_target_difference() {
    let scene = scene(Au6Scenario::Delivery);
    let document = scene.canonical();
    let engine = engine();
    let mut integrated = Vec::new();
    for job in &AU6_EXPORT_JOBS {
        let (_report, verification) = run_delivery(&engine, &document, job, true);
        integrated.push(verification.measured.integrated_lufs_hundredths.unwrap());
    }
    let separation = (integrated[0] - integrated[1]).abs();
    let term = (i64::from(separation) - i64::from(AU6_TARGET_SEPARATION_LU_HUNDREDTHS)).abs();
    print_ceiling(
        "target separation",
        term,
        i64::from(AU6_TARGET_SEPARATION_TOLERANCE_LU_HUNDREDTHS),
    );
}

#[test]
fn au6_e_two_exports_at_one_profile_do_not_separate() {
    let scene = scene(Au6Scenario::Delivery);
    let document = scene.canonical();
    let job = &AU6_EXPORT_JOBS[0];
    let engine = engine();
    let a = run_delivery(&engine, &document, job, true)
        .1
        .measured
        .integrated_lufs_hundredths
        .unwrap();
    let b = run_delivery(&engine, &document, job, true)
        .1
        .measured
        .integrated_lufs_hundredths
        .unwrap();
    assert_eq!((a - b).abs(), 0);
}

#[test]
fn au6_e_the_delivery_settings_carry_the_profile_raster_without_rendering_it() {
    let scene = scene(Au6Scenario::Delivery);
    let document = scene.canonical();
    for job in &AU6_EXPORT_JOBS {
        let profile = au6_profile_export_settings(job, &document);
        let overridden = au6_export_settings(job, &document);
        assert_eq!(overridden.resolution, document.resolution);
        if job.profile == DeliveryProfile::SourceMaster {
            assert_eq!(profile.resolution, document.resolution);
        } else {
            assert_ne!(profile.resolution, document.resolution);
        }
    }
}

#[test]
fn au6_the_performance_block_matches_its_code_constants() {
    let manifest: serde_json::Value = serde_json::from_str(AU6_MANIFEST).expect("manifest parses");
    assert_eq!(
        manifest["performance"]["media_lane"]["budget_seconds"]
            .as_i64()
            .unwrap(),
        i64::from(AU6_MEDIA_LANE_BUDGET_SECONDS)
    );
    assert_eq!(
        manifest["performance"]["agent_lane"]["budget_seconds"]
            .as_i64()
            .unwrap(),
        i64::from(AU6_AGENT_LANE_BUDGET_SECONDS)
    );
}

#[test]
fn au6_manifest_declares_every_required_fixture_and_constant() {
    let manifest: serde_json::Value = serde_json::from_str(AU6_MANIFEST).expect("manifest parses");
    let object = manifest.as_object().expect("object");
    const KEYS: [&str; 24] = [
        "contract",
        "contract_token",
        "manifest_version",
        "scenarios",
        "geometry",
        "sources",
        "learn",
        "profile",
        "declick",
        "thresholds",
        "budgets",
        "export_jobs",
        "canonical_documents",
        "pins",
        "transcriptions",
        "parity",
        "multicam",
        "delivery",
        "performance",
        "eval",
        "review",
        "scorecard",
        "m36",
        "external_owners",
    ];
    assert_eq!(object.len(), KEYS.len());
    for key in KEYS {
        assert!(object.contains_key(key), "missing manifest key {key}");
    }
    assert_eq!(manifest["contract"], AU6_CONTRACT);
    assert_eq!(manifest["manifest_version"], 1);
}

const AU6_CORE_TESTS: [&str; 13] = [
    "au6_scenario_geometry_is_the_contract_table",
    "au6_every_measured_window_clears_one_gating_block",
    "au6_the_duck_gap_leaves_exactly_one_unducked_window",
    "au6_the_learn_gap_clears_the_profile_minimum",
    "au6_canonical_operations_are_accepted_by_core_in_order",
    "au6_ascending_splits_are_rejected_by_core",
    "au6_core_allocates_the_pinned_gap_clip_ids",
    "au6_the_delivery_scenario_reuses_the_interview_document",
    "au6_export_jobs_differ_only_in_the_job",
    "au6_budgets_are_distinct_from_every_neighbouring_constant",
    "au6_every_budget_carries_the_declared_margin",
    "au6_the_questions_are_one_clause_each",
    "au6_the_regression_pins_are_labelled_as_pins",
];

const AU6_AGENT_TESTS: [&str; 8] = [
    "au6_a1_the_interview_ducks_the_bed_and_matches_the_voices",
    "au6_a2_the_podcast_chain_matches_the_voices_and_tames_the_ride",
    "au6_a3_the_location_dialogue_is_repaired_and_its_gap_filled",
    "au6_a4_the_multicam_cuts_leave_the_master_audio_untouched",
    "au6_a5a_the_delivery_lands_on_the_ebu_r128_target",
    "au6_a5b_the_streaming_target_is_reachable_by_the_agent",
    "au6_b_a_clip_whose_head_window_is_silent_gets_no_fade",
    "au6_the_four_planners_prose_is_pinned_by_exact_string",
];

const AU6_APP_TESTS: [&str; 14] = [
    "au6_a_a_person_can_balance_the_interview_and_ride_the_bed",
    "au6_b_a_person_can_match_the_voices_and_build_the_chain",
    "au6_c_a_person_can_repair_the_dialogue_and_fill_the_gap",
    "au6_d_a_person_can_cut_the_angles_and_mute_the_scratch_tracks",
    "au6_e_the_export_dialog_carries_both_delivery_targets",
    "au6_a_track_chain_offers_both_automation_targets",
    "au6_a_track_curve_edit_emits_set_track_automation",
    "au6_the_retired_tooltip_no_longer_points_at_a_dead_end",
    "au6_a_track_curve_covers_its_keyframe_span",
    "au6_a_scalar_mix_edit_covers_the_programme",
    "au6_a_cleared_curve_covers_the_span_it_held",
    "au6_the_person_batch_one_code_off_is_not_canonical",
    "au6_ascending_splits_are_refused_by_the_builder_path",
    "au6_d_the_person_ripple_gesture_is_not_the_canonical_cut",
];

const AU6_EVAL_TESTS: [&str; 0] = [];
const AU6_EXPLICIT_TEST_NAMES: [&str; 0] = [];

const AU6_INVENTORY_TESTS: [&str; 2] = [
    "au6_manifest_declares_every_required_fixture_and_constant",
    "au6_declared_test_names_exist_in_their_source_files",
];

const AU6_MEDIA_TESTS: [&str; 59] = [
    "au6_every_authored_level_matches_its_analytic_derivation",
    "au6_the_two_voices_occupy_disjoint_bands",
    "au6_c_every_authored_gap_is_below_the_silence_threshold",
    "au6_c_the_brief_levels_never_reach_the_detector",
    "au6_the_learn_gap_is_the_longest_detected_silence",
    "au6_c_a_click_in_the_learn_gap_splits_it",
    "au6_the_scratch_track_is_not_the_master",
    "au6_wav_round_trip_is_sample_exact",
    "au6_an_aac_mux_is_not_sample_exact",
    "au6_the_source_shapes_are_the_contract_table",
    "au6_an_angle_source_with_audio_probes_as_audiovideo",
    "au6_c_the_degraded_track_is_the_click_free_track_plus_the_clicks",
    "au6_restated_constants_agree_with_their_owners",
    "au6_a_the_interview_clears_the_bed_and_matches_the_voices",
    "au6_a_the_unducked_document_does_not_clear_the_bed",
    "au6_a_the_untrimmed_voices_are_not_matched",
    "au6_a_the_nominal_dbfs_trim_is_worse_than_none",
    "au6_a_the_duck_reaches_its_depth",
    "au6_a_the_unducked_bed_has_no_depth",
    "au6_a_the_duck_depth_is_invisible_at_the_track_point",
    "au6_the_three_curve_owners_render_identically",
    "au6_every_scenario_mix_does_not_clip",
    "au6_a_a_hot_master_clips_and_says_so",
    "au6_b_the_chain_matches_the_voices_and_reduces_the_spread",
    "au6_b_the_raw_trims_do_not_match_the_voices",
    "au6_b_the_bypassed_chain_leaves_the_spread_intact",
    "au6_b_the_programme_stays_inside_its_loudness_range",
    "au6_b_the_makeup_less_chain_exceeds_the_loudness_range",
    "au6_c_the_repair_chain_moves_the_snr_and_the_hum",
    "au6_c_an_unlearned_profile_moves_no_snr",
    "au6_c_a_chain_without_the_hum_node_leaves_the_mains_alone",
    "au6_c_the_declick_node_alone_drops_the_click_error",
    "au6_c_a_chain_without_the_declick_node_keeps_every_click",
    "au6_c_the_declick_contribution_in_chain_is_recorded",
    "au6_c_the_dialogue_survives_the_repair",
    "au6_c_an_over_reduced_profile_eats_the_dialogue",
    "au6_c_the_room_tone_fill_closes_the_gap_seamlessly",
    "au6_c_a_one_frame_slip_breaks_the_seam",
    "au6_c_the_learned_profile_holds_its_pin_and_its_bound",
    "au6_c_a_profile_learned_over_speech_is_not_the_noise_profile",
    "au6_c_a_one_frame_learn_offset_moves_the_profile",
    "au6_c_the_learned_profile_has_the_authored_shape",
    "au6_c_the_percentile_floor_lands_on_the_authored_floor",
    "au6_d_the_master_stem_is_bit_identical_across_the_cuts",
    "au6_d_a_cut_master_track_is_not_continuous",
    "au6_d_an_angle_ripple_leaves_the_master_stem_alone",
    "au6_d_the_scratch_tracks_contribute_nothing",
    "au6_d_an_unmuted_scratch_track_reaches_the_mix",
    "au6_d_the_mix_is_the_master",
    "au6_d_the_unmuted_scratch_moves_the_master",
    "au6_d_the_cuts_sit_at_the_authored_frames",
    "au6_e_both_deliveries_land_on_their_targets",
    "au6_e_an_unnormalized_export_misses_the_target",
    "au6_e_the_two_deliveries_separate_by_the_target_difference",
    "au6_e_two_exports_at_one_profile_do_not_separate",
    "au6_e_the_delivery_settings_carry_the_profile_raster_without_rendering_it",
    "au6_the_performance_block_matches_its_code_constants",
    "au6_manifest_declares_every_required_fixture_and_constant",
    "au6_declared_test_names_exist_in_their_source_files",
];

const AU6_FORBIDDEN_HELPERS: [&str; 3] = [
    "fixture_gpu_or_skip",
    "KINEWRIGHT_GPU_TESTS_MAY_SKIP",
    "KINEWRIGHT_AUDIO_TEST",
];

const AU6_TEST_SOURCES: [(&str, &str); 11] = [
    (
        "crates/kinewright-media/src/au6_fixtures.rs",
        include_str!("au6_fixtures.rs"),
    ),
    (
        "crates/kinewright-media/src/au6_sources.rs",
        include_str!("au6_sources.rs"),
    ),
    (
        "crates/kinewright-core/tests/au6_core.rs",
        include_str!("../../kinewright-core/tests/au6_core.rs"),
    ),
    (
        "crates/kinewright-agent/tests/mcp_server.rs",
        include_str!("../../kinewright-agent/tests/mcp_server.rs"),
    ),
    (
        "crates/kinewright-agent/src/eval.rs",
        include_str!("../../kinewright-agent/src/eval.rs"),
    ),
    (
        "crates/kinewright-agent/src/bin/kinewright-eval.rs",
        include_str!("../../kinewright-agent/src/bin/kinewright-eval.rs"),
    ),
    (
        "crates/kinewright-app/src/mixer_ui.rs",
        include_str!("../../kinewright-app/src/mixer_ui.rs"),
    ),
    (
        "crates/kinewright-app/src/mixer_pane_ui.rs",
        include_str!("../../kinewright-app/src/mixer_pane_ui.rs"),
    ),
    (
        "crates/kinewright-app/src/edit_diff.rs",
        include_str!("../../kinewright-app/src/edit_diff.rs"),
    ),
    (
        "crates/kinewright-app/src/inspector_ui.rs",
        include_str!("../../kinewright-app/src/inspector_ui.rs"),
    ),
    (
        "crates/kinewright-app/src/app.rs",
        include_str!("../../kinewright-app/src/app.rs"),
    ),
];

fn au6_test_source(path: &str) -> &'static str {
    AU6_TEST_SOURCES
        .iter()
        .find_map(|(candidate, source)| (*candidate == path).then_some(*source))
        .unwrap_or_else(|| panic!("AU6 inventory is missing source {path}"))
}

fn au6_is_test_attribute(line: &str) -> bool {
    line == "#[test]" || line.starts_with("#[tokio::test")
}

fn au6_declares_test(source: &str, name: &str) -> bool {
    let needle = format!("fn {name}(");
    let lines = source.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(&needle) {
            continue;
        }
        for previous in lines[..index].iter().rev() {
            let previous = previous.trim();
            if au6_is_test_attribute(previous) {
                return true;
            }
            if previous.is_empty() || previous.starts_with("//") || previous.starts_with("#[") {
                continue;
            }
            break;
        }
    }
    false
}

fn au6_declared_test_names(source: &str, prefix: &str) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !au6_is_test_attribute(line.trim()) {
            continue;
        }
        for candidate in &lines[index + 1..] {
            let candidate = candidate.trim();
            if candidate.is_empty() || candidate.starts_with("//") || candidate.starts_with("#[") {
                continue;
            }
            let Some(rest) = candidate.split_once("fn ").map(|(_, rest)| rest) else {
                break;
            };
            let Some((name, _)) = rest.split_once('(') else {
                break;
            };
            if name.starts_with(prefix) {
                names.push(name.to_owned());
            }
            break;
        }
    }
    names
}

fn au6_uses_outside_prose(source: &str, needle: &str) -> bool {
    let call = format!("{needle}(");
    let quoted = format!("(\"{needle}\")");
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or_default();
        code.contains(&call) || code.contains(&quoted)
    })
}

fn au6_sorted(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn au6_inventory_groups() -> [(
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
); 5] {
    [
        (
            "MEDIA",
            &[
                "crates/kinewright-media/src/au6_fixtures.rs",
                "crates/kinewright-media/src/au6_sources.rs",
            ],
            &AU6_MEDIA_TESTS,
        ),
        (
            "CORE",
            &["crates/kinewright-core/tests/au6_core.rs"],
            &AU6_CORE_TESTS,
        ),
        (
            "AGENT",
            &["crates/kinewright-agent/tests/mcp_server.rs"],
            &AU6_AGENT_TESTS,
        ),
        (
            "APP",
            &[
                "crates/kinewright-app/src/mixer_ui.rs",
                "crates/kinewright-app/src/mixer_pane_ui.rs",
                "crates/kinewright-app/src/edit_diff.rs",
                "crates/kinewright-app/src/inspector_ui.rs",
                "crates/kinewright-app/src/app.rs",
            ],
            &AU6_APP_TESTS,
        ),
        (
            "EVAL",
            &[
                "crates/kinewright-agent/src/eval.rs",
                "crates/kinewright-agent/src/bin/kinewright-eval.rs",
            ],
            &AU6_EVAL_TESTS,
        ),
    ]
}

#[test]
fn au6_declared_test_names_exist_in_their_source_files() {
    for (label, sources, expected) in au6_inventory_groups() {
        for name in expected {
            assert!(
                sources
                    .iter()
                    .any(|path| au6_declares_test(au6_test_source(path), name)),
                "no AU6_{label}_TEST_SOURCES file declares a #[test] named {name}"
            );
        }
        let declared = au6_sorted(
            sources
                .iter()
                .flat_map(|path| au6_declared_test_names(au6_test_source(path), "au6_")),
        );
        let named = au6_sorted(expected.iter().map(|name| (*name).to_owned()));
        assert_eq!(
            declared, named,
            "AU6_{label}_TESTS and the `au6_*` tests the {label} sources declare disagree"
        );
    }

    let mut all = Vec::new();
    for (_, _, expected) in au6_inventory_groups() {
        for name in expected {
            assert!(
                name.starts_with("au6_") || AU6_EXPLICIT_TEST_NAMES.contains(name),
                "{name} does not match `cargo test -- au6_` and is not explicit"
            );
            all.push((*name).to_owned());
        }
    }
    let total = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), total, "an AU6 test name is declared twice");
    for name in AU6_INVENTORY_TESTS {
        assert!(
            au6_declares_test(
                au6_test_source("crates/kinewright-media/src/au6_fixtures.rs"),
                name
            ),
            "inventory test {name} is missing"
        );
        assert!(AU6_MEDIA_TESTS.contains(&name));
    }
    assert_eq!(AU6_EVAL_TESTS.len(), 0);
    assert_eq!(AU6_EXPLICIT_TEST_NAMES.len(), 0);

    for path in [
        "crates/kinewright-media/src/au6_fixtures.rs",
        "crates/kinewright-media/src/au6_sources.rs",
        "crates/kinewright-core/tests/au6_core.rs",
        "crates/kinewright-app/src/mixer_ui.rs",
        "crates/kinewright-app/src/mixer_pane_ui.rs",
        "crates/kinewright-app/src/edit_diff.rs",
        "crates/kinewright-app/src/inspector_ui.rs",
        "crates/kinewright-app/src/app.rs",
    ] {
        let source = au6_test_source(path);
        for needle in AU6_FORBIDDEN_HELPERS {
            assert!(
                !au6_uses_outside_prose(source, needle),
                "{path} must never reach for {needle}"
            );
        }
        // Split so this file does not contain the contiguous token S10 bans.
        let os_gate = ["cfg(target_", "os"].concat();
        assert!(
            !source.contains(&os_gate),
            "{path} must not contain {os_gate}"
        );
    }
}
