//! Project-owned `orchestration@1` contracts for S3 H1 registration.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {Orchestrator,EventSink,CommandReceiptStore,ProjectionStore,EffectOutbox}.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright replaces SQL authority with typed
//! Studio root updates, binds receipts to the complete command scope, and keeps
//! live credentials and callbacks outside serializable project state.
#![allow(clippy::missing_errors_doc)]

use super::{
    bundle_records::{BranchId, ObjectRef, ProjectId, RecordKind, SessionId},
    receipts::{CommandDigest, CommandId},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const ORCHESTRATION_SUPPORT: &str = "orchestration@1";
pub const COMMAND_BYTES: usize = 256 * 1024;
pub const CONTROL_BYTES: usize = 64 * 1024 * 1024;
pub const TASK_LIMIT: usize = crate::run_admission::TASKS;
pub const ITEM_LIMIT: usize = 256;
pub const TASK_FEEDBACK_LIMIT: usize = 256;
pub const TASK_DECISION_LIMIT: usize = 240;
/// First retained fork, history checkpoint, manifest, dispatch and containment intents.
/// Reserved attempts hold this room against every other ordinary context writer.
pub const NATIVE_ATTEMPT_DECISIONS: usize = 5;
pub const TASK_RETAINED_DECISION_LIMIT: usize = 256;
pub const TASK_FEEDBACK_CAPACITY_WORDS: &str =
    "Task feedback context is full. Stop this run and start a new run to supply more feedback.";
pub const TASK_DECISION_CAPACITY_WORDS: &str =
    "Task decision context is full. Stop this run; start a new run for additional context.";
pub const ATTEMPT_LIMIT: usize = 3;
pub const BACKGROUND_LIMIT: u32 = 256;

macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub SessionId);
    )+};
}
identity!(
    RunId,
    AgentThreadId,
    TaskId,
    AttemptId,
    EffectId,
    DeliveryId,
    QuestionId,
    ResultId,
    StableMessageId,
    EndpointId
);

pub use super::editing_session::BranchBinding;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Director,
    Editor,
    Motion,
    Audio,
    Colour,
    Reviewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskScope {
    pub operations: BTreeSet<String>,
    pub objects: BTreeSet<String>,
    pub fields: BTreeSet<String>,
    pub tracks: BTreeSet<String>,
    pub bus_sidechain_closure: BTreeSet<String>,
    pub asset_roots: Vec<ObjectRef>,
    pub base: BranchBinding,
    pub consent_refs: Vec<ObjectRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderOptionRecord {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSelectionRecord {
    pub connector: String,
    pub model: String,
    pub options: Vec<ProviderOptionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBrief {
    pub plan: ObjectRef,
    pub script: ObjectRef,
    pub inputs: ObjectRef,
    pub assignment: ObjectRef,
    pub role: Role,
    pub provider: ProviderSelectionRecord,
    pub scope: TaskScope,
}

/// Nonsecret references only. This is receipt identity, never a live credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBinding {
    pub grant_ref: ObjectRef,
    pub owner_epoch: u64,
    pub endpoint: Option<EndpointId>,
    pub attempt: Option<AttemptId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpaqueRevision(pub SessionId);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationEnvelope {
    pub id: CommandId,
    pub digest: CommandDigest,
    pub project: ProjectId,
    pub expected_generation: u64,
    pub authority: AuthorityBinding,
    pub command: OrchestrationCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRun {
    pub director: TaskBrief,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignTask {
    pub run: RunId,
    pub parent: TaskId,
    pub brief: TaskBrief,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishResult {
    pub task: TaskId,
    pub result: TaskResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinishTask {
    pub task: TaskId,
    pub expected_revision: kinewright_core::TimelineRevision,
}

/// Accepted before either completion side effect. Substep identities never
/// depend on a retry's clock, generation, endpoint or owner incarnation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionIntent {
    pub id: CommandId,
    pub digest: CommandDigest,
    pub attempt: AttemptId,
    pub request: FinishTask,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStep {
    Freeze,
    Publication,
}
impl CompletionIntent {
    pub fn step_id(&self, step: CompletionStep) -> Result<CommandId, Refusal> {
        // Leave room for the issuer cursor after the final publication, too.
        self.id.sequence.checked_add(3).ok_or(Refusal::Capacity)?;
        Ok(CommandId {
            issuer: self.id.issuer.clone(),
            sequence: self.id.sequence
                + match step {
                    CompletionStep::Freeze => 1,
                    CompletionStep::Publication => 2,
                },
        })
    }
    pub fn validate(&self, task: &TaskId) -> Result<(), Refusal> {
        if self.id.sequence == 0
            || &self.request.task != task
            || self.digest
                != task_command_digest(&OrchestrationCommand::FinishTask(self.request.clone()))?
        {
            return Err(Refusal::ScopeConflict);
        }
        self.step_id(CompletionStep::Publication)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerQuestion {
    pub question: QuestionId,
    pub answer: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeTask {
    pub task: TaskId,
    pub target: BranchBinding,
    pub result: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OrchestrationCommand {
    StartRun(Box<StartRun>),
    AssignTask(Box<AssignTask>),
    FreezeTask(TaskId),
    PublishResult(Box<PublishResult>),
    FinishTask(FinishTask),
    ObserveDelivery(DeliveryId),
    AnswerQuestion(AnswerQuestion),
    CancelTask(TaskId),
    MergeTask(MergeTask),
    ControlRun(RunIntent),
    BeginTurn(TaskTurn),
    EndTurn(TaskTurn),
    BackgroundWork(TaskTurn, u32),
    /// Owner settlement after the expired turn's native attempt is proven retired.
    SettleExpiredBackground(TaskTurn),
    Request(RuntimeRequest),
    AdmitQuestionWake(QuestionId),
    ExecuteQuestionWake(QuestionId),
    ExpireSession(SessionId),
    OfferDeliveries(TaskId, AttemptId),
    WaitTimeout(TaskId),
    RecordContext {
        task: TaskId,
        feedback: Vec<ObjectRef>,
        decisions: Vec<ObjectRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIntent {
    pub command_id: CommandId,
    pub run_id: RunId,
    pub expected_control_revision: OpaqueRevision,
    pub action: RunAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[serde(deny_unknown_fields)]
pub struct OutcomeDetail {
    pub code: String,
    pub detail_ref: Option<ObjectRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitDetail {
    pub reason: ObjectRef,
    pub missing: Vec<ObjectRef>,
    pub attempt: Option<AttemptId>,
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
    ProviderSettings(WaitDetail),
    Replan(WaitDetail),
    Question(QuestionId),
    Children(Vec<TaskId>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupIdentity {
    pub attempt: AttemptId,
    pub owner_instance: SessionId,
    pub nonce: SessionId,
    pub recovery_ref: ObjectRef,
}

/// Original Kinewright guardian protocol, not translated T3 Code logic.
/// UUID identities use the existing validated lowercase 32-hex wire encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIdentity {
    pub version: u32,
    pub project: ProjectId,
    pub run: RunId,
    pub task: TaskId,
    pub attempt: AttemptId,
    pub nonce: SessionId,
    pub owner_instance: SessionId,
}

impl ExecutionIdentity {
    pub fn validate(&self) -> Result<(), Refusal> {
        if self.version != 1 {
            return Err(Refusal::Unsupported);
        }
        encoded(self, 1024)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptReservation {
    pub identity: ExecutionIdentity,
    pub owner_epoch: u64,
    pub state: AttemptState,
    pub recovery_ref: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RunCleanup {
    Settled,
    Pending(Vec<CleanupIdentity>),
    Blocked(Vec<CleanupIdentity>, ObjectRef),
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
    CleanupBlocked(CleanupIdentity),
    Retired(AttemptOutcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
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
    CleanupBlocked(CleanupIdentity),
    Terminal(ObjectRef),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "attempt",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectSafety {
    ReplaySafe,
    ProcessBound,
    ReconcileExternal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DeliveryState {
    Pending,
    Offered(AttemptId, StableMessageId),
    Observed(RunId),
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRecord {
    pub id: QuestionId,
    pub task: TaskId,
    pub prompt: ObjectRef,
    pub origin: TaskTurn,
    pub status: QuestionStatus,
    pub wake: QuestionWake,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionStatus {
    Pending,
    Answered(ObjectRef),
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionWake {
    Pending,
    Admitted,
    Executed,
    Disposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnPurpose {
    Work,
    QuestionAttention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTurn {
    pub task: TaskId,
    pub attempt: AttemptId,
    pub session: SessionId,
    pub ordinal: u64,
    pub owner_epoch: u64,
    pub expected_head: super::bundle_records::HeadId,
    pub purpose: TurnPurpose,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DependencyReservation {
    #[default]
    Free,
    Reserved,
    Backed(AttemptId),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskProgress {
    pub capacity: DependencyReservation,
    pub turn: Option<TaskTurn>,
    pub turn_ended: bool,
    pub expired: bool,
    pub background: u32,
    pub feedback: Vec<ObjectRef>,
    pub decisions: Vec<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<Box<CompletionIntent>>,
}

impl TaskProgress {
    pub fn check_context_capacity(&self, feedback: usize, decisions: usize) -> Result<(), Refusal> {
        if self.feedback.len() > TASK_FEEDBACK_LIMIT
            || feedback > TASK_FEEDBACK_LIMIT.saturating_sub(self.feedback.len())
        {
            return Err(Refusal::TaskFeedbackCapacity);
        }
        if self.decisions.len() > TASK_DECISION_LIMIT
            || decisions > TASK_DECISION_LIMIT.saturating_sub(self.decisions.len())
        {
            return Err(Refusal::TaskDecisionCapacity);
        }
        Ok(())
    }
    pub fn check_retained_context(&self) -> Result<(), Refusal> {
        if self.feedback.len() > TASK_FEEDBACK_LIMIT {
            return Err(Refusal::TaskFeedbackCapacity);
        }
        if self.decisions.len() > TASK_RETAINED_DECISION_LIMIT {
            return Err(Refusal::TaskDecisionCapacity);
        }
        Ok(())
    }
    /// Sixteen decision slots are held back from new work for terminal bookkeeping.
    pub fn check_recovery_decisions(&self, additions: usize) -> Result<(), Refusal> {
        self.check_retained_context()?;
        // A new recovery reason must leave room for a first terminal history cut.
        if additions > 0
            && additions > (TASK_RETAINED_DECISION_LIMIT - 1).saturating_sub(self.decisions.len())
        {
            return Err(Refusal::TaskDecisionCapacity);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionReference {
    pub task: TaskId,
    pub attempt: AttemptId,
    pub request_ref: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "detail",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RuntimeRequest {
    Question(QuestionRecord),
    Approval(AttentionReference),
    Secret(AttentionReference),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskResult {
    pub outcome: AttemptOutcome,
    pub base: Option<BranchBinding>,
    pub head: Option<BranchBinding>,
    pub delta_ref: Option<ObjectRef>,
    pub artifacts: Vec<ObjectRef>,
    pub evidence: Vec<ObjectRef>,
    pub usage_ref: ObjectRef,
    pub explanation: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageEventKey {
    pub attempt: AttemptId,
    pub turn: u64,
    pub event: String,
}

/// Categories are optional. Normalization and cumulative/incremental policy belong to C5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvailableUsage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
    pub cost_micros: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCoverage {
    Partial,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageRecord {
    pub version: u32,
    pub identity: UsageEventKey,
    pub categories: AvailableUsage,
    pub coverage: UsageCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecord {
    pub id: TaskId,
    pub parent: Option<TaskId>,
    pub brief: TaskBrief,
    pub state: TaskState,
    #[serde(default)]
    pub progress: TaskProgress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRecord {
    pub id: EffectId,
    pub task: TaskId,
    pub lane: BranchId,
    pub order: u64,
    pub safety: EffectSafety,
    pub state: EffectState,
    pub payload: ObjectRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRecord {
    pub id: DeliveryId,
    pub task: TaskId,
    pub result: ObjectRef,
    pub generation: u64,
    pub wake: EffectId,
    pub message: StableMessageId,
    pub state: DeliveryState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProjection {
    pub run: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_consent: Option<ObjectRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edit_approvals: Vec<ObjectRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edit_applications: Vec<ObjectRef>,
    pub sequence: u64,
    pub control_revision: OpaqueRevision,
    pub phase: RunPhase,
    pub outcome: RunOutcome,
    pub wait: RunWait,
    pub cleanup: RunCleanup,
    pub tasks: Vec<TaskRecord>,
    pub effects: Vec<EffectRecord>,
    pub attempts: Vec<AttemptReservation>,
    pub deliveries: Vec<DeliveryRecord>,
    pub requests: Vec<RuntimeRequest>,
    pub results: Vec<ObjectRef>,
    pub usage: Vec<ObjectRef>,
}

impl RunProjection {
    /// Conservatively protect the whole lifecycle allowance until containment.
    /// This is derived from the durable attempt state, not a second reservation ledger.
    pub fn check_task_context_capacity(
        &self,
        task: &TaskId,
        feedback: usize,
        decisions: usize,
    ) -> Result<(), Refusal> {
        let record = self
            .tasks
            .iter()
            .find(|record| &record.id == task)
            .ok_or(Refusal::ScopeConflict)?;
        let held = if decisions > 0
            && self.attempts.iter().any(|attempt| {
                &attempt.identity.task == task && attempt.state == AttemptState::Reserved
            }) {
            NATIVE_ATTEMPT_DECISIONS
        } else {
            0
        };
        record.progress.check_context_capacity(
            feedback,
            decisions
                .checked_add(held)
                .ok_or(Refusal::TaskDecisionCapacity)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationEvent {
    pub command: CommandId,
    pub sequence: u64,
    pub projection: RunProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSegment {
    pub run: RunId,
    pub first_sequence: u64,
    pub previous: Option<ObjectRef>,
    pub events: Vec<OrchestrationEvent>,
}

/// The wire event binds the full state once; it does not embed another projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventBinding {
    pub command: CommandId,
    pub sequence: u64,
    pub projection_digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentBinding {
    pub run: RunId,
    pub first_sequence: u64,
    pub previous: Option<ObjectRef>,
    pub event: EventBinding,
}
impl EventSegment {
    pub(crate) fn binding(&self) -> Result<SegmentBinding, Refusal> {
        let [event] = self.events.as_slice() else {
            return Err(Refusal::InvalidState);
        };
        if self.first_sequence == 0
            || self.first_sequence != event.sequence
            || event.projection.sequence != event.sequence
            || event.projection.run != self.run
            || (self.first_sequence > 1 && self.previous.is_none())
        {
            return Err(Refusal::InvalidState);
        }
        if let Some(previous) = &self.previous {
            validate_reference(previous)?;
            if previous.record_kind != RecordKind::RunIndex {
                return Err(Refusal::InvalidState);
            }
        }
        Ok(SegmentBinding {
            run: self.run.clone(),
            first_sequence: self.first_sequence,
            previous: self.previous.clone(),
            event: EventBinding {
                command: event.command.clone(),
                sequence: event.sequence,
                projection_digest: kinewright_media::sha256_bytes(&encoded(
                    &event.projection,
                    CONTROL_BYTES,
                )?),
            },
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredEventSegment {
    pub binding: SegmentBinding,
    pub snapshot: ObjectRef,
}
impl StoredEventSegment {
    pub fn restore(&self, projection: &RunProjection) -> Result<EventSegment, Refusal> {
        // A recovered command owns this immutable state independently of the loaded record.
        let segment = EventSegment {
            run: self.binding.run.clone(),
            first_sequence: self.binding.first_sequence,
            previous: self.binding.previous.clone(),
            events: vec![OrchestrationEvent {
                command: self.binding.event.command.clone(),
                sequence: self.binding.event.sequence,
                projection: projection.clone(),
            }],
        };
        if segment.binding()? != self.binding {
            return Err(Refusal::InvalidState);
        }
        Ok(segment)
    }
}

/// Durable identity of one task's registered actor, independent of an attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskHistoryRecord {
    pub project: ProjectId,
    pub run: RunId,
    pub task: TaskId,
    pub attempt: AttemptId,
    pub base: BranchBinding,
    pub source: BranchBinding,
    pub base_ref: ObjectRef,
    /// Legacy lineage hint. Current recovery resolves the checkpoint-bound shared
    /// candidate record from `StudioRoot`, independently of task restore order.
    pub candidate_base_ref: ObjectRef,
    pub frozen: bool,
    pub merge_envelope: Option<crate::receipts::CommandEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_authority: Option<TaskMergeAuthority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_result: Option<TaskResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskMergeAuthority {
    pub caller: ExecutionIdentity,
    pub id: CommandId,
    pub digest: CommandDigest,
}

/// Retired records retain their original identities and native recovery facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredAttemptArchive {
    pub run: RunId,
    pub attempts: Vec<AttemptReservation>,
}

/// All tagged payloads require sealed H1 admission. `RunIndex` is unsupported on main.
/// These types do not admit them as opaque evidence blobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "record",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OrchestrationPayload {
    TaskHistory(Box<TaskHistoryRecord>),
    AttemptArchive(Box<RetiredAttemptArchive>),
    RunIndex(Vec<ObjectRef>),
    Snapshot(Box<RunProjection>),
    EventSegment(StoredEventSegment),
    Effect(EffectRecord),
    Request(RuntimeRequest),
    Delivery(DeliveryRecord),
    Result(Box<TaskResult>),
    Usage(UsageRecord),
    // S4 C5
    Accounting(Box<super::orchestration_accounting::AccountingRecord>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationRecord {
    pub retention: crate::orchestration_retention::Retention,
    pub version: u32,
    pub payload: OrchestrationPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    TaskFeedbackCapacity,
    TaskDecisionCapacity,
    Unsupported,
    Capacity,
    ScopeConflict,
    StaleGeneration,
    StaleControl,
    InvalidState,
    Unknown,
    RetiredUnresolved,
    UnresolvedOrigin,
    PublicationUnknown,
    DependencyCapacity,
    H3AttentionUnavailable,
}

pub fn encoded<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, Refusal> {
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("orchestration capacity"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value).map_err(|_| Refusal::Capacity)?;
    Ok(output.bytes)
}

impl OrchestrationEnvelope {
    /// Digest includes the original epoch, generation, scope, action and object hashes.
    pub fn computed_digest(&self) -> Result<CommandDigest, Refusal> {
        let bytes = encoded(
            &(
                &self.id,
                &self.project,
                self.expected_generation,
                &self.authority,
                &self.command,
            ),
            COMMAND_BYTES,
        )?;
        serde_json::from_value(serde_json::Value::String(kinewright_media::sha256_bytes(
            &bytes,
        )))
        .map_err(|_| Refusal::InvalidState)
    }
    pub fn validate(&self) -> Result<(), Refusal> {
        if self.id.sequence == 0 || self.digest != self.computed_digest()? {
            return Err(Refusal::ScopeConflict);
        }
        if let OrchestrationCommand::ControlRun(intent) = &self.command
            && intent.command_id != self.id
        {
            return Err(Refusal::ScopeConflict);
        }
        encoded(self, COMMAND_BYTES)?;
        Ok(())
    }
    /// Domain separated stable identities from the accepted scoped command.
    pub fn identity<T: for<'de> Deserialize<'de>>(&self, domain: &str) -> Result<T, Refusal> {
        self.validate()?;
        let bytes = encoded(
            &(ORCHESTRATION_SUPPORT, &self.digest, domain),
            COMMAND_BYTES,
        )?;
        serde_json::from_value(serde_json::Value::String(
            kinewright_media::sha256_bytes(&bytes)[..32].to_owned(),
        ))
        .map_err(|_| Refusal::InvalidState)
    }
}

pub fn validate_reference(reference: &ObjectRef) -> Result<(), Refusal> {
    if reference.sha256.len() != 64
        || !reference
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || (reference.byte_length > CONTROL_BYTES as u64 && !reference.record_kind.is_owned_bytes())
    {
        return Err(Refusal::InvalidState);
    }
    if !reference.record_kind.supported() && reference.record_kind != RecordKind::RunIndex {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}

/// Client-known logical identity for receipt reconciliation. Admission controls
/// and host grant fields are outside this digest. Host-owned publication usage
/// and assignment provider policy are omitted; all caller-owned fields remain exact.
pub fn task_command_digest(command: &OrchestrationCommand) -> Result<CommandDigest, Refusal> {
    let mut value = serde_json::to_value(command).map_err(|_| Refusal::InvalidState)?;
    if matches!(command, OrchestrationCommand::PublishResult(_)) {
        value["value"]["result"]
            .as_object_mut()
            .ok_or(Refusal::InvalidState)?
            .remove("usage_ref");
    }
    if matches!(command, OrchestrationCommand::AssignTask(_)) {
        value["value"]["brief"]
            .as_object_mut()
            .ok_or(Refusal::InvalidState)?
            .remove("provider");
    }
    command_value_digest(&value)
}

/// Read-only receipt lookup also accepts the exact pre-specialist assignment
/// digest, which included provider policy. This grants no write admission.
/// # Errors
/// Returns an error if the command cannot fit the bounded canonical encoding.
pub fn task_receipt_digest_matches(
    command: &OrchestrationCommand,
    expected: &CommandDigest,
) -> Result<bool, Refusal> {
    if task_command_digest(command)? == *expected {
        return Ok(true);
    }
    if !matches!(command, OrchestrationCommand::AssignTask(_)) {
        return Ok(false);
    }
    let original = serde_json::to_value(command).map_err(|_| Refusal::InvalidState)?;
    Ok(command_value_digest(&original)? == *expected)
}

fn command_value_digest(value: &serde_json::Value) -> Result<CommandDigest, Refusal> {
    let mut bytes = b"kinewright.task-command.v1\0".to_vec();
    bytes.extend(encoded(value, CONTROL_BYTES)?);
    Ok(CommandDigest(kinewright_media::sha256_bytes(&bytes)))
}
