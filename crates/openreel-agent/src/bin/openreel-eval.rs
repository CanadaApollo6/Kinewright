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

use openreel_agent::{
    ClaudeCodeDriver, CodexDriver, CursorAcpDriver,
    eval::{
        EnvironmentStamp, EvalAssertion, EvalBudgets, EvalDefinition, EvalDeliverableSpec,
        EvalError, EvalResult, ExpectedSourceClip, FixtureContext, HumanReviewFile,
        PreparedFixture, human_review_template, maximum_duration_after_expected_silence_cuts,
        render_jsonl, render_scoreboard, result_path, run_eval, run_eval_with_artifacts,
        summarize_human_review,
    },
    fixture_pack::{FixturePackManifest, fixture_cache_root},
};
use openreel_core::{
    AgentDriver, Analysis, AssetSceneChanges, AssetSilences, AssetTranscript, AuthenticationStatus,
    CaptionMotion, Clip, ClipId, DeliveryProfile, Document, MediaAsset, Rational, SceneStatus,
    SilenceStatus, TimeCode, Track, TrackId, TrackKind, TranscriptStatus,
    map_source_range_to_project,
};
use openreel_media::{
    FfmpegMediaEngine,
    test_support::{GeneratedMedia, SpeechClip, joined_words, normalized_words, test_engine},
};
use serde::Deserialize;

const FPS: u32 = 30;
const LONG_SILENCE_FRAMES: i64 = 20;
const SCENE_CONFIDENCE_BASIS_POINTS: u16 = 1_000;
const FILLER_WORDS: &[&str] = &["um", "uh", "erm", "er"];

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("openreel-eval: {error}");
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
    if let Some(review_path) = &options.score_review {
        return score_review_file(review_path).map(|()| true);
    }
    run_subscription_suite(&options)
}

fn run_subscription_suite(options: &Options) -> Result<bool, EvalError> {
    if env::var("OPENREEL_EVAL").as_deref() != Ok("1") {
        return Err(EvalError::Agent(
            "refusing to run a subscription eval; set OPENREEL_EVAL=1 explicitly".to_owned(),
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
        "auto-edit-v1" | "v1" => Ok(("openreel-auto-edit-v1", seed_suite())),
        "finished-cut-v2" | "v2" => Ok(("openreel-finished-cut-v2", finished_cut_suite())),
        "editorial-cut-v3" | "v3" => Ok(("openreel-editorial-cut-v3", editorial_cut_suite())),
        "dialogue-pacing-v4" | "v4" => Ok(("openreel-dialogue-pacing-v4", dialogue_pacing_suite())),
        "generalization-v5" | "v5" => Ok(("openreel-generalization-v5", generalization_suite())),
        other => Err(EvalError::Agent(format!(
            "unknown suite {other:?}; expected auto-edit-v1, finished-cut-v2, editorial-cut-v3, dialogue-pacing-v4, or generalization-v5"
        ))),
    }
}

fn is_packaged_benchmark(benchmark_id: &str) -> bool {
    matches!(
        benchmark_id,
        "openreel-finished-cut-v2"
            | "openreel-editorial-cut-v3"
            | "openreel-dialogue-pacing-v4"
            | "openreel-generalization-v5"
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
    let review = human_review_template(benchmark_id, run_id, results);
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
    println!("Review: {}", review_path.display());
    println!("Package: {}", run_directory.display());
    Ok(())
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
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, EvalError> {
        let mut harness = "claude-code".to_owned();
        let mut model = None;
        let mut only = None;
        let mut samples = 1_u32;
        let mut suite = "auto-edit-v1".to_owned();
        let mut score_review = None;
        let mut prepare_fixtures = None;
        let mut verify_fixtures = None;
        let mut arguments = arguments.peekable();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--harness" => {
                    harness = arguments
                        .next()
                        .ok_or_else(|| EvalError::Agent("--harness requires a value".to_owned()))?;
                }
                "--model" => {
                    model =
                        Some(arguments.next().ok_or_else(|| {
                            EvalError::Agent("--model requires a value".to_owned())
                        })?);
                }
                "--only" => {
                    only = Some(arguments.next().ok_or_else(|| {
                        EvalError::Agent("--only requires an eval name (e.g. e7)".to_owned())
                    })?);
                }
                "--samples" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| EvalError::Agent("--samples requires a count".to_owned()))?;
                    samples = value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=25).contains(n))
                        .ok_or_else(|| {
                            EvalError::Agent("--samples must be an integer in 1..=25".to_owned())
                        })?;
                }
                "--suite" => {
                    suite = arguments
                        .next()
                        .ok_or_else(|| EvalError::Agent("--suite requires a value".to_owned()))?;
                }
                "--score-review" => {
                    score_review = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        EvalError::Agent("--score-review requires a JSON path".to_owned())
                    })?));
                }
                "--prepare-fixtures" => {
                    prepare_fixtures = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        EvalError::Agent(
                            "--prepare-fixtures requires a fixture-pack manifest path".to_owned(),
                        )
                    })?));
                }
                "--verify-fixtures" => {
                    verify_fixtures = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        EvalError::Agent(
                            "--verify-fixtures requires a fixture-pack manifest path".to_owned(),
                        )
                    })?));
                }
                "-h" | "--help" => {
                    println!(
                        "Usage: OPENREEL_EVAL=1 cargo run -p openreel-agent --bin openreel-eval -- [--suite auto-edit-v1|finished-cut-v2|editorial-cut-v3|dialogue-pacing-v4|generalization-v5] [--harness claude-code|codex|cursor] [--model MODEL] [--only EVAL] [--samples N]\n       cargo run -p openreel-agent --bin openreel-eval -- --prepare-fixtures MANIFEST\n       cargo run -p openreel-agent --bin openreel-eval -- --verify-fixtures MANIFEST\n       cargo run -p openreel-agent --bin openreel-eval -- --score-review PATH"
                    );
                    return Err(EvalError::Agent("help requested".to_owned()));
                }
                other => {
                    return Err(EvalError::Agent(format!("unknown argument {other:?}")));
                }
            }
        }
        let exclusive_actions = [
            score_review.is_some(),
            prepare_fixtures.is_some(),
            verify_fixtures.is_some(),
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();
        if exclusive_actions > 1 {
            return Err(EvalError::Agent(
                "--score-review, --prepare-fixtures, and --verify-fixtures are mutually exclusive"
                    .to_owned(),
            ));
        }
        Ok(Self {
            harness,
            model,
            only,
            samples,
            suite,
            score_review,
            prepare_fixtures,
            verify_fixtures,
        })
    }
}

fn run_identifier(environment: &EnvironmentStamp) -> String {
    result_path(Path::new(""), environment)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("openreel-eval-run")
        .to_owned()
}

fn score_review_file(path: &Path) -> Result<(), EvalError> {
    let bytes = fs::read(path).map_err(|error| {
        EvalError::Output(format!("could not read review {}: {error}", path.display()))
    })?;
    let review: HumanReviewFile = serde_json::from_slice(&bytes).map_err(|error| {
        EvalError::Output(format!(
            "could not parse review {}: {error}",
            path.display()
        ))
    })?;
    let summary = summarize_human_review(&review)?;
    let output = path.with_file_name("human-score.json");
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
        }),
    }]
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
        }),
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
    vec![EvalDefinition {
        name: "g1 real interview recovery story",
        rationale: "Measures whether the agent can find, clean, caption, frame, and deliver one coherent story from pinned public-domain interview footage it has not seen in the synthetic benchmarks.",
        fixture_builder: fixture_real_interview_story,
        prompts: &[
            "Create a finished vertical social interview cut from interview-raw. Open exactly these four capability schemas in one get_capability call: get_transcript, plan_dialogue_assembly, add_styled_captions, and get_editorial_readiness. Inspect the full source transcript. Build only Helen Hill's coherent first-person story about recovering her films after Hurricane Katrina: begin with her thought starting 'recently I was living in New Orleans' and end after 'Hurricane Katrina films.' Exclude the interview questions, Columbia and South Carolina setup, symposium and festival explanation, and the later Dan Streible discussion. Use plan_dialogue_assembly with one source range around that answer, remove conservative fillers and raw dead air of at least 20 source frames, retain 8 source frames around ordinary silence cuts, inspect its prepared plan preview, and commit that exact plan. Keep the real A/V source dialogue audible without duplicating it onto an audio track. Add social preset captions with pop motion from the final timeline transcript; do not paraphrase or invent wording. Finish with one get_editorial_readiness call using vertical_short, centered 50/50 focus, nine storyboard frames, 240-pixel cells, and a 20-source-frame silence threshold. Do not queue export; the benchmark renders and independently transcribes the verified snapshot. Keep working until readiness is true.",
        ],
        assertions: vec![
            EvalAssertion::TimelineNonEmpty,
            EvalAssertion::AssetOrder {
                aliases: aliases(&["interview-raw"]),
                collapse_adjacent: true,
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
            maximum_word_error_rate_basis_points: 2_000,
        }),
    }]
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
    PreparedFixture::new(document, media, context, vec![Box::new(generated)])
}

fn fixture_e2() -> Result<PreparedFixture, EvalError> {
    const SPEECH: &str = "<speak version='1.0' xml:lang='en-US'>Alpha.<break time='1400ms'/>Bravo.<break time='1400ms'/>Charlie.</speak>";
    let media = eval_engine();
    let generated = SpeechClip::ssml("eval-e2-silence", SPEECH, "OPENREEL_EVAL_E2_AUDIO");
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
    PreparedFixture::new(document, media, context, vec![Box::new(generated)])
}

fn fixture_e3() -> Result<PreparedFixture, EvalError> {
    const SPEECH: &str = "Hello, um, this is an Open Reel filler word evaluation.";
    let media = eval_engine();
    let generated = SpeechClip::plain("eval-e3-filler", SPEECH, "OPENREEL_EVAL_E3_AUDIO");
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
    PreparedFixture::new(document, media, context, vec![Box::new(generated)])
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
    PreparedFixture::new(document, media, context, vec![Box::new(generated)])
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
        let environment_name = format!("OPENREEL_EVAL_TAKE_{}_AUDIO", index + 1);
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
    PreparedFixture::new(document, media, context, resources)
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
    expected_dialogue: String,
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
    let expected_normalized = normalized_words(&truth.expected_dialogue);
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
    PreparedFixture::new(document, media, context, Vec::new())
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
        let environment_name = format!("OPENREEL_EVAL_EDITORIAL_TAKE_{}_AUDIO", index + 1);
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
    PreparedFixture::new(document, media, context, resources)
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
    const FONT: &str = "crates/openreel-app/assets/fonts/Inter-SemiBold.ttf";
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
    PreparedFixture::new(document, media, context, resources)
}

fn eval_engine() -> Arc<FfmpegMediaEngine> {
    Arc::new(test_engine("OPENREEL_EVAL_DATA_DIR"))
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
        catalog: openreel_core::MediaCatalog::default(),
        audio_mix: openreel_core::AudioMix::default(),
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
            content: openreel_core::ClipContent::Media,
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
        catalog: openreel_core::MediaCatalog::default(),
        audio_mix: openreel_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: timeline_clips,
        }],
        media_pool: assets,
        markers: Vec::new(),
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
        "# Agent evals\n\nOpenReel's Arc 2 editing competence suite runs only when `OPENREEL_EVAL=1` is explicitly set. It uses generated media, the real MCP server, and an installed subscription harness. CI covers the framework with a fake driver and spends nothing.\n\n## Run\n\n```powershell\n$env:OPENREEL_EVAL = '1'\ncargo run -p openreel-agent --bin openreel-eval\n# Optional: -- --harness codex\n```\n\nResults are written as timestamped, environment-stamped JSONL under `target/evals/`. A full live suite is intentionally expensive and must not be placed in CI.\n\nThe versioned public contract and first machine-readable baseline live under [`benchmarks/auto-edit/v1`](../benchmarks/auto-edit/v1/README.md). A refreshed docs snapshot never overwrites that historical baseline.\n\n## Seed suite\n\n| Eval | Rationale | USD ceiling |\n|---|---|---:|\n",
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
    use super::*;

    #[test]
    fn published_v1_manifest_tracks_the_executable_seed_suite() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v1/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["benchmark_id"], "openreel-auto-edit-v1");
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
        assert_eq!(manifest["benchmark_id"], "openreel-finished-cut-v2");
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
        assert_eq!(manifest["benchmark_id"], "openreel-editorial-cut-v3");
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
        assert_eq!(manifest["benchmark_id"], "openreel-dialogue-pacing-v4");
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
    fn published_v5_manifest_tracks_the_real_interview_suite_and_fixture_pack() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../benchmarks/auto-edit/v5/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest["schema_version"], 5);
        assert_eq!(manifest["benchmark_id"], "openreel-generalization-v5");
        assert_eq!(manifest["status"], "in_progress");
        let definitions = generalization_suite();
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
            Some("recovery-dialogue")
        );
        assert_eq!(deliverable.maximum_word_error_rate_basis_points, 2_000);
        assert_eq!(
            task["budget"]["tool_calls"],
            definition.budgets.max_tool_calls
        );
        assert_eq!(task["budget"]["tokens"], definition.budgets.max_tokens);

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
        assert_eq!(truth.source_story_range.end, 2_548);
        assert_eq!(truth.required_terms.len(), 19);
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
            asset: openreel_core::AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: fps,
            words: vec![
                openreel_core::TranscriptWord {
                    text: "audible".to_owned(),
                    source_start: TimeCode(10),
                    source_end: TimeCode(20),
                    speaker: None,
                },
                openreel_core::TranscriptWord {
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
            spans: vec![openreel_core::SilenceSpan {
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
    #[ignore = "requires local Windows SAPI speech generation and Whisper analysis"]
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
}
