//! Bounded foreground commands and the C9 accepted manual-history stream.
#![allow(clippy::missing_errors_doc)]
use super::HostAuth;
use kinewright_core::HistorySnapshot;
use kinewright_project::{
    bundle::{BundleError, BundleSaveReport, PublicationState},
    bundle_records::{BranchId, HeadId, ObjectRef, ProjectId},
    bundle_recovery::RecoverySelection,
    source_boundary::BorrowedSourceDescriptor,
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf};

pub const HOST_PROTOCOL: u32 = 1;
pub const HOST_BODY_BYTES: usize = 40 * 1024 * 1024;
pub const HOST_COMMAND_CAPACITY: usize = 64;
pub const RETENTION_LOST: &str = "The host could not re-establish this session's retained data. Your edits remain here. Saving and rescue are paused, including saving elsewhere. Close this editing context explicitly to end it.";

/// Resolved only from a privately issued attachment, never from request fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentRole {
    Viewer,
    Editor,
    Producer,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRoots {
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub project: String,
    pub path: Option<PathBuf>,
    pub roots: SessionRoots,
    pub origin: HostOrigin,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "Bounded attachment wire values stay inline to preserve the shared App and host API"
)]
pub enum HostOrigin {
    Headless,
    Gui {
        retention: HostSourceBinding,
        initial_snapshot: HostSnapshot,
        expected_root_sha256: Option<String>,
        expected_project_id: Option<ProjectId>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSourceBinding {
    pub source: Option<BorrowedSourceDescriptor>,
    pub inputs: Option<ObjectRef>,
    pub callback: HostBootstrap,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostDestination {
    pub path: PathBuf,
    pub parent_device: u64,
    pub parent_inode: u64,
}
/// Public discovery contains no credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostEndpoint {
    pub address: SocketAddr,
    pub project: String,
    pub protocol: u32,
}
/// Sent only through the private bootstrap pipe or an authenticated request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostBootstrap {
    pub endpoint: HostEndpoint,
    pub auth: HostAuth,
    pub roots: SessionRoots,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSnapshot {
    pub history: HistorySnapshot,
    pub history_sha256: String,
}
impl HostSnapshot {
    pub fn capture(core: &kinewright_core::Core) -> Result<Self, super::HostError> {
        let history = core
            .export_history()
            .map_err(|e| super::HostError::Protocol(e.to_string()))?;
        Self::from_history(history)
    }
    pub fn from_history(history: HistorySnapshot) -> Result<Self, super::HostError> {
        let history_sha256 = kinewright_media::sha256_bytes(&serde_json::to_vec(&history)?);
        Ok(Self {
            history,
            history_sha256,
        })
    }
    pub(crate) fn verify(&self) -> Result<(), super::HostError> {
        if kinewright_media::sha256_bytes(&serde_json::to_vec(&self.history)?)
            != self.history_sha256
        {
            return Err(HostRefusal::new(
                "history_changed",
                "captured history digest does not match its bytes",
            )
            .into());
        }
        if self
            .history
            .encoded_bytes()
            .map_err(|e| super::HostError::Protocol(e.to_string()))?
            > kinewright_project::storage_budget::StorageLimits::default().branch_history_bytes
        {
            return Err(BundleError::Capacity("foreground history snapshot").into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostRecoverySelection {
    WorkingBranch { branch: BranchId },
    Checkpoint { root: ObjectRef, branch: BranchId },
}
impl From<RecoverySelection> for HostRecoverySelection {
    fn from(value: RecoverySelection) -> Self {
        match value {
            RecoverySelection::WorkingBranch(branch) => Self::WorkingBranch { branch },
            RecoverySelection::Checkpoint { root, branch } => Self::Checkpoint { root, branch },
        }
    }
}
impl From<HostRecoverySelection> for RecoverySelection {
    fn from(value: HostRecoverySelection) -> Self {
        match value {
            HostRecoverySelection::WorkingBranch { branch } => Self::WorkingBranch(branch),
            HostRecoverySelection::Checkpoint { root, branch } => Self::Checkpoint { root, branch },
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSessionConfig {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub max_turns: Option<u32>,
}
/// Runtime plan only. Its durable identity/result are the ordinary envelope and receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryBarrier {
    pub switch_generation: u64,
    pub client: String,
    pub envelope: kinewright_project::receipts::CommandEnvelope,
    pub checkpoint_ref: ObjectRef,
    pub before: HostSnapshot,
    pub after: HostSnapshot,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "The bounded foreground wire command owns its snapshot and source binding without another boxed API layer"
)]
pub enum HostCommand {
    IssueMetadataIssuer {
        issuer: Option<kinewright_project::receipts::IssuerId>,
    },
    DispatchOrchestration {
        request: Box<super::ScopedOrchestrationRequest>,
    },
    ObserveRun {
        run: kinewright_project::orchestration_records::RunId,
        after_sequence: u64,
    },
    ResolveAttention {
        response: super::LiveAttentionResponse,
    },
    DetachForContinuation,
    Metadata {
        envelope: kinewright_project::studio_records::MetadataEnvelope,
        update: kinewright_project::studio_records::MetadataUpdate,
    },
    /// S3 C3: seal one bounded typed/drawn feedback evidence object for a later
    /// `SaveFeedback`. The attachment keeps its pin until detach.
    AdmitFeedbackEvidence {
        kind: kinewright_project::studio_records::FeedbackEvidenceKind,
        bytes: Vec<u8>,
    },
    /// S3 C3: the person's private feedback index (saved revisions and batches).
    ListFeedback,
    ListVersions,
    ReadSnapshot {
        key: kinewright_project::studio_records::SnapshotKey,
    },
    ReadFeedback {
        reference: ObjectRef,
    },
    ReadSubmittedBatch {
        batch: kinewright_project::studio_records::FeedbackBatchId,
    },
    PrepareSwitch {
        target: kinewright_project::bundle_switch::SwitchTarget,
    },
    Switch {
        envelope: Box<kinewright_project::receipts::CommandEnvelope>,
    },
    SwitchReceipt {
        envelope: Box<kinewright_project::receipts::CommandEnvelope>,
    },
    CurrentAttachment,
    ConfirmSwitch {
        client: String,
        installed: Box<kinewright_project::bundle_switch::SwitchAttachmentIdentity>,
    },
    BeginRecovery {
        client: String,
        frozen: Option<HostSnapshot>,
        pending: Vec<kinewright_core::AcceptedHistory>,
        discard_batches: Option<usize>,
    },
    PublishBarrier {
        barrier: Box<RecoveryBarrier>,
    },
    ConfirmBarrier {
        barrier: Box<RecoveryBarrier>,
        installed: HostSnapshot,
    },
    AcquireManual {
        client: String,
    },
    PrepareOwnedMedia {
        client: String,
        path: PathBuf,
        asset_id: kinewright_core::AssetId,
        snapshot: HostSnapshot,
    },
    PrepareMediaInterpretation {
        client: String,
        operation: kinewright_core::Operation,
        snapshot: HostSnapshot,
    },
    RefreshUnsavedInputs {
        binding: HostSourceBinding,
        snapshot: HostSnapshot,
    },
    CommitHistory {
        client: String,
        messages: Vec<kinewright_core::AcceptedHistory>,
    },
    Attach,
    AdmitDestination {
        destination: PathBuf,
    },
    Status,
    ReadHistory,
    Save {
        snapshot: HostSnapshot,
    },
    SaveAs {
        destination: PathBuf,
        snapshot: HostSnapshot,
    },
    FirstSave {
        destination: PathBuf,
        snapshot: HostSnapshot,
        source: BorrowedSourceDescriptor,
        inputs: ObjectRef,
        callback: HostBootstrap,
    },
    Retry,
    PreviewRescue {
        snapshot: HostSnapshot,
    },
    ConsentTrim,
    ConsentOmission,
    RescueTo {
        destination: PathBuf,
    },
    Cancel,
    PreviewRecovery {
        target: HostRecoverySelection,
        snapshot: HostSnapshot,
    },
    OpenRecovery,
    Abandon,
    PreviewRetirement {
        branch: BranchId,
    },
    RemoveBranch,
    PreviewForget,
    Forget,
    Reconcile {
        destination: PathBuf,
    },
    PreviewRelease {
        destination: PathBuf,
    },
    Release,
    SaveSession {
        branch: BranchId,
    },
    CloseSession {
        branch: BranchId,
    },
    StartScripted {
        turns: Vec<crate::ScriptedTurn>,
        prompts: Vec<String>,
    },
    ReopenScripted {
        branch: BranchId,
        base: ObjectRef,
        turns: Vec<crate::ScriptedTurn>,
        prompts: Vec<String>,
    },
    StartSession {
        harness: String,
        config: HostSessionConfig,
    },
    SendMessage {
        session: u64,
        message: String,
    },
    InterruptSession {
        session: u64,
    },
    SessionStatus {
        session: u64,
    },
    Detach,
    Shutdown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    pub protocol: u32,
    pub project: String,
    pub roots: SessionRoots,
    pub command: HostCommand,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostRefusal {
    pub code: String,
    pub message: String,
    pub native_code: Option<i32>,
}
impl HostRefusal {
    #[must_use]
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            native_code: None,
        }
    }
    pub(crate) fn roots(message: &str) -> Self {
        Self::new("root_refused", message)
    }
}
impl std::fmt::Display for HostRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for HostRefusal {}
impl From<BundleError> for HostRefusal {
    fn from(value: BundleError) -> Self {
        let code = match &value {
            BundleError::RescueMissingOwnedBytes(_) => "rescue_missing_owned_bytes",
            BundleError::CoreMutationRefused(_) => "core_mutation_refused",
            BundleError::Io(_) => "io",
            BundleError::Identity(_) => "identity_refused",
            BundleError::Contention(_) => "contention",
            BundleError::DurabilityUnsupported => "durability_unsupported",
            BundleError::DurabilityUnknown | BundleError::DurabilityUnknownAt(_) => {
                "durability_unknown"
            }
            BundleError::HistoryChanged => "history_changed",
            BundleError::SaveInProgress => "save_in_progress",
            BundleError::HistoryBarrierInvariant => "history_barrier_invariant",
            BundleError::HistoryCommitInvariant => "history_commit_invariant",
            BundleError::CleanupPending(_) => "cleanup_pending",
            BundleError::Capacity(_) => "storage_backpressure",
            BundleError::Unsupported(_) => "unsupported_bundle",
            BundleError::Corrupt(_) => "damaged_bundle",
            BundleError::Unsettled => "unsettled",
        };
        Self {
            code: code.into(),
            message: value.to_string(),
            native_code: match value {
                BundleError::Io(ref e) => e.raw_os_error(),
                _ => None,
            },
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSaveReport {
    pub history_floor_offset: Option<u64>,
    pub removed_undo: u64,
    pub floor_disclosure: String,
    pub path: PathBuf,
    pub history_sha256: String,
    pub root_sha256: String,
    pub generation: u64,
    pub head_id: HeadId,
}
impl From<&BundleSaveReport> for HostSaveReport {
    fn from(r: &BundleSaveReport) -> Self {
        Self {
            history_floor_offset: None,
            removed_undo: 0,
            floor_disclosure: String::new(),
            path: r.path.clone(),
            history_sha256: r.history_sha256.clone(),
            root_sha256: r.root_sha256.clone(),
            generation: r.generation,
            head_id: r.head_id.clone(),
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPublication {
    NeverAttempted,
    DefinitelyRefused,
    Unknown,
    Committed,
}
impl From<PublicationState> for HostPublication {
    fn from(value: PublicationState) -> Self {
        match value {
            PublicationState::NeverAttempted => Self::NeverAttempted,
            PublicationState::DefinitelyRefused => Self::DefinitelyRefused,
            PublicationState::Unknown => Self::Unknown,
            PublicationState::Committed => Self::Committed,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostChoiceKind {
    Recovery,
    Rescue,
    Remove,
    Forget,
    Release,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent wire observations and explicit consent flags reflect the existing authoritative owner states"
)]
pub struct HostStatus {
    #[serde(default)]
    pub continuation_runs: usize,
    pub choice: Option<HostChoiceKind>,
    pub rescue_trim_required: bool,
    pub rescue_trim_accepted: bool,
    pub rescue_omission_required: bool,
    pub rescue_omission_accepted: bool,
    pub admitted_roots: SessionRoots,
    pub association: Option<PathBuf>,
    pub saved_digest: Option<String>,
    pub frozen: bool,
    pub retention_proven: bool,
    pub prepared_selection: bool,
    pub unresolved: bool,
    pub recovery_points: Vec<HostRecoverySelection>,
    pub branches: Vec<(BranchId, HeadId, Vec<String>)>,
    pub retirement_candidates: Vec<BranchId>,
    pub destinations: Vec<(PathBuf, HostPublication)>,
    pub close_disclosure: String,
    pub destination_change: Option<String>,
    pub selection_availability: String,
    pub save_disclosure: String,
    pub message: String,
    pub rescue_removed_undo: u64,
    pub rescue_omissions: Vec<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostSessionStatus {
    pub branch: Option<BranchId>,
    pub base: Option<ObjectRef>,
    pub id: u64,
    pub running: bool,
    pub turns: u32,
    pub events: Vec<HostAgentEvent>,
    pub overflowed: bool,
    pub stop: Option<String>,
    pub history: Option<HistorySnapshot>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostAgentEvent {
    pub kind: String,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostActivationKind {
    Rescue,
    Recovery,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostActivation {
    pub kind: HostActivationKind,
    pub history_sha256: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostReply {
    pub metadata_issuer: Option<super::MetadataIssuer>,
    pub orchestration: Option<kinewright_project::orchestration_commit::OrchestrationReceipt>,
    pub run_view: Option<super::RunViewPage>,
    pub attention_outcome: Option<super::AttentionOutcome>,
    pub detached: Option<super::DetachOutcome>,
    #[serde(default)]
    pub attention: Vec<super::AttentionPrompt>,
    pub metadata: Option<kinewright_project::receipts::CommandReceipt>,
    pub revise_result: Option<kinewright_project::studio_records::ReviseResult>,
    #[serde(default)]
    pub submitted_revisions: Vec<kinewright_project::studio_records::FeedbackRevision>,
    #[serde(default)]
    pub versions: Vec<kinewright_project::studio_records::SnapshotKey>,
    pub snapshot: Option<kinewright_project::studio_records::SnapshotDescriptor>,
    pub feedback: Option<kinewright_project::studio_records::FeedbackRevision>,
    pub feedback_evidence: Option<ObjectRef>,
    pub feedback_index: Option<kinewright_project::studio_records::FeedbackIndex>,
    pub submitted_batch: Option<kinewright_project::studio_records::FeedbackBatch>,
    #[serde(default)]
    pub unsaved_inputs_refreshed: bool,
    pub owned_media: Option<kinewright_core::MediaAsset>,
    pub owned_media_edit: Option<kinewright_core::Operation>,
    pub switch_preview: Option<kinewright_project::bundle_switch::SwitchPayload>,
    pub switch_envelope: Option<kinewright_project::receipts::CommandEnvelope>,
    pub switch_result: Option<kinewright_project::bundle_switch::SwitchResult>,
    pub switch_attachment: Option<kinewright_project::bundle_switch::SwitchAttachment>,
    pub recovery: Option<RecoveryBarrier>,
    pub barrier_checkpoint: Option<kinewright_project::bundle_records::BranchCheckpoint>,
    pub manual_lease: Option<kinewright_project::manual_history::ManualLease>,
    pub manual_inputs: Option<kinewright_project::owned_inputs::OwnedInputManifest>,
    pub manual_ack: Vec<kinewright_core::AcceptedHistory>,
    pub activation: Option<HostActivation>,
    pub status: HostStatus,
    pub saved: Option<HostSaveReport>,
    pub history: Option<HistorySnapshot>,
    pub session: Option<HostSessionStatus>,
    pub error: Option<HostRefusal>,
}
