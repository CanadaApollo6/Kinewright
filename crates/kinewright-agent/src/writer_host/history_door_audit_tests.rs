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

fn holds_snapshot(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key("history_sha256") || map.values().any(holds_snapshot)
        }
        serde_json::Value::Array(values) => values.iter().any(holds_snapshot),
        _ => false,
    }
}

#[test]
fn snapshot_doors_admit_a_long_history_past_the_envelope_cap() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "history-door-ingress",
    );
    let host = WriterHost::start(HostConfig {
        scheduler_settings: Some(temp.path("scheduler")),
        project: "history-door-ingress".into(),
        path: None,
        roots: SessionRoots::default(),
        origin: HostOrigin::Headless,
    })
    .unwrap();
    // One long label stands for a long history: the snapshot alone is larger
    // than the envelope cap that used to apply to these doors.
    let long = kinewright_project::operation_actor::Core::for_test(kinewright_core::Document {
        markers: vec![kinewright_core::Marker {
            id: kinewright_core::MarkerId(1),
            position: kinewright_core::TimeCode(0),
            label: "\"".repeat(HOST_ENVELOPE_BYTES),
            color_token: 0,
        }],
        ..kinewright_core::Document::default()
    })
    .unwrap();
    let snapshot = HostSnapshot::capture(&long).unwrap();
    let branch = kinewright_project::bundle_records::BranchId::fresh().unwrap();
    let barrier = RecoveryBarrier {
        switch_generation: 0,
        client: "a".repeat(64),
        acknowledged: vec![],
        checkpoint_ref: DoorSamples::object(
            kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
        ),
        before: snapshot.clone(),
        after: snapshot.clone(),
    };
    let samples = DoorSamples {
        temp: &temp,
        snapshot: &snapshot,
        barrier: &barrier,
        source: serde_json::from_value(serde_json::json!({
            "admission": "1".repeat(32), "project_id": null, "session_id": "2".repeat(32),
            "owner_epoch": 0, "extra_overlap_bytes": 0, "roots": [], "objects": [],
        }))
        .unwrap(),
        branch: &branch,
        callback: host.client().bootstrap(),
    };
    let mut next = Door::after(None);
    while let Some(door) = next {
        let command = samples.command(door);
        assert_eq!(
            carries_snapshot(&command),
            holds_snapshot(&serde_json::to_value(&command).unwrap()),
            "SNAPSHOT_INGRESS_CLASS_MATCHES_PAYLOAD: {door:?}"
        );
        next = Door::after(Some(door));
    }
    for door in [
        Door::RefreshUnsavedInputs,
        Door::PrepareOwnedMedia,
        Door::PrepareMediaInterpretation,
        Door::BeginRecovery,
        Door::PublishBarrier,
        Door::ConfirmBarrier,
    ] {
        let command = samples.command(door);
        assert!(
            serde_json::to_vec(&command).unwrap().len() > HOST_ENVELOPE_BYTES,
            "SNAPSHOT_DOOR_BODY_EXCEEDS_ENVELOPE: {door:?}"
        );
        // The door may refuse this foreign snapshot; HTTP ingress must not.
        let reply = host.client().request(command);
        assert!(
            reply.is_ok(),
            "SNAPSHOT_DOOR_PASSES_HTTP_INGRESS: {door:?}: {:?}",
            reply.err()
        );
    }
    host.shutdown().unwrap();
}

/// `base` with a transport body of exactly `raw` bytes, all worst-case
/// escaping: a JSON string of `\"` pairs, padded when the count is odd.
fn sized_snapshot(base: &HostSnapshot, raw: u64) -> HostSnapshot {
    let mut wire = serde_json::to_value(base).unwrap();
    let mut body = String::from("\"");
    body.push_str(&"\\\"".repeat(usize::try_from((raw - 2) / 2).unwrap()));
    body.push_str(&"a".repeat(usize::try_from((raw - 2) % 2).unwrap()));
    body.push('"');
    wire["history"]["snapshot"]["body"] = serde_json::Value::String(body);
    let snapshot: HostSnapshot = serde_json::from_value(wire).unwrap();
    assert_eq!(
        snapshot.history.snapshot.body_bytes(),
        raw,
        "SIZED_SNAPSHOT"
    );
    snapshot
}

#[test]
fn snapshot_over_its_door_cap_is_refused_at_the_client_before_sending() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "history-door-snapshot-cap",
    );
    let host = WriterHost::start(HostConfig {
        scheduler_settings: Some(temp.path("scheduler")),
        project: "history-door-snapshot-cap".into(),
        path: None,
        roots: SessionRoots::default(),
        origin: HostOrigin::Headless,
    })
    .unwrap();
    let client = host.client();
    let read = || {
        client
            .request(HostCommand::ReadHistory)
            .unwrap()
            .history
            .unwrap()
    };
    let before = read();
    let base = HostSnapshot::from_history(before.clone()).unwrap();
    // A client whose endpoint accepts no connection: any byte it tried to send
    // would fail as a connection error, not as the capacity refusal.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut endpoint = client.endpoint().clone();
    endpoint.address = closed;
    let bootstrap = client.bootstrap();
    let unsent = HostClient::connect(endpoint, bootstrap.auth, bootstrap.roots).unwrap();
    let barrier = |before: &HostSnapshot, after: &HostSnapshot| {
        Box::new(RecoveryBarrier {
            switch_generation: 0,
            client: "a".repeat(64),
            acknowledged: vec![],
            checkpoint_ref: DoorSamples::object(
                kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
            ),
            before: before.clone(),
            after: after.clone(),
        })
    };
    let refused = |command: HostCommand, label: &str| {
        for (name, target) in [("closed", &unsent), ("live", &client)] {
            let error = target.request(command.clone()).err();
            assert!(
                matches!(&error, Some(HostError::Refused(refusal)) if refusal.code == "snapshot_capacity"),
                "SNAPSHOT_CAPACITY_REFUSED_BEFORE_SENDING {label} {name}: {error:?}"
            );
        }
        assert_eq!(read(), before, "SNAPSHOT_CAPACITY_LEAVES_HISTORY {label}");
    };
    let admitted = |command: HostCommand, label: &str| {
        let encoded = serde_json::to_vec(&command).unwrap().len();
        println!("SNAPSHOT_DOOR_AT_CAP {label} body={encoded} limit={HOST_BODY_BYTES}");
        // The door may refuse this foreign snapshot; neither the client gate
        // nor HTTP ingress may.
        let reply = client.request(command);
        assert!(
            reply.is_ok(),
            "SNAPSHOT_AT_CAP_CROSSES_HTTP {label}: {:?}",
            reply.err()
        );
        assert_eq!(read(), before, "SNAPSHOT_AT_CAP_LEAVES_HISTORY {label}");
    };

    let over = sized_snapshot(&base, SNAPSHOT_DOOR_BYTES + 1);
    refused(HostCommand::Save { snapshot: over }, "Save");
    let at = sized_snapshot(&base, SNAPSHOT_DOOR_BYTES);
    // The same call, retried within the cap, reaches the host.
    admitted(HostCommand::Save { snapshot: at }, "Save");

    let at = sized_snapshot(&base, CONFIRM_BARRIER_SNAPSHOT_BYTES);
    let over = sized_snapshot(&base, CONFIRM_BARRIER_SNAPSHOT_BYTES + 1);
    // Any one of the three snapshots over the cap refuses the whole request.
    for (before_snapshot, after_snapshot, installed, slot) in [
        (&over, &at, &at, "before"),
        (&at, &over, &at, "after"),
        (&at, &at, &over, "installed"),
    ] {
        refused(
            HostCommand::ConfirmBarrier {
                barrier: barrier(before_snapshot, after_snapshot),
                installed: installed.clone(),
            },
            &format!("ConfirmBarrier {slot}"),
        );
    }
    admitted(
        HostCommand::ConfirmBarrier {
            barrier: barrier(&at, &at),
            installed: at.clone(),
        },
        "ConfirmBarrier",
    );
    host.shutdown().unwrap();
}

/// Measurement only: how close a large, realistic session comes to the
/// snapshot door caps. A transcript is not part of the document or its
/// history, so the 40,000-word transcript adds nothing to a snapshot.
#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn snapshot_door_caps_against_a_large_realistic_session() {
    use kinewright_core::{
        AssetId, Clip, ClipContent, ClipId, ColorDescription, Command, Document, Marker, MarkerId,
        MediaAsset, MediaKind, MediaSourceFingerprint, Operation, Rational, TimeCode, Track,
        TrackId, TrackKind,
    };
    const ASSETS: u64 = 40;
    const TRACKS: u64 = 4;
    const CLIPS: u64 = 3_000;
    const STEPS: u64 = 2_000;
    let fps = Rational::new(30, 1).unwrap();
    // 3,000 clips over four tracks of 750 clips, 432 frames each: three
    // hours at 30 fps.
    let clip_len = 432_i64;
    let assets: Vec<MediaAsset> = (1..=ASSETS)
        .map(|id| MediaAsset {
            id: AssetId(id),
            path: format!(
                "/media/interview/day-{}/A{id:03}_C{id:03}_0613RK.mov",
                id % 3
            )
            .into(),
            name: format!("A{id:03}_C{id:03}_0613RK"),
            duration: TimeCode(30 * 60 * 30),
            fps,
            kind: MediaKind::Video,
            resolution: Some((3840, 2160)),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorDescription::default(),
            assumed_from: None,
        })
        .collect();
    let tracks = (0..TRACKS)
        .map(|track| Track {
            id: TrackId(track + 1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: (0..CLIPS / TRACKS)
                .map(|slot| {
                    let id = track * (CLIPS / TRACKS) + slot + 1;
                    let source = i64::try_from(id * 97 % 40_000).unwrap();
                    Clip {
                        enabled: true,
                        enabled_curve: None,
                        id: ClipId(id),
                        asset: AssetId(id % ASSETS + 1),
                        source_range: TimeCode(source)..TimeCode(source + clip_len),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(i64::try_from(slot).unwrap() * clip_len),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                        audio_gain_curve: None,
                        blend_mode: kinewright_core::BlendMode::Normal,
                    }
                })
                .collect(),
        })
        .collect();
    let document = Document {
        tracks,
        media_pool: assets,
        fps,
        resolution: (3840, 2160),
        duration: TimeCode(i64::try_from(CLIPS / TRACKS).unwrap() * clip_len),
        ..Document::default()
    };
    document.validate().unwrap();
    let pool = kinewright_project::operation_history::HistoryPool::new(
        kinewright_project::operation_history::PoolLimits::default(),
    );
    let core = kinewright_project::operation_actor::Core::spawn(
        &pool,
        kinewright_project::operation_history::Role::Ordinary,
        kinewright_project::operation_history::CreationOrigin {
            branch: serde_json::from_str("\"11111111111111111111111111111111\"").unwrap(),
            batch: serde_json::from_str("\"22222222222222222222222222222222\"").unwrap(),
        },
        document,
    )
    .unwrap();
    // 2,000 ordinary edits: audio levels on every step, a review note every
    // fifth step.
    for step in 0..STEPS {
        let clip = ClipId(step * 37 % CLIPS + 1);
        let mut operations = vec![Operation::SetClipAudio {
            clip,
            gain_tenth_db: -i32::try_from(step % 60).unwrap(),
            fade_in_frames: TimeCode(i64::try_from(step % 12).unwrap()),
            fade_out_frames: TimeCode::ZERO,
        }];
        if step % 5 == 0 {
            operations.push(Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(step + 1),
                    position: TimeCode(i64::try_from(step).unwrap() * 150),
                    label: format!("Tighten the answer before take {step}; keep the laugh"),
                    color_token: 0,
                },
            });
        }
        core.request(Command::DoBatch(operations)).unwrap();
    }
    let snapshot = HostSnapshot::capture(&core).unwrap();
    let raw = snapshot.history.snapshot.body_bytes();
    let save = serde_json::to_vec(&HostCommand::Save {
        snapshot: snapshot.clone(),
    })
    .unwrap()
    .len();
    let confirm = serde_json::to_vec(&HostCommand::ConfirmBarrier {
        barrier: Box::new(RecoveryBarrier {
            switch_generation: 0,
            client: "a".repeat(64),
            acknowledged: vec![],
            checkpoint_ref: DoorSamples::object(
                kinewright_project::bundle_records::RecordKind::BranchCheckpoint,
            ),
            before: snapshot.clone(),
            after: snapshot.clone(),
        }),
        installed: snapshot,
    })
    .unwrap()
    .len();
    println!(
        "SNAPSHOT_SESSION_MEASURE raw={raw} save_body={save} confirm_body={confirm} \
         single_cap={SNAPSHOT_DOOR_BYTES} single_headroom={} \
         begin_recovery_cap={BEGIN_RECOVERY_SNAPSHOT_BYTES} begin_recovery_headroom={} \
         publish_cap={PUBLISH_BARRIER_SNAPSHOT_BYTES} publish_headroom={} \
         confirm_cap={CONFIRM_BARRIER_SNAPSHOT_BYTES} confirm_headroom={} \
         body_limit={HOST_BODY_BYTES}",
        SNAPSHOT_DOOR_BYTES / raw,
        BEGIN_RECOVERY_SNAPSHOT_BYTES / raw,
        PUBLISH_BARRIER_SNAPSHOT_BYTES / raw,
        CONFIRM_BARRIER_SNAPSHOT_BYTES / raw,
    );
}
