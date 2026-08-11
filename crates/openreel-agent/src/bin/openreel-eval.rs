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
    ClaudeCodeDriver, CodexDriver,
    eval::{
        EnvironmentStamp, EvalAssertion, EvalBudgets, EvalDefinition, EvalError, EvalResult,
        ExpectedSourceClip, FixtureContext, PreparedFixture, render_jsonl, render_scoreboard,
        result_path, run_eval,
    },
};
use openreel_core::{
    AgentDriver, Analysis, AssetId, AssetSceneChanges, AssetSilences, AssetTranscript,
    AuthenticationStatus, Clip, ClipId, Document, MediaAsset, Rational, SceneStatus, SilenceStatus,
    TimeCode, Track, TrackId, TrackKind, TranscriptStatus, map_source_range_to_project,
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
        other => {
            return Err(EvalError::Agent(format!(
                "unknown harness {other:?}; expected claude-code or codex"
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
    let working_directory = env::current_dir().ok();
    let mut results = Vec::with_capacity(definitions.len());
    for (index, definition) in definitions.iter().enumerate() {
        println!(
            "\n[{}/{}] {} - {}",
            index + 1,
            definitions.len(),
            definition.name,
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

    let scoreboard = render_scoreboard(&results);
    println!("\n{scoreboard}");
    let output_root = Path::new("target/evals");
    fs::create_dir_all(output_root).map_err(|error| EvalError::Output(error.to_string()))?;
    let output_path = result_path(output_root, &environment);
    let jsonl = render_jsonl(&environment, &results)?;
    fs::write(&output_path, jsonl).map_err(|error| EvalError::Output(error.to_string()))?;
    let docs = render_evals_document(&definitions, &environment, &results, &output_path);
    fs::write("docs/EVALS.md", docs).map_err(|error| EvalError::Output(error.to_string()))?;
    println!("JSONL: {}", output_path.display());
    println!("Docs: docs/EVALS.md");
    Ok(results.iter().all(|result| result.passed))
}

struct Options {
    harness: String,
    model: Option<String>,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, EvalError> {
        let mut harness = "claude-code".to_owned();
        let mut model = None;
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
                "-h" | "--help" => {
                    println!(
                        "Usage: OPENREEL_EVAL=1 cargo run -p openreel-agent --bin openreel-eval -- [--harness claude-code|codex] [--model MODEL]"
                    );
                    return Err(EvalError::Agent("help requested".to_owned()));
                }
                other => {
                    return Err(EvalError::Agent(format!("unknown argument {other:?}")));
                }
            }
        }
        Ok(Self { harness, model })
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
        max_cost_usd: 0.75,
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
        max_cost_usd: 1.50,
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
    let transcript = wait_for_transcript_result(media.as_ref(), asset.id)?;
    let silences = wait_for_silences(media.as_ref(), asset.id)?;
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
    let maximum = TimeCode(asset.duration.0.saturating_sub(long_silence));
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
    let transcript = wait_for_transcript_result(media.as_ref(), asset.id)?;
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
    let scenes = wait_for_scenes(media.as_ref(), asset.id)?;
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
        transcripts.insert(
            asset.id,
            wait_for_transcript_result(media.as_ref(), asset.id)?,
        );
        silences.insert(asset.id, wait_for_silences(media.as_ref(), asset.id)?);
    }
    let mut context = FixtureContext::default();
    for ((name, _, _), asset) in takes.iter().zip(&assets) {
        context.asset_aliases.insert((*name).to_owned(), asset.id);
    }
    let selected = [0_usize, 2, 3];
    let mut selected_union = BTreeSet::new();
    let mut selected_fillers = BTreeSet::new();
    let mut selected_duration = 0_i64;
    let mut selected_long_silence = 0_i64;
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
        selected_duration = selected_duration.saturating_add(asset.duration.0);
        selected_long_silence = selected_long_silence
            .saturating_add(silence_frames(&silences[&asset.id], LONG_SILENCE_FRAMES));
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
    let maximum = TimeCode(selected_duration.saturating_sub(selected_long_silence));
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
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: Vec::new(),
        }],
        media_pool: assets,
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
            timeline_start,
            effects: Vec::new(),
            transition_in: None,
        });
        timeline_start = timeline_start
            .checked_add(duration)
            .ok_or_else(|| EvalError::Fixture("fixture duration overflowed".to_owned()))?;
    }
    Ok(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: timeline_clips,
        }],
        media_pool: assets,
        fps,
        resolution: (320, 180),
        duration: timeline_start,
    })
}

fn wait_for_transcript_result(
    media: &dyn Analysis,
    asset: AssetId,
) -> Result<Arc<AssetTranscript>, EvalError> {
    let deadline = Instant::now() + Duration::from_mins(20);
    let mut previous = String::new();
    loop {
        let status = media.transcript_status(asset);
        let summary = format!("{status:?}");
        if summary != previous {
            println!("  ASR {asset}: {summary}");
            previous = summary;
        }
        match status {
            TranscriptStatus::Ready(transcript) => return Ok(transcript),
            TranscriptStatus::NoSpeech => {
                return Err(EvalError::Fixture(format!(
                    "asset {asset} produced no ASR speech"
                )));
            }
            TranscriptStatus::Failed(error) => return Err(EvalError::Fixture(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(EvalError::Fixture(format!(
                "asset {asset} transcription timed out"
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_silences(
    media: &dyn Analysis,
    asset: AssetId,
) -> Result<Arc<AssetSilences>, EvalError> {
    let deadline = Instant::now() + Duration::from_mins(3);
    loop {
        match media.silence_status(asset) {
            SilenceStatus::Ready(silences) => return Ok(silences),
            SilenceStatus::NoAudio => {
                return Err(EvalError::Fixture(format!(
                    "asset {asset} has no audio for silence analysis"
                )));
            }
            SilenceStatus::Failed(error) => return Err(EvalError::Fixture(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(EvalError::Fixture(format!(
                "asset {asset} silence analysis timed out"
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_scenes(
    media: &dyn Analysis,
    asset: AssetId,
) -> Result<Arc<AssetSceneChanges>, EvalError> {
    let deadline = Instant::now() + Duration::from_mins(3);
    loop {
        match media.scene_status(asset) {
            SceneStatus::Ready(scenes) => return Ok(scenes),
            SceneStatus::NoVideo => {
                return Err(EvalError::Fixture(format!(
                    "asset {asset} has no video for scene analysis"
                )));
            }
            SceneStatus::Failed(error) => return Err(EvalError::Fixture(error)),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(EvalError::Fixture(format!(
                "asset {asset} scene analysis timed out"
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
        "# Agent evals\n\nOpenReel's Arc 2 editing competence suite runs only when `OPENREEL_EVAL=1` is explicitly set. It uses generated media, the real MCP server, and an installed subscription harness. CI covers the framework with a fake driver and spends nothing.\n\n## Run\n\n```powershell\n$env:OPENREEL_EVAL = '1'\ncargo run -p openreel-agent --bin openreel-eval\n# Optional: -- --harness codex\n```\n\nResults are written as timestamped, environment-stamped JSONL under `target/evals/`. A full live suite is intentionally expensive and must not be placed in CI.\n\n## Seed suite\n\n| Eval | Rationale | USD ceiling |\n|---|---|---:|\n",
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
