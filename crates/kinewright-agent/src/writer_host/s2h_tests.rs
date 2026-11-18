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
    let core = if let Some(path) = std::env::var_os("JA_S2H_STATUS_HISTORY") {
        Core::peer_for_test(serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()).unwrap()
    } else {
        Core::for_test(Document {
            tracks: vec![Track {
                name: None,
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap()
    };
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    owner
        .first_save(&temp.path("a.kinewright"), StorageLimits::default())
        .unwrap();
    let mut host = HostCommandOwner::open(HostConfig {
        data_dir: Some((temp.path("scheduler")).with_extension("voice-data")),
        scheduler_settings: Some(temp.path("scheduler")),
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
    received_reply_for_test(host.execute(command, host.config.roots.clone()))
}
fn checked(host: &mut HostCommandOwner, command: HostCommand) -> HostReply {
    let reply = run(host, command);
    assert!(reply.error.is_none(), "{:?}", reply.error);
    reply
}

#[test]
fn s3_fix1_admitted_extension_recovers_after_host_restart() {
    for colour in [false, true] {
        let (temp, mut host) = fixture();
        let path = temp.path("restart.wav");
        std::fs::write(
            &path,
            kinewright_media::test_support::wav_f32(&vec![0.25; 4800], 48_000, 1),
        )
        .unwrap();
        let client = "e".repeat(64);
        checked(
            &mut host,
            HostCommand::AcquireManual {
                client: client.clone(),
            },
        );
        if colour {
            host.owner_mut().unwrap().import_owned_media(&path).unwrap();
            host.manual = None;
            checked(
                &mut host,
                HostCommand::AcquireManual {
                    client: client.clone(),
                },
            );
        }
        let before = received_snapshot_for_test(host.core()).unwrap();
        let command = if colour {
            HostCommand::PrepareMediaInterpretation {
                client: client.clone(),
                operation: kinewright_core::assume_rec709_operation(
                    kinewright_core::AssetId(1),
                    &document_for_test(host.core()).media_pool[0].color_description,
                ),
                snapshot: before.clone(),
            }
        } else {
            HostCommand::PrepareOwnedMedia {
                client: client.clone(),
                path: path.clone(),
                asset_id: kinewright_core::AssetId(1),
                snapshot: before.clone(),
            }
        };
        let prepared = checked(&mut host, command);
        let gui = kinewright_project::operation_actor::Core::peer_for_test(before.history).unwrap();
        let replica = Ja1EditingSession::new(temp.root(), &gui, StorageLimits::default()).unwrap();
        let permit = replica
            .bind_manual_producer(
                prepared.manual_lease.unwrap(),
                prepared.manual_inputs.unwrap(),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            )
            .unwrap();
        let operation = prepared
            .owned_media_edit
            .unwrap_or_else(|| Operation::AddAsset {
                asset: prepared.owned_media.unwrap(),
            });
        assert!(matches!(
            permit.request(CoreCommand::Do(operation)).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let pending = permit.pending().unwrap();
        let frozen = received_snapshot_for_test(&gui).unwrap();
        let mut config = host.config.clone();
        config.path = Some(temp.path("a.kinewright"));
        drop(host);
        std::fs::remove_file(path).unwrap();
        let mut host = HostCommandOwner::open(config).unwrap();
        let recovered = run(
            &mut host,
            HostCommand::BeginRecovery {
                client,
                frozen: Some(frozen.clone()),
                pending,
            },
        );
        assert!(
            recovered.error.is_none(),
            "C0_FIX1_RESTART_EXTENSION: {:?}",
            recovered.error
        );
        let barrier = recovered.recovery.unwrap();
        let installed = barrier.after.history.identity();
        checked(
            &mut host,
            HostCommand::PublishBarrier {
                barrier: Box::new(barrier),
            },
        );
        let mut loaded = kinewright_project::bundle::LoadedBundle::read(
            &temp.path("a.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let restored = loaded.open_manual().unwrap();
        let expected = gui.export_history().unwrap();
        let actual = restored.export_history().unwrap();
        assert_eq!(
            actual.history().document,
            expected.history().document,
            "C0_FIX1_RESTART_SUFFIX_DOCUMENT"
        );
        assert!(
            serde_json::to_value(&actual.history().undo).unwrap()
                == serde_json::to_value(&expected.history().undo).unwrap(),
            "C0_FIX1_RESTART_SUFFIX_UNDO"
        );
        assert!(
            serde_json::to_value(&actual.history().redo).unwrap()
                == serde_json::to_value(&expected.history().redo).unwrap(),
            "C0_FIX1_RESTART_SUFFIX_REDO"
        );
        assert_eq!(
            actual.resources(),
            expected.resources(),
            "C0_FIX1_RESTART_SUFFIX_RESOURCES"
        );
        assert_eq!(
            restored.snapshot_identity().unwrap(),
            installed,
            "C0_FIX1_RESTART_PUBLISHED_EPOCH"
        );
    }
}

#[test]
fn s3_host_owned_input_admission_rebinds_only_the_exact_editor_cut() {
    let (temp, mut host) = fixture();
    let path = temp.path("owned.wav");
    std::fs::write(
        &path,
        kinewright_media::test_support::wav_f32(
            &kinewright_media::test_support::tone(440.0, 0.25, 48_000, 48_000),
            48_000,
            1,
        ),
    )
    .unwrap();
    let client = "p".repeat(64);
    checked(
        &mut host,
        HostCommand::AcquireManual {
            client: client.clone(),
        },
    );
    let snapshot = received_snapshot_for_test(host.core()).unwrap();
    let original_lease = host
        .manual
        .as_ref()
        .map(|authority| authority.lease().clone());
    let before_refusal_root = host.owner().unwrap().root_sha256().unwrap();
    let altered =
        kinewright_project::operation_actor::Core::peer_for_test(snapshot.history.clone()).unwrap();
    edit(&altered, false);
    for (request_client, request_snapshot, code) in [
        ("x".repeat(64), snapshot.clone(), "manual_lease"),
        (
            client.clone(),
            received_snapshot_for_test(&altered).unwrap(),
            "history_changed",
        ),
    ] {
        let refused = run(
            &mut host,
            HostCommand::PrepareOwnedMedia {
                client: request_client,
                path: path.clone(),
                asset_id: kinewright_core::AssetId(1),
                snapshot: request_snapshot,
            },
        );
        assert_eq!(
            refused.error.as_ref().map(|error| error.code.as_str()),
            Some(code)
        );
        assert_eq!(
            host.manual
                .as_ref()
                .map(kinewright_project::editing_session::ManualAuthority::lease),
            original_lease.as_ref()
        );
        assert_eq!(
            host.owner().unwrap().root_sha256().unwrap(),
            before_refusal_root
        );
    }
    let command = HostCommand::PrepareOwnedMedia {
        client: client.clone(),
        path: path.clone(),
        asset_id: kinewright_core::AssetId(1),
        snapshot: snapshot.clone(),
    };
    let roots = host.config.roots.clone();
    let prepared = host.execute_operation(command.clone(), roots.clone(), Some(&client), Some(1));
    assert!(prepared.error.is_none(), "{:?}", prepared.error);
    assert_eq!(
        received_snapshot_for_test(host.core()).unwrap(),
        snapshot,
        "admission cannot publish the asset"
    );
    let root = host.owner().unwrap().root_sha256().unwrap();
    std::fs::remove_file(&path).unwrap();
    let replay = host.execute_operation(command, roots, Some(&client), Some(1));
    assert_eq!(
        serde_json::to_vec(&prepared).unwrap(),
        serde_json::to_vec(&replay).unwrap()
    );
    assert_eq!(host.owner().unwrap().root_sha256().unwrap(), root);
    let replica_core =
        kinewright_project::operation_actor::Core::peer_for_test(snapshot.history.clone()).unwrap();
    let replica =
        Ja1EditingSession::new(temp.root(), &replica_core, StorageLimits::default()).unwrap();
    let permit = replica
        .bind_manual_producer(
            prepared.manual_lease.unwrap(),
            prepared.manual_inputs.unwrap(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
        .unwrap();
    let asset = prepared.owned_media.unwrap();
    assert!(matches!(
        permit
            .request(CoreCommand::Do(Operation::AddAsset {
                asset: asset.clone()
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    checked(
        &mut host,
        HostCommand::CommitHistory {
            client,
            messages: permit.pending().unwrap(),
        },
    );
    assert_eq!(document_for_test(host.core()).media_pool, vec![asset]);
    assert!(
        run(
            &mut host,
            HostCommand::PrepareOwnedMedia {
                client: "x".repeat(64),
                path,
                asset_id: kinewright_core::AssetId(2),
                snapshot
            }
        )
        .error
        .is_some()
    );
}
#[test]
fn s3_gui_owned_input_forwarded_scope_refuses_before_admission() {
    let (temp, mut host) = fixture();
    let path = temp.path("chosen.wav");
    std::fs::write(
        &path,
        kinewright_media::test_support::wav_f32(&vec![0.25; 4800].into_boxed_slice(), 48_000, 1),
    )
    .unwrap();
    let client = "g".repeat(64);
    checked(
        &mut host,
        HostCommand::AcquireManual {
            client: client.clone(),
        },
    );
    let snapshot = received_snapshot_for_test(host.core()).unwrap();
    host.config.origin = HostOrigin::Gui {
        initial_snapshot: Some(snapshot.clone()),
        retention: HostSourceBinding {
            source: None,
            inputs: None,
            callback: HostBootstrap {
                endpoint: HostEndpoint {
                    address: "127.0.0.1:1".parse().unwrap(),
                    project: "unused".into(),
                    protocol: HOST_PROTOCOL,
                },
                auth: HostAuth::generate().unwrap(),
                roots: SessionRoots::default(),
            },
        },
        expected_root_sha256: None,
        expected_project_id: None,
    };
    let before = host.owner().unwrap().root_sha256().unwrap();
    let lease = host
        .manual
        .as_ref()
        .map(|authority| authority.lease().clone());
    let refusal = host
        .prepare_owned_media(
            &client,
            &path,
            kinewright_core::AssetId(1),
            &snapshot,
            &SessionRoots::default(),
            &mut HostReply::default(),
        )
        .err()
        .map(|error| error.refusal().code);
    assert_eq!(
        refusal.as_deref(),
        Some("root_refused"),
        "forwarded input scope must refuse before admission"
    );
    assert_eq!(host.owner().unwrap().root_sha256().unwrap(), before);
    assert_eq!(
        host.manual
            .as_ref()
            .map(kinewright_project::editing_session::ManualAuthority::lease),
        lease.as_ref()
    );
    assert!(host.prepared_owned_media.is_none());
    let mut reply = HostReply::default();
    host.prepare_owned_media(
        &client,
        &path,
        kinewright_core::AssetId(1),
        &snapshot,
        &SessionRoots {
            read: vec![temp.root().into()],
            write: vec![],
        },
        &mut reply,
    )
    .unwrap();
    assert!(reply.owned_media.is_some());
    assert_eq!(received_snapshot_for_test(host.core()).unwrap(), snapshot);
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
    let snapshot = received_snapshot_for_test(host.core()).unwrap();
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
        .fork_durable_branch(
            "unrelated metadata",
            kinewright_project::editing_session::DurableBranchRole::Session,
        )
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
        .fork_durable_branch(
            "first",
            kinewright_project::editing_session::DurableBranchRole::Session,
        )
        .unwrap();
    let second = host
        .owner_mut()
        .unwrap()
        .fork_durable_branch(
            "second",
            kinewright_project::editing_session::DurableBranchRole::Session,
        )
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
        0,
        "CHANGED_STATUS_USES_CHANGE_IDS_WITHOUT_HASHING_HISTORY"
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
        !changed.events.iter().any(|event| matches!(
            event.operation,
            "studio_save_disclosure_select" | "studio_dirty_history_hash"
        )),
        "MANUAL_STATUS_USES_CHANGE_IDS_WITHOUT_HISTORY_SELECTION"
    );
}

#[test]
fn s2h_status_and_save_cost_observer() {
    let (_temp, mut host) = fixture();
    host.owner_mut()
        .unwrap()
        .fork_durable_branch(
            "first",
            kinewright_project::editing_session::DurableBranchRole::Session,
        )
        .unwrap();
    host.owner_mut()
        .unwrap()
        .fork_durable_branch(
            "second",
            kinewright_project::editing_session::DurableBranchRole::Session,
        )
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
        let snapshot = received_snapshot_for_test(host.core()).unwrap();
        let start = std::time::Instant::now();
        checked(&mut host, HostCommand::Save { snapshot });
        save_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    assert_eq!(host.core().export_history().unwrap(), history);
    println!(
        "STATUS_COST {}",
        serde_json::json!({"history_commands":history.history().position, "registered_sessions":2, "status_ms":status_ms, "save_ms":save_ms})
    );
}

#[test]
fn s2h_post_grant_identity_failure_revokes_and_retries_one_lease() {
    let (_temp, mut host) = fixture();
    let client = "i".repeat(64);
    host.acquire_manual(client.clone()).unwrap();
    let target = host
        .owner()
        .unwrap()
        .list_recovery_points()
        .unwrap()
        .remove(0);
    let snapshot = received_snapshot_for_test(host.core()).unwrap();
    let prepared = checked(
        &mut host,
        HostCommand::PreviewRecovery {
            target: target.into(),
            snapshot,
        },
    );
    let selected = checked(
        &mut host,
        HostCommand::Switch {
            envelope: Box::new(prepared.switch_envelope.unwrap()),
        },
    );
    let installed = selected.switch_attachment.unwrap().identity().unwrap();
    let command = HostCommand::ConfirmSwitch {
        client,
        installed: Box::new(installed),
    };
    let failed = io_observer::fail_nth("manual_switch_identity", 2, || {
        run(&mut host, command.clone())
    });
    assert!(failed.error.is_some(), "post-grant identity read must fail");
    assert!(
        host.manual.is_none(),
        "undelivered identity grant must be revoked"
    );
    assert!(host.switch_frozen);
    assert_eq!(host.core().attachment_state(), Ok(AttachmentState::Frozen));
    let reply = checked(&mut host, command.clone());
    let generation = host.next_manual_generation;
    let duplicate = checked(&mut host, command);
    assert_eq!(reply.manual_lease, duplicate.manual_lease);
    assert_eq!(host.next_manual_generation, generation);
}

#[test]
fn s2h_recovery_plan_retries_bind_original_client_cut_and_chain() {
    let (_temp, mut host) = fixture();
    let frozen = received_snapshot_for_test(host.core()).unwrap();
    let begin = HostCommand::BeginRecovery {
        client: "r".repeat(64),
        frozen: Some(frozen.clone()),
        pending: vec![],
    };
    let original = checked(&mut host, begin.clone()).recovery.unwrap();
    let retry = checked(&mut host, begin).recovery.unwrap();
    assert_eq!(retry, original);
    for changed in [
        HostCommand::BeginRecovery {
            client: "x".repeat(64),
            frozen: Some(frozen),
            pending: vec![],
        },
        HostCommand::BeginRecovery {
            client: "r".repeat(64),
            frozen: None,
            pending: vec![],
        },
    ] {
        assert!(run(&mut host, changed).error.is_some());
    }
    let changed_chain = HostCommand::BeginRecovery {
        client: "r".repeat(64),
        frozen: Some(original.before.clone()),
        pending: vec![
            serde_json::from_value(serde_json::json!(
                serde_json::json!({"sequence":1,"predecessor":vec![0;32]}).to_string()
            ))
            .unwrap(),
        ],
    };
    assert!(run(&mut host, changed_chain).error.is_some());
    assert_eq!(host.recovery, Some(original));
}

#[test]
fn a_new_foreground_operation_releases_the_acknowledged_reply_before_capturing() {
    let (_temp, mut host) = fixture();
    let roots = host.config.roots.clone();
    let client = "g".repeat(64);
    let save = HostCommand::Save {
        snapshot: received_snapshot_for_test(host.core()).unwrap(),
    };
    let saved = host.execute_operation(save, roots.clone(), Some(&client), Some(1));
    assert!(saved.error.is_none(), "{:?}", saved.error);
    drop(saved);
    let current = host.execute_operation(HostCommand::ReadHistory, roots, Some(&client), Some(2));
    assert!(current.error.is_none(), "{:?}", current.error);
    assert!(
        host.core()
            .history_pool()
            .lease_counts(kinewright_project::operation_history::Role::Ordinary)
            .unwrap()
            .1
            <= 2,
        "ACKNOWLEDGED_FOREGROUND_REPLY_IS_RELEASED_BEFORE_NEXT_CAPTURE"
    );
}

#[test]
fn s2h_foreground_result_replay_is_exact_bounded_and_client_bound() {
    let (_temp, mut host) = fixture();
    let roots = host.config.roots.clone();
    let client = "g".repeat(64);
    let snapshot = received_snapshot_for_test(host.core()).unwrap();
    let save = HostCommand::Save { snapshot };
    let reply = host.execute_operation(save.clone(), roots.clone(), Some(&client), Some(1));
    assert!(reply.error.is_none(), "{:?}", reply.error);
    let root = host.owner().unwrap().root_sha256().unwrap();
    let repeated = host.execute_operation(save, roots.clone(), Some(&client), Some(1));
    assert_eq!(
        serde_json::to_vec(&reply).unwrap(),
        serde_json::to_vec(&repeated).unwrap()
    );
    assert_eq!(host.owner().unwrap().root_sha256().unwrap(), root);
    let branch = host
        .owner_mut()
        .unwrap()
        .fork_durable_branch(
            "M3-interleaved-session",
            kinewright_project::editing_session::DurableBranchRole::Session,
        )
        .unwrap()
        .branch_id()
        .clone();
    let interleaved = run(&mut host, HostCommand::SaveSession { branch });
    assert!(interleaved.error.is_none(), "{:?}", interleaved.error);
    let current_status = serde_json::to_value(host.status()).unwrap();
    let cached_result = host.execute_operation(
        HostCommand::Save {
            snapshot: received_snapshot_for_test(host.core()).unwrap(),
        },
        roots.clone(),
        Some(&client),
        Some(1),
    );
    assert!(cached_result.error.is_none(), "{:?}", cached_result.error);
    assert_eq!(
        serde_json::to_value(&cached_result.status).unwrap(),
        current_status,
        "M3_REPLAY_CURRENT_STATUS"
    );
    let mut exact = cached_result;
    exact.status = reply.status.clone();
    assert_eq!(
        serde_json::to_value(exact).unwrap(),
        serde_json::to_value(reply).unwrap()
    );
    for (identity, operation) in [
        (client.clone(), 0),
        ("x".repeat(64), 1),
        (client.clone(), 1),
    ] {
        assert!(
            host.execute_operation(
                HostCommand::Cancel,
                roots.clone(),
                Some(&identity),
                Some(operation)
            )
            .error
            .is_some()
        );
    }
    let next = host.execute_operation(HostCommand::Status, roots, Some(&client), Some(2));
    assert!(next.error.is_none());
    assert_eq!(host.foreground_reply.as_ref().unwrap().1, 2);
}

#[test]
fn s2h_lost_rescue_result_returns_original_activation_without_second_copy() {
    use kinewright_project::bundle::LoadedBundle;
    let parent = kinewright_media::test_support::workspace_target_dir();
    let temp = kinewright_media::test_support::TempDirectory::new_in(&parent, "fix3-rescue-result");
    let drive = temp.path("drive");
    std::fs::create_dir(&drive).unwrap();
    let core = Core::for_test(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            name: None,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let mut owner = Ja1EditingSession::new(&drive, &core, StorageLimits::default()).unwrap();
    owner
        .first_save(&drive.join("a.kinewright"), StorageLimits::default())
        .unwrap();
    edit(&core, false);
    assert!(io_observer::fail_once("root_parent_sync", || owner.save_active()).is_err());
    let expected = document_for_test(&core);
    std::fs::rename(&drive, temp.path("hidden-drive")).unwrap();
    let roots = SessionRoots {
        read: vec![temp.root().into()],
        write: vec![temp.root().into()],
    };
    let mut host = HostCommandOwner::open(HostConfig {
        data_dir: Some((temp.path("scheduler")).with_extension("voice-data")),
        scheduler_settings: Some(temp.path("scheduler")),
        project: "fix3-rescue-result".into(),
        path: None,
        roots: roots.clone(),
        origin: HostOrigin::Headless,
    })
    .unwrap();
    host.initial_core = core.clone();
    host.owner = Some(owner);
    checked(
        &mut host,
        HostCommand::PreviewRescue {
            snapshot: received_snapshot_for_test(&core).unwrap(),
        },
    );
    let destination = temp.path("rescue.kinewright");
    let command = HostCommand::RescueTo {
        destination: destination.clone(),
    };
    let client = "s".repeat(64);
    let lost = host.execute_operation(command.clone(), roots.clone(), Some(&client), Some(1));
    assert!(lost.error.is_none(), "{:?}", lost.error);
    assert!(lost.activation.is_some());
    assert_eq!(core.attachment_state(), Ok(AttachmentState::Sealed));
    let actor = host.core().actor_identity();
    let root = host.owner().unwrap().root_sha256().unwrap();
    let retry = host.execute_operation(command, roots, Some(&client), Some(1));
    assert_eq!(
        serde_json::to_vec(&retry).unwrap(),
        serde_json::to_vec(&lost).unwrap(),
        "Retry must return the original activation"
    );
    assert_eq!(host.owner().unwrap().root_sha256().unwrap(), root);
    assert_eq!(host.core().actor_identity(), actor);
    assert_eq!(
        host.core().export_history().unwrap().history().document,
        expected
    );
    drop(host);
    let mut loaded = LoadedBundle::read(
        &destination,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let restored = loaded.open_manual().unwrap();
    assert_eq!(
        restored.snapshot_identity().unwrap(),
        lost.history.unwrap().identity()
    );
}

/// S3 C7 fix 1: the reply that confirms a rescue names the rescue copy's
/// project. Studio keys feedback recovery by that status, so notes typed after
/// a rescue are never filed under the original project.
#[test]
fn c7_fix1_rescue_status_names_the_rescue_copy() {
    let parent = kinewright_media::test_support::workspace_target_dir();
    let temp = kinewright_media::test_support::TempDirectory::new_in(&parent, "c7-fix1-rescue");
    let drive = temp.path("drive");
    std::fs::create_dir(&drive).unwrap();
    let core = Core::for_test(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            name: None,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let mut owner = Ja1EditingSession::new(&drive, &core, StorageLimits::default()).unwrap();
    owner
        .first_save(&drive.join("a.kinewright"), StorageLimits::default())
        .unwrap();
    let original = owner.project_id().unwrap();
    edit(&core, false);
    assert!(io_observer::fail_once("root_parent_sync", || owner.save_active()).is_err());
    std::fs::rename(&drive, temp.path("hidden-drive")).unwrap();
    let roots = SessionRoots {
        read: vec![temp.root().into()],
        write: vec![temp.root().into()],
    };
    let mut host = HostCommandOwner::open(HostConfig {
        data_dir: Some((temp.path("scheduler")).with_extension("voice-data")),
        scheduler_settings: Some(temp.path("scheduler")),
        project: "c7-fix1-rescue".into(),
        path: None,
        roots: roots.clone(),
        origin: HostOrigin::Headless,
    })
    .unwrap();
    host.initial_core = core.clone();
    host.owner = Some(owner);
    checked(
        &mut host,
        HostCommand::PreviewRescue {
            snapshot: received_snapshot_for_test(&core).unwrap(),
        },
    );
    let destination = temp.path("rescue.kinewright");
    let reply = host.execute_operation(
        HostCommand::RescueTo {
            destination: destination.clone(),
        },
        roots,
        Some(&"s".repeat(64)),
        Some(1),
    );
    assert!(reply.error.is_none(), "{:?}", reply.error);
    drop(host);
    let rescued = Ja1EditingSession::open(
        &destination,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap()
    .project_id()
    .unwrap();
    println!(
        "C7_RESCUE_IDENTITY original={} rescued={} status={:?}",
        original.as_str(),
        rescued.as_str(),
        reply.status.project_id
    );
    assert_eq!(reply.status.project_id, Some(rescued), "C7_RESCUE_IDENTITY");
}
