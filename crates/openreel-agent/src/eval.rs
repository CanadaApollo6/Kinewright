//! Budgeted, typed evaluation support for installed editing-agent harnesses.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openreel_core::{
    AgentDriver, AgentEvent, Analysis, AssetId, AssetSilences, AssetTranscript, AudioLoudness,
    CaptionMotion, ClipContent, Command, Core, DeliveryConformanceReport, DeliveryProfile,
    Document, Event, Export, ExportCancellation, HarnessInfo, MediaKind, Operation, ParamValue,
    Playback, Query, QueryResult, SessionConfig, TimeCode, TimelineSceneChange,
    TimelineSilenceSpan, TimelineTranscriptWord, TitlePosition, TrackId, TranscriptStatus,
    dedup_timeline_words, delivery_conformance, document_for_delivery_profile,
    map_source_range_to_project, qa_document,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ConfirmationBroker, McpServer, compact_tool_names,
    pacing::dialogue_pacing_gaps,
    render::cuttable_timeline_silences,
    server::{ReframeSubjectProvenance, TrackedSubjectBounds, decode_reframe_subject_provenance},
    shrink_silence_span_for_cutting_with_transcript,
};

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
    /// Optional because subscription harnesses may expose token counts without
    /// exposing an attributable USD price.
    pub max_cost_usd: Option<f64>,
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
    pub deliverable: Option<EvalDeliverableSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalDeliverableSpec {
    pub profile: DeliveryProfile,
    pub focus_x_percent: u8,
    pub focus_y_percent: u8,
    pub proof_frames: u8,
    pub proof_cell_width: u32,
    pub require_audio: bool,
    pub expected_transcript_word_set: Option<&'static str>,
    pub maximum_word_error_rate_basis_points: u16,
    pub maximum_caption_word_error_rate_basis_points: Option<u16>,
    pub loudness: Option<EvalLoudnessSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EvalLoudnessSpec {
    pub minimum_integrated_lufs_hundredths: i32,
    pub maximum_integrated_lufs_hundredths: i32,
    pub maximum_sample_peak_dbfs_hundredths: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSourceClip {
    pub asset_alias: String,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedTimelineClip {
    pub asset_alias: String,
    pub timeline_start: TimeCode,
    pub timeline_end: TimeCode,
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
    MediaGapless,
    DurationBounds {
        bounds: String,
    },
    ExactSourceClips {
        clips: Vec<ExpectedSourceClip>,
    },
    ExactTrackClips {
        track: TrackId,
        clips: Vec<ExpectedTimelineClip>,
    },
    WordsRetained {
        word_set: String,
    },
    WordsAbsent {
        word_set: String,
    },
    CaptionWordsExact {
        word_set: String,
    },
    CaptionSentencesCoherent,
    CaptionPresentation {
        allowed_positions: Vec<TitlePosition>,
        color_token: u8,
        background_scrim: bool,
    },
    NoSilenceAtLeast {
        source_frames: TimeCode,
    },
    DialoguePauseBounds {
        minimum_project_frames: TimeCode,
        maximum_project_frames: TimeCode,
        capitalization_boundary_minimum_frames: TimeCode,
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
    StyledCaptions {
        minimum_cues: usize,
        motion: CaptionMotion,
    },
    CaptionSafeArea {
        profile: DeliveryProfile,
    },
    AudioPresent,
    ProgramAudioContinuous {
        track: TrackId,
        asset_alias: String,
    },
    ReframeStability {
        track: TrackId,
        minimum_keyframes_per_axis: usize,
        min_x_percent: i64,
        max_x_percent: i64,
        min_y_percent: i64,
        max_y_percent: i64,
        maximum_step_percent: i64,
    },
    QaExportReady,
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
    pub exporter: Arc<dyn Export>,
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
        T: Playback + Analysis + Export + 'static,
    {
        original_document
            .validate()
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        media.set_document(Arc::new(original_document.clone()));
        let core = Core::spawn(original_document.clone())
            .map_err(|error| EvalError::Fixture(error.to_string()))?;
        let playback: Arc<dyn Playback> = media.clone();
        let analysis: Arc<dyn Analysis> = media.clone();
        let exporter: Arc<dyn Export> = media;
        Ok(Self {
            original_document,
            core,
            playback,
            analysis,
            exporter,
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
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_output_tokens: Option<u64>,
    pub tool_surface: crate::ToolSurfaceMetrics,
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

    #[must_use]
    pub fn uncached_input_tokens(&self) -> Option<u64> {
        self.cached_input_tokens
            .map(|cached| self.input_tokens.saturating_sub(cached))
    }
}

#[derive(Debug, Clone)]
pub struct EvalOutcome {
    pub final_document: Document,
    pub final_words: Vec<String>,
    pub final_timeline_words: Vec<TimelineTranscriptWord>,
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
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_output_tokens: Option<u64>,
    pub tool_surface: crate::ToolSurfaceMetrics,
    pub cost_usd: Option<f64>,
    pub wall_time_ms: u64,
    pub operations_applied: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliverable: Option<EvalDeliverableResult>,
    pub execution_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalDeliverableResult {
    pub profile: DeliveryProfile,
    pub output_path: PathBuf,
    pub document_path: PathBuf,
    pub proof_path: PathBuf,
    pub resolution: (u32, u32),
    pub duration_frames: TimeCode,
    pub conformance: Option<DeliveryConformanceReport>,
    pub output_bytes: Option<u64>,
    pub output_sha256: Option<String>,
    pub exported_frames: Option<u64>,
    pub probed_resolution: Option<(u32, u32)>,
    pub probed_duration_frames: Option<TimeCode>,
    pub probed_media_kind: Option<MediaKind>,
    pub rendered_transcript_required: bool,
    pub rendered_transcript: Option<RenderedTranscriptVerification>,
    pub rendered_caption_alignment_required: bool,
    pub rendered_caption_alignment: Option<RenderedTranscriptVerification>,
    pub rendered_loudness_contract: Option<EvalLoudnessSpec>,
    pub rendered_loudness: Option<RenderedLoudnessVerification>,
    pub rendered_reframe: Option<RenderedReframeVerification>,
    pub proof_sample_frames: Vec<TimeCode>,
    pub machine_passed: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedLoudnessVerification {
    pub measurement: AudioLoudness,
    pub minimum_integrated_lufs_hundredths: i32,
    pub maximum_integrated_lufs_hundredths: i32,
    pub maximum_sample_peak_dbfs_hundredths: i32,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedReframeVerification {
    pub expected_animated_clips: usize,
    pub preserved_animated_clips: usize,
    pub expected_subject_provenance_clips: usize,
    pub preserved_subject_provenance_clips: usize,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedTranscriptVerification {
    pub expected_words: Vec<String>,
    pub observed_words: Vec<String>,
    pub missing_words: Vec<String>,
    pub unexpected_words: Vec<String>,
    pub edit_distance: usize,
    pub word_error_rate_basis_points: u16,
    pub maximum_word_error_rate_basis_points: u16,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanReviewFile {
    pub schema_version: u32,
    pub benchmark_id: String,
    pub run_id: String,
    pub reviewer: Option<String>,
    pub tasks: Vec<HumanTaskReview>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanTaskReview {
    pub task_id: String,
    pub artifact_sha256: Option<String>,
    pub accepted: Option<bool>,
    pub ratings: HumanRatings,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HumanRatings {
    pub story: Option<f64>,
    pub pacing: Option<f64>,
    pub visual_finish: Option<f64>,
    pub audio_finish: Option<f64>,
    pub captions: Option<f64>,
    pub delivery_readiness: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanReviewSummary {
    pub schema_version: u32,
    pub benchmark_id: String,
    pub run_id: String,
    pub reviewer: Option<String>,
    pub tasks_total: usize,
    pub tasks_reviewed: usize,
    pub tasks_pending: usize,
    pub tasks_accepted: usize,
    pub acceptance_rate: Option<f64>,
    pub overall_mean_rating: Option<f64>,
    pub mean_ratings: HumanMeanRatings,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HumanMeanRatings {
    pub story: Option<f64>,
    pub pacing: Option<f64>,
    pub visual_finish: Option<f64>,
    pub audio_finish: Option<f64>,
    pub captions: Option<f64>,
    pub delivery_readiness: Option<f64>,
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
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens: 0,
            reasoning_output_tokens: None,
            tool_surface: crate::ToolSurfaceMetrics::default(),
            cost_usd: None,
            wall_time_ms: 0,
            operations_applied: 0,
            deliverable: None,
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
    run_eval_with_artifacts(definition, driver, model, working_directory, None)
}

/// Execute one eval and, when requested by its definition, render a real
/// delivery package before restoring the original timeline.
///
/// # Errors
///
/// Returns setup or observation failures. Delivery failures are retained as
/// scored task failures so the run still produces reviewable evidence.
pub fn run_eval_with_artifacts(
    definition: &EvalDefinition,
    driver: &dyn AgentDriver,
    model: Option<&str>,
    working_directory: Option<&Path>,
    artifact_directory: Option<&Path>,
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
        tool_names: Some(compact_tool_names()),
    };
    let mut session = collect_session(
        driver,
        config,
        definition.prompts,
        &definition.budgets,
        Some(&confirmations),
        || query_operations(&fixture.core).map(|operations| operations.len()),
    )?;
    session.tool_surface = server.tool_surface_metrics();
    let final_document = query_document(&fixture.core)?;
    let operations = query_operations(&fixture.core)?;
    let final_timeline_words = dedup_timeline_words(
        fixture
            .analysis
            .timeline_transcript(&final_document, None)
            .map_err(|error| EvalError::Media(error.to_string()))?,
    );
    let final_words = final_timeline_words
        .iter()
        .map(|word| word.text.clone())
        .collect::<Vec<_>>();
    let remaining_silences = fixture
        .analysis
        .timeline_silences(&final_document, None, TimeCode(1))
        .map_err(|error| EvalError::Media(error.to_string()))?;
    let remaining_scenes = fixture
        .analysis
        .timeline_scene_changes(&final_document, None, 0)
        .map_err(|error| EvalError::Media(error.to_string()))?;
    let deliverable = definition.deliverable.map(|spec| {
        artifact_directory.map_or_else(
            || {
                failed_deliverable(
                    spec,
                    &final_document,
                    Path::new("unavailable"),
                    "the benchmark runner did not provide an artifact directory".to_owned(),
                )
            },
            |directory| {
                produce_deliverable(
                    spec,
                    &final_document,
                    fixture.analysis.as_ref(),
                    fixture.exporter.as_ref(),
                    &fixture.context,
                    directory,
                )
            },
        )
    });
    let undo_steps_to_original = restore_original(
        &fixture.core,
        &fixture.original_document,
        definition.budgets.max_undos,
    )?;
    session.wall_time_ms = duration_millis(eval_started.elapsed());
    let outcome = EvalOutcome {
        final_document: (*final_document).clone(),
        final_words,
        final_timeline_words,
        remaining_silences,
        remaining_scenes,
        context: fixture.context.clone(),
        session,
        operations,
        undo_steps_to_original,
    };
    let mut result = evaluate(definition, &outcome);
    if let Some(deliverable) = deliverable {
        result
            .assertions
            .extend(deliverable_assertions(&deliverable));
        result.passed = result.assertions.iter().all(|assertion| assertion.passed);
        result.deliverable = Some(deliverable);
    }
    server.shutdown();
    Ok(result)
}

fn produce_deliverable(
    spec: EvalDeliverableSpec,
    document: &Document,
    analysis: &dyn Analysis,
    exporter: &dyn Export,
    context: &FixtureContext,
    directory: &Path,
) -> EvalDeliverableResult {
    let mut result = deliverable_shell(spec, document, directory);
    if let Err(error) = fs::create_dir_all(directory) {
        result.errors.push(format!(
            "could not create artifact directory {}: {error}",
            directory.display()
        ));
        return finish_deliverable(result);
    }
    match serde_json::to_vec_pretty(document)
        .map_err(|error| error.to_string())
        .and_then(|json| fs::write(&result.document_path, json).map_err(|error| error.to_string()))
    {
        Ok(()) => {}
        Err(error) => result.errors.push(format!(
            "could not write final document {}: {error}",
            result.document_path.display()
        )),
    }

    let report = match delivery_conformance(
        document,
        spec.profile,
        spec.focus_x_percent,
        spec.focus_y_percent,
    ) {
        Ok(report) => report,
        Err(error) => {
            result.errors.push(format!(
                "delivery profile could not be materialized: {error}"
            ));
            return finish_deliverable(result);
        }
    };
    result.resolution = report.resolution;
    if !report.export_ready() {
        result.errors.push(format!(
            "delivery conformance reported {} blocking issue(s)",
            report
                .issues
                .iter()
                .filter(|issue| issue.severity == openreel_core::QaSeverity::Error)
                .count()
        ));
    }
    result.conformance = Some(report);

    let delivery_document = match document_for_delivery_profile(
        document,
        spec.profile,
        spec.focus_x_percent,
        spec.focus_y_percent,
    ) {
        Ok(document) => Arc::new(document),
        Err(error) => {
            result
                .errors
                .push(format!("delivery document could not be built: {error}"));
            return finish_deliverable(result);
        }
    };
    result.rendered_reframe = rendered_reframe_verification(document, &delivery_document);
    match render_proof_sheet(
        analysis,
        &delivery_document,
        spec.proof_frames,
        spec.proof_cell_width,
        &result.proof_path,
    ) {
        Ok(frames) => result.proof_sample_frames = frames,
        Err(error) => result.errors.push(error),
    }

    if result
        .conformance
        .as_ref()
        .is_some_and(DeliveryConformanceReport::export_ready)
    {
        let expected_words = rendered_transcript_expectation(spec, context, &mut result);
        export_and_probe(
            &mut result,
            spec,
            analysis,
            exporter,
            &delivery_document,
            expected_words,
        );
    }
    finish_deliverable(result)
}

/// Render and verify a saved edit decision without starting another agent turn.
///
/// This is useful when a renderer or delivery-contract fix needs to be checked
/// against the exact document an agent already produced.
#[must_use]
pub fn render_saved_deliverable(
    spec: EvalDeliverableSpec,
    document: &Document,
    analysis: &dyn Analysis,
    exporter: &dyn Export,
    directory: &Path,
) -> EvalDeliverableResult {
    produce_deliverable(
        spec,
        document,
        analysis,
        exporter,
        &FixtureContext::default(),
        directory,
    )
}

fn rendered_transcript_expectation<'a>(
    spec: EvalDeliverableSpec,
    context: &'a FixtureContext,
    result: &mut EvalDeliverableResult,
) -> Option<&'a [String]> {
    let expected = spec
        .expected_transcript_word_set
        .and_then(|word_set| context.word_sets.get(word_set).map(Vec::as_slice));
    if spec.expected_transcript_word_set.is_some() && expected.is_none() {
        result.errors.push(format!(
            "unknown rendered transcript word set {:?}",
            spec.expected_transcript_word_set
        ));
    }
    if spec.maximum_word_error_rate_basis_points > 10_000 {
        result.errors.push(format!(
            "maximum rendered transcript word error rate must be at most 10000 basis points, got {}",
            spec.maximum_word_error_rate_basis_points
        ));
    }
    if spec
        .maximum_caption_word_error_rate_basis_points
        .is_some_and(|maximum| maximum > 10_000)
    {
        result.errors.push(format!(
            "maximum rendered caption word error rate must be at most 10000 basis points, got {:?}",
            spec.maximum_caption_word_error_rate_basis_points
        ));
    }
    expected
}

fn export_and_probe(
    result: &mut EvalDeliverableResult,
    spec: EvalDeliverableSpec,
    analysis: &dyn Analysis,
    exporter: &dyn Export,
    document: &Arc<Document>,
    expected_transcript_words: Option<&[String]>,
) {
    if result.output_path.exists() {
        result.errors.push(format!(
            "refusing to overwrite existing benchmark artifact {}",
            result.output_path.display()
        ));
        return;
    }
    let settings = spec
        .profile
        .export_settings(document, ExportCancellation::default());
    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    if let Err(error) = exporter.export_document(
        Arc::clone(document),
        &result.output_path,
        settings,
        progress_tx,
    ) {
        result.errors.push(format!("export failed: {error}"));
        return;
    }
    result.exported_frames = progress_rx
        .try_iter()
        .last()
        .map(|progress| progress.completed_frames);
    let metadata = match fs::metadata(&result.output_path) {
        Ok(metadata) if metadata.len() > 0 => metadata,
        Ok(_) => {
            result
                .errors
                .push("export backend produced an empty media file".to_owned());
            return;
        }
        Err(error) => {
            result.errors.push(format!(
                "export backend returned success but {} is unavailable: {error}",
                result.output_path.display()
            ));
            return;
        }
    };
    result.output_bytes = Some(metadata.len());
    match openreel_media::sha256_file(&result.output_path) {
        Ok(hash) => result.output_sha256 = Some(hash),
        Err(error) => result.errors.push(error.to_string()),
    }
    let asset = match analysis.probe(&result.output_path) {
        Ok(asset) => asset,
        Err(error) => {
            result
                .errors
                .push(format!("export could not be probed: {error}"));
            return;
        }
    };
    result.probed_resolution = asset.resolution;
    result.probed_duration_frames = Some(asset.duration);
    result.probed_media_kind = Some(asset.kind);
    if asset.resolution != Some(result.resolution) {
        result.errors.push(format!(
            "export probe raster {:?} does not match {:?}",
            asset.resolution, result.resolution
        ));
    }
    if asset.duration.0.abs_diff(result.duration_frames.0) > 1 {
        result.errors.push(format!(
            "export probe duration {} differs from timeline {} by more than one frame",
            asset.duration.0, result.duration_frames.0
        ));
    }
    if spec.require_audio && asset.kind != MediaKind::AudioVideo {
        result.errors.push(format!(
            "export probe found {:?}; finished cut requires video with an audio stream",
            asset.kind
        ));
    }
    if let Some(contract) = spec.loudness {
        verify_rendered_loudness(result, analysis, &asset, contract);
    }
    if let Some(expected_words) = expected_transcript_words {
        verify_rendered_delivery_transcript(
            result,
            spec,
            analysis,
            &asset,
            document,
            expected_words,
        );
    }
}

fn verify_rendered_loudness(
    result: &mut EvalDeliverableResult,
    analysis: &dyn Analysis,
    asset: &openreel_core::MediaAsset,
    contract: EvalLoudnessSpec,
) {
    if contract.minimum_integrated_lufs_hundredths > contract.maximum_integrated_lufs_hundredths {
        result
            .errors
            .push("rendered loudness bounds are reversed".to_owned());
        return;
    }
    let measurement = match analysis.asset_loudness(asset) {
        Ok(measurement) => measurement,
        Err(error) => {
            result
                .errors
                .push(format!("rendered loudness measurement failed: {error}"));
            return;
        }
    };
    let passed = measurement
        .integrated_lufs_hundredths
        .is_some_and(|loudness| {
            (contract.minimum_integrated_lufs_hundredths
                ..=contract.maximum_integrated_lufs_hundredths)
                .contains(&loudness)
        })
        && measurement
            .sample_peak_dbfs_hundredths
            .is_some_and(|peak| peak <= contract.maximum_sample_peak_dbfs_hundredths);
    if !passed {
        result.errors.push(format!(
            "rendered audio violates loudness delivery: integrated_lufs_hundredths={:?}, required={}..={}; sample_peak_dbfs_hundredths={:?}, maximum={}",
            measurement.integrated_lufs_hundredths,
            contract.minimum_integrated_lufs_hundredths,
            contract.maximum_integrated_lufs_hundredths,
            measurement.sample_peak_dbfs_hundredths,
            contract.maximum_sample_peak_dbfs_hundredths,
        ));
    }
    result.rendered_loudness = Some(RenderedLoudnessVerification {
        measurement,
        minimum_integrated_lufs_hundredths: contract.minimum_integrated_lufs_hundredths,
        maximum_integrated_lufs_hundredths: contract.maximum_integrated_lufs_hundredths,
        maximum_sample_peak_dbfs_hundredths: contract.maximum_sample_peak_dbfs_hundredths,
        passed,
    });
}

fn verify_rendered_delivery_transcript(
    result: &mut EvalDeliverableResult,
    spec: EvalDeliverableSpec,
    analysis: &dyn Analysis,
    asset: &openreel_core::MediaAsset,
    document: &Document,
    expected_words: &[String],
) {
    match verify_rendered_transcript(
        analysis,
        asset,
        expected_words,
        spec.maximum_word_error_rate_basis_points,
    ) {
        Ok(verification) => {
            if !verification.passed {
                result.errors.push(format!(
                    "rendered transcript exceeds its authored-ground-truth error ceiling: wer_bp={}, maximum_bp={}, missing={:?}, unexpected={:?}",
                    verification.word_error_rate_basis_points,
                    verification.maximum_word_error_rate_basis_points,
                    verification.missing_words,
                    verification.unexpected_words
                ));
            }
            if let Some(maximum) = spec.maximum_caption_word_error_rate_basis_points {
                let caption_words = ordered_caption_words(document);
                let alignment =
                    verify_word_sequences(&verification.observed_words, &caption_words, maximum);
                if !alignment.passed {
                    result.errors.push(format!(
                        "rendered captions disagree with rendered audio: wer_bp={}, maximum_bp={}, missing={:?}, unexpected={:?}",
                        alignment.word_error_rate_basis_points,
                        alignment.maximum_word_error_rate_basis_points,
                        alignment.missing_words,
                        alignment.unexpected_words
                    ));
                }
                result.rendered_caption_alignment = Some(alignment);
            }
            result.rendered_transcript = Some(verification);
        }
        Err(error) => result.errors.push(error),
    }
}

fn verify_rendered_transcript(
    analysis: &dyn Analysis,
    asset: &openreel_core::MediaAsset,
    expected_words: &[String],
    maximum_word_error_rate_basis_points: u16,
) -> Result<RenderedTranscriptVerification, String> {
    analysis.request_transcription_with_language(asset.clone(), Some("en"));
    let deadline = Instant::now() + Duration::from_mins(20);
    let transcript = loop {
        match analysis.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => break transcript,
            TranscriptStatus::NoSpeech => {
                return Err("post-render transcription found no speech".to_owned());
            }
            TranscriptStatus::Cancelled => {
                return Err("post-render transcription was cancelled".to_owned());
            }
            TranscriptStatus::Failed(error) => {
                return Err(format!("post-render transcription failed: {error}"));
            }
            TranscriptStatus::NotRequested
            | TranscriptStatus::Queued
            | TranscriptStatus::Hashing
            | TranscriptStatus::DownloadingModel { .. }
            | TranscriptStatus::Transcribing { .. } => {}
        }
        if Instant::now() >= deadline {
            return Err("post-render transcription timed out after twenty minutes".to_owned());
        }
        thread::sleep(Duration::from_millis(100));
    };
    if normalize_word_sequence(expected_words.iter().map(String::as_str)).is_empty() {
        return Err("rendered transcript ground truth contains no words".to_owned());
    }
    let observed_words =
        normalize_word_sequence(transcript.words.iter().map(|word| word.text.as_str()));
    Ok(verify_word_sequences(
        expected_words,
        &observed_words,
        maximum_word_error_rate_basis_points,
    ))
}

fn verify_word_sequences(
    expected_words: &[String],
    observed_words: &[String],
    maximum_word_error_rate_basis_points: u16,
) -> RenderedTranscriptVerification {
    let expected_words = normalize_word_sequence(expected_words.iter().map(String::as_str));
    let observed_words = normalize_word_sequence(observed_words.iter().map(String::as_str));
    let (missing_words, unexpected_words) = word_sequence_delta(&expected_words, &observed_words);
    let edit_distance = word_sequence_edit_distance(&expected_words, &observed_words);
    let word_error_rate_basis_points =
        word_error_rate_basis_points(edit_distance, expected_words.len());
    RenderedTranscriptVerification {
        passed: word_error_rate_basis_points <= maximum_word_error_rate_basis_points,
        expected_words,
        observed_words,
        missing_words,
        unexpected_words,
        edit_distance,
        word_error_rate_basis_points,
        maximum_word_error_rate_basis_points,
    }
}

fn ordered_caption_words(document: &Document) -> Vec<String> {
    let mut captions = timeline_clips(document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => {
                Some((clip.timeline_start, clip.id, title.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    captions.sort_by_key(|(start, clip, _)| (*start, *clip));
    normalize_word_sequence(captions.into_iter().map(|(_, _, text)| text))
}

fn failed_deliverable(
    spec: EvalDeliverableSpec,
    document: &Document,
    directory: &Path,
    error: String,
) -> EvalDeliverableResult {
    let mut result = deliverable_shell(spec, document, directory);
    result.errors.push(error);
    finish_deliverable(result)
}

fn deliverable_shell(
    spec: EvalDeliverableSpec,
    document: &Document,
    directory: &Path,
) -> EvalDeliverableResult {
    EvalDeliverableResult {
        profile: spec.profile,
        output_path: directory.join(format!("finished.{}", spec.profile.container_extension())),
        document_path: directory.join("final-document.json"),
        proof_path: directory.join("proof.png"),
        resolution: spec.profile.resolution(document.resolution),
        duration_frames: document.duration,
        conformance: None,
        output_bytes: None,
        output_sha256: None,
        exported_frames: None,
        probed_resolution: None,
        probed_duration_frames: None,
        probed_media_kind: None,
        rendered_transcript_required: spec.expected_transcript_word_set.is_some(),
        rendered_transcript: None,
        rendered_caption_alignment_required: spec
            .maximum_caption_word_error_rate_basis_points
            .is_some(),
        rendered_caption_alignment: None,
        rendered_loudness_contract: spec.loudness,
        rendered_loudness: None,
        rendered_reframe: None,
        proof_sample_frames: Vec::new(),
        machine_passed: false,
        errors: Vec::new(),
    }
}

#[must_use]
pub fn human_review_template(
    benchmark_id: &str,
    run_id: &str,
    results: &[EvalResult],
) -> HumanReviewFile {
    let mut occurrences = BTreeMap::<&str, usize>::new();
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
        *occurrence += 1;
        let task_id = if *occurrence == 1 {
            base_task_id.to_owned()
        } else {
            format!("{base_task_id}-sample-{occurrence}")
        };
        tasks.push(HumanTaskReview {
            task_id,
            artifact_sha256: deliverable.output_sha256.clone(),
            accepted: None,
            ratings: HumanRatings::default(),
            notes: None,
        });
    }

    HumanReviewFile {
        schema_version: 1,
        benchmark_id: benchmark_id.to_owned(),
        run_id: run_id.to_owned(),
        reviewer: None,
        tasks,
    }
}

/// Validate a review and compute acceptance separately from machine scores.
/// Pending tasks remain pending and never count as rejected or accepted.
///
/// # Errors
///
/// Returns an error for duplicate tasks, partial reviews, invalid digests, or
/// ratings outside the inclusive 1..=5 scale or its half-point increments.
pub fn summarize_human_review(review: &HumanReviewFile) -> Result<HumanReviewSummary, EvalError> {
    if review.schema_version != 1 {
        return Err(EvalError::Output(format!(
            "unsupported human-review schema version {}",
            review.schema_version
        )));
    }
    let mut task_ids = BTreeSet::new();
    let mut reviewed = Vec::new();
    for task in &review.tasks {
        if task.task_id.trim().is_empty() || !task_ids.insert(task.task_id.as_str()) {
            return Err(EvalError::Output(format!(
                "human review contains an empty or duplicate task id {:?}",
                task.task_id
            )));
        }
        if let Some(hash) = &task.artifact_sha256
            && (hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(EvalError::Output(format!(
                "task {} has an invalid artifact sha256",
                task.task_id
            )));
        }
        let rating_values = task.ratings.values();
        let any_rating = rating_values.iter().any(Option::is_some);
        match task.accepted {
            None if any_rating => {
                return Err(EvalError::Output(format!(
                    "task {} has ratings but no acceptance decision",
                    task.task_id
                )));
            }
            None => {}
            Some(_) if rating_values.iter().any(Option::is_none) => {
                return Err(EvalError::Output(format!(
                    "task {} must fill all six ratings when accepted is set",
                    task.task_id
                )));
            }
            Some(_) => {
                if let Some(invalid) = rating_values
                    .into_iter()
                    .flatten()
                    .find(|rating| !valid_human_rating(*rating))
                {
                    return Err(EvalError::Output(format!(
                        "task {} rating {invalid} must be between 1 and 5 in 0.5 increments",
                        task.task_id
                    )));
                }
                reviewed.push(task);
            }
        }
    }
    let accepted = reviewed
        .iter()
        .filter(|task| task.accepted == Some(true))
        .count();
    let acceptance_rate = if reviewed.is_empty() {
        None
    } else {
        let accepted = u32::try_from(accepted)
            .map_err(|_| EvalError::Output("too many accepted human-review tasks".to_owned()))?;
        let reviewed = u32::try_from(reviewed.len())
            .map_err(|_| EvalError::Output("too many human-review tasks".to_owned()))?;
        Some(f64::from(accepted) / f64::from(reviewed))
    };
    Ok(HumanReviewSummary {
        schema_version: 1,
        benchmark_id: review.benchmark_id.clone(),
        run_id: review.run_id.clone(),
        reviewer: review.reviewer.clone(),
        tasks_total: review.tasks.len(),
        tasks_reviewed: reviewed.len(),
        tasks_pending: review.tasks.len().saturating_sub(reviewed.len()),
        tasks_accepted: accepted,
        acceptance_rate,
        overall_mean_rating: overall_mean_rating(&reviewed),
        mean_ratings: HumanMeanRatings {
            story: mean_rating(&reviewed, |ratings| ratings.story),
            pacing: mean_rating(&reviewed, |ratings| ratings.pacing),
            visual_finish: mean_rating(&reviewed, |ratings| ratings.visual_finish),
            audio_finish: mean_rating(&reviewed, |ratings| ratings.audio_finish),
            captions: mean_rating(&reviewed, |ratings| ratings.captions),
            delivery_readiness: mean_rating(&reviewed, |ratings| ratings.delivery_readiness),
        },
    })
}

impl HumanRatings {
    fn values(&self) -> [Option<f64>; 6] {
        [
            self.story,
            self.pacing,
            self.visual_finish,
            self.audio_finish,
            self.captions,
            self.delivery_readiness,
        ]
    }
}

fn valid_human_rating(rating: f64) -> bool {
    rating.is_finite()
        && (1.0..=5.0).contains(&rating)
        && (rating * 2.0).fract().abs() <= f64::EPSILON
}

fn mean_rating(
    reviewed: &[&HumanTaskReview],
    select: impl Fn(&HumanRatings) -> Option<f64>,
) -> Option<f64> {
    if reviewed.is_empty() {
        return None;
    }
    let sum = reviewed
        .iter()
        .filter_map(|task| select(&task.ratings))
        .sum::<f64>();
    let count = u32::try_from(reviewed.len()).ok()?;
    Some(sum / f64::from(count))
}

fn overall_mean_rating(reviewed: &[&HumanTaskReview]) -> Option<f64> {
    if reviewed.is_empty() {
        return None;
    }
    let sum = reviewed
        .iter()
        .flat_map(|task| task.ratings.values())
        .flatten()
        .sum::<f64>();
    let rating_count = reviewed.len().checked_mul(6)?;
    let rating_count = u32::try_from(rating_count).ok()?;
    Some(sum / f64::from(rating_count))
}

fn finish_deliverable(mut result: EvalDeliverableResult) -> EvalDeliverableResult {
    result.machine_passed = result.errors.is_empty()
        && result
            .conformance
            .as_ref()
            .is_some_and(DeliveryConformanceReport::export_ready)
        && result.output_bytes.is_some_and(|bytes| bytes > 0)
        && result
            .output_sha256
            .as_ref()
            .is_some_and(|hash| hash.len() == 64)
        && result.probed_resolution == Some(result.resolution)
        && result
            .probed_duration_frames
            .is_some_and(|duration| duration.0.abs_diff(result.duration_frames.0) <= 1)
        && result.probed_media_kind.is_some()
        && (!result.rendered_transcript_required
            || result
                .rendered_transcript
                .as_ref()
                .is_some_and(|verification| verification.passed))
        && (!result.rendered_caption_alignment_required
            || result
                .rendered_caption_alignment
                .as_ref()
                .is_some_and(|verification| verification.passed))
        && (result.rendered_loudness_contract.is_none()
            || result
                .rendered_loudness
                .as_ref()
                .is_some_and(|verification| verification.passed))
        && result
            .rendered_reframe
            .as_ref()
            .is_none_or(|verification| verification.passed)
        && !result.proof_sample_frames.is_empty()
        && result.proof_path.is_file()
        && result.document_path.is_file();
    result
}

fn deliverable_assertions(result: &EvalDeliverableResult) -> Vec<AssertionResult> {
    let conformance_ready = result
        .conformance
        .as_ref()
        .is_some_and(DeliveryConformanceReport::export_ready);
    let mut assertions = vec![
        assertion_result(
            "delivery conformance",
            conformance_ready,
            format!(
                "profile={}, resolution={}x{}, ready={conformance_ready}",
                result.profile.as_str(),
                result.resolution.0,
                result.resolution.1
            ),
        ),
        assertion_result(
            "rendered proof",
            result.proof_path.is_file() && !result.proof_sample_frames.is_empty(),
            format!(
                "path={}, sampled_frames={:?}",
                result.proof_path.display(),
                result.proof_sample_frames
            ),
        ),
        assertion_result(
            "finished media artifact",
            result.machine_passed,
            format!(
                "path={}, bytes={:?}, sha256={:?}, exported_frames={:?}, probed_resolution={:?}, probed_duration={:?}, probed_kind={:?}, errors={:?}",
                result.output_path.display(),
                result.output_bytes,
                result.output_sha256,
                result.exported_frames,
                result.probed_resolution,
                result.probed_duration_frames,
                result.probed_media_kind,
                result.errors
            ),
        ),
    ];
    if result.rendered_transcript_required {
        assertions.push(match &result.rendered_transcript {
            Some(verification) => assertion_result(
                "rendered dialogue accuracy",
                verification.passed,
                format!(
                    "expected={:?}, observed={:?}, edit_distance={}, wer_bp={}, maximum_bp={}, missing={:?}, unexpected={:?}",
                    verification.expected_words,
                    verification.observed_words,
                    verification.edit_distance,
                    verification.word_error_rate_basis_points,
                    verification.maximum_word_error_rate_basis_points,
                    verification.missing_words,
                    verification.unexpected_words
                ),
            ),
            None => assertion_result(
                "rendered dialogue accuracy",
                false,
                format!(
                    "required post-render transcription is unavailable; errors={:?}",
                    result.errors
                ),
            ),
        });
    }
    if result.rendered_caption_alignment_required {
        assertions.push(match &result.rendered_caption_alignment {
            Some(verification) => assertion_result(
                "rendered caption/audio agreement",
                verification.passed,
                format!(
                    "audio={:?}, captions={:?}, edit_distance={}, wer_bp={}, maximum_bp={}, missing_from_captions={:?}, unexpected_in_captions={:?}",
                    verification.expected_words,
                    verification.observed_words,
                    verification.edit_distance,
                    verification.word_error_rate_basis_points,
                    verification.maximum_word_error_rate_basis_points,
                    verification.missing_words,
                    verification.unexpected_words
                ),
            ),
            None => assertion_result(
                "rendered caption/audio agreement",
                false,
                format!(
                    "required rendered caption/audio comparison is unavailable; errors={:?}",
                    result.errors
                ),
            ),
        });
    }
    if result.rendered_loudness_contract.is_some() {
        assertions.push(rendered_loudness_assertion(result));
    }
    if result.rendered_reframe.is_some() {
        assertions.push(rendered_reframe_assertion(result));
    }
    assertions
}

fn rendered_reframe_assertion(result: &EvalDeliverableResult) -> AssertionResult {
    let Some(verification) = &result.rendered_reframe else {
        return assertion_result(
            "rendered reframe automation",
            false,
            "animated reframe verification is unavailable".to_owned(),
        );
    };
    assertion_result(
        "rendered reframe automation",
        verification.passed,
        format!(
            "preserved {} of {} same-aspect animated reframe clips and {} of {} tracked-subject provenance sidecars",
            verification.preserved_animated_clips,
            verification.expected_animated_clips,
            verification.preserved_subject_provenance_clips,
            verification.expected_subject_provenance_clips,
        ),
    )
}

fn rendered_reframe_verification(
    source: &Document,
    delivered: &Document,
) -> Option<RenderedReframeVerification> {
    let (width, height) = delivered.resolution;
    let aspect_basis_points = i64::from(width)
        .saturating_mul(10_000)
        .saturating_add(i64::from(height) / 2)
        / i64::from(height.max(1));
    let expected = source
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| {
            clip.effects.iter().filter_map(move |effect| {
                (effect.name == "reframe"
                    && !effect.keyframes.is_empty()
                    && effect.parameters.get("target_aspect_basis_points")
                        == Some(&ParamValue::Integer(aspect_basis_points)))
                .then_some((clip.id, effect))
            })
        })
        .collect::<Vec<_>>();
    if expected.is_empty() {
        return None;
    }
    let source_provenance = valid_reframe_subject_provenances(source);
    let delivered_provenance = valid_reframe_subject_provenances(delivered);
    let preserved = expected
        .iter()
        .filter(|(clip_id, effect)| {
            delivered
                .tracks
                .iter()
                .flat_map(|track| &track.clips)
                .find(|clip| clip.id == *clip_id)
                .is_some_and(|clip| clip.effects.contains(effect))
        })
        .count();
    let expected_subject_provenance = expected
        .iter()
        .filter_map(|(clip_id, effect)| {
            source_provenance
                .iter()
                .find(|provenance| provenance.clip == *clip_id && provenance.effect == effect.id)
        })
        .collect::<Vec<_>>();
    let preserved_subject_provenance = expected_subject_provenance
        .iter()
        .filter(|provenance| delivered_provenance.contains(provenance))
        .count();
    Some(RenderedReframeVerification {
        expected_animated_clips: expected.len(),
        preserved_animated_clips: preserved,
        expected_subject_provenance_clips: expected_subject_provenance.len(),
        preserved_subject_provenance_clips: preserved_subject_provenance,
        passed: preserved == expected.len()
            && preserved_subject_provenance == expected_subject_provenance.len(),
    })
}

fn rendered_loudness_assertion(result: &EvalDeliverableResult) -> AssertionResult {
    match &result.rendered_loudness {
        Some(verification) => assertion_result(
            "rendered audio loudness",
            verification.passed,
            format!(
                "integrated_lufs_hundredths={:?}, required={}..={}; sample_peak_dbfs_hundredths={:?}, maximum={}",
                verification.measurement.integrated_lufs_hundredths,
                verification.minimum_integrated_lufs_hundredths,
                verification.maximum_integrated_lufs_hundredths,
                verification.measurement.sample_peak_dbfs_hundredths,
                verification.maximum_sample_peak_dbfs_hundredths,
            ),
        ),
        None => assertion_result(
            "rendered audio loudness",
            false,
            format!(
                "required rendered loudness measurement is unavailable; errors={:?}",
                result.errors
            ),
        ),
    }
}

fn render_proof_sheet(
    analysis: &dyn Analysis,
    document: &Arc<Document>,
    requested_frames: u8,
    requested_width: u32,
    output: &Path,
) -> Result<Vec<TimeCode>, String> {
    if document.duration <= TimeCode::ZERO {
        return Err("cannot render proof frames for an empty timeline".to_owned());
    }
    let count = usize::from(requested_frames.clamp(1, 16));
    let cell_width = requested_width.clamp(64, 512);
    let frames = uniform_sample_frames(document.duration, count);
    let mut cells = Vec::with_capacity(frames.len());
    for frame in &frames {
        let image = analysis
            .thumbnail_for_document(Arc::clone(document), *frame, cell_width)
            .map_err(|error| format!("proof frame {} failed: {error}", frame.0))?;
        let image = image::RgbaImage::from_raw(image.width, image.height, image.pixels)
            .ok_or_else(|| format!("proof frame {} returned truncated RGBA data", frame.0))?;
        cells.push(image);
    }
    let mut columns = 1_usize;
    while columns.saturating_mul(columns) < cells.len() {
        columns = columns.saturating_add(1);
    }
    let rows = cells.len().div_ceil(columns);
    let gutter = 4_u32;
    let width = cells
        .iter()
        .map(image::GenericImageView::width)
        .max()
        .unwrap_or(cell_width);
    let height = cells
        .iter()
        .map(image::GenericImageView::height)
        .max()
        .unwrap_or(1);
    let sheet_width = u32::try_from(columns)
        .unwrap_or(u32::MAX)
        .saturating_mul(width.saturating_add(gutter))
        .saturating_sub(gutter);
    let sheet_height = u32::try_from(rows)
        .unwrap_or(u32::MAX)
        .saturating_mul(height.saturating_add(gutter))
        .saturating_sub(gutter);
    let mut sheet =
        image::RgbaImage::from_pixel(sheet_width, sheet_height, image::Rgba([18, 20, 24, 255]));
    for (index, cell) in cells.iter().enumerate() {
        let column = u32::try_from(index % columns).unwrap_or(u32::MAX);
        let row = u32::try_from(index / columns).unwrap_or(u32::MAX);
        image::imageops::replace(
            &mut sheet,
            cell,
            i64::from(column.saturating_mul(width.saturating_add(gutter))),
            i64::from(row.saturating_mul(height.saturating_add(gutter))),
        );
    }
    sheet
        .save_with_format(output, image::ImageFormat::Png)
        .map_err(|error| format!("could not write proof sheet {}: {error}", output.display()))?;
    Ok(frames)
}

fn uniform_sample_frames(duration: TimeCode, count: usize) -> Vec<TimeCode> {
    let last = duration.0.saturating_sub(1).max(0);
    if count <= 1 {
        return vec![TimeCode(last / 2)];
    }
    let denominator = i64::try_from(count.saturating_sub(1)).unwrap_or(i64::MAX);
    (0..count)
        .map(|index| {
            let index = i64::try_from(index).unwrap_or(i64::MAX);
            TimeCode(last.saturating_mul(index).saturating_div(denominator))
        })
        .collect()
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
        cached_input_tokens: Some(0),
        cache_creation_input_tokens: Some(0),
        reasoning_output_tokens: Some(0),
        ..SessionMetrics::default()
    };
    let mut cost_is_complete = true;
    let mut saw_usage = false;
    let trace_events = std::env::var_os("OPENREEL_EVAL_TRACE").is_some();
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
                Ok(AgentEvent::ToolCall { name, arguments }) => {
                    if trace_events {
                        eprintln!("EVAL TRACE tool_call {name}: {}", bounded_trace(&arguments));
                    }
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
                    cached_input_tokens,
                    cache_creation_input_tokens,
                    output_tokens,
                    reasoning_output_tokens,
                    cost_usd,
                }) => {
                    saw_usage = true;
                    metrics.input_tokens = metrics.input_tokens.saturating_add(input_tokens);
                    accumulate_optional_tokens(
                        &mut metrics.cached_input_tokens,
                        cached_input_tokens,
                    );
                    accumulate_optional_tokens(
                        &mut metrics.cache_creation_input_tokens,
                        cache_creation_input_tokens,
                    );
                    metrics.output_tokens = metrics.output_tokens.saturating_add(output_tokens);
                    accumulate_optional_tokens(
                        &mut metrics.reasoning_output_tokens,
                        reasoning_output_tokens,
                    );
                    match cost_usd {
                        Some(cost) if cost_is_complete => {
                            let total = metrics.cost_usd.get_or_insert(0.0);
                            *total += cost;
                            if budgets.max_cost_usd.is_some_and(|maximum| *total > maximum) {
                                metrics.errors.push(format!(
                                    "cost ceiling exceeded (${total:.4} > ${:.2})",
                                    budgets.max_cost_usd.unwrap_or_default()
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
                Ok(AgentEvent::Text(text)) => {
                    if trace_events {
                        eprintln!("EVAL TRACE agent_text: {}", bounded_trace(&text));
                    }
                }
                Ok(AgentEvent::ToolResult { name, result }) => {
                    if trace_events {
                        eprintln!("EVAL TRACE tool_result {name}: {}", bounded_trace(&result));
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
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
    if !saw_usage {
        metrics.cached_input_tokens = None;
        metrics.cache_creation_input_tokens = None;
        metrics.reasoning_output_tokens = None;
    }
    metrics.wall_time_ms = duration_millis(started.elapsed());
    session.interrupt();
    Ok(metrics)
}

fn bounded_trace(value: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let mut characters = value.chars();
    let bounded = characters.by_ref().take(MAX_CHARS).collect::<String>();
    if characters.next().is_some() {
        format!("{bounded}...[truncated]")
    } else {
        bounded
    }
}

fn accumulate_optional_tokens(total: &mut Option<u64>, reported: Option<u64>) {
    match (total.as_mut(), reported) {
        (Some(total), Some(reported)) => *total = total.saturating_add(reported),
        (_, None) => *total = None,
        (None, Some(_)) => {}
    }
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
        cached_input_tokens: outcome.session.cached_input_tokens,
        cache_creation_input_tokens: outcome.session.cache_creation_input_tokens,
        output_tokens: outcome.session.output_tokens,
        reasoning_output_tokens: outcome.session.reasoning_output_tokens,
        tool_surface: outcome.session.tool_surface,
        cost_usd: outcome.session.cost_usd,
        wall_time_ms: outcome.session.wall_time_ms,
        operations_applied: u32::try_from(outcome.operations.len()).unwrap_or(u32::MAX),
        deliverable: None,
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
            let present =
                timeline_media_clips(&outcome.final_document).any(|clip| clip.asset == *asset);
            assertion_result(
                "asset absent",
                !present,
                format!("asset {alias} ({asset}) present={present}"),
            )
        }
        EvalAssertion::Gapless => evaluate_gapless(&outcome.final_document),
        EvalAssertion::MediaGapless => evaluate_media_gapless(&outcome.final_document),
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
        EvalAssertion::ExactTrackClips { track, clips } => {
            evaluate_track_clips(*track, clips, outcome)
        }
        EvalAssertion::WordsRetained { word_set } => evaluate_word_set(word_set, outcome, true),
        EvalAssertion::WordsAbsent { word_set } => evaluate_word_set(word_set, outcome, false),
        EvalAssertion::CaptionWordsExact { word_set } => evaluate_caption_words(word_set, outcome),
        EvalAssertion::CaptionSentencesCoherent => evaluate_caption_sentences(outcome),
        EvalAssertion::CaptionPresentation {
            allowed_positions,
            color_token,
            background_scrim,
        } => evaluate_caption_presentation(
            allowed_positions,
            *color_token,
            *background_scrim,
            outcome,
        ),
        EvalAssertion::NoSilenceAtLeast { source_frames } => {
            let remaining = cuttable_timeline_silences(
                &outcome.final_document,
                &outcome.remaining_silences,
                &outcome.context.transcripts,
                *source_frames,
            )
            .len();
            assertion_result(
                "long silence absent",
                remaining == 0,
                format!(
                    "observed {remaining} transcript-safe cuttable silence spans at least {} source frames",
                    source_frames.0
                ),
            )
        }
        EvalAssertion::DialoguePauseBounds {
            minimum_project_frames,
            maximum_project_frames,
            capitalization_boundary_minimum_frames,
        } => evaluate_dialogue_pause_bounds(
            &outcome.final_timeline_words,
            &outcome.remaining_silences,
            *minimum_project_frames,
            *maximum_project_frames,
            *capitalization_boundary_minimum_frames,
        ),
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
        EvalAssertion::StyledCaptions {
            minimum_cues,
            motion,
        } => evaluate_styled_captions(*minimum_cues, *motion, outcome),
        EvalAssertion::CaptionSafeArea { profile } => evaluate_caption_safe_area(*profile, outcome),
        EvalAssertion::AudioPresent => evaluate_audio_present(&outcome.final_document),
        EvalAssertion::ProgramAudioContinuous { track, asset_alias } => {
            evaluate_program_audio_continuous(*track, asset_alias, outcome)
        }
        EvalAssertion::ReframeStability {
            track,
            minimum_keyframes_per_axis,
            min_x_percent,
            max_x_percent,
            min_y_percent,
            max_y_percent,
            maximum_step_percent,
        } => evaluate_reframe_stability(
            *track,
            *minimum_keyframes_per_axis,
            *min_x_percent..=*max_x_percent,
            *min_y_percent..=*max_y_percent,
            *maximum_step_percent,
            outcome,
        ),
        EvalAssertion::QaExportReady => {
            let report = qa_document(&outcome.final_document);
            assertion_result(
                "technical QA",
                report.export_ready(),
                format!(
                    "errors={}, warnings={}, info={}",
                    report.count(openreel_core::QaSeverity::Error),
                    report.count(openreel_core::QaSeverity::Warning),
                    report.count(openreel_core::QaSeverity::Info)
                ),
            )
        }
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
    let (cost_passed, cost_detail) = match (cost, budgets.max_cost_usd) {
        (Some(value), Some(maximum)) => {
            (value <= maximum, format!("${value:.4} <= ${maximum:.2}"))
        }
        (None, Some(_)) => (false, "harness did not report USD cost".to_owned()),
        (Some(value), None) => (
            true,
            format!("${value:.4} reported; no portable USD ceiling is enforced"),
        ),
        (None, None) => (
            true,
            "subscription harness does not expose attributable USD cost; token and wall-time ceilings remain enforced".to_owned(),
        ),
    };
    results.push(assertion_result("cost ceiling", cost_passed, cost_detail));
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
    let mut observed = timeline_media_clips(&outcome.final_document)
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

fn evaluate_media_gapless(document: &Document) -> AssertionResult {
    let mut errors = Vec::new();
    for track in &document.tracks {
        let clips = track
            .clips
            .iter()
            .filter(|clip| {
                matches!(
                    &clip.content,
                    ClipContent::Media | ClipContent::Freeze { .. }
                )
            })
            .collect::<Vec<_>>();
        if let Some(first) = clips.first()
            && first.timeline_start != TimeCode::ZERO
        {
            errors.push(format!(
                "track {} media starts at frame {}",
                track.id, first.timeline_start.0
            ));
        }
        for adjacent in clips.windows(2) {
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
                    "track {} media gap/overlap between clips {} and {}: {} then {}",
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
        "primary media gapless",
        errors.is_empty(),
        if errors.is_empty() {
            "all populated media tracks start at zero and are contiguous; caption gaps are allowed"
                .to_owned()
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
    let observed = timeline_media_clips(&outcome.final_document)
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

fn evaluate_track_clips(
    track_id: TrackId,
    clips: &[ExpectedTimelineClip],
    outcome: &EvalOutcome,
) -> AssertionResult {
    let reverse = outcome
        .context
        .asset_aliases
        .iter()
        .map(|(alias, asset)| (*asset, alias.as_str()))
        .collect::<BTreeMap<_, _>>();
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "exact track clips",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let observed = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .map(|clip| {
            let timeline_end = outcome
                .final_document
                .asset(clip.asset)
                .and_then(|asset| {
                    map_source_range_to_project(
                        clip.source_range.clone(),
                        asset.fps,
                        outcome.final_document.fps,
                    )
                    .ok()
                })
                .and_then(|duration| clip.timeline_start.checked_add(duration))
                .unwrap_or(TimeCode(i64::MIN));
            ExpectedTimelineClip {
                asset_alias: reverse.get(&clip.asset).map_or_else(
                    || format!("asset-{}", clip.asset.0),
                    |alias| (*alias).to_owned(),
                ),
                timeline_start: clip.timeline_start,
                timeline_end,
                source_start: clip.source_range.start,
                source_end: clip.source_range.end,
            }
        })
        .collect::<Vec<_>>();
    assertion_result(
        "exact track clips",
        observed == clips,
        format!("track={track_id}, expected={clips:?}, observed={observed:?}"),
    )
}

#[allow(clippy::too_many_lines)]
fn evaluate_reframe_stability(
    track_id: TrackId,
    minimum_keyframes_per_axis: usize,
    x_bounds: std::ops::RangeInclusive<i64>,
    y_bounds: std::ops::RangeInclusive<i64>,
    maximum_step_percent: i64,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "reframe stability",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let (provenances, mut errors) = reframe_subject_provenances(&outcome.final_document);
    let media_clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    for clip in &media_clips {
        let reframes = clip
            .effects
            .iter()
            .filter(|effect| effect.name == "reframe")
            .collect::<Vec<_>>();
        if reframes.len() != 1 {
            errors.push(format!(
                "clip {} has {} reframe effects",
                clip.id,
                reframes.len()
            ));
            continue;
        }
        let effect = reframes[0];
        for (axis, percent_name, basis_points_name, bounds) in [
            (
                "focus_x",
                "focus_x_percent",
                "focus_x_basis_points",
                &x_bounds,
            ),
            (
                "focus_y",
                "focus_y_percent",
                "focus_y_basis_points",
                &y_bounds,
            ),
        ] {
            let Some((curve, units)) = reframe_focus_curve(effect, percent_name, basis_points_name)
            else {
                errors.push(format!("clip {} has no {axis} curve", clip.id));
                continue;
            };
            if curve.keyframes.len() < minimum_keyframes_per_axis {
                errors.push(format!(
                    "clip {} {axis} has {} keyframes, expected at least {minimum_keyframes_per_axis}",
                    clip.id,
                    curve.keyframes.len()
                ));
            }
            if curve.keyframes.iter().any(|keyframe| {
                keyframe.interpolation != openreel_core::KeyframeInterpolation::Linear
            }) {
                errors.push(format!(
                    "clip {} {axis} is not linearly interpolated",
                    clip.id
                ));
            }
            for keyframe in &curve.keyframes {
                if !units.contains(bounds, keyframe.value) {
                    errors.push(format!(
                        "clip {} {axis} value {} is outside {}",
                        clip.id,
                        keyframe.value,
                        units.render_bounds(bounds),
                    ));
                }
            }
            for pair in curve.keyframes.windows(2) {
                let step = pair[0].value.abs_diff(pair[1].value);
                if step > units.step_limit(maximum_step_percent) {
                    errors.push(format!(
                        "clip {} {axis} jumps {} between frames {} and {}",
                        clip.id,
                        units.render_value(step),
                        pair[0].at.0,
                        pair[1].at.0
                    ));
                }
            }
        }
        let matching_provenance = provenances
            .iter()
            .filter(|provenance| provenance.clip == clip.id && provenance.effect == effect.id)
            .collect::<Vec<_>>();
        if matching_provenance.len() != 1 {
            errors.push(format!(
                "clip {} has {} tracked-subject provenance sidecars for reframe effect {}",
                clip.id,
                matching_provenance.len(),
                effect.id,
            ));
            continue;
        }
        errors.extend(evaluate_tracked_subject_containment(
            &outcome.final_document,
            clip,
            effect,
            matching_provenance[0],
        ));
    }
    assertion_result(
        "reframe stability",
        !media_clips.is_empty() && errors.is_empty(),
        if errors.is_empty() {
            format!(
                "track {track_id} has {} bounded, speed-limited, linearly interpolated reframes that contain their tracked subjects",
                media_clips.len()
            )
        } else {
            errors.join("; ")
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReframeFocusUnits {
    Percent,
    BasisPoints,
}

impl ReframeFocusUnits {
    fn contains(self, bounds: &std::ops::RangeInclusive<i64>, value: i64) -> bool {
        match self {
            Self::Percent => bounds.contains(&value),
            Self::BasisPoints => {
                let minimum = bounds.start().saturating_mul(100);
                let maximum = bounds.end().saturating_mul(100);
                (minimum..=maximum).contains(&value)
            }
        }
    }

    fn render_bounds(self, bounds: &std::ops::RangeInclusive<i64>) -> String {
        match self {
            Self::Percent => format!("{}..={} percent", bounds.start(), bounds.end()),
            Self::BasisPoints => format!(
                "{}..={} basis points",
                bounds.start().saturating_mul(100),
                bounds.end().saturating_mul(100),
            ),
        }
    }

    fn step_limit(self, percent: i64) -> u64 {
        let limit = match self {
            Self::Percent => percent,
            Self::BasisPoints => percent.saturating_mul(100),
        };
        u64::try_from(limit).unwrap_or_default()
    }

    fn render_value(self, value: u64) -> String {
        match self {
            Self::Percent => format!("{value} percent"),
            Self::BasisPoints => format!("{value} basis points"),
        }
    }
}

fn reframe_focus_curve<'a>(
    effect: &'a openreel_core::Effect,
    percent_name: &str,
    basis_points_name: &str,
) -> Option<(&'a openreel_core::AutomationCurve, ReframeFocusUnits)> {
    effect
        .keyframes
        .get(basis_points_name)
        .map(|curve| (curve, ReframeFocusUnits::BasisPoints))
        .or_else(|| {
            effect
                .keyframes
                .get(percent_name)
                .map(|curve| (curve, ReframeFocusUnits::Percent))
        })
}

fn reframe_focus_at_basis_points(
    effect: &openreel_core::Effect,
    percent_name: &str,
    basis_points_name: &str,
    at: TimeCode,
) -> Option<i64> {
    effect
        .integer_parameter_at(basis_points_name, at)
        .or_else(|| {
            effect
                .integer_parameter_at(percent_name, at)
                .map(|percent| percent.saturating_mul(100))
        })
}

fn reframe_subject_provenances(
    document: &Document,
) -> (Vec<ReframeSubjectProvenance>, Vec<String>) {
    let mut provenances = Vec::new();
    let mut errors = Vec::new();
    for marker in &document.markers {
        match decode_reframe_subject_provenance(&marker.label) {
            Ok(Some(provenance)) => provenances.push(provenance),
            Ok(None) => {}
            Err(error) => errors.push(format!(
                "tracked-subject provenance marker {} is malformed: {error}",
                marker.id.0
            )),
        }
    }
    (provenances, errors)
}

fn valid_reframe_subject_provenances(document: &Document) -> Vec<ReframeSubjectProvenance> {
    reframe_subject_provenances(document).0
}

// Template matching follows a supplied search box, not a segmented face edge.
// Provenance bounds round outward and crop bounds round outward, so strict
// containment is both deterministic and conservative.
const SUBJECT_CONTAINMENT_TOLERANCE_BASIS_POINTS: i64 = 0;
const SUBJECT_CONTAINMENT_ENDPOINT_WINDOW_FRAMES: i64 = 25;

fn evaluate_tracked_subject_containment(
    document: &Document,
    clip: &openreel_core::Clip,
    effect: &openreel_core::Effect,
    provenance: &ReframeSubjectProvenance,
) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(asset) = document.asset(clip.asset) else {
        return vec![format!("clip {} has no source asset", clip.id)];
    };
    let Some((source_width, source_height)) = asset.resolution else {
        return vec![format!(
            "clip {} source asset {} has no resolution for reframe containment",
            clip.id, asset.id
        )];
    };
    if source_width == 0 || source_height == 0 {
        return vec![format!(
            "clip {} source asset {} has an invalid {}x{} resolution",
            clip.id, asset.id, source_width, source_height
        )];
    }
    let duration = match document.clip_duration(clip) {
        Ok(duration) => duration,
        Err(error) => {
            return vec![format!(
                "clip {} duration is unavailable for reframe containment: {error}",
                clip.id
            )];
        }
    };
    let trailing_start = TimeCode(
        duration
            .0
            .saturating_sub(SUBJECT_CONTAINMENT_ENDPOINT_WINDOW_FRAMES),
    );
    let mut trailing_samples = 0_usize;
    for sample in &provenance.samples {
        if sample.at >= duration {
            errors.push(format!(
                "clip {} tracked-subject sample at frame {} is outside duration {}",
                clip.id, sample.at.0, duration.0
            ));
            continue;
        }
        if sample.at >= trailing_start {
            trailing_samples = trailing_samples.saturating_add(1);
        }
        let crop = match reframe_crop_bounds_basis_points(
            effect,
            source_width,
            source_height,
            sample.at,
        ) {
            Ok(crop) => crop,
            Err(error) => {
                errors.push(format!(
                    "clip {} frame {} cannot resolve reframe crop: {error}",
                    clip.id, sample.at.0
                ));
                continue;
            }
        };
        if !crop.contains(sample, SUBJECT_CONTAINMENT_TOLERANCE_BASIS_POINTS) {
            errors.push(format!(
                "clip {} frame {} crop {}..={} x {}..={} basis points does not contain tracked subject {}..={} x {}..={} basis points",
                clip.id,
                sample.at.0,
                crop.left,
                crop.right,
                crop.top,
                crop.bottom,
                sample.left_basis_points,
                sample.right_basis_points,
                sample.top_basis_points,
                sample.bottom_basis_points,
            ));
        }
    }
    if trailing_samples == 0 {
        errors.push(format!(
            "clip {} has no tracked-subject sample in its final {} frames",
            clip.id, SUBJECT_CONTAINMENT_ENDPOINT_WINDOW_FRAMES
        ));
    }
    errors
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReframeCropBounds {
    left: i64,
    right: i64,
    top: i64,
    bottom: i64,
}

impl ReframeCropBounds {
    fn contains(self, subject: &TrackedSubjectBounds, tolerance: i64) -> bool {
        self.left <= i64::from(subject.left_basis_points).saturating_add(tolerance)
            && self.right >= i64::from(subject.right_basis_points).saturating_sub(tolerance)
            && self.top <= i64::from(subject.top_basis_points).saturating_add(tolerance)
            && self.bottom >= i64::from(subject.bottom_basis_points).saturating_sub(tolerance)
    }
}

fn reframe_crop_bounds_basis_points(
    effect: &openreel_core::Effect,
    source_width: u32,
    source_height: u32,
    at: TimeCode,
) -> Result<ReframeCropBounds, String> {
    let target_aspect_basis_points = effect
        .integer_parameter_at("target_aspect_basis_points", at)
        .ok_or_else(|| "missing target_aspect_basis_points".to_owned())?;
    if target_aspect_basis_points <= 0 {
        return Err(format!(
            "target_aspect_basis_points must be positive, found {target_aspect_basis_points}"
        ));
    }
    let focus_x =
        reframe_focus_at_basis_points(effect, "focus_x_percent", "focus_x_basis_points", at)
            .ok_or_else(|| "missing horizontal focus".to_owned())?
            .clamp(0, 10_000);
    let focus_y =
        reframe_focus_at_basis_points(effect, "focus_y_percent", "focus_y_basis_points", at)
            .ok_or_else(|| "missing vertical focus".to_owned())?
            .clamp(0, 10_000);
    let source_width = i128::from(source_width);
    let source_height = i128::from(source_height);
    let target_aspect = i128::from(target_aspect_basis_points);
    let source_is_wider =
        source_width.saturating_mul(10_000) > source_height.saturating_mul(target_aspect);
    let source_is_taller =
        source_width.saturating_mul(10_000) < source_height.saturating_mul(target_aspect);
    let (visible_width, visible_height) = if source_is_wider {
        (
            i64::try_from(ceil_div_positive(
                target_aspect.saturating_mul(source_height),
                source_width,
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
            10_000,
        )
    } else if source_is_taller {
        (
            10_000,
            i64::try_from(ceil_div_positive(
                source_width.saturating_mul(100_000_000),
                source_height.saturating_mul(target_aspect),
            ))
            .unwrap_or(10_000)
            .clamp(1, 10_000),
        )
    } else {
        (10_000, 10_000)
    };
    let (left, right) = crop_axis(focus_x, visible_width);
    let (top, bottom) = crop_axis(focus_y, visible_height);
    Ok(ReframeCropBounds {
        left,
        right,
        top,
        bottom,
    })
}

fn ceil_div_positive(numerator: i128, denominator: i128) -> i128 {
    numerator
        .saturating_add(denominator.saturating_sub(1))
        .checked_div(denominator.max(1))
        .unwrap_or_default()
}

fn crop_axis(focus_basis_points: i64, visible_basis_points: i64) -> (i64, i64) {
    let visible_basis_points = visible_basis_points.clamp(1, 10_000);
    let maximum_left = 10_000_i64.saturating_sub(visible_basis_points);
    let left = focus_basis_points
        .saturating_sub(visible_basis_points / 2)
        .clamp(0, maximum_left);
    (left, left.saturating_add(visible_basis_points))
}

fn evaluate_program_audio_continuous(
    track_id: TrackId,
    asset_alias: &str,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let Some(asset) = outcome.context.asset_aliases.get(asset_alias) else {
        return assertion_result(
            "program audio continuous",
            false,
            format!("unknown asset alias {asset_alias:?}"),
        );
    };
    let Some(track) = outcome
        .final_document
        .tracks
        .iter()
        .find(|track| track.id == track_id)
    else {
        return assertion_result(
            "program audio continuous",
            false,
            format!("track {track_id} does not exist"),
        );
    };
    let media_clips = track
        .clips
        .iter()
        .filter(|clip| clip.content.is_media())
        .collect::<Vec<_>>();
    let passed = track.kind == openreel_core::TrackKind::Audio
        && media_clips.len() == 1
        && media_clips[0].asset == *asset
        && media_clips[0].audio_gain_tenth_db == 0
        && media_clips[0].audio_fade_in_frames == TimeCode::ZERO
        && media_clips[0].audio_fade_out_frames == TimeCode::ZERO
        && media_clips[0].speed_percent == 100
        && media_clips[0].effects.is_empty()
        && media_clips[0].transition_in.is_none();
    assertion_result(
        "program audio continuous",
        passed,
        format!(
            "track={track_id}, kind={:?}, clips={}, asset={}, expected_asset={}, gain_tenth_db={:?}, fades={:?}, speed={:?}, effects={:?}, transition={:?}",
            track.kind,
            media_clips.len(),
            media_clips.first().map_or(AssetId(0), |clip| clip.asset),
            asset,
            media_clips.first().map(|clip| clip.audio_gain_tenth_db),
            media_clips
                .first()
                .map(|clip| (clip.audio_fade_in_frames, clip.audio_fade_out_frames)),
            media_clips.first().map(|clip| clip.speed_percent),
            media_clips.first().map(|clip| clip.effects.len()),
            media_clips
                .first()
                .and_then(|clip| clip.transition_in.as_ref())
                .map(|transition| transition.name.as_str()),
        ),
    )
}

fn evaluate_dialogue_pause_bounds(
    words: &[TimelineTranscriptWord],
    silences: &[TimelineSilenceSpan],
    minimum: TimeCode,
    maximum: TimeCode,
    capitalization_minimum: TimeCode,
) -> AssertionResult {
    let boundaries =
        dialogue_pacing_gaps(words, silences, minimum, maximum, capitalization_minimum);
    let violations = boundaries
        .iter()
        .filter(|gap| gap.status != "target")
        .map(|gap| {
            format!(
                "{:?}->{:?}={} ({}, transcript={})",
                gap.previous_word,
                gap.next_word,
                gap.pause_frames.0,
                gap.measurement,
                gap.transcript_pause_frames.0,
            )
        })
        .collect::<Vec<_>>();
    let observed = boundaries
        .iter()
        .map(|gap| {
            format!(
                "{:?}->{:?}={} ({})",
                gap.previous_word, gap.next_word, gap.pause_frames.0, gap.measurement,
            )
        })
        .collect::<Vec<_>>();
    assertion_result(
        "dialogue sentence pacing",
        !boundaries.is_empty() && violations.is_empty(),
        format!(
            "expected every detected boundary in {}..={} project frames; observed={observed:?}, violations={violations:?}",
            minimum.0, maximum.0,
        ),
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

fn evaluate_caption_words(word_set: &str, outcome: &EvalOutcome) -> AssertionResult {
    let Some(expected) = outcome.context.word_sets.get(word_set) else {
        return assertion_result(
            "caption words exact",
            false,
            format!("unknown authored word set {word_set:?}"),
        );
    };
    let mut captions = timeline_clips(&outcome.final_document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => {
                Some((clip.timeline_start, clip.id, title.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    captions.sort_by_key(|(start, clip, _)| (*start, *clip));
    let observed = normalize_word_sequence(captions.iter().map(|(_, _, text)| *text));
    let expected = normalize_word_sequence(expected.iter().map(String::as_str));
    let (missing, unexpected) = word_sequence_delta(&expected, &observed);
    assertion_result(
        "caption words exact",
        observed == expected,
        format!(
            "authored_set={word_set:?}, expected={expected:?}, observed={observed:?}, missing={missing:?}, unexpected={unexpected:?}"
        ),
    )
}

fn evaluate_caption_sentences(outcome: &EvalOutcome) -> AssertionResult {
    let mut captions = timeline_clips(&outcome.final_document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => {
                Some((clip.timeline_start, clip.id, title.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    captions.sort_by_key(|(start, clip, _)| (*start, *clip));
    let crossovers = captions
        .iter()
        .filter(|(_, _, text)| {
            caption_contains_sentence_crossover(text)
                || caption_contains_capitalized_sentence_crossover(text)
        })
        .map(|(_, clip, text)| format!("clip {}: {text:?}", clip.0))
        .collect::<Vec<_>>();
    let dangling = captions
        .iter()
        .filter(|(_, _, text)| caption_ends_with_dangling_word(text))
        .map(|(_, clip, text)| format!("clip {}: {text:?}", clip.0))
        .collect::<Vec<_>>();
    let semantic_splits = captions
        .windows(2)
        .filter(|pair| caption_boundary_breaks_phrase(pair[0].2, pair[1].2))
        .map(|pair| {
            format!(
                "clips {} -> {}: {:?} / {:?}",
                pair[0].1.0, pair[1].1.0, pair[0].2, pair[1].2
            )
        })
        .collect::<Vec<_>>();
    let missing_punctuation = captions
        .windows(2)
        .filter(|pair| {
            caption_starts_likely_sentence(pair[1].2) && !caption_ends_sentence(pair[0].2)
        })
        .map(|pair| {
            format!(
                "clips {} -> {}: {:?} / {:?}",
                pair[0].1.0, pair[1].1.0, pair[0].2, pair[1].2
            )
        })
        .collect::<Vec<_>>();
    let final_punctuated = captions
        .last()
        .is_some_and(|(_, _, text)| caption_ends_sentence(text));
    let passed = crossovers.is_empty()
        && dangling.is_empty()
        && semantic_splits.is_empty()
        && missing_punctuation.is_empty()
        && final_punctuated;
    assertion_result(
        "caption sentence grouping",
        passed,
        if passed {
            format!(
                "{} caption cues preserve punctuation and semantic phrase boundaries",
                captions.len()
            )
        } else {
            format!(
                "sentence_crossovers={crossovers:?}, dangling_endings={dangling:?}, semantic_splits={semantic_splits:?}, missing_punctuation={missing_punctuation:?}, final_punctuated={final_punctuated}"
            )
        },
    )
}

fn evaluate_caption_presentation(
    allowed_positions: &[TitlePosition],
    color_token: u8,
    background_scrim: bool,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let violations = timeline_clips(&outcome.final_document)
        .filter_map(|clip| match &clip.content {
            ClipContent::Title(title) if title.caption_preset.is_some() => (!allowed_positions
                .contains(&title.position)
                || title.color_token != color_token
                || title.background_scrim != background_scrim)
                .then(|| {
                    format!(
                        "clip {} position={} color_token={} scrim={}",
                        clip.id.0,
                        title.position.as_str(),
                        title.color_token,
                        title.background_scrim
                    )
                }),
            _ => None,
        })
        .collect::<Vec<_>>();
    assertion_result(
        "caption presentation",
        violations.is_empty(),
        if violations.is_empty() {
            format!(
                "all captions use positions={:?}, color_token={color_token}, scrim={background_scrim}",
                allowed_positions
                    .iter()
                    .map(|position| position.as_str())
                    .collect::<Vec<_>>()
            )
        } else {
            format!("violations={violations:?}")
        },
    )
}

fn caption_contains_sentence_crossover(text: &str) -> bool {
    let words = text.split_whitespace().collect::<Vec<_>>();
    words
        .iter()
        .take(words.len().saturating_sub(1))
        .any(|word| {
            let without_closers = word.trim_end_matches(|character| {
                matches!(
                    character,
                    '\'' | '"' | ')' | ']' | '}' | '\u{2019}' | '\u{201d}'
                )
            });
            matches!(without_closers.chars().next_back(), Some('.' | '!' | '?'))
        })
}

fn caption_contains_capitalized_sentence_crossover(text: &str) -> bool {
    text.split_whitespace()
        .skip(1)
        .any(caption_starts_likely_sentence)
}

fn caption_starts_likely_sentence(text: &str) -> bool {
    let word = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric());
    let starts_uppercase = word
        .chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_uppercase);
    starts_uppercase
        && matches!(
            word.to_ascii_lowercase().as_str(),
            "and" | "but" | "so" | "then" | "they" | "meanwhile" | "however"
        )
}

fn caption_ends_sentence(text: &str) -> bool {
    let without_closers = text.trim_end_matches(|character| {
        matches!(
            character,
            '\'' | '"' | ')' | ']' | '}' | '\u{2019}' | '\u{201d}'
        )
    });
    matches!(without_closers.chars().next_back(), Some('.' | '!' | '?'))
}

fn caption_ends_with_dangling_word(text: &str) -> bool {
    let word = text
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric())
        .to_ascii_lowercase();
    matches!(
        word.as_str(),
        "a" | "an"
            | "the"
            | "and"
            | "or"
            | "but"
            | "of"
            | "to"
            | "in"
            | "on"
            | "at"
            | "for"
            | "from"
            | "with"
            | "my"
            | "your"
            | "their"
            | "our"
            | "its"
    )
}

fn caption_boundary_breaks_phrase(previous: &str, next: &str) -> bool {
    let previous_word = previous.split_whitespace().next_back().unwrap_or_default();
    let next_word = next.split_whitespace().next().unwrap_or_default();
    if caption_ends_sentence(previous) || caption_ends_clause(previous_word) {
        return false;
    }
    let previous_normalized = normalize_caption_word(previous_word);
    let next_normalized = normalize_caption_word(next_word);
    let proper_name = starts_with_uppercase(previous_word) && starts_with_uppercase(next_word);
    proper_name
        || caption_ends_with_dangling_word(previous)
        || matches!(
            next_normalized.as_str(),
            "of" | "to" | "in" | "on" | "at" | "for" | "from" | "with"
        )
        || matches!(
            previous_normalized.as_str(),
            "i" | "ive"
                | "im"
                | "you"
                | "youre"
                | "he"
                | "hes"
                | "she"
                | "shes"
                | "it"
                | "its"
                | "we"
                | "were"
                | "they"
                | "theyre"
                | "very"
                | "recently"
                | "especially"
                | "maybe"
                | "just"
                | "even"
                | "that"
                | "these"
                | "those"
                | "this"
                | "where"
        )
        || matches!(
            (previous_normalized.as_str(), next_normalized.as_str()),
            ("super", "8") | ("home", "movies")
        )
}

fn caption_ends_clause(text: &str) -> bool {
    matches!(
        text.trim_end_matches(['\'', '"', ')', ']', '}'])
            .chars()
            .next_back(),
        Some(',' | ';' | ':')
    )
}

fn normalize_caption_word(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn starts_with_uppercase(text: &str) -> bool {
    text.chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_uppercase)
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
            !timeline_media_clips(&outcome.final_document)
                .any(|clip| clip.asset == *asset && clip.source_range.start == *source_frame)
        })
        .copied()
        .collect::<Vec<_>>();
    let interior = scenes
        .iter()
        .filter(|(asset, source_frame)| {
            timeline_media_clips(&outcome.final_document).any(|clip| {
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
    let matched = timeline_media_clips(&outcome.final_document)
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
    let matched = timeline_media_clips(&outcome.final_document)
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

fn evaluate_styled_captions(
    minimum_cues: usize,
    motion: CaptionMotion,
    outcome: &EvalOutcome,
) -> AssertionResult {
    let titles = timeline_clips(&outcome.final_document)
        .filter(|clip| matches!(&clip.content, ClipContent::Title(_)))
        .collect::<Vec<_>>();
    let matching = titles
        .iter()
        .filter(|clip| match motion {
            CaptionMotion::None => clip.effects.is_empty(),
            CaptionMotion::Fade => has_animated_parameter(clip, "opacity", "percent"),
            CaptionMotion::Pop => {
                has_animated_parameter(clip, "opacity", "percent")
                    && has_animated_parameter(clip, "transform", "scale_percent")
            }
            CaptionMotion::SlideUp => {
                has_animated_parameter(clip, "opacity", "percent")
                    && has_animated_parameter(clip, "transform", "y_percent")
            }
        })
        .count();
    assertion_result(
        "styled captions",
        titles.len() >= minimum_cues && matching >= minimum_cues,
        format!(
            "expected at least {minimum_cues} {} cues, observed {} title cues and {matching} matching motion curves",
            motion.as_str(),
            titles.len()
        ),
    )
}

fn evaluate_caption_safe_area(profile: DeliveryProfile, outcome: &EvalOutcome) -> AssertionResult {
    match delivery_conformance(&outcome.final_document, profile, 50, 50) {
        Ok(report) => {
            let violations = report
                .issues
                .iter()
                .filter(|issue| {
                    matches!(
                        issue.code.as_str(),
                        "caption_outside_safe_area" | "title_layout_unavailable"
                    )
                })
                .count();
            assertion_result(
                "delivery caption safe area",
                violations == 0,
                format!(
                    "profile={}, raster={}x{}, violations={violations}",
                    profile.as_str(),
                    report.resolution.0,
                    report.resolution.1
                ),
            )
        }
        Err(error) => assertion_result(
            "delivery caption safe area",
            false,
            format!(
                "profile={} could not be materialized: {error}",
                profile.as_str()
            ),
        ),
    }
}

fn evaluate_audio_present(document: &Document) -> AssertionResult {
    let audible_clips = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter(|clip| {
            clip.content.is_media()
                && clip.speed_percent == 100
                && document.asset(clip.asset).is_some_and(|asset| {
                    matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo)
                })
        })
        .count();
    assertion_result(
        "timeline audio present",
        audible_clips > 0,
        format!("real-time audio-bearing media clips={audible_clips}"),
    )
}

fn has_animated_parameter(clip: &openreel_core::Clip, effect_name: &str, parameter: &str) -> bool {
    clip.effects.iter().any(|effect| {
        effect.name == effect_name
            && effect
                .keyframes
                .get(parameter)
                .is_some_and(|curve| !curve.keyframes.is_empty())
    })
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

fn timeline_media_clips(document: &Document) -> impl Iterator<Item = &openreel_core::Clip> {
    timeline_clips(document).filter(|clip| {
        matches!(
            &clip.content,
            ClipContent::Media | ClipContent::Freeze { .. }
        )
    })
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

fn normalize_word_sequence<'a>(words: impl Iterator<Item = &'a str>) -> Vec<String> {
    words
        .flat_map(str::split_whitespace)
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn word_sequence_delta(expected: &[String], observed: &[String]) -> (Vec<String>, Vec<String>) {
    let mut expected_counts = BTreeMap::<&str, usize>::new();
    let mut observed_counts = BTreeMap::<&str, usize>::new();
    for word in expected {
        *expected_counts.entry(word).or_default() += 1;
    }
    for word in observed {
        *observed_counts.entry(word).or_default() += 1;
    }
    let missing = expected_counts
        .iter()
        .flat_map(|(word, count)| {
            let observed = observed_counts.get(word).copied().unwrap_or(0);
            std::iter::repeat_n((*word).to_owned(), count.saturating_sub(observed))
        })
        .collect();
    let unexpected = observed_counts
        .iter()
        .flat_map(|(word, count)| {
            let expected = expected_counts.get(word).copied().unwrap_or(0);
            std::iter::repeat_n((*word).to_owned(), count.saturating_sub(expected))
        })
        .collect();
    (missing, unexpected)
}

fn word_sequence_edit_distance(expected: &[String], observed: &[String]) -> usize {
    let mut previous = (0..=observed.len()).collect::<Vec<_>>();
    let mut current = vec![0; observed.len() + 1];
    for (expected_index, expected_word) in expected.iter().enumerate() {
        current[0] = expected_index + 1;
        for (observed_index, observed_word) in observed.iter().enumerate() {
            let substitution = previous[observed_index]
                .saturating_add(usize::from(expected_word != observed_word));
            let deletion = previous[observed_index + 1].saturating_add(1);
            let insertion = current[observed_index].saturating_add(1);
            current[observed_index + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[observed.len()]
}

fn word_error_rate_basis_points(edit_distance: usize, expected_words: usize) -> u16 {
    if expected_words == 0 {
        return u16::MAX;
    }
    let numerator = u64::try_from(edit_distance)
        .unwrap_or(u64::MAX)
        .saturating_mul(10_000);
    let denominator = u64::try_from(expected_words).unwrap_or(u64::MAX);
    u16::try_from(
        numerator
            .saturating_add(denominator.saturating_sub(1))
            .checked_div(denominator)
            .unwrap_or(u64::MAX),
    )
    .unwrap_or(u16::MAX)
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
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub reasoning_output_tokens: Option<u64>,
    pub tool_schema_bytes: u64,
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
            cached_input_tokens: sum_reported_tokens(
                results.iter().map(|result| result.cached_input_tokens),
            ),
            cache_creation_input_tokens: sum_reported_tokens(
                results
                    .iter()
                    .map(|result| result.cache_creation_input_tokens),
            ),
            output_tokens: results.iter().map(|result| result.output_tokens).sum(),
            reasoning_output_tokens: sum_reported_tokens(
                results.iter().map(|result| result.reasoning_output_tokens),
            ),
            tool_schema_bytes: results
                .iter()
                .map(|result| result.tool_surface.serialized_bytes)
                .sum(),
            cost_usd: all_costs_reported
                .then(|| results.iter().filter_map(|result| result.cost_usd).sum()),
            wall_time_ms: results.iter().map(|result| result.wall_time_ms).sum(),
            operations_applied: results.iter().map(|result| result.operations_applied).sum(),
        }
    }
}

fn sum_reported_tokens(mut values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values.try_fold(0_u64, |total, value| {
        value.map(|value| total.saturating_add(value))
    })
}

/// Render the same compact Markdown scoreboard used on stdout and in `docs/EVALS.md`.
#[must_use]
pub fn render_scoreboard(results: &[EvalResult]) -> String {
    let mut output = String::from(
        "| Eval | Pass | Assertions | Turns | Tools | Tokens | Cached in | Reasoning | Schema | USD | Wall | Ops |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for result in results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        let usd = result
            .cost_usd
            .map_or_else(|| "n/a".to_owned(), |cost| format!("${cost:.4}"));
        let cached = result
            .cached_input_tokens
            .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
        let reasoning = result
            .reasoning_output_tokens
            .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
        let _ = writeln!(
            output,
            "| {} | {status} | {}/{} | {} | {} | {} | {cached} | {reasoning} | {} B | {usd} | {} | {} |",
            result.name,
            result.passed_assertion_count(),
            result.assertions.len(),
            result.turns,
            result.tool_call_count(),
            result.total_tokens(),
            result.tool_surface.serialized_bytes,
            format_millis(result.wall_time_ms),
            result.operations_applied,
        );
    }
    let totals = SuiteTotals::from_results(results);
    let status = if totals.failed == 0 { "PASS" } else { "FAIL" };
    let usd = totals
        .cost_usd
        .map_or_else(|| "n/a".to_owned(), |cost| format!("${cost:.4}"));
    let cached = totals
        .cached_input_tokens
        .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
    let reasoning = totals
        .reasoning_output_tokens
        .map_or_else(|| "n/a".to_owned(), |tokens| tokens.to_string());
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
        "| **TOTAL** | **{status}** | **{assertion_passes}/{assertion_total}** | **{}** | **{}** | **{}** | **{cached}** | **{reasoning}** | **{} B** | **{usd}** | **{}** | **{}** |",
        totals.turns,
        totals.tool_calls,
        totals.input_tokens.saturating_add(totals.output_tokens),
        totals.tool_schema_bytes,
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
        AgentError, AgentSession, AssetSilences, AuthenticationStatus, CaptionPreset, Clip, ClipId,
        HarnessId, Marker, MarkerId, MediaAsset, MediaKind, Rational, SilenceSpan, Track, TrackId,
        TrackKind,
    };

    use super::*;

    #[test]
    fn diagnostic_trace_is_bounded_on_character_boundaries() {
        let long = "é".repeat(2_001);
        let traced = bounded_trace(&long);

        assert_eq!(traced.chars().take(2_000).count(), 2_000);
        assert!(traced.ends_with("...[truncated]"));
        assert_eq!(bounded_trace("short"), "short");
    }

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
            max_cost_usd: Some(0.75),
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

    fn provenance_marker(effect: u64, samples: &[(i64, u16, u16, u16, u16)]) -> Marker {
        Marker {
            id: MarkerId(99),
            position: TimeCode::ZERO,
            label: crate::server::encode_reframe_subject_provenance(&ReframeSubjectProvenance {
                clip: ClipId(1),
                effect: openreel_core::EffectId(effect),
                samples: samples
                    .iter()
                    .map(
                        |(
                            at,
                            left_basis_points,
                            right_basis_points,
                            top_basis_points,
                            bottom_basis_points,
                        )| {
                            TrackedSubjectBounds {
                                at: TimeCode(*at),
                                left_basis_points: *left_basis_points,
                                right_basis_points: *right_basis_points,
                                top_basis_points: *top_basis_points,
                                bottom_basis_points: *bottom_basis_points,
                            }
                        },
                    )
                    .collect(),
            }),
            color_token: 3,
        }
    }

    #[test]
    fn rendered_reframe_verification_detects_lost_delivery_curves() {
        let mut source = document();
        source.tracks[0].clips[0]
            .effects
            .push(openreel_core::Effect {
                id: openreel_core::EffectId(9),
                name: "reframe".to_owned(),
                parameters: BTreeMap::from([(
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(5_625),
                )]),
                keyframes: BTreeMap::from([(
                    "focus_x_percent".to_owned(),
                    openreel_core::AutomationCurve {
                        keyframes: vec![openreel_core::Keyframe {
                            at: TimeCode::ZERO,
                            value: 42,
                            interpolation: openreel_core::KeyframeInterpolation::Linear,
                        }],
                    },
                )]),
            });
        source.markers.push(provenance_marker(
            9,
            &[
                (0, 4_000, 5_000, 3_500, 6_500),
                (59, 4_000, 5_000, 3_500, 6_500),
            ],
        ));
        let delivered =
            document_for_delivery_profile(&source, DeliveryProfile::VerticalShort, 50, 50).unwrap();
        let preserved = rendered_reframe_verification(&source, &delivered).unwrap();
        assert_eq!(preserved.expected_animated_clips, 1);
        assert_eq!(preserved.preserved_animated_clips, 1);
        assert_eq!(preserved.expected_subject_provenance_clips, 1);
        assert_eq!(preserved.preserved_subject_provenance_clips, 1);
        assert!(preserved.passed);

        let mut lost = delivered;
        lost.tracks[0].clips[0].effects[0].keyframes.clear();
        let rejected = rendered_reframe_verification(&source, &lost).unwrap();
        assert_eq!(rejected.preserved_animated_clips, 0);
        assert!(!rejected.passed);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn tracked_subject_containment_rejects_static_or_wrong_direction_reframes() {
        let effect = |focus_at_end: i64| openreel_core::Effect {
            id: openreel_core::EffectId(7),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([(
                "target_aspect_basis_points".to_owned(),
                ParamValue::Integer(5_625),
            )]),
            keyframes: BTreeMap::from([
                (
                    "focus_x_basis_points".to_owned(),
                    openreel_core::AutomationCurve {
                        keyframes: vec![
                            openreel_core::Keyframe {
                                at: TimeCode::ZERO,
                                value: 5_000,
                                interpolation: openreel_core::KeyframeInterpolation::Linear,
                            },
                            openreel_core::Keyframe {
                                at: TimeCode(36),
                                value: focus_at_end,
                                interpolation: openreel_core::KeyframeInterpolation::Linear,
                            },
                        ],
                    },
                ),
                (
                    "focus_y_basis_points".to_owned(),
                    openreel_core::AutomationCurve {
                        keyframes: vec![openreel_core::Keyframe {
                            at: TimeCode::ZERO,
                            value: 5_000,
                            interpolation: openreel_core::KeyframeInterpolation::Linear,
                        }],
                    },
                ),
            ]),
        };
        let provenance = ReframeSubjectProvenance {
            clip: ClipId(1),
            effect: openreel_core::EffectId(7),
            samples: vec![
                TrackedSubjectBounds {
                    at: TimeCode::ZERO,
                    left_basis_points: 4_400,
                    right_basis_points: 5_600,
                    top_basis_points: 3_500,
                    bottom_basis_points: 6_500,
                },
                TrackedSubjectBounds {
                    at: TimeCode(36),
                    left_basis_points: 7_000,
                    right_basis_points: 8_000,
                    top_basis_points: 3_500,
                    bottom_basis_points: 6_500,
                },
            ],
        };
        let mut final_document = document();
        final_document.tracks[0].clips[0]
            .effects
            .push(effect(5_000));
        let clip = &final_document.tracks[0].clips[0];
        let rejected_static = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &provenance,
        );
        assert!(
            rejected_static
                .iter()
                .any(|detail| detail.contains("does not contain tracked subject")),
            "{rejected_static:?}"
        );

        final_document.tracks[0].clips[0].effects[0] = effect(3_000);
        let clip = &final_document.tracks[0].clips[0];
        let rejected_wrong_direction = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &provenance,
        );
        assert!(
            rejected_wrong_direction
                .iter()
                .any(|detail| detail.contains("does not contain tracked subject")),
            "{rejected_wrong_direction:?}"
        );

        final_document.media_pool[0].resolution = Some((352, 288));
        final_document.tracks[0].clips[0].effects[0] = effect(4_500);
        let laura_edge = ReframeSubjectProvenance {
            clip: ClipId(1),
            effect: openreel_core::EffectId(7),
            samples: vec![TrackedSubjectBounds {
                at: TimeCode(36),
                left_basis_points: 2_150,
                right_basis_points: 4_650,
                top_basis_points: 3_500,
                bottom_basis_points: 6_500,
            }],
        };
        let clip = &final_document.tracks[0].clips[0];
        let rejected_edge_clip = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &laura_edge,
        );
        assert!(
            rejected_edge_clip
                .iter()
                .any(|detail| detail.contains("does not contain tracked subject")),
            "{rejected_edge_clip:?}"
        );

        final_document.media_pool[0].resolution = Some((320, 180));
        final_document.tracks[0].clips[0].effects[0] = effect(7_400);
        let clip = &final_document.tracks[0].clips[0];
        let accepted = evaluate_tracked_subject_containment(
            &final_document,
            clip,
            &clip.effects[0],
            &provenance,
        );
        assert!(accepted.is_empty(), "{accepted:?}");
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
                    cached_input_tokens: Some(100),
                    cache_creation_input_tokens: Some(4),
                    output_tokens: 30,
                    reasoning_output_tokens: Some(12),
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
        assert_eq!(metrics.uncached_input_tokens(), Some(20));
        assert_eq!(metrics.reasoning_output_tokens, Some(12));
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
        assert_eq!(session.cached_input_tokens, None);
        assert_eq!(session.cache_creation_input_tokens, None);
        assert_eq!(session.reasoning_output_tokens, None);
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
            deliverable: None,
        };
        let outcome = EvalOutcome {
            final_document,
            final_words: Vec::new(),
            final_timeline_words: Vec::new(),
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
            deliverable: None,
        };
        let outcome = EvalOutcome {
            final_document: document(),
            final_words: vec!["alpha".to_owned(), "bravo".to_owned()],
            final_timeline_words: Vec::new(),
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
                cached_input_tokens: Some(80),
                cache_creation_input_tokens: Some(5),
                output_tokens: 20,
                reasoning_output_tokens: Some(10),
                tool_surface: crate::ToolSurfaceMetrics {
                    tool_count: 7,
                    serialized_bytes: 2_048,
                    input_schema_bytes: 1_024,
                    description_bytes: 512,
                },
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
    fn audio_presence_follows_the_real_mixer_contract() {
        let mut with_audio = document();
        assert!(!evaluate_audio_present(&with_audio).passed);
        with_audio.media_pool[0].kind = MediaKind::AudioVideo;
        assert!(evaluate_audio_present(&with_audio).passed);

        with_audio.tracks[0].clips[0].speed_percent = 200;
        assert!(!evaluate_audio_present(&with_audio).passed);
    }

    #[test]
    fn reframe_stability_rejects_eased_or_fast_virtual_camera_motion() {
        let mut final_document = document();
        let curve = |end: i64, interpolation| openreel_core::AutomationCurve {
            keyframes: vec![
                openreel_core::Keyframe {
                    at: TimeCode::ZERO,
                    value: 50,
                    interpolation,
                },
                openreel_core::Keyframe {
                    at: TimeCode(12),
                    value: end,
                    interpolation,
                },
            ],
        };
        final_document.tracks[0].clips[0]
            .effects
            .push(openreel_core::Effect {
                id: openreel_core::EffectId(1),
                name: "reframe".to_owned(),
                parameters: BTreeMap::from([(
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(5_625),
                )]),
                keyframes: BTreeMap::from([
                    (
                        "focus_x_percent".to_owned(),
                        curve(58, openreel_core::KeyframeInterpolation::EaseInOut),
                    ),
                    (
                        "focus_y_percent".to_owned(),
                        curve(50, openreel_core::KeyframeInterpolation::EaseInOut),
                    ),
                ]),
            });
        final_document.markers.push(provenance_marker(
            1,
            &[
                (0, 4_500, 5_500, 3_500, 6_500),
                (12, 4_500, 5_500, 3_500, 6_500),
                (59, 4_500, 5_500, 3_500, 6_500),
            ],
        ));
        let mut outcome = EvalOutcome {
            final_document,
            final_words: Vec::new(),
            final_timeline_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context: FixtureContext::default(),
            session: SessionMetrics::default(),
            operations: Vec::new(),
            undo_steps_to_original: None,
        };

        let rejected = evaluate_reframe_stability(TrackId(1), 2, 25..=75, 20..=80, 2, &outcome);
        assert!(!rejected.passed);
        assert!(rejected.detail.contains("not linearly interpolated"));
        assert!(rejected.detail.contains("jumps 8 percent"));

        let effect = &mut outcome.final_document.tracks[0].clips[0].effects[0];
        effect.keyframes.insert(
            "focus_x_percent".to_owned(),
            curve(52, openreel_core::KeyframeInterpolation::Linear),
        );
        effect.keyframes.insert(
            "focus_y_percent".to_owned(),
            curve(50, openreel_core::KeyframeInterpolation::Linear),
        );
        assert!(evaluate_reframe_stability(TrackId(1), 2, 25..=75, 20..=80, 2, &outcome).passed);
    }

    #[test]
    fn dialogue_pause_assertion_catches_the_m38_short_boundary() {
        let word = |text: &str, asset: u64, start: i64, end: i64| TimelineTranscriptWord {
            text: text.to_owned(),
            speaker: None,
            asset: AssetId(asset),
            track: TrackId(1),
            clip: ClipId(asset),
            source_start: TimeCode(start),
            source_end: TimeCode(end),
            project_start: TimeCode(start),
            project_end: TimeCode(end),
        };
        let words = vec![
            word("rain", 1, 80, 100),
            word("Neighbors", 1, 112, 130),
            word("beds", 2, 280, 300),
            word("Then", 2, 307, 325),
            word("peppers.", 2, 380, 400),
            word("Now", 3, 412, 430),
        ];

        let rejected =
            evaluate_dialogue_pause_bounds(&words, &[], TimeCode(9), TimeCode(15), TimeCode(4));
        assert!(!rejected.passed);
        assert!(rejected.detail.contains("beds"));
        assert!(rejected.detail.contains("=7"));
    }

    #[test]
    fn exact_caption_words_catch_the_m37_material_error() {
        let mut final_document = document();
        final_document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(2),
                asset: AssetId::default(),
                source_range: TimeCode::ZERO..TimeCode(30),
                content: ClipContent::Title(CaptionPreset::Social.title("Map Steady the Exped")),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });
        let mut context = FixtureContext::default();
        context.word_sets.insert(
            "authored".to_owned(),
            normalize_word_sequence(["River map steadies the expedition"].into_iter()),
        );
        let mut outcome = EvalOutcome {
            final_document,
            final_words: Vec::new(),
            final_timeline_words: Vec::new(),
            remaining_silences: Vec::new(),
            remaining_scenes: Vec::new(),
            context,
            session: SessionMetrics::default(),
            operations: Vec::new(),
            undo_steps_to_original: None,
        };

        let rejected = evaluate_caption_words("authored", &outcome);
        assert!(!rejected.passed);
        assert!(rejected.detail.contains("expedition"));
        let ClipContent::Title(title) = &mut outcome.final_document.tracks[1].clips[0].content
        else {
            panic!("caption fixture should be a title");
        };
        title.text = "River map steadies the expedition".to_owned();
        assert!(evaluate_caption_words("authored", &outcome).passed);
    }

    #[test]
    fn caption_sentence_grouping_rejects_crossovers() {
        assert!(caption_contains_sentence_crossover(
            "and rainwater. Neighbors decided it could feed"
        ));
        assert!(!caption_contains_sentence_crossover(
            "Last spring this empty lot collected weeds and rainwater."
        ));
        assert!(!caption_contains_sentence_crossover(
            "Over three weekends volunteers"
        ));
    }

    #[test]
    fn rendered_dialogue_wer_uses_ordered_edits_and_rounds_up() {
        let expected = normalize_word_sequence(["river map steadies the expedition"].into_iter());
        let one_substitution =
            normalize_word_sequence(["river map steadies an expedition"].into_iter());
        let reordered = normalize_word_sequence(["map river steadies the expedition"].into_iter());

        assert_eq!(word_sequence_edit_distance(&expected, &one_substitution), 1);
        assert_eq!(word_error_rate_basis_points(1, expected.len()), 2_000);
        assert_eq!(word_sequence_edit_distance(&expected, &reordered), 2);
        assert_eq!(word_error_rate_basis_points(1, 3), 3_334);
    }

    #[test]
    fn required_rendered_transcript_has_an_explicit_failure_assertion() {
        let mut deliverable = deliverable_shell(
            EvalDeliverableSpec {
                profile: DeliveryProfile::VerticalShort,
                focus_x_percent: 50,
                focus_y_percent: 50,
                proof_frames: 9,
                proof_cell_width: 240,
                require_audio: true,
                expected_transcript_word_set: Some("authored"),
                maximum_word_error_rate_basis_points: 1_500,
                maximum_caption_word_error_rate_basis_points: None,
                loudness: None,
            },
            &document(),
            Path::new("artifacts/f2"),
        );
        deliverable
            .errors
            .push("post-render transcription failed deliberately".to_owned());
        let assertions = deliverable_assertions(&deliverable);
        let rendered = assertions
            .iter()
            .find(|assertion| assertion.assertion == "rendered dialogue accuracy")
            .expect("required rendered transcript assertion");
        assert!(!rendered.passed);
        assert!(rendered.detail.contains("unavailable"));
    }

    #[test]
    fn rendered_caption_alignment_detects_words_missing_from_the_screen() {
        let expected = "river map steadies the expedition"
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let observed = "map steady the exped"
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let verification = verify_word_sequences(&expected, &observed, 500);
        assert!(!verification.passed);
        assert!(verification.word_error_rate_basis_points > 500);
        assert!(!verification.missing_words.is_empty());
    }

    #[test]
    fn caption_sentence_checks_reject_the_first_interview_attempt_failures() {
        assert!(caption_contains_capitalized_sentence_crossover(
            "movies and I've been cleaning They"
        ));
        assert!(caption_ends_with_dangling_word(
            "But recently I was living in New Orleans and"
        ));
        assert!(caption_starts_likely_sentence(
            "And I've been cleaning them"
        ));
        assert!(!caption_ends_sentence("submerged in those floodwaters"));
        assert!(caption_boundary_breaks_phrase(
            "But recently I was living in New",
            "Orleans, and my house flooded,"
        ));
        assert!(caption_boundary_breaks_phrase("and a lot", "of my films,"));
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
            deliverable: None,
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

    #[test]
    fn human_review_stays_pending_until_a_complete_person_score_exists() {
        let definition = EvalDefinition {
            name: "f1 finished cut",
            rationale: "review fixture",
            fixture_builder: unused_fixture,
            prompts: &["finish it"],
            assertions: Vec::new(),
            budgets: budgets(),
            deliverable: None,
        };
        let mut result =
            EvalResult::execution_failure(&definition, &EvalError::Agent("unused".to_owned()));
        let mut deliverable = deliverable_shell(
            EvalDeliverableSpec {
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
            },
            &document(),
            Path::new("artifacts/f1"),
        );
        deliverable.output_sha256 = Some("a".repeat(64));
        result.deliverable = Some(deliverable);

        let sampled_review = human_review_template(
            "finished-v2",
            "run-sampled",
            &[result.clone(), result.clone()],
        );
        assert_eq!(sampled_review.tasks[0].task_id, "f1");
        assert_eq!(sampled_review.tasks[1].task_id, "f1-sample-2");

        let mut review = human_review_template("finished-v2", "run-1", &[result]);
        let pending = summarize_human_review(&review).unwrap();
        assert_eq!(pending.tasks_reviewed, 0);
        assert_eq!(pending.tasks_pending, 1);
        assert_eq!(pending.acceptance_rate, None);

        review.reviewer = Some("human".to_owned());
        review.tasks[0].accepted = Some(true);
        review.tasks[0].ratings = HumanRatings {
            story: Some(4.0),
            pacing: Some(2.5),
            visual_finish: Some(5.0),
            audio_finish: Some(4.0),
            captions: Some(5.0),
            delivery_readiness: Some(4.0),
        };
        let scored = summarize_human_review(&review).unwrap();
        assert_eq!(scored.tasks_reviewed, 1);
        assert_eq!(scored.tasks_accepted, 1);
        assert_eq!(scored.acceptance_rate, Some(1.0));
        assert_eq!(scored.mean_ratings.pacing, Some(2.5));
        assert_eq!(scored.overall_mean_rating, Some(24.5 / 6.0));
    }

    #[test]
    fn human_review_rejects_partial_or_out_of_range_scores() {
        let mut review = HumanReviewFile {
            schema_version: 1,
            benchmark_id: "finished-v2".to_owned(),
            run_id: "run-1".to_owned(),
            reviewer: None,
            tasks: vec![HumanTaskReview {
                task_id: "f1".to_owned(),
                artifact_sha256: Some("b".repeat(64)),
                accepted: Some(false),
                ratings: HumanRatings {
                    story: Some(0.5),
                    ..HumanRatings::default()
                },
                notes: None,
            }],
        };
        assert!(summarize_human_review(&review).is_err());
        review.tasks[0].ratings = HumanRatings {
            story: Some(1.0),
            pacing: Some(2.0),
            visual_finish: Some(3.0),
            audio_finish: Some(4.0),
            captions: Some(5.0),
            delivery_readiness: Some(6.0),
        };
        assert!(summarize_human_review(&review).is_err());

        review.tasks[0].ratings.delivery_readiness = Some(4.25);
        assert!(summarize_human_review(&review).is_err());
    }

    #[test]
    fn proof_sampling_is_uniform_and_includes_both_visible_edges() {
        assert_eq!(
            uniform_sample_frames(TimeCode(10), 4),
            vec![TimeCode(0), TimeCode(3), TimeCode(6), TimeCode(9)]
        );
        assert_eq!(uniform_sample_frames(TimeCode(10), 1), vec![TimeCode(4)]);
    }
}
