use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as _,
    fs,
    path::Path,
    process::ExitCode,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use openreel_agent::{
    ClaudeCodeDriver, CodexDriver, CursorAcpDriver,
    eval::{
        EnvironmentStamp, EvalAssertion, EvalBudgets, EvalDefinition, EvalError, EvalResult,
        ExpectedSourceClip, FixtureContext, PreparedFixture,
        maximum_duration_after_expected_silence_cuts, render_jsonl, render_scoreboard, result_path,
        run_eval,
    },
};
use openreel_core::{
    AgentDriver, Analysis, AssetSceneChanges, AssetSilences, AssetTranscript, AuthenticationStatus,
    Clip, ClipId, Document, MediaAsset, Rational, SceneStatus, SilenceStatus, TimeCode, Track,
    TrackId, TrackKind, TranscriptStatus, map_source_range_to_project,
};
use openreel_media::{
    FfmpegMediaEngine,
    test_support::{GeneratedMedia, SpeechClip, joined_words, normalized_words, test_engine},
};

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
    if env::var("OPENREEL_EVAL").as_deref() != Ok("1") {
        return Err(EvalError::Agent(
            "refusing to run a subscription eval; set OPENREEL_EVAL=1 explicitly".to_owned(),
        ));
    }
    let options = Options::parse(env::args().skip(1))?;
    let driver: Box<dyn AgentDriver> = match options.harness.as_str() {
        "claude" | "claude-code" => Box::new(ClaudeCodeDriver),
        "codex" => Box::new(CodexDriver),
        "cursor" => Box::new(CursorAcpDriver),
        other => {
            return Err(EvalError::Agent(format!(
                "unknown harness {other:?}; expected claude-code, codex, or cursor"
            )));
        }
    };
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
    let definitions = seed_suite();
    let definitions = match &options.only {
        Some(name) => {
            let filtered: Vec<_> = definitions
                .into_iter()
                .filter(|definition| {
                    definition.name == *name
                        || definition.name.split_whitespace().next() == Some(name.as_str())
                })
                .collect();
            if filtered.is_empty() {
                return Err(EvalError::Agent(format!(
                    "--only {name:?} matched no eval in the suite"
                )));
            }
            filtered
        }
        None => definitions,
    };
    let working_directory = env::current_dir().ok();
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
            let result = run_eval(
                definition,
                driver.as_ref(),
                options.model.as_deref(),
                working_directory.as_deref(),
            )
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
    let output_root = Path::new("target/evals");
    fs::create_dir_all(output_root).map_err(|error| EvalError::Output(error.to_string()))?;
    let output_path = result_path(output_root, &environment);
    let jsonl = render_jsonl(&environment, &results)?;
    fs::write(&output_path, jsonl).map_err(|error| EvalError::Output(error.to_string()))?;
    // A filtered or multi-sample run is a measurement exercise, not a new
    // baseline: docs/EVALS.md only records complete single-pass suites.
    if options.only.is_none() && options.samples == 1 {
        let docs = render_evals_document(&definitions, &environment, &results, &output_path);
        fs::write("docs/EVALS.md", docs).map_err(|error| EvalError::Output(error.to_string()))?;
        println!("Docs: docs/EVALS.md");
    }
    println!("JSONL: {}", output_path.display());
    Ok(results.iter().all(|result| result.passed))
}

struct Options {
    harness: String,
    model: Option<String>,
    only: Option<String>,
    samples: u32,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, EvalError> {
        let mut harness = "claude-code".to_owned();
        let mut model = None;
        let mut only = None;
        let mut samples = 1_u32;
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
                "-h" | "--help" => {
                    println!(
                        "Usage: OPENREEL_EVAL=1 cargo run -p openreel-agent --bin openreel-eval -- [--harness claude-code|codex|cursor] [--model MODEL] [--only EVAL] [--samples N]"
                    );
                    return Err(EvalError::Agent("help requested".to_owned()));
                }
                other => {
                    return Err(EvalError::Agent(format!("unknown argument {other:?}")));
                }
            }
        }
        Ok(Self {
            harness,
            model,
            only,
            samples,
        })
    }
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
                required_any(&["apply_edit_plan", "split_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(4, 2),
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
                required_any(&["apply_edit_plan", "trim_clip", "delete_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: speech_budget(12, 12),
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
                required_any(&["apply_edit_plan", "trim_clip", "delete_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: speech_budget(12, 8),
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
                required_any(&["apply_edit_plan", "split_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(10, 8),
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
                required_any(&["apply_edit_plan", "add_effect", "add_transition"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(5, 4),
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
                required_any(&["apply_edit_plan", "split_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: standard_budget(6, 5),
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
                required_any(&["apply_edit_plan", "add_clip"]),
                EvalAssertion::UndoIntegrity,
            ],
            budgets: flagship_budget(),
        },
    ]
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
        max_cost_usd: 2.00,
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
        max_cost_usd: 2.50,
        max_wall_time: Duration::from_mins(40),
        max_undos: 30,
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
        let summary = format!("{status:?}");
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
        let _ = writeln!(
            document,
            "| {} | {} | ${:.2} |",
            definition.name, definition.rationale, definition.budgets.max_cost_usd
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
}
