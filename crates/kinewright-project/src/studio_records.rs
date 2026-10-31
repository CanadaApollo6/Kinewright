//! Shared Studio data. Runtime capabilities and scheduler decisions stay outside records.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{BranchId, HeadId, ObjectRef, OccurrenceId, ProjectId, RecordKind},
    receipts::CommandId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub(crate) crate::bundle_records::SessionId);
        impl $name {
            pub fn fresh() -> Result<Self, BundleError> { Ok(Self(crate::bundle_records::SessionId::fresh()?)) }
            #[must_use] pub fn as_str(&self) -> &str { self.0.as_str() }
        }
    )+};
}
identity!(
    VersionId,
    CommentId,
    FeedbackBatchId,
    NoteId,
    JobId,
    RunId,
    TaskId,
    AttemptId,
    EffectId,
    DeliveryId,
    QuestionId,
    AgentThreadId
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotKey {
    pub project: ProjectId,
    pub version: VersionId,
    pub head: HeadId,
    pub document_hash: String,
    pub input_manifest_ref: ObjectRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDescriptor {
    pub key: SnapshotKey,
    pub checkpoint_ref: ObjectRef,
    pub document_ref: ObjectRef,
    pub lineage_ref: ObjectRef,
    pub input_manifest_ref: ObjectRef,
    pub revision_refs: Vec<ObjectRef>,
    pub parent_version: Option<VersionId>,
    pub run_provenance: Option<RunId>,
}
impl SnapshotDescriptor {
    #[must_use]
    pub fn references(&self) -> Vec<ObjectRef> {
        let mut refs = vec![
            self.checkpoint_ref.clone(),
            self.document_ref.clone(),
            self.lineage_ref.clone(),
            self.input_manifest_ref.clone(),
        ];
        refs.extend(self.revision_refs.clone());
        refs
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishSnapshot {
    pub version: VersionId,
    pub expected_head: HeadId,
    pub checkpoint_ref: ObjectRef,
    pub parent_version: Option<VersionId>,
    pub run_provenance: Option<RunId>,
    pub revision_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRate {
    pub numerator: u32,
    pub denominator: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Upright,
    Rotate90,
    Rotate180,
    Rotate270,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackTarget {
    pub local_clip_id: kinewright_core::ClipId,
    pub occurrence_id: Option<OccurrenceId>,
    pub local_frame: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackAnchor {
    pub snapshot: SnapshotKey,
    pub frame: i64,
    pub fps: FrameRate,
    pub raster_size: [u32; 2],
    pub orientation: Orientation,
    pub target: Option<FeedbackTarget>,
    pub source_ref: Option<ObjectRef>,
    pub script_beat_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioRange {
    pub start_micros: u64,
    pub end_micros: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceRef {
    pub note_id: NoteId,
    pub audio_ref: ObjectRef,
    pub duration_micros: u64,
    pub range: Option<AudioRange>,
    pub playhead_log_ref: Option<ObjectRef>,
    pub original_asr_ref: Option<ObjectRef>,
    pub chosen_transcript_revision: Option<ObjectRef>,
}
impl VoiceRef {
    pub(crate) fn matches_note(&self, saved: &Self) -> bool {
        self.audio_ref == saved.audio_ref
            && self.duration_micros == saved.duration_micros
            && self.playhead_log_ref == saved.playhead_log_ref
            && self
                .original_asr_ref
                .as_ref()
                .is_none_or(|reference| saved.original_asr_ref.as_ref() == Some(reference))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackRevision {
    pub comment_id: CommentId,
    pub revision: u64,
    pub predecessor: Option<ObjectRef>,
    pub anchor: FeedbackAnchor,
    pub original_text_ref: ObjectRef,
    pub strokes_ref: ObjectRef,
    pub frame_proof_ref: ObjectRef,
    pub voice_ref: Option<VoiceRef>,
    pub transcript_revision: Option<ObjectRef>,
}
impl FeedbackRevision {
    #[must_use]
    pub fn references(&self) -> Vec<ObjectRef> {
        let mut refs = vec![
            self.anchor.snapshot.input_manifest_ref.clone(),
            self.original_text_ref.clone(),
            self.strokes_ref.clone(),
            self.frame_proof_ref.clone(),
        ];
        refs.extend(self.predecessor.clone());
        refs.extend(self.anchor.source_ref.clone());
        refs.extend(self.transcript_revision.clone());
        if let Some(voice) = &self.voice_ref {
            refs.push(voice.audio_ref.clone());
            refs.extend(voice.playhead_log_ref.clone());
            refs.extend(voice.original_asr_ref.clone());
            refs.extend(voice.chosen_transcript_revision.clone());
        }
        refs
    }
}
/// The opaque evidence a typed or drawn revision names. Each kind has its own ingress bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackEvidenceKind {
    /// UTF-8 text, at most `comment_text_bytes`.
    Text,
    /// Strokes JSON, at most `comment_points` points.
    Strokes,
    /// The displayed-frame proof, at most `FRAME_PROOF_BYTES`.
    FrameProof,
}
/// Proofs name the displayed identity and a pixel digest, never the pixels themselves.
pub const FRAME_PROOF_BYTES: u64 = 16 << 10;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackBatch {
    pub batch_id: FeedbackBatchId,
    pub revision_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviseRequest {
    pub command_id: CommandId,
    pub batch_id: FeedbackBatchId,
    pub expected_candidate_head: HeadId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviseResult {
    Accepted {
        run_id: RunId,
        batch_id: FeedbackBatchId,
    },
    Waiting {
        reason: String,
    },
    Conflict,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalStatus {
    Exact,
    Mapped,
    Ambiguous,
    Removed,
    Unresolved,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialStatus {
    Exact,
    Transformed,
    Unresolved,
    Removed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveFeedback {
    pub comment_revision_ref: ObjectRef,
    pub candidate_checkpoint_ref: ObjectRef,
    pub beat_map_ref: Option<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentResolution {
    pub original_ref: ObjectRef,
    pub candidate_head: HeadId,
    pub temporal: TemporalStatus,
    pub spatial: SpatialStatus,
    pub evidence_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispositionOutcome {
    Addressed,
    RemovedTarget,
    Unresolved,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disposition {
    pub original_ref: ObjectRef,
    pub resolution_ref: ObjectRef,
    pub resulting_snapshot: Option<SnapshotKey>,
    pub outcome: DispositionOutcome,
    pub reason_ref: Option<ObjectRef>,
    pub evidence_refs: Vec<ObjectRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionIndex {
    pub version: u32,
    pub snapshots: BTreeMap<VersionId, SnapshotDescriptor>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmittedBatch {
    pub batch_ref: ObjectRef,
    pub request: ReviseRequest,
    pub consumed_by: Option<RunId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackIndex {
    pub version: u32,
    pub revisions: BTreeMap<CommentId, Vec<ObjectRef>>,
    pub batches: BTreeMap<FeedbackBatchId, SubmittedBatch>,
    pub notes: BTreeMap<NoteId, VoiceRef>,
}
impl Default for VersionIndex {
    fn default() -> Self {
        Self {
            version: 1,
            snapshots: BTreeMap::new(),
        }
    }
}
impl Default for FeedbackIndex {
    fn default() -> Self {
        Self {
            version: 1,
            revisions: BTreeMap::new(),
            batches: BTreeMap::new(),
            notes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpaqueRevision(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Queued,
    Running,
    Reviewing,
    Paused,
    Interrupted,
    Finished,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeDetail {
    pub reason: String,
    pub evidence_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RunOutcome {
    Pending,
    Ready,
    Failed(OutcomeDetail),
    Cancelled(OutcomeDetail),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWaitReason {
    RateLimit,
    Authentication,
    Approval,
    Secret,
    Capacity,
    Unavailable,
    Other,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitDetail {
    pub reason: String,
    pub provider_reason: Option<ProviderWaitReason>,
    pub attempt: Option<AttemptId>,
    pub missing_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RunWait {
    None,
    Files(WaitDetail),
    Provider(WaitDetail),
    Question(QuestionId),
    Children(Vec<TaskId>),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunCleanup {
    Settled,
    Pending {
        identities: Vec<String>,
    },
    Blocked {
        identities: Vec<String>,
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunActionKind {
    Pause,
    Stop,
    Continue,
    SendMessage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunAction {
    Pause,
    Stop,
    Continue,
    SendMessage { text_ref: ObjectRef },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIntent {
    pub command_id: CommandId,
    pub run_id: RunId,
    pub expected_control_revision: OpaqueRevision,
    pub action: RunAction,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AttemptState {
    Reserved,
    Owned,
    Contained,
    Executing,
    Waiting,
    Stopping,
    CleanupBlocked(String),
    Retired(OutcomeDetail),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum TaskState {
    Queued,
    Preparing,
    Running(AttemptId),
    WaitingForQuestion(QuestionId),
    WaitingForChildren(Vec<TaskId>),
    Freezing,
    Cancelling,
    CleanupBlocked(String),
    Terminal(ObjectRef),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EffectState {
    Pending,
    Claimed(AttemptId),
    OutcomeUnknown,
    Settled,
    Disposed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectSafety {
    ReplaySafe,
    ProcessBound,
    ReconcileExternal,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliveryState {
    Pending,
    Offered {
        attempt: AttemptId,
        message_id: String,
    },
    Observed {
        run: RunId,
    },
    Disposed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "reference",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RuntimeRequest {
    Question(ObjectRef),
    Approval(ObjectRef),
    Secret(ObjectRef),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptView {
    pub id: AttemptId,
    pub state: AttemptState,
    pub session: Option<AgentThreadId>,
    pub turn: u64,
    pub guardian: Option<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectView {
    pub id: EffectId,
    pub state: EffectState,
    pub safety: EffectSafety,
    pub record_ref: ObjectRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryView {
    pub id: DeliveryId,
    pub generation: u64,
    pub state: DeliveryState,
    pub result_ref: ObjectRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskView {
    pub id: TaskId,
    pub state: TaskState,
    pub base: Option<ObjectRef>,
    pub attempts: Vec<AttemptView>,
    pub effects: Vec<EffectView>,
    pub deliveries: Vec<DeliveryView>,
    pub requests: Vec<RuntimeRequest>,
    pub frozen_result: Option<ObjectRef>,
    pub terminal_result: Option<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page<T> {
    pub sequence: u64,
    pub rows: Vec<T>,
    pub next: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveTaskViews {
    pub focused: TaskId,
    pub tasks: Page<TaskView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssumptionView {
    pub text_ref: ObjectRef,
    pub evidence_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewView {
    pub state: String,
    pub evidence_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunView {
    pub run_id: RunId,
    pub sequence: u64,
    pub control_revision: OpaqueRevision,
    pub phase: RunPhase,
    pub outcome: RunOutcome,
    pub wait: RunWait,
    pub cleanup: RunCleanup,
    pub allowed_actions: Vec<RunActionKind>,
    pub display_label: String,
    pub current_task: Option<ActiveTaskViews>,
    pub completed: Page<TaskView>,
    pub queued: Page<TaskView>,
    pub assumptions: Page<AssumptionView>,
    pub review_state: ReviewView,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunViewDelta {
    pub view: RunView,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEvent {
    pub run_id: RunId,
    pub sequence: u64,
    pub delta: RunViewDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub run_id: RunId,
    pub projection: RunView,
    pub snapshot_refs: Vec<ObjectRef>,
    pub event_segment_refs: Vec<ObjectRef>,
    pub effect_refs: Vec<ObjectRef>,
    pub request_refs: Vec<ObjectRef>,
    pub delivery_refs: Vec<ObjectRef>,
    pub result_refs: Vec<ObjectRef>,
    pub usage_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIndex {
    pub version: u32,
    pub runs: BTreeMap<RunId, RunRecord>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cumulative: BTreeMap<crate::orchestration_records::RunId, ObjectRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub orchestration: BTreeMap<
        crate::orchestration_records::RunId,
        Vec<crate::orchestration_commit::PublishedRunCut>,
    >,
}
impl Default for RunIndex {
    fn default() -> Self {
        Self {
            version: 1,
            runs: BTreeMap::new(),
            orchestration: BTreeMap::new(),
            cumulative: BTreeMap::new(),
        }
    }
}

pub(crate) fn kind(reference: &ObjectRef, expected: RecordKind) -> Result<(), BundleError> {
    reference.validate()?;
    if reference.record_kind != expected {
        return Err(BundleError::Corrupt("Studio reference kind".into()));
    }
    Ok(())
}
pub(crate) fn schema(version: u32) -> Result<(), BundleError> {
    if version != 1 {
        return Err(BundleError::Unsupported("Studio record schema".into()));
    }
    Ok(())
}
pub(crate) fn bounded<T: Serialize>(value: &T, limit: u64) -> Result<(), BundleError> {
    serde_json::to_writer(&mut crate::store::BoundedCounter { bytes: 0, limit }, value)
        .map_err(|_| BundleError::Capacity("Studio payload bytes"))
}
impl RunRecord {
    #[must_use]
    pub fn references(&self) -> Vec<ObjectRef> {
        let mut refs = Vec::new();
        for items in [
            &self.snapshot_refs,
            &self.event_segment_refs,
            &self.effect_refs,
            &self.request_refs,
            &self.delivery_refs,
            &self.result_refs,
            &self.usage_refs,
        ] {
            refs.extend(items.clone());
        }
        refs.extend(self.projection.references());
        refs
    }
}
impl RunView {
    /// Shape and resource validation only. S4 owns lifecycle and action eligibility.
    pub fn validate(&self) -> Result<(), BundleError> {
        if self.sequence == 0
            || self.control_revision.0.trim().is_empty()
            || self.control_revision.0.len() > 256
            || self.display_label.len() > 16_384
            || self.allowed_actions.len() > 4
        {
            return Err(BundleError::Capacity(
                "run projection identity/label/actions",
            ));
        }
        if self
            .allowed_actions
            .iter()
            .enumerate()
            .any(|(index, action)| self.allowed_actions[..index].contains(action))
            || matches!(&self.wait, RunWait::Children(children) if children.is_empty())
        {
            return Err(BundleError::Corrupt(
                "run projection actions/children".into(),
            ));
        }
        let pages = [&self.completed, &self.queued];
        for page in pages
            .into_iter()
            .chain(self.current_task.iter().map(|a| &a.tasks))
        {
            if page.sequence != self.sequence
                || page.rows.len() > 200
                || page.next.as_ref().is_some_and(|s| s.len() > 256)
            {
                return Err(BundleError::Capacity("run page bound/sequence"));
            }
            if page.rows.iter().any(|task| {
                matches!(&task.state, TaskState::WaitingForChildren(children) if children.is_empty())
            }) {
                return Err(BundleError::Corrupt("run task children".into()));
            }
        }
        if self.assumptions.sequence != self.sequence
            || self.assumptions.rows.len() > 200
            || self
                .assumptions
                .next
                .as_ref()
                .is_some_and(|s| s.len() > 256)
        {
            return Err(BundleError::Capacity("run assumptions page"));
        }
        bounded(self, 8 << 20)
    }
    #[must_use]
    pub fn references(&self) -> Vec<ObjectRef> {
        let mut refs = self.review_state.evidence_refs.clone();
        match &self.outcome {
            RunOutcome::Failed(d) | RunOutcome::Cancelled(d) => {
                refs.extend(d.evidence_refs.clone());
            }
            _ => {}
        }
        match &self.wait {
            RunWait::Files(d) | RunWait::Provider(d) => refs.extend(d.missing_refs.clone()),
            _ => {}
        }
        for assumption in &self.assumptions.rows {
            refs.push(assumption.text_ref.clone());
            refs.extend(assumption.evidence_refs.clone());
        }
        for task in self
            .completed
            .rows
            .iter()
            .chain(&self.queued.rows)
            .chain(self.current_task.iter().flat_map(|a| &a.tasks.rows))
        {
            refs.extend(task.base.clone());
            refs.extend(task.frozen_result.clone());
            refs.extend(task.terminal_result.clone());
            if let TaskState::Terminal(r) = &task.state {
                refs.push(r.clone());
            }
            for attempt in &task.attempts {
                refs.extend(attempt.guardian.clone());
                if let AttemptState::Retired(d) = &attempt.state {
                    refs.extend(d.evidence_refs.clone());
                }
            }
            refs.extend(task.effects.iter().map(|e| e.record_ref.clone()));
            refs.extend(task.deliveries.iter().map(|d| d.result_ref.clone()));
            for request in &task.requests {
                match request {
                    RuntimeRequest::Question(r)
                    | RuntimeRequest::Approval(r)
                    | RuntimeRequest::Secret(r) => refs.push(r.clone()),
                }
            }
        }
        refs
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataScope {
    Feedback,
    Snapshot,
    Orchestration,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataEnvelope {
    pub id: CommandId,
    pub project_id: ProjectId,
    pub target_branch: BranchId,
    pub owner_epoch: u64,
    pub expected_head: Option<HeadId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataUpdate {
    pub expected_root_generation: u64,
    pub change: MetadataChange,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[expect(
    clippy::large_enum_variant,
    reason = "Bounded metadata requests keep the same inline owner and wire DTOs"
)]
pub enum MetadataChange {
    PublishSnapshot(PublishSnapshot),
    SaveFeedback(FeedbackRevision),
    SaveVoice(VoiceRef),
    SubmitBatch {
        batch: FeedbackBatch,
        request: ReviseRequest,
    },
    Orchestration {
        runs: Vec<RunRecord>,
        consume_batch: Option<(FeedbackBatchId, RunId)>,
    },
    #[serde(skip_deserializing)]
    OrchestrationUpdate {
        transition: Box<crate::orchestration_commit::ValidatedRunUpdate>,
        consume_batch: Option<FeedbackBatchId>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MetadataResult {
    Snapshot(SnapshotKey),
    Feedback(ObjectRef),
    Voice(NoteId),
    Batch(FeedbackBatchId),
    Orchestration(Vec<RunId>),
}
impl MetadataResult {
    #[must_use]
    pub fn references(&self) -> Vec<ObjectRef> {
        match self {
            Self::Snapshot(key) => vec![key.input_manifest_ref.clone()],
            Self::Feedback(r) => vec![r.clone()],
            _ => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSnapshotRecord {
    pub version: u32,
    pub view: RunView,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEventSegment {
    pub version: u32,
    pub run_id: RunId,
    pub events: Vec<RunEvent>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEffectRecord {
    pub version: u32,
    pub run_id: RunId,
    pub task_id: TaskId,
    pub effect: EffectView,
    pub payload_refs: Vec<ObjectRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRequestRecord {
    pub version: u32,
    pub run_id: RunId,
    pub task_id: TaskId,
    pub request_id: QuestionId,
    pub request: RuntimeRequest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDeliveryRecord {
    pub version: u32,
    pub run_id: RunId,
    pub task_id: TaskId,
    pub delivery: DeliveryView,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunResultRecord {
    pub version: u32,
    pub run_id: RunId,
    pub task_id: TaskId,
    pub outcome: OutcomeDetail,
    pub base: Option<ObjectRef>,
    pub head: Option<ObjectRef>,
    pub artifacts: Vec<ObjectRef>,
    pub usage_ref: ObjectRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunUsageRecord {
    pub version: u32,
    pub run_id: RunId,
    pub event_id: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cost_microunits: Option<u64>,
    pub coverage: String,
    pub evidence_refs: Vec<ObjectRef>,
}
