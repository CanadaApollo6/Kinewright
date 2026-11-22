//! Bounded foreground commands and the C9 accepted manual-history stream.
#![allow(clippy::missing_errors_doc)]
use super::HostAuth;
use kinewright_project::operation_actor::PortableHistory;
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
/// Maximum serialized body at the borrowed-source callback endpoint.
pub(super) const SOURCE_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Every request outside the payload: editor, operation, project, roots and the
/// command's own small fields. A request without a payload is refused above it.
pub const HOST_ENVELOPE_BYTES: usize = 256 * 1024;
/// A full manual window as it crosses HTTP. Each message's raw JSON travels as
/// a JSON string. Valid raw JSON escapes only `"`, `\`, tab, LF and CR, each to
/// two bytes; every other byte stays one. Each message adds two quotes and a comma.
pub const COMMIT_HISTORY_WINDOW_BYTES: u64 = 2
    * kinewright_project::operation_protocol::PENDING_BYTES
    + 3 * kinewright_project::operation_protocol::PENDING_COMMANDS as u64;
/// The `CommitHistory` ingress cap. The codec refuses a window before HTTP can.
pub const COMMIT_HISTORY_BODY_BYTES: u64 = COMMIT_HISTORY_WINDOW_BYTES + HOST_ENVELOPE_BYTES as u64;
const _: () = assert!(
    COMMIT_HISTORY_BODY_BYTES >= COMMIT_HISTORY_WINDOW_BYTES + HOST_ENVELOPE_BYTES as u64
        && COMMIT_HISTORY_BODY_BYTES <= HOST_BODY_BYTES as u64,
    "FULL_MANUAL_WINDOW_CROSSES_HTTP"
);
/// One snapshot's wire bytes outside its transport body: role, sizing,
/// identity, branch, revision and `history_sha256`, with their JSON syntax.
pub const SNAPSHOT_WIRE_HEADER_BYTES: u64 = 4096;
/// Split the snapshot budget after reserving room for the envelope and
/// `other` payload bytes. The client checks the serialized request separately.
const fn snapshot_door_bytes(snapshots: u64, other: u64) -> u64 {
    (HOST_BODY_BYTES as u64 - HOST_ENVELOPE_BYTES as u64 - other) / snapshots
        - SNAPSHOT_WIRE_HEADER_BYTES
}
const fn snapshot_door_fits(cap: u64, snapshots: u64, other: u64) -> bool {
    HOST_ENVELOPE_BYTES as u64 + other + snapshots * (cap + SNAPSHOT_WIRE_HEADER_BYTES)
        <= HOST_BODY_BYTES as u64
}
/// `Save`, `SaveAs`, `FirstSave`, `PreviewRescue`, `PreviewRecovery`,
/// `RefreshUnsavedInputs`, `PrepareOwnedMedia` and
/// `PrepareMediaInterpretation` carry one snapshot.
pub const SNAPSHOT_DOOR_BYTES: u64 = snapshot_door_bytes(1, 0);
/// `BeginRecovery` carries one frozen snapshot and the pending manual window.
pub const BEGIN_RECOVERY_SNAPSHOT_BYTES: u64 = snapshot_door_bytes(1, COMMIT_HISTORY_WINDOW_BYTES);
/// `PublishBarrier` carries the barrier's two snapshots.
pub const PUBLISH_BARRIER_SNAPSHOT_BYTES: u64 = snapshot_door_bytes(2, 0);
/// `ConfirmBarrier` carries the barrier's two snapshots and the installed one.
pub const CONFIRM_BARRIER_SNAPSHOT_BYTES: u64 = snapshot_door_bytes(3, 0);
const _: () = assert!(
    snapshot_door_fits(SNAPSHOT_DOOR_BYTES, 1, 0)
        && snapshot_door_fits(
            BEGIN_RECOVERY_SNAPSHOT_BYTES,
            1,
            COMMIT_HISTORY_WINDOW_BYTES
        )
        && snapshot_door_fits(PUBLISH_BARRIER_SNAPSHOT_BYTES, 2, 0)
        && snapshot_door_fits(CONFIRM_BARRIER_SNAPSHOT_BYTES, 3, 0),
    "SNAPSHOT_DOORS_CROSS_HTTP"
);
/// The snapshots one command carries and the escaped transport bytes each may take.
pub(crate) struct SnapshotDoor<'a> {
    pub cap: u64,
    pub snapshots: [Option<&'a HostSnapshot>; 3],
}
impl HostCommand {
    /// Maximum serialized request size at the foreground host endpoint.
    #[must_use]
    pub fn ingress_bytes(&self) -> usize {
        if matches!(self, Self::CommitHistory { .. }) {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the const assertion bounds this value by HOST_BODY_BYTES, a usize"
            )]
            let limit = COMMIT_HISTORY_BODY_BYTES as usize;
            limit
        } else if self.snapshot_door().is_some() {
            HOST_BODY_BYTES
        } else {
            HOST_ENVELOPE_BYTES
        }
    }

    pub(super) fn check_request_capacity(
        &self,
        bytes: usize,
        limit: usize,
    ) -> Result<(), HostRefusal> {
        if bytes <= limit {
            return Ok(());
        }
        let code = if self.snapshot_door().is_some() {
            "snapshot_capacity"
        } else {
            "request_capacity"
        };
        Err(HostRefusal::body_capacity(code, bytes, limit))
    }

    /// `None` for a command that carries no snapshot.
    pub(crate) fn snapshot_door(&self) -> Option<SnapshotDoor<'_>> {
        let (cap, snapshots) = match self {
            Self::Save { snapshot }
            | Self::SaveAs { snapshot, .. }
            | Self::FirstSave { snapshot, .. }
            | Self::PreviewRescue { snapshot }
            | Self::PreviewRecovery { snapshot, .. }
            | Self::RefreshUnsavedInputs { snapshot, .. }
            | Self::PrepareOwnedMedia { snapshot, .. }
            | Self::PrepareMediaInterpretation { snapshot, .. } => {
                (SNAPSHOT_DOOR_BYTES, [Some(snapshot), None, None])
            }
            Self::BeginRecovery { frozen, .. } => {
                (BEGIN_RECOVERY_SNAPSHOT_BYTES, [frozen.as_ref(), None, None])
            }
            Self::PublishBarrier { barrier } => (
                PUBLISH_BARRIER_SNAPSHOT_BYTES,
                [Some(&barrier.before), Some(&barrier.after), None],
            ),
            Self::ConfirmBarrier { barrier, installed } => (
                CONFIRM_BARRIER_SNAPSHOT_BYTES,
                [Some(&barrier.before), Some(&barrier.after), Some(installed)],
            ),
            _ => return None,
        };
        Some(SnapshotDoor { cap, snapshots })
    }
    /// Refuses a snapshot over its share before serializing the full request.
    pub(crate) fn check_snapshot_capacity(&self) -> Result<(), HostRefusal> {
        let Some(door) = self.snapshot_door() else {
            return Ok(());
        };
        for snapshot in door.snapshots.into_iter().flatten() {
            let bytes = snapshot.history.snapshot.escaped_body_bytes();
            if bytes > door.cap {
                return Err(HostRefusal::new(
                    "snapshot_capacity",
                    &format!(
                        "this history's snapshot is {bytes} escaped bytes, and this request carries at most {} escaped bytes per snapshot",
                        door.cap
                    ),
                ));
            }
        }
        Ok(())
    }
}
pub const HOST_COMMAND_CAPACITY: usize = 64;
pub const RETENTION_LOST: &str = "The host could not re-establish this session's retained data. Your edits remain here. Saving and rescue are paused, including saving elsewhere. Close this editing context explicitly to end it.";
pub const RECOVERY_REOPEN: &str = "Your edits so far are saved, and this session is read-only. Close and reopen the project to continue.";

/// Resolved only from a privately issued attachment, never from request fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentRole {
    Viewer,
    Editor,
    Producer,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReadiness {
    Ready,
    #[default]
    Blocked,
    Stopping,
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
    /// Application data root; `None` resolves per-user storage when media is used.
    /// Tests supply an explicit fixture root.
    pub data_dir: Option<PathBuf>,
    pub project: String,
    pub path: Option<PathBuf>,
    pub roots: SessionRoots,
    pub origin: HostOrigin,
    /// Where S4 reads the person's Director setting. `None` is the per-user
    /// scheduler folder; tests name their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler_settings: Option<PathBuf>,
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
        initial_snapshot: Option<HostSnapshot>,
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
    pub history: PortableHistory,
    pub history_sha256: String,
}
impl HostSnapshot {
    pub fn capture(
        core: &kinewright_project::operation_actor::Core,
    ) -> Result<Self, super::HostError> {
        Self::from_history(
            core.portable_history()
                .map_err(|e| super::HostError::Protocol(e.to_string()))?,
        )
    }
    pub fn from_history(history: PortableHistory) -> Result<Self, super::HostError> {
        let history_sha256 = history.identity().to_string();
        Ok(Self {
            history,
            history_sha256,
        })
    }
    /// Cheap envelope check only. All payloads cross `SnapshotTransport`'s
    /// lease admission and complete semantic validation before they can install.
    pub(crate) fn verify(&self) -> Result<(), super::HostError> {
        if self.history.identity().to_string() != self.history_sha256 {
            return Err(
                HostRefusal::new("history_changed", "combined snapshot identity differs").into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod snapshot_capacity_tests {
    #[test]
    fn host_snapshot_forged_cursor_counts_refuse_before_adoption() {
        let core = kinewright_project::operation_actor::Core::for_test(
            kinewright_core::Document::default(),
        )
        .unwrap();
        let before = core.snapshot_identity().unwrap();
        let portable = core.portable_history().unwrap();
        let mut wire = serde_json::to_value(&portable).unwrap();
        let mut body: serde_json::Value =
            serde_json::from_str(wire["snapshot"]["body"].as_str().unwrap()).unwrap();
        body["history"]["position"] = serde_json::json!(u64::MAX);
        wire["snapshot"]["body"] = serde_json::json!(serde_json::to_string(&body).unwrap());
        let forged: kinewright_project::operation_actor::PortableHistory =
            serde_json::from_value(wire).unwrap();
        let error = forged
            .validate(&kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ))
            .err()
            .expect("forged transport refused");
        assert!(
            matches!(
                error,
                kinewright_project::operation_history::Error::InvalidRecord
                    | kinewright_project::operation_history::Error::IdentityMismatch
                    | kinewright_project::operation_history::Error::History(_)
            ),
            "FORGED_POSITION_REFUSES_BEFORE_ADOPTION: {error:?}"
        );
        assert_eq!(
            core.snapshot_identity().unwrap(),
            before,
            "FORGED_CAPTURE_CANNOT_CHANGE_SOURCE"
        );
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
    pub acknowledged: Vec<kinewright_project::operation_protocol::Receipt>,
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
    /// Project-read recovery route; never authorizes a task mutation.
    ReconcileTaskReceipt {
        query: super::receipt_reconciliation::ReceiptQuery,
    },
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
    /// S4: start the Director run for one submitted batch, exactly once per
    /// request. Replies with `revise_result`.
    ReviseCut {
        request: kinewright_project::studio_records::ReviseRequest,
    },
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
    VoiceReceipt {
        upload: kinewright_project::voice_ingress::VoiceUpload,
    },
    AcknowledgeVoice {
        upload: kinewright_project::voice_ingress::VoiceUpload,
    },
    StartTranscription {
        note: kinewright_project::studio_records::NoteId,
    },
    VoiceStatus {
        note: kinewright_project::studio_records::NoteId,
    },
    ChooseTranscript {
        envelope: kinewright_project::studio_records::MetadataEnvelope,
        note: kinewright_project::studio_records::NoteId,
        text_ref: ObjectRef,
    },
    ReadVoiceRange {
        revision: ObjectRef,
        start_micros: u64,
        end_micros: u64,
    },
    LeaseSnapshot {
        key: kinewright_project::studio_records::SnapshotKey,
    },
    /// Lease the current edit's input graph for Current-edit playback. Only
    /// the host's current inputs can be leased (S3 C11).
    LeaseCurrentEdit {
        inputs: ObjectRef,
    },
    SnapshotLeaseStatus {
        id: String,
        payload_sha256: String,
    },
    ReleaseSnapshot {
        id: String,
        payload_sha256: String,
    },
    ReadFeedbackView {
        revision: ObjectRef,
    },
    ReadFeedbackEvidence {
        revision: ObjectRef,
        evidence: ObjectRef,
    },
    ListVersions,
    ReadEditorial {
        version: Option<kinewright_project::studio_records::SnapshotKey>,
    },
    Review(kinewright_project::review_job::ReviewControl),
    DecideRulebook {
        decision: kinewright_project::editorial::RulebookDecision,
    },
    ReadSnapshot {
        key: kinewright_project::studio_records::SnapshotKey,
    },
    ReadFeedback {
        reference: ObjectRef,
    },
    /// Map one indexed note onto a version or the saved current edit. Read-only;
    /// nothing is kept, so a reopened App asks again (S3 C10).
    ResolveFeedback {
        revision: ObjectRef,
        onto: MapTarget,
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
        pending: Vec<kinewright_project::operation_protocol::EncodedMessage>,
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
        messages: Vec<kinewright_project::operation_protocol::EncodedMessage>,
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
const MEDIA_PROBE_TIMED_OUT: &str = "media_probe_timed_out";

impl HostRefusal {
    #[must_use]
    pub fn is_media_probe_timeout(&self) -> bool {
        self.code == MEDIA_PROBE_TIMED_OUT
    }

    /// These refusals are issued before dispatch, so recovery can safely resend the request.
    #[must_use]
    pub fn retry_after_recovery(&self) -> bool {
        matches!(
            self.code.as_str(),
            "recovery_frozen"
                | "startup_wait_busy"
                | "startup_wait_failed"
                | "startup_wait_expired"
        )
    }
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.code == "host_busy"
    }
    pub(crate) fn busy() -> Self {
        Self::new(
            "host_busy",
            "The host is busy. This request has not been dispatched.",
        )
    }
    pub(crate) fn stopping() -> Self {
        Self::new(
            "host_stopping",
            "The host is stopping. Your request has not been dispatched.",
        )
    }
    #[must_use]
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            native_code: None,
        }
    }
    pub(super) fn body_capacity(code: &str, bytes: usize, limit: usize) -> Self {
        Self::new(
            code,
            &format!("this request is {bytes} bytes and may carry at most {limit} bytes"),
        )
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
            BundleError::Operation(kinewright_project::operation_actor::Error::History(
                kinewright_project::operation_history::Error::PoolFull(_)
                | kinewright_project::operation_history::Error::Reservation,
            )) => "history_backpressure",
            BundleError::Operation(kinewright_project::operation_actor::Error::Protocol(_)) => {
                "history_conflict"
            }
            BundleError::Operation(_) => "history_refused",
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
            BundleError::RunLimit | BundleError::RunAdmission(_) => "run_limit",
            BundleError::Unsupported(_) => "unsupported_bundle",
            BundleError::MediaProbeTimedOut { .. } => MEDIA_PROBE_TIMED_OUT,
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
    pub path: PathBuf,
    pub history_sha256: String,
    pub root_sha256: String,
    pub generation: u64,
    pub head_id: HeadId,
}
impl From<&BundleSaveReport> for HostSaveReport {
    fn from(r: &BundleSaveReport) -> Self {
        Self {
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
    pub recovery_readiness: RecoveryReadiness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_runtime_status: Option<String>,
    #[serde(default)]
    pub project_id: Option<kinewright_project::bundle_records::ProjectId>,
    #[serde(default)]
    pub continuation_runs: usize,
    /// JA-1 S6 C3: the review job is Queued, Encoding, Checking or
    /// `ReadyToSeal`. Closing the window detaches instead of shutting down.
    #[serde(default)]
    pub review_active: bool,
    /// JA-1 S6 C2: the running or last save of a sealed cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_save: Option<kinewright_project::review_job::delivery::SealedSaveStatus>,
    /// The owner epoch and input manifest Current-edit playback leases. A
    /// change means the GUI renews its lease (S3 C11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_scope: Option<(u64, ObjectRef)>,
    /// The manual branch's saved checkpoint, which Current-edit mappings are
    /// made on. A change means those mappings are recomputed (S3 C11 fu1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_checkpoint: Option<ObjectRef>,
    pub choice: Option<HostChoiceKind>,

    pub rescue_omission_required: bool,
    pub rescue_omission_accepted: bool,
    pub admitted_roots: SessionRoots,
    pub association: Option<PathBuf>,
    pub saved_digest: Option<String>,
    pub saved_position: Option<kinewright_core::undo::CommandKey>,
    pub frozen: bool,
    pub retention_proven: bool,
    pub prepared_selection: bool,
    pub unresolved: bool,
    pub recovery_points: Vec<HostRecoverySelection>,
    pub branches: Vec<(BranchId, HeadId, Vec<String>)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_task_work: Vec<kinewright_project::editing_session::RetainedTaskWork>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_task_failures: Vec<kinewright_project::editing_session::RetainedWorkFailure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_task_branches: Vec<BranchId>,
    pub retirement_candidates: Vec<BranchId>,
    pub destinations: Vec<(PathBuf, HostPublication)>,
    pub close_disclosure: String,
    pub destination_change: Option<String>,
    pub selection_availability: String,
    pub save_disclosure: String,
    pub message: String,

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
    pub history: Option<PortableHistory>,
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
/// What a note is mapped onto: a published version, or the manual branch's
/// saved checkpoint. The host chooses the checkpoint; the App never names one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "onto", rename_all = "snake_case", deny_unknown_fields)]
pub enum MapTarget {
    Version {
        key: kinewright_project::studio_records::SnapshotKey,
    },
    CurrentEdit,
}
/// One note's mapping, as `ProjectBundle::resolve_feedback` computes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackMapping {
    pub resolution: kinewright_project::studio_records::CommentResolution,
    /// Candidate frames on the target, at most the mapper's 512.
    pub candidates: Vec<(kinewright_core::ClipId, i64)>,
    pub original_frame: i64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_receipt: Option<super::receipt_reconciliation::ReceiptReply>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<kinewright_project::review_job::ReviewStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_annotations: Option<kinewright_project::review_job::ReviewAnnotationFields>,
    /// The seal receipt of the version, or of the seal this command made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_receipt: Option<kinewright_project::review_job::ReviewReceipt>,
    pub snapshot_lease: Option<kinewright_project::store::snapshot_reader::SnapshotLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_edit_lease: Option<kinewright_project::store::snapshot_reader::CurrentEditLease>,
    pub evidence_bytes: Option<Vec<u8>>,
    pub voice: Option<super::voice::VoiceStatus>,
    pub voice_audio: Option<Vec<u8>>,
    pub voice_receipt: Option<super::voice::VoiceReceipt>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editorial_audit: Option<kinewright_project::editorial::EditorialAudit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rulebook_proposal: Option<(
        kinewright_project::bundle_records::ObjectRef,
        kinewright_project::editorial::RulebookProposal,
    )>,
    pub feedback: Option<kinewright_project::studio_records::FeedbackRevision>,
    /// `ReadFeedback`: the revision's exact text and strokes evidence, so a
    /// GUI whose bundle the host holds can show the saved note (S3 C7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_text: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_strokes: Option<Vec<u8>>,
    pub feedback_evidence: Option<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_mapping: Option<FeedbackMapping>,
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
    pub manual_lease: Option<kinewright_project::operation_protocol::Lease>,
    pub manual_inputs: Option<kinewright_project::owned_inputs::OwnedInputManifest>,
    pub manual_ack: Vec<kinewright_project::operation_protocol::Receipt>,
    /// When present, `error` refused the message at `manual_ack.len()`.
    /// This counts the messages after it that the host did not attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_unattempted: Option<usize>,
    pub activation: Option<HostActivation>,
    pub status: HostStatus,
    pub saved: Option<HostSaveReport>,
    pub history: Option<PortableHistory>,
    pub session: Option<HostSessionStatus>,
    pub error: Option<HostRefusal>,
}
