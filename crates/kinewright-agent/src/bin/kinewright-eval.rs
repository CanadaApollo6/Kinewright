use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use kinewright_agent::{
    ClaudeCodeDriver, CodexDriver, CursorAcpDriver,
    eval::{
        BLIND_DIRECTORY_NAME, BLIND_FORM_FILE_NAME, BLIND_KEY_FILE_NAME, BLIND_SCHEMA_VERSION,
        BlindKeyEntry, BlindKeyFile, BlindReviewForm, COLOR_WORKFLOW_BENCHMARK_ID,
        ColorEvalRequest, EnvironmentStamp, EvalAssertion, EvalAudioTailSpec, EvalBudgets,
        EvalDefinition, EvalDeliverableSpec, EvalError, EvalLoudnessSpec, EvalResult,
        ExpectedSourceClip, ExpectedTimelineClip, FixtureContext, HumanQuestion, HumanReviewFile,
        PreparedFixture, SourceRangeExclusion, blind_review_form,
        human_review_template_with_questions, maximum_duration_after_expected_silence_cuts,
        render_jsonl, render_saved_deliverable, render_scoreboard, result_path, run_eval,
        run_eval_with_artifacts, summarize_human_review,
    },
    fixture_pack::{FixturePackManifest, fixture_cache_root},
};
use kinewright_core::{
    AgentDriver, Analysis, AssetBeats, AssetId, AssetSceneChanges, AssetSilences, AssetTranscript,
    AuthenticationStatus, BeatStatus, CaptionMotion, Clip, ClipContent, ClipId,
    DeliveryEncodeDepth, DeliveryProfile, Document, FrameRounding, MediaAsset, MediaCatalog,
    NormalizedRoi, Rational, SceneStatus, SilenceStatus, SyncGroup, SyncGroupId, SyncGroupMember,
    ThreePointMode, TimeCode, TitlePosition, Track, TrackId, TrackKind, TranscriptStatus,
    TranscriptWord,
    cc7_scenarios::{
        CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS, CC7_CANDIDATE_CLIP_ID, CC7_CHART_BAND_ROI,
        CC7_CHART_PATCHES, CC7_DEEP_SHADOW_ROI, CC7_F_KEYFRAMED_PARAMETERS, CC7_LOG_CUBE_SIZE,
        CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE, CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX,
        CC7_NODE_EFFECT_ID, CC7_PRODUCT_PATCH_PIXEL_COUNT, CC7_PRODUCT_RED_ROI,
        CC7_REFERENCE_CLIP_ID, CC7_SCENARIOS, CC7_SINGLE_CLIP_ID, CC7_SKIN_BAND_ROI,
        CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS, CC7_SOURCE_FPS, CC7_SOURCE_FRAMES, CC7_SOURCE_HEIGHT,
        CC7_SOURCE_WIDTH, CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES,
        CC7_TRACK_SURVIVING_SAMPLE_FRAMES, Cc7Camera, cc7_spec,
    },
    map_frames_with_rounding, map_source_range_to_project,
};
use kinewright_media::{
    FfmpegMediaEngine,
    cc7_sources::{
        Cc7SourceKind, cc7_camera_source, cc7_log_source, cc7_tracked_source,
        write_log_like_inverse_cube,
    },
    test_support::{
        GeneratedMedia, SpeechClip, TempDirectory, joined_words, normalized_words, test_engine,
    },
};
use serde::Deserialize;

const FPS: u32 = 30;
const LONG_SILENCE_FRAMES: i64 = 20;
const SCENE_CONFIDENCE_BASIS_POINTS: u16 = 1_000;
const FILLER_WORDS: &[&str] = &["um", "uh", "erm", "er"];
const MUSIC_SOURCE_BEAT_SET: &str = "music-bed-source-beats";
const MUSIC_PROJECT_BEAT_SET: &str = "music-bed-project-beats";
const MUSIC_REVIEWED_EVENT_SET: &str = "music-bed-reviewed-events";
const MUSIC_SOURCE_SCENE_SET: &str = "montage-source-scenes";
const MUSIC_SOURCE_EXCLUSION_SET: &str = "montage-source-exclusions";
const MUSIC_MONTAGE_PROMPT_V10: &str = r"Create a finished 18-second 1080p YouTube trailer edit using Tears of Steel as the only photographed source and Vanguard on music-bed as the only audio. Build one chronological action arc: establish the tower-scale mechanical threat, answer it with the workshop team's preparation, reveal the operator briefly, bridge into the machine activating, drive into the strongest battle, then end every photographed image at frame 388 on a purpose-built TEARS OF STEEL title card for the musical decay.

Open exactly these ten capability schemas in one get_capability call: get_source_shot_board, plan_music_fit, get_music_structure, plan_beat_montage, split_clip, replace_clip, add_title, plan_audio_normalization, get_cut_neighborhoods, and get_editorial_readiness. Do not call get_source_storyboard. Call get_source_shot_board exactly once over the full Tears of Steel source with candidate_selection coverage, minimum_duration_frames 30, minimum_confidence_basis_points 1000, candidate_count 12, and max_width 160. Treat the board as scouting evidence and never cross a returned scene boundary.

First call plan_music_fit on audio track 2 with music-bed, project range 0..450, preferred source start 6334, preferred source end 6875, maximum end drift 2 frames, minimum strength 10 percent, and overwrite mode. Inspect and commit that endpoint-anchored plan. Keep it unchanged as the sole audio: no loop, retime, duplication, source-video audio, or later trim. Call get_music_structure over 0..450 with minimum strength 10 percent, meter 4, 4 bars per phrase, and structural_only=false. Use frames 48, 126, and 263 as the three exact musical cut anchors. Frame 203 is deliberately a story cut, not a detected beat: it shortens the operator once the preparation is readable and reveals the activation before the climax.

Build the initial photographed montage only over frames 0..388. Call plan_beat_montage on video track 1 with exactly four ordered selects and preferred anchors [48,126,263], shot bounds 40..140, minimum beat strength 10 percent, overwrite mode, cadence {minimum_duration_buckets:3, duration_bucket_frames:15, maximum_similar_run:3, similar_tolerance_frames:6}, and anchor repair with maximum_movement_frames 0 and locked_anchor_indices [0,1,2]. Select one threat shot inside source window 165..221, one preparation shot inside 221..309, one operator shot inside 482..635, and one sustained battle shot inside 987..1118. The initial durations must resolve to 48, 78, 137, and 125 project frames. Inspect and commit the montage.

Then split the operator clip exactly at project frame 203. Reinspect the timeline to identify the new right-hand clip covering 203..263 and replace that clip with the exact activation select source 789..847 from Tears of Steel. This ordinary story edit is the reviewed exception to beat alignment; do not move frame 203 to a beat. The final five photographed shots must cover 0..388 with durations 48, 78, 77, 60, and 125 frames and move strictly forward through threat, preparation, operator, activation, and battle.

At project frame 388 add one title clip on video track 1 for exactly 62 frames. Its text must be TEARS OF STEEL, font_size_token 2, color_token 0, position center, background_scrim false, fade_in_frames 5, fade_out_frames 15, and caption_preset null. The empty compositor background supplies black. Do not add a transition, subtitle, freeze frame, fallen-robot image, aftermath source clip, or any photographed media after frame 388. The title card is the resolution and must end at frame 450.

Normalize only audio track 2 to -1600 LUFS hundredths with a -100 dBFS-hundredths sample-peak ceiling and 100-hundredths tolerance. Add no captions, source-video audio, visual effects, transitions, freeze frames, or retiming. Call get_cut_neighborhoods on video track 1 with frames_before 1, frames_after 3, cut_offset 0, cut_count 12, maximum_secondary_change_basis_points 1200, and max_width 160. Review the four photographed cut edges at 48, 126, 203, and 263; a CUT EDGE REVIEW FAILED result is blocking. Finally call get_editorial_readiness using youtube_1080p, check_silence=false, centered 50/50 focus, 10 storyboard frames, and 160-pixel cells. Confirm the whole sheet shows the chronological arc, no action after frame 388, and the centered title resolving to black. Do not queue export; the benchmark renders the verified snapshot. Keep working until readiness is true.";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("kinewright-eval: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool, EvalError> {
    let options = Options::parse(env::args().skip(1))?;
    if let Some(manifest_path) = &options.prepare_fixtures {
        let manifest = FixturePackManifest::load(manifest_path)
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        let report = manifest
            .prepare(&fixture_cache_root())
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        println!(
            "Prepared fixture pack {} at {} (downloaded={}, already_present={})",
            report.pack_id,
            report.cache_root.display(),
            report.downloaded.len(),
            report.already_present.len()
        );
        return Ok(true);
    }
    if let Some(manifest_path) = &options.verify_fixtures {
        let manifest = FixturePackManifest::load(manifest_path)
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        let report = manifest
            .verify(&fixture_cache_root())
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        println!(
            "Verified fixture pack {} at {} (assets={})",
            report.pack_id,
            report.cache_root.display(),
            report.already_present.len()
        );
        return Ok(true);
    }
    if let Some(directory) = &options.write_color_smoke_media {
        return write_color_smoke_media(directory).map(|()| true);
    }
    if let Some(review_path) = &options.score_review {
        return score_review_file(review_path).map(|()| true);
    }
    if let Some(document_path) = &options.rerender_document {
        return rerender_document(document_path, &options);
    }
    run_subscription_suite(&options)
}

/// The files `--write-color-smoke-media` materializes, in order: the file name
/// and the CC7 source it is cut from. Names are stable so the smoke-test
/// document can refer to them.
const COLOR_SMOKE_MEDIA: [(&str, Cc7SourceKind); 6] = [
    ("cc7-camera-a.mkv", Cc7SourceKind::Camera(Cc7Camera::A)),
    ("cc7-camera-b.mkv", Cc7SourceKind::Camera(Cc7Camera::B)),
    ("cc7-camera-c1.mkv", Cc7SourceKind::Camera(Cc7Camera::C1)),
    ("cc7-camera-c2.mkv", Cc7SourceKind::Camera(Cc7Camera::C2)),
    ("cc7-log-carrier.mkv", Cc7SourceKind::Log),
    ("cc7-tracked.mkv", Cc7SourceKind::Tracked),
];

/// Write every CC7 synthetic source plus the 65³ inverse `.cube` into
/// `directory`, so a person can open the same known-answer footage the
/// technical gates run on. The generators author each raster in Rust and mux
/// it FFV1 lossless (CC7 §3.2); nothing here depends on a model or a network.
fn write_color_smoke_media(directory: &Path) -> Result<(), EvalError> {
    let io = |error: std::io::Error| EvalError::Fixture(error.to_string());
    fs::create_dir_all(directory).map_err(io)?;
    for (name, kind) in COLOR_SMOKE_MEDIA {
        let generated = match kind {
            Cc7SourceKind::Camera(camera) => cc7_camera_source(camera),
            Cc7SourceKind::Log => cc7_log_source(),
            Cc7SourceKind::Tracked => cc7_tracked_source(),
        };
        let destination = directory.join(name);
        fs::copy(generated.path(), &destination).map_err(io)?;
        println!("{}", destination.display());
    }
    let cube = write_log_like_inverse_cube(directory, CC7_LOG_CUBE_SIZE);
    println!("{}", cube.display());
    println!(
        "{} × {} at {} fps, {} frames per camera; see docs/COLOR-SMOKE-TEST.md",
        CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT, CC7_SOURCE_FPS, CC7_SOURCE_FRAMES
    );
    Ok(())
}

fn run_subscription_suite(options: &Options) -> Result<bool, EvalError> {
    if env::var("KINEWRIGHT_EVAL").as_deref() != Ok("1") {
        return Err(EvalError::Agent(
            "refusing to run a subscription eval; set KINEWRIGHT_EVAL=1 explicitly".to_owned(),
        ));
    }
    let driver = eval_driver(&options.harness)?;
    let harness_info = driver
        .detect()
        .ok_or_else(|| EvalError::Agent(format!("{} is not installed", driver.id().0)))?;
    if harness_info.authentication == AuthenticationStatus::Unauthenticated {
        return Err(EvalError::Agent(format!(
            "{} is installed but not authenticated",
            driver.id().0
        )));
    }
    let environment = EnvironmentStamp::capture(
        Some(&harness_info),
        &driver.id().0,
        options.model.as_deref(),
    );
    let (benchmark_id, definitions) = eval_suite(&options.suite)?;
    let definitions = filter_definitions(definitions, options.only.as_deref())?;
    let working_directory = env::current_dir().ok();
    let packaged_run = is_packaged_benchmark(benchmark_id);
    let run_id = run_identifier(&environment);
    let run_directory = Path::new("target/evals").join(&run_id);
    let total_runs = definitions.len() * options.samples as usize;
    let mut results = Vec::with_capacity(total_runs);
    let mut run_number = 0_usize;
    for definition in &definitions {
        for sample in 0..options.samples {
            run_number += 1;
            println!(
                "\n[{run_number}/{total_runs}] {} (sample {}/{}) - {}",
                definition.name,
                sample + 1,
                options.samples,
                definition.rationale
            );
            let artifact_directory = packaged_run.then(|| {
                run_directory.join("artifacts").join(format!(
                    "{}-sample-{}",
                    definition.name.split_whitespace().next().unwrap_or("task"),
                    sample + 1
                ))
            });
            let result = if packaged_run {
                run_eval_with_artifacts(
                    definition,
                    driver.as_ref(),
                    options.model.as_deref(),
                    working_directory.as_deref(),
                    artifact_directory.as_deref(),
                )
            } else {
                run_eval(
                    definition,
                    driver.as_ref(),
                    options.model.as_deref(),
                    working_directory.as_deref(),
                )
            }
            .unwrap_or_else(|error| EvalResult::execution_failure(definition, &error));
            print_result_details(&result);
            results.push(result);
        }
    }

    let scoreboard = render_scoreboard(&results);
    println!("\n{scoreboard}");
    if options.samples > 1 {
        print_pass_rates(&definitions, &results, options.samples);
    }
    let output_path = persist_results(
        options,
        benchmark_id,
        &run_id,
        &run_directory,
        &environment,
        &definitions,
        &results,
    )?;
    println!("JSONL: {}", output_path.display());
    Ok(results.iter().all(|result| result.passed))
}

fn eval_driver(harness: &str) -> Result<Box<dyn AgentDriver>, EvalError> {
    match harness {
        "claude" | "claude-code" => Ok(Box::new(ClaudeCodeDriver)),
        "codex" => Ok(Box::new(CodexDriver)),
        "cursor" => Ok(Box::new(CursorAcpDriver)),
        other => Err(EvalError::Agent(format!(
            "unknown harness {other:?}; expected claude-code, codex, or cursor"
        ))),
    }
}

fn eval_suite(suite: &str) -> Result<(&'static str, Vec<EvalDefinition>), EvalError> {
    match suite {
        "auto-edit-v1" | "v1" => Ok(("kinewright-auto-edit-v1", seed_suite())),
        "finished-cut-v2" | "v2" => Ok(("kinewright-finished-cut-v2", finished_cut_suite())),
        "editorial-cut-v3" | "v3" => Ok(("kinewright-editorial-cut-v3", editorial_cut_suite())),
        "dialogue-pacing-v4" | "v4" => {
            Ok(("kinewright-dialogue-pacing-v4", dialogue_pacing_suite()))
        }
        "generalization-v5" | "v5" => Ok(("kinewright-generalization-v5", generalization_suite())),
        "color-workflow-v6" | "v6" => Ok((COLOR_WORKFLOW_BENCHMARK_ID, color_workflow_suite())),
        other => Err(EvalError::Agent(format!(
            "unknown suite {other:?}; expected auto-edit-v1, finished-cut-v2, editorial-cut-v3, dialogue-pacing-v4, generalization-v5, or color-workflow-v6"
        ))),
    }
}

/// v1 is deliberately excluded: it is the only suite whose run may rewrite
/// `docs/EVALS.md`. A new packaged suite that is forgotten here produces no
/// artifacts, no review package, and overwrites that document.
fn is_packaged_benchmark(benchmark_id: &str) -> bool {
    matches!(
        benchmark_id,
        "kinewright-finished-cut-v2"
            | "kinewright-editorial-cut-v3"
            | "kinewright-dialogue-pacing-v4"
            | "kinewright-generalization-v5"
            | COLOR_WORKFLOW_BENCHMARK_ID
    )
}

fn filter_definitions(
    definitions: Vec<EvalDefinition>,
    only: Option<&str>,
) -> Result<Vec<EvalDefinition>, EvalError> {
    let Some(name) = only else {
        return Ok(definitions);
    };
    let filtered = definitions
        .into_iter()
        .filter(|definition| {
            definition.name == name || definition.name.split_whitespace().next() == Some(name)
        })
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        return Err(EvalError::Agent(format!(
            "--only {name:?} matched no eval in the suite"
        )));
    }
    Ok(filtered)
}

fn persist_results(
    options: &Options,
    benchmark_id: &str,
    run_id: &str,
    run_directory: &Path,
    environment: &EnvironmentStamp,
    definitions: &[EvalDefinition],
    results: &[EvalResult],
) -> Result<PathBuf, EvalError> {
    let output_root = Path::new("target/evals");
    fs::create_dir_all(output_root).map_err(|error| EvalError::Output(error.to_string()))?;
    let packaged_run = is_packaged_benchmark(benchmark_id);
    let output_path = if packaged_run {
        fs::create_dir_all(run_directory).map_err(|error| EvalError::Output(error.to_string()))?;
        run_directory.join("results.jsonl")
    } else {
        result_path(output_root, environment)
    };
    fs::write(&output_path, render_jsonl(environment, results)?)
        .map_err(|error| EvalError::Output(error.to_string()))?;
    // A filtered or multi-sample run is a measurement exercise, not a new
    // baseline: docs/EVALS.md only records complete single-pass suites.
    if !packaged_run && options.only.is_none() && options.samples == 1 {
        let docs = render_evals_document(definitions, environment, results, &output_path);
        fs::write("docs/EVALS.md", docs).map_err(|error| EvalError::Output(error.to_string()))?;
        println!("Docs: docs/EVALS.md");
    }
    if packaged_run {
        write_review_package(benchmark_id, run_id, run_directory, environment, results)?;
    }
    Ok(output_path)
}

fn write_review_package(
    benchmark_id: &str,
    run_id: &str,
    run_directory: &Path,
    environment: &EnvironmentStamp,
    results: &[EvalResult],
) -> Result<(), EvalError> {
    let review = human_review_template_with_questions(
        benchmark_id,
        run_id,
        results,
        &review_questions(benchmark_id),
    );
    let review_path = run_directory.join("human-review.json");
    let review_json =
        serde_json::to_vec_pretty(&review).map_err(|error| EvalError::Output(error.to_string()))?;
    fs::write(&review_path, review_json).map_err(|error| EvalError::Output(error.to_string()))?;
    let machine_report = serde_json::json!({
        "schema_version": 1,
        "benchmark_id": benchmark_id,
        "run_id": run_id,
        "environment": environment,
        "machine_passed": results.iter().all(|result| result.passed),
        "results": results,
        "human_review": {
            "status": "pending",
            "template": review_path,
        },
    });
    fs::write(
        run_directory.join("machine-report.json"),
        serde_json::to_vec_pretty(&machine_report)
            .map_err(|error| EvalError::Output(error.to_string()))?,
    )
    .map_err(|error| EvalError::Output(error.to_string()))?;
    // Every packaged run gets a blind package. There is no flag: a review
    // surface that has to be asked for is one that will not be there when it
    // matters, and the unblinded files stay exactly where they were.
    let blind_directory =
        write_blind_package(benchmark_id, run_id, run_directory, &review, results)?;
    println!("Review: {}", review_path.display());
    println!("Blind: {}", blind_directory.display());
    println!("Package: {}", run_directory.display());
    Ok(())
}

/// The creative questions this benchmark puts to a reviewer, by base task id.
///
/// The colour suite returns its scenarios' `human_question` entries, keyed
/// `c1`..`c6`, so the blind form carries the matrix's question verbatim and
/// nothing else. Scenario (c) has none — it is objective-only and the matrix
/// has no row for log-like input — so `c3` contributes no entry at all rather
/// than an empty one. Every other suite asks none, which is why the map is
/// empty for them.
fn review_questions(benchmark_id: &str) -> BTreeMap<String, Vec<HumanQuestion>> {
    let mut questions = BTreeMap::new();
    if benchmark_id != COLOR_WORKFLOW_BENCHMARK_ID {
        return questions;
    }
    for (index, scenario) in CC7_SCENARIOS.into_iter().enumerate() {
        let spec = cc7_spec(scenario);
        let Some(prompt) = spec.human_question else {
            continue;
        };
        questions.insert(
            format!("c{}", index + 1),
            vec![HumanQuestion {
                id: spec.id.to_owned(),
                prompt: prompt.to_owned(),
                answer: None,
                notes: None,
            }],
        );
    }
    questions
}

/// One packaged result, paired with the task id the review template gives it.
struct PackagedTask<'a> {
    task_id: String,
    sample: u32,
    deliverable: &'a kinewright_agent::eval::EvalDeliverableResult,
}

/// Rebuild the review template's task ids from the results, in the same order
/// and by the same rule, so the key cannot drift from the review.
fn packaged_tasks(results: &[EvalResult]) -> Vec<PackagedTask<'_>> {
    let mut occurrences = BTreeMap::<&str, u32>::new();
    let mut tasks = Vec::new();
    for result in results {
        let Some(deliverable) = result.deliverable.as_ref() else {
            continue;
        };
        let base_task_id = result
            .name
            .split_whitespace()
            .next()
            .unwrap_or(&result.name);
        let occurrence = occurrences.entry(base_task_id).or_default();
        *occurrence = occurrence.saturating_add(1);
        let task_id = if *occurrence == 1 {
            base_task_id.to_owned()
        } else {
            format!("{base_task_id}-sample-{occurrence}")
        };
        tasks.push(PackagedTask {
            task_id,
            sample: *occurrence,
            deliverable,
        });
    }
    tasks
}

/// Write `blind/<blind_id>.{png,mp4}`, `blind/review-form.json`, and the key.
///
/// The media files are byte copies, so the originals under `artifacts/` are
/// untouched and `blind/` can be handed over whole. `blind/` contains those
/// files and nothing else: no README, no subdirectory, no sample index — and
/// the key is written to the run root, deliberately outside it.
fn write_blind_package(
    benchmark_id: &str,
    run_id: &str,
    run_directory: &Path,
    review: &HumanReviewFile,
    results: &[EvalResult],
) -> Result<PathBuf, EvalError> {
    let blind_directory = run_directory.join(BLIND_DIRECTORY_NAME);
    fs::create_dir_all(&blind_directory).map_err(|error| {
        EvalError::Output(format!(
            "could not create {}: {error}",
            blind_directory.display()
        ))
    })?;
    let packaged = packaged_tasks(results);
    let mut key_entries = Vec::new();
    for task in &review.tasks {
        let (Some(blind_id), Some(artifact_sha256)) =
            (task.blind_id.as_ref(), task.artifact_sha256.as_ref())
        else {
            continue;
        };
        let Some(packaged) = packaged
            .iter()
            .find(|candidate| candidate.task_id == task.task_id)
        else {
            continue;
        };
        copy_blind_artifact(&blind_directory, blind_id, &packaged.deliverable.proof_path)?;
        copy_blind_artifact(
            &blind_directory,
            blind_id,
            &packaged.deliverable.output_path,
        )?;
        key_entries.push(BlindKeyEntry {
            blind_id: blind_id.clone(),
            task_id: task.task_id.clone(),
            sample: packaged.sample,
            artifact_sha256: artifact_sha256.clone(),
            artifact_path: packaged.deliverable.output_path.display().to_string(),
        });
    }
    // The key is NOT deduplicated: two tasks whose artefacts are
    // byte-identical share one blind identifier, and both mappings must
    // survive so one viewing scores both rows.
    key_entries.sort_by(|left, right| {
        left.blind_id
            .cmp(&right.blind_id)
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    write_json(
        &blind_directory.join(BLIND_FORM_FILE_NAME),
        &blind_review_form(review),
    )?;
    write_json(
        &run_directory.join(BLIND_KEY_FILE_NAME),
        &BlindKeyFile {
            schema_version: BLIND_SCHEMA_VERSION,
            benchmark_id: benchmark_id.to_owned(),
            run_id: run_id.to_owned(),
            entries: key_entries,
        },
    )?;
    Ok(blind_directory)
}

/// Copy one artefact under its blind name, keeping its own extension.
///
/// The blind id is the **encode**'s digest, so two tasks with byte-identical
/// encodes share one name deliberately — that is how one viewing scores two
/// rows. Their proof sheets are then written to the same `.png`, which is only
/// safe while the two proofs are themselves identical. A destination that
/// already exists with different bytes would silently make one proof stand for
/// two rows, so it is a typed refusal rather than an overwrite.
fn copy_blind_artifact(
    blind_directory: &Path,
    blind_id: &str,
    source: &Path,
) -> Result<(), EvalError> {
    if !source.exists() {
        return Ok(());
    }
    let Some(extension) = source.extension().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    let destination = blind_directory.join(format!("{blind_id}.{extension}"));
    if destination.exists() {
        let (existing, incoming) = (fs::read(&destination), fs::read(source));
        if let (Ok(existing), Ok(incoming)) = (existing, incoming) {
            if existing != incoming {
                return Err(EvalError::Output(format!(
                    "two artefacts collide on blind id {blind_id}: {} would overwrite {} with different bytes",
                    source.display(),
                    destination.display()
                )));
            }
            return Ok(());
        }
    }
    fs::copy(source, &destination).map_err(|error| {
        EvalError::Output(format!(
            "could not copy {} to {}: {error}",
            source.display(),
            destination.display()
        ))
    })?;
    Ok(())
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), EvalError> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| EvalError::Output(error.to_string()))?;
    fs::write(path, bytes)
        .map_err(|error| EvalError::Output(format!("could not write {}: {error}", path.display())))
}

struct Options {
    harness: String,
    model: Option<String>,
    only: Option<String>,
    samples: u32,
    suite: String,
    score_review: Option<PathBuf>,
    prepare_fixtures: Option<PathBuf>,
    verify_fixtures: Option<PathBuf>,
    rerender_document: Option<PathBuf>,
    artifact_directory: Option<PathBuf>,
    delivery_profile: DeliveryProfile,
    loudness_contract: Option<EvalLoudnessSpec>,
    audio_tail_contract: Option<EvalAudioTailSpec>,
    /// Write the CC7 synthetic colour scenarios to a directory for a hands-on
    /// smoke test (`docs/COLOR-SMOKE-TEST.md`).
    write_color_smoke_media: Option<PathBuf>,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, EvalError> {
        let mut options = Self::defaults();
        let mut arguments = arguments;
        while let Some(argument) = arguments.next() {
            options.parse_argument(&argument, &mut arguments)?;
        }
        options.validate()?;
        Ok(options)
    }

    fn defaults() -> Self {
        Self {
            harness: "claude-code".to_owned(),
            model: None,
            only: None,
            samples: 1,
            suite: "auto-edit-v1".to_owned(),
            score_review: None,
            prepare_fixtures: None,
            verify_fixtures: None,
            rerender_document: None,
            artifact_directory: None,
            delivery_profile: DeliveryProfile::VerticalShort,
            loudness_contract: None,
            audio_tail_contract: None,
            write_color_smoke_media: None,
        }
    }

    fn parse_argument(
        &mut self,
        argument: &str,
        arguments: &mut impl Iterator<Item = String>,
    ) -> Result<(), EvalError> {
        match argument {
            "--harness" => self.harness = next_option_value(arguments, argument, "a value")?,
            "--model" => self.model = Some(next_option_value(arguments, argument, "a value")?),
            "--only" => {
                self.only = Some(next_option_value(
                    arguments,
                    argument,
                    "an eval name (e.g. e7)",
                )?);
            }
            "--samples" => {
                self.samples = next_option_value(arguments, argument, "a count")?
                    .parse::<u32>()
                    .ok()
                    .filter(|n| (1..=25).contains(n))
                    .ok_or_else(|| {
                        EvalError::Agent("--samples must be an integer in 1..=25".to_owned())
                    })?;
            }
            "--suite" => self.suite = next_option_value(arguments, argument, "a value")?,
            "--score-review" => {
                self.score_review = Some(PathBuf::from(next_option_value(
                    arguments,
                    argument,
                    "a JSON path",
                )?));
            }
            "--prepare-fixtures" | "--verify-fixtures" => {
                let path = PathBuf::from(next_option_value(
                    arguments,
                    argument,
                    "a fixture-pack manifest path",
                )?);
                if argument == "--prepare-fixtures" {
                    self.prepare_fixtures = Some(path);
                } else {
                    self.verify_fixtures = Some(path);
                }
            }
            "--rerender-document" => {
                self.rerender_document = Some(PathBuf::from(next_option_value(
                    arguments,
                    argument,
                    "a JSON path",
                )?));
            }
            "--artifact-directory" => {
                self.artifact_directory = Some(PathBuf::from(next_option_value(
                    arguments, argument, "a path",
                )?));
            }
            "--delivery-profile" => {
                self.delivery_profile =
                    parse_delivery_profile(&next_option_value(arguments, argument, "a value")?)?;
            }
            "--loudness-contract" => {
                self.loudness_contract = Some(parse_loudness_contract(&next_option_value(
                    arguments,
                    argument,
                    "MIN_LUFS,MAX_LUFS,MAX_PEAK",
                )?)?);
            }
            "--audio-tail-contract" => {
                self.audio_tail_contract = Some(parse_audio_tail_contract(&next_option_value(
                    arguments,
                    argument,
                    "TERMINAL_FRAMES,MAX_PEAK,ACTIVITY_FRAMES,MIN_ACTIVE_LUFS,MAX_INACTIVE_FRAMES",
                )?)?);
            }
            "--write-color-smoke-media" => {
                self.write_color_smoke_media = Some(PathBuf::from(next_option_value(
                    arguments,
                    "--write-color-smoke-media",
                )?));
            }
            "-h" | "--help" => {
                print_usage();
                return Err(EvalError::Agent("help requested".to_owned()));
            }
            other => return Err(EvalError::Agent(format!("unknown argument {other:?}"))),
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), EvalError> {
        let exclusive_actions = [
            self.score_review.is_some(),
            self.prepare_fixtures.is_some(),
            self.verify_fixtures.is_some(),
            self.rerender_document.is_some(),
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();
        if exclusive_actions > 1 {
            return Err(EvalError::Agent(
                "--score-review, --prepare-fixtures, --verify-fixtures, and --rerender-document are mutually exclusive".to_owned(),
            ));
        }
        if self.rerender_document.is_some() != self.artifact_directory.is_some() {
            return Err(EvalError::Agent(
                "--rerender-document and --artifact-directory must be provided together".to_owned(),
            ));
        }
        Ok(())
    }
}

fn print_usage() {
    println!("{}", usage_text());
}

/// The usage banner, as one string, so a test can assert the suite list is
/// complete without capturing stdout.
fn usage_text() -> &'static str {
    "Usage: KINEWRIGHT_EVAL=1 cargo run -p kinewright-agent --bin kinewright-eval -- [--suite auto-edit-v1|finished-cut-v2|editorial-cut-v3|dialogue-pacing-v4|generalization-v5|color-workflow-v6] [--harness claude-code|codex|cursor] [--model MODEL] [--only EVAL] [--samples N]\n       cargo run -p kinewright-agent --bin kinewright-eval -- --prepare-fixtures MANIFEST\n       cargo run -p kinewright-agent --bin kinewright-eval -- --verify-fixtures MANIFEST\n       cargo run -p kinewright-agent --bin kinewright-eval -- --score-review PATH\n       cargo run -p kinewright-agent --bin kinewright-eval -- --rerender-document DOCUMENT --artifact-directory DIRECTORY [--delivery-profile vertical_short] [--loudness-contract MIN_LUFS,MAX_LUFS,MAX_PEAK] [--audio-tail-contract TERMINAL_FRAMES,MAX_PEAK,ACTIVITY_FRAMES,MIN_ACTIVE_LUFS,MAX_INACTIVE_FRAMES]\n       cargo run -p kinewright-agent --bin kinewright-eval -- --write-color-smoke-media DIRECTORY"
}

fn next_option_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
    expected: &str,
) -> Result<String, EvalError> {
    arguments
        .next()
        .ok_or_else(|| EvalError::Agent(format!("{option} requires {expected}")))
}

fn parse_delivery_profile(value: &str) -> Result<DeliveryProfile, EvalError> {
    DeliveryProfile::ALL
        .into_iter()
        .find(|profile| profile.as_str() == value)
        .ok_or_else(|| {
            EvalError::Agent(format!(
                "unknown delivery profile {value:?}; expected source_master, youtube_1080p, vertical_short, or square_social"
            ))
        })
}

fn parse_loudness_contract(value: &str) -> Result<EvalLoudnessSpec, EvalError> {
    let values = value
        .split(',')
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            EvalError::Agent(
                "--loudness-contract values must be signed integer hundredths".to_owned(),
            )
        })?;
    let [minimum, maximum, peak] = values.as_slice() else {
        return Err(EvalError::Agent(
            "--loudness-contract requires MIN_LUFS,MAX_LUFS,MAX_PEAK".to_owned(),
        ));
    };
    if minimum > maximum || *peak > 0 {
        return Err(EvalError::Agent(
            "--loudness-contract requires MIN_LUFS <= MAX_LUFS and MAX_PEAK <= 0".to_owned(),
        ));
    }
    Ok(EvalLoudnessSpec {
        minimum_integrated_lufs_hundredths: *minimum,
        maximum_integrated_lufs_hundredths: *maximum,
        maximum_sample_peak_dbfs_hundredths: *peak,
    })
}

fn parse_audio_tail_contract(value: &str) -> Result<EvalAudioTailSpec, EvalError> {
    let values = value
        .split(',')
        .map(str::parse::<i64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            EvalError::Agent("--audio-tail-contract values must be signed integers".to_owned())
        })?;
    let [
        terminal_frames,
        maximum_peak,
        activity_frames,
        minimum_active_lufs,
        maximum_inactive_frames,
    ] = values.as_slice()
    else {
        return Err(EvalError::Agent(
            "--audio-tail-contract requires TERMINAL_FRAMES,MAX_PEAK,ACTIVITY_FRAMES,MIN_ACTIVE_LUFS,MAX_INACTIVE_FRAMES"
                .to_owned(),
        ));
    };
    let maximum_peak = i32::try_from(*maximum_peak).map_err(|_| {
        EvalError::Agent("--audio-tail-contract MAX_PEAK is outside the i32 range".to_owned())
    })?;
    let minimum_active_lufs = i32::try_from(*minimum_active_lufs).map_err(|_| {
        EvalError::Agent(
            "--audio-tail-contract MIN_ACTIVE_LUFS is outside the i32 range".to_owned(),
        )
    })?;
    if *terminal_frames <= 0
        || maximum_peak > 0
        || *activity_frames <= 0
        || minimum_active_lufs > 0
        || *maximum_inactive_frames < 0
    {
        return Err(EvalError::Agent(
            "--audio-tail-contract requires positive windows, non-positive peak/loudness thresholds, and a non-negative inactive-frame limit"
                .to_owned(),
        ));
    }
    Ok(EvalAudioTailSpec {
        terminal_window_frames: TimeCode(*terminal_frames),
        maximum_sample_peak_dbfs_hundredths: maximum_peak,
        activity_window_frames: TimeCode(*activity_frames),
        minimum_active_integrated_lufs_hundredths: minimum_active_lufs,
        maximum_trailing_inactive_frames: TimeCode(*maximum_inactive_frames),
    })
}

fn rerender_document(document_path: &Path, options: &Options) -> Result<bool, EvalError> {
    let artifact_directory = options.artifact_directory.as_deref().ok_or_else(|| {
        EvalError::Agent("--artifact-directory is required for rerendering".to_owned())
    })?;
    let bytes = fs::read(document_path).map_err(|error| {
        EvalError::Output(format!(
            "could not read saved document {}: {error}",
            document_path.display()
        ))
    })?;
    let document: Document = serde_json::from_slice(&bytes).map_err(|error| {
        EvalError::Output(format!(
            "could not parse saved document {}: {error}",
            document_path.display()
        ))
    })?;
    let engine = eval_engine();
    let result = render_saved_deliverable(
        EvalDeliverableSpec {
            profile: options.delivery_profile,
            focus_x_percent: 50,
            focus_y_percent: 50,
            proof_frames: 9,
            proof_cell_width: 240,
            require_audio: true,
            expected_transcript_word_set: None,
            maximum_word_error_rate_basis_points: 10_000,
            maximum_caption_word_error_rate_basis_points: None,
            loudness: options.loudness_contract,
            audio_tail: options.audio_tail_contract,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        },
        &document,
        engine.as_ref(),
        engine.as_ref(),
        artifact_directory,
    );
    fs::create_dir_all(artifact_directory).map_err(|error| EvalError::Output(error.to_string()))?;
    let report_path = artifact_directory.join("rerender-report.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&result).map_err(|error| EvalError::Output(error.to_string()))?,
    )
    .map_err(|error| EvalError::Output(error.to_string()))?;
    println!("Rerender: {}", result.output_path.display());
    println!("Proof: {}", result.proof_path.display());
    println!("Report: {}", report_path.display());
    Ok(result.machine_passed)
}

fn run_identifier(environment: &EnvironmentStamp) -> String {
    result_path(Path::new(""), environment)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("kinewright-eval-run")
        .to_owned()
}

/// Score either `human-review.json` or `blind/review-form.json`.
///
/// The blind form is resolved through `blind-key.json` **before**
/// `verify_review_artifact_bindings` runs: that function rebuilds task ids
/// from the machine report's result names and looks each reviewed task up by
/// `task_id`, so a form keyed on `blind_id` alone fails there for every task
/// unless the resolution happens first. That ordering is the change this
/// caller makes; `verify_review_artifact_bindings` itself is untouched.
fn score_review_file(path: &Path) -> Result<(), EvalError> {
    let (review, run_directory) = load_review_for_scoring(path)?;
    let machine_report_path = run_directory.join("machine-report.json");
    let machine_report_bytes = fs::read(&machine_report_path).map_err(|error| {
        EvalError::Output(format!(
            "could not read machine report {} for review binding: {error}",
            machine_report_path.display()
        ))
    })?;
    let machine_report: serde_json::Value =
        serde_json::from_slice(&machine_report_bytes).map_err(|error| {
            EvalError::Output(format!(
                "could not parse machine report {} for review binding: {error}",
                machine_report_path.display()
            ))
        })?;
    verify_review_artifact_bindings(&review, &machine_report)?;
    let summary = summarize_human_review(&review)?;
    let output = run_directory.join("human-score.json");
    fs::write(
        &output,
        serde_json::to_vec_pretty(&summary)
            .map_err(|error| EvalError::Output(error.to_string()))?,
    )
    .map_err(|error| EvalError::Output(error.to_string()))?;
    println!(
        "Human review: {}/{} reviewed, {}/{} accepted",
        summary.tasks_reviewed, summary.tasks_total, summary.tasks_accepted, summary.tasks_reviewed
    );
    println!("Score: {}", output.display());
    Ok(())
}

/// Load the review to score and the run root its outputs belong in.
///
/// A `human-review.json` is taken exactly as it always was. A
/// `blind/review-form.json` is unblinded through the sibling
/// `blind-key.json` in the run root and the resulting `human-review.json` is
/// written there, so the reviewer's answers survive whatever the binding
/// check decides next.
fn load_review_for_scoring(path: &Path) -> Result<(HumanReviewFile, PathBuf), EvalError> {
    let bytes = fs::read(path).map_err(|error| {
        EvalError::Output(format!("could not read review {}: {error}", path.display()))
    })?;
    let document: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        EvalError::Output(format!(
            "could not parse review {}: {error}",
            path.display()
        ))
    })?;
    let directory = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    if document.get("tasks").is_some() {
        let review: HumanReviewFile = serde_json::from_value(document).map_err(|error| {
            EvalError::Output(format!(
                "could not parse review {}: {error}",
                path.display()
            ))
        })?;
        return Ok((review, directory));
    }
    let form: BlindReviewForm = serde_json::from_value(document).map_err(|error| {
        EvalError::Output(format!(
            "{} is neither a human review nor a blind review form: {error}",
            path.display()
        ))
    })?;
    // §9.5 declares both blind files `schema_version: 1`. `summarize_human_review`
    // checks its own version, and a form or key written by a later CC would
    // otherwise be read as if it meant what this one means.
    if form.schema_version != BLIND_SCHEMA_VERSION {
        return Err(EvalError::Output(format!(
            "{} carries blind schema_version {}, not {BLIND_SCHEMA_VERSION}",
            path.display(),
            form.schema_version
        )));
    }
    // The key lives in the run root, outside `blind/`.
    let run_directory = directory
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let key_path = run_directory.join(BLIND_KEY_FILE_NAME);
    let key_bytes = fs::read(&key_path).map_err(|error| {
        EvalError::Output(format!(
            "blind review form {} has no {} at {}: {error}",
            path.display(),
            BLIND_KEY_FILE_NAME,
            key_path.display()
        ))
    })?;
    let key: BlindKeyFile = serde_json::from_slice(&key_bytes).map_err(|error| {
        EvalError::Output(format!("could not parse {}: {error}", key_path.display()))
    })?;
    if key.schema_version != BLIND_SCHEMA_VERSION {
        return Err(EvalError::Output(format!(
            "{} carries blind schema_version {}, not {BLIND_SCHEMA_VERSION}",
            key_path.display(),
            key.schema_version
        )));
    }
    let review = key.unblind(&form)?;
    write_json(&run_directory.join("human-review.json"), &review)?;
    Ok((review, run_directory))
}

fn verify_review_artifact_bindings(
    review: &HumanReviewFile,
    machine_report: &serde_json::Value,
) -> Result<(), EvalError> {
    if machine_report["benchmark_id"].as_str() != Some(review.benchmark_id.as_str())
        || machine_report["run_id"].as_str() != Some(review.run_id.as_str())
    {
        return Err(EvalError::Output(
            "human review benchmark_id/run_id does not match its sibling machine report".to_owned(),
        ));
    }
    let results = machine_report["results"]
        .as_array()
        .ok_or_else(|| EvalError::Output("machine report results must be an array".to_owned()))?;
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut expected_hashes = BTreeMap::<String, String>::new();
    for result in results {
        let Some(name) = result["name"].as_str() else {
            continue;
        };
        let base_task_id = name.split_whitespace().next().unwrap_or(name);
        let occurrence = occurrences.entry(base_task_id.to_owned()).or_default();
        *occurrence = occurrence.saturating_add(1);
        let task_id = if *occurrence == 1 {
            base_task_id.to_owned()
        } else {
            format!("{base_task_id}-sample-{occurrence}")
        };
        let Some(hash) = result["deliverable"]["output_sha256"].as_str() else {
            continue;
        };
        if expected_hashes
            .insert(task_id.clone(), hash.to_owned())
            .is_some()
        {
            return Err(EvalError::Output(format!(
                "machine report contains duplicate human-review task id {task_id:?}"
            )));
        }
    }
    for task in &review.tasks {
        let expected = expected_hashes.get(&task.task_id).ok_or_else(|| {
            EvalError::Output(format!(
                "human-review task {:?} has no delivered artifact in the machine report",
                task.task_id
            ))
        })?;
        match task.artifact_sha256.as_deref() {
            Some(observed) if observed == expected => {}
            Some(observed) => {
                return Err(EvalError::Output(format!(
                    "human-review task {:?} is bound to artifact {observed}, but the machine report records {expected}",
                    task.task_id
                )));
            }
            None if task.accepted.is_some() => {
                return Err(EvalError::Output(format!(
                    "reviewed task {:?} must include the machine-reported artifact sha256 {expected}",
                    task.task_id
                )));
            }
            None => {}
        }
    }
    Ok(())
}

fn print_pass_rates(definitions: &[EvalDefinition], results: &[EvalResult], samples: u32) {
    println!("\nPass rates over {samples} samples:");
    for definition in definitions {
        let runs: Vec<_> = results
            .iter()
            .filter(|result| result.name == definition.name)
            .collect();
        let passes = runs.iter().filter(|result| result.passed).count();
        let total_usd: f64 = runs.iter().filter_map(|result| result.cost_usd).sum();
        println!(
            "  {}: {passes}/{} passed · ${total_usd:.2} total",
            definition.name,
            runs.len()
        );
    }
}

fn print_result_details(result: &EvalResult) {
    println!("RESULT: {}", if result.passed { "PASS" } else { "FAIL" });
    if let Some(error) = &result.execution_error {
        println!("  execution: {error}");
    }
    for assertion in &result.assertions {
        println!(
            "  {} {}: {}",
            if assertion.passed { "PASS" } else { "FAIL" },
            assertion.assertion,
            assertion.detail
        );
    }
}

#[allow(clippy::too_many_lines)]
fn seed_suite() -> Vec<EvalDefinition> {
    vec![
        EvalDefinition {
            name: "e1 split-and-delete",
            rationale: "Measures the original M3 compound edit with exact source-range semantics.",
            fixture_builder: fixture_e1,
            prompts: &[
                "Split the first clip at frame 30, then delete the second clip as it existed before any edits.",
            ],
            assertions: vec![
                EvalAssertion::ClipCount {
                    minimum: 2,
                    maximum: 2,
                },
                EvalAssertion::ExactSourceClips {
                    clips: vec![
                        ExpectedSourceClip {
                            asset_alias: "take".to_owned(),
                            source_start: TimeCode(0),
                            source_end: TimeCode(30),
                        },
                        ExpectedSourceClip {
                            asset_alias: "take".to_owned(),
                            source_start: TimeCode(30),
                            source_end: TimeCode(90),
                        },
                    ],
                },
                EvalAssertion::Gapless,
                required_all(&["get_timeline_state"]),
                required_any(&["commit_edit_plan", "apply_edit_plan", "split_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(4, 2),
            deliverable: None,
            color: None,
        },
        EvalDefinition {
            name: "e2 silence-gap removal",
            rationale: "Measures analysis-led dead-air removal without coupling success to Whisper spelling.",
            fixture_builder: fixture_e2,
            prompts: &[
                "Remove every silent gap at least 20 source frames long. Keep all spoken content and butt the remaining clips together.",
            ],
            assertions: vec![
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::Gapless,
                EvalAssertion::WordsRetained {
                    word_set: "spoken-content".to_owned(),
                },
                EvalAssertion::NoSilenceAtLeast {
                    source_frames: TimeCode(LONG_SILENCE_FRAMES),
                },
                EvalAssertion::DurationBounds {
                    bounds: "without-long-silence".to_owned(),
                },
                required_any(&["get_silences", "get_timeline_silences"]),
                required_any(&[
                    "commit_edit_plan",
                    "apply_edit_plan",
                    "trim_clip",
                    "delete_clip",
                ]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: speech_budget(12, 12),
            deliverable: None,
            color: None,
        },
        EvalDefinition {
            name: "e3 filler-word removal",
            rationale: "Measures transcript-driven deletion while retaining every non-filler word heard pre-edit.",
            fixture_builder: fixture_e3,
            prompts: &[
                "Use the transcript to remove the filler words. Preserve all other spoken words and close the gaps.",
            ],
            assertions: vec![
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::Gapless,
                EvalAssertion::WordsRetained {
                    word_set: "non-filler".to_owned(),
                },
                EvalAssertion::WordsAbsent {
                    word_set: "recognized-fillers".to_owned(),
                },
                required_any(&["get_transcript", "get_timeline_transcript"]),
                required_any(&[
                    "commit_edit_plan",
                    "apply_edit_plan",
                    "trim_clip",
                    "delete_clip",
                ]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: speech_budget(12, 8),
            deliverable: None,
            color: None,
        },
        EvalDefinition {
            name: "e4 scene-cut",
            rationale: "Measures scene analysis and exact splitting without prescribing a plan shape.",
            fixture_builder: fixture_e4,
            prompts: &[
                "Split the clip at every detected scene change with confidence at least 1000 basis points.",
            ],
            assertions: vec![
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::Gapless,
                EvalAssertion::SceneChangesAreCuts {
                    scene_set: "detected-scenes".to_owned(),
                },
                required_any(&["get_scene_changes", "get_timeline_scene_changes"]),
                required_any(&["commit_edit_plan", "apply_edit_plan", "split_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(10, 8),
            deliverable: None,
            color: None,
        },
        EvalDefinition {
            name: "e5 effect-and-transition",
            rationale: "Measures non-destructive effect and transition orchestration on an ordinal target.",
            fixture_builder: fixture_e5,
            prompts: &[
                "Brighten the second clip by 20 percent and add a 12-frame crossfade into it.",
            ],
            assertions: vec![
                EvalAssertion::ClipCount {
                    minimum: 2,
                    maximum: 2,
                },
                EvalAssertion::AssetOrder {
                    aliases: aliases(&["first", "second"]),
                    collapse_adjacent: false,
                },
                EvalAssertion::Gapless,
                EvalAssertion::EffectOnAsset {
                    asset_alias: "second".to_owned(),
                    effect_name: "brightness".to_owned(),
                    integer_parameter: Some(("percent".to_owned(), 20)),
                },
                EvalAssertion::TransitionOnAsset {
                    asset_alias: "second".to_owned(),
                    transition_name: "crossfade".to_owned(),
                },
                required_all(&["get_timeline_state"]),
                required_any(&[
                    "commit_edit_plan",
                    "apply_edit_plan",
                    "add_effect",
                    "add_transition",
                ]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(5, 4),
            deliverable: None,
            color: None,
        },
        EvalDefinition {
            name: "e6 ordinal-resolution stress",
            rationale: "Catches the M3 trap where an early split renumbers later ordinal targets.",
            fixture_builder: fixture_e6,
            prompts: &[
                "Resolve all ordinals against the initial timeline: split the first clip at frame 30, delete the second clip, close the resulting timeline gap, then brighten the third clip by 15 percent.",
            ],
            assertions: vec![
                EvalAssertion::AssetOrder {
                    aliases: aliases(&["first", "third"]),
                    collapse_adjacent: true,
                },
                EvalAssertion::AssetAbsent {
                    alias: "second".to_owned(),
                },
                EvalAssertion::EffectOnAsset {
                    asset_alias: "third".to_owned(),
                    effect_name: "brightness".to_owned(),
                    integer_parameter: Some(("percent".to_owned(), 15)),
                },
                EvalAssertion::Gapless,
                required_all(&["get_timeline_state"]),
                required_any(&["commit_edit_plan", "apply_edit_plan", "split_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(6, 5),
            deliverable: None,
            color: None,
        },
        EvalDefinition {
            name: "e7 flagship rough cut",
            rationale: "Measures deterministic rough-cut assembly, cleanup, ordering, and reversibility from an empty timeline.",
            fixture_builder: fixture_e7,
            prompts: &[
                "Assemble a rough cut from the media pool. Use take-A's content first, then take-C, then take-D. Do not use take-B or take-E. Cut dead air at least 20 source frames long and remove recognized filler words. Butt every resulting clip together with no gaps. The brief is explicit; do not choose alternate takes.",
            ],
            assertions: vec![
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::AssetOrder {
                    aliases: aliases(&["take-A", "take-C", "take-D"]),
                    collapse_adjacent: true,
                },
                EvalAssertion::AssetAbsent {
                    alias: "take-B".to_owned(),
                },
                EvalAssertion::AssetAbsent {
                    alias: "take-E".to_owned(),
                },
                EvalAssertion::Gapless,
                EvalAssertion::WordsRetained {
                    word_set: "take-A-content".to_owned(),
                },
                EvalAssertion::WordsRetained {
                    word_set: "take-C-content".to_owned(),
                },
                EvalAssertion::WordsRetained {
                    word_set: "take-D-content".to_owned(),
                },
                EvalAssertion::WordsAbsent {
                    word_set: "take-B-unique".to_owned(),
                },
                EvalAssertion::WordsAbsent {
                    word_set: "selected-fillers".to_owned(),
                },
                EvalAssertion::NoSilenceAtLeast {
                    source_frames: TimeCode(LONG_SILENCE_FRAMES),
                },
                EvalAssertion::DurationBounds {
                    bounds: "rough-cut".to_owned(),
                },
                required_all(&["get_timeline_state"]),
                required_any(&["get_transcript", "get_timeline_transcript"]),
                required_any(&["get_silences", "get_timeline_silences"]),
                required_any(&["commit_edit_plan", "apply_edit_plan", "add_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: flagship_budget(),
            deliverable: None,
            color: None,
        },
    ]
}

fn finished_cut_suite() -> Vec<EvalDefinition> {
    vec![EvalDefinition {
        name: "f1 finished vertical story",
        rationale: "Measures footage selection, dialogue cleanup, styled captions, visual verification, technical QA, and a real delivery artifact as one first-cut workflow.",
        fixture_builder: fixture_e7,
        prompts: &[
            "Create a finished vertical social cut from the media pool. Use take-A's content first, then take-C, then take-D. Do not use take-B or take-E. Prefer plan_dialogue_assembly to calculate a gapless track-1 assembly that removes every raw dead-air span at least 20 source frames long and every conservative recognized filler word while preserving all other spoken content. Keep each selected real-time A/V clip's source dialogue audible; video-track A/V clips already feed the mixer, so do not duplicate them onto an audio track. Then use add_styled_captions with the social preset and pop motion. Inspect the final timeline silences and keep correcting until zero cuttable spans remain. Inspect a 9:16 delivery storyboard, run technical QA, and inspect vertical_short delivery conformance at a centered 50/50 focal point. Do not queue the export; the benchmark will render the exact verified timeline snapshot. Keep working until every inspector confirms the brief.",
        ],
        assertions: vec![
            EvalAssertion::TimelineNonEmpty,
            EvalAssertion::AssetOrder {
                aliases: aliases(&["take-A", "take-C", "take-D"]),
                collapse_adjacent: true,
            },
            EvalAssertion::AssetAbsent {
                alias: "take-B".to_owned(),
            },
            EvalAssertion::AssetAbsent {
                alias: "take-E".to_owned(),
            },
            EvalAssertion::MediaGapless,
            EvalAssertion::WordsRetained {
                word_set: "take-A-content".to_owned(),
            },
            EvalAssertion::WordsRetained {
                word_set: "take-C-content".to_owned(),
            },
            EvalAssertion::WordsRetained {
                word_set: "take-D-content".to_owned(),
            },
            EvalAssertion::WordsAbsent {
                word_set: "take-B-unique".to_owned(),
            },
            EvalAssertion::WordsAbsent {
                word_set: "selected-fillers".to_owned(),
            },
            EvalAssertion::NoSilenceAtLeast {
                source_frames: TimeCode(LONG_SILENCE_FRAMES),
            },
            EvalAssertion::DurationBounds {
                bounds: "rough-cut".to_owned(),
            },
            EvalAssertion::StyledCaptions {
                minimum_cues: 3,
                motion: CaptionMotion::Pop,
            },
            EvalAssertion::CaptionSafeArea {
                profile: DeliveryProfile::VerticalShort,
            },
            EvalAssertion::AudioPresent,
            EvalAssertion::QaExportReady,
            required_all(&[
                "get_timeline_state",
                "plan_dialogue_assembly",
                "add_styled_captions",
            ]),
            required_any(&[
                "plan_dialogue_assembly",
                "get_transcript",
                "get_timeline_transcript",
            ]),
            required_any(&[
                "plan_dialogue_assembly",
                "get_silences",
                "get_timeline_silences",
            ]),
            required_any(&["commit_edit_plan", "apply_edit_plan", "add_clip"]),
            required_all(&[
                "get_delivery_variant_storyboard",
                "get_qa_report",
                "get_delivery_conformance",
            ]),
            EvalAssertion::UndoIntegrity,
        ],
        budgets: finished_cut_budget(),
        deliverable: Some(EvalDeliverableSpec {
            profile: DeliveryProfile::VerticalShort,
            focus_x_percent: 50,
            focus_y_percent: 50,
            proof_frames: 9,
            proof_cell_width: 240,
            require_audio: true,
            expected_transcript_word_set: None,
            maximum_word_error_rate_basis_points: 0,
            maximum_caption_word_error_rate_basis_points: None,
            loudness: None,
            audio_tail: None,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        }),
        color: None,
    }]
}

fn event_multicam_definition() -> EvalDefinition {
    EvalDefinition {
        name: "g2 real meeting multicam introductions",
        rationale: "Measures whether the agent can turn synchronized licensed meeting cameras plus speaker-labelled source metadata into continuous speaker-aware coverage with normalized program audio and face-safe editable reframing.",
        fixture_builder: fixture_real_event_multicam,
        prompts: &[
            "Create a finished vertical multicam cut of the already bounded AMI meeting introduction. Open exactly these six capability schemas in one get_capability call: get_transcript, plan_speaker_multicam, add_effect, track_reframe_subject, plan_audio_normalization, and get_editorial_readiness. Do not call search_capabilities unless one of those exact lookups fails. Inspect the timeline and the speaker-labelled transcript on camera-laura. Use sync group 1 and video track 1. Map Laura to 'Laura closeup', David to 'David closeup', Andrew to 'Andrew closeup', and Craig to 'Craig closeup'. Call plan_speaker_multicam for group frames 1750 through 2544 exclusive, record start 0, maximum word gap 5 frames, and minimum shot length 25 frames. This deliberately suppresses brief overlapping backchannels. Inspect that planner's prepared_edit_plan preview and commit its returned plan id directly; do not call prepare_edit_plan or rewrite its operations. Keep track 2 as the single uninterrupted program-audio source; do not cut, retime, duplicate, fade, or change its clip gain. Reinspect the generated video clips. In one model-authored prepared edit plan, add exactly one reframe effect to every video clip using unique effect ids, target_aspect_basis_points 5625, and initial focus_x_percent/focus_y_percent 50/42; inspect and commit that plan. Then call track_reframe_subject once for each video clip with its reframe effect, subject width 25 percent, subject height 30 percent, initial subject center 50/42, horizontal focus bounds 25 through 75, vertical focus bounds 20 through 80, subject dead zone 6 percent, maximum focus step 2 percent, step 12 frames, search radius 10 percent, and max width 256. Each tracking call returns a stabilized prepared_edit_plan with linear camera motion; inspect and commit its returned plan id directly before tracking the next clip, without calling prepare_edit_plan or copying keyframe operations. Preserve the continuous program source but call plan_audio_normalization for track 2 at -1600 LUFS hundredths with a -100 dBFS-hundredths sample-peak ceiling and 100-hundredths tolerance; inspect and commit its returned plan. Do not add captions, transitions, music, titles, or dialogue edits. Finish with one get_editorial_readiness call using vertical_short, check_silence false because continuous program audio is intentionally preserved, centered 50/50 delivery focus, nine storyboard frames, and 240-pixel cells. Do not queue export; the benchmark renders and independently probes the verified snapshot. Keep working until readiness is true.",
        ],
        assertions: event_multicam_assertions(),
        budgets: EvalBudgets {
            max_turns: 1,
            max_tool_calls: 24,
            max_operations: 80,
            max_tokens: 500_000,
            max_cost_usd: None,
            max_wall_time: Duration::from_hours(1),
            max_undos: 80,
        },
        deliverable: Some(EvalDeliverableSpec {
            profile: DeliveryProfile::VerticalShort,
            focus_x_percent: 50,
            focus_y_percent: 50,
            proof_frames: 9,
            proof_cell_width: 240,
            require_audio: true,
            expected_transcript_word_set: Some("event-dialogue"),
            maximum_word_error_rate_basis_points: 3_000,
            maximum_caption_word_error_rate_basis_points: None,
            loudness: Some(EvalLoudnessSpec {
                minimum_integrated_lufs_hundredths: -1_800,
                maximum_integrated_lufs_hundredths: -1_400,
                maximum_sample_peak_dbfs_hundredths: -100,
            }),
            audio_tail: None,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        }),
        color: None,
    }
}

fn event_multicam_assertions() -> Vec<EvalAssertion> {
    vec![
        EvalAssertion::TimelineNonEmpty,
        EvalAssertion::ClipCount {
            minimum: 6,
            maximum: 6,
        },
        EvalAssertion::ExactTrackClips {
            track: TrackId(1),
            clips: vec![
                ExpectedTimelineClip {
                    asset_alias: "camera-laura".to_owned(),
                    timeline_start: TimeCode(0),
                    timeline_end: TimeCode(184),
                    source_start: TimeCode(1_750),
                    source_end: TimeCode(1_934),
                },
                ExpectedTimelineClip {
                    asset_alias: "camera-david".to_owned(),
                    timeline_start: TimeCode(184),
                    timeline_end: TimeCode(287),
                    source_start: TimeCode(1_934),
                    source_end: TimeCode(2_037),
                },
                ExpectedTimelineClip {
                    asset_alias: "camera-andrew".to_owned(),
                    timeline_start: TimeCode(287),
                    timeline_end: TimeCode(380),
                    source_start: TimeCode(2_037),
                    source_end: TimeCode(2_130),
                },
                ExpectedTimelineClip {
                    asset_alias: "camera-craig".to_owned(),
                    timeline_start: TimeCode(380),
                    timeline_end: TimeCode(475),
                    source_start: TimeCode(2_130),
                    source_end: TimeCode(2_225),
                },
                ExpectedTimelineClip {
                    asset_alias: "camera-laura".to_owned(),
                    timeline_start: TimeCode(475),
                    timeline_end: TimeCode(794),
                    source_start: TimeCode(2_225),
                    source_end: TimeCode(2_544),
                },
            ],
        },
        EvalAssertion::ExactTrackClips {
            track: TrackId(2),
            clips: vec![ExpectedTimelineClip {
                asset_alias: "program-audio".to_owned(),
                timeline_start: TimeCode(0),
                timeline_end: TimeCode(794),
                source_start: TimeCode(2_100),
                source_end: TimeCode(3_053),
            }],
        },
        EvalAssertion::MediaGapless,
        EvalAssertion::DurationBounds {
            bounds: "event-introductions".to_owned(),
        },
        EvalAssertion::ReframeStability {
            track: TrackId(1),
            minimum_keyframes_per_axis: 2,
            min_x_percent: 25,
            max_x_percent: 75,
            min_y_percent: 20,
            max_y_percent: 80,
            maximum_step_percent: 2,
        },
        EvalAssertion::AudioPresent,
        EvalAssertion::ProgramAudioContinuous {
            track: TrackId(2),
            asset_alias: "program-audio".to_owned(),
        },
        EvalAssertion::QaExportReady,
        required_all(&[
            "get_timeline_state",
            "get_transcript",
            "plan_speaker_multicam",
            "prepare_edit_plan",
            "commit_edit_plan",
            "track_reframe_subject",
            "plan_audio_normalization",
            "get_editorial_readiness",
        ]),
        EvalAssertion::UndoIntegrity,
    ]
}

fn music_montage_definition() -> EvalDefinition {
    let truth: MusicMontageGroundTruth = serde_json::from_str(include_str!(
        "../../../../benchmarks/auto-edit/v5/music-ground-truth-v10.json"
    ))
    .expect("checked-in v5 music ground truth must parse");
    EvalDefinition {
        name: "g3 single-source trailer edit",
        rationale: "Measures whether the agent can inspect one licensed narrative source, recut it into a coherent character-led trailer, and resolve a deliberate beat-aware edit on a trailer cue's authored final tag.",
        fixture_builder: fixture_real_music_montage,
        prompts: &[MUSIC_MONTAGE_PROMPT_V10],
        assertions: music_montage_assertions(),
        budgets: EvalBudgets {
            max_turns: 1,
            max_tool_calls: 24,
            max_operations: 80,
            max_tokens: 500_000,
            max_cost_usd: None,
            max_wall_time: Duration::from_hours(1),
            max_undos: 80,
        },
        deliverable: Some(EvalDeliverableSpec {
            profile: DeliveryProfile::Youtube1080p,
            focus_x_percent: 50,
            focus_y_percent: 50,
            proof_frames: 9,
            proof_cell_width: 240,
            require_audio: true,
            expected_transcript_word_set: None,
            maximum_word_error_rate_basis_points: 0,
            maximum_caption_word_error_rate_basis_points: None,
            loudness: Some(EvalLoudnessSpec {
                minimum_integrated_lufs_hundredths: -1_800,
                maximum_integrated_lufs_hundredths: -1_400,
                maximum_sample_peak_dbfs_hundredths: -100,
            }),
            audio_tail: Some(EvalAudioTailSpec {
                terminal_window_frames: TimeCode(truth.rendered_tail_window_frames),
                maximum_sample_peak_dbfs_hundredths: truth
                    .rendered_tail_maximum_peak_dbfs_hundredths,
                activity_window_frames: TimeCode(truth.rendered_activity_window_frames),
                minimum_active_integrated_lufs_hundredths: truth
                    .rendered_activity_minimum_integrated_lufs_hundredths,
                maximum_trailing_inactive_frames: TimeCode(truth.maximum_trailing_inactive_frames),
            }),
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        }),
        color: None,
    }
}

#[allow(clippy::too_many_lines)]
fn music_montage_assertions() -> Vec<EvalAssertion> {
    let truth: MusicMontageGroundTruth = serde_json::from_str(include_str!(
        "../../../../benchmarks/auto-edit/v5/music-ground-truth-v10.json"
    ))
    .expect("checked-in v5 music ground truth must parse");
    let visual_aliases = truth
        .visual_asset_ids
        .iter()
        .map(|fixture_id| {
            music_fixture_alias(fixture_id)
                .expect("checked-in v5 visual asset must have a stable alias")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let primary_visual_alias = visual_aliases
        .first()
        .expect("checked-in single-source trailer must name one visual asset")
        .clone();
    let mut assertions = vec![
        EvalAssertion::TimelineNonEmpty,
        EvalAssertion::ExactProjectDuration {
            duration: TimeCode(truth.timeline_range.end),
        },
        EvalAssertion::ExactTrackMediaCoverage {
            track: TrackId(truth.video_track_id),
            range: TimeCode(truth.timeline_range.start)
                ..TimeCode(truth.title_card.timeline_range.start),
        },
        EvalAssertion::ExactTrackMediaCoverage {
            track: TrackId(truth.audio_track_id),
            range: TimeCode(truth.timeline_range.start)..TimeCode(truth.timeline_range.end),
        },
        EvalAssertion::ClipCount {
            minimum: truth.minimum_visual_shots + 2,
            maximum: truth.maximum_visual_shots + 2,
        },
        EvalAssertion::MediaClipCount {
            track: TrackId(truth.video_track_id),
            minimum: truth.minimum_visual_shots,
            maximum: truth.maximum_visual_shots,
            minimum_duration: TimeCode(truth.minimum_shot_frames),
            maximum_duration: TimeCode(truth.maximum_shot_frames),
            reject_non_media: false,
        },
        EvalAssertion::RequiredAssetsOnTrack {
            track: TrackId(truth.video_track_id),
            aliases: visual_aliases.clone(),
        },
        EvalAssertion::SourceRangesSeparated {
            track: TrackId(truth.video_track_id),
            minimum_separation_frames: TimeCode(truth.minimum_source_separation_frames),
        },
        EvalAssertion::SourceRangesChronological {
            track: TrackId(truth.video_track_id),
            minimum_forward_gap_frames: TimeCode(truth.minimum_forward_gap_frames),
        },
        EvalAssertion::SourceRangesSceneClean {
            track: TrackId(truth.video_track_id),
            scene_set: MUSIC_SOURCE_SCENE_SET.to_owned(),
            allowed_baked_sequence_starts: truth
                .reviewed_story_events
                .iter()
                .map(|event| TimeCode(event.project_frame))
                .collect(),
        },
        EvalAssertion::SourceRangesAvoid {
            track: TrackId(truth.video_track_id),
            exclusion_set: MUSIC_SOURCE_EXCLUSION_SET.to_owned(),
        },
        EvalAssertion::ClipSourceWithin {
            track: TrackId(truth.video_track_id),
            timeline_start: TimeCode(truth.timeline_range.start),
            asset_alias: primary_visual_alias.clone(),
            source_window: TimeCode(truth.opening_source_window.start)
                ..TimeCode(truth.opening_source_window.end),
        },
        EvalAssertion::ClipSourceWithin {
            track: TrackId(truth.video_track_id),
            timeline_start: TimeCode(truth.reviewed_music_events[0].project_frame),
            asset_alias: primary_visual_alias.clone(),
            source_window: TimeCode(truth.preparation_source_window.start)
                ..TimeCode(truth.preparation_source_window.end),
        },
        EvalAssertion::ClipSourceWithin {
            track: TrackId(truth.video_track_id),
            timeline_start: TimeCode(truth.reviewed_music_events[1].project_frame),
            asset_alias: primary_visual_alias.clone(),
            source_window: TimeCode(truth.operator_source_window.start)
                ..TimeCode(truth.operator_source_window.end),
        },
        EvalAssertion::ClipSourceWithin {
            track: TrackId(truth.video_track_id),
            timeline_start: TimeCode(truth.reviewed_story_events[0].project_frame),
            asset_alias: primary_visual_alias.clone(),
            source_window: TimeCode(truth.activation_source_window.start)
                ..TimeCode(truth.activation_source_window.end),
        },
        EvalAssertion::ClipSourceWithin {
            track: TrackId(truth.video_track_id),
            timeline_start: TimeCode(truth.reviewed_music_events[2].project_frame),
            asset_alias: primary_visual_alias,
            source_window: TimeCode(truth.climax_source_window.start)
                ..TimeCode(truth.climax_source_window.end),
        },
        EvalAssertion::ShotCadenceVariation {
            track: TrackId(truth.video_track_id),
            minimum_duration_buckets: truth.minimum_duration_buckets,
            duration_bucket_frames: TimeCode(truth.duration_bucket_frames),
            maximum_similar_run: truth.maximum_similar_run,
            similar_tolerance_frames: TimeCode(truth.similar_tolerance_frames),
        },
        EvalAssertion::NoAlternatingShotPattern {
            track: TrackId(truth.video_track_id),
            maximum_repeated_pairs: 2,
            tolerance_frames: TimeCode(truth.similar_tolerance_frames),
        },
        EvalAssertion::NoVisualTransitionsEffectsOrRetiming {
            track: TrackId(truth.video_track_id),
        },
        EvalAssertion::TitleCard {
            track: TrackId(truth.title_card.track_id),
            timeline_start: TimeCode(truth.title_card.timeline_range.start),
            duration: TimeCode(
                truth
                    .title_card
                    .timeline_range
                    .end
                    .saturating_sub(truth.title_card.timeline_range.start),
            ),
            text: truth.title_card.text.clone(),
            font_size_token: truth.title_card.font_size_token,
            color_token: truth.title_card.color_token,
            position: truth.title_card.position,
            background_scrim: truth.title_card.background_scrim,
            fade_in_frames: TimeCode(truth.title_card.fade_in_frames),
            fade_out_frames: TimeCode(truth.title_card.fade_out_frames),
        },
        EvalAssertion::EdgeShotHolds {
            track: TrackId(truth.video_track_id),
            minimum_opening_shot_frames: TimeCode(truth.minimum_opening_shot_frames),
            minimum_closing_shot_frames: TimeCode(truth.minimum_closing_shot_frames),
        },
        EvalAssertion::CutsAlignedToBeatSetAtLeast {
            track: TrackId(truth.video_track_id),
            beat_set: MUSIC_REVIEWED_EVENT_SET.to_owned(),
            tolerance_frames: TimeCode(truth.beat_alignment_tolerance_frames),
            minimum_aligned_cuts: truth.reviewed_music_events.len(),
            minimum_aligned_basis_points: u16::try_from(
                truth.reviewed_music_events.len().saturating_mul(10_000)
                    / truth.minimum_visual_shots.saturating_sub(1),
            )
            .expect("checked-in music cut share fits basis points"),
        },
        EvalAssertion::MusicFit {
            track: TrackId(truth.audio_track_id),
            asset_alias: "music-bed".to_owned(),
            source_beat_set: MUSIC_SOURCE_BEAT_SET.to_owned(),
            timeline_start: TimeCode(truth.timeline_range.start),
            timeline_end: TimeCode(truth.timeline_range.end),
            tolerance_source_frames: TimeCode::ZERO,
        },
        EvalAssertion::MusicSourceEnd {
            track: TrackId(truth.audio_track_id),
            asset_alias: "music-bed".to_owned(),
            expected_source_end: TimeCode(truth.music_preferred_source_end),
            tolerance_source_frames: TimeCode(truth.music_maximum_end_drift_frames),
        },
        EvalAssertion::MediaGapless,
        EvalAssertion::AudioPresent,
        EvalAssertion::SingleAudioMediaClip {
            track: TrackId(truth.audio_track_id),
            asset_alias: "music-bed".to_owned(),
        },
        EvalAssertion::QaExportReady,
        required_all(&[
            "get_capability",
            "get_source_shot_board",
            "plan_music_fit",
            "get_music_structure",
            "plan_beat_montage",
            "plan_audio_normalization",
            "get_cut_neighborhoods",
            "get_editorial_readiness",
            "commit_edit_plan",
        ]),
        EvalAssertion::UndoIntegrity,
    ];
    for alias in &visual_aliases {
        assertions.insert(
            7,
            EvalAssertion::AssetUseMinimum {
                track: TrackId(truth.video_track_id),
                asset_alias: alias.clone(),
                minimum_clip_count: truth.minimum_clips_per_visual_asset,
                minimum_project_frames: TimeCode(truth.minimum_project_frames_per_visual_asset),
            },
        );
    }
    if visual_aliases.len() > 1 {
        for alias in &visual_aliases {
            assertions.insert(
                8,
                EvalAssertion::AssetTemporalSpread {
                    track: TrackId(truth.video_track_id),
                    asset_alias: alias.clone(),
                    latest_early_start: TimeCode(truth.latest_early_start_per_visual_asset),
                    earliest_late_start: TimeCode(truth.earliest_late_start_per_visual_asset),
                },
            );
        }
    }
    assertions
}

fn editorial_cut_suite() -> Vec<EvalDefinition> {
    vec![EvalDefinition {
        name: "f2 coherent neighborhood garden story",
        rationale: "Measures coherent take selection, natural dialogue cleanup, exact authored captions, independent post-render speech verification, visual review, technical QA, and a real vertical delivery artifact.",
        fixture_builder: fixture_editorial_story,
        prompts: &[
            "Create a finished vertical social story about a neighborhood garden from the five takes. Batch-load the exact named capabilities in this brief in one get_capability call, and search only for a need that remains unnamed. Inspect all five takes in one get_transcripts call. Choose the three takes that form this factual arc: the empty lot collected weeds and rainwater; neighbors turned it into food-growing space by building raised beds and planting tomatoes, herbs, and peppers; the Saturday market now supplies fresh produce to dozens of local families. Reject every flub or factually wrong alternate. Use plan_dialogue_assembly to preserve the clean spoken content in story order, remove all audible um sounds with 3 source frames of filler padding, remove raw dead air at least 20 source frames long, and retain 6 source frames of natural pause across every silence cut. Keep the real-time A/V source dialogue audible without duplicating it onto an audio track. Use add_styled_captions with the social preset and pop motion. The exact intended caption wording, excluding fillers, is: 'Last spring this empty lot collected weeds and rainwater. Neighbors decided it could feed families instead. Over three weekends volunteers built raised beds. Then they planted tomatoes herbs and peppers. Now the Saturday market supplies fresh produce to dozens of local families.' Inspect every generated cue with get_captions and use plan_caption_corrections plus prepare_edit_plan and commit_edit_plan if any cue differs from those intended words. Finish with one get_editorial_readiness call using vertical_short, minimum 20 source frames, centered 50/50 focus, nine storyboard frames, and 240-pixel cells. Do not queue export; the benchmark renders and independently transcribes the exact verified snapshot. Keep working until readiness is true.",
        ],
        assertions: vec![
            EvalAssertion::TimelineNonEmpty,
            EvalAssertion::AssetOrder {
                aliases: aliases(&["take-01", "take-03", "take-04"]),
                collapse_adjacent: true,
            },
            EvalAssertion::AssetAbsent {
                alias: "take-02".to_owned(),
            },
            EvalAssertion::AssetAbsent {
                alias: "take-05".to_owned(),
            },
            EvalAssertion::MediaGapless,
            EvalAssertion::WordsRetained {
                word_set: "take-01-recognized-content".to_owned(),
            },
            EvalAssertion::WordsRetained {
                word_set: "take-03-recognized-content".to_owned(),
            },
            EvalAssertion::WordsRetained {
                word_set: "take-04-recognized-content".to_owned(),
            },
            EvalAssertion::WordsAbsent {
                word_set: "selected-recognized-fillers".to_owned(),
            },
            EvalAssertion::WordsAbsent {
                word_set: "authored-exclusions".to_owned(),
            },
            EvalAssertion::NoSilenceAtLeast {
                source_frames: TimeCode(LONG_SILENCE_FRAMES),
            },
            EvalAssertion::DurationBounds {
                bounds: "editorial-cut".to_owned(),
            },
            EvalAssertion::StyledCaptions {
                minimum_cues: 5,
                motion: CaptionMotion::Pop,
            },
            EvalAssertion::CaptionWordsExact {
                word_set: "authored-dialogue".to_owned(),
            },
            EvalAssertion::CaptionSafeArea {
                profile: DeliveryProfile::VerticalShort,
            },
            EvalAssertion::AudioPresent,
            EvalAssertion::QaExportReady,
            required_all(&[
                "get_timeline_state",
                "get_transcripts",
                "plan_dialogue_assembly",
                "add_styled_captions",
                "get_captions",
                "get_editorial_readiness",
            ]),
            required_any(&["commit_edit_plan", "apply_edit_plan"]),
            EvalAssertion::UndoIntegrity,
        ],
        budgets: editorial_cut_budget(),
        deliverable: Some(EvalDeliverableSpec {
            profile: DeliveryProfile::VerticalShort,
            focus_x_percent: 50,
            focus_y_percent: 50,
            proof_frames: 9,
            proof_cell_width: 240,
            require_audio: true,
            expected_transcript_word_set: Some("authored-dialogue"),
            maximum_word_error_rate_basis_points: 1_500,
            maximum_caption_word_error_rate_basis_points: None,
            loudness: None,
            audio_tail: None,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        }),
        color: None,
    }]
}

fn dialogue_pacing_suite() -> Vec<EvalDefinition> {
    let mut definitions = editorial_cut_suite();
    let definition = &mut definitions[0];
    definition.name = "f3 sentence-paced neighborhood garden story";
    definition.rationale = "Measures coherent editorial assembly plus explicit, independently scored sentence-boundary rhythm and sentence-coherent authored captions after filler removal.";
    definition.fixture_builder = fixture_dialogue_pacing_story;
    definition.budgets.max_tool_calls = 24;
    definition.budgets.max_tokens = 350_000;
    definition.prompts = &[
        "Create a finished vertical social story about a neighborhood garden from the five takes. Open exactly these five capability schemas in one get_capability call: get_transcripts, plan_dialogue_assembly, add_styled_captions, get_dialogue_pacing, and get_editorial_readiness. Do not call search_capabilities unless one of those exact lookups fails or a genuinely unnamed need appears. Inspect all five takes in one get_transcripts call. Choose the three takes that form this factual arc: the empty lot collected weeds and rainwater; neighbors turned it into food-growing space by building raised beds and planting tomatoes, herbs, and peppers; the Saturday market now supplies fresh produce to dozens of local families. Reject every flub or factually wrong alternate. Use plan_dialogue_assembly to preserve the clean spoken content in story order, remove all audible um sounds with 3 source frames of filler padding, remove raw dead air at least 20 source frames long, retain 9 source frames around ordinary silence cuts, and cap acoustic pauses around removed filler bridges at 31 source frames without shortening a bridge that is already below that cap. Inspect that planner's prepared_edit_plan preview and commit its returned plan id directly; do not call prepare_edit_plan, manually rewrite, or preemptively retime its source ranges. Keep the real-time A/V source dialogue audible without duplicating it onto an audio track. Use add_styled_captions with the social preset and pop motion, passing this exact intended wording as its script: 'Last spring this empty lot collected weeds and rainwater. Neighbors decided it could feed families instead. Over three weekends volunteers built raised beds. Then they planted tomatoes herbs and peppers. Now the Saturday market supplies fresh produce to dozens of local families.' Trust its deterministic transcript timing and sentence-boundary alignment; do not inspect or manually regroup captions unless the tool reports an alignment error. Verify get_dialogue_pacing with a 10-to-40-project-frame acoustic target and a 4-frame capitalization boundary minimum. When pacing is ready and every gap is target, leave those gaps unchanged because natural variation is desired. Only then finish with one get_editorial_readiness call using vertical_short, minimum 20 source frames, centered 50/50 focus, nine storyboard frames, and 240-pixel cells. Do not queue export; the benchmark renders and independently transcribes the exact verified snapshot. Keep working until both pacing and readiness are true.",
    ];
    definition.assertions.insert(
        12,
        EvalAssertion::DialoguePauseBounds {
            minimum_project_frames: TimeCode(10),
            maximum_project_frames: TimeCode(40),
            capitalization_boundary_minimum_frames: TimeCode(4),
        },
    );
    definition
        .assertions
        .insert(15, EvalAssertion::CaptionSentencesCoherent);
    for assertion in &mut definition.assertions {
        if let EvalAssertion::RequiredToolUsage { all_of, .. } = assertion {
            all_of.retain(|tool| tool != "get_captions");
        }
    }
    let undo = definition.assertions.len().saturating_sub(1);
    definition
        .assertions
        .insert(undo, required_all(&["get_dialogue_pacing"]));
    definitions
}

fn generalization_suite() -> Vec<EvalDefinition> {
    vec![
        EvalDefinition {
            name: "g1 real interview recovery story",
            rationale: "Measures whether the agent can find, clean, caption, frame, and deliver one coherent story from pinned public-domain interview footage it has not seen in the synthetic benchmarks.",
            fixture_builder: fixture_real_interview_story,
            prompts: &[
                "Create a finished vertical social interview cut from interview-raw. Open exactly these four capability schemas in one get_capability call: get_transcript, plan_dialogue_assembly, add_styled_captions, and get_editorial_readiness. Inspect the full source transcript. Build only Helen Hill's coherent first-person story about recovering her films after Hurricane Katrina: begin with her thought starting 'recently I was living in New Orleans' and end after 'Hurricane Katrina films.' Exclude the interview questions, Columbia and South Carolina setup, symposium and festival explanation, the next speaker's response, and the later Dan Streible discussion. Use plan_dialogue_assembly with exactly one half-open source range from frame 1682 through 2547 exclusive, remove conservative fillers and raw dead air of at least 20 source frames, retain 8 source frames around ordinary silence cuts, inspect its prepared plan preview, and commit that exact plan. Do not extend the source range: speech after frame 2547 belongs to the interviewer. Keep the real A/V source dialogue audible without duplicating it onto an audio track. Add social preset captions with pop motion and intent=verbatim, subject_y_percent=50, and this corrected exact script: 'But recently I was living in New Orleans, and my house flooded, and a lot of my films, and especially my recently shot Super 8 home movies, and I've been cleaning them. They deteriorated very quickly in that short, you know, two weeks where they were submerged in those floodwaters. And I've been cleaning them, and they look deteriorated and old even though they're just, you know, maybe 12 months old. So I'm going to be screening just a selection of some of that cleaned flood damage by Hurricane Katrina films.' Preserve that wording exactly; do not silently paraphrase it. Finish with one get_editorial_readiness call using vertical_short, centered 50/50 focus, nine storyboard frames, 240-pixel cells, and a 20-source-frame silence threshold. Do not queue export; the benchmark renders and independently transcribes the verified snapshot. Keep working until readiness is true.",
            ],
            assertions: vec![
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::AssetOrder {
                    aliases: aliases(&["interview-raw"]),
                    collapse_adjacent: true,
                },
                EvalAssertion::ExactSourceClips {
                    clips: vec![ExpectedSourceClip {
                        asset_alias: "interview-raw".to_owned(),
                        source_start: TimeCode(1_682),
                        source_end: TimeCode(2_547),
                    }],
                },
                EvalAssertion::MediaGapless,
                EvalAssertion::WordsRetained {
                    word_set: "recovery-required".to_owned(),
                },
                EvalAssertion::WordsAbsent {
                    word_set: "off-story-exclusions".to_owned(),
                },
                EvalAssertion::DurationBounds {
                    bounds: "recovery-story".to_owned(),
                },
                EvalAssertion::StyledCaptions {
                    minimum_cues: 4,
                    motion: CaptionMotion::Pop,
                },
                EvalAssertion::CaptionPresentation {
                    allowed_positions: vec![TitlePosition::LowerThird, TitlePosition::Top],
                    color_token: 0,
                    background_scrim: false,
                },
                EvalAssertion::CaptionWordsExact {
                    word_set: "recovery-dialogue".to_owned(),
                },
                EvalAssertion::CaptionSentencesCoherent,
                EvalAssertion::CaptionSafeArea {
                    profile: DeliveryProfile::VerticalShort,
                },
                EvalAssertion::AudioPresent,
                EvalAssertion::QaExportReady,
                required_all(&[
                    "get_timeline_state",
                    "get_transcript",
                    "plan_dialogue_assembly",
                    "add_styled_captions",
                    "get_editorial_readiness",
                    "commit_edit_plan",
                ]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: EvalBudgets {
                max_turns: 1,
                max_tool_calls: 20,
                max_operations: 80,
                max_tokens: 300_000,
                max_cost_usd: None,
                max_wall_time: Duration::from_mins(50),
                max_undos: 80,
            },
            deliverable: Some(EvalDeliverableSpec {
                profile: DeliveryProfile::VerticalShort,
                focus_x_percent: 50,
                focus_y_percent: 50,
                proof_frames: 9,
                proof_cell_width: 240,
                require_audio: true,
                expected_transcript_word_set: Some("recovery-dialogue"),
                maximum_word_error_rate_basis_points: 0,
                maximum_caption_word_error_rate_basis_points: Some(0),
                loudness: None,
                audio_tail: None,
                delivery_bit_depth: DeliveryEncodeDepth::Eight,
            }),
            color: None,
        },
        event_multicam_definition(),
        music_montage_definition(),
    ]
}

// ===========================================================================
// CC7 §7 — `color-workflow-v6`, the model path.
//
// Six named colour workflows, one model turn each. Every prompt names the
// clips and the intended outcome and never names a parameter or a value: the
// point of the suite is to find out whether the model can choose them.
//
// Every threshold below is a `kinewright_core::cc7_scenarios` constant, never
// a literal (R-M18), so a re-baseline moves one constant and not six suite
// lines. The technical gates that discharge the same claims without a model
// are ordinary `cargo test` fixtures; this suite adds the model lane on top
// of them and does not replace them.
// ===========================================================================

/// The saved project c3 imports its LUT into. The file need not pre-exist for
/// the store root to resolve, but a fixture that claims "the project is saved"
/// writes it, so the prompt's first sentence is true.
const CC7_PROJECT_FILE_NAME: &str = "cc7-color-workflow.kinewright";

/// The name c3's prompt gives the inverse LUT on disk.
const CC7_LOG_CUBE_FILE_NAME: &str = "log-inverse.cube";

/// CC7 §2.3.4: the candidate clip of a two-clip (a)/(b) document starts where
/// the 60-frame reference ends, so every (a)/(b) measurement is taken there.
fn cc7_candidate_project_frame() -> i64 {
    i64::from(CC7_SOURCE_FRAMES)
}

/// The twelve achromatic chart rectangles (A1), in index order.
fn cc7_chart_patch_rois() -> Vec<NormalizedRoi> {
    CC7_CHART_PATCHES.iter().map(|patch| patch.roi).collect()
}

/// A CC7 basis-point constant in the width the assertion field asks for.
fn cc7_basis_points(value: i64) -> u32 {
    u32::try_from(value).expect("a CC7 basis-point constant is a small positive integer")
}

/// A CC7 pixel-count constant in the width the assertion field asks for.
fn cc7_pixel_count(value: i64) -> u64 {
    u64::try_from(value).expect("a CC7 pixel-count constant is non-negative")
}

/// CC7 §7.4's budget: `EvalBudgets`' field set with CC7's own values, not
/// `standard_budget`'s. The wall time and the operation ceiling are spread
/// over for `c6`, which commits twice and samples five matte frames.
fn color_workflow_budget(max_tool_calls: u32) -> EvalBudgets {
    EvalBudgets {
        max_turns: 1,
        max_tool_calls,
        max_operations: 8,
        max_tokens: 60_000,
        max_cost_usd: Some(2.00),
        max_wall_time: Duration::from_mins(15),
        max_undos: 8,
    }
}

/// CC7 §7.6: every task carries one deliverable at `Eight`. The `Ten` lane
/// for every scenario is exercised by §4(g)'s `cargo test` fixtures rather
/// than by six more model sessions (A10), so the suite's cost stays at one
/// encode per task while the delivery evidence stays complete at both depths.
fn color_workflow_deliverable(depth: DeliveryEncodeDepth) -> EvalDeliverableSpec {
    EvalDeliverableSpec {
        profile: DeliveryProfile::SourceMaster,
        focus_x_percent: 50,
        focus_y_percent: 50,
        proof_frames: 5,
        proof_cell_width: 160,
        require_audio: false,
        expected_transcript_word_set: None,
        maximum_word_error_rate_basis_points: 0,
        maximum_caption_word_error_rate_basis_points: None,
        loudness: None,
        audio_tail: None,
        delivery_bit_depth: depth,
    }
}

/// The colour QC call every scenario makes, as wire tokens.
fn cc7_qc_checks() -> Vec<String> {
    aliases(&["range", "gamut", "tags"])
}

#[allow(clippy::too_many_lines)]
fn color_workflow_suite() -> Vec<EvalDefinition> {
    let c1_assertions = vec![
        EvalAssertion::ReferenceClipUntouched {
            clip_id: CC7_REFERENCE_CLIP_ID.0,
        },
        EvalAssertion::NeutralPatchSpreadAtMost {
            patch_rois: cc7_chart_patch_rois(),
            maximum_code: CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        },
        EvalAssertion::SkinHueWithinBand {
            roi: CC7_SKIN_BAND_ROI,
            minimum_in_band_basis_points: cc7_basis_points(CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS),
        },
        EvalAssertion::ColorQcTechnicalPass {
            clip_id: CC7_CANDIDATE_CLIP_ID.0,
            frame: cc7_candidate_project_frame(),
            checks: cc7_qc_checks(),
        },
        EvalAssertion::DeliveryVerificationWithinBudgets {
            depth: DeliveryEncodeDepth::Eight,
        },
        required_all(&[
            "analyze_color_shot",
            "plan_shot_match",
            "prepare_edit_plan",
            "commit_edit_plan",
            "render_color_proof",
            "get_color_qc",
        ]),
        EvalAssertion::UndoIntegrity,
    ];
    let mut c1_color =
        ColorEvalRequest::from_assertions(&c1_assertions).expect("c1 gates colour assertions");
    // Evidence no variant reads: the chart-band luma mean difference between
    // the reference clip's frame and the candidate clip's (§4(a)(3)).
    c1_color.chart_luma_roi = Some(CC7_CHART_BAND_ROI);
    c1_color.chart_luma_reference_frame = 0;
    c1_color.chart_luma_candidate_frame = cc7_candidate_project_frame();

    let c2_assertions = vec![
        EvalAssertion::SkinHueWithinBand {
            roi: CC7_SKIN_BAND_ROI,
            // The corrected C2 clip is the one scenario whose skin band is
            // NOT exactly 10 000 bp: probe-2 measured 9 411, still above
            // `SKIN_BAND_EXCEPTION_BASIS_POINTS`, and that residual is the
            // compromise the human is asked about (§4(b)(3)).
            minimum_in_band_basis_points: cc7_basis_points(
                CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS,
            ),
        },
        EvalAssertion::ColorQcTechnicalPass {
            clip_id: CC7_CANDIDATE_CLIP_ID.0,
            frame: cc7_candidate_project_frame(),
            // (b2) is the one task that asks for per-node attribution, so the
            // range Warning can be attributed to the correction node.
            checks: aliases(&["range", "gamut", "tags", "per_node"]),
        },
        EvalAssertion::DeliveryVerificationWithinBudgets {
            depth: DeliveryEncodeDepth::Eight,
        },
        required_all(&[
            "plan_shot_match",
            "prepare_edit_plan",
            "commit_edit_plan",
            "get_color_qc",
        ]),
        EvalAssertion::UndoIntegrity,
    ];
    let mut c2_color =
        ColorEvalRequest::from_assertions(&c2_assertions).expect("c2 gates colour assertions");
    c2_color.chart_luma_roi = Some(CC7_CHART_BAND_ROI);
    c2_color.chart_luma_reference_frame = 0;
    c2_color.chart_luma_candidate_frame = cc7_candidate_project_frame();

    let c3_assertions = vec![
        EvalAssertion::ColorQcTechnicalPass {
            clip_id: CC7_SINGLE_CLIP_ID.0,
            frame: 0,
            checks: cc7_qc_checks(),
        },
        EvalAssertion::DeliveryVerificationWithinBudgets {
            depth: DeliveryEncodeDepth::Eight,
        },
        required_all(&[
            "import_lut_asset",
            "list_look_assets",
            "plan_technical_lut",
            "prepare_edit_plan",
            "commit_edit_plan",
            "get_color_qc",
        ]),
        EvalAssertion::UndoIntegrity,
    ];
    let c3_color =
        ColorEvalRequest::from_assertions(&c3_assertions).expect("c3 gates colour assertions");

    let c4_assertions = vec![
        EvalAssertion::MatteContainmentExact {
            roi: CC7_PRODUCT_RED_ROI,
            expected_covered_pixel_count: u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT),
            expected_full_pixel_count: u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT),
            // CC7 names its own zero here: `CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX`
            // is the "no tolerance may excuse one pixel" constant of §4(d)(2),
            // and (d)'s containment is exact for the same reason.
            expected_partial_pixel_count: cc7_pixel_count(CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX),
        },
        EvalAssertion::SkinHueWithinBand {
            roi: CC7_SKIN_BAND_ROI,
            minimum_in_band_basis_points: cc7_basis_points(CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS),
        },
        EvalAssertion::ColorQcTechnicalPass {
            clip_id: CC7_SINGLE_CLIP_ID.0,
            frame: 0,
            checks: cc7_qc_checks(),
        },
        EvalAssertion::DeliveryVerificationWithinBudgets {
            depth: DeliveryEncodeDepth::Eight,
        },
        required_all(&[
            "plan_secondary_correction",
            "prepare_edit_plan",
            "commit_edit_plan",
            "inspect_grade_matte",
            "get_color_qc",
        ]),
        EvalAssertion::UndoIntegrity,
    ];
    // `matte_node` is deliberately left `None`: the runner resolves the one
    // node whose `matte_enabled` is non-zero, which does not assume an effect
    // id the model did not choose.
    let c4_color =
        ColorEvalRequest::from_assertions(&c4_assertions).expect("c4 gates colour assertions");

    let c5_assertions = vec![
        EvalAssertion::LookBypassMatchesAbsent {
            clip_id: CC7_SINGLE_CLIP_ID.0,
            effect_id: CC7_NODE_EFFECT_ID.0,
            frame: 0,
        },
        EvalAssertion::ColorQcTechnicalPass {
            clip_id: CC7_SINGLE_CLIP_ID.0,
            frame: 0,
            checks: cc7_qc_checks(),
        },
        EvalAssertion::DeliveryVerificationWithinBudgets {
            depth: DeliveryEncodeDepth::Eight,
        },
        required_all(&[
            "plan_creative_look",
            "prepare_edit_plan",
            "commit_edit_plan",
            "render_color_proof",
            "get_color_qc",
            "list_look_assets",
        ]),
        EvalAssertion::UndoIntegrity,
    ];
    let mut c5_color =
        ColorEvalRequest::from_assertions(&c5_assertions).expect("c5 gates colour assertions");
    // Evidence no variant reads: the `deep_shadow` patch's out-of-gamut
    // population, which is exact at 192 px under the `warm` look (§4(e)(2)).
    c5_color.gamut_roi = Some(CC7_DEEP_SHADOW_ROI);

    let c6_assertions = vec![
        EvalAssertion::TrackKeyframesMatchExpected {
            parameter: CC7_F_KEYFRAMED_PARAMETERS[0].to_owned(),
            expected_local_frames: CC7_TRACK_SURVIVING_SAMPLE_FRAMES.to_vec(),
            absent_local_frames: CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES.to_vec(),
        },
        EvalAssertion::TrackKeyframesMatchExpected {
            parameter: CC7_F_KEYFRAMED_PARAMETERS[1].to_owned(),
            expected_local_frames: CC7_TRACK_SURVIVING_SAMPLE_FRAMES.to_vec(),
            absent_local_frames: CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES.to_vec(),
        },
        EvalAssertion::ColorQcTechnicalPass {
            clip_id: CC7_SINGLE_CLIP_ID.0,
            frame: 0,
            checks: cc7_qc_checks(),
        },
        EvalAssertion::DeliveryVerificationWithinBudgets {
            depth: DeliveryEncodeDepth::Eight,
        },
        required_all(&[
            "plan_secondary_correction",
            "track_matte_window",
            "prepare_edit_plan",
            "commit_edit_plan",
            "inspect_grade_matte",
        ]),
        EvalAssertion::UndoIntegrity,
    ];
    let c6_color =
        ColorEvalRequest::from_assertions(&c6_assertions).expect("c6 gates colour assertions");

    vec![
        EvalDefinition {
            name: "c1 Mixed-camera interview match",
            rationale: "Measures whether the model can match a second camera to an approved reference without touching the reference, leaving the intentional skin difference intact and the chart neutral.",
            fixture_builder: fixture_cc7_mixed_camera,
            prompts: &[CC7_PROMPT_C1],
            assertions: c1_assertions,
            budgets: color_workflow_budget(16),
            deliverable: Some(color_workflow_deliverable(DeliveryEncodeDepth::Eight)),
            color: Some(c1_color),
        },
        EvalDefinition {
            name: "c2 Wrong white balance and underexposure",
            rationale: "Measures whether the model recovers a badly balanced, badly underexposed take as far as the primary controls allow, says plainly where it ran out of authority, and checks the recovered clip against the delivery range.",
            fixture_builder: fixture_cc7_white_balance,
            prompts: &[CC7_PROMPT_C2],
            assertions: c2_assertions,
            budgets: color_workflow_budget(16),
            deliverable: Some(color_workflow_deliverable(DeliveryEncodeDepth::Eight)),
            color: Some(c2_color),
        },
        EvalDefinition {
            name: "c3 Log-like input normalisation",
            rationale: "Measures whether the model imports a LUT from disk into a saved project's store and binds it at the input stage, rather than reaching for a creative look or a primary correction.",
            fixture_builder: fixture_cc7_log_like,
            prompts: &[CC7_PROMPT_C3],
            assertions: c3_assertions,
            budgets: color_workflow_budget(16),
            deliverable: Some(color_workflow_deliverable(DeliveryEncodeDepth::Eight)),
            color: Some(c3_color),
        },
        EvalDefinition {
            name: "c4 Product and skin secondary",
            rationale: "Measures whether the model isolates one product patch with a qualifier, shows which pixels it affected, and proves the skin tones it did not touch are unchanged.",
            fixture_builder: fixture_cc7_product_and_skin,
            prompts: &[CC7_PROMPT_C4],
            assertions: c4_assertions,
            budgets: color_workflow_budget(16),
            deliverable: Some(color_workflow_deliverable(DeliveryEncodeDepth::Eight)),
            color: Some(c4_color),
        },
        EvalDefinition {
            name: "c5 Creative look with a gamut exception",
            rationale: "Measures whether the model applies a built-in look, proves the bypass is the lossless twin of absent, and reports the gamut excursion the look deliberately creates.",
            fixture_builder: fixture_cc7_creative_look,
            prompts: &[CC7_PROMPT_C5],
            assertions: c5_assertions,
            budgets: color_workflow_budget(16),
            deliverable: Some(color_workflow_deliverable(DeliveryEncodeDepth::Eight)),
            color: Some(c5_color),
        },
        EvalDefinition {
            name: "c6 Tracked secondary through an occlusion",
            rationale: "Measures whether the model tracks a moving subject, stops at the occlusion rather than inventing a position it did not measure, and says which samples it could not trust.",
            fixture_builder: fixture_cc7_tracked_secondary,
            prompts: &[CC7_PROMPT_C6],
            assertions: c6_assertions,
            budgets: EvalBudgets {
                max_tool_calls: 24,
                max_operations: 24,
                max_undos: 24,
                max_wall_time: Duration::from_mins(25),
                ..color_workflow_budget(24)
            },
            deliverable: Some(color_workflow_deliverable(DeliveryEncodeDepth::Eight)),
            color: Some(c6_color),
        },
    ]
}

// ---------------------------------------------------------------------------
// CC7 §7.3 — the prompts, one user turn each.
//
// Each names the clips and the intended outcome and never names a parameter
// or a value: the point of the suite is to find out whether the model can
// choose them.
// ---------------------------------------------------------------------------

const CC7_PROMPT_C1: &str = "Clips 1 and 2 are the same interview shot from two cameras. Clip 1 is the reference the colourist approved. Match clip 2 to it so the neutral chart in both reads neutral and the two cut together, leave clip 1 exactly as it is, then show me a proof of the corrected clip and confirm the skin in clip 2 still reads as skin.";

const CC7_PROMPT_C2: &str = "Clip 2 was shot on the wrong white-balance preset and badly underexposed; clip 1 is a correctly balanced take of the same scene. Recover clip 2 as far as the primary controls allow, tell me plainly where you ran out of authority, and check whether the recovered clip now pushes anything outside the delivery range.";

const CC7_PROMPT_C3: &str = "Clip 1 was recorded on a flat, log-like curve and looks washed out. The project is saved, and a matching inverse LUT is on disk beside it as `log-inverse.cube`. Normalise clip 1 back to a correct Rec.709 rendering using that file, then show me the result and a QC report.";

const CC7_PROMPT_C4: &str = "On clip 1, make the red product read richer without touching anyone's skin. Show me exactly which pixels you affected, and prove the skin tones are unchanged.";

const CC7_PROMPT_C5: &str = "Give clip 1 a warm evening look using one of the built-in looks. Show me before, after, and the look bypassed at the same frame, and tell me whether the look pushes any pixels out of gamut.";

const CC7_PROMPT_C6: &str = "On clip 1, isolate the moving red product with a tracked secondary and lift its saturation. Something crosses in front of it partway through — do not invent a position you did not measure, and tell me which samples you could not trust.";

// ---------------------------------------------------------------------------
// CC7 §7.2 — the six fixture builders.
//
// Every builder imports its media from `kinewright_media::cc7_sources`, the
// same generator the technical gates use, so a raster cannot drift between
// the `cargo test` claim and the model claim made about it.
// ---------------------------------------------------------------------------

/// A CC7 document: one clip per generated source, in timeline order, at
/// CC7's own 25 fps and 320 x 180 rather than the synthetic suites' 30 fps.
fn cc7_timeline_document(assets: Vec<MediaAsset>) -> Result<Document, EvalError> {
    let fps =
        Rational::new(CC7_SOURCE_FPS, 1).map_err(|error| EvalError::Fixture(error.to_string()))?;
    let mut timeline_start = TimeCode::ZERO;
    let mut clips = Vec::with_capacity(assets.len());
    for (index, asset) in assets.iter().enumerate() {
        let source_range = TimeCode::ZERO..asset.duration;
        let duration = map_source_range_to_project(source_range.clone(), asset.fps, fps)
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        clips.push(Clip {
            id: ClipId(u64::try_from(index + 1).unwrap_or(u64::MAX)),
            asset: asset.id,
            source_range,
            content: ClipContent::Media,
            timeline_start,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        });
        timeline_start = timeline_start
            .checked_add(duration)
            .ok_or_else(|| EvalError::Fixture("a CC7 document duration overflowed".to_owned()))?;
    }
    Ok(Document {
        catalog: MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips,
        }],
        media_pool: assets,
        markers: Vec::new(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        fps,
        resolution: (CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT),
        duration: timeline_start,
    })
}

/// Probe every generated CC7 source, alias it, and build its document.
fn cc7_fixture_from_sources(
    media: &Arc<FfmpegMediaEngine>,
    sources: &[(&str, &GeneratedMedia)],
) -> Result<(Document, FixtureContext), EvalError> {
    let mut assets = Vec::with_capacity(sources.len());
    let mut context = FixtureContext::default();
    for (alias, generated) in sources {
        let asset = probe_named(media, generated.path(), alias)?;
        context.asset_aliases.insert((*alias).to_owned(), asset.id);
        assets.push(asset);
    }
    let document = cc7_timeline_document(assets)?;
    Ok((document, context))
}

fn fixture_cc7_mixed_camera() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let reference = cc7_camera_source(Cc7Camera::A);
    let candidate = cc7_camera_source(Cc7Camera::B);
    let (document, context) = cc7_fixture_from_sources(
        &media,
        &[("cc7-camera-a", &reference), ("cc7-camera-b", &candidate)],
    )?;
    PreparedFixture::new(
        document,
        media,
        context,
        None,
        vec![Box::new(reference), Box::new(candidate)],
    )
}

fn fixture_cc7_white_balance() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let reference = cc7_camera_source(Cc7Camera::A);
    // Scenario (b)'s committed document is (b2)'s, so the candidate is C2 —
    // the take that is beyond the planner's authority (§2.5).
    let candidate = cc7_camera_source(Cc7Camera::C2);
    let (document, context) = cc7_fixture_from_sources(
        &media,
        &[("cc7-camera-a", &reference), ("cc7-camera-c2", &candidate)],
    )?;
    PreparedFixture::new(
        document,
        media,
        context,
        None,
        vec![Box::new(reference), Box::new(candidate)],
    )
}

/// The one fixture that saves a project.
///
/// `import_lut_asset` refuses `project_not_saved` until the server carries a
/// project-path handle (R-B2), so c3 saves the document into its own
/// temporary directory and writes the inverse `.cube` beside it under the
/// name the prompt gives. Without both, c3's prompt is unsatisfiable.
fn fixture_cc7_log_like() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let carrier = cc7_log_source();
    let (document, context) = cc7_fixture_from_sources(&media, &[("cc7-log-carrier", &carrier)])?;

    let temporary = TempDirectory::new("cc7-log-project");
    let project_path = temporary.path(CC7_PROJECT_FILE_NAME);
    let generated_cube = write_log_like_inverse_cube(temporary.root(), CC7_LOG_CUBE_SIZE);
    let cube_path = temporary.path(CC7_LOG_CUBE_FILE_NAME);
    fs::rename(&generated_cube, &cube_path).map_err(|error| {
        EvalError::Fixture(format!(
            "the CC7 inverse cube could not be named {CC7_LOG_CUBE_FILE_NAME}: {error}"
        ))
    })?;
    let saved = serde_json::to_vec_pretty(&document)
        .map_err(|error| EvalError::Fixture(error.to_string()))?;
    fs::write(&project_path, saved).map_err(|error| {
        EvalError::Fixture(format!(
            "the CC7 project could not be saved at {}: {error}",
            project_path.display()
        ))
    })?;

    PreparedFixture::new(
        document,
        media,
        context,
        Some(project_path),
        vec![Box::new(carrier), Box::new(temporary)],
    )
}

fn fixture_cc7_product_and_skin() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let source = cc7_camera_source(Cc7Camera::A);
    let (document, context) = cc7_fixture_from_sources(&media, &[("cc7-camera-a", &source)])?;
    PreparedFixture::new(document, media, context, None, vec![Box::new(source)])
}

fn fixture_cc7_creative_look() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let source = cc7_camera_source(Cc7Camera::A);
    let (document, context) = cc7_fixture_from_sources(&media, &[("cc7-camera-a", &source)])?;
    PreparedFixture::new(document, media, context, None, vec![Box::new(source)])
}

fn fixture_cc7_tracked_secondary() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let source = cc7_tracked_source();
    let (document, context) = cc7_fixture_from_sources(&media, &[("cc7-tracked", &source)])?;
    PreparedFixture::new(document, media, context, None, vec![Box::new(source)])
}

fn aliases(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn required_all(tools: &[&str]) -> EvalAssertion {
    EvalAssertion::RequiredToolUsage {
        all_of: aliases(tools),
        any_of: Vec::new(),
    }
}

fn required_any(tools: &[&str]) -> EvalAssertion {
    EvalAssertion::RequiredToolUsage {
        all_of: Vec::new(),
        any_of: aliases(tools),
    }
}

fn standard_budget(max_operations: u32, max_undos: u32) -> EvalBudgets {
    EvalBudgets {
        max_turns: 1,
        max_tool_calls: 16,
        max_operations,
        max_tokens: 30_000,
        // The first session of any batch pays a ~3x cold-cache billing
        // premium, and in full-suite runs that lands on whichever eval runs
        // first. The ceiling catches runaways, not positional billing
        // variance (observed: a correct $0.40-class e1 billed $1.67 as the
        // suite opener).
        max_cost_usd: Some(2.00),
        max_wall_time: Duration::from_mins(5),
        max_undos,
    }
}

fn speech_budget(max_operations: u32, max_undos: u32) -> EvalBudgets {
    EvalBudgets {
        max_wall_time: Duration::from_mins(25),
        ..standard_budget(max_operations, max_undos)
    }
}

fn flagship_budget() -> EvalBudgets {
    EvalBudgets {
        max_turns: 1,
        max_tool_calls: 36,
        max_operations: 30,
        max_tokens: 70_000,
        // Calibrated from 20 live samples: correct runs cluster at ~$0.60-0.70
        // with one ~$2.00 cold-cache-priced outlier per batch. The ceiling
        // catches runaways, not billing variance.
        max_cost_usd: Some(2.50),
        max_wall_time: Duration::from_mins(40),
        max_undos: 30,
    }
}

fn finished_cut_budget() -> EvalBudgets {
    EvalBudgets {
        max_turns: 1,
        max_tool_calls: 64,
        max_operations: 80,
        // Codex reports cumulative input across its tool loop. Two calibration
        // runs used 553,648 and 731,311 tokens, so this is a measured upper
        // guard rather than a single-response token assumption.
        max_tokens: 750_000,
        // Subscription Codex exposes tokens but no attributable USD cost.
        max_cost_usd: None,
        max_wall_time: Duration::from_mins(50),
        max_undos: 80,
    }
}

fn editorial_cut_budget() -> EvalBudgets {
    EvalBudgets {
        max_turns: 1,
        max_tool_calls: 48,
        max_operations: 80,
        max_tokens: 500_000,
        max_cost_usd: None,
        max_wall_time: Duration::from_mins(50),
        max_undos: 80,
    }
}

fn fixture_e1() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let generated = generated_video(
        "e1-take",
        "testsrc2=size=320x180:rate=30:duration=6",
        6,
        440,
    );
    let asset = probe_named(&media, generated.path(), "take")?;
    let document = timeline_document(
        vec![asset.clone()],
        &[
            (0, TimeCode(0)..TimeCode(90)),
            (0, TimeCode(90)..TimeCode(150)),
        ],
    )?;
    let mut context = FixtureContext::default();
    context.asset_aliases.insert("take".to_owned(), asset.id);
    PreparedFixture::new(document, media, context, None, vec![Box::new(generated)])
}

fn fixture_e2() -> Result<PreparedFixture, EvalError> {
    const SPEECH: &str = "<speak version='1.0' xml:lang='en-US'>Alpha.<break time='1400ms'/>Bravo.<break time='1400ms'/>Charlie.</speak>";
    let media = eval_engine();
    let generated = SpeechClip::ssml("eval-e2-silence", SPEECH, "KINEWRIGHT_EVAL_E2_AUDIO");
    let asset = probe_named(&media, &generated.mp4, "silence-take")?;
    media.request_transcription(asset.clone());
    media.request_silence_detection(asset.clone());
    let transcript = wait_for_transcript_result(media.as_ref(), &asset)?;
    let silences = wait_for_silences(media.as_ref(), &asset)?;
    let long_silence = silence_frames(&silences, LONG_SILENCE_FRAMES);
    if long_silence == 0 {
        return Err(EvalError::Fixture(format!(
            "e2 generated no silence spans at least {LONG_SILENCE_FRAMES} frames"
        )));
    }
    let document = single_asset_document(asset.clone());
    let mut context = FixtureContext::default();
    context
        .asset_aliases
        .insert("silence-take".to_owned(), asset.id);
    context.word_sets.insert(
        "spoken-content".to_owned(),
        normalized_words(&joined_words(&transcript)),
    );
    let maximum = maximum_duration_after_expected_silence_cuts(
        asset.duration,
        &silences,
        Some(transcript.as_ref()),
        TimeCode(LONG_SILENCE_FRAMES),
    );
    context.transcripts.insert(asset.id, transcript);
    let minimum = TimeCode(maximum.0.saturating_mul(2) / 5);
    context
        .duration_bounds
        .insert("without-long-silence".to_owned(), (minimum, maximum));
    PreparedFixture::new(document, media, context, None, vec![Box::new(generated)])
}

fn fixture_e3() -> Result<PreparedFixture, EvalError> {
    const SPEECH: &str = "Hello, um, this is an Kinewright filler word evaluation.";
    let media = eval_engine();
    let generated = SpeechClip::plain("eval-e3-filler", SPEECH, "KINEWRIGHT_EVAL_E3_AUDIO");
    let asset = probe_named(&media, &generated.mp4, "filler-take")?;
    media.request_transcription(asset.clone());
    let transcript = wait_for_transcript_result(media.as_ref(), &asset)?;
    let words = normalized_words(&joined_words(&transcript));
    let (fillers, kept_words) = partition_fillers(&words);
    if fillers.is_empty() || kept_words.is_empty() {
        return Err(EvalError::Fixture(format!(
            "e3 ASR must recognize filler and content words; heard {words:?}"
        )));
    }
    let document = single_asset_document(asset.clone());
    let mut context = FixtureContext::default();
    context
        .asset_aliases
        .insert("filler-take".to_owned(), asset.id);
    context
        .word_sets
        .insert("recognized-fillers".to_owned(), fillers);
    context
        .word_sets
        .insert("non-filler".to_owned(), kept_words);
    PreparedFixture::new(document, media, context, None, vec![Box::new(generated)])
}

fn fixture_e4() -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let generated = GeneratedMedia::ffmpeg(
        "eval-e4-scenes",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=320x180:rate=30:duration=2",
            "-f",
            "lavfi",
            "-i",
            "color=c=green:size=320x180:rate=30:duration=2",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:size=320x180:rate=30:duration=2",
            "-filter_complex",
            "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v]",
            "-map",
            "[v]",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
        "mp4",
    );
    let asset = probe_named(&media, generated.path(), "scene-take")?;
    media.request_scene_detection(asset.clone());
    let scenes = wait_for_scenes(media.as_ref(), &asset)?;
    let scene_set = scenes
        .changes
        .iter()
        .filter(|change| change.confidence_basis_points >= SCENE_CONFIDENCE_BASIS_POINTS)
        .map(|change| (asset.id, change.source_frame))
        .collect::<Vec<_>>();
    if scene_set.is_empty() {
        return Err(EvalError::Fixture(
            "e4 scene detector found no qualifying hard cuts".to_owned(),
        ));
    }
    let document = single_asset_document(asset.clone());
    let mut context = FixtureContext::default();
    context
        .asset_aliases
        .insert("scene-take".to_owned(), asset.id);
    context
        .scene_sets
        .insert("detected-scenes".to_owned(), scene_set);
    PreparedFixture::new(document, media, context, None, vec![Box::new(generated)])
}

fn fixture_e5() -> Result<PreparedFixture, EvalError> {
    fixture_distinct_timeline("e5", &["first", "second"], &["testsrc", "smptebars"])
}

fn fixture_e6() -> Result<PreparedFixture, EvalError> {
    fixture_distinct_timeline(
        "e6",
        &["first", "second", "third"],
        &["testsrc2", "rgbtestsrc", "yuvtestsrc"],
    )
}

#[allow(clippy::too_many_lines)]
fn fixture_e7() -> Result<PreparedFixture, EvalError> {
    let takes = [
        (
            "take-A",
            "<speak version='1.0' xml:lang='en-US'>Aurora opens the story.<break time='1300ms'/>Um, copper lanterns guide the morning crew.</speak>",
            "testsrc2",
        ),
        (
            "take-B",
            "<speak version='1.0' xml:lang='en-US'>Badger quartz archives describe the discarded route.<break time='1200ms'/>This take stays out.</speak>",
            "smptebars",
        ),
        (
            "take-C",
            "<speak version='1.0' xml:lang='en-US'>Cobalt carries the middle passage.<break time='1400ms'/>Uh, river maps steady the expedition.</speak>",
            "rgbtestsrc",
        ),
        (
            "take-D",
            "<speak version='1.0' xml:lang='en-US'>Delta closes the journey.<break time='1300ms'/>Um, silver beacons welcome the crew home.</speak>",
            "yuvtestsrc",
        ),
        (
            "take-E",
            "<speak version='1.0' xml:lang='en-US'>Ember violet notes belong to the alternate ending.<break time='1200ms'/>Leave this take unused.</speak>",
            "testsrc",
        ),
    ];
    let media = eval_engine();
    let mut resources: Vec<Box<dyn Send>> = Vec::new();
    let mut assets = Vec::new();
    for (index, (name, speech, pattern)) in takes.iter().enumerate() {
        let environment_name = format!("KINEWRIGHT_EVAL_TAKE_{}_AUDIO", index + 1);
        let generated = patterned_speech(name, speech, pattern, &environment_name);
        let asset = probe_named(&media, generated.path(), name)?;
        media.request_transcription(asset.clone());
        media.request_silence_detection(asset.clone());
        assets.push(asset);
        resources.push(Box::new(generated));
    }
    let mut transcripts = BTreeMap::new();
    let mut silences = BTreeMap::new();
    for asset in &assets {
        transcripts.insert(asset.id, wait_for_transcript_result(media.as_ref(), asset)?);
        silences.insert(asset.id, wait_for_silences(media.as_ref(), asset)?);
    }
    let mut context = FixtureContext::default();
    for ((name, _, _), asset) in takes.iter().zip(&assets) {
        context.asset_aliases.insert((*name).to_owned(), asset.id);
    }
    context.transcripts.extend(
        transcripts
            .iter()
            .map(|(asset, transcript)| (*asset, Arc::clone(transcript))),
    );
    let selected = [0_usize, 2, 3];
    let mut selected_union = BTreeSet::new();
    let mut selected_fillers = BTreeSet::new();
    let mut selected_maximum_duration = 0_i64;
    for index in selected {
        let asset = &assets[index];
        let words = normalized_words(&joined_words(&transcripts[&asset.id]));
        let (fillers, kept_words) = partition_fillers(&words);
        if kept_words.is_empty() {
            return Err(EvalError::Fixture(format!(
                "{} ASR produced no content words: {words:?}",
                takes[index].0
            )));
        }
        selected_union.extend(kept_words.iter().cloned());
        selected_fillers.extend(fillers);
        context
            .word_sets
            .insert(format!("{}-content", takes[index].0), kept_words);
        selected_maximum_duration = selected_maximum_duration.saturating_add(
            maximum_duration_after_expected_silence_cuts(
                asset.duration,
                &silences[&asset.id],
                Some(transcripts[&asset.id].as_ref()),
                TimeCode(LONG_SILENCE_FRAMES),
            )
            .0,
        );
    }
    if selected_fillers.is_empty() {
        return Err(EvalError::Fixture(
            "e7 ASR recognized no filler words in the selected takes".to_owned(),
        ));
    }
    context.word_sets.insert(
        "selected-fillers".to_owned(),
        selected_fillers.into_iter().collect(),
    );
    let take_b_words = normalized_words(&joined_words(&transcripts[&assets[1].id]));
    let take_b_unique = take_b_words
        .into_iter()
        .filter(|word| !selected_union.contains(word) && !is_filler(word))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if take_b_unique.is_empty() {
        return Err(EvalError::Fixture(
            "e7 take-B has no ASR-relative unique words".to_owned(),
        ));
    }
    context
        .word_sets
        .insert("take-B-unique".to_owned(), take_b_unique);
    let maximum = TimeCode(selected_maximum_duration);
    let minimum = TimeCode(maximum.0.saturating_mul(2) / 5);
    context
        .duration_bounds
        .insert("rough-cut".to_owned(), (minimum, maximum));
    let document = empty_timeline_document(assets);
    PreparedFixture::new(document, media, context, None, resources)
}

#[derive(Debug, Deserialize)]
struct EditorialGroundTruth {
    schema_version: u32,
    story_id: String,
    expected_take_order: Vec<String>,
    expected_dialogue: String,
    excluded_words: Vec<String>,
    takes: Vec<EditorialTake>,
}

#[derive(Debug, Deserialize)]
struct EditorialTake {
    id: String,
    role: String,
    visual: String,
    accepted: bool,
    ssml: String,
}

#[derive(Debug, Deserialize)]
struct RealInterviewGroundTruth {
    schema_version: u32,
    story_id: String,
    asset_id: String,
    source_story_range: GroundTruthRange,
    duration_bounds_project_frames: GroundTruthDurationBounds,
    required_terms: Vec<String>,
    excluded_terms: Vec<String>,
    source_asr_dialogue: String,
    expected_dialogue: String,
}

#[derive(Debug, Deserialize)]
struct EventMulticamGroundTruth {
    schema_version: u32,
    meeting_id: String,
    reference_asset_id: String,
    audio_master_asset_id: String,
    source_range: GroundTruthRange,
    audio_source_range: GroundTruthRange,
    speaker_assignments: Vec<EventSpeakerAssignment>,
    turns: Vec<EventSpeakerTurn>,
    expected_video_cuts: Vec<EventExpectedCut>,
    reframe: EventReframeTruth,
    expected_dialogue: String,
}

#[derive(Debug, Deserialize)]
struct EventSpeakerAssignment {
    speaker: String,
    angle_asset_id: String,
    angle_name: String,
}

#[derive(Debug, Deserialize)]
struct EventSpeakerTurn {
    speaker: String,
    start: i64,
    end: i64,
    text: String,
}

#[derive(Debug, Deserialize)]
struct EventExpectedCut {
    angle_asset_id: String,
    timeline_start: i64,
    timeline_end: i64,
    source_start: i64,
    source_end: i64,
}

#[derive(Debug, Deserialize)]
struct EventReframeTruth {
    target_aspect_basis_points: i64,
    minimum_keyframes_per_axis: usize,
    min_x_percent: i64,
    max_x_percent: i64,
    min_y_percent: i64,
    max_y_percent: i64,
    maximum_step_percent: i64,
}

#[derive(Debug, Deserialize)]
struct MusicMontageGroundTruth {
    schema_version: u32,
    montage_id: String,
    project_fps: Rational,
    project_resolution: GroundTruthResolution,
    timeline_range: GroundTruthRange,
    video_track_id: u64,
    audio_track_id: u64,
    visual_asset_ids: Vec<String>,
    music_asset_id: String,
    minimum_visual_shots: usize,
    maximum_visual_shots: usize,
    minimum_shot_frames: i64,
    maximum_shot_frames: i64,
    minimum_beat_strength_percent: f64,
    beat_alignment_tolerance_frames: i64,
    minimum_visual_assets_used: usize,
    minimum_clips_per_visual_asset: usize,
    minimum_project_frames_per_visual_asset: i64,
    latest_early_start_per_visual_asset: i64,
    earliest_late_start_per_visual_asset: i64,
    minimum_source_separation_frames: i64,
    minimum_forward_gap_frames: i64,
    source_scene_minimum_confidence_percent: f64,
    source_exclusions: Vec<GroundTruthSourceExclusion>,
    minimum_duration_buckets: usize,
    duration_bucket_frames: i64,
    maximum_similar_run: usize,
    similar_tolerance_frames: i64,
    minimum_clean_selectable_project_frames: i64,
    selection_headroom_frames: i64,
    meter_beats: u8,
    phrase_bars: u8,
    reviewed_music_events: Vec<ReviewedMusicEvent>,
    reviewed_story_events: Vec<ReviewedMusicEvent>,
    title_card: GroundTruthTitleCard,
    opening_source_window: GroundTruthRange,
    preparation_source_window: GroundTruthRange,
    operator_source_window: GroundTruthRange,
    activation_source_window: GroundTruthRange,
    climax_source_window: GroundTruthRange,
    music_preferred_source_start: i64,
    music_preferred_source_end: i64,
    music_maximum_end_drift_frames: i64,
    minimum_opening_shot_frames: i64,
    minimum_closing_shot_frames: i64,
    rendered_tail_window_frames: i64,
    rendered_tail_maximum_peak_dbfs_hundredths: i32,
    rendered_activity_window_frames: i64,
    rendered_activity_minimum_integrated_lufs_hundredths: i32,
    maximum_trailing_inactive_frames: i64,
    story_brief: String,
    attribution: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewedMusicEvent {
    project_frame: i64,
    role: String,
}

#[derive(Debug, Deserialize)]
struct GroundTruthTitleCard {
    track_id: u64,
    timeline_range: GroundTruthRange,
    text: String,
    font_size_token: u8,
    color_token: u8,
    position: TitlePosition,
    background_scrim: bool,
    fade_in_frames: i64,
    fade_out_frames: i64,
}

fn music_fixture_alias(fixture_id: &str) -> Option<&'static str> {
    match fixture_id {
        "sintel-trailer-1080p" => Some("sintel"),
        "tears-of-steel-battle-720p" => Some("tears-of-steel"),
        "uprising-scott-buckley" | "vanguard-scott-buckley" => Some("music-bed"),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct GroundTruthSourceExclusion {
    asset_id: String,
    source_range: GroundTruthRange,
    reason: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct GroundTruthResolution {
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct GroundTruthRange {
    start: i64,
    end: i64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct GroundTruthDurationBounds {
    minimum: i64,
    maximum: i64,
}

/// Return the project-frame duration that can be selected from source ranges
/// which are both scene-clean and outside the manually reviewed exclusions.
///
/// A source scene is an indivisible editorial unit for the montage planner, so
/// each candidate interval is split at detected boundaries before exclusions
/// are subtracted. Intervals shorter than the minimum shot length are not
/// counted: they cannot satisfy the active shot contract even if their frames
/// are otherwise clean.
fn clean_selectable_project_frames(
    assets: &[MediaAsset],
    project_fps: Rational,
    scene_boundaries: &[(AssetId, TimeCode)],
    exclusions: &[SourceRangeExclusion],
    minimum_shot_frames: TimeCode,
) -> Result<i64, EvalError> {
    if minimum_shot_frames <= TimeCode::ZERO {
        return Err(EvalError::Fixture(
            "music source feasibility requires a positive minimum shot length".to_owned(),
        ));
    }
    let mut selectable_project_frames = 0_i64;
    for asset in assets
        .iter()
        .filter(|asset| asset.kind.supports(TrackKind::Video))
    {
        let mut boundaries = vec![TimeCode::ZERO, asset.duration];
        boundaries.extend(
            scene_boundaries
                .iter()
                .filter(|(asset_id, frame)| {
                    *asset_id == asset.id && *frame > TimeCode::ZERO && *frame < asset.duration
                })
                .map(|(_, frame)| *frame),
        );
        boundaries.sort_unstable();
        boundaries.dedup();

        let asset_exclusions = exclusions
            .iter()
            .filter(|exclusion| exclusion.asset == asset.id)
            .collect::<Vec<_>>();
        for window in boundaries.windows(2) {
            let scene_start = window[0];
            let scene_end = window[1];
            let mut cuts = vec![scene_start, scene_end];
            for exclusion in &asset_exclusions {
                if exclusion.source_range.end <= scene_start
                    || exclusion.source_range.start >= scene_end
                {
                    continue;
                }
                cuts.push(exclusion.source_range.start.max(scene_start));
                cuts.push(exclusion.source_range.end.min(scene_end));
            }
            cuts.sort_unstable();
            cuts.dedup();
            for interval in cuts.windows(2) {
                let source_start = interval[0];
                let source_end = interval[1];
                if asset_exclusions.iter().any(|exclusion| {
                    exclusion.source_range.start < source_end
                        && exclusion.source_range.end > source_start
                }) {
                    continue;
                }
                let source_frames = source_end.0.saturating_sub(source_start.0);
                if source_frames < minimum_shot_frames.0 {
                    continue;
                }
                let project_frames = map_frames_with_rounding(
                    TimeCode(source_frames),
                    asset.fps,
                    project_fps,
                    FrameRounding::Nearest,
                )
                .map_err(|error| {
                    EvalError::Fixture(format!(
                        "could not map clean source interval {source_start}..{source_end} for {}: {error}",
                        asset.id
                    ))
                })?;
                selectable_project_frames =
                    selectable_project_frames.saturating_add(project_frames.0);
            }
        }
    }
    Ok(selectable_project_frames)
}

fn fixture_real_interview_story() -> Result<PreparedFixture, EvalError> {
    let truth: RealInterviewGroundTruth = serde_json::from_str(include_str!(
        "../../../../benchmarks/auto-edit/v5/ground-truth.json"
    ))
    .map_err(|error| EvalError::Fixture(format!("invalid v5 ground truth: {error}")))?;
    if truth.schema_version != 1
        || truth.story_id.trim().is_empty()
        || truth.source_story_range.start < 0
        || truth.source_story_range.start >= truth.source_story_range.end
        || truth.duration_bounds_project_frames.minimum < 0
        || truth.duration_bounds_project_frames.minimum
            >= truth.duration_bounds_project_frames.maximum
        || truth.required_terms.is_empty()
        || truth.excluded_terms.is_empty()
    {
        return Err(EvalError::Fixture(
            "v5 ground truth has an invalid schema, identifier, range, or word set".to_owned(),
        ));
    }
    let pack = FixturePackManifest::from_json(include_str!(
        "../../../../benchmarks/auto-edit/v5/fixture-pack.json"
    ))
    .map_err(|error| EvalError::Fixture(error.to_string()))?;
    let path = pack
        .verified_asset(&fixture_cache_root(), &truth.asset_id)
        .map_err(|error| EvalError::Fixture(error.to_string()))?;
    let media = eval_engine();
    let asset = probe_named(&media, &path, "interview-raw")?;
    if truth.source_story_range.end > asset.duration.0 {
        return Err(EvalError::Fixture(format!(
            "v5 story range ends at {}, beyond asset duration {}",
            truth.source_story_range.end, asset.duration.0
        )));
    }
    media.request_transcription_with_language(asset.clone(), Some("en"));
    media.request_silence_detection(asset.clone());
    let transcript = wait_for_transcript_result(media.as_ref(), &asset)?;
    let _silences = wait_for_silences(media.as_ref(), &asset)?;
    let observed_story = transcript
        .words
        .iter()
        .filter(|word| {
            word.source_start.0 >= truth.source_story_range.start
                && word.source_end.0 <= truth.source_story_range.end
        })
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let expected_dialogue = truth
        .expected_dialogue
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let expected_normalized = normalized_words(&truth.source_asr_dialogue);
    let observed_dialogue = normalized_words(&observed_story);
    if observed_dialogue != expected_normalized {
        return Err(EvalError::Fixture(format!(
            "v5 pinned source transcript no longer matches ground truth: expected {expected_normalized:?}, observed {observed_dialogue:?}"
        )));
    }

    let mut context = FixtureContext::default();
    context
        .asset_aliases
        .insert("interview-raw".to_owned(), asset.id);
    context.transcripts.insert(asset.id, transcript);
    context
        .word_sets
        .insert("recovery-required".to_owned(), truth.required_terms);
    context
        .word_sets
        .insert("off-story-exclusions".to_owned(), truth.excluded_terms);
    context
        .word_sets
        .insert("recovery-dialogue".to_owned(), expected_dialogue);
    context.duration_bounds.insert(
        "recovery-story".to_owned(),
        (
            TimeCode(truth.duration_bounds_project_frames.minimum),
            TimeCode(truth.duration_bounds_project_frames.maximum),
        ),
    );
    let mut document = empty_timeline_document(vec![asset]);
    document.resolution = (720, 1280);
    PreparedFixture::new(document, media, context, None, Vec::new())
}

#[allow(clippy::too_many_lines)]
fn fixture_real_event_multicam() -> Result<PreparedFixture, EvalError> {
    let truth: EventMulticamGroundTruth = serde_json::from_str(include_str!(
        "../../../../benchmarks/auto-edit/v5/event-ground-truth.json"
    ))
    .map_err(|error| EvalError::Fixture(format!("invalid v5 event ground truth: {error}")))?;
    if truth.schema_version != 1
        || truth.meeting_id.trim().is_empty()
        || truth.source_range.start < 0
        || truth.source_range.start >= truth.source_range.end
        || truth.audio_source_range.start < 0
        || truth.audio_source_range.start >= truth.audio_source_range.end
        || truth.speaker_assignments.len() < 2
        || truth.turns.is_empty()
        || truth.expected_video_cuts.len() < 2
        || truth.expected_dialogue.trim().is_empty()
    {
        return Err(EvalError::Fixture(
            "v5 event ground truth has an invalid schema, range, speaker map, cut list, or dialogue"
                .to_owned(),
        ));
    }
    if truth.speaker_assignments.iter().any(|assignment| {
        assignment.speaker.trim().is_empty()
            || assignment.angle_asset_id.trim().is_empty()
            || assignment.angle_name.trim().is_empty()
    }) || truth.reframe.target_aspect_basis_points != 5_625
        || truth.reframe.minimum_keyframes_per_axis < 2
        || truth.reframe.min_x_percent > truth.reframe.max_x_percent
        || truth.reframe.min_y_percent > truth.reframe.max_y_percent
        || truth.reframe.maximum_step_percent <= 0
    {
        return Err(EvalError::Fixture(
            "v5 event speaker assignments or reframe contract are invalid".to_owned(),
        ));
    }
    let expected_duration = truth
        .source_range
        .end
        .saturating_sub(truth.source_range.start);
    let mut expected_timeline_start = 0_i64;
    for cut in &truth.expected_video_cuts {
        if cut.angle_asset_id.trim().is_empty()
            || cut.timeline_start != expected_timeline_start
            || cut.timeline_end <= cut.timeline_start
            || cut.source_start < truth.source_range.start
            || cut.source_end > truth.source_range.end
            || cut.source_end.saturating_sub(cut.source_start)
                != cut.timeline_end.saturating_sub(cut.timeline_start)
        {
            return Err(EvalError::Fixture(
                "v5 event expected cuts are not contiguous real-time source selections".to_owned(),
            ));
        }
        expected_timeline_start = cut.timeline_end;
    }
    if expected_timeline_start != expected_duration {
        return Err(EvalError::Fixture(
            "v5 event expected cuts do not cover the full bounded event".to_owned(),
        ));
    }
    let pack = FixturePackManifest::from_json(include_str!(
        "../../../../benchmarks/auto-edit/v5/event-fixture-pack.json"
    ))
    .map_err(|error| EvalError::Fixture(error.to_string()))?;
    let cache = fixture_cache_root();
    let _annotations = pack
        .verified_asset(&cache, "ami-manual-annotations-1-6-2")
        .map_err(|error| EvalError::Fixture(error.to_string()))?;
    let media = eval_engine();
    let mut assets_by_fixture_id = BTreeMap::new();
    let mut aliases_by_fixture_id = BTreeMap::new();
    let alias_for = |fixture_id: &str| match fixture_id {
        "ami-es2002a-closeup1" => Some("camera-david"),
        "ami-es2002a-closeup2" => Some("camera-craig"),
        "ami-es2002a-closeup3" => Some("camera-andrew"),
        "ami-es2002a-closeup4" => Some("camera-laura"),
        "ami-es2002a-headset-mix" => Some("program-audio"),
        _ => None,
    };
    for fixture_id in truth
        .speaker_assignments
        .iter()
        .map(|assignment| assignment.angle_asset_id.as_str())
        .chain(std::iter::once(truth.audio_master_asset_id.as_str()))
    {
        if assets_by_fixture_id.contains_key(fixture_id) {
            continue;
        }
        let alias = alias_for(fixture_id).ok_or_else(|| {
            EvalError::Fixture(format!("v5 event asset {fixture_id:?} has no stable alias"))
        })?;
        let path = pack
            .verified_asset(&cache, fixture_id)
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        let asset = probe_named(&media, &path, alias)?;
        aliases_by_fixture_id.insert(fixture_id.to_owned(), alias.to_owned());
        assets_by_fixture_id.insert(fixture_id.to_owned(), asset);
    }
    let reference = assets_by_fixture_id
        .get(&truth.reference_asset_id)
        .ok_or_else(|| EvalError::Fixture("v5 event reference asset is missing".to_owned()))?;
    let audio_master = assets_by_fixture_id
        .get(&truth.audio_master_asset_id)
        .ok_or_else(|| EvalError::Fixture("v5 event audio master is missing".to_owned()))?;
    let project_fps = Rational::new(25, 1).expect("AMI fixture fps is valid");
    if reference.fps != project_fps {
        return Err(EvalError::Fixture(format!(
            "v5 event reference fps is {}/{}, expected 25/1",
            reference.fps.numerator(),
            reference.fps.denominator()
        )));
    }
    if truth.source_range.end > reference.duration.0
        || truth.audio_source_range.end > audio_master.duration.0
    {
        return Err(EvalError::Fixture(
            "v5 event source range exceeds one of its pinned assets".to_owned(),
        ));
    }

    let mut transcript_words = Vec::new();
    for turn in &truth.turns {
        let tokens = turn.text.split_whitespace().collect::<Vec<_>>();
        if turn.start < 0 || turn.end <= turn.start || tokens.is_empty() {
            return Err(EvalError::Fixture(format!(
                "v5 event speaker turn {:?} has an invalid range or no words",
                turn.speaker
            )));
        }
        let token_count = i64::try_from(tokens.len()).unwrap_or(i64::MAX);
        let duration = turn.end.saturating_sub(turn.start);
        for (index, token) in tokens.into_iter().enumerate() {
            let index = i64::try_from(index).unwrap_or(i64::MAX);
            let start = turn
                .start
                .saturating_add(duration.saturating_mul(index) / token_count);
            let end = turn
                .start
                .saturating_add(duration.saturating_mul(index.saturating_add(1)) / token_count);
            transcript_words.push(TranscriptWord {
                text: token.to_owned(),
                source_start: TimeCode(start),
                source_end: TimeCode(end.max(start.saturating_add(1))),
                speaker: Some(turn.speaker.clone()),
            });
        }
    }
    transcript_words.sort_by_key(|word| (word.source_start, word.source_end, word.text.clone()));
    let reference_manifest = pack
        .assets
        .iter()
        .find(|asset| asset.id == truth.reference_asset_id)
        .ok_or_else(|| EvalError::Fixture("v5 event reference manifest is missing".to_owned()))?;
    let transcript = Arc::new(AssetTranscript {
        asset: reference.id,
        content_sha256: reference_manifest.sha256.clone(),
        source_fps: project_fps,
        words: transcript_words,
    });
    media
        .register_transcript(reference, transcript.as_ref().clone())
        .map_err(|error| EvalError::Fixture(error.to_string()))?;

    let video_asset = assets_by_fixture_id
        .get("ami-es2002a-closeup4")
        .ok_or_else(|| EvalError::Fixture("v5 event Laura camera is missing".to_owned()))?;
    let duration = TimeCode(
        truth
            .source_range
            .end
            .saturating_sub(truth.source_range.start),
    );
    let audio_duration = map_source_range_to_project(
        TimeCode(truth.audio_source_range.start)..TimeCode(truth.audio_source_range.end),
        audio_master.fps,
        project_fps,
    )
    .map_err(|error| EvalError::Fixture(error.to_string()))?;
    if audio_duration != duration {
        return Err(EvalError::Fixture(format!(
            "v5 event audio maps to {} project frames, expected {}",
            audio_duration.0, duration.0
        )));
    }
    let media_pool = assets_by_fixture_id.values().cloned().collect::<Vec<_>>();
    let sync_members = truth
        .speaker_assignments
        .iter()
        .map(|assignment| {
            let asset = assets_by_fixture_id
                .get(&assignment.angle_asset_id)
                .ok_or_else(|| {
                    EvalError::Fixture(format!(
                        "v5 event angle asset {:?} is missing",
                        assignment.angle_asset_id
                    ))
                })?;
            Ok(SyncGroupMember {
                asset: asset.id,
                offset: TimeCode::ZERO,
                angle_name: assignment.angle_name.clone(),
            })
        })
        .chain(std::iter::once(Ok(SyncGroupMember {
            asset: audio_master.id,
            offset: TimeCode::ZERO,
            angle_name: "Program audio".to_owned(),
        })))
        .collect::<Result<Vec<_>, EvalError>>()?;
    let document = Document {
        catalog: MediaCatalog {
            sync_groups: vec![SyncGroup {
                id: SyncGroupId(1),
                name: "AMI ES2002a introductions".to_owned(),
                members: sync_members,
            }],
            ..MediaCatalog::default()
        },
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: video_asset.id,
                    source_range: TimeCode(truth.source_range.start)
                        ..TimeCode(truth.source_range.end),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(2),
                    asset: audio_master.id,
                    source_range: TimeCode(truth.audio_source_range.start)
                        ..TimeCode(truth.audio_source_range.end),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            },
        ],
        media_pool,
        markers: Vec::new(),
        fps: project_fps,
        resolution: (352, 288),
        duration,
    };
    let mut context = FixtureContext::default();
    for (fixture_id, asset) in &assets_by_fixture_id {
        let alias = aliases_by_fixture_id.get(fixture_id).ok_or_else(|| {
            EvalError::Fixture(format!("v5 event asset {fixture_id:?} lost its alias"))
        })?;
        context.asset_aliases.insert(alias.clone(), asset.id);
    }
    context.transcripts.insert(reference.id, transcript);
    context.word_sets.insert(
        "event-dialogue".to_owned(),
        truth
            .expected_dialogue
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    );
    context
        .duration_bounds
        .insert("event-introductions".to_owned(), (duration, duration));
    PreparedFixture::new(document, media, context, None, Vec::new())
}

#[allow(clippy::too_many_lines)]
fn fixture_real_music_montage() -> Result<PreparedFixture, EvalError> {
    let truth: MusicMontageGroundTruth = serde_json::from_str(include_str!(
        "../../../../benchmarks/auto-edit/v5/music-ground-truth-v10.json"
    ))
    .map_err(|error| EvalError::Fixture(format!("invalid v5 music ground truth: {error}")))?;
    let project_fps = Rational::new(25, 1).expect("music fixture fps is valid");
    let unique_visual_asset_ids = truth.visual_asset_ids.iter().collect::<BTreeSet<_>>();
    if truth.schema_version != 1
        || truth.montage_id.trim().is_empty()
        || truth.project_fps != project_fps
        || truth.project_resolution.width != 1_920
        || truth.project_resolution.height != 1_080
        || truth.timeline_range.start < 0
        || truth.timeline_range.end <= truth.timeline_range.start
        || truth.video_track_id != 1
        || truth.audio_track_id != 2
        || truth.visual_asset_ids.is_empty()
        || truth.visual_asset_ids.iter().any(|id| id.trim().is_empty())
        || unique_visual_asset_ids.len() != truth.visual_asset_ids.len()
        || truth.visual_asset_ids.contains(&truth.music_asset_id)
        || truth.music_asset_id.trim().is_empty()
        || truth.source_exclusions.iter().any(|exclusion| {
            exclusion.asset_id.trim().is_empty()
                || exclusion.source_range.start < 0
                || exclusion.source_range.end <= exclusion.source_range.start
                || exclusion.reason.trim().is_empty()
        })
        || truth.minimum_visual_shots < 2
        || truth.minimum_visual_shots > truth.maximum_visual_shots
        || truth.minimum_visual_shots != truth.maximum_visual_shots
        || truth.minimum_shot_frames <= 0
        || truth.minimum_shot_frames > truth.maximum_shot_frames
        || !(0.0..=100.0).contains(&truth.minimum_beat_strength_percent)
        || truth.beat_alignment_tolerance_frames < 0
        || truth.minimum_visual_assets_used != truth.visual_asset_ids.len()
        || truth.minimum_clips_per_visual_asset == 0
        || truth.minimum_project_frames_per_visual_asset <= 0
        || truth.latest_early_start_per_visual_asset < truth.timeline_range.start
        || truth.latest_early_start_per_visual_asset >= truth.earliest_late_start_per_visual_asset
        || truth.earliest_late_start_per_visual_asset >= truth.timeline_range.end
        || truth
            .minimum_clips_per_visual_asset
            .saturating_mul(truth.visual_asset_ids.len())
            > truth.maximum_visual_shots
        || truth
            .minimum_project_frames_per_visual_asset
            .saturating_mul(i64::try_from(truth.visual_asset_ids.len()).unwrap_or(i64::MAX))
            > truth
                .title_card
                .timeline_range
                .start
                .saturating_sub(truth.timeline_range.start)
        || truth.minimum_source_separation_frames < 0
        || truth.minimum_forward_gap_frames < 0
        || !(0.0..=100.0).contains(&truth.source_scene_minimum_confidence_percent)
        || truth.minimum_duration_buckets < 2
        || truth.duration_bucket_frames <= 0
        || truth.maximum_similar_run == 0
        || truth.similar_tolerance_frames < 0
        || truth.minimum_clean_selectable_project_frames <= 0
        || truth.selection_headroom_frames < 0
        || truth.minimum_clean_selectable_project_frames
            < truth
                .timeline_range
                .end
                .saturating_sub(truth.timeline_range.start)
                .saturating_add(truth.selection_headroom_frames)
        || !(2..=12).contains(&truth.meter_beats)
        || !(1..=16).contains(&truth.phrase_bars)
        || truth.reviewed_music_events.len() + truth.reviewed_story_events.len() + 1
            != truth.minimum_visual_shots
        || truth.reviewed_music_events.iter().any(|event| {
            event.project_frame <= truth.timeline_range.start
                || event.project_frame >= truth.timeline_range.end
                || event.role.trim().is_empty()
        })
        || truth
            .reviewed_music_events
            .windows(2)
            .any(|events| events[0].project_frame >= events[1].project_frame)
        || truth.reviewed_story_events.is_empty()
        || truth.reviewed_story_events.iter().any(|event| {
            event.project_frame <= truth.timeline_range.start
                || event.project_frame >= truth.title_card.timeline_range.start
                || event.role.trim().is_empty()
        })
        || truth.title_card.track_id != truth.video_track_id
        || truth.title_card.timeline_range.start <= truth.timeline_range.start
        || truth.title_card.timeline_range.end != truth.timeline_range.end
        || truth.title_card.text.trim().is_empty()
        || truth.title_card.font_size_token > 2
        || truth.title_card.color_token > 2
        || truth.title_card.fade_in_frames < 0
        || truth.title_card.fade_out_frames < 0
        || truth
            .title_card
            .fade_in_frames
            .saturating_add(truth.title_card.fade_out_frames)
            > truth
                .title_card
                .timeline_range
                .end
                .saturating_sub(truth.title_card.timeline_range.start)
        || truth.opening_source_window.start < 0
        || truth.opening_source_window.end <= truth.opening_source_window.start
        || truth.preparation_source_window.start < 0
        || truth.preparation_source_window.end <= truth.preparation_source_window.start
        || truth.operator_source_window.start < 0
        || truth.operator_source_window.end <= truth.operator_source_window.start
        || truth.activation_source_window.start < 0
        || truth.activation_source_window.end <= truth.activation_source_window.start
        || truth.climax_source_window.start < 0
        || truth.climax_source_window.end <= truth.climax_source_window.start
        || truth.opening_source_window.end > truth.preparation_source_window.start
        || truth.preparation_source_window.end > truth.operator_source_window.start
        || truth.operator_source_window.end > truth.activation_source_window.start
        || truth.activation_source_window.end > truth.climax_source_window.start
        || truth.music_preferred_source_start < 0
        || truth.music_preferred_source_end <= truth.music_preferred_source_start
        || truth.music_maximum_end_drift_frames < 0
        || truth.minimum_opening_shot_frames < truth.minimum_shot_frames
        || truth.minimum_opening_shot_frames > truth.maximum_shot_frames
        || truth.minimum_closing_shot_frames < truth.minimum_shot_frames
        || truth.minimum_closing_shot_frames > truth.maximum_shot_frames
        || truth.rendered_tail_window_frames <= 0
        || truth.rendered_tail_maximum_peak_dbfs_hundredths > 0
        || truth.rendered_activity_window_frames <= 0
        || truth.rendered_activity_minimum_integrated_lufs_hundredths > 0
        || truth.maximum_trailing_inactive_frames < 0
        || truth.story_brief.trim().is_empty()
        || truth.attribution.is_empty()
    {
        return Err(EvalError::Fixture(
            "v5 music ground truth has an invalid schema, project contract, asset set, or montage constraints"
                .to_owned(),
        ));
    }

    let minimum_strength_basis_points = percentage_to_basis_points_for_fixture(
        truth.minimum_beat_strength_percent,
        "minimum_beat_strength_percent",
    )?;
    let source_scene_minimum_confidence_basis_points = percentage_to_basis_points_for_fixture(
        truth.source_scene_minimum_confidence_percent,
        "source_scene_minimum_confidence_percent",
    )?;
    let pack = FixturePackManifest::from_json(include_str!(
        "../../../../benchmarks/auto-edit/v5/music-fixture-pack-v4.json"
    ))
    .map_err(|error| EvalError::Fixture(error.to_string()))?;
    let cache = fixture_cache_root();
    let media = eval_engine();
    let mut assets_by_fixture_id = BTreeMap::new();
    let mut aliases_by_fixture_id = BTreeMap::new();
    for fixture_id in truth
        .visual_asset_ids
        .iter()
        .chain(std::iter::once(&truth.music_asset_id))
    {
        if assets_by_fixture_id.contains_key(fixture_id) {
            continue;
        }
        let alias = music_fixture_alias(fixture_id).ok_or_else(|| {
            EvalError::Fixture(format!("v5 music asset {fixture_id:?} has no stable alias"))
        })?;
        let path = pack
            .verified_asset(&cache, fixture_id)
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        let mut asset = probe_named(&media, &path, alias)?;
        if truth.visual_asset_ids.iter().any(|id| id == fixture_id) {
            // The visual source files may carry an incidental audio stream.
            // The benchmark deliberately makes the pinned music bed the only program audio.
            asset.kind = kinewright_core::MediaKind::Video;
        }
        aliases_by_fixture_id.insert(fixture_id.clone(), alias.to_owned());
        assets_by_fixture_id.insert(fixture_id.clone(), asset);
    }

    for exclusion in &truth.source_exclusions {
        if !truth
            .visual_asset_ids
            .iter()
            .any(|asset_id| asset_id == &exclusion.asset_id)
        {
            return Err(EvalError::Fixture(format!(
                "v5 music source exclusion {:?} is not attached to a pinned visual asset",
                exclusion.asset_id
            )));
        }
        let asset = assets_by_fixture_id
            .get(&exclusion.asset_id)
            .expect("validated source exclusion asset exists");
        if exclusion.source_range.end > asset.duration.0 {
            return Err(EvalError::Fixture(format!(
                "v5 music source exclusion {:?} ends at {}, beyond asset duration {}",
                exclusion.asset_id, exclusion.source_range.end, asset.duration.0
            )));
        }
    }
    let source_exclusions = truth
        .source_exclusions
        .iter()
        .map(|exclusion| {
            let asset = assets_by_fixture_id
                .get(&exclusion.asset_id)
                .expect("validated source exclusion asset exists");
            SourceRangeExclusion {
                asset: asset.id,
                source_range: TimeCode(exclusion.source_range.start)
                    ..TimeCode(exclusion.source_range.end),
                reason: exclusion.reason.clone(),
            }
        })
        .collect::<Vec<_>>();

    let music = assets_by_fixture_id
        .get(&truth.music_asset_id)
        .ok_or_else(|| EvalError::Fixture("v5 music asset is missing".to_owned()))?;
    if !music.kind.supports(TrackKind::Audio) {
        return Err(EvalError::Fixture(format!(
            "v5 music asset {} is not audio-capable after probe: {:?}",
            music.id, music.kind
        )));
    }
    for fixture_id in &truth.visual_asset_ids {
        let asset = assets_by_fixture_id.get(fixture_id).ok_or_else(|| {
            EvalError::Fixture(format!("v5 visual asset {fixture_id:?} is missing"))
        })?;
        if asset.kind != kinewright_core::MediaKind::Video {
            return Err(EvalError::Fixture(format!(
                "v5 visual asset {fixture_id:?} was not forced to video kind"
            )));
        }
    }
    let primary_visual = assets_by_fixture_id
        .get(&truth.visual_asset_ids[0])
        .expect("validated primary visual fixture exists");
    for (role, range) in [
        ("opening", &truth.opening_source_window),
        ("preparation", &truth.preparation_source_window),
        ("operator", &truth.operator_source_window),
        ("activation", &truth.activation_source_window),
        ("climax", &truth.climax_source_window),
    ] {
        if range.end > primary_visual.duration.0 {
            return Err(EvalError::Fixture(format!(
                "v5 music {role} source window ends at {}, beyond primary visual duration {}",
                range.end, primary_visual.duration.0
            )));
        }
    }
    let mut source_scenes = Vec::new();
    for fixture_id in &truth.visual_asset_ids {
        let asset = assets_by_fixture_id
            .get(fixture_id)
            .expect("validated visual fixture asset exists");
        media.request_scene_detection(asset.clone());
        let scenes = wait_for_scenes(media.as_ref(), asset)?;
        source_scenes.extend(
            scenes
                .changes
                .iter()
                .filter(|change| {
                    change.confidence_basis_points >= source_scene_minimum_confidence_basis_points
                })
                .map(|change| (asset.id, change.source_frame)),
        );
    }
    if source_scenes.is_empty() {
        return Err(EvalError::Fixture(
            "v5 music visual sources produced no qualifying scene boundaries".to_owned(),
        ));
    }

    let media_pool = truth
        .visual_asset_ids
        .iter()
        .chain(std::iter::once(&truth.music_asset_id))
        .map(|fixture_id| {
            assets_by_fixture_id
                .get(fixture_id)
                .cloned()
                .ok_or_else(|| EvalError::Fixture(format!("v5 music asset {fixture_id:?} missing")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let clean_selectable_project_frames = clean_selectable_project_frames(
        &media_pool,
        project_fps,
        &source_scenes,
        &source_exclusions,
        TimeCode(truth.minimum_shot_frames),
    )?;
    let target_project_frames = truth
        .timeline_range
        .end
        .saturating_sub(truth.timeline_range.start);
    let required_clean_project_frames =
        target_project_frames.saturating_add(truth.selection_headroom_frames);
    if clean_selectable_project_frames < truth.minimum_clean_selectable_project_frames
        || clean_selectable_project_frames < required_clean_project_frames
    {
        return Err(EvalError::Fixture(format!(
            "v5 music source ranges provide only {clean_selectable_project_frames} clean selectable project frames; required at least {} (target {target_project_frames} + headroom {})",
            truth.minimum_clean_selectable_project_frames, truth.selection_headroom_frames
        )));
    }
    let document = Document {
        catalog: MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![
            Track {
                id: TrackId(truth.video_track_id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
            Track {
                id: TrackId(truth.audio_track_id),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            },
        ],
        media_pool,
        markers: Vec::new(),
        fps: project_fps,
        resolution: (
            truth.project_resolution.width,
            truth.project_resolution.height,
        ),
        duration: TimeCode::ZERO,
    };

    let music = document
        .asset(music.id)
        .cloned()
        .ok_or_else(|| EvalError::Fixture("music asset was lost from media pool".to_owned()))?;
    media.request_beat_detection(music.clone());
    let beats = wait_for_beats(media.as_ref(), &music)?;
    let beat_status = BeatStatus::Ready(Arc::clone(&beats));
    let timeline_range = TimeCode(truth.timeline_range.start)..TimeCode(truth.timeline_range.end);
    let music_plan = kinewright_core::music_fit_plan_with_end_anchor(
        &document,
        TrackId(truth.audio_track_id),
        music.id,
        timeline_range.clone(),
        Some(TimeCode(truth.music_preferred_source_start)),
        Some(kinewright_core::MusicEndAnchor {
            preferred_source_end: TimeCode(truth.music_preferred_source_end),
            maximum_drift_frames: TimeCode(truth.music_maximum_end_drift_frames),
        }),
        &beat_status,
        minimum_strength_basis_points,
        ThreePointMode::Overwrite,
    )
    .map_err(|error| {
        EvalError::Fixture(format!("v5 music fit contract is not feasible: {error}"))
    })?;
    if music_plan.timeline_range != timeline_range
        || music_plan
            .source_range
            .end
            .0
            .abs_diff(truth.music_preferred_source_end)
            > truth.music_maximum_end_drift_frames.unsigned_abs()
        || music_plan.source_range.start < TimeCode::ZERO
        || music_plan.source_range.end > music.duration
        || music_plan.source_range.end <= music_plan.source_range.start
    {
        return Err(EvalError::Fixture(
            "v5 music fit returned an invalid source or project range".to_owned(),
        ));
    }

    let source_beats = beats
        .beats
        .iter()
        .filter(|beat| {
            beat.source_frame >= TimeCode::ZERO
                && beat.source_frame < music.duration
                && beat.strength_basis_points >= minimum_strength_basis_points
        })
        .map(|beat| beat.source_frame)
        .collect::<Vec<_>>();
    if source_beats.is_empty() || !source_beats.contains(&music_plan.source_range.start) {
        return Err(EvalError::Fixture(
            "v5 music fit anchor is absent from the eligible source beat set".to_owned(),
        ));
    }

    let mut project_beats = beats
        .beats
        .iter()
        .filter(|beat| {
            beat.source_frame >= music_plan.source_range.start
                && beat.source_frame < music_plan.source_range.end
                && beat.strength_basis_points >= minimum_strength_basis_points
        })
        .filter_map(|beat| {
            let offset = beat
                .source_frame
                .checked_sub(music_plan.source_range.start)?;
            let project_offset =
                map_frames_with_rounding(offset, music.fps, project_fps, FrameRounding::Nearest)
                    .ok()?;
            let project_frame = music_plan
                .timeline_range
                .start
                .checked_add(project_offset)?;
            (project_frame >= music_plan.timeline_range.start
                && project_frame < music_plan.timeline_range.end)
                .then_some(project_frame)
        })
        .collect::<Vec<_>>();
    project_beats.sort_unstable();
    project_beats.dedup();
    if project_beats.is_empty() {
        return Err(EvalError::Fixture(
            "v5 music fit produced no eligible project-frame beats".to_owned(),
        ));
    }
    let mut structure_document = document.clone();
    kinewright_core::apply_batch(&mut structure_document, &music_plan.operations).map_err(
        |error| {
            EvalError::Fixture(format!(
                "v5 music fit could not build the structure-analysis snapshot: {error}"
            ))
        },
    )?;
    let structure_timeline_beats = media
        .timeline_beats(
            &structure_document,
            Some(timeline_range.clone()),
            minimum_strength_basis_points,
        )
        .map_err(|error| {
            EvalError::Fixture(format!(
                "v5 music structure beats could not be mapped: {error}"
            ))
        })?;
    let _structure = kinewright_core::music_structure_analysis(
        &structure_document,
        music.id,
        timeline_range.clone(),
        &structure_timeline_beats,
        &kinewright_core::TimelineBeatAnalysisState::Ready,
        minimum_strength_basis_points,
        truth.meter_beats,
        truth.phrase_bars,
    )
    .map_err(|error| EvalError::Fixture(format!("v5 music structure failed: {error}")))?;
    let reviewed_events = truth
        .reviewed_music_events
        .iter()
        .map(|event| TimeCode(event.project_frame))
        .collect::<Vec<_>>();
    if reviewed_events.iter().any(|reviewed| {
        !project_beats.iter().any(|beat| {
            beat.0.abs_diff(reviewed.0) <= truth.beat_alignment_tolerance_frames.unsigned_abs()
        })
    }) {
        return Err(EvalError::Fixture(format!(
            "v5 reviewed music events {reviewed_events:?} are not all present in detected project beats {project_beats:?}"
        )));
    }

    let mut context = FixtureContext::default();
    for (fixture_id, asset) in &assets_by_fixture_id {
        let alias = aliases_by_fixture_id.get(fixture_id).ok_or_else(|| {
            EvalError::Fixture(format!("v5 music asset {fixture_id:?} lost its alias"))
        })?;
        context.asset_aliases.insert(alias.clone(), asset.id);
    }
    context
        .source_beat_sets
        .insert(MUSIC_SOURCE_BEAT_SET.to_owned(), source_beats);
    context
        .timeline_beat_sets
        .insert(MUSIC_PROJECT_BEAT_SET.to_owned(), project_beats);
    context
        .timeline_beat_sets
        .insert(MUSIC_REVIEWED_EVENT_SET.to_owned(), reviewed_events);
    context
        .scene_sets
        .insert(MUSIC_SOURCE_SCENE_SET.to_owned(), source_scenes);
    context
        .exclusion_sets
        .insert(MUSIC_SOURCE_EXCLUSION_SET.to_owned(), source_exclusions);
    PreparedFixture::new(document, media, context, None, Vec::new())
}

#[allow(clippy::too_many_lines)]
fn fixture_editorial_story() -> Result<PreparedFixture, EvalError> {
    fixture_editorial_story_from(
        include_str!("../../../../benchmarks/auto-edit/v3/ground-truth.json"),
        "v3",
    )
}

fn fixture_dialogue_pacing_story() -> Result<PreparedFixture, EvalError> {
    fixture_editorial_story_from(
        include_str!("../../../../benchmarks/auto-edit/v4/ground-truth.json"),
        "v4",
    )
}

#[allow(clippy::too_many_lines)]
fn fixture_editorial_story_from(
    ground_truth: &str,
    benchmark_version: &str,
) -> Result<PreparedFixture, EvalError> {
    let truth: EditorialGroundTruth = serde_json::from_str(ground_truth).map_err(|error| {
        EvalError::Fixture(format!("invalid {benchmark_version} ground truth: {error}"))
    })?;
    if truth.schema_version != 1 || truth.story_id.trim().is_empty() {
        return Err(EvalError::Fixture(format!(
            "{benchmark_version} ground truth has an unsupported schema or empty story id"
        )));
    }
    let accepted_order = truth
        .takes
        .iter()
        .filter(|take| take.accepted)
        .map(|take| take.id.as_str())
        .collect::<Vec<_>>();
    if accepted_order
        != truth
            .expected_take_order
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        return Err(EvalError::Fixture(format!(
            "{benchmark_version} accepted takes do not match expected_take_order"
        )));
    }

    let media = eval_engine();
    let mut resources: Vec<Box<dyn Send>> = Vec::new();
    let mut assets = Vec::new();
    for (index, take) in truth.takes.iter().enumerate() {
        let environment_name = format!("KINEWRIGHT_EVAL_EDITORIAL_TAKE_{}_AUDIO", index + 1);
        let generated = documentary_speech(
            &take.id,
            &take.ssml,
            &take.visual,
            &take.role,
            &environment_name,
        )?;
        let asset = probe_named(&media, generated.path(), &take.id)?;
        media.request_transcription_with_language(asset.clone(), Some("en"));
        media.request_silence_detection(asset.clone());
        assets.push(asset);
        resources.push(Box::new(generated));
    }

    let mut transcripts = BTreeMap::new();
    let mut silences = BTreeMap::new();
    for asset in &assets {
        transcripts.insert(asset.id, wait_for_transcript_result(media.as_ref(), asset)?);
        silences.insert(asset.id, wait_for_silences(media.as_ref(), asset)?);
    }
    let mut context = FixtureContext::default();
    for (take, asset) in truth.takes.iter().zip(&assets) {
        context.asset_aliases.insert(take.id.clone(), asset.id);
    }
    context.transcripts.extend(
        transcripts
            .iter()
            .map(|(asset, transcript)| (*asset, Arc::clone(transcript))),
    );
    let mut selected_fillers = BTreeSet::new();
    for take_id in &truth.expected_take_order {
        let asset_id = *context.asset_aliases.get(take_id).ok_or_else(|| {
            EvalError::Fixture(format!(
                "{benchmark_version} expected take {take_id:?} does not exist"
            ))
        })?;
        let recognized = if benchmark_version == "v4" {
            acoustically_scorable_words(&transcripts[&asset_id], &silences[&asset_id])
        } else {
            normalized_words(&joined_words(&transcripts[&asset_id]))
        };
        let (fillers, spoken_words) = partition_fillers(&recognized);
        if spoken_words.is_empty() {
            return Err(EvalError::Fixture(format!(
                "{benchmark_version} accepted take {take_id:?} ASR produced no content words"
            )));
        }
        selected_fillers.extend(fillers);
        context
            .word_sets
            .insert(format!("{take_id}-recognized-content"), spoken_words);
    }
    let required_fillers = BTreeSet::from(["um".to_owned()]);
    if !required_fillers.is_subset(&selected_fillers) {
        return Err(EvalError::Fixture(format!(
            "{benchmark_version} source ASR must recognize the authored filler before it can be scored; observed {selected_fillers:?}"
        )));
    }
    context.word_sets.insert(
        "selected-recognized-fillers".to_owned(),
        selected_fillers.into_iter().collect(),
    );
    let authored_dialogue = normalized_words(&truth.expected_dialogue);
    if authored_dialogue.is_empty() || truth.excluded_words.is_empty() {
        return Err(EvalError::Fixture(format!(
            "{benchmark_version} authored dialogue and exclusions must be non-empty"
        )));
    }
    context
        .word_sets
        .insert("authored-dialogue".to_owned(), authored_dialogue);
    context
        .word_sets
        .insert("authored-exclusions".to_owned(), truth.excluded_words);

    let selected_assets = truth
        .expected_take_order
        .iter()
        .map(|take| {
            let asset_id = context.asset_aliases.get(take).ok_or_else(|| {
                EvalError::Fixture(format!(
                    "{benchmark_version} expected take {take:?} does not exist"
                ))
            })?;
            assets
                .iter()
                .find(|asset| asset.id == *asset_id)
                .ok_or_else(|| {
                    EvalError::Fixture(format!("{benchmark_version} asset {asset_id} is missing"))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let maximum = TimeCode(
        selected_assets
            .iter()
            .map(|asset| asset.duration.0)
            .fold(0_i64, i64::saturating_add),
    );
    let minimum = TimeCode(maximum.0.saturating_mul(45) / 100);
    context
        .duration_bounds
        .insert("editorial-cut".to_owned(), (minimum, maximum));
    let mut document = empty_timeline_document(assets);
    document.resolution = (360, 640);
    PreparedFixture::new(document, media, context, None, resources)
}

fn documentary_speech(
    label: &str,
    speech: &str,
    visual: &str,
    role: &str,
    audio_override_env: &str,
) -> Result<GeneratedMedia, EvalError> {
    let speech_clip = SpeechClip::ssml(label, speech, audio_override_env);
    let input = speech_clip.mp4.to_string_lossy().into_owned();
    let video_source = documentary_visual(visual, role)?;
    Ok(GeneratedMedia::ffmpeg(
        &format!("editorial-{label}"),
        &[
            "-f",
            "lavfi",
            "-i",
            &video_source,
            "-i",
            &input,
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "copy",
            "-shortest",
        ],
        "mp4",
    ))
}

fn documentary_visual(visual: &str, role: &str) -> Result<String, EvalError> {
    const FONT: &str = "crates/kinewright-app/assets/fonts/Inter-SemiBold.ttf";
    let label = match role {
        "opening" => "THE EMPTY LOT",
        "opening_alternate" => "RETAKE",
        "community_action" => "NEIGHBORS BUILD",
        "result" => "SATURDAY MARKET",
        "result_alternate" => "TUESDAY?",
        _ => "COMMUNITY GARDEN",
    };
    let scene = match visual {
        "empty_lot" => concat!(
            "color=c=0x8CB9D9:size=360x640:rate=30,",
            "drawbox=x=0:y=360:w=360:h=280:color=0x76553D:t=fill,",
            "drawbox=x=72:y=330:w=8:h=55:color=0x456B3A:t=fill,",
            "drawbox=x=178:y=342:w=7:h=43:color=0x456B3A:t=fill,",
            "drawbox=x=284:y=326:w=9:h=59:color=0x456B3A:t=fill,",
            "drawbox=x=25:y=110:w=5:h=145:color=white@0.25:t=fill,",
            "drawbox=x=105:y=70:w=5:h=160:color=white@0.25:t=fill,",
            "drawbox=x=220:y=95:w=5:h=150:color=white@0.25:t=fill,",
            "drawbox=x=315:y=55:w=5:h=165:color=white@0.25:t=fill"
        ),
        "empty_lot_retake" => concat!(
            "color=c=0x87929B:size=360x640:rate=30,",
            "drawbox=x=0:y=310:w=360:h=330:color=0x3F454A:t=fill,",
            "drawbox=x=45:y=390:w=110:h=8:color=white@0.75:t=fill,",
            "drawbox=x=205:y=390:w=110:h=8:color=white@0.75:t=fill,",
            "drawbox=x=45:y=520:w=110:h=8:color=white@0.75:t=fill,",
            "drawbox=x=205:y=520:w=110:h=8:color=white@0.75:t=fill,",
            "drawbox=x=22:y=22:w=316:h=84:color=0xA52A2A@0.92:t=fill"
        ),
        "garden_build" => concat!(
            "color=c=0x95C7E5:size=360x640:rate=30,",
            "drawbox=x=0:y=365:w=360:h=275:color=0x78A85B:t=fill,",
            "drawbox=x=35:y=390:w=125:h=72:color=0x8A5A3B:t=fill,",
            "drawbox=x=200:y=390:w=125:h=72:color=0x8A5A3B:t=fill,",
            "drawbox=x=55:y=402:w=85:h=42:color=0x3E2C22:t=fill,",
            "drawbox=x=220:y=402:w=85:h=42:color=0x3E2C22:t=fill,",
            "drawbox=x=77:y=365:w=7:h=40:color=0x2F7D32:t=fill,",
            "drawbox=x=112:y=358:w=7:h=47:color=0x2F7D32:t=fill,",
            "drawbox=x=242:y=362:w=7:h=43:color=0x2F7D32:t=fill,",
            "drawbox=x=278:y=354:w=7:h=51:color=0x2F7D32:t=fill,",
            "drawbox=x=80:y=245:w=42:h=95:color=0x325A74:t=fill,",
            "drawbox=x=238:y=245:w=42:h=95:color=0xC86B3C:t=fill"
        ),
        "market_result" => concat!(
            "color=c=0xA7D1E8:size=360x640:rate=30,",
            "drawbox=x=0:y=430:w=360:h=210:color=0x6FA45A:t=fill,",
            "drawbox=x=28:y=300:w=132:h=155:color=0xF1E4C8:t=fill,",
            "drawbox=x=200:y=300:w=132:h=155:color=0xF1E4C8:t=fill,",
            "drawbox=x=20:y=275:w=148:h=45:color=0xD85C4A:t=fill,",
            "drawbox=x=192:y=275:w=148:h=45:color=0xE2B447:t=fill,",
            "drawbox=x=50:y=390:w=86:h=48:color=0x8A5A3B:t=fill,",
            "drawbox=x=224:y=390:w=86:h=48:color=0x8A5A3B:t=fill,",
            "drawbox=x=61:y=400:w=14:h=14:color=0xD84A3A:t=fill,",
            "drawbox=x=84:y=400:w=14:h=14:color=0xE9B949:t=fill,",
            "drawbox=x=236:y=400:w=14:h=14:color=0x4F8F42:t=fill,",
            "drawbox=x=260:y=400:w=14:h=14:color=0xD84A3A:t=fill"
        ),
        "market_wrong_day" => concat!(
            "color=c=0xA7D1E8:size=360x640:rate=30,",
            "drawbox=x=0:y=430:w=360:h=210:color=0x6FA45A:t=fill,",
            "drawbox=x=28:y=300:w=304:h=155:color=0xD8D8D8:t=fill,",
            "drawbox=x=20:y=275:w=320:h=45:color=0x7A7A7A:t=fill,",
            "drawbox=x=22:y=22:w=316:h=84:color=0xA52A2A@0.92:t=fill"
        ),
        _ => {
            return Err(EvalError::Fixture(format!(
                "unknown v3 visual scene {visual:?}"
            )));
        }
    };
    Ok(format!(
        "{scene},drawtext=fontfile={FONT}:text='{label}':x=(w-text_w)/2:y=48:fontsize=28:fontcolor=white:box=1:boxcolor=black@0.55:boxborderw=12,noise=alls=2:allf=t"
    ))
}

fn fixture_distinct_timeline(
    label: &str,
    names: &[&str],
    patterns: &[&str],
) -> Result<PreparedFixture, EvalError> {
    let media = eval_engine();
    let mut assets = Vec::new();
    let mut resources: Vec<Box<dyn Send>> = Vec::new();
    for (index, (name, pattern)) in names.iter().zip(patterns).enumerate() {
        let source = format!("{pattern}=size=320x180:rate=30:duration=4");
        let generated = generated_video(
            &format!("{label}-{name}"),
            &source,
            4,
            400_u32.saturating_add(u32::try_from(index).unwrap_or(0) * 100),
        );
        assets.push(probe_named(&media, generated.path(), name)?);
        resources.push(Box::new(generated));
    }
    let ranges = assets
        .iter()
        .enumerate()
        .map(|(index, asset)| (index, TimeCode::ZERO..asset.duration))
        .collect::<Vec<_>>();
    let document = timeline_document(assets.clone(), &ranges)?;
    let context = FixtureContext {
        asset_aliases: names
            .iter()
            .zip(&assets)
            .map(|(name, asset)| ((*name).to_owned(), asset.id))
            .collect(),
        ..FixtureContext::default()
    };
    PreparedFixture::new(document, media, context, None, resources)
}

fn eval_engine() -> Arc<FfmpegMediaEngine> {
    Arc::new(test_engine("KINEWRIGHT_EVAL_DATA_DIR"))
}

fn generated_video(
    label: &str,
    video_source: &str,
    duration_seconds: u32,
    frequency: u32,
) -> GeneratedMedia {
    let audio_source =
        format!("sine=frequency={frequency}:sample_rate=16000:duration={duration_seconds}");
    GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            video_source,
            "-f",
            "lavfi",
            "-i",
            &audio_source,
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
        "mp4",
    )
}

fn patterned_speech(
    label: &str,
    speech: &str,
    pattern: &str,
    audio_override_env: &str,
) -> GeneratedMedia {
    let speech_clip = SpeechClip::ssml(label, speech, audio_override_env);
    let input = speech_clip.mp4.to_string_lossy().into_owned();
    let video_source = format!("{pattern}=size=320x180:rate=30");
    GeneratedMedia::ffmpeg(
        &format!("eval-{label}"),
        &[
            "-f",
            "lavfi",
            "-i",
            &video_source,
            "-i",
            &input,
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "copy",
            "-shortest",
        ],
        "mp4",
    )
}

fn probe_named(
    media: &FfmpegMediaEngine,
    path: &Path,
    name: &str,
) -> Result<MediaAsset, EvalError> {
    let mut asset = media
        .probe(path)
        .map_err(|error| EvalError::Fixture(error.to_string()))?;
    name.clone_into(&mut asset.name);
    Ok(asset)
}

fn single_asset_document(asset: MediaAsset) -> Document {
    let duration = asset.duration;
    timeline_document(vec![asset], &[(0, TimeCode::ZERO..duration)])
        .expect("a probed full-range asset must make a valid document")
}

fn empty_timeline_document(assets: Vec<MediaAsset>) -> Document {
    Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: assets,
        markers: Vec::new(),
        fps: Rational::new(FPS, 1).expect("fixture fps is valid"),
        resolution: (320, 180),
        duration: TimeCode::ZERO,
    }
}

fn timeline_document(
    assets: Vec<MediaAsset>,
    clips: &[(usize, std::ops::Range<TimeCode>)],
) -> Result<Document, EvalError> {
    let fps = Rational::new(FPS, 1).map_err(|error| EvalError::Fixture(error.to_string()))?;
    let mut timeline_start = TimeCode::ZERO;
    let mut timeline_clips = Vec::with_capacity(clips.len());
    for (index, (asset_index, source_range)) in clips.iter().enumerate() {
        let asset = assets.get(*asset_index).ok_or_else(|| {
            EvalError::Fixture(format!("missing fixture asset index {asset_index}"))
        })?;
        let duration = map_source_range_to_project(source_range.clone(), asset.fps, fps)
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        timeline_clips.push(Clip {
            id: ClipId(u64::try_from(index + 1).unwrap_or(u64::MAX)),
            asset: asset.id,
            source_range: source_range.clone(),
            content: kinewright_core::ClipContent::Media,
            timeline_start,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        });
        timeline_start = timeline_start
            .checked_add(duration)
            .ok_or_else(|| EvalError::Fixture("fixture duration overflowed".to_owned()))?;
    }
    Ok(Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: timeline_clips,
        }],
        media_pool: assets,
        markers: Vec::new(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        fps,
        resolution: (320, 180),
        duration: timeline_start,
    })
}

fn wait_for_transcript_result(
    media: &dyn Analysis,
    asset: &MediaAsset,
) -> Result<Arc<AssetTranscript>, EvalError> {
    let deadline = Instant::now() + Duration::from_mins(20);
    let mut previous = String::new();
    let label = asset.id;
    loop {
        let status = media.transcript_status(asset);
        let summary = match &status {
            TranscriptStatus::Ready(transcript) => format!(
                "Ready(words={}, source_fps={}/{}, sha256={}...)",
                transcript.words.len(),
                transcript.source_fps.numerator(),
                transcript.source_fps.denominator(),
                transcript.content_sha256.get(..12).unwrap_or("invalid")
            ),
            other => format!("{other:?}"),
        };
        if summary != previous {
            println!("  ASR {label}: {summary}");
            previous = summary;
        }
        match status {
            TranscriptStatus::Ready(transcript) => return Ok(transcript),
            TranscriptStatus::NoSpeech => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} produced no ASR speech"
                )));
            }
            TranscriptStatus::Cancelled => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} transcription was cancelled"
                )));
            }
            TranscriptStatus::Failed(error) => return Err(EvalError::Fixture(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(EvalError::Fixture(format!(
                "asset {label} transcription timed out"
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_silences(
    media: &dyn Analysis,
    asset: &MediaAsset,
) -> Result<Arc<AssetSilences>, EvalError> {
    let deadline = Instant::now() + Duration::from_mins(3);
    let label = asset.id;
    loop {
        match media.silence_status(asset) {
            SilenceStatus::Ready(silences) => return Ok(silences),
            SilenceStatus::NoAudio => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} has no audio for silence analysis"
                )));
            }
            SilenceStatus::Cancelled => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} silence analysis was cancelled"
                )));
            }
            SilenceStatus::Failed(error) => return Err(EvalError::Fixture(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(EvalError::Fixture(format!(
                "asset {label} silence analysis timed out"
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_scenes(
    media: &dyn Analysis,
    asset: &MediaAsset,
) -> Result<Arc<AssetSceneChanges>, EvalError> {
    let deadline = Instant::now() + Duration::from_mins(3);
    let label = asset.id;
    loop {
        match media.scene_status(asset) {
            SceneStatus::Ready(scenes) => return Ok(scenes),
            SceneStatus::NoVideo => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} has no video for scene analysis"
                )));
            }
            SceneStatus::Cancelled => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} scene analysis was cancelled"
                )));
            }
            SceneStatus::Failed(error) => return Err(EvalError::Fixture(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(EvalError::Fixture(format!(
                "asset {label} scene analysis timed out"
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_beats(media: &dyn Analysis, asset: &MediaAsset) -> Result<Arc<AssetBeats>, EvalError> {
    let deadline = Instant::now() + Duration::from_mins(5);
    let label = asset.id;
    let mut previous = String::new();
    loop {
        let status = media.beat_status(asset);
        let summary = match &status {
            BeatStatus::Ready(beats) => format!(
                "Ready(beats={}, source_fps={}/{}, bpm={})",
                beats.beats.len(),
                beats.source_fps.numerator(),
                beats.source_fps.denominator(),
                beats.estimated_bpm_milli
            ),
            other => format!("{other:?}"),
        };
        if summary != previous {
            println!("  BEATS {label}: {summary}");
            previous = summary;
        }
        match status {
            BeatStatus::Ready(beats) => return Ok(beats),
            BeatStatus::NoAudio => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} has no audio for beat analysis"
                )));
            }
            BeatStatus::Cancelled => {
                return Err(EvalError::Fixture(format!(
                    "asset {label} beat analysis was cancelled"
                )));
            }
            BeatStatus::Failed(error) => return Err(EvalError::Fixture(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(EvalError::Fixture(format!(
                "asset {label} beat analysis timed out"
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percentage_to_basis_points_for_fixture(value: f64, field: &str) -> Result<u16, EvalError> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(EvalError::Fixture(format!(
            "{field} must be finite and between 0 and 100 percent, observed {value}"
        )));
    }
    Ok((value * 100.0).round() as u16)
}

fn silence_frames(silences: &AssetSilences, minimum_frames: i64) -> i64 {
    silences
        .spans
        .iter()
        .map(|span| span.source_end.0.saturating_sub(span.source_start.0))
        .filter(|duration| *duration >= minimum_frames)
        .fold(0_i64, i64::saturating_add)
}

fn acoustically_scorable_words(
    transcript: &AssetTranscript,
    silences: &AssetSilences,
) -> Vec<String> {
    transcript
        .words
        .iter()
        .filter(|word| {
            !silences.spans.iter().any(|span| {
                span.source_start <= word.source_start && span.source_end >= word.source_end
            })
        })
        .flat_map(|word| normalized_words(&word.text))
        .collect()
}

fn partition_fillers(words: &[String]) -> (Vec<String>, Vec<String>) {
    words.iter().cloned().partition(|word| is_filler(word))
}

fn is_filler(word: &str) -> bool {
    FILLER_WORDS.contains(&word)
}

fn render_evals_document(
    definitions: &[EvalDefinition],
    environment: &EnvironmentStamp,
    results: &[EvalResult],
    output_path: &Path,
) -> String {
    let mut document = String::from(
        "# Agent evals\n\nKinewright's Arc 2 editing competence suite runs only when `KINEWRIGHT_EVAL=1` is explicitly set. It uses generated media, the real MCP server, and an installed subscription harness. CI covers the framework with a fake driver and spends nothing.\n\n## Run\n\n```powershell\n$env:KINEWRIGHT_EVAL = '1'\ncargo run -p kinewright-agent --bin kinewright-eval\n# Optional: -- --harness codex\n```\n\nResults are written as timestamped, environment-stamped JSONL under `target/evals/`. A full live suite is intentionally expensive and must not be placed in CI.\n\nThe versioned public contract and first machine-readable baseline live under [`benchmarks/auto-edit/v1`](../benchmarks/auto-edit/v1/README.md). A refreshed docs snapshot never overwrites that historical baseline.\n\n## Seed suite\n\n| Eval | Rationale | USD ceiling |\n|---|---|---:|\n",
    );
    for definition in definitions {
        let cost_ceiling = definition
            .budgets
            .max_cost_usd
            .map_or_else(|| "n/a".to_owned(), |cost| format!("${cost:.2}"));
        let _ = writeln!(
            document,
            "| {} | {} | {cost_ceiling} |",
            definition.name, definition.rationale
        );
    }
    document.push_str("\n## Baseline snapshot\n\n");
    if results
        .iter()
        .any(|result| result.execution_error.is_some())
    {
        document.push_str(
            "**Pending orchestrator run.** This sandbox could not complete every live fixture, so no live baseline is claimed. The attempted run is recorded below for diagnosis.\n\n",
        );
    } else {
        document.push_str("This is the latest complete live run. Assertion failures remain part of the measured baseline; they are not rewritten as fixture success.\n\n");
    }
    let _ = write!(
        document,
        "- Date: `{}`\n- Harness: `{}`\n- Harness version: `{}`\n- Model: `{}`\n- Platform: `{}-{}`\n- Result artifact: `{}`\n\n",
        &environment.timestamp_utc[..10],
        environment.harness,
        environment.harness_version.as_deref().unwrap_or("unknown"),
        environment.model,
        environment.os,
        environment.architecture,
        output_path.display()
    );
    document.push_str(&render_scoreboard(results));
    document.push_str("\n### Failures\n\n");
    let mut failure_count = 0_usize;
    for result in results.iter().filter(|result| !result.passed) {
        failure_count += 1;
        let _ = write!(document, "- `{}`", result.name);
        if let Some(error) = &result.execution_error {
            let _ = writeln!(document, ": {error}");
        } else {
            let failed = result
                .assertions
                .iter()
                .filter(|assertion| !assertion.passed)
                .map(|assertion| format!("{} ({})", assertion.assertion, assertion.detail))
                .collect::<Vec<_>>()
                .join("; ");
            let _ = writeln!(document, ": {failed}");
        }
    }
    if failure_count == 0 {
        document.push_str("- None.\n");
    }
    document
}

#[cfg(test)]
mod tests {
    use kinewright_agent::eval::{
        BlindReviewForm, COLOR_WORKFLOW_NOT_APPLICABLE, EvalDeliverableResult, HumanRatings,
    };
    use kinewright_core::{
        apply_batch,
        cc7_scenarios::{
            CC7_B1_OPERATIONS, CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED, CC7_D2_OPERATIONS,
            CC7_LOG_CUBE_BYTES_REPORTED, CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
            CC7_LOG_INVERSE_MAX_CODE, CC7_LOG_P99_MAX_CODE16,
            CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS, CC7_LOOK_MIX_BASIS_POINTS,
            CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS, CC7_MATCH_PROPOSAL_B, CC7_MATCH_PROPOSAL_C1,
            CC7_MATCH_PROPOSAL_C2, CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES,
            CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS, CC7_TRACK_TOLERANCE_BASIS_POINTS,
            CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE, Cc7PersonPath, Cc7Scenario, Cc7Source,
            cc7_canonical_operations, cc7_log_lut_asset, cc7_lut_backed_canonical_operations,
            cc7_target_clip, cc7_tracking_sample_frames,
        },
    };

    use super::*;

    #[test]
    fn rerender_options_parse_delivery_and_audio_contracts() {
        assert_eq!(
            parse_delivery_profile("vertical_short").unwrap(),
            DeliveryProfile::VerticalShort
        );
        assert!(parse_delivery_profile("portrait-ish").is_err());

        let contract = parse_loudness_contract("-1800,-1400,-100").unwrap();
        assert_eq!(contract.minimum_integrated_lufs_hundredths, -1_800);
        assert_eq!(contract.maximum_integrated_lufs_hundredths, -1_400);
        assert_eq!(contract.maximum_sample_peak_dbfs_hundredths, -100);
        assert!(parse_loudness_contract("-1400,-1800,-100").is_err());
        assert!(parse_loudness_contract("-1800,-1400,1").is_err());

        let tail = parse_audio_tail_contract("5,-1600,25,-3000,25").unwrap();
        assert_eq!(tail.terminal_window_frames, TimeCode(5));
        assert_eq!(tail.maximum_sample_peak_dbfs_hundredths, -1_600);
        assert_eq!(tail.activity_window_frames, TimeCode(25));
        assert_eq!(tail.minimum_active_integrated_lufs_hundredths, -3_000);
        assert_eq!(tail.maximum_trailing_inactive_frames, TimeCode(25));
        assert!(parse_audio_tail_contract("0,-1600,25,-3000,25").is_err());
        assert!(parse_audio_tail_contract("5,1,25,-3000,25").is_err());
        assert!(parse_audio_tail_contract("5,-1600,25,-3000").is_err());
    }

    #[test]
    fn clean_selectable_frames_do_not_count_excluded_intervals() {
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture".to_owned(),
            duration: TimeCode(100),
            fps: Rational::new(25, 1).unwrap(),
            kind: kinewright_core::MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let exclusions = [SourceRangeExclusion {
            asset: asset.id,
            source_range: TimeCode(20)..TimeCode(40),
            reason: "baked transition".to_owned(),
        }];
        let clean = clean_selectable_project_frames(
            &[asset],
            Rational::new(25, 1).unwrap(),
            &[(AssetId(1), TimeCode(70))],
            &exclusions,
            TimeCode(10),
        )
        .unwrap();
        assert_eq!(clean, 80);
    }

    #[test]
    fn human_review_scoring_is_bound_to_the_machine_report_artifact() {
        let review_json = r#"{
          "schema_version": 1,
          "benchmark_id": "bench",
          "run_id": "run",
          "reviewer": "owner",
          "tasks": [{
            "task_id": "g3",
            "artifact_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "accepted": true,
            "ratings": {
              "story": 4.0,
              "pacing": 4.0,
              "visual_finish": 4.0,
              "audio_finish": 4.0,
              "captions": null,
              "delivery_readiness": 4.0
            },
            "not_applicable": ["captions"],
            "notes": null
          }]
        }"#;
        let mut review: HumanReviewFile = serde_json::from_str(review_json).unwrap();
        let report = serde_json::json!({
            "benchmark_id": "bench",
            "run_id": "run",
            "results": [{
                "name": "g3 real montage",
                "deliverable": {
                    "output_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            }]
        });
        verify_review_artifact_bindings(&review, &report).unwrap();

        review.tasks[0].artifact_sha256 =
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned());
        assert!(verify_review_artifact_bindings(&review, &report).is_err());
        review.tasks[0].artifact_sha256 = None;
        assert!(verify_review_artifact_bindings(&review, &report).is_err());
        review.tasks[0].accepted = None;
        verify_review_artifact_bindings(&review, &report).unwrap();
    }

    #[test]
    fn published_v1_manifest_tracks_the_executable_seed_suite() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v1/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["benchmark_id"], "kinewright-auto-edit-v1");
        let tasks = manifest["tasks"].as_array().unwrap();
        let definitions = seed_suite();
        assert_eq!(tasks.len(), definitions.len());
        for (task, definition) in tasks.iter().zip(&definitions) {
            assert_eq!(
                task["id"].as_str(),
                definition.name.split_whitespace().next()
            );
            assert_eq!(task["name"], definition.name[3..].to_owned());
            assert_eq!(task["prompt"], definition.prompts[0]);
            assert_eq!(task["budget"]["turns"], definition.budgets.max_turns);
            assert_eq!(
                task["budget"]["tool_calls"],
                definition.budgets.max_tool_calls
            );
            assert_eq!(
                task["budget"]["operations"],
                definition.budgets.max_operations
            );
            assert_eq!(task["budget"]["tokens"], definition.budgets.max_tokens);
            assert_eq!(
                task["budget"]["cost_usd"].as_f64(),
                definition.budgets.max_cost_usd
            );
            assert_eq!(
                task["budget"]["wall_time_ms"],
                u64::try_from(definition.budgets.max_wall_time.as_millis()).unwrap()
            );
            assert_eq!(task["budget"]["undos"], definition.budgets.max_undos);
        }
    }

    #[test]
    fn published_v1_baseline_reconciles_without_claiming_human_acceptance() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v1/baseline.json"
        ))
        .unwrap();
        let summary = &baseline["summary"];
        let tasks = baseline["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 7);
        assert_eq!(
            tasks.iter().filter(|task| task["passed"] == true).count(),
            usize::try_from(summary["tasks_passed"].as_u64().unwrap()).unwrap()
        );
        let passed_assertions: u64 = tasks
            .iter()
            .map(|task| task["assertions_passed"].as_u64().unwrap())
            .sum();
        let total_tokens: u64 = tasks
            .iter()
            .map(|task| task["tokens"].as_u64().unwrap())
            .sum();
        assert_eq!(passed_assertions, summary["assertions_passed"]);
        assert_eq!(total_tokens, summary["total_tokens"]);
        assert!(summary["human_first_pass_acceptance"].is_null());
    }

    #[test]
    fn published_v2_manifest_tracks_the_finished_cut_suite() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v2/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 2);
        assert_eq!(manifest["benchmark_id"], "kinewright-finished-cut-v2");
        let tasks = manifest["tasks"].as_array().unwrap();
        let definitions = finished_cut_suite();
        assert_eq!(tasks.len(), definitions.len());
        for (task, definition) in tasks.iter().zip(&definitions) {
            let deliverable = definition.deliverable.unwrap();
            assert_eq!(
                task["id"].as_str(),
                definition.name.split_whitespace().next()
            );
            assert_eq!(task["prompt"], definition.prompts[0]);
            assert_eq!(task["delivery"]["profile"], deliverable.profile.as_str());
            assert_eq!(task["delivery"]["proof_frames"], deliverable.proof_frames);
            assert_eq!(task["budget"]["turns"], definition.budgets.max_turns);
            assert_eq!(
                task["budget"]["tool_calls"],
                definition.budgets.max_tool_calls
            );
            assert_eq!(
                task["budget"]["operations"],
                definition.budgets.max_operations
            );
            assert_eq!(task["budget"]["tokens"], definition.budgets.max_tokens);
            assert_eq!(
                task["budget"]["cost_usd"].as_f64(),
                definition.budgets.max_cost_usd
            );
            assert_eq!(
                task["budget"]["wall_time_ms"],
                u64::try_from(definition.budgets.max_wall_time.as_millis()).unwrap()
            );
            assert_eq!(task["budget"]["undos"], definition.budgets.max_undos);
        }
    }

    #[test]
    fn published_v3_manifest_tracks_the_editorial_suite_and_ground_truth() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v3/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 3);
        assert_eq!(manifest["benchmark_id"], "kinewright-editorial-cut-v3");
        let definitions = editorial_cut_suite();
        assert_eq!(definitions.len(), 1);
        let definition = &definitions[0];
        let task = &manifest["tasks"][0];
        assert_eq!(
            task["id"].as_str(),
            definition.name.split_whitespace().next()
        );
        assert_eq!(task["prompt"], definition.prompts[0]);
        let deliverable = definition.deliverable.unwrap();
        assert_eq!(deliverable.profile, DeliveryProfile::VerticalShort);
        assert_eq!(
            deliverable.expected_transcript_word_set,
            Some("authored-dialogue")
        );
        assert_eq!(deliverable.maximum_word_error_rate_basis_points, 1_500);
        assert_eq!(task["delivery"]["profile"], deliverable.profile.as_str());
        assert_eq!(task["delivery"]["proof_frames"], deliverable.proof_frames);
        assert_eq!(
            task["delivery"]["maximum_word_error_rate_basis_points"],
            deliverable.maximum_word_error_rate_basis_points
        );
        assert_eq!(task["budget"]["turns"], definition.budgets.max_turns);
        assert_eq!(
            task["budget"]["tool_calls"],
            definition.budgets.max_tool_calls
        );
        assert_eq!(
            task["budget"]["operations"],
            definition.budgets.max_operations
        );
        assert_eq!(task["budget"]["tokens"], definition.budgets.max_tokens);
        assert_eq!(
            task["budget"]["wall_time_ms"],
            u64::try_from(definition.budgets.max_wall_time.as_millis()).unwrap()
        );
        assert_eq!(task["budget"]["undos"], definition.budgets.max_undos);
        assert!(
            task["machine_assertions"]
                .as_array()
                .is_some_and(|assertions| assertions.len() >= 15)
        );

        let truth: EditorialGroundTruth = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v3/ground-truth.json"
        ))
        .unwrap();
        assert_eq!(truth.schema_version, 1);
        assert_eq!(truth.takes.len(), 5);
        assert_eq!(truth.expected_take_order, ["take-01", "take-03", "take-04"]);
        assert_eq!(truth.takes.iter().filter(|take| take.accepted).count(), 3);
    }

    #[test]
    fn published_v4_manifest_tracks_the_dialogue_pacing_suite_and_ground_truth() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v4/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 4);
        assert_eq!(manifest["benchmark_id"], "kinewright-dialogue-pacing-v4");
        let definitions = dialogue_pacing_suite();
        assert_eq!(definitions.len(), 1);
        let definition = &definitions[0];
        let task = &manifest["tasks"][0];
        assert_eq!(
            task["id"].as_str(),
            definition.name.split_whitespace().next()
        );
        assert_eq!(task["prompt"], definition.prompts[0]);
        let deliverable = definition.deliverable.unwrap();
        assert_eq!(deliverable.profile, DeliveryProfile::VerticalShort);
        assert_eq!(
            deliverable.expected_transcript_word_set,
            Some("authored-dialogue")
        );
        assert_eq!(deliverable.maximum_word_error_rate_basis_points, 1_500);
        assert_eq!(task["delivery"]["profile"], deliverable.profile.as_str());
        assert_eq!(task["delivery"]["proof_frames"], deliverable.proof_frames);
        assert_eq!(
            task["delivery"]["maximum_word_error_rate_basis_points"],
            deliverable.maximum_word_error_rate_basis_points
        );
        assert_eq!(task["budget"]["turns"], definition.budgets.max_turns);
        assert_eq!(
            task["budget"]["tool_calls"],
            definition.budgets.max_tool_calls
        );
        assert_eq!(
            task["budget"]["operations"],
            definition.budgets.max_operations
        );
        assert_eq!(task["budget"]["tokens"], definition.budgets.max_tokens);
        assert_eq!(
            task["budget"]["wall_time_ms"],
            u64::try_from(definition.budgets.max_wall_time.as_millis()).unwrap()
        );
        assert_eq!(task["budget"]["undos"], definition.budgets.max_undos);
        assert!(definition.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::DialoguePauseBounds {
                minimum_project_frames: TimeCode(10),
                maximum_project_frames: TimeCode(40),
                capitalization_boundary_minimum_frames: TimeCode(4),
            }
        )));

        let truth: EditorialGroundTruth = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v4/ground-truth.json"
        ))
        .unwrap();
        assert_eq!(truth.schema_version, 1);
        assert_eq!(truth.takes.len(), 5);
        assert_eq!(truth.expected_take_order, ["take-01", "take-03", "take-04"]);
        assert_eq!(truth.takes.iter().filter(|take| take.accepted).count(), 3);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn published_v5_manifest_tracks_both_real_footage_families_and_fixture_packs() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 5);
        assert_eq!(manifest["benchmark_id"], "kinewright-generalization-v5");
        assert_eq!(manifest["status"], "in_progress");
        assert_eq!(
            manifest["score_layers"]["human"]["acceptance_required_for_scored_artifact"],
            true
        );
        assert_eq!(
            manifest["score_layers"]["human"]["not_applicable_dimensions_excluded_from_means"],
            true
        );
        for dimension in [
            "story",
            "pacing",
            "visual_finish",
            "audio_finish",
            "captions",
            "delivery_readiness",
        ] {
            assert_eq!(
                manifest["milestone_exit"]["minimum_mean_human_rating_by_applicable_dimension"]
                    [dimension],
                4.0,
                "every applicable human-rating dimension must clear the 4.0 gate: {dimension}"
            );
        }
        let definitions = generalization_suite();
        let tasks = manifest["tasks"].as_array().unwrap();
        assert_eq!(manifest["fixture_packs"].as_array().unwrap().len(), 5);
        assert!(
            manifest["fixture_packs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "benchmarks/auto-edit/v5/music-fixture-pack-v4.json")
        );
        assert_eq!(definitions.len(), 3);
        assert_eq!(tasks.len(), definitions.len());
        for (task, definition) in tasks.iter().zip(&definitions) {
            assert_eq!(
                task["id"].as_str(),
                definition.name.split_whitespace().next()
            );
            assert_eq!(task["prompt"], definition.prompts[0]);
            assert_eq!(
                task["budget"]["tool_calls"],
                definition.budgets.max_tool_calls
            );
            assert_eq!(task["budget"]["tokens"], definition.budgets.max_tokens);
        }
        let interview = &definitions[0];
        let deliverable = interview.deliverable.unwrap();
        assert_eq!(deliverable.profile, DeliveryProfile::VerticalShort);
        assert_eq!(
            deliverable.expected_transcript_word_set,
            Some("recovery-dialogue")
        );
        assert_eq!(deliverable.maximum_word_error_rate_basis_points, 0);
        assert_eq!(
            deliverable.maximum_caption_word_error_rate_basis_points,
            Some(0)
        );
        assert!(interview.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ExactSourceClips { clips }
                if clips == &[ExpectedSourceClip {
                    asset_alias: "interview-raw".to_owned(),
                    source_start: TimeCode(1_682),
                    source_end: TimeCode(2_547),
                }]
        )));
        assert!(interview.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::CaptionPresentation {
                allowed_positions,
                color_token: 0,
                background_scrim: false,
            } if allowed_positions == &[TitlePosition::LowerThird, TitlePosition::Top]
        )));
        let event = &definitions[1];
        let event_delivery = event.deliverable.unwrap();
        assert_eq!(
            event_delivery.expected_transcript_word_set,
            Some("event-dialogue")
        );
        assert_eq!(event_delivery.maximum_word_error_rate_basis_points, 3_000);
        assert!(event.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ExactTrackClips { track: TrackId(1), clips }
                if clips.len() == 5
        )));
        assert!(event.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ReframeStability {
                track: TrackId(1),
                minimum_keyframes_per_axis: 2,
                maximum_step_percent: 2,
                ..
            }
        )));
        let music = &definitions[2];
        assert_eq!(
            tasks[2]["ground_truth"],
            "benchmarks/auto-edit/v5/music-ground-truth-v10.json"
        );
        let music_delivery = music.deliverable.unwrap();
        assert_eq!(music_delivery.profile, DeliveryProfile::Youtube1080p);
        assert_eq!(music_delivery.expected_transcript_word_set, None);
        assert_eq!(
            music_delivery.audio_tail,
            Some(EvalAudioTailSpec {
                terminal_window_frames: TimeCode(5),
                maximum_sample_peak_dbfs_hundredths: -1_600,
                activity_window_frames: TimeCode(25),
                minimum_active_integrated_lufs_hundredths: -3_000,
                maximum_trailing_inactive_frames: TimeCode(25),
            })
        );
        assert_eq!(
            music_delivery.loudness,
            Some(EvalLoudnessSpec {
                minimum_integrated_lufs_hundredths: -1_800,
                maximum_integrated_lufs_hundredths: -1_400,
                maximum_sample_peak_dbfs_hundredths: -100,
            })
        );
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ClipCount {
                minimum: 7,
                maximum: 7,
            }
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::MediaClipCount {
                track: TrackId(1),
                minimum: 5,
                maximum: 5,
                minimum_duration: TimeCode(40),
                maximum_duration: TimeCode(140),
                reject_non_media: false,
            }
        )));
        assert!(
            !music
                .assertions
                .iter()
                .any(|assertion| matches!(assertion, EvalAssertion::BeatAlignedCuts { .. }))
        );
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::CutsAlignedToBeatSetAtLeast {
                track: TrackId(1),
                beat_set,
                tolerance_frames: TimeCode(1),
                minimum_aligned_cuts: 3,
                minimum_aligned_basis_points: 7_500,
            } if beat_set == MUSIC_REVIEWED_EVENT_SET
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::MusicFit {
                track: TrackId(2),
                asset_alias,
                source_beat_set,
                timeline_start: TimeCode(0),
                timeline_end: TimeCode(450),
                tolerance_source_frames: TimeCode(0),
            } if asset_alias == "music-bed" && source_beat_set == MUSIC_SOURCE_BEAT_SET
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::MusicSourceEnd {
                track: TrackId(2),
                asset_alias,
                expected_source_end: TimeCode(6_875),
                tolerance_source_frames: TimeCode(2),
            } if asset_alias == "music-bed"
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::RequiredAssetsOnTrack {
                track: TrackId(1),
                aliases,
            } if aliases == &["tears-of-steel".to_owned()]
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::AssetUseMinimum {
                track: TrackId(1),
                asset_alias,
                minimum_clip_count: 5,
                minimum_project_frames: TimeCode(388),
            } if asset_alias == "tears-of-steel"
        )));
        assert!(
            !music
                .assertions
                .iter()
                .any(|assertion| matches!(assertion, EvalAssertion::AssetTemporalSpread { .. }))
        );
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::SourceRangesSeparated {
                track: TrackId(1),
                minimum_separation_frames: TimeCode(0),
            }
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ExactProjectDuration {
                duration: TimeCode(450),
            }
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ExactTrackMediaCoverage {
                track: TrackId(1),
                range,
            } if range == &(TimeCode::ZERO..TimeCode(388))
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ExactTrackMediaCoverage {
                track: TrackId(2),
                range,
            } if range == &(TimeCode::ZERO..TimeCode(450))
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::SourceRangesSceneClean {
                track: TrackId(1),
                scene_set,
                ..
            } if scene_set == MUSIC_SOURCE_SCENE_SET
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ClipSourceWithin {
                track: TrackId(1),
                timeline_start: TimeCode(0),
                asset_alias,
                source_window,
            } if asset_alias == "tears-of-steel"
                && source_window == &(TimeCode(165)..TimeCode(221))
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::SourceRangesChronological {
                track: TrackId(1),
                minimum_forward_gap_frames: TimeCode(0),
            }
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::ShotCadenceVariation {
                track: TrackId(1),
                minimum_duration_buckets: 3,
                duration_bucket_frames: TimeCode(15),
                maximum_similar_run: 3,
                similar_tolerance_frames: TimeCode(6),
            }
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::SingleAudioMediaClip {
                track: TrackId(2),
                asset_alias,
            } if asset_alias == "music-bed"
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::NoAlternatingShotPattern {
                track: TrackId(1),
                maximum_repeated_pairs: 2,
                tolerance_frames: TimeCode(6),
            }
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::NoVisualTransitionsEffectsOrRetiming { track: TrackId(1) }
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::TitleCard {
                track: TrackId(1),
                timeline_start: TimeCode(388),
                duration: TimeCode(62),
                text,
                font_size_token: 2,
                color_token: 0,
                position: TitlePosition::Center,
                background_scrim: false,
                fade_in_frames: TimeCode(5),
                fade_out_frames: TimeCode(15),
            } if text == "TEARS OF STEEL"
        )));
        assert!(music.assertions.iter().any(|assertion| matches!(
            assertion,
            EvalAssertion::EdgeShotHolds {
                track: TrackId(1),
                minimum_opening_shot_frames: TimeCode(48),
                minimum_closing_shot_frames: TimeCode(125),
            }
        )));
        assert!(
            !music
                .assertions
                .iter()
                .any(|assertion| matches!(assertion, EvalAssertion::SourcePhaseArc { .. }))
        );
        let music_prompt = music.prompts[0];
        for required in [
            "Call get_source_shot_board exactly once over the full Tears of Steel source",
            "candidate_selection coverage",
            "candidate_count 12",
            "minimum_confidence_basis_points 1000",
            "Tears of Steel as the only photographed source",
            "preferred source start 6334",
            "preferred source end 6875",
            "maximum end drift 2 frames",
            "structural_only=false",
            "preferred anchors [48,126,263]",
            "source window 165..221",
            "frame 203",
            "exact activation select source 789..847",
            "durations 48, 78, 77, 60, and 125 frames",
            "At project frame 388 add one title clip",
            "TEARS OF STEEL",
            "fade_in_frames 5",
            "fade_out_frames 15",
            "Do not add a transition, subtitle, freeze frame",
            "cadence {minimum_duration_buckets:3, duration_bucket_frames:15, maximum_similar_run:3, similar_tolerance_frames:6}",
            "maximum_movement_frames 0",
            "10 storyboard frames",
            "no action after frame 388",
        ] {
            assert!(
                music_prompt.contains(required),
                "G3 prompt is missing required contract text: {required}"
            );
        }
        assert!(!music_prompt.contains("structural_only=true"));
        assert!(!music_prompt.contains("big-buck-bunny"));

        assert_v5_fixture_packs_and_truth();
    }

    fn assert_v5_fixture_packs_and_truth() {
        assert_interview_fixture_and_truth();
        assert_event_fixture_and_truth();
        assert_music_fixture_and_truth();
    }

    fn assert_interview_fixture_and_truth() {
        let pack = FixturePackManifest::from_json(include_str!(
            "../../../../benchmarks/auto-edit/v5/fixture-pack.json"
        ))
        .unwrap();
        assert_eq!(pack.pack_id, "m40-interview-v1");
        assert_eq!(pack.assets.len(), 1);
        assert_eq!(pack.assets[0].bytes, 9_294_247);
        assert_eq!(pack.assets[0].sha256.len(), 64);

        let truth: RealInterviewGroundTruth = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/ground-truth.json"
        ))
        .unwrap();
        assert_eq!(truth.schema_version, 1);
        assert_eq!(truth.source_story_range.start, 1_682);
        assert_eq!(truth.source_story_range.end, 2_547);
        assert_eq!(truth.required_terms.len(), 19);
        assert_ne!(truth.source_asr_dialogue, truth.expected_dialogue);
    }

    fn assert_event_fixture_and_truth() {
        let event_pack = FixturePackManifest::from_json(include_str!(
            "../../../../benchmarks/auto-edit/v5/event-fixture-pack.json"
        ))
        .unwrap();
        assert_eq!(event_pack.pack_id, "m40-event-multicam-v1");
        assert_eq!(event_pack.assets.len(), 6);
        assert_eq!(
            event_pack
                .assets
                .iter()
                .map(|asset| asset.bytes)
                .sum::<u64>(),
            245_654_467
        );

        let event_truth: EventMulticamGroundTruth = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/event-ground-truth.json"
        ))
        .unwrap();
        assert_eq!(event_truth.schema_version, 1);
        assert_eq!(event_truth.meeting_id, "ES2002a");
        assert_eq!(event_truth.speaker_assignments.len(), 4);
        assert_eq!(event_truth.expected_video_cuts.len(), 5);
        assert_eq!(event_truth.source_range.start, 1_750);
        assert_eq!(event_truth.source_range.end, 2_544);
        assert_eq!(event_truth.reframe.target_aspect_basis_points, 5_625);
        assert_eq!(event_truth.reframe.min_x_percent, 25);
        assert_eq!(event_truth.reframe.max_x_percent, 75);
        assert_eq!(event_truth.reframe.min_y_percent, 20);
        assert_eq!(event_truth.reframe.max_y_percent, 80);
        assert_eq!(event_truth.reframe.maximum_step_percent, 2);
    }

    fn assert_music_fixture_and_truth() {
        let music_pack = FixturePackManifest::from_json(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-fixture-pack-v4.json"
        ))
        .unwrap();
        assert_eq!(music_pack.pack_id, "m40-single-source-trailer-v4");
        assert_eq!(music_pack.assets.len(), 2);
        assert_eq!(
            music_pack
                .assets
                .iter()
                .map(|asset| asset.bytes)
                .sum::<u64>(),
            29_728_929
        );
        let music_truth: MusicMontageGroundTruth = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-ground-truth-v10.json"
        ))
        .unwrap();
        assert_eq!(music_truth.schema_version, 1);
        assert_eq!(
            music_truth.montage_id,
            "tears-of-steel-single-source-vanguard-title-card-resolution"
        );
        assert_eq!(music_truth.project_fps, Rational::new(25, 1).unwrap());
        assert_eq!(
            music_truth.visual_asset_ids,
            ["tears-of-steel-battle-720p".to_owned()]
        );
        assert_eq!(music_truth.music_asset_id, "vanguard-scott-buckley");
        assert_eq!(music_truth.timeline_range.start, 0);
        assert_eq!(music_truth.timeline_range.end, 450);
        assert_eq!(music_truth.minimum_visual_shots, 5);
        assert_eq!(music_truth.maximum_visual_shots, 5);
        assert_eq!(music_truth.minimum_shot_frames, 40);
        assert_eq!(music_truth.maximum_shot_frames, 140);
        assert!((music_truth.minimum_beat_strength_percent - 10.0).abs() < f64::EPSILON);
        assert_eq!(music_truth.minimum_source_separation_frames, 0);
        assert_eq!(music_truth.minimum_forward_gap_frames, 0);
        assert!((music_truth.source_scene_minimum_confidence_percent - 10.0).abs() < f64::EPSILON);
        assert_eq!(music_truth.source_exclusions.len(), 1);
        assert_eq!(music_truth.source_exclusions[0].source_range.start, 986);
        assert_eq!(music_truth.source_exclusions[0].source_range.end, 987);
        assert_eq!(music_truth.minimum_duration_buckets, 3);
        assert_eq!(music_truth.duration_bucket_frames, 15);
        assert_eq!(music_truth.maximum_similar_run, 3);
        assert_eq!(music_truth.similar_tolerance_frames, 6);
        assert_eq!(music_truth.meter_beats, 4);
        assert_eq!(music_truth.phrase_bars, 4);
        assert_eq!(music_truth.reviewed_music_events.len(), 3);
        assert_eq!(music_truth.reviewed_music_events[0].project_frame, 48);
        assert_eq!(music_truth.reviewed_music_events[2].project_frame, 263);
        assert_eq!(music_truth.reviewed_story_events.len(), 1);
        assert_eq!(music_truth.reviewed_story_events[0].project_frame, 203);
        assert_eq!(music_truth.title_card.track_id, 1);
        assert_eq!(music_truth.title_card.timeline_range.start, 388);
        assert_eq!(music_truth.title_card.timeline_range.end, 450);
        assert_eq!(music_truth.title_card.text, "TEARS OF STEEL");
        assert_eq!(music_truth.title_card.fade_in_frames, 5);
        assert_eq!(music_truth.title_card.fade_out_frames, 15);
        assert_eq!(music_truth.opening_source_window.start, 165);
        assert_eq!(music_truth.opening_source_window.end, 221);
        assert_eq!(music_truth.preparation_source_window.start, 221);
        assert_eq!(music_truth.preparation_source_window.end, 309);
        assert_eq!(music_truth.operator_source_window.start, 482);
        assert_eq!(music_truth.operator_source_window.end, 635);
        assert_eq!(music_truth.activation_source_window.start, 789);
        assert_eq!(music_truth.activation_source_window.end, 847);
        assert_eq!(music_truth.climax_source_window.start, 987);
        assert_eq!(music_truth.climax_source_window.end, 1_118);
        assert_eq!(music_truth.minimum_clips_per_visual_asset, 5);
        assert_eq!(music_truth.minimum_project_frames_per_visual_asset, 388);
        assert_eq!(music_truth.music_preferred_source_start, 6_334);
        assert_eq!(music_truth.music_preferred_source_end, 6_875);
        assert_eq!(music_truth.music_maximum_end_drift_frames, 2);
        assert_eq!(music_truth.minimum_opening_shot_frames, 48);
        assert_eq!(music_truth.minimum_closing_shot_frames, 125);
        assert_eq!(music_truth.rendered_tail_window_frames, 5);
        assert_eq!(
            music_truth.rendered_tail_maximum_peak_dbfs_hundredths,
            -1_600
        );
        assert_eq!(music_truth.rendered_activity_window_frames, 25);
        assert_eq!(
            music_truth.rendered_activity_minimum_integrated_lufs_hundredths,
            -3_000
        );
        assert_eq!(music_truth.maximum_trailing_inactive_frames, 25);
    }

    #[test]
    fn published_v5_caption_recovery_keeps_machine_and_human_truth_separate() {
        let rejected: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/baseline.json"
        ))
        .unwrap();
        let recovery: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/caption-recovery-baseline.json"
        ))
        .unwrap();

        assert_eq!(rejected["human_review"]["status"], "reviewed_rejected");
        assert_eq!(recovery["machine_summary"]["assertions_passed"], 28);
        assert_eq!(recovery["machine_summary"]["assertions_total"], 28);
        assert_eq!(
            recovery["deliverable"]["rendered_word_error_rate_basis_points"],
            0
        );
        assert_eq!(
            recovery["deliverable"]["rendered_caption_audio_word_error_rate_basis_points"],
            0
        );
        assert_eq!(recovery["deliverable"]["source_range"]["end"], 2_547);
        assert_eq!(recovery["human_review"]["status"], "pending");
        assert_ne!(
            rejected["deliverable"]["output_sha256"],
            recovery["deliverable"]["output_sha256"]
        );
    }

    #[test]
    fn published_v5_event_baseline_preserves_machine_success_and_human_rejection() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/event-multicam-baseline.json"
        ))
        .unwrap();
        assert_eq!(baseline["benchmark_id"], "kinewright-generalization-v5");
        assert_eq!(baseline["scope"]["status"], "event_multicam_preflight");
        assert_eq!(baseline["machine_summary"]["samples_passed"], 1);
        assert_eq!(baseline["machine_summary"]["assertions_passed"], 23);
        assert_eq!(baseline["machine_summary"]["assertions_total"], 23);
        assert_eq!(baseline["machine_summary"]["tool_calls"], 20);
        assert_eq!(baseline["machine_summary"]["tool_call_budget"], 24);
        assert_eq!(baseline["deliverable"]["video_shots"], 5);
        assert_eq!(baseline["deliverable"]["program_audio_clips"], 1);
        assert_eq!(baseline["deliverable"]["tracked_reframe_clips"], 5);
        assert_eq!(baseline["human_review"]["status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["accepted"], false);
        assert_eq!(baseline["benchmark_status"], "in_progress");
    }

    #[test]
    fn published_v5_music_baseline_preserves_machine_success_and_human_rejection() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-montage-baseline.json"
        ))
        .unwrap();
        assert_eq!(baseline["benchmark_id"], "kinewright-generalization-v5");
        assert_eq!(baseline["scope"]["status"], "music_montage_preflight");
        assert_eq!(baseline["machine_summary"]["samples_passed"], 1);
        assert_eq!(baseline["machine_summary"]["assertions_passed"], 22);
        assert_eq!(baseline["machine_summary"]["assertions_total"], 22);
        assert_eq!(baseline["machine_summary"]["tool_calls"], 11);
        assert_eq!(baseline["machine_summary"]["tool_call_budget"], 24);
        assert_eq!(baseline["machine_summary"]["operations_applied"], 12);
        assert_eq!(baseline["machine_summary"]["total_tokens"], 154_358);
        assert_eq!(baseline["deliverable"]["visual_shots"], 10);
        assert_eq!(baseline["deliverable"]["music_clips"], 1);
        assert_eq!(baseline["deliverable"]["duration_frames"], 800);
        assert_eq!(
            baseline["deliverable"]["rendered_integrated_lufs_hundredths"],
            -1_602
        );
        assert_eq!(baseline["human_review"]["status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["accepted"], false);
        assert_eq!(baseline["human_review"]["ratings"]["story"], 1.0);
        assert_eq!(baseline["human_review"]["ratings"]["pacing"], 1.5);
        assert_eq!(baseline["human_review"]["ratings"]["visual_finish"], 2.0);
        assert_eq!(baseline["human_review"]["ratings"]["audio_finish"], 4.5);
        assert!(baseline["human_review"]["ratings"]["captions"].is_null());
        assert!(baseline["human_review"]["ratings"]["delivery_readiness"].is_null());
        assert_eq!(baseline["benchmark_status"], "in_progress");
    }

    #[test]
    fn published_v5_music_recovery_keeps_machine_success_and_human_review_separate() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-montage-recovery-baseline.json"
        ))
        .unwrap();
        assert_eq!(
            baseline["supersedes_artifact"]["human_status"],
            "reviewed_rejected"
        );
        assert_eq!(baseline["fixture"]["pack_id"], "m40-music-montage-v2");
        assert_eq!(baseline["machine_summary"]["assertions_passed"], 34);
        assert_eq!(baseline["machine_summary"]["assertions_total"], 34);
        assert_eq!(baseline["machine_summary"]["tool_calls"], 15);
        assert_eq!(baseline["deliverable"]["duration_frames"], 600);
        assert_eq!(baseline["deliverable"]["visual_shots"], 9);
        assert_eq!(baseline["deliverable"]["structural_cut_count"], 6);
        assert_eq!(
            baseline["deliverable"]["independent_frame_audit"]["status"],
            "passed"
        );
        assert_eq!(baseline["human_review"]["status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["accepted"], false);
        assert_eq!(baseline["human_review"]["ratings"]["story"], 2.5);
        assert_eq!(baseline["human_review"]["ratings"]["pacing"], 3.5);
        assert_eq!(baseline["human_review"]["ratings"]["visual_finish"], 4.0);
        assert_eq!(baseline["human_review"]["ratings"]["audio_finish"], 2.0);
        assert_eq!(
            baseline["human_review"]["ratings"]["delivery_readiness"],
            2.0
        );
        assert!(baseline["human_review"]["ratings"]["captions"].is_null());
        assert_eq!(baseline["benchmark_status"], "in_progress");
    }

    #[test]
    fn published_v7_trailer_candidate_binds_clean_edges_and_rejection() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-trailer-v7-baseline.json"
        ))
        .unwrap();
        assert_eq!(baseline["machine_summary"]["assertions_passed"], 39);
        assert_eq!(baseline["machine_summary"]["assertions_total"], 39);
        assert_eq!(baseline["machine_summary"]["tool_calls"], 12);
        assert_eq!(
            baseline["deliverable"]["climax_source_range"],
            serde_json::json!({"start": 987, "end": 1108})
        );
        assert_eq!(
            baseline["deliverable"]["resolution_source_range"],
            serde_json::json!({"start": 309, "end": 381})
        );
        assert_eq!(
            baseline["deliverable"]["independent_cut_audit"]["status"],
            "passed"
        );
        assert_eq!(baseline["supersedes"]["human_status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["accepted"], false);
        assert_eq!(baseline["benchmark_status"], "in_progress");
    }

    #[test]
    fn published_v8_trailer_candidate_binds_connected_opening_and_rejection() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-trailer-v8-baseline.json"
        ))
        .unwrap();
        assert_eq!(baseline["machine_summary"]["assertions_passed"], 40);
        assert_eq!(baseline["machine_summary"]["assertions_total"], 40);
        assert_eq!(baseline["machine_summary"]["tool_calls"], 16);
        assert_eq!(
            baseline["deliverable"]["opening_source_range"],
            serde_json::json!({"start": 716, "end": 762})
        );
        assert_eq!(
            baseline["deliverable"]["independent_story_audit"]["status"],
            "passed"
        );
        assert_eq!(
            baseline["deliverable"]["independent_cut_audit"]["status"],
            "passed"
        );
        assert_eq!(baseline["supersedes"]["human_status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["accepted"], false);
        assert_eq!(baseline["benchmark_status"], "in_progress");
    }

    #[test]
    fn published_v9_trailer_candidate_binds_chronology_and_pending_review() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-trailer-v9-baseline.json"
        ))
        .unwrap();
        assert_eq!(baseline["machine_summary"]["assertions_passed"], 43);
        assert_eq!(baseline["machine_summary"]["assertions_total"], 43);
        assert_eq!(baseline["machine_summary"]["tool_calls"], 12);
        assert_eq!(
            baseline["deliverable"]["story_source_ranges"],
            serde_json::json!([
                {"role": "threat", "start": 165, "end": 211},
                {"role": "preparation", "start": 221, "end": 296},
                {"role": "operator", "start": 482, "end": 613},
                {"role": "battle", "start": 987, "end": 1107},
                {"role": "aftermath", "start": 1285, "end": 1345}
            ])
        );
        assert_eq!(
            baseline["deliverable"]["independent_story_audit"]["status"],
            "passed"
        );
        assert_eq!(
            baseline["deliverable"]["independent_cut_audit"]["status"],
            "passed"
        );
        assert_eq!(baseline["supersedes"]["human_status"], "reviewed_rejected");
        assert_eq!(baseline["human_review"]["status"], "pending");
        assert!(baseline["human_review"]["accepted"].is_null());
        assert_eq!(baseline["benchmark_status"], "in_progress");
        assert!(
            baseline["machine_gap_audit"]["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change
                    .as_str()
                    .unwrap()
                    .contains("source_ranges_chronological"))
        );
    }

    #[test]
    fn published_v10_trailer_candidate_binds_title_resolution_and_pending_review() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/music-trailer-v10-baseline.json"
        ))
        .unwrap();
        assert_eq!(
            baseline["ground_truth"],
            "benchmarks/auto-edit/v5/music-ground-truth-v10.json"
        );
        assert_eq!(baseline["environment"]["os"], "linux");
        assert_eq!(baseline["machine_summary"]["assertions_passed"], 43);
        assert_eq!(baseline["machine_summary"]["assertions_total"], 43);
        assert_eq!(baseline["machine_summary"]["tool_calls"], 19);
        assert_eq!(baseline["deliverable"]["photographed_frames"], 388);
        assert_eq!(
            baseline["deliverable"]["story_cut_frames"],
            serde_json::json!([203])
        );
        assert_eq!(
            baseline["deliverable"]["title_card"],
            serde_json::json!({
                "timeline_range": {"start": 388, "end": 450},
                "text": "TEARS OF STEEL",
                "font_size_token": 2,
                "color_token": 0,
                "position": "center",
                "background_scrim": false,
                "fade_in_frames": 5,
                "fade_out_frames": 15
            })
        );
        assert_eq!(
            baseline["deliverable"]["story_source_ranges"][3],
            serde_json::json!({"role": "activation", "start": 789, "end": 847})
        );
        assert_eq!(
            baseline["deliverable"]["independent_story_audit"]["status"],
            "passed"
        );
        assert_eq!(
            baseline["deliverable"]["independent_cut_audit"]["status"],
            "passed"
        );
        assert_eq!(
            baseline["director_reference"]["status"],
            "accepted_editorial_direction"
        );
        assert_eq!(baseline["human_review"]["status"], "pending");
        assert!(baseline["human_review"]["accepted"].is_null());
        assert_eq!(baseline["benchmark_status"], "in_progress");
    }

    #[test]
    fn v3_visual_scenes_render_with_the_pinned_ffmpeg() {
        for (index, (visual, role)) in [
            ("empty_lot", "opening"),
            ("empty_lot_retake", "opening_alternate"),
            ("garden_build", "community_action"),
            ("market_result", "result"),
            ("market_wrong_day", "result_alternate"),
        ]
        .into_iter()
        .enumerate()
        {
            let source = documentary_visual(visual, role).unwrap();
            let generated = GeneratedMedia::ffmpeg(
                &format!("v3-scene-{index}"),
                &["-f", "lavfi", "-i", &source, "-t", "0.1", "-c:v", "libx264"],
                "mp4",
            );
            assert!(generated.path().metadata().unwrap().len() > 0);
        }
    }

    #[test]
    fn acoustic_word_scoring_ignores_asr_words_fully_inside_detected_silence() {
        let fps = Rational::new(30, 1).unwrap();
        let transcript = AssetTranscript {
            asset: kinewright_core::AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![
                kinewright_core::TranscriptWord {
                    text: "audible".to_owned(),
                    source_start: TimeCode(10),
                    source_end: TimeCode(20),
                    speaker: None,
                },
                kinewright_core::TranscriptWord {
                    text: "late".to_owned(),
                    source_start: TimeCode(30),
                    source_end: TimeCode(40),
                    speaker: None,
                },
            ],
        };
        let silences = AssetSilences {
            asset: transcript.asset,
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            source_frames: TimeCode(60),
            threshold_dbfs_hundredths: -3_500,
            window_milliseconds: 10,
            spans: vec![kinewright_core::SilenceSpan {
                source_start: TimeCode(25),
                source_end: TimeCode(45),
            }],
        };

        assert_eq!(
            acoustically_scorable_words(&transcript, &silences),
            ["audible"]
        );
    }

    #[test]
    #[ignore = "requires local speech synthesis and Whisper analysis"]
    fn v3_fixture_builds_with_authored_and_recognized_truth() {
        let fixture = fixture_editorial_story().unwrap();
        assert_eq!(fixture.original_document.media_pool.len(), 5);
        assert_eq!(fixture.original_document.resolution, (360, 640));
        assert_eq!(fixture.context.asset_aliases.len(), 5);
        assert_eq!(
            fixture.context.word_sets["selected-recognized-fillers"],
            ["um"]
        );
        for take in ["take-01", "take-03", "take-04"] {
            assert!(!fixture.context.word_sets[&format!("{take}-recognized-content")].is_empty());
        }
    }

    #[test]
    #[ignore = "requires the explicitly prepared M40 fixture pack and Whisper analysis"]
    fn v5_fixture_builds_from_verified_real_media() {
        let fixture = fixture_real_interview_story().unwrap();
        assert_eq!(fixture.original_document.media_pool.len(), 1);
        assert_eq!(fixture.original_document.resolution, (720, 1280));
        assert_eq!(fixture.context.asset_aliases.len(), 1);
        assert_eq!(fixture.context.word_sets["recovery-required"].len(), 19);
        assert_eq!(fixture.context.word_sets["off-story-exclusions"].len(), 7);
        assert!(fixture.context.word_sets["recovery-dialogue"].len() > 60);
        assert!(fixture.context.word_sets["recovery-dialogue"].contains(&"8".to_owned()));
        assert!(fixture.context.word_sets["recovery-dialogue"].contains(&"12".to_owned()));
    }

    #[test]
    #[ignore = "requires the explicitly prepared M40 event fixture pack"]
    fn v5_event_fixture_builds_with_real_sync_audio_and_speaker_labels() {
        let fixture = fixture_real_event_multicam().unwrap();
        let document = &fixture.original_document;
        assert_eq!(document.media_pool.len(), 5);
        assert_eq!(document.tracks.len(), 2);
        assert_eq!(document.duration, TimeCode(794));
        assert_eq!(document.catalog.sync_groups.len(), 1);
        assert_eq!(document.catalog.sync_groups[0].members.len(), 5);
        assert_eq!(fixture.context.asset_aliases.len(), 5);
        let reference = document
            .media_pool
            .iter()
            .find(|asset| asset.name == "camera-laura")
            .unwrap();
        let TranscriptStatus::Ready(transcript) = fixture.analysis.transcript_status(reference)
        else {
            panic!("registered AMI transcript should be ready");
        };
        let speakers = transcript
            .words
            .iter()
            .filter_map(|word| word.speaker.as_deref())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            speakers,
            BTreeSet::from(["Andrew", "Craig", "David", "Laura"])
        );
        let plan = kinewright_core::plan_speaker_multicam(
            document,
            &transcript,
            &kinewright_core::SpeakerMulticamSettings {
                sync_group: SyncGroupId(1),
                target_track: TrackId(1),
                group_start: TimeCode(1_750),
                group_end: TimeCode(2_544),
                record_start: TimeCode::ZERO,
                maximum_word_gap_frames: TimeCode(5),
                minimum_shot_frames: TimeCode(25),
                assignments: vec![
                    kinewright_core::SpeakerAngleAssignment {
                        speaker: "Laura".to_owned(),
                        angle_name: "Laura closeup".to_owned(),
                    },
                    kinewright_core::SpeakerAngleAssignment {
                        speaker: "David".to_owned(),
                        angle_name: "David closeup".to_owned(),
                    },
                    kinewright_core::SpeakerAngleAssignment {
                        speaker: "Andrew".to_owned(),
                        angle_name: "Andrew closeup".to_owned(),
                    },
                    kinewright_core::SpeakerAngleAssignment {
                        speaker: "Craig".to_owned(),
                        angle_name: "Craig closeup".to_owned(),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(plan.suppressed_short_shots, 3);
        assert_eq!(plan.cuts.len(), 5);
        assert_eq!(
            plan.cuts
                .iter()
                .map(|cut| (
                    cut.angle_name.as_str(),
                    cut.timeline_start.0,
                    cut.timeline_end.0
                ))
                .collect::<Vec<_>>(),
            vec![
                ("Laura closeup", 0, 184),
                ("David closeup", 184, 287),
                ("Andrew closeup", 287, 380),
                ("Craig closeup", 380, 475),
                ("Laura closeup", 475, 794),
            ]
        );
    }

    #[test]
    #[ignore = "requires the explicitly prepared M40 music fixture pack and beat analysis"]
    #[allow(clippy::too_many_lines)]
    fn v5_music_fixture_builds_with_real_media_and_pinned_beats() {
        let fixture = fixture_real_music_montage().unwrap();
        let document = &fixture.original_document;
        assert_eq!(document.media_pool.len(), 2);
        assert_eq!(document.tracks.len(), 2);
        assert_eq!(document.duration, TimeCode::ZERO);
        assert_eq!(document.fps, Rational::new(25, 1).unwrap());
        assert_eq!(document.resolution, (1_920, 1_080));
        assert_eq!(fixture.context.asset_aliases.len(), 2);
        let source_beats = &fixture.context.source_beat_sets[MUSIC_SOURCE_BEAT_SET];
        let project_beats = &fixture.context.timeline_beat_sets[MUSIC_PROJECT_BEAT_SET];
        let source_scenes = &fixture.context.scene_sets[MUSIC_SOURCE_SCENE_SET];
        println!(
            "Vanguard fixture: source_beats={} project_beats={} strong_source_scene_boundaries={}",
            source_beats.len(),
            project_beats.len(),
            source_scenes.len()
        );
        assert!(!source_beats.is_empty());
        assert!(!project_beats.is_empty());
        assert!(!source_scenes.is_empty());
        assert_eq!(
            fixture.context.asset_aliases["tears-of-steel"],
            document.media_pool[0].id
        );
        assert_eq!(
            fixture.context.asset_aliases["music-bed"],
            document.media_pool[1].id
        );

        let mut planned_document = document.clone();
        let music = planned_document.media_pool[1].clone();
        let status = fixture.analysis.beat_status(&music);
        let music_plan = kinewright_core::music_fit_plan_with_end_anchor(
            &planned_document,
            TrackId(2),
            music.id,
            TimeCode::ZERO..TimeCode(450),
            Some(TimeCode(6_334)),
            Some(kinewright_core::MusicEndAnchor {
                preferred_source_end: TimeCode(6_875),
                maximum_drift_frames: TimeCode(2),
            }),
            &status,
            1_000,
            ThreePointMode::Overwrite,
        )
        .unwrap();
        assert_eq!(music_plan.timeline_range, TimeCode::ZERO..TimeCode(450));
        assert!(
            music_plan.source_range.end.0.abs_diff(6_875) <= 2,
            "resolved source range was {:?}",
            music_plan.source_range
        );
        let end_anchor = music_plan.end_anchor.unwrap();
        assert_eq!(end_anchor.target_source_end, TimeCode(6_875));
        assert!(end_anchor.signed_offset_frames.unsigned_abs() <= 2);
        println!(
            "Vanguard resolved music range: {:?}, endpoint offset={} source frames",
            music_plan.source_range, end_anchor.signed_offset_frames
        );
        kinewright_core::apply_batch(&mut planned_document, &music_plan.operations).unwrap();
        let timeline_beats = fixture
            .analysis
            .timeline_beats(
                &planned_document,
                Some(TimeCode::ZERO..TimeCode(450)),
                1_000,
            )
            .unwrap();
        let structure = kinewright_core::music_structure_analysis(
            &planned_document,
            music.id,
            TimeCode::ZERO..TimeCode(450),
            &timeline_beats,
            &kinewright_core::TimelineBeatAnalysisState::Ready,
            1_000,
            4,
            4,
        )
        .unwrap();
        println!(
            "Vanguard inferred structure: parameters={:?} candidates={:?}",
            structure.parameters,
            structure
                .candidates
                .iter()
                .map(|candidate| (
                    candidate.project_frame,
                    candidate.role,
                    candidate.strength_basis_points,
                    candidate.confidence_basis_points,
                ))
                .collect::<Vec<_>>()
        );
        assert!(
            structure
                .candidates
                .iter()
                .any(|candidate| candidate.role == kinewright_core::MusicStructureRole::Phrase)
        );
        assert!(structure.candidates.len() >= 3);

        let visual = planned_document.media_pool[0].clone();
        let selects = [165..221, 221..309, 482..635, 987..1_118, 1_285..1_345].map(|range| {
            kinewright_core::BeatMontageSelect {
                asset: visual.id,
                source_range: TimeCode(range.start)..TimeCode(range.end),
            }
        });
        let anchors = [48, 126, 263, 388].map(TimeCode);
        let montage = kinewright_core::beat_montage_plan_with_anchors(
            &planned_document,
            TrackId(1),
            music.id,
            TimeCode::ZERO..TimeCode(450),
            &selects,
            &anchors,
            &timeline_beats,
            &kinewright_core::TimelineBeatAnalysisState::Ready,
            1_000,
            TimeCode(40),
            TimeCode(140),
            ThreePointMode::Overwrite,
        )
        .unwrap();
        let cadence = kinewright_core::validate_beat_montage_plan_cadence(
            &montage,
            kinewright_core::BeatMontageCadenceContract {
                minimum_duration_buckets: 3,
                duration_bucket_frames: TimeCode(15),
                maximum_similar_run: 3,
                similar_tolerance_frames: TimeCode(6),
            },
        )
        .unwrap();
        assert_eq!(montage.shots.len(), 5);
        assert!(cadence.distinct_buckets.len() >= 3);
        assert!(cadence.longest_similar_run <= 3);
    }

    #[test]
    fn published_v2_baseline_keeps_machine_and_human_results_separate() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v2/baseline.json"
        ))
        .unwrap();
        let machine = &baseline["machine_summary"];
        let deliverable = &baseline["deliverable"];
        assert_eq!(machine["tasks_passed"], machine["tasks_total"]);
        assert_eq!(machine["assertions_passed"], machine["assertions_total"]);
        assert_eq!(
            machine["input_tokens"].as_u64().unwrap() + machine["output_tokens"].as_u64().unwrap(),
            machine["total_tokens"]
        );
        assert_eq!(
            deliverable["duration_frames"],
            deliverable["probed_duration_frames"]
        );
        assert_eq!(deliverable["resolution"], deliverable["probed_resolution"]);
        assert_eq!(deliverable["output_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(baseline["human_review"]["first_pass_acceptance"], false);
        assert_eq!(baseline["human_review"]["mean_rating"], 2.25);
        assert_eq!(baseline["human_review"]["ratings"]["pacing"], 2.5);
    }

    #[test]
    fn published_v4_baseline_records_machine_success_and_pending_human_review() {
        let baseline: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v4/baseline.json"
        ))
        .unwrap();
        let machine = &baseline["machine_summary"];
        assert_eq!(machine["samples_passed"], 3);
        assert_eq!(machine["samples_total"], 3);
        assert_eq!(machine["assertions_passed"], 102);
        assert_eq!(machine["assertions_total"], 102);
        assert_eq!(
            baseline["deliverable"]["sentence_boundaries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|boundary| boundary["gap_project_frames"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [33, 15, 23, 16]
        );
        assert_eq!(machine["tool_calls"], 24);
        assert_eq!(machine["mean_total_tokens"], 108_296.333_3);
        let samples = baseline["samples"].as_array().unwrap();
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample["total_tokens"].as_u64().unwrap())
                .sum::<u64>(),
            machine["total_tokens"].as_u64().unwrap()
        );
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample["tool_calls"].as_u64().unwrap())
                .sum::<u64>(),
            machine["tool_calls"].as_u64().unwrap()
        );
        let artifact_hash = samples[0]["output_sha256"].as_str().unwrap();
        assert!(
            samples
                .iter()
                .all(|sample| sample["output_sha256"] == artifact_hash)
        );
        assert_eq!(
            baseline["deliverable"]["unique_artifacts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            baseline["deliverable"]["caption_cues"]
                .as_array()
                .unwrap()
                .iter()
                .all(|cue| {
                    let words = cue.as_str().unwrap().split_whitespace().collect::<Vec<_>>();
                    words
                        .iter()
                        .take(words.len().saturating_sub(1))
                        .all(|word| !word.ends_with(['.', '!', '?']))
                })
        );
        assert_eq!(baseline["human_review"]["status"], "pending");
        assert_eq!(baseline["benchmark_status"], "pending_human_review");
    }

    // -----------------------------------------------------------------------
    // CC7 §8.5 / §11.2.32 — the blind package and its key.
    // -----------------------------------------------------------------------

    /// Everything the reviewer must not be able to read off the package.
    ///
    /// The scenario identity is deliberately absent from this list: it is
    /// inherent in the question being asked, and CC7 states that plainly
    /// rather than claiming a blindness the tooling cannot deliver.
    ///
    /// The machine-provenance words are the only literals here. Every task id
    /// comes from the suite and every parameter name comes from a
    /// `cc7_scenarios` canonical operation, so a parameter a later slice adds
    /// to a canonical document is scanned for without anyone remembering to
    /// add it (B-E9).
    const CC7_MACHINE_PROVENANCE_NEEDLES: [&str; 7] = [
        "-sample-", "agent", "person", "model", "harness", "passed", "assert",
    ];

    /// The two descriptor neutrals the canonical documents deliberately do
    /// **not** store, and which therefore cannot be read off an operation:
    /// `input_encoding_token = 0` (§2.5, (c)) and `mix_basis_points = 10 000`
    /// (§2.5, (e)). A leaking form would still name them.
    const CC7_UNSTORED_NEUTRAL_PARAMETERS: [&str; 2] = ["input_encoding_token", "mix_basis_points"];

    /// Every parameter name a CC7 canonical document can carry, from
    /// `cc7_scenarios` rather than from a list somebody maintains by hand.
    fn cc7_canonical_parameter_names() -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for scenario in CC7_SCENARIOS {
            for operation in cc7_spec(scenario).canonical_operations {
                names.extend(
                    operation
                        .parameters
                        .iter()
                        .map(|(name, _)| (*name).to_owned()),
                );
            }
        }
        for operations in [CC7_B1_OPERATIONS.as_slice(), CC7_D2_OPERATIONS.as_slice()] {
            for operation in operations {
                names.extend(
                    operation
                        .parameters
                        .iter()
                        .map(|(name, _)| (*name).to_owned()),
                );
            }
        }
        names.extend(CC7_F_KEYFRAMED_PARAMETERS.map(str::to_owned));
        names.extend(CC7_UNSTORED_NEUTRAL_PARAMETERS.map(str::to_owned));
        names
    }

    /// Everything the reviewer must not be able to read off the package:
    /// the suite's task ids, **the run id, the benchmark id**, the
    /// machine-provenance words, and every canonical parameter name.
    ///
    /// §8.5 names all three identifier classes. The run and the benchmark are
    /// the caller's, not this module's, so they are passed in: a scan that
    /// hard-coded them would only ever prove the fixture's own ids absent.
    fn cc7_leak_needles(run_id: &str, benchmark_id: &str) -> Vec<String> {
        let mut needles = color_workflow_suite()
            .iter()
            .filter_map(|definition| definition.name.split_whitespace().next())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        needles.push(run_id.to_ascii_lowercase());
        needles.push(benchmark_id.to_ascii_lowercase());
        needles.extend(CC7_MACHINE_PROVENANCE_NEEDLES.map(str::to_owned));
        needles.extend(cc7_canonical_parameter_names());
        needles
    }

    /// Canonical parameter values, as a reviewer would see them rendered.
    ///
    /// The set is deliberately the *distinctive* ones — every value here is at
    /// least three digits, because a two-digit needle would match a digit pair
    /// anywhere in the package and the scan would stop meaning anything.
    fn cc7_leak_value_needles() -> Vec<String> {
        [
            CC7_MATCH_PROPOSAL_B.exposure_milli_stops,
            CC7_MATCH_PROPOSAL_C1.exposure_milli_stops,
            CC7_MATCH_PROPOSAL_C2.exposure_milli_stops,
            CC7_LOOK_MIX_BASIS_POINTS,
            CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
            CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES,
        ]
        .into_iter()
        .map(|value| value.to_string())
        .collect()
    }

    fn cc7_blind_hash(prefix: &str) -> String {
        assert_eq!(prefix.len(), 12);
        format!("{prefix}{}", "0".repeat(52))
    }

    fn cc7_packaged_result(name: &str, directory: &Path, hash: &str) -> EvalResult {
        let artifacts = directory.join("artifacts").join(
            name.split_whitespace()
                .next()
                .expect("a task name has an id"),
        );
        fs::create_dir_all(&artifacts).expect("artifact directory");
        let output_path = artifacts.join("finished.mp4");
        let proof_path = artifacts.join("proof.png");
        fs::write(&output_path, b"finished-media").expect("finished media");
        fs::write(&proof_path, b"proof-sheet").expect("proof sheet");
        EvalResult {
            name: name.to_owned(),
            rationale: "colour workflow".to_owned(),
            passed: true,
            assertions: Vec::new(),
            measurements: Vec::new(),
            turns: 1,
            tool_calls: BTreeMap::new(),
            input_tokens: 0,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens: 0,
            reasoning_output_tokens: None,
            tool_surface: kinewright_agent::ToolSurfaceMetrics::default(),
            cost_usd: None,
            wall_time_ms: 0,
            operations_applied: 0,
            deliverable: Some(EvalDeliverableResult {
                profile: DeliveryProfile::SourceMaster,
                output_path,
                document_path: artifacts.join("final-document.json"),
                proof_path,
                resolution: (320, 180),
                duration_frames: TimeCode(60),
                conformance: None,
                output_bytes: Some(14),
                output_sha256: Some(hash.to_owned()),
                exported_frames: Some(60),
                probed_resolution: Some((320, 180)),
                probed_duration_frames: Some(TimeCode(60)),
                probed_media_kind: None,
                rendered_transcript_required: false,
                rendered_transcript: None,
                rendered_caption_alignment_required: false,
                rendered_caption_alignment: None,
                rendered_loudness_contract: None,
                rendered_loudness: None,
                rendered_audio_tail_contract: None,
                rendered_audio_tail: None,
                rendered_reframe: None,
                proof_sample_frames: Vec::new(),
                machine_passed: true,
                errors: Vec::new(),
            }),
            execution_error: None,
        }
    }

    fn cc7_blind_listing(blind_directory: &Path) -> Vec<String> {
        let mut names = fs::read_dir(blind_directory)
            .expect("blind directory")
            .map(|entry| {
                entry
                    .expect("blind directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    /// `<12 lowercase hex>.png` or `.mp4`, and nothing else.
    fn cc7_is_blind_media_name(name: &str) -> bool {
        let Some((stem, extension)) = name.rsplit_once('.') else {
            return false;
        };
        stem.len() == 12
            && stem
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            && matches!(extension, "png" | "mp4")
    }

    fn cc7_leaked_needle(haystack: &str, run_id: &str, benchmark_id: &str) -> Option<String> {
        let lowered = haystack.to_ascii_lowercase();
        cc7_leak_needles(run_id, benchmark_id)
            .into_iter()
            .chain(cc7_leak_value_needles())
            .find(|needle| lowered.contains(needle.as_str()))
    }

    /// The reviewer sees the artefact and the question, and nothing that says
    /// how the machine judged it or which run, sample, harness or model
    /// produced it. Both surfaces the reviewer touches are scanned — the
    /// `blind/` listing and the form's serialised bytes — and both are proved
    /// able to fail.
    // Two surfaces scanned, two leak directions, and a per-needle
    // non-vacuity sweep: the body is long because the check has to be shown
    // able to fail on each of them, not because it does several things.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc7_the_blind_package_discloses_no_machine_provenance() {
        let temporary = TempDirectory::new("cc7-blind-package");
        let run_directory = temporary.root();
        let run_id = "kinewright-eval-20260101T000000Z-claude-code";
        let results = vec![
            cc7_packaged_result(
                "c1 Mixed-camera interview match",
                run_directory,
                &cc7_blind_hash("0f3a1d2e4b5a"),
            ),
            cc7_packaged_result(
                "c5 Creative look with a gamut exception",
                run_directory,
                &cc7_blind_hash("9e8d7b6a5f40"),
            ),
        ];
        let benchmark_id = "kinewright-color-workflow-v6";
        assert_eq!(benchmark_id, COLOR_WORKFLOW_BENCHMARK_ID);
        let environment = EnvironmentStamp::capture(None, "claude-code", None);
        write_review_package(benchmark_id, run_id, run_directory, &environment, &results)
            .expect("the review package is written");

        let blind_directory = run_directory.join(BLIND_DIRECTORY_NAME);
        let listing = cc7_blind_listing(&blind_directory);
        // Two artefacts per task, plus the one form the reviewer opens.
        assert_eq!(listing.len(), 5, "{listing:?}");
        assert!(listing.contains(&BLIND_FORM_FILE_NAME.to_owned()));
        for name in &listing {
            if name == BLIND_FORM_FILE_NAME {
                continue;
            }
            assert!(
                cc7_is_blind_media_name(name),
                "{name} is not a blind media name"
            );
        }
        // The key is in the run root, deliberately outside `blind/`.
        assert!(run_directory.join(BLIND_KEY_FILE_NAME).exists());
        assert!(!blind_directory.join(BLIND_KEY_FILE_NAME).exists());

        for name in &listing {
            assert_eq!(
                cc7_leaked_needle(name, run_id, benchmark_id),
                None,
                "the blind listing discloses provenance in {name}"
            );
        }
        let form_bytes = fs::read_to_string(blind_directory.join(BLIND_FORM_FILE_NAME))
            .expect("the blind form is readable");
        assert_eq!(
            cc7_leaked_needle(&form_bytes, run_id, benchmark_id),
            None,
            "the blind form discloses provenance: {form_bytes}"
        );

        // Failing direction one: a deliberately leaked artefact name.
        fs::write(blind_directory.join("c1-sample-1.mp4"), b"leak").expect("leaked artefact");
        let leaked_listing = cc7_blind_listing(&blind_directory);
        assert_eq!(leaked_listing.len(), 6);
        assert!(
            leaked_listing
                .iter()
                .any(|name| cc7_leaked_needle(name, run_id, benchmark_id).is_some()),
            "the listing scan cannot see a leaked task id"
        );
        assert!(
            leaked_listing
                .iter()
                .any(|name| name != BLIND_FORM_FILE_NAME && !cc7_is_blind_media_name(name)),
            "the name-shape check cannot see a leaked task id"
        );

        // Failing direction two: a form entry that carries its task id.
        let leaked_form = serde_json::json!({
            "schema_version": 1,
            "entries": [{
                "blind_id": "0f3a1d2e4b5a",
                "task_id": "c1",
                "ratings": {},
                "not_applicable": [],
                "accepted": null,
                "notes": null
            }]
        });
        assert!(
            cc7_leaked_needle(&leaked_form.to_string(), run_id, benchmark_id).is_some(),
            "the byte scan cannot see a leaked task id"
        );

        // Failing direction three: a form entry that carries the run id and
        // the benchmark id — the two identifier classes §8.5 names beside the
        // task id. `BlindReviewForm` structurally cannot carry either today,
        // so this is the direction that would catch a future breadcrumb
        // field before a reviewer ever saw it.
        for identifier in [run_id, benchmark_id] {
            let leaked_form = serde_json::json!({
                "schema_version": 1,
                "entries": [{
                    "blind_id": "0f3a1d2e4b5a",
                    "run_id": identifier,
                    "ratings": {},
                    "not_applicable": [],
                    "accepted": null,
                    "notes": null
                }]
            });
            assert_eq!(
                cc7_leaked_needle(&leaked_form.to_string(), run_id, benchmark_id).as_deref(),
                Some(identifier.to_ascii_lowercase().as_str()),
                "the byte scan cannot see a leaked {identifier}"
            );
        }

        // Both directions, per needle: every needle the scan carries is one
        // the scan can actually see, and every needle is long enough that
        // seeing it means something. A needle nothing can trip is a check
        // that cannot fail (11.0.5).
        let needles = cc7_leak_needles(run_id, benchmark_id);
        assert!(needles.len() > CC7_MACHINE_PROVENANCE_NEEDLES.len());
        assert!(needles.contains(&run_id.to_ascii_lowercase()));
        assert!(needles.contains(&benchmark_id.to_ascii_lowercase()));
        for needle in needles.iter().chain(cc7_leak_value_needles().iter()) {
            assert!(needle.len() >= 2, "{needle} is too short to mean anything");
            assert_eq!(
                cc7_leaked_needle(&format!("blind-{needle}-payload"), run_id, benchmark_id)
                    .as_deref(),
                Some(needle.as_str()),
                "the scan cannot see its own needle {needle}"
            );
        }
        for value in cc7_leak_value_needles() {
            assert!(
                value.len() >= 3,
                "a value needle shorter than three digits matches any digit pair: {value}"
            );
        }
        // The needle list is the canonical documents', not a hand-kept copy.
        for parameter in cc7_canonical_parameter_names() {
            assert!(
                needles.contains(&parameter),
                "{parameter} is a canonical parameter the leak scan does not carry"
            );
        }
    }

    /// An accepted colour entry must answer every question the form poses;
    /// the reviewer's answer is what `--score-review` carries back.
    fn cc7_answer_every_question(form: &mut BlindReviewForm) {
        for entry in &mut form.entries {
            for question in &mut entry.questions {
                question.answer = Some(true);
            }
        }
    }

    fn cc7_blind_run(directory: &Path) -> (String, Vec<EvalResult>) {
        let run_id = "kinewright-eval-20260101T000000Z-claude-code".to_owned();
        let results = vec![cc7_packaged_result(
            "c1 Mixed-camera interview match",
            directory,
            &cc7_blind_hash("0f3a1d2e4b5a"),
        )];
        let environment = EnvironmentStamp::capture(None, "claude-code", None);
        write_review_package(
            "kinewright-color-workflow-v6",
            &run_id,
            directory,
            &environment,
            &results,
        )
        .expect("the review package is written");
        (run_id, results)
    }

    /// `--score-review` accepts the blind form and resolves it through the
    /// key **before** the artefact bindings are checked, which is the only
    /// order in which a form keyed on `blind_id` can bind at all.
    #[test]
    fn cc7_score_review_resolves_a_blind_form_before_binding() {
        let temporary = TempDirectory::new("cc7-score-blind-form");
        let run_directory = temporary.root();
        let (_run_id, _results) = cc7_blind_run(run_directory);
        let form_path = run_directory
            .join(BLIND_DIRECTORY_NAME)
            .join(BLIND_FORM_FILE_NAME);

        // The reviewer fills in the form they were handed, and nothing else.
        let mut form: BlindReviewForm =
            serde_json::from_slice(&fs::read(&form_path).expect("blind form"))
                .expect("form parses");
        assert_eq!(form.entries.len(), 1);
        assert_eq!(form.entries[0].blind_id, "0f3a1d2e4b5a");
        // The colour form pre-marks the editorial dimensions not applicable,
        // so the reviewer rates only the two a chart raster can be rated on.
        assert_eq!(form.entries[0].not_applicable.len(), 4);
        // The colour form also carries the scenario's matrix question, and an
        // accepted entry must answer every one of them.
        assert_eq!(form.entries[0].questions.len(), 1);
        form.entries[0].accepted = Some(true);
        form.entries[0].ratings = HumanRatings {
            visual_finish: Some(4.5),
            delivery_readiness: Some(4.5),
            ..HumanRatings::default()
        };
        cc7_answer_every_question(&mut form);
        fs::write(
            &form_path,
            serde_json::to_vec_pretty(&form).expect("form serialises"),
        )
        .expect("form is written back");

        score_review_file(&form_path).expect("a blind form scores through the key");

        // The unblinded review lands in the run root, carrying the task id
        // the key resolved and the artefact digest the machine reported.
        let unblinded: HumanReviewFile = serde_json::from_slice(
            &fs::read(run_directory.join("human-review.json")).expect("unblinded review"),
        )
        .expect("unblinded review parses");
        assert_eq!(unblinded.schema_version, 2);
        assert_eq!(unblinded.tasks[0].task_id, "c1");
        assert_eq!(unblinded.tasks[0].blind_id.as_deref(), Some("0f3a1d2e4b5a"));
        assert_eq!(
            unblinded.tasks[0].artifact_sha256.as_deref(),
            Some(cc7_blind_hash("0f3a1d2e4b5a").as_str())
        );
        assert_eq!(unblinded.tasks[0].accepted, Some(true));
        assert!(run_directory.join("human-score.json").exists());

        // A blind id the key does not carry is a typed error naming the id.
        form.entries[0].blind_id = "ffffffffffff".to_owned();
        fs::write(
            &form_path,
            serde_json::to_vec_pretty(&form).expect("form serialises"),
        )
        .expect("form is written back");
        let error = score_review_file(&form_path)
            .expect_err("an unknown blind id is refused")
            .to_string();
        assert!(error.contains("ffffffffffff"), "{error}");
        assert!(error.contains(BLIND_KEY_FILE_NAME), "{error}");
    }

    /// A blind identifier is a digest prefix, so two byte-identical
    /// artefacts share one — and one viewing therefore scores both rows.
    /// That is the existing convention made mechanical rather than manual.
    #[test]
    fn cc7_one_viewing_scores_every_row_with_the_same_artifact() {
        let temporary = TempDirectory::new("cc7-shared-blind-id");
        let run_directory = temporary.root();
        let hash = cc7_blind_hash("0f3a1d2e4b5a");
        let results = vec![
            cc7_packaged_result("c1 Mixed-camera interview match", run_directory, &hash),
            cc7_packaged_result("c2 Wrong white balance", run_directory, &hash),
        ];
        let environment = EnvironmentStamp::capture(None, "claude-code", None);
        write_review_package(
            "kinewright-color-workflow-v6",
            "kinewright-eval-20260101T000000Z-claude-code",
            run_directory,
            &environment,
            &results,
        )
        .expect("the review package is written");

        // One blind identifier, one form entry, two artefacts, two key rows.
        let listing = cc7_blind_listing(&run_directory.join(BLIND_DIRECTORY_NAME));
        assert_eq!(listing.len(), 3, "{listing:?}");
        let key: BlindKeyFile = serde_json::from_slice(
            &fs::read(run_directory.join(BLIND_KEY_FILE_NAME)).expect("key"),
        )
        .expect("key parses");
        assert_eq!(key.entries.len(), 2);
        assert_eq!(key.entries[0].task_id, "c1");
        assert_eq!(key.entries[1].task_id, "c2");

        let form_path = run_directory
            .join(BLIND_DIRECTORY_NAME)
            .join(BLIND_FORM_FILE_NAME);
        let mut form: BlindReviewForm =
            serde_json::from_slice(&fs::read(&form_path).expect("form")).expect("form parses");
        assert_eq!(form.entries.len(), 1);
        form.entries[0].accepted = Some(true);
        form.entries[0].ratings = HumanRatings {
            visual_finish: Some(4.5),
            delivery_readiness: Some(4.0),
            ..HumanRatings::default()
        };
        cc7_answer_every_question(&mut form);
        fs::write(
            &form_path,
            serde_json::to_vec_pretty(&form).expect("form serialises"),
        )
        .expect("form is written back");

        score_review_file(&form_path).expect("one viewing scores both rows");
        let unblinded: HumanReviewFile = serde_json::from_slice(
            &fs::read(run_directory.join("human-review.json")).expect("unblinded review"),
        )
        .expect("unblinded review parses");
        assert_eq!(unblinded.tasks.len(), 2);
        assert_eq!(unblinded.tasks[0].task_id, "c1");
        assert_eq!(unblinded.tasks[1].task_id, "c2");
        assert!(
            unblinded
                .tasks
                .iter()
                .all(|task| task.accepted == Some(true))
        );
    }

    /// A blind form without its key is a typed error naming the missing
    /// path, never a silent fallback.
    #[test]
    fn cc7_a_blind_form_without_its_key_is_refused_by_name() {
        let temporary = TempDirectory::new("cc7-blind-form-no-key");
        let run_directory = temporary.root();
        cc7_blind_run(run_directory);
        fs::remove_file(run_directory.join(BLIND_KEY_FILE_NAME)).expect("key is removed");
        let form_path = run_directory
            .join(BLIND_DIRECTORY_NAME)
            .join(BLIND_FORM_FILE_NAME);
        let error = score_review_file(&form_path)
            .expect_err("a form without a key is refused")
            .to_string();
        assert!(error.contains(BLIND_KEY_FILE_NAME), "{error}");
    }

    /// A pre-CC7 run has no key and no form; `--score-review` on its
    /// `human-review.json` behaves exactly as it always did.
    #[test]
    fn cc7_score_review_still_takes_the_plain_human_review_path() {
        let temporary = TempDirectory::new("cc7-score-plain-review");
        let run_directory = temporary.root();
        cc7_blind_run(run_directory);
        fs::remove_dir_all(run_directory.join(BLIND_DIRECTORY_NAME)).expect("blind is removed");
        fs::remove_file(run_directory.join(BLIND_KEY_FILE_NAME)).expect("key is removed");

        let review_path = run_directory.join("human-review.json");
        let mut review: HumanReviewFile =
            serde_json::from_slice(&fs::read(&review_path).expect("review"))
                .expect("review parses");
        review.tasks[0].accepted = Some(true);
        review.tasks[0].not_applicable.clear();
        review.tasks[0].ratings = HumanRatings {
            story: Some(4.0),
            pacing: Some(4.0),
            visual_finish: Some(4.5),
            audio_finish: Some(4.0),
            captions: Some(4.0),
            delivery_readiness: Some(4.5),
        };
        for question in &mut review.tasks[0].questions {
            question.answer = Some(true);
        }
        fs::write(
            &review_path,
            serde_json::to_vec_pretty(&review).expect("review serialises"),
        )
        .expect("review is written back");
        score_review_file(&review_path).expect("a plain human review still scores");
        assert!(run_directory.join("human-score.json").exists());
    }

    /// Every suite constructor still builds, and every deliverable spec now
    /// declares its lane. This is the compile coverage `delivery_bit_depth`
    /// buys: the field has no `Default`, so a forgotten literal is a build
    /// failure rather than a silent eight-bit encode.
    #[test]
    fn cc7_every_suite_constructor_declares_a_delivery_bit_depth() {
        let suites = [
            ("auto-edit-v1", seed_suite()),
            ("finished-cut-v2", finished_cut_suite()),
            ("editorial-cut-v3", editorial_cut_suite()),
            ("dialogue-pacing-v4", dialogue_pacing_suite()),
            ("generalization-v5", generalization_suite()),
        ];
        let mut deliverables = 0_usize;
        for (name, definitions) in suites {
            assert!(!definitions.is_empty(), "{name} builds no definition");
            for definition in &definitions {
                // No colour suite has landed yet, so every definition's
                // colour request is absent and `EvalOutcome::color` stays
                // `None` for every existing suite.
                assert!(
                    definition.color.is_none(),
                    "{} carries a colour request",
                    definition.name
                );
                if let Some(spec) = definition.deliverable {
                    assert_eq!(spec.delivery_bit_depth, DeliveryEncodeDepth::Eight);
                    deliverables += 1;
                }
            }
        }
        assert!(deliverables >= 4, "expected packaged deliverables");
    }

    // -----------------------------------------------------------------------
    // CC7 §7.7 / §11.2.32 — the published `color-workflow-v6` benchmark.
    // -----------------------------------------------------------------------

    /// The initial document a scenario's canonical batch is applied to.
    ///
    /// The layout is `cc7_timeline_document`'s — one clip per generated
    /// source, in timeline order, at CC7's own 25 fps and 320x180 — built from
    /// synthetic asset records rather than probed ones, so the published
    /// ground truth is a property of `cc7_scenarios` and not of any file on
    /// the machine that generated it.
    fn cc7_ground_truth_initial_document(scenario: Cc7Scenario) -> Document {
        let spec = cc7_spec(scenario);
        let clips = i64::try_from(spec.clips.len()).expect("a scenario has clips");
        let per_clip = i64::from(spec.frames) / clips;
        let assets = spec
            .clips
            .iter()
            .enumerate()
            .map(|(index, clip)| MediaAsset {
                id: AssetId(u64::try_from(index + 1).unwrap_or(u64::MAX)),
                path: PathBuf::from(format!("{}.mkv", cc7_source_token(clip.source))),
                name: cc7_source_token(clip.source).to_owned(),
                duration: TimeCode(per_clip),
                fps: Rational::new(CC7_SOURCE_FPS, 1).expect("25 fps"),
                kind: kinewright_core::MediaKind::Video,
                resolution: Some((CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)),
                source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
                color_description: kinewright_core::ColorContext::sdr_rec709().delivery,
            })
            .collect::<Vec<_>>();
        cc7_timeline_document(assets).expect("the CC7 clip layout builds")
    }

    /// §7.7's canonical **document**: `cc7_canonical_operations` applied to the
    /// scenario's initial document, recorded as the effects, parameters and
    /// keyframes it leaves on each clip.
    ///
    /// Recording the applied document rather than only the batch is what makes
    /// the checked-in bytes carry the document claim: a batch that core would
    /// no longer accept, or that landed a node on a different clip, cannot be
    /// published.
    fn cc7_canonical_document(scenario: Cc7Scenario) -> serde_json::Value {
        let mut document = cc7_ground_truth_initial_document(scenario);
        // (c) and (e) commit a LUT node, and core refuses an `InsertEffect`
        // whose `lut_asset_id` no `AddLutAsset` has registered
        // (`operation.rs:2592-2619`), so the published batch is applied behind
        // the same `AddLutAsset` prefix `cc7_lut_backed_canonical_operations`
        // puts in front of it. The prefix registers an asset record and adds no
        // effect, so the projection below is the same either way; the record is
        // a placeholder because `sha256` and `byte_len` are properties of a
        // generated file this module never reads.
        let operations = match scenario {
            Cc7Scenario::LogLike | Cc7Scenario::CreativeLook => {
                cc7_lut_backed_canonical_operations(
                    scenario,
                    cc7_log_lut_asset(
                        &"0".repeat(64),
                        u64::try_from(CC7_LOG_CUBE_BYTES_REPORTED).unwrap_or_default(),
                        CC7_LOG_CUBE_FILE_NAME,
                    ),
                )
            }
            _ => cc7_canonical_operations(scenario),
        };
        apply_batch(&mut document, &operations)
            .unwrap_or_else(|error| panic!("{scenario:?}: core must accept the batch: {error}"));
        serde_json::json!({
            "clips": document
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .map(|clip| serde_json::json!({
                    "clip_id": clip.id.0,
                    "effects": clip.effects,
                }))
                .collect::<Vec<_>>(),
        })
    }

    /// The checked-in ground truth, and the only writer of it.
    ///
    /// Regeneration is deliberately **not** an environment knob (Q4): §10
    /// forbids env-conditional behaviour in a gate, so when the generated and
    /// the checked-in bytes diverge the test prints the generated bytes and
    /// the implementer pastes them, which makes a ground-truth change always
    /// a reviewed edit.
    fn cc7_ground_truth_json() -> String {
        let mut scenarios = Vec::new();
        for (index, scenario) in CC7_SCENARIOS.into_iter().enumerate() {
            let spec = cc7_spec(scenario);
            // `cc7_canonical_operations` already carries (f)'s two
            // `SetEffectKeyframes`, so the batch is written once; what is
            // recorded beside it is only the parameter names those curves are
            // on, which the (f) assertions read.
            let keyframed_parameters = if scenario == Cc7Scenario::TrackedSecondary {
                CC7_F_KEYFRAMED_PARAMETERS.to_vec()
            } else {
                Vec::new()
            };
            scenarios.push(serde_json::json!({
                "task_id": format!("c{}", index + 1),
                "scenario_id": spec.id,
                "title": spec.title,
                "fps": spec.fps,
                "resolution": [spec.width, spec.height],
                "frames": spec.frames,
                "clips": spec
                    .clips
                    .iter()
                    .map(|clip| serde_json::json!({
                        "clip_id": clip.clip_id,
                        "camera": cc7_camera_token(clip.camera),
                        "source": cc7_source_token(clip.source),
                    }))
                    .collect::<Vec<_>>(),
                "target_clip_id": cc7_target_clip(scenario).0,
                "canonical_document": cc7_canonical_document(scenario),
                "person_path": cc7_person_path_token(spec.person_path),
                "human_question": spec.human_question,
                "canonical_operations": cc7_canonical_operations(scenario),
                "keyframed_parameters": keyframed_parameters,
                "expected_measurements": cc7_expected_measurements(scenario),
            }));
        }
        let document = serde_json::json!({
            "schema_version": 1,
            "benchmark_id": COLOR_WORKFLOW_BENCHMARK_ID,
            "generator": "cc7_ground_truth_json (crates/kinewright-agent/src/bin/kinewright-eval.rs)",
            "authority": "kinewright_core::cc7_scenarios",
            "note": "Generated, never hand-edited. `published_v6_manifest_tracks_the_color_workflow_suite` asserts the checked-in bytes equal the generated bytes and prints the generated bytes on a mismatch.",
            "scenarios": scenarios,
        });
        let mut text = serde_json::to_string_pretty(&document).expect("ground truth serialises");
        text.push('\n');
        text
    }

    fn cc7_camera_token(camera: Cc7Camera) -> &'static str {
        match camera {
            Cc7Camera::A => "a",
            Cc7Camera::B => "b",
            Cc7Camera::C1 => "c1_recoverable",
            Cc7Camera::C2 => "c2_unrecoverable",
            Cc7Camera::LogLike => "log_like",
        }
    }

    fn cc7_source_token(source: Cc7Source) -> &'static str {
        match source {
            Cc7Source::BaseScene => "base_scene",
            Cc7Source::TrackedSquare => "tracked_square",
        }
    }

    fn cc7_person_path_token(person_path: Cc7PersonPath) -> serde_json::Value {
        match person_path {
            Cc7PersonPath::Expressible => serde_json::json!({ "kind": "expressible" }),
            Cc7PersonPath::NotApplicable { reason } => serde_json::json!({
                "kind": "not_applicable",
                "reason": reason,
            }),
        }
    }

    /// What each scenario expects, in `cc7_scenarios`' own constants. Every
    /// number here is a named constant, never a literal (R-M18).
    fn cc7_expected_measurements(scenario: Cc7Scenario) -> serde_json::Value {
        let delivery = serde_json::json!(DeliveryEncodeDepth::Eight.as_str());
        match scenario {
            Cc7Scenario::MixedCamera => serde_json::json!({
                "neutral_patch_spread_max_code": CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
                "chart_luma_mean_max_code_millionths": CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
                "skin_in_band_minimum_basis_points": CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS,
                "delivery_bit_depth": delivery,
            }),
            Cc7Scenario::WhiteBalance => serde_json::json!({
                "skin_in_band_minimum_basis_points": CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS,
                "residual_spread_reported_code": CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE,
                "over_range_reported_basis_points": CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED,
                "delivery_bit_depth": delivery,
            }),
            Cc7Scenario::LogLike => serde_json::json!({
                "log_first_percentile_min_code16": CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
                "log_p99_max_code16": CC7_LOG_P99_MAX_CODE16,
                "log_inverse_max_code": CC7_LOG_INVERSE_MAX_CODE,
                "log_cube_size": CC7_LOG_CUBE_SIZE,
                "delivery_bit_depth": delivery,
            }),
            Cc7Scenario::ProductAndSkin => serde_json::json!({
                "matte_covered_pixel_count": CC7_PRODUCT_PATCH_PIXEL_COUNT,
                "matte_full_pixel_count": CC7_PRODUCT_PATCH_PIXEL_COUNT,
                "matte_partial_pixel_count": CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX,
                "skin_in_band_minimum_basis_points": CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS,
                "delivery_bit_depth": delivery,
            }),
            Cc7Scenario::CreativeLook => serde_json::json!({
                "deep_shadow_out_of_gamut_pixels": CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
                "look_mix_basis_points": CC7_LOOK_MIX_BASIS_POINTS,
                "delivery_bit_depth": delivery,
            }),
            Cc7Scenario::TrackedSecondary => serde_json::json!({
                "track_sample_frames": cc7_tracking_sample_frames(),
                "surviving_sample_frames": CC7_TRACK_SURVIVING_SAMPLE_FRAMES,
                "expected_low_confidence_frames": CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES,
                "minimum_confidence_basis_points": CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
                "observation_tolerance_basis_points": CC7_TRACK_TOLERANCE_BASIS_POINTS,
                "delivery_bit_depth": delivery,
            }),
        }
    }

    /// The manifest is the executable suite, written down.
    ///
    /// Asserted in both directions: every task id, prompt, budget, delivery
    /// lane and assertion count on the page comes from the definition the
    /// runner would execute, and the checked-in `ground-truth.json` is byte
    /// for byte what `cc7_ground_truth_json` generates.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn published_v6_manifest_tracks_the_color_workflow_suite() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v6/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 6);
        assert_eq!(manifest["benchmark_id"], COLOR_WORKFLOW_BENCHMARK_ID);
        assert_eq!(
            manifest["runner"],
            "KINEWRIGHT_EVAL=1 cargo run -p kinewright-agent --bin kinewright-eval -- --suite color-workflow-v6"
        );
        assert_eq!(
            manifest["implementation"],
            "crates/kinewright-agent/src/bin/kinewright-eval.rs"
        );
        assert_eq!(
            manifest["score_layers"]["blind_human_review"]["not_applicable_dimensions"]
                .as_array()
                .unwrap()
                .len(),
            COLOR_WORKFLOW_NOT_APPLICABLE.len()
        );
        assert_eq!(manifest["acceptance_target"]["samples_per_scenario"], 3);
        assert_eq!(
            manifest["acceptance_target"]["minimum_machine_passes_per_scenario"],
            3
        );
        assert_eq!(
            manifest["acceptance_target"]["minimum_human_accepts_per_scenario"],
            2
        );
        assert_eq!(
            manifest["acceptance_target"]["minimum_mean_human_rating"],
            4.0
        );
        assert_eq!(
            manifest["acceptance_target"]["every_question_answered"],
            true
        );

        let definitions = color_workflow_suite();
        let tasks = manifest["tasks"].as_array().unwrap();
        assert_eq!(definitions.len(), CC7_SCENARIOS.len());
        assert_eq!(tasks.len(), definitions.len());
        for ((task, definition), scenario) in tasks.iter().zip(&definitions).zip(CC7_SCENARIOS) {
            let mut name = definition.name.splitn(2, ' ');
            let id = name.next().expect("a task name carries an id");
            let title = name.next().expect("a task name carries a title");
            assert_eq!(task["id"].as_str(), Some(id));
            assert_eq!(task["name"].as_str(), Some(title));
            assert_eq!(task["scenario"].as_str(), Some(cc7_spec(scenario).id));
            assert_eq!(task["prompt"], definition.prompts[0]);
            assert_eq!(
                task["ground_truth"],
                "benchmarks/auto-edit/v6/ground-truth.json"
            );
            assert!(
                task["fixture"]
                    .as_str()
                    .is_some_and(|fixture| fixture.starts_with("fixture_cc7_")),
                "{task:?}"
            );
            assert_eq!(task["budget"]["turns"], definition.budgets.max_turns);
            assert_eq!(
                task["budget"]["tool_calls"],
                definition.budgets.max_tool_calls
            );
            assert_eq!(
                task["budget"]["operations"],
                definition.budgets.max_operations
            );
            assert_eq!(task["budget"]["tokens"], definition.budgets.max_tokens);
            assert_eq!(task["budget"]["undos"], definition.budgets.max_undos);
            assert_eq!(
                task["budget"]["cost_usd"],
                serde_json::json!(definition.budgets.max_cost_usd)
            );
            assert_eq!(
                task["budget"]["wall_time_ms"],
                serde_json::json!(
                    u64::try_from(definition.budgets.max_wall_time.as_millis()).unwrap()
                )
            );
            let deliverable = definition.deliverable.expect("every c-task delivers");
            assert_eq!(task["delivery"]["profile"], deliverable.profile.as_str());
            assert_eq!(
                task["delivery"]["delivery_bit_depth"],
                deliverable.delivery_bit_depth.as_str()
            );
            assert_eq!(task["delivery"]["proof_frames"], deliverable.proof_frames);
            assert_eq!(
                task["delivery"]["proof_cell_width"],
                deliverable.proof_cell_width
            );
            assert_eq!(
                task["machine_assertions"].as_array().unwrap().len(),
                definition.assertions.len(),
                "{id} declares a different number of machine assertions than it executes"
            );
            assert_eq!(
                task["human_question"],
                serde_json::json!(cc7_spec(scenario).human_question)
            );
        }

        let generated = cc7_ground_truth_json();
        let checked_in = include_str!("../../../../benchmarks/auto-edit/v6/ground-truth.json");
        assert_eq!(
            checked_in, generated,
            "benchmarks/auto-edit/v6/ground-truth.json is stale. Paste the generated bytes below into that file:\n{generated}"
        );
    }

    /// Without this the run produces no artifacts, no review package, and
    /// **overwrites `docs/EVALS.md`**. It is the one registration that fails
    /// silently, so it gets its own test.
    #[test]
    fn cc7_color_workflow_suite_is_a_packaged_benchmark() {
        assert!(is_packaged_benchmark(COLOR_WORKFLOW_BENCHMARK_ID));
        let (benchmark_id, definitions) = eval_suite("color-workflow-v6").unwrap();
        assert_eq!(benchmark_id, COLOR_WORKFLOW_BENCHMARK_ID);
        assert_eq!(definitions.len(), CC7_SCENARIOS.len());
        let (alias_id, alias) = eval_suite("v6").unwrap();
        assert_eq!(alias_id, COLOR_WORKFLOW_BENCHMARK_ID);
        assert_eq!(alias.len(), definitions.len());
        // The failing direction: an unknown suite still names every known one.
        let Err(error) = eval_suite("color-workflow-v7") else {
            panic!("an unknown suite must be refused");
        };
        let error = error.to_string();
        assert!(error.contains("color-workflow-v6"), "{error}");
    }

    /// The usage banner enumerates the suites literally, so a suite that is
    /// registered but undiscoverable fails here.
    #[test]
    fn cc7_the_usage_banner_lists_every_registered_suite() {
        let usage = usage_text();
        for suite in [
            "auto-edit-v1",
            "finished-cut-v2",
            "editorial-cut-v3",
            "dialogue-pacing-v4",
            "generalization-v5",
            "color-workflow-v6",
        ] {
            assert!(usage.contains(suite), "the usage banner omits {suite}");
        }
    }

    /// Every colour task carries a request, so `EvalOutcome::color` is `Some`
    /// for the colour suite; every other suite leaves it `None`.
    #[test]
    fn cc7_every_color_task_carries_a_color_eval_request() {
        for definition in color_workflow_suite() {
            let request = definition
                .color
                .as_ref()
                .unwrap_or_else(|| panic!("{} carries no colour request", definition.name));
            assert!(
                !definition.assertions.is_empty(),
                "{} asserts nothing",
                definition.name
            );
            // The named evidence extras, asserted **positively** per task
            // before they are copied across: without this the comparison
            // below only proves "nothing outside those six fields differs",
            // and c1 could carry a gamut region or c5 a chart band unnoticed.
            let task = definition
                .name
                .split_whitespace()
                .next()
                .expect("a task name has an id");
            let (chart_luma, gamut) = match task {
                "c1" | "c2" => (Some(CC7_CHART_BAND_ROI), None),
                "c5" => (None, Some(CC7_DEEP_SHADOW_ROI)),
                _ => (None, None),
            };
            assert_eq!(request.chart_luma_roi, chart_luma, "{task}'s chart band");
            assert_eq!(request.gamut_roi, gamut, "{task}'s gamut region");
            assert_eq!(
                (
                    request.chart_luma_reference_frame,
                    request.chart_luma_candidate_frame
                ),
                if chart_luma.is_some() {
                    (0, i64::from(CC7_SOURCE_FRAMES))
                } else {
                    (0, 0)
                },
                "{task}'s chart band frames"
            );
            // Neither extra is ever pinned: the QC region is the whole frame
            // and the matte node is the runner's resolver's (F-E4).
            assert_eq!(request.qc_roi, None, "{task} pins a qc region");
            assert_eq!(request.matte_node, None, "{task} pins a matte node");
            // Derived from the assertions, so a region is written once.
            assert_eq!(
                request,
                &{
                    let mut derived = ColorEvalRequest::from_assertions(&definition.assertions)
                        .expect("a colour task gates colour assertions");
                    derived.chart_luma_roi = request.chart_luma_roi;
                    derived.chart_luma_reference_frame = request.chart_luma_reference_frame;
                    derived.chart_luma_candidate_frame = request.chart_luma_candidate_frame;
                    derived.gamut_roi = request.gamut_roi;
                    derived.qc_roi = request.qc_roi;
                    derived.matte_node = request.matte_node;
                    derived
                },
                "{}'s request is not its assertions plus the named evidence extras",
                definition.name
            );
        }
        for (_, definitions) in [
            ("generalization-v5", generalization_suite()),
            ("dialogue-pacing-v4", dialogue_pacing_suite()),
        ] {
            for definition in definitions {
                assert!(
                    definition.color.is_none(),
                    "{} is not a colour task and must leave the block None",
                    definition.name
                );
            }
        }
    }

    /// R-M18: the suite call site passes a `cc7_scenarios` constant into every
    /// threshold field, never a literal, so a re-baseline moves one constant
    /// and not six suite lines.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cc7_every_color_assertion_threshold_is_a_cc7_scenarios_constant() {
        let mut spreads = 0_usize;
        let mut skins = 0_usize;
        let mut mattes = 0_usize;
        let mut tracks = 0_usize;
        let mut deliveries = 0_usize;
        let mut qcs = 0_usize;
        for definition in color_workflow_suite() {
            for assertion in &definition.assertions {
                match assertion {
                    EvalAssertion::ColorQcTechnicalPass { checks, .. } => {
                        qcs += 1;
                        // Every token round-trips: `ColorEvalRequest` derives
                        // its `qc_checks` through a `filter_map`, so a token
                        // core does not spell drops silently and the technical
                        // pass would run one check short.
                        let derived =
                            ColorEvalRequest::from_assertions(std::slice::from_ref(assertion))
                                .expect("a qc assertion derives a request");
                        assert_eq!(
                            derived.qc_checks.len(),
                            checks.len(),
                            "{}: a ColorQcTechnicalPass token does not round-trip: {checks:?}",
                            definition.name
                        );
                    }
                    EvalAssertion::NeutralPatchSpreadAtMost {
                        patch_rois,
                        maximum_code,
                    } => {
                        spreads += 1;
                        assert_eq!(*maximum_code, CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE);
                        assert_eq!(patch_rois.len(), CC7_CHART_PATCHES.len());
                        assert_eq!(patch_rois.as_slice(), cc7_chart_patch_rois().as_slice());
                    }
                    EvalAssertion::SkinHueWithinBand {
                        roi,
                        minimum_in_band_basis_points,
                    } => {
                        skins += 1;
                        assert_eq!(*roi, CC7_SKIN_BAND_ROI);
                        assert!(
                            i64::from(*minimum_in_band_basis_points)
                                == CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS
                                || i64::from(*minimum_in_band_basis_points)
                                    == CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS,
                            "{minimum_in_band_basis_points} is not a cc7_scenarios skin constant"
                        );
                    }
                    EvalAssertion::MatteContainmentExact {
                        roi,
                        expected_covered_pixel_count,
                        expected_full_pixel_count,
                        expected_partial_pixel_count,
                    } => {
                        mattes += 1;
                        assert_eq!(*roi, CC7_PRODUCT_RED_ROI);
                        assert_eq!(
                            *expected_covered_pixel_count,
                            u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT)
                        );
                        assert_eq!(
                            *expected_full_pixel_count,
                            u64::from(CC7_PRODUCT_PATCH_PIXEL_COUNT)
                        );
                        assert_eq!(
                            i64::try_from(*expected_partial_pixel_count).unwrap(),
                            CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX
                        );
                    }
                    EvalAssertion::TrackKeyframesMatchExpected {
                        parameter,
                        expected_local_frames,
                        absent_local_frames,
                    } => {
                        tracks += 1;
                        assert!(
                            CC7_F_KEYFRAMED_PARAMETERS.contains(&parameter.as_str()),
                            "{parameter} is not a CC7 keyframed parameter"
                        );
                        assert_eq!(
                            expected_local_frames.as_slice(),
                            CC7_TRACK_SURVIVING_SAMPLE_FRAMES.as_slice()
                        );
                        assert_eq!(
                            absent_local_frames.as_slice(),
                            CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES.as_slice()
                        );
                    }
                    EvalAssertion::DeliveryVerificationWithinBudgets { depth } => {
                        deliveries += 1;
                        assert_eq!(*depth, DeliveryEncodeDepth::Eight);
                        assert_eq!(
                            definition
                                .deliverable
                                .expect("a verified task delivers")
                                .delivery_bit_depth,
                            *depth,
                            "{} verifies a depth it does not encode",
                            definition.name
                        );
                    }
                    EvalAssertion::ReferenceClipUntouched { clip_id } => {
                        assert_eq!(*clip_id, CC7_REFERENCE_CLIP_ID.0);
                    }
                    EvalAssertion::LookBypassMatchesAbsent {
                        clip_id, effect_id, ..
                    } => {
                        assert_eq!(*clip_id, CC7_SINGLE_CLIP_ID.0);
                        assert_eq!(*effect_id, CC7_NODE_EFFECT_ID.0);
                    }
                    _ => {}
                }
            }
        }
        // Non-vacuity: every gated family is actually present in the suite.
        assert_eq!(spreads, 1);
        assert_eq!(skins, 3);
        assert_eq!(mattes, 1);
        assert_eq!(tracks, CC7_F_KEYFRAMED_PARAMETERS.len());
        assert_eq!(deliveries, CC7_SCENARIOS.len());
        assert!(qcs > 0, "no task gates the colour QC technical pass");
    }

    /// §8.3: the colour template pre-marks the four dimensions a chart raster
    /// cannot be rated on and attaches the scenario's matrix question. (c) has
    /// none — it is objective-only — and contributes no entry at all.
    #[test]
    fn cc7_the_color_review_template_asks_only_the_matrix_question() {
        let questions = review_questions(COLOR_WORKFLOW_BENCHMARK_ID);
        assert_eq!(questions.len(), CC7_SCENARIOS.len() - 1);
        assert!(!questions.contains_key("c3"));
        for (index, scenario) in CC7_SCENARIOS.into_iter().enumerate() {
            let key = format!("c{}", index + 1);
            match cc7_spec(scenario).human_question {
                Some(prompt) => {
                    let asked = questions.get(&key).expect("a scenario with a question");
                    assert_eq!(asked.len(), 1);
                    assert_eq!(asked[0].id, cc7_spec(scenario).id);
                    assert_eq!(asked[0].prompt, prompt);
                    assert_eq!(asked[0].answer, None);
                }
                None => assert!(!questions.contains_key(&key)),
            }
        }
        // Every other benchmark asks none, unchanged.
        assert!(review_questions("kinewright-generalization-v5").is_empty());

        let temporary = TempDirectory::new("cc7-review-template");
        let results = vec![
            cc7_packaged_result(
                "c1 Mixed-camera interview match",
                temporary.root(),
                &cc7_blind_hash("0f3a1d2e4b5a"),
            ),
            cc7_packaged_result(
                "c3 Log-like input normalisation",
                temporary.root(),
                &cc7_blind_hash("9e8d7b6a5f40"),
            ),
        ];
        let review = human_review_template_with_questions(
            COLOR_WORKFLOW_BENCHMARK_ID,
            "kinewright-eval-20260101T000000Z-claude-code",
            &results,
            &questions,
        );
        assert_eq!(review.tasks.len(), 2);
        for task in &review.tasks {
            assert_eq!(task.not_applicable, COLOR_WORKFLOW_NOT_APPLICABLE.to_vec());
        }
        assert_eq!(review.tasks[0].questions.len(), 1);
        assert!(
            review.tasks[1].questions.is_empty(),
            "scenario (c) is objective-only and contributes no question"
        );
    }

    /// Every fixture builder mixes its own sources and lands a valid document.
    ///
    /// This is the only check that the fixture half of the suite works without
    /// a model: `PreparedFixture::new` validates the document, so a builder
    /// that mis-times a clip or mis-maps a 25 fps source fails here rather
    /// than in a paid session. It shells out to the pinned `FFmpeg` once per
    /// generated raster (~11 s on the software lane).
    ///
    /// Ignored for the same reason every other fixture-build test in this file
    /// is: it constructs a real `FfmpegMediaEngine`, and this binary carries
    /// none otherwise. The engine's process-exit teardown raises the known
    /// SIGSEGV that `tests/mcp_server.rs` already lives with, *after* every
    /// test has reported ok, which would make the default lane red on a
    /// teardown that has nothing to do with the fixtures. Run it with
    /// `cargo test -p kinewright-agent --bin kinewright-eval -- --ignored`.
    #[test]
    #[ignore = "constructs a real media engine, whose process-exit teardown raises the known SIGSEGV"]
    fn cc7_every_color_fixture_builds_a_valid_document() {
        for (definition, scenario) in color_workflow_suite().iter().zip(CC7_SCENARIOS) {
            let fixture = (definition.fixture_builder)()
                .unwrap_or_else(|error| panic!("{} does not build: {error}", definition.name));
            let document = &fixture.original_document;
            let spec = cc7_spec(scenario);
            assert_eq!(document.resolution, (spec.width, spec.height));
            assert_eq!(document.fps.numerator(), spec.fps);
            assert_eq!(document.fps.denominator(), 1);
            assert_eq!(document.duration, TimeCode(i64::from(spec.frames)));
            assert_eq!(document.tracks.len(), 1);
            assert_eq!(
                document.tracks[0].clips.len(),
                spec.clips.len(),
                "{} builds a different clip count than its spec",
                definition.name
            );
            for (clip, expected) in document.tracks[0].clips.iter().zip(spec.clips) {
                assert_eq!(clip.id, ClipId(expected.clip_id));
                assert!(clip.effects.is_empty(), "a CC7 fixture starts ungraded");
            }
            // c3 is the one task that saves a project, and the LUT its prompt
            // names has to be on disk beside it or the task is unsatisfiable.
            if scenario == Cc7Scenario::LogLike {
                let project = fixture
                    .project_path
                    .as_ref()
                    .expect("c3 saves its project so import_lut_asset can resolve a store");
                assert!(project.is_file(), "{}", project.display());
                let cube = project
                    .parent()
                    .expect("a saved project has a directory")
                    .join(CC7_LOG_CUBE_FILE_NAME);
                assert!(cube.is_file(), "{}", cube.display());
            } else {
                assert!(
                    fixture.project_path.is_none(),
                    "{} saves a project it does not need",
                    definition.name
                );
            }
        }
    }
}
