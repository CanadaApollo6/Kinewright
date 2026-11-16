//! Agent-facing ingress cannot choose or install a person's snapshot.
use super::*;
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
    let source = serde_json::from_value(serde_json::json!({
        "admission": "1".repeat(32), "project_id": null, "session_id": "2".repeat(32),
        "owner_epoch": 0, "extra_overlap_bytes": 0, "roots": [], "objects": [],
    }))
    .unwrap();
    let commands = [
        (
            "First Save",
            HostCommand::FirstSave {
                destination: temp.path("first.kinewright"),
                snapshot: snapshot.clone(),
                source,
                inputs: kinewright_project::bundle_records::ObjectRef {
                    sha256: "b".repeat(64),
                    byte_length: 1,
                    record_kind: kinewright_project::bundle_records::RecordKind::InputManifest,
                },
                callback: host.client().bootstrap(),
            },
        ),
        (
            "prepared media",
            HostCommand::PrepareOwnedMedia {
                client: "a".repeat(64),
                path: temp.path("external.wav"),
                asset_id: kinewright_core::AssetId(1),
                snapshot: snapshot.clone(),
            },
        ),
        (
            "private interpretation",
            HostCommand::PrepareMediaInterpretation {
                client: "a".repeat(64),
                operation: kinewright_core::Operation::SetTrackSyncLock {
                    track: kinewright_core::TrackId(1),
                    locked: true,
                },
                snapshot: snapshot.clone(),
            },
        ),
        (
            "Save/adoption",
            HostCommand::Save {
                snapshot: snapshot.clone(),
            },
        ),
        (
            "Save As",
            HostCommand::SaveAs {
                destination: temp.path("copy.kinewright"),
                snapshot: snapshot.clone(),
            },
        ),
        (
            "rescue preview",
            HostCommand::PreviewRescue {
                snapshot: snapshot.clone(),
            },
        ),
        (
            "rescue selection",
            HostCommand::RescueTo {
                destination: temp.path("rescue.kinewright"),
            },
        ),
        (
            "checkpoint preview",
            HostCommand::PreviewRecovery {
                target: HostRecoverySelection::WorkingBranch {
                    branch: branch.clone(),
                },
                snapshot,
            },
        ),
        ("checkpoint selection", HostCommand::OpenRecovery),
        (
            "branch switch",
            HostCommand::PrepareSwitch {
                target: kinewright_project::bundle_switch::SwitchTarget::WorkingBranch { branch },
            },
        ),
        (
            "recovery begin",
            HostCommand::BeginRecovery {
                client: "a".repeat(64),
                frozen: None,
                pending: vec![],
                discard_batches: None,
            },
        ),
        (
            "recovery publish",
            HostCommand::PublishBarrier {
                barrier: Box::new(barrier.clone()),
            },
        ),
        (
            "recovery confirm",
            HostCommand::ConfirmBarrier {
                barrier: Box::new(barrier),
                installed: HostSnapshot::from_history(before.clone()).unwrap(),
            },
        ),
    ];
    for role in [AttachmentRole::Viewer, AttachmentRole::Producer] {
        let agent = host
            .attach(format!("agent-{role:?}"), role, SessionRoots::default())
            .unwrap();
        for (door, command) in &commands {
            assert!(
                !manual::connector_command(command),
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
