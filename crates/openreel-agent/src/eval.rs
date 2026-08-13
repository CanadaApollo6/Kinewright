//! Budgeted, typed evaluation support for installed editing-agent harnesses.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openreel_core::{
    AgentDriver, AgentEvent, Analysis, AssetId, AssetSilences, AssetTranscript, Command, Core,
    Document, Event, HarnessInfo, Operation, ParamValue, Playback, Query, QueryResult,
    SessionConfig, SilenceSpan, TimeCode, TimelineSceneChange, TimelineSilenceSpan,
    map_source_range_to_project,
};
use serde::Serialize;
use thiserror::Error;

use crate::{ConfirmationBroker, McpServer, shrink_silence_span_for_cutting_with_transcript};

/// Compute the upper duration bound after cutting every qualifying reported
/// silence. Each cut receives one project-frame boundary-rounding allowance.
#[must_use]
pub fn maximum_duration_after_expected_silence_cuts(
    source_duration: TimeCode,
    silences: &AssetSilences,
    transcript: Option<&AssetTranscript>,
    minimum_source_frames: TimeCode,
) -> TimeCode {
    let (removable_frames, cut_count) = silences
        .spans
        .iter()
        .filter(|span| {
            span.source_end.0.saturating_sub(span.source_start.0) >= minimum_source_frames.0
        })
        .flat_map(|span| {
            shrink_silence_span_for_cutting_with_transcript(
                *span,
                silences.source_fps,
                transcript.map(|transcript| transcript.words.as_slice()),
            )
        })
        .fold((0_i64, 0_i64), |(frames, cuts), span| {
            (
                frames.saturating_add(span.source_end.0.saturating_sub(span.source_start.0)),
                cuts.saturating_add(1),
            )
        });
    TimeCode(
        source_duration
            .0
            .saturating_sub(removable_frames)
            .saturating_add(cut_count),
    )
}

pub type FixtureBuilder = fn() -> Result<PreparedFixture, EvalError>;

#[derive(Debug, Clone, PartialEq)]
pub struct EvalBudgets {
    pub max_turns: u32,
    pub max_tool_calls: u32,
    pub max_operations: u32,
    pub max_tokens: u64,
    pub max_cost_usd: f64,
    pub max_wall_time: Duration,
    pub max_undos: u32,
}

pub struct EvalDefinition {
    pub name: &'static str,
    pub rationale: &'static str,
    pub fixture_builder: FixtureBuilder,
    pub prompts: &'static [&'static str],
    pub assertions: Vec<EvalAssertion>,
    pub budgets: EvalBudgets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSourceClip {
    pub asset_alias: String,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalAssertion {
    TimelineNonEmpty,
    ClipCount {
        minimum: usize,
        maximum: usize,
    },
    AssetOrder {
        aliases: Vec<String>,
        collapse_adjacent: bool,
    },
    AssetAbsent {
        alias: String,
    },
    Gapless,
    DurationBounds {
        bounds: String,
    },
    ExactSourceClips {
        clips: Vec<ExpectedSourceClip>,
    },
    WordsRetained {
        word_set: String,
    },
    WordsAbsent {
        word_set: String,
    },
    NoSilenceAtLeast {
        source_frames: TimeCode,
    },
    SceneChangesAreCuts {
        scene_set: String,
    },
    RequiredToolUsage {
        all_of: Vec<String>,
        any_of: Vec<String>,
    },
    EffectOnAsset {
        asset_alias: String,
        effect_name: String,
        integer_parameter: Option<(String, i64)>,
    },
    TransitionOnAsset {
        asset_alias: String,
        transition_name: String,
    },
    UndoIntegrity,
}

#[derive(Debug, Clone, Default)]
pub struct FixtureContext {
    pub asset_aliases: BTreeMap<String, AssetId>,
    pub transcripts: BTreeMap<AssetId, Arc<AssetTranscript>>,
    pub word_sets: BTreeMap<String, Vec<String>>,
    pub scene_sets: BTreeMap<String, Vec<(AssetId, TimeCode)>>,
    pub duration_bounds: BTreeMap<String, (TimeCode, TimeCode)>,
}

pub struct PreparedFixture {
    pub original_document: Document,
    pub core: Core,
    pub playback: Arc<dyn Playback>,
    pub analysis: Arc<dyn Analysis>,
    pub context: FixtureContext,
    _resources: Vec<Box<dyn Send>>,
}

impl PreparedFixture {
    /// Keep generated resources alive and connect one media engine to a fresh core actor.
    ///
    /// # Errors
    ///
    /// Returns an error if the initial document violates a core invariant.
    pub fn new<T>(
        original_document: Document,
        media: Arc<T>,
        context: FixtureContext,
        resources: Vec<Box<dyn Send>>,
    ) -> Result<Self, EvalError>
    where
        T: Playback + Analysis + 'static,
    {
        original_document
            .validate()
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        media.set_document(Arc::new(original_document.clone()));
        let core = Core::spawn(original_document.clone())
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media;
        Ok(Self {
            original_document,
            core,
            playback,
            analysis,
            context,
            _resources: resources,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SessionMetrics {
    pub turns: u32,
    pub tool_calls: BTreeMap<String, u32>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: u64,
    pub errors: Vec<String>,
    pub interrupted: bool,
}

impl SessionMetrics {
    #[must_use]
    pub fn tool_call_count(&self) -> u32 {
        self.tool_calls.values().copied().sum()
    }

    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, Clone)]
pub struct EvalOutcome {
    pub final_document: Document,
    pub final_words: Vec<String>,
    pub remaining_silences: Vec<TimelineSilenceSpan>,
    pub remaining_scenes: Vec<TimelineSceneChange>,
    pub context: FixtureContext,
    pub session: SessionMetrics,
    pub operations: Vec<Operation>,
    pub undo_steps_to_original: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssertionResult {
    pub assertion: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalResult {
    pub name: String,
    pub rationale: String,
    pub passed: bool,
    pub assertions: Vec<AssertionResult>,
    pub turns: u32,
    pub tool_calls: BTreeMap<String, u32>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: u64,
    pub operations_applied: u32,
    pub execution_error: Option<String>,
}

impl EvalResult {
    #[must_use]
    pub fn execution_failure(definition: &EvalDefinition, error: &EvalError) -> Self {
        Self {
            name: definition.name.to_owned(),
            rationale: definition.rationale.to_owned(),
            passed: false,
            assertions: Vec::new(),
            turns: 0,
            tool_calls: BTreeMap::new(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
            wall_time_ms: 0,
            operations_applied: 0,
            execution_error: Some(error.to_string()),
        }
    }

    #[must_use]
    pub fn passed_assertion_count(&self) -> usize {
        self.assertions
            .iter()
            .filter(|assertion| assertion.passed)
            .count()
    }

    #[must_use]
    pub fn tool_call_count(&self) -> u32 {
        self.tool_calls.values().copied().sum()
    }

    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentStamp {
    pub timestamp_utc: String,
    pub timestamp_unix_ms: u128,
    pub harness: String,
    pub harness_version: Option<String>,
    pub model: String,
    pub os: String,
    pub architecture: String,
    pub openreel_version: String,
}

impl EnvironmentStamp {
    #[must_use]
    pub fn capture(info: Option<&HarnessInfo>, harness: &str, model: Option<&str>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            timestamp_utc: format_utc_timestamp(now.as_secs()),
            timestamp_unix_ms: now.as_millis(),
            harness: harness.to_owned(),
            harness_version: info.and_then(|value| value.version.clone()),
            model: model.unwrap_or("harness-default").to_owned(),
            os: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            openreel_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvalError {
    #[error("fixture setup failed: {0}")]
    Fixture(String),
    #[error("MCP server setup failed: {0}")]
    Server(String),
    #[error("agent session failed: {0}")]
    Agent(String),
    #[error("core query failed: {0}")]
    Core(String),
    #[error("media observation failed: {0}")]
    Media(String),
    #[error("result output failed: {0}")]
    Output(String),
}

/// Execute one real driver session, observe the edited project, and evaluate its contracts.
///
/// # Errors
///
/// Returns setup or observation failures. Agent-reported errors are recorded as failed assertions.
pub fn run_eval(
    definition: &EvalDefinition,
    driver: &dyn AgentDriver,
    model: Option<&str>,
    working_directory: Option<&Path>,
) -> Result<EvalResult, EvalError> {
    let eval_started = Instant::now();
    let fixture = std::panic::catch_unwind(definition.fixture_builder)
        .map_err(|payload| EvalError::Fixture(panic_message(&payload)))??;
    let server = McpServer::start(
        fixture.core.clone(),
        Arc::clone(&fixture.playback),
        Arc::clone(&fixture.analysis),
    )
    .map_err(|error| EvalError::Server(error.to_string()))?;
    let confirmations = server.confirmations();
    let config = SessionConfig {
        working_directory: working_directory.map(Path::to_path_buf),
        model: model.map(str::to_owned),
        effort: None,
        service_tier: None,
        max_turns: Some(definition.budgets.max_tool_calls.saturating_add(2)),
        mcp_url: Some(server.endpoint().to_owned()),
    };
    let mut session = collect_session(
        driver,
        config,
        definition.prompts,
        &definition.budgets,
        Some(&confirmations),
        || query_operations(&fixture.core).map(|operations| operations.len()),
    )?;
    let final_document = query_document(&fixture.core)?;
    let operations = query_operations(&fixture.core)?;
    let final_words = fixture
        .analysis
        .timeline_transcript(&final_document, None)
        .map_err(|error| EvalError::Media(error.to_string()))?
        .into_iter()
        .map(|word| word.text)
        .collect::<Vec<_>>();
    let remaining_silences = fixture
        .analysis
        .timeline_silences(&final_document, None, TimeCode(1))
        .map_err(|error| EvalError::Media(error.to_string()))?;
    let remaining_scenes = fixture
        .analysis
        .timeline_scene_changes(&final_document, None, 0)
        .map_err(|error| EvalError::Media(error.to_string()))?;
    let undo_steps_to_original = restore_original(
        &fixture.core,
        &fixture.original_document,
        definition.budgets.max_undos,
    )?;
    session.wall_time_ms = duration_millis(eval_started.elapsed());
    let outcome = EvalOutcome {
        final_document: (*final_document).clone(),
        final_words,
        remaining_silences,
        remaining_scenes,
        context: fixture.context.clone(),
        session,
        operations,
        undo_steps_to_original,
    };
    let result = evaluate(definition, &outcome);
    server.shutdown();
    Ok(result)
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload.downcast_ref::<String>().map_or_else(
        || {
            payload.downcast_ref::<&str>().map_or_else(
                || "fixture builder panicked".to_owned(),
                |message| (*message).to_owned(),
            )
        },
        Clone::clone,
    )
}

/// Run prompt turns through an `AgentDriver`, including a fake driver in unit tests.
///
/// # Errors
///
/// Returns driver setup/protocol failures or an operation-budget probe failure.
#[allow(clippy::too_many_lines)]
pub fn collect_session<F>(
    driver: &dyn AgentDriver,
    config: SessionConfig,
    prompts: &[&str],
    budgets: &EvalBudgets,
    confirmations: Option<&ConfirmationBroker>,
    mut operation_count: F,
) -> Result<SessionMetrics, EvalError>
where
    F: FnMut() -> Result<usize, EvalError>,
{
    let started = Instant::now();
    let mut session = driver
        .start_session(config)
        .map_err(|error| EvalError::Agent(error.to_string()))?;
    let events = session.events();
    let mut metrics = SessionMetrics {
        cost_usd: Some(0.0),
        ..SessionMetrics::default()
    };
    let mut cost_is_complete = true;
    for prompt in prompts {
        if metrics.turns >= budgets.max_turns {
            metrics.errors.push(format!(
                "turn budget exceeded before prompt {}",
                metrics.turns.saturating_add(1)
            ));
            metrics.interrupted = true;
            session.interrupt();
            break;
        }
        session
            .send_user_message((*prompt).to_owned())
            .map_err(|error| EvalError::Agent(error.to_string()))?;
        metrics.turns = metrics.turns.saturating_add(1);
        let mut turn_done = false;
        while !turn_done {
            if let Some(broker) = confirmations {
                for request in broker.pending_requests() {
                    if !broker.approve(request.id) {
                        metrics.errors.push(format!(
                            "confirmation {} disappeared before approval",
                            request.id
                        ));
                    }
                }
            }
            if started.elapsed() > budgets.max_wall_time {
                metrics.errors.push(format!(
                    "wall-time budget exceeded ({:.1}s)",
                    budgets.max_wall_time.as_secs_f64()
                ));
                metrics.interrupted = true;
                session.interrupt();
                break;
            }
            let observed_operations = operation_count()?;
            if observed_operations > usize::try_from(budgets.max_operations).unwrap_or(usize::MAX) {
                metrics.errors.push(format!(
                    "operation budget exceeded ({observed_operations} > {})",
                    budgets.max_operations
                ));
                metrics.interrupted = true;
                session.interrupt();
                break;
            }
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(AgentEvent::Error(error)) => metrics.errors.push(error),
                Ok(AgentEvent::ToolCall { name, .. }) => {
                    let count = metrics.tool_calls.entry(name).or_default();
                    *count = count.saturating_add(1);
                    if metrics.tool_call_count() > budgets.max_tool_calls {
                        metrics.errors.push(format!(
                            "tool-call budget exceeded ({} > {})",
                            metrics.tool_call_count(),
                            budgets.max_tool_calls
                        ));
                        metrics.interrupted = true;
                        session.interrupt();
                        break;
                    }
                }
                Ok(AgentEvent::Cost {
                    input_tokens,
                    output_tokens,
                    cost_usd,
                }) => {
                    metrics.input_tokens = metrics.input_tokens.saturating_add(input_tokens);
                    metrics.output_tokens = metrics.output_tokens.saturating_add(output_tokens);
                    match cost_usd {
                        Some(cost) if cost_is_complete => {
                            let total = metrics.cost_usd.get_or_insert(0.0);
                            *total += cost;
                            if *total > budgets.max_cost_usd {
                                metrics.errors.push(format!(
                                    "cost ceiling exceeded (${total:.4} > ${:.2})",
                                    budgets.max_cost_usd
                                ));
                                metrics.interrupted = true;
                                session.interrupt();
                                break;
                            }
                        }
                        Some(_) => {}
                        None => {
                            cost_is_complete = false;
                            metrics.cost_usd = None;
                        }
                    }
                }
                Ok(AgentEvent::Done) => turn_done = true,
                Ok(AgentEvent::Text(_) | AgentEvent::ToolResult { .. })
                | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    metrics
                        .errors
                        .push("agent event stream disconnected".to_owned());
                    metrics.interrupted = true;
                    break;
                }
            }
        }
        if metrics.interrupted {
            break;
        }
    }
    metrics.wall_time_ms = duration_millis(started.elapsed());
    session.interrupt();
    Ok(metrics)
}

#[must_use]
pub fn evaluate(definition: &EvalDefinition, outcome: &EvalOutcome) -> EvalResult {
    let mut assertions = definition
        .assertions
        .iter()
        .map(|assertion| evaluate_assertion(assertion, definition, outcome))
        .collect::<Vec<_>>();
    assertions.extend(evaluate_budgets(&definition.budgets, outcome));
    let passed = assertions.iter().all(|assertion| assertion.passed);
    EvalResult {
        name: definition.name.to_owned(),
        rationale: definition.rationale.to_owned(),
        passed,
        assertions,
        turns: outcome.session.turns,
        tool_calls: outcome.session.tool_calls.clone(),
        input_tokens: outcome.session.input_tokens,
        output_tokens: outcome.session.output_tokens,
        cost_usd: outcome.session.cost_usd,
        wall_time_ms: outcome.session.wall_time_ms,
        operations_applied: u32::try_from(outcome.operations.len()).unwrap_or(u32::MAX),
        execution_error: None,
    }
}

#[allow(clippy::too_many_lines)]
fn evaluate_assertion(
    assertion: &EvalAssertion,
    definition: &EvalDefinition,
    outcome: &EvalOutcome,
) -> AssertionResult {
    match assertion {
        EvalAssertion::TimelineNonEmpty => {
            let count = timeline_clips(&outcome.final_document).count();
            assertion_result(
                "timeline non-empty",
                count > 0,
                format!("observed {count} clips"),
            )
        }
        EvalAssertion::ClipCount { minimum, maximum } => {
            let count = timeline_clips(&outcome.final_document).count();
            assertion_result(
                "clip count",
                (*minimum..=*maximum).contains(&count),
                format!("expected {minimum}..={maximum}, observed {count}"),
            )
        }
        EvalAssertion::AssetOrder {
            aliases,
            collapse_adjacent,
        } => evaluate_asset_order(aliases, *collapse_adjacent, outcome),
        EvalAssertion::AssetAbsent { alias } => {
            let Some(asset) = outcome.context.asset_aliases.get(alias) else {
                return assertion_result(
                    "asset absent",
                    false,
                    format!("unknown asset alias {alias:?}"),
                );
            };
            let present = timeline_clips(&outcome.final_document).any(|clip| clip.asset == *asset);
            assertion_result(
                "asset absent",
                !present,
                format!("asset {alias} ({asset}) present={present}"),
            )
        }
        EvalAssertion::Gapless => evaluate_gapless(&outcome.final_document),
        EvalAssertion::DurationBounds { bounds } => {
            let Some((minimum, maximum)) = outcome.context.duration_bounds.get(bounds) else {
                return assertion_result(
                    "duration bounds",
                    false,
                    format!("unknown duration bounds {bounds:?}"),
                );
            };
            assertion_result(
                "duration bounds",
                (*minimum..=*maximum).contains(&outcome.final_document.duration),
                format!(
                    "expected {}..={} frames, observed {}",
                    minimum.0, maximum.0, outcome.final_document.duration.0
                ),
            )
        }
        EvalAssertion::ExactSourceClips { clips } => evaluate_source_clips(clips, outcome),
        EvalAssertion::WordsRetained { word_set } => evaluate_word_set(word_set, outcome, true),
        EvalAssertion::WordsAbsent { word_set } => evaluate_word_set(word_set, outcome, false),
        EvalAssertion::NoSilenceAtLeast { source_frames } => {
            let remaining = outcome
                .remaining_silences
                .iter()
                .filter(|span| {
                    span.source_end.0.saturating_sub(span.source_start.0) >= source_frames.0
                })
                .flat_map(|span| {
                    let source_fps = outcome
                        .final_document
                        .asset(span.asset)
                        .map_or(outcome.final_document.fps, |asset| asset.fps);
                    let transcript = outcome.context.transcripts.get(&span.asset);
                    shrink_silence_span_for_cutting_with_transcript(
                        SilenceSpan {
                            source_start: span.source_start,
                            source_end: span.source_end,
                        },
                        source_fps,
                        transcript.map(|transcript| transcript.words.as_slice()),
                    )
                })
                .count();
            assertion_result(
                "long silence absent",
                remaining == 0,
                format!(
                    "observed {remaining} cuttable silence spans from raw spans at least {} source frames",
                    source_frames.0
                ),
            )
        }
        EvalAssertion::SceneChangesAreCuts { scene_set } => evaluate_scene_cuts(scene_set, outcome),
        EvalAssertion::RequiredToolUsage { all_of, any_of } => {
            let called = outcome
                .session
                .tool_calls
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let missing = all_of
                .iter()
                .filter(|tool| !called.contains(tool.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let any_match =
                any_of.is_empty() || any_of.iter().any(|tool| called.contains(tool.as_str()));
            assertion_result(
                "required tool usage",
                missing.is_empty() && any_match,
                format!(
                    "called={called:?}, missing_all={missing:?}, any_of={any_of:?} matched={any_match}"
                ),
            )
        }
        EvalAssertion::EffectOnAsset {
            asset_alias,
            effect_name,
            integer_parameter,
        } => evaluate_effect(
            asset_alias,
            effect_name,
            integer_parameter.as_ref(),
            outcome,
        ),
        EvalAssertion::TransitionOnAsset {
            asset_alias,
            transition_name,
        } => evaluate_transition(asset_alias, transition_name, outcome),
        EvalAssertion::UndoIntegrity => assertion_result(
            "undo integrity",
            outcome.undo_steps_to_original.is_some(),
            outcome.undo_steps_to_original.map_or_else(
                || {
                    format!(
                        "original was not restored within {} undos",
                        definition.budgets.max_undos
                    )
                },
                |steps| format!("original restored after {steps} undos"),
            ),
        ),
    }
}

fn evaluate_budgets(budgets: &EvalBudgets, outcome: &EvalOutcome) -> Vec<AssertionResult> {
    let turns = outcome.session.turns;
    let tool_calls = outcome.session.tool_call_count();
    let operations = u32::try_from(outcome.operations.len()).unwrap_or(u32::MAX);
    let tokens = outcome.session.total_tokens();
    let mut results = vec![
        assertion_result(
            "agent completed without errors",
            outcome.session.errors.is_empty(),
            if outcome.session.errors.is_empty() {
                "no driver errors".to_owned()
            } else {
                outcome.session.errors.join("; ")
            },
        ),
        assertion_result(
            "turn budget",
            turns <= budgets.max_turns,
            format!("{turns} <= {}", budgets.max_turns),
        ),
        assertion_result(
            "tool-call budget",
            tool_calls <= budgets.max_tool_calls,
            format!("{tool_calls} <= {}", budgets.max_tool_calls),
        ),
        assertion_result(
            "operation budget",
            operations <= budgets.max_operations,
            format!("{operations} <= {}", budgets.max_operations),
        ),
        assertion_result(
            "token budget",
            tokens <= budgets.max_tokens,
            format!("{tokens} <= {}", budgets.max_tokens),
        ),
        assertion_result(
            "wall-time budget",
            outcome.session.wall_time_ms <= duration_millis(budgets.max_wall_time),
            format!(
                "{}ms <= {}ms",
                outcome.session.wall_time_ms,
                duration_millis(budgets.max_wall_time)
            ),
        ),
    ];
    let cost = outcome.session.cost_usd;
    results.push(assertion_result(
        "cost ceiling",
        cost.is_some_and(|value| value <= budgets.max_cost_usd),
        cost.map_or_else(
            || "harness did not report USD cost".to_owned(),
            |value| format!("${value:.4} <= ${:.2}", budgets.max_cost_usd),
        ),
    ));
    results
}

fn evaluate_asset_order(
    aliases: &[String],
    collapse_adjacent: bool,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let reverse = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut observed = timeline_clips(&outcome.final_document)
        .map(|clip| {
            reverse.get(&clip.asset).map_or_else(
                || format!("asset-{}", clip.asset.0),
                |alias| (*alias).to_owned(),
            )
        })
        .collect::<Vec<_>>();
    if collapse_adjacent {
        observed.dedup();
    }
    assertion_result(
        "asset order",
        observed == aliases,
        format!("expected {aliases:?}, observed {observed:?}"),
    )
}

fn evaluate_gapless(document: &Document) -> AssertionResult {
    let mut errors = Vec::new();
    for track in &document.tracks {
        if let Some(first) = track.clips.first()
            && first.timeline_start != TimeCode::ZERO
        {
            errors.push(format!(
                "track {} starts at frame {}",
                track.id, first.timeline_start.0
            ));
        }
        for adjacent in track.clips.windows(2) {
            let Some(asset) = document.asset(adjacent[0].asset) else {
                errors.push(format!("missing asset {}", adjacent[0].asset));
                continue;
            };
            match map_source_range_to_project(
                adjacent[0].source_range.clone(),
                asset.fps,
                document.fps,
            )
            .and_then(|duration| {
                adjacent[0]
                    .timeline_start
                    .checked_add(duration)
                    .ok_or(openreel_core::TimeMappingError::Overflow)
            }) {
                Ok(left_end) if left_end == adjacent[1].timeline_start => {}
                Ok(left_end) => errors.push(format!(
                    "track {} gap/overlap between clips {} and {}: {} then {}",
                    track.id,
                    adjacent[0].id,
                    adjacent[1].id,
                    left_end.0,
                    adjacent[1].timeline_start.0
                )),
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    assertion_result(
        "timeline gapless",
        errors.is_empty(),
        if errors.is_empty() {
            "all populated tracks start at zero and are contiguous".to_owned()
        } else {
            errors.join("; ")
        },
    )
}

fn evaluate_source_clips(clips: &[ExpectedSourceClip], outcome: &EvalOutcome) -> AssertionResult {
    let reverse = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let observed = timeline_clips(&outcome.final_document)
        .map(|clip| ExpectedSourceClip {
            asset_alias: reverse.get(&clip.asset).map_or_else(
                || format!("asset-{}", clip.asset.0),
                |alias| (*alias).to_owned(),
            ),
            source_start: clip.source_range.start,
            source_end: clip.source_range.end,
        })
        .collect::<Vec<_>>();
    assertion_result(
        "exact source clips",
        observed == clips,
        format!("expected {clips:?}, observed {observed:?}"),
    )
}

fn evaluate_word_set(word_set: &str, outcome: &EvalOutcome, retained: bool) -> AssertionResult {
    let Some(expected) = outcome.context.word_sets.get(word_set) else {
        return assertion_result(
            if retained {
                "words retained"
            } else {
                "words absent"
            },
            false,
            format!("unknown word set {word_set:?}"),
        );
    };
    if expected.is_empty() {
        return assertion_result(
            if retained {
                "words retained"
            } else {
                "words absent"
            },
            false,
            format!("pre-edit word set {word_set:?} is empty"),
        );
    }
    let final_words = normalize_words(outcome.final_words.iter().map(String::as_str));
    let expected = normalize_words(expected.iter().map(String::as_str));
    let matches = expected
        .iter()
        .filter(|word| final_words.contains(*word))
        .cloned()
        .collect::<BTreeSet<_>>();
    let passed = if retained {
        matches == expected
    } else {
        matches.is_empty()
    };
    assertion_result(
        if retained {
            "words retained"
        } else {
            "words absent"
        },
        passed,
        format!("pre-edit set={word_set:?} expected={expected:?}, present after edit={matches:?}"),
    )
}

fn evaluate_scene_cuts(scene_set: &str, outcome: &EvalOutcome) -> AssertionResult {
    let Some(scenes) = outcome.context.scene_sets.get(scene_set) else {
        return assertion_result(
            "scene changes are cuts",
            false,
            format!("unknown scene set {scene_set:?}"),
        );
    };
    if scenes.is_empty() {
        return assertion_result(
            "scene changes are cuts",
            false,
            format!("pre-edit scene set {scene_set:?} is empty"),
        );
    }
    let missing = scenes
        .iter()
        .filter(|(asset, source_frame)| {
            !timeline_clips(&outcome.final_document)
                .any(|clip| clip.asset == *asset && clip.source_range.start == *source_frame)
        })
        .copied()
        .collect::<Vec<_>>();
    let interior = scenes
        .iter()
        .filter(|(asset, source_frame)| {
            timeline_clips(&outcome.final_document).any(|clip| {
                clip.asset == *asset
                    && clip.source_range.start < *source_frame
                    && *source_frame < clip.source_range.end
            })
        })
        .copied()
        .collect::<Vec<_>>();
    assertion_result(
        "scene changes are cuts",
        missing.is_empty() && interior.is_empty(),
        format!("missing boundaries={missing:?}, interior changes={interior:?}"),
    )
}

fn evaluate_effect(
    asset_alias: &str,
    effect_name: &str,
    integer_parameter: Option<&(String, i64)>,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return assertion_result(
            "effect on asset",
            false,
            format!("unknown asset alias {asset_alias:?}"),
        );
    };
    let matched = timeline_clips(&outcome.final_document)
        .filter(|clip| clip.asset == *asset)
        .flat_map(|clip| &clip.effects)
        .any(|effect| {
            effect.name == effect_name
                && integer_parameter.as_ref().is_none_or(|(name, expected)| {
                    effect.parameters.get(name) == Some(&ParamValue::Integer(*expected))
                })
        });
    assertion_result(
        "effect on asset",
        matched,
        format!(
            "asset={asset_alias:?}, effect={effect_name:?}, parameter={integer_parameter:?}, matched={matched}"
        ),
    )
}

fn evaluate_transition(
    asset_alias: &str,
    transition_name: &str,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return assertion_result(
            "transition on asset",
            false,
            format!("unknown asset alias {asset_alias:?}"),
        );
    };
    let matched = timeline_clips(&outcome.final_document)
        .filter(|clip| clip.asset == *asset)
        .any(|clip| {
            clip.transition_in
                .as_ref()
                .is_some_and(|transition| transition.name == transition_name)
        });
    assertion_result(
        "transition on asset",
        matched,
        format!("asset={asset_alias:?}, transition={transition_name:?}, matched={matched}"),
    )
}

fn assertion_result(assertion: impl Into<String>, passed: bool, detail: String) -> AssertionResult {
    AssertionResult {
        assertion: assertion.into(),
        passed,
        detail,
    }
}

fn timeline_clips(document: &Document) -> impl Iterator<Item = &openreel_core::Clip> {
    document.tracks.iter().flat_map(|track| &track.clips)
}

fn normalize_words<'a>(words: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    words
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphabetic())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn query_document(core: &Core) -> Result<Arc<Document>, EvalError> {
    let Event::QueryResult(QueryResult::Document(document)) = core
        .request(Command::Query(Query::Document))
        .map_err(|error| EvalError::Core(error.to_string()))?
    else {
        return Err(EvalError::Core(
            "document query returned the wrong event".to_owned(),
        ));
    };
    Ok(document)
}

fn query_operations(core: &Core) -> Result<Vec<Operation>, EvalError> {
    let Event::QueryResult(QueryResult::OpLog(operations)) = core
        .request(Command::Query(Query::OpLog))
        .map_err(|error| EvalError::Core(error.to_string()))?
    else {
        return Err(EvalError::Core(
            "operation-log query returned the wrong event".to_owned(),
        ));
    };
    Ok((*operations).clone())
}

fn restore_original(
    core: &Core,
    original: &Document,
    maximum_undos: u32,
) -> Result<Option<u32>, EvalError> {
    if &*query_document(core)? == original {
        return Ok(Some(0));
    }
    for step in 1..=maximum_undos {
        let Event::DocumentChanged { doc, .. } = core
            .request(Command::Undo)
            .map_err(|error| EvalError::Core(error.to_string()))?
        else {
            return Err(EvalError::Core("undo returned the wrong event".to_owned()));
        };
        if &*doc == original {
            return Ok(Some(step));
        }
    }
    Ok(None)
}

#[derive(Serialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
enum JsonlRecord<'a> {
    Environment { environment: &'a EnvironmentStamp },
    EvalResult { result: &'a EvalResult },
    Totals { totals: SuiteTotals },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SuiteTotals {
    pub evals: usize,
    pub passed: usize,
    pub failed: usize,
    pub turns: u32,
    pub tool_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: u64,
    pub operations_applied: u32,
}

impl SuiteTotals {
    #[must_use]
    pub fn from_results(results: &[EvalResult]) -> Self {
        let all_costs_reported = results.iter().all(|result| result.cost_usd.is_some());
        Self {
            evals: results.len(),
            passed: results.iter().filter(|result| result.passed).count(),
            failed: results.iter().filter(|result| !result.passed).count(),
            turns: results.iter().map(|result| result.turns).sum(),
            tool_calls: results.iter().map(EvalResult::tool_call_count).sum(),
            input_tokens: results.iter().map(|result| result.input_tokens).sum(),
            output_tokens: results.iter().map(|result| result.output_tokens).sum(),
            cost_usd: all_costs_reported
                .then(|| results.iter().filter_map(|result| result.cost_usd).sum()),
            wall_time_ms: results.iter().map(|result| result.wall_time_ms).sum(),
            operations_applied: results.iter().map(|result| result.operations_applied).sum(),
        }
    }
}

/// Render the same compact Markdown scoreboard used on stdout and in `docs/EVALS.md`.
#[must_use]
pub fn render_scoreboard(results: &[EvalResult]) -> String {
    let mut output = String::from(
        "| Eval | Pass | Assertions | Turns | Tools | Tokens | USD | Wall | Ops |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for result in results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        let usd = result
            .cost_usd
            .map_or_else(|| "n/a".to_owned(), |cost| format!("${cost:.4}"));
        let _ = writeln!(
            output,
            "| {} | {status} | {}/{} | {} | {} | {} | {usd} | {} | {} |",
            result.name,
            result.passed_assertion_count(),
            result.assertions.len(),
            result.turns,
            result.tool_call_count(),
            result.total_tokens(),
            format_millis(result.wall_time_ms),
            result.operations_applied,
        );
    }
    let totals = SuiteTotals::from_results(results);
    let status = if totals.failed == 0 { "PASS" } else { "FAIL" };
    let usd = totals
        .cost_usd
        .map_or_else(|| "n/a".to_owned(), |cost| format!("${cost:.4}"));
    let assertion_passes = results
        .iter()
        .map(EvalResult::passed_assertion_count)
        .sum::<usize>();
    let assertion_total = results
        .iter()
        .map(|result| result.assertions.len())
        .sum::<usize>();
    let _ = writeln!(
        output,
        "| **TOTAL** | **{status}** | **{assertion_passes}/{assertion_total}** | **{}** | **{}** | **{}** | **{usd}** | **{}** | **{}** |",
        totals.turns,
        totals.tool_calls,
        totals.input_tokens.saturating_add(totals.output_tokens),
        format_millis(totals.wall_time_ms),
        totals.operations_applied,
    );
    output
}

/// Serialize one environment header, one line per eval, and one totals footer.
///
/// # Errors
///
/// Returns a serialization error if a result cannot be represented as JSON.
pub fn render_jsonl(
    environment: &EnvironmentStamp,
    results: &[EvalResult],
) -> Result<String, EvalError> {
    let mut output = String::new();
    append_json_line(&mut output, &JsonlRecord::Environment { environment })?;
    for result in results {
        append_json_line(&mut output, &JsonlRecord::EvalResult { result })?;
    }
    append_json_line(
        &mut output,
        &JsonlRecord::Totals {
            totals: SuiteTotals::from_results(results),
        },
    )?;
    Ok(output)
}

fn append_json_line<T: Serialize>(output: &mut String, value: &T) -> Result<(), EvalError> {
    let line =
        serde_json::to_string(value).map_err(|error| EvalError::Output(error.to_string()))?;
    output.push_str(&line);
    output.push('\n');
    Ok(())
}

#[must_use]
pub fn result_path(root: &Path, environment: &EnvironmentStamp) -> PathBuf {
    let timestamp = environment
        .timestamp_utc
        .replace([':', '-'], "")
        .replace('T', "-")
        .replace('Z', "");
    root.join(format!(
        "openreel-eval-{timestamp}-{}.jsonl",
        environment.harness
    ))
}

fn format_millis(milliseconds: u64) -> String {
    format!(
        "{}.{:01}s",
        milliseconds / 1_000,
        milliseconds % 1_000 / 100
    )
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn format_utc_timestamp(unix_seconds: u64) -> String {
    let days = i64::try_from(unix_seconds / 86_400).unwrap_or(i64::MAX);
    let seconds = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = seconds % 3_600 / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

const fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Mutex};

    use crossbeam_channel::{Receiver, Sender, unbounded};
    use openreel_core::{
        AgentError, AgentSession, AssetSilences, AuthenticationStatus, Clip, ClipId, HarnessId,
        MediaAsset, MediaKind, Rational, SilenceSpan, Track, TrackId, TrackKind,
    };

    use super::*;

    struct FakeDriver {
        events: Vec<AgentEvent>,
    }

    impl AgentDriver for FakeDriver {
        fn id(&self) -> HarnessId {
            HarnessId::new("fake")
        }

        fn detect(&self) -> Option<HarnessInfo> {
            Some(HarnessInfo {
                id: self.id(),
                executable: PathBuf::from("fake"),
                version: Some("1.0".to_owned()),
                authentication: AuthenticationStatus::Authenticated,
                subscription_tier: None,
            })
        }

        fn start_session(&self, _cfg: SessionConfig) -> Result<Box<dyn AgentSession>, AgentError> {
            let (sender, receiver) = unbounded();
            Ok(Box::new(FakeSession {
                scripted: Mutex::new(Some(self.events.clone())),
                sender,
                receiver,
            }))
        }
    }

    struct FakeSession {
        scripted: Mutex<Option<Vec<AgentEvent>>>,
        sender: Sender<AgentEvent>,
        receiver: Receiver<AgentEvent>,
    }

    impl AgentSession for FakeSession {
        fn send_user_message(&mut self, _text: String) -> Result<(), AgentError> {
            for event in self
                .scripted
                .lock()
                .map_err(|_| AgentError::Harness("fake lock poisoned".to_owned()))?
                .take()
                .unwrap_or_default()
            {
                self.sender
                    .send(event)
                    .map_err(|error| AgentError::Harness(error.to_string()))?;
            }
            Ok(())
        }

        fn events(&self) -> Receiver<AgentEvent> {
            self.receiver.clone()
        }

        fn interrupt(&mut self) {}
    }

    fn budgets() -> EvalBudgets {
        EvalBudgets {
            max_turns: 1,
            max_tool_calls: 4,
            max_operations: 3,
            max_tokens: 1_000,
            max_cost_usd: 0.75,
            max_wall_time: Duration::from_secs(1),
            max_undos: 2,
        }
    }

    fn document() -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("fixture.mp4"),
            name: "fixture".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((320, 180)),
        };
        Document {
            catalog: openreel_core::MediaCatalog::default(),
            audio_mix: openreel_core::AudioMix::default(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(60),
                    content: openreel_core::ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(60),
        }
    }

    fn unused_fixture() -> Result<PreparedFixture, EvalError> {
        Err(EvalError::Fixture("unused by unit test".to_owned()))
    }

    #[test]
    fn fake_driver_metrics_are_collected_without_a_subscription_call() {
        let driver = FakeDriver {
            events: vec![
                AgentEvent::ToolCall {
                    name: "get_timeline_state".to_owned(),
                    arguments: "{}".to_owned(),
                },
                AgentEvent::ToolCall {
                    name: "apply_edit_plan".to_owned(),
                    arguments: "{}".to_owned(),
                },
                AgentEvent::Cost {
                    input_tokens: 120,
                    output_tokens: 30,
                    cost_usd: Some(0.04),
                },
                AgentEvent::Done,
            ],
        };
        let metrics = collect_session(
            &driver,
            SessionConfig::default(),
            &["edit it"],
            &budgets(),
            None,
            || Ok(2),
        )
        .unwrap();
        assert_eq!(metrics.turns, 1);
        assert_eq!(metrics.tool_call_count(), 2);
        assert_eq!(metrics.total_tokens(), 150);
        assert_eq!(metrics.cost_usd, Some(0.04));
        assert!(metrics.errors.is_empty());
    }

    #[test]
    fn fake_driver_eval_accepts_the_transcript_clamped_bound_and_rounding_allowance() {
        let silences = AssetSilences {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(100),
            threshold_dbfs_hundredths: -4_000,
            window_milliseconds: 20,
            spans: vec![
                SilenceSpan {
                    source_start: TimeCode(10),
                    source_end: TimeCode(40),
                },
                SilenceSpan {
                    source_start: TimeCode(50),
                    source_end: TimeCode(80),
                },
            ],
        };
        let transcript = AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            words: vec![
                openreel_core::TranscriptWord {
                    text: "left".to_owned(),
                    source_start: TimeCode(0),
                    source_end: TimeCode(15),
                    speaker: None,
                },
                openreel_core::TranscriptWord {
                    text: "middle".to_owned(),
                    source_start: TimeCode(38),
                    source_end: TimeCode(52),
                    speaker: None,
                },
                openreel_core::TranscriptWord {
                    text: "right".to_owned(),
                    source_start: TimeCode(78),
                    source_end: TimeCode(90),
                    speaker: None,
                },
            ],
        };
        let maximum = maximum_duration_after_expected_silence_cuts(
            TimeCode(100),
            &silences,
            Some(&transcript),
            TimeCode(20),
        );
        assert_eq!(maximum, TimeCode(65));

        let driver = FakeDriver {
            events: vec![AgentEvent::Done],
        };
        let session = collect_session(
            &driver,
            SessionConfig::default(),
            &["remove the silence"],
            &budgets(),
            None,
            || Ok(0),
        )
        .unwrap();
        let mut final_document = document();
        final_document.media_pool[0].duration = TimeCode(100);
        final_document.tracks[0].clips[0].source_range.end = maximum;
        final_document.duration = maximum;
        let mut context = FixtureContext::default();
        context.transcripts.insert(AssetId(1), Arc::new(transcript));
        context
            .duration_bounds
            .insert("padded-cut".to_owned(), (TimeCode(20), maximum));
        let definition = EvalDefinition {
            name: "fake-padded-bound",
            rationale: "exercise padded silence bounds",
            fixture_builder: unused_fixture,
            prompts: &["remove the silence"],
            assertions: vec![EvalAssertion::DurationBounds {
                bounds: "padded-cut".to_owned(),
            }],
            budgets: budgets(),
        };
        let outcome = EvalOutcome {
            final_document,
            final_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context,
            session,
            operations: Vec::new(),
            undo_steps_to_original: None,
        };

        let result = evaluate(&definition, &outcome);
        assert!(result.passed, "{:#?}", result.assertions);
    }

    #[test]
    fn typed_predicates_cover_shape_words_tools_budgets_and_undo() {
        let mut context = FixtureContext::default();
        context
            .asset_aliases
            .insert("take-a".to_owned(), AssetId(1));
        context.word_sets.insert(
            "content".to_owned(),
            vec!["Alpha".to_owned(), "Bravo".to_owned()],
        );
        context
            .word_sets
            .insert("removed".to_owned(), vec!["um".to_owned()]);
        context
            .duration_bounds
            .insert("rough-cut".to_owned(), (TimeCode(50), TimeCode(70)));
        let definition = EvalDefinition {
            name: "fake-eval",
            rationale: "exercise predicates",
            fixture_builder: unused_fixture,
            prompts: &["edit it"],
            assertions: vec![
                EvalAssertion::TimelineNonEmpty,
                EvalAssertion::ClipCount {
                    minimum: 1,
                    maximum: 1,
                },
                EvalAssertion::Gapless,
                EvalAssertion::DurationBounds {
                    bounds: "rough-cut".to_owned(),
                },
                EvalAssertion::WordsRetained {
                    word_set: "content".to_owned(),
                },
                EvalAssertion::WordsAbsent {
                    word_set: "removed".to_owned(),
                },
                EvalAssertion::RequiredToolUsage {
                    all_of: vec!["apply_edit_plan".to_owned()],
                    any_of: vec!["get_timeline_state".to_owned()],
                },
                EvalAssertion::UndoIntegrity,
            ],
            budgets: budgets(),
        };
        let outcome = EvalOutcome {
            final_document: document(),
            final_words: vec!["alpha".to_owned(), "bravo".to_owned()],
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context,
            session: SessionMetrics {
                turns: 1,
                tool_calls: BTreeMap::from([
                    ("apply_edit_plan".to_owned(), 1),
                    ("get_timeline_state".to_owned(), 1),
                ]),
                input_tokens: 100,
                output_tokens: 20,
                cost_usd: Some(0.03),
                wall_time_ms: 10,
                errors: Vec::new(),
                interrupted: false,
            },
            operations: vec![Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(30),
            }],
            undo_steps_to_original: Some(1),
        };
        let result = evaluate(&definition, &outcome);
        assert!(result.passed, "{:#?}", result.assertions);
        assert!(result.assertions.iter().all(|assertion| assertion.passed));
    }

    #[test]
    fn scoreboard_and_jsonl_have_stable_machine_readable_shapes() {
        let definition = EvalDefinition {
            name: "fake-eval",
            rationale: "exercise reporting",
            fixture_builder: unused_fixture,
            prompts: &["edit it"],
            assertions: Vec::new(),
            budgets: budgets(),
        };
        let mut result =
            EvalResult::execution_failure(&definition, &EvalError::Agent("deliberate".to_owned()));
        result.cost_usd = Some(0.0);
        let scoreboard = render_scoreboard(std::slice::from_ref(&result));
        assert!(scoreboard.contains("| fake-eval | FAIL |"));
        assert!(scoreboard.contains("| **TOTAL** | **FAIL** |"));

        let environment = EnvironmentStamp {
            timestamp_utc: "2026-08-10T12:00:00Z".to_owned(),
            timestamp_unix_ms: 0,
            harness: "fake".to_owned(),
            harness_version: Some("1.0".to_owned()),
            model: "fake-model".to_owned(),
            os: "test".to_owned(),
            architecture: "test".to_owned(),
            openreel_version: "0.1.0".to_owned(),
        };
        let jsonl = render_jsonl(&environment, &[result]).unwrap();
        let records = jsonl
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["record_type"], "environment");
        assert_eq!(records[1]["record_type"], "eval_result");
        assert_eq!(records[2]["record_type"], "totals");
        assert_eq!(records[1]["result"]["name"], "fake-eval");
    }

    #[test]
    fn utc_timestamp_conversion_covers_the_milestone_date() {
        assert_eq!(format_utc_timestamp(1_786_291_200), "2026-08-09T16:00:00Z");
    }
}
