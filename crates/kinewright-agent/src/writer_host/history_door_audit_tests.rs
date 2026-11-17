//! Agent-facing ingress cannot choose or install a person's snapshot.
use super::*;
use kinewright_project::{bundle_records::ObjectRef, source_boundary::BorrowedSourceDescriptor};
#[test]
fn snapshot_selection_doors_refuse_every_agent_attachment_before_install() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "history-door-authority",
    );
    let host = WriterHost::start(HostConfig {
        scheduler_settings: Some(temp.path("scheduler")),
        project: "history-door-authority".into(),
        path: None,
        roots: SessionRoots::default(),
        origin: HostOrigin::Headless,
    })
    .unwrap();
    let before = host
        .client()
        .request(HostCommand::ReadHistory)
        .unwrap()
        .history
        .unwrap();
    let snapshot = HostSnapshot::from_history(before.clone()).unwrap();
    let branch = kinewright_project::bundle_records::BranchId::fresh().unwrap();
    let checkpoint = kinewright_project::bundle_records::ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 1,
        record_kind: kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
    };
    let barrier = RecoveryBarrier {
        switch_generation: 0,
        client: "a".repeat(64),
        acknowledged: vec![],
        checkpoint_ref: checkpoint,
        before: snapshot.clone(),
        after: snapshot.clone(),
    };
    let source: BorrowedSourceDescriptor = serde_json::from_value(serde_json::json!({
        "admission": "1".repeat(32), "project_id": null, "session_id": "2".repeat(32),
        "owner_epoch": 0, "extra_overlap_bytes": 0, "roots": [], "objects": [],
    }))
    .unwrap();
    let samples = DoorSamples {
        temp: &temp,
        snapshot: &snapshot,
        barrier: &barrier,
        source,
        branch: &branch,
        callback: host.client().bootstrap(),
    };
    let mut commands = Vec::new();
    let mut next = Door::after(None);
    while let Some(door) = next {
        let command = samples.command(door);
        assert_eq!(Door::of(&command), Some(door), "DOOR_SAMPLE_IS_ITS_DOOR");
        commands.push((format!("{door:?}"), command));
        next = Door::after(Some(door));
    }
    assert_eq!(commands.len(), Door::COUNT, "DOOR_CHAIN_VISITS_EVERY_DOOR");
    for role in [AttachmentRole::Viewer, AttachmentRole::Producer] {
        let agent = host
            .attach(format!("agent-{role:?}"), role, SessionRoots::default())
            .unwrap();
        for (door, command) in &commands {
            // A receipt lookup reads the committed switch result; it is a
            // connector read like `ReadHistory`. Every other door selects or installs.
            assert_eq!(
                manual::connector_command(command),
                door == "SwitchReceipt",
                "AGENT_MCP_HAS_NO_SNAPSHOT_SELECTION: {door}"
            );
            let reply = agent.request(command.clone()).unwrap();
            assert_eq!(
                reply.error.as_ref().map(|e| e.code.as_str()),
                Some("role_refused"),
                "AGENT_SNAPSHOT_DOOR_REFUSES_BEFORE_INSTALL: {role:?}: {door}"
            );
            assert_eq!(
                host.client()
                    .request(HostCommand::ReadHistory)
                    .unwrap()
                    .history
                    .as_ref(),
                Some(&before),
                "AGENT_SNAPSHOT_REFUSAL_PRESERVES_FULL_OWNER: {door}"
            );
        }
    }
    assert!(!temp.path("copy.kinewright").exists() && !temp.path("rescue.kinewright").exists());
    host.shutdown().unwrap();
}

/// Every host command that carries a snapshot, a recovery barrier or a command
/// envelope, every manual-history and switch door, and the person's selection
/// doors. `Door::of` matches all of `HostCommand`, so a new command does not
/// compile until it is classified here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Door {
    FirstSave,
    PrepareOwnedMedia,
    PrepareMediaInterpretation,
    RefreshUnsavedInputs,
    Save,
    SaveAs,
    PreviewRescue,
    RescueTo,
    PreviewRecovery,
    OpenRecovery,
    BeginRecovery,
    PublishBarrier,
    ConfirmBarrier,
    AcquireManual,
    CommitHistory,
    PrepareSwitch,
    Switch,
    SwitchReceipt,
    ConfirmSwitch,
}
impl Door {
    const COUNT: usize = 19;
    #[allow(
        clippy::match_same_arms,
        reason = "each command is classified on its own line"
    )]
    fn of(command: &HostCommand) -> Option<Self> {
        match command {
            HostCommand::FirstSave { .. } => Some(Self::FirstSave),
            HostCommand::PrepareOwnedMedia { .. } => Some(Self::PrepareOwnedMedia),
            HostCommand::PrepareMediaInterpretation { .. } => {
                Some(Self::PrepareMediaInterpretation)
            }
            HostCommand::RefreshUnsavedInputs { .. } => Some(Self::RefreshUnsavedInputs),
            HostCommand::Save { .. } => Some(Self::Save),
            HostCommand::SaveAs { .. } => Some(Self::SaveAs),
            HostCommand::PreviewRescue { .. } => Some(Self::PreviewRescue),
            HostCommand::RescueTo { .. } => Some(Self::RescueTo),
            HostCommand::PreviewRecovery { .. } => Some(Self::PreviewRecovery),
            HostCommand::OpenRecovery => Some(Self::OpenRecovery),
            HostCommand::BeginRecovery { .. } => Some(Self::BeginRecovery),
            HostCommand::PublishBarrier { .. } => Some(Self::PublishBarrier),
            HostCommand::ConfirmBarrier { .. } => Some(Self::ConfirmBarrier),
            HostCommand::AcquireManual { .. } => Some(Self::AcquireManual),
            HostCommand::CommitHistory { .. } => Some(Self::CommitHistory),
            HostCommand::PrepareSwitch { .. } => Some(Self::PrepareSwitch),
            HostCommand::Switch { .. } => Some(Self::Switch),
            HostCommand::SwitchReceipt { .. } => Some(Self::SwitchReceipt),
            HostCommand::ConfirmSwitch { .. } => Some(Self::ConfirmSwitch),
            // No snapshot, barrier, command envelope, manual or switch authority.
            // `Metadata` and `ChooseTranscript` carry a metadata compare-and-swap
            // envelope, which writes person metadata and installs no history.
            HostCommand::ReconcileTaskReceipt { .. }
            | HostCommand::IssueMetadataIssuer { .. }
            | HostCommand::DispatchOrchestration { .. }
            | HostCommand::ObserveRun { .. }
            | HostCommand::ResolveAttention { .. }
            | HostCommand::DetachForContinuation
            | HostCommand::ReviseCut { .. }
            | HostCommand::Metadata { .. }
            | HostCommand::AdmitFeedbackEvidence { .. }
            | HostCommand::ListFeedback
            | HostCommand::VoiceReceipt { .. }
            | HostCommand::AcknowledgeVoice { .. }
            | HostCommand::StartTranscription { .. }
            | HostCommand::VoiceStatus { .. }
            | HostCommand::ChooseTranscript { .. }
            | HostCommand::ReadVoiceRange { .. }
            | HostCommand::LeaseSnapshot { .. }
            | HostCommand::LeaseCurrentEdit { .. }
            | HostCommand::SnapshotLeaseStatus { .. }
            | HostCommand::ReleaseSnapshot { .. }
            | HostCommand::ReadFeedbackView { .. }
            | HostCommand::ReadFeedbackEvidence { .. }
            | HostCommand::ListVersions
            | HostCommand::ReadSnapshot { .. }
            | HostCommand::ReadFeedback { .. }
            | HostCommand::ResolveFeedback { .. }
            | HostCommand::ReadSubmittedBatch { .. }
            | HostCommand::CurrentAttachment
            | HostCommand::Attach
            | HostCommand::AdmitDestination { .. }
            | HostCommand::Status
            | HostCommand::ReadHistory
            | HostCommand::Retry
            | HostCommand::ConsentOmission
            | HostCommand::Cancel
            | HostCommand::Abandon
            | HostCommand::PreviewRetirement { .. }
            | HostCommand::RemoveBranch
            | HostCommand::PreviewForget
            | HostCommand::Forget
            | HostCommand::Reconcile { .. }
            | HostCommand::PreviewRelease { .. }
            | HostCommand::Release
            | HostCommand::SaveSession { .. }
            | HostCommand::CloseSession { .. }
            | HostCommand::StartScripted { .. }
            | HostCommand::ReopenScripted { .. }
            | HostCommand::StartSession { .. }
            | HostCommand::SendMessage { .. }
            | HostCommand::InterruptSession { .. }
            | HostCommand::SessionStatus { .. }
            | HostCommand::Detach
            | HostCommand::Shutdown => None,
        }
    }
    /// The audit's visiting order. A new door does not compile until it has a place.
    const fn after(door: Option<Self>) -> Option<Self> {
        match door {
            None => Some(Self::FirstSave),
            Some(Self::FirstSave) => Some(Self::PrepareOwnedMedia),
            Some(Self::PrepareOwnedMedia) => Some(Self::PrepareMediaInterpretation),
            Some(Self::PrepareMediaInterpretation) => Some(Self::RefreshUnsavedInputs),
            Some(Self::RefreshUnsavedInputs) => Some(Self::Save),
            Some(Self::Save) => Some(Self::SaveAs),
            Some(Self::SaveAs) => Some(Self::PreviewRescue),
            Some(Self::PreviewRescue) => Some(Self::RescueTo),
            Some(Self::RescueTo) => Some(Self::PreviewRecovery),
            Some(Self::PreviewRecovery) => Some(Self::OpenRecovery),
            Some(Self::OpenRecovery) => Some(Self::BeginRecovery),
            Some(Self::BeginRecovery) => Some(Self::PublishBarrier),
            Some(Self::PublishBarrier) => Some(Self::ConfirmBarrier),
            Some(Self::ConfirmBarrier) => Some(Self::AcquireManual),
            Some(Self::AcquireManual) => Some(Self::CommitHistory),
            Some(Self::CommitHistory) => Some(Self::PrepareSwitch),
            Some(Self::PrepareSwitch) => Some(Self::Switch),
            Some(Self::Switch) => Some(Self::SwitchReceipt),
            Some(Self::SwitchReceipt) => Some(Self::ConfirmSwitch),
            Some(Self::ConfirmSwitch) => None,
        }
    }
}
struct DoorSamples<'a> {
    temp: &'a kinewright_media::test_support::TempDirectory,
    snapshot: &'a HostSnapshot,
    barrier: &'a RecoveryBarrier,
    source: BorrowedSourceDescriptor,
    branch: &'a kinewright_project::bundle_records::BranchId,
    callback: HostBootstrap,
}
impl DoorSamples<'_> {
    fn id() -> String {
        "1".repeat(32)
    }
    fn object(kind: kinewright_project::bundle_records::RecordKind) -> ObjectRef {
        ObjectRef {
            sha256: "b".repeat(64),
            byte_length: 1,
            record_kind: kind,
        }
    }
    fn envelope(&self) -> Box<kinewright_project::receipts::CommandEnvelope> {
        Box::new(
            serde_json::from_value(serde_json::json!({
                "version": 1, "action": "switch",
                "id": {"issuer": Self::id(), "sequence": 1},
                "project_id": Self::id(), "target_branch": self.branch,
                "owner_epoch": 0, "lease": null, "expected_head": null,
                "expected_revision": 0, "command": null, "comment_text": null,
                "comment_strokes": null, "evidence": [],
            }))
            .unwrap(),
        )
    }
    fn command(&self, door: Door) -> HostCommand {
        use kinewright_project::bundle_records::RecordKind;
        let client = "a".repeat(64);
        let snapshot = self.snapshot.clone();
        match door {
            Door::FirstSave => HostCommand::FirstSave {
                destination: self.temp.path("first.kinewright"),
                snapshot,
                source: self.source.clone(),
                inputs: Self::object(RecordKind::InputManifest),
                callback: self.callback.clone(),
            },
            Door::PrepareOwnedMedia => HostCommand::PrepareOwnedMedia {
                client,
                path: self.temp.path("external.wav"),
                asset_id: kinewright_core::AssetId(1),
                snapshot,
            },
            Door::PrepareMediaInterpretation => HostCommand::PrepareMediaInterpretation {
                client,
                operation: kinewright_core::Operation::SetTrackSyncLock {
                    track: kinewright_core::TrackId(1),
                    locked: true,
                },
                snapshot,
            },
            Door::RefreshUnsavedInputs => HostCommand::RefreshUnsavedInputs {
                binding: HostSourceBinding {
                    source: Some(self.source.clone()),
                    inputs: Some(Self::object(RecordKind::InputManifest)),
                    callback: self.callback.clone(),
                },
                snapshot,
            },
            Door::Save => HostCommand::Save { snapshot },
            Door::SaveAs => HostCommand::SaveAs {
                destination: self.temp.path("copy.kinewright"),
                snapshot,
            },
            Door::PreviewRescue => HostCommand::PreviewRescue { snapshot },
            Door::RescueTo => HostCommand::RescueTo {
                destination: self.temp.path("rescue.kinewright"),
            },
            Door::PreviewRecovery => HostCommand::PreviewRecovery {
                target: HostRecoverySelection::WorkingBranch {
                    branch: self.branch.clone(),
                },
                snapshot,
            },
            Door::OpenRecovery => HostCommand::OpenRecovery,
            Door::BeginRecovery => HostCommand::BeginRecovery {
                client,
                frozen: Some(snapshot),
                pending: vec![],
            },
            Door::PublishBarrier => HostCommand::PublishBarrier {
                barrier: Box::new(self.barrier.clone()),
            },
            Door::ConfirmBarrier => HostCommand::ConfirmBarrier {
                barrier: Box::new(self.barrier.clone()),
                installed: snapshot,
            },
            Door::AcquireManual => HostCommand::AcquireManual { client },
            Door::CommitHistory => HostCommand::CommitHistory {
                client,
                messages: vec![],
            },
            Door::PrepareSwitch => HostCommand::PrepareSwitch {
                target: kinewright_project::bundle_switch::SwitchTarget::WorkingBranch {
                    branch: self.branch.clone(),
                },
            },
            Door::Switch => HostCommand::Switch {
                envelope: self.envelope(),
            },
            Door::SwitchReceipt => HostCommand::SwitchReceipt {
                envelope: self.envelope(),
            },
            Door::ConfirmSwitch => {
                let object = |kind| serde_json::to_value(Self::object(kind)).unwrap();
                let zero = [0_u8; 32];
                HostCommand::ConfirmSwitch {
                    client,
                    installed: serde_json::from_value(serde_json::json!({
                        "project": Self::id(), "root_sha256": "c".repeat(64),
                        "root_generation": 0, "switch_generation": 0,
                        "current": {
                            "branch": self.branch, "head": Self::id(),
                            "checkpoint": object(RecordKind::BranchCheckpoint),
                            "history": object(RecordKind::BranchCheckpoint),
                            "history_role": "Ordinary",
                            "history_digest": {"core": zero, "owner": zero},
                            "inputs": object(RecordKind::InputManifest),
                            "revision": 0,
                        },
                    }))
                    .unwrap(),
                }
            }
        }
    }
}

#[test]
fn task_capabilities_refuse_every_snapshot_shaped_door() {
    use crate::orchestration::{
        orchestration_records::{Role, TaskScope},
        policy::TaskPolicy,
        tasks::{RuntimeTaskAuthority, TaskEndpoint},
    };
    fn parse<T: serde::de::DeserializeOwned>(n: u64) -> T {
        serde_json::from_value(serde_json::json!(format!("{n:032x}"))).unwrap()
    }
    let core =
        kinewright_project::operation_actor::Core::for_test(kinewright_core::Document::default())
            .unwrap();
    let endpoint = TaskEndpoint::for_fixture(
        TaskPolicy {
            role: Role::Editor,
            scope: TaskScope {
                operations: std::collections::BTreeSet::new(),
                objects: std::collections::BTreeSet::new(),
                fields: std::collections::BTreeSet::new(),
                tracks: std::collections::BTreeSet::new(),
                bus_sidechain_closure: std::collections::BTreeSet::new(),
                asset_roots: vec![],
                base: kinewright_project::editing_session::BranchBinding {
                    branch: parse(10),
                    head: parse(11),
                    checkpoint: DoorSamples::object(
                        kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
                    ),
                    document_hash: "2".repeat(64),
                },
                consent_refs: vec![],
            },
        },
        RuntimeTaskAuthority {
            project: parse(5),
            run: parse(1),
            task: parse(2),
            attempt: parse(3),
            endpoint: parse(4),
            owner_epoch: 1,
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(300),
        },
        &core,
    );
    endpoint
        .admit_capability("get_timeline_state")
        .expect("TASK_CAPABILITY_CONTROL_ADMITTED");
    let mut next = Door::after(None);
    let mut names = Vec::new();
    while let Some(door) = next {
        // The wire name each door already has, plus the forms a capability
        // would use to reach the same snapshot authority.
        let name = format!("{door:?}")
            .chars()
            .enumerate()
            .flat_map(|(at, c)| {
                let lower = c.to_ascii_lowercase();
                (at > 0 && c.is_ascii_uppercase())
                    .then_some('_')
                    .into_iter()
                    .chain(std::iter::once(lower))
            })
            .collect::<String>();
        names.push(format!("{name}_snapshot"));
        names.push(name);
        next = Door::after(Some(door));
    }
    names.extend(
        [
            "install_snapshot",
            "lease_snapshot",
            "read_snapshot",
            "restore_snapshot",
            "read_history",
        ]
        .map(str::to_owned),
    );
    for name in &names {
        let refusal = endpoint
            .admit_capability(name)
            .expect_err("TASK_SNAPSHOT_CAPABILITY_REFUSED");
        assert_eq!(
            refusal,
            format!("task_scope: capability {name} is unavailable"),
            "TASK_SNAPSHOT_CAPABILITY_SCOPE_REFUSAL: {name}"
        );
    }
}
