//! Review witnesses use the serialized production command owner and existing IO hooks.
use super::*;
use kinewright_core::{Command as CoreCommand, Event, Operation, Track, TrackId, TrackKind};
use kinewright_project::{editing_session::Ja1EditingSession, io_observer};

fn fixture() -> (
    kinewright_media::test_support::TempDirectory,
    HostCommandOwner,
) {
    let parent = kinewright_media::test_support::workspace_target_dir();
    std::fs::create_dir_all(&parent).unwrap();
    let temp = kinewright_media::test_support::TempDirectory::new_in(&parent, "s2h-host");
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    if let Some(path) = std::env::var_os("JA_S2H_STATUS_HISTORY") {
        let history = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        core.restore_history(history).unwrap();
    }
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    owner
        .first_save(&temp.path("a.kinewright"), StorageLimits::default())
        .unwrap();
    let mut host = HostCommandOwner::open(HostConfig {
        project: "s2h-review".into(),
        path: None,
        roots: SessionRoots {
            read: vec![temp.root().into()],
            write: vec![temp.root().into()],
        },
        origin: HostOrigin::Headless,
    })
    .unwrap();
    host.initial_core = core;
    host.owner = Some(owner);
    (temp, host)
}
fn run(host: &mut HostCommandOwner, command: HostCommand) -> HostReply {
    host.execute(command, host.config.roots.clone())
}
fn checked(host: &mut HostCommandOwner, command: HostCommand) -> HostReply {
    let reply = run(host, command);
    assert!(reply.error.is_none(), "{:?}", reply.error);
    reply
}
fn edit(core: &Core, locked: bool) {
    assert!(matches!(
        core.request(CoreCommand::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}

#[test]
fn s2h_confirm_inputs_failure_revokes_partial_grant_and_refreezes() {
    let (temp, mut host) = fixture();
    edit(host.core(), false);
    host.owner_mut().unwrap().save_active().unwrap();
    let target = host
        .owner()
        .unwrap()
        .list_recovery_points()
        .unwrap()
        .remove(0);
    let client = "a".repeat(64);
    host.acquire_manual(client.clone()).unwrap();
    let snapshot = HostSnapshot::capture(host.core()).unwrap();
    let prepared = checked(
        &mut host,
        HostCommand::PreviewRecovery {
            target: target.into(),
            snapshot,
        },
    );
    assert!(prepared.switch_envelope.is_some());
    let selected = checked(
        &mut host,
        HostCommand::Switch {
            envelope: Box::new(prepared.switch_envelope.unwrap()),
        },
    );
    let installed = selected.switch_attachment.unwrap().identity().unwrap();
    let input = temp.path(&format!(
        "a.kinewright-assets/studio/objects/{}",
        installed.current.inputs.sha256
    ));
    let hidden = input.with_extension("unavailable");
    let original = input.clone();
    let moved = hidden.clone();
    let failed = io_observer::at_barrier(
        "root_rename",
        move || {
            if original.exists() {
                std::fs::rename(&original, &moved).unwrap();
            }
        },
        || {
            run(
                &mut host,
                HostCommand::ConfirmSwitch {
                    client: client.clone(),
                    installed: Box::new(installed.clone()),
                },
            )
        },
    );
    std::fs::rename(&hidden, &input).unwrap();
    assert!(
        failed.error.is_some(),
        "the post-grant input read must fail"
    );
    assert!(
        host.manual.is_none(),
        "an undelivered grant must be revoked"
    );
    assert!(host.switch_frozen);
    assert_eq!(host.core().attachment_state(), Ok(AttachmentState::Frozen));
    let reply = checked(
        &mut host,
        HostCommand::ConfirmSwitch {
            client: client.clone(),
            installed: Box::new(installed.clone()),
        },
    );
    let generation = host.next_manual_generation;
    let before_metadata = host.owner().unwrap().current_switch_attachment().unwrap();
    let _unrelated = host
        .owner_mut()
        .unwrap()
        .fork_durable_branch("unrelated metadata")
        .unwrap();
    let after_metadata = host.owner().unwrap().current_switch_attachment().unwrap();
    assert_ne!(before_metadata.root_sha256, after_metadata.root_sha256);
    assert_eq!(before_metadata.current, after_metadata.current);
    let repeated = checked(
        &mut host,
        HostCommand::ConfirmSwitch {
            client,
            installed: Box::new(installed),
        },
    );
    assert_eq!(repeated.manual_lease, reply.manual_lease);
    assert_eq!(
        host.next_manual_generation, generation,
        "duplicate confirmation must not issue another lease"
    );
}

#[test]
fn s2h_status_disclosures_cache_and_invalidate_actual_histories() {
    let (_temp, mut host) = fixture();
    let first = host
        .owner_mut()
        .unwrap()
        .fork_durable_branch("first")
        .unwrap();
    let second = host
        .owner_mut()
        .unwrap()
        .fork_durable_branch("second")
        .unwrap();
    checked(&mut host, HostCommand::Status);
    let ((), repeated) = io_observer::observe(|| {
        for _ in 0..3 {
            checked(&mut host, HostCommand::Status);
        }
    });
    assert!(
        !repeated.events.iter().any(|event| matches!(
            event.operation,
            "studio_dirty_history_hash" | "studio_save_disclosure_select"
        )),
        "unchanged Status must not traverse history"
    );
    edit(first.core(), false);
    let (dirty, changed) = io_observer::observe(|| checked(&mut host, HostCommand::Status));
    assert!(
        dirty
            .status
            .close_disclosure
            .contains("first has unsaved work")
    );
    assert!(
        !dirty
            .status
            .close_disclosure
            .contains("second has unsaved work")
    );
    assert_eq!(
        changed
            .events
            .iter()
            .filter(|event| event.operation == "studio_dirty_history_hash")
            .count(),
        1
    );
    edit(second.core(), false);
    assert!(
        checked(&mut host, HostCommand::Status)
            .status
            .close_disclosure
            .contains("second has unsaved work")
    );
    edit(host.core(), false);
    let (_, changed) = io_observer::observe(|| checked(&mut host, HostCommand::Status));
    assert!(
        changed
            .events
            .iter()
            .any(|event| event.operation == "studio_save_disclosure_select")
    );
}

#[test]
fn s2h_status_and_save_cost_observer() {
    let (_temp, mut host) = fixture();
    host.owner_mut()
        .unwrap()
        .fork_durable_branch("first")
        .unwrap();
    host.owner_mut()
        .unwrap()
        .fork_durable_branch("second")
        .unwrap();
    let history = host.core().export_history().unwrap();
    let mut status_ms = Vec::new();
    checked(&mut host, HostCommand::Status);
    for _ in 0..5 {
        let start = std::time::Instant::now();
        checked(&mut host, HostCommand::Status);
        status_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mut save_ms = Vec::new();
    for _ in 0..3 {
        let snapshot = HostSnapshot::capture(host.core()).unwrap();
        let start = std::time::Instant::now();
        checked(&mut host, HostCommand::Save { snapshot });
        save_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    assert_eq!(host.core().export_history().unwrap(), history);
    println!(
        "STATUS_COST {}",
        serde_json::json!({"history_commands":history.batches.len(), "registered_sessions":2, "status_ms":status_ms, "save_ms":save_ms})
    );
}
