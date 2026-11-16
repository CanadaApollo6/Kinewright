#![allow(
    clippy::too_many_lines,
    reason = "complete real process recovery matrices retain their independent checks together"
)]
use super::*;

#[cfg(target_os = "linux")]
pub(super) fn restart_case(
    committed: bool,
    pressure: bool,
    lose_transition_reply: bool,
    suffix: u8,
) {
    use kinewright_core::{Command as Edit, Event, Operation, Track, TrackId, TrackKind};
    use kinewright_project::{
        editing_session::Ja1EditingSession, operation_protocol::PENDING_COMMANDS,
        storage_budget::StorageLimits,
    };
    let root = temp("c10-restart");
    let path = root.path("manual.kinewright");
    let gui = Core::for_test(Document {
        duration: kinewright_core::TimeCode(90),
        tracks: vec![Track { id: TrackId(1), kind: TrackKind::Video, sync_lock: true,
            clips: vec![serde_json::from_value(json!({"id":1,"asset":0,"source_range":{"start":0,"end":90},"content":{"solid":{"r":20,"g":40,"b":60}},"timeline_start":0,"effects":[],"transition_in":null})).unwrap()] }],
        ..Document::default()
    }).unwrap();
    let mut source = Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
    source.first_save(&path, StorageLimits::default()).unwrap();
    let (configuration, _origin_callback) = handoff_config(&mut source, root.root());
    let initial = source.freeze_host_cut().unwrap();
    source.complete_host_cut(&initial, None, None).unwrap();
    let original_occurrence =
        gui.boundary(0).unwrap().lineage().occurrences[&kinewright_core::ClipId(1)].clone();
    let mut host = ProcessHost::start(configuration);
    let identity = "e".repeat(64);
    let lease = successful(
        &host.client,
        HostCommand::AcquireManual {
            client: identity.clone(),
        },
    );
    let permit = source
        .bind_manual_producer(
            lease.manual_lease.unwrap(),
            lease.manual_inputs.unwrap(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
        .unwrap();
    let client = host.client.with_manual_editor(identity.clone());
    let lock = |locked| {
        Edit::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        })
    };
    let drag = |locked| Edit::DoBatchCoalesced {
        operations: vec![Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }],
        coalesce_key: "A-B-C".into(),
    };
    let mut prefix_receipts = Vec::new();
    for command in [lock(false), drag(true)] {
        assert!(matches!(
            permit.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let reply = successful(
            &client,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: permit.pending().unwrap(),
            },
        );
        prefix_receipts.extend(reply.manual_ack.iter().cloned());
        source
            .acknowledge_manual(&permit, reply.manual_ack)
            .unwrap();
    }
    permit
        .request(if suffix == 1 || suffix == 2 {
            Edit::Undo
        } else {
            drag(false)
        })
        .unwrap();
    if suffix == 2 {
        permit.request(Edit::Redo).unwrap();
    }
    if pressure {
        while permit.pending().unwrap().len() < PENDING_COMMANDS {
            permit.close_gesture().unwrap();
        }
        let before = gui.change_id().unwrap();
        assert!(
            permit.request(lock(true)).is_err(),
            "C10_FULL_OUTBOX_REFUSES_BEFORE_ACCEPTANCE"
        );
        assert_eq!(gui.change_id().unwrap(), before);
    }
    let pending = permit.pending().unwrap();
    let accepted = serde_json::from_value::<HostSnapshot>(
        serde_json::to_value(HostSnapshot::capture(&gui).unwrap()).unwrap(),
    )
    .unwrap();
    if committed {
        let dropped = successful(
            &client,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: pending.clone(),
            },
        );
        assert_eq!(dropped.manual_ack.len(), pending.len());
        for (receipt, encoded) in dropped.manual_ack.iter().zip(&pending) {
            assert_eq!(
                receipt.command,
                encoded.decode().unwrap().digest().unwrap(),
                "C10_LOST_ACK_NAMES_ORIGINAL_PAYLOAD"
            );
        }
    }
    let cut = source.freeze_host_cut().unwrap();
    assert_eq!(cut.identity(), accepted.history.identity());
    let mut endpoint = source
        .manual_recovery_endpoint(&cut, pending.clone())
        .unwrap();
    let death = Instant::now();
    host.child.kill().unwrap();
    host.child.wait().unwrap();
    host = ProcessHost::start(config(root.root(), Some(path.clone())));
    let successor = "a".repeat(64);
    let client = host.client.with_manual_editor(successor.clone());
    if pressure {
        let refused = client
            .request(HostCommand::BeginRecovery {
                client: successor.clone(),
                frozen: Some(accepted.clone()),
                pending: pending.clone(),
                discard_batches: Some(1),
            })
            .unwrap();
        assert!(
            refused.error.is_some(),
            "C10_ONLY_EXECUTOR_CAN_SELECT_A_FOLD"
        );
    }
    let prepared = successful(
        &client,
        HostCommand::BeginRecovery {
            client: successor.clone(),
            frozen: Some(accepted.clone()),
            pending: pending.clone(),
            discard_batches: None,
        },
    );
    let barrier = prepared.recovery.unwrap();
    assert_eq!(
        barrier.acknowledged.len(),
        pending.len(),
        "C10_EVERY_ORIGINAL_SUFFIX_MESSAGE_HAS_A_RECEIPT"
    );
    let publication = successful(
        &host.client,
        HostCommand::PublishBarrier {
            barrier: Box::new(barrier.clone()),
        },
    );
    assert_eq!(publication.history.as_ref(), Some(&barrier.after.history));
    if lose_transition_reply {
        host.child.kill().unwrap();
        host.child.wait().unwrap();
        host = ProcessHost::start(config(root.root(), Some(path.clone())));
        let recovered = successful(
            &host.client,
            HostCommand::PublishBarrier {
                barrier: Box::new(barrier.clone()),
            },
        );
        assert_eq!(recovered.barrier_checkpoint, publication.barrier_checkpoint);
    }
    endpoint
        .install(barrier.after.history.clone(), barrier.acknowledged.clone())
        .unwrap();
    assert!(
        permit.request(Edit::Undo).is_err(),
        "C10_OLD_PERMIT_STAYS_FENCED_DURING_INSTALL"
    );
    let reply = successful(
        &host.client,
        HostCommand::ConfirmBarrier {
            barrier: Box::new(barrier.clone()),
            installed: HostSnapshot::capture(&gui).unwrap(),
        },
    );
    let digest = reply.manual_lease.as_ref().unwrap().snapshot.to_string();
    let fresh = endpoint
        .finish(
            reply.manual_lease.unwrap(),
            reply.manual_inputs.unwrap(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
        .unwrap();
    source.finish_host_manual_recovery(&digest).unwrap();
    assert!(
        permit.request(drag(true)).is_err(),
        "C10_TRANSFER_NEVER_REVIVES_STALE_PERMIT"
    );
    assert_eq!(
        gui.snapshot_identity().unwrap(),
        barrier.after.history.identity(),
        "C10_RESTART_INSTALL_IS_EXACT"
    );
    let installed = Core::peer_for_test(barrier.after.history.clone()).unwrap();
    let before = installed.export_history().unwrap();
    assert_eq!(
        before.history().document.as_ref(),
        accepted
            .history
            .clone()
            .validate(&kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default()
            ))
            .unwrap()
            .document()
    );
    assert!(
        before.history().epoch > cut.change().position.epoch,
        "C10_RECOVERY_STARTS_A_FRESH_EPOCH"
    );
    assert_eq!(
        gui.boundary(before.history().undo.len())
            .unwrap()
            .lineage()
            .occurrences[&kinewright_core::ClipId(1)],
        original_occurrence,
        "C10_OCCURRENCE_SURVIVES_RESTART_AND_INSTALL"
    );
    drop(before);
    drop(installed);
    fresh.request(drag(true)).unwrap();
    fresh.request(Edit::Undo).unwrap();
    assert_eq!(
        gui.export_history().unwrap().history().document.as_ref(),
        accepted
            .history
            .clone()
            .validate(&kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default()
            ))
            .unwrap()
            .document(),
        "C10_NEW_GESTURE_UNDO_RESTORES_RECOVERED_DOCUMENT"
    );
    assert!(
        host.client
            .with_manual_editor(identity.clone())
            .request(HostCommand::CommitHistory {
                client: identity,
                messages: pending.clone()
            })
            .unwrap()
            .error
            .is_some(),
        "C10_FORMER_EDITOR_CANNOT_WRITE"
    );
    println!(
        "C10_RECOVERY_MS committed={committed} pressure={pressure} dropped={lose_transition_reply} elapsed={:.3}",
        death.elapsed().as_secs_f64() * 1000.0
    );
    host.client = host.client.with_manual_editor(successor);
    host.orderly();
    let reopened = Ja1EditingSession::open(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    assert_eq!(
        reopened.core().snapshot_identity().unwrap(),
        barrier.after.history.identity(),
        "C10_RECOVERY_COLD_OPEN_RETAINS_EXACT_OWNER"
    );
    prefix_receipts.extend(barrier.acknowledged.iter().cloned());
    let retired = prefix_receipts.len().saturating_sub(PENDING_COMMANDS);
    prefix_receipts.drain(..retired);
    assert_eq!(
        reopened
            .installed_manual_receipts(accepted.history.identity())
            .unwrap()
            .unwrap(),
        prefix_receipts,
        "C10_REOPEN_RETAINS_ORIGINAL_ACKNOWLEDGEMENTS"
    );
}

#[cfg(all(target_os = "linux", feature = "eval-harness"))]
pub(super) fn pressure_floor() {
    // Capacity now belongs to the executor and its bounded stream, never a caller-selected floor.
    restart_case(false, true, false, 2);
}

pub(super) fn fast_suffix() {
    #[cfg(target_os = "linux")]
    for committed in [false, true] {
        restart_case(committed, false, false, 0);
    }
    #[cfg(not(target_os = "linux"))]
    protocol_refusal();
}
pub(super) fn fast_floor() {
    #[cfg(target_os = "linux")]
    restart_case(false, true, true, 0);
    #[cfg(not(target_os = "linux"))]
    protocol_refusal();
}

#[cfg(not(target_os = "linux"))]
fn protocol_refusal() {
    let root = temp("c10-native-refusal");
    let mut host = ProcessHost::start(config(root.root(), None));
    let reply = host
        .client
        .request(HostCommand::BeginRecovery {
            client: "e".repeat(64),
            frozen: None,
            pending: vec![],
            discard_batches: None,
        })
        .unwrap();
    assert!(reply.error.is_some());
    assert!(reply.manual_lease.is_none());
    assert!(
        host.client
            .request(HostCommand::AcquireManual {
                client: "e".repeat(64)
            })
            .unwrap()
            .error
            .is_some()
    );
    host.orderly();
}

#[cfg(target_os = "linux")]
pub(super) fn prefix_and_pause_cases(case_filter: Option<usize>) {
    use kinewright_core::{Command as Edit, Event, Operation, Track, TrackId, TrackKind};
    use kinewright_project::{editing_session::Ja1EditingSession, storage_budget::StorageLimits};
    for case in 0..4 {
        if case_filter.is_some_and(|selected| selected != case) {
            continue;
        }
        let root = temp("c10-prefix-pause");
        let path = if case == 3 {
            std::fs::create_dir(root.path("drive")).unwrap();
            root.path("drive/prefix.kinewright")
        } else {
            root.path("prefix.kinewright")
        };
        let gui = Core::for_test(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut source =
            Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
        source.first_save(&path, StorageLimits::default()).unwrap();
        let (configuration, _origin_callback) = handoff_config(&mut source, root.root());
        let initial = source.freeze_host_cut().unwrap();
        source.complete_host_cut(&initial, None, None).unwrap();
        let mut host = ProcessHost::start(configuration);
        let identity = "f".repeat(64);
        let granted = successful(
            &host.client,
            HostCommand::AcquireManual {
                client: identity.clone(),
            },
        );
        let permit = source
            .bind_manual_producer(
                granted.manual_lease.unwrap(),
                granted.manual_inputs.unwrap(),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            )
            .unwrap();
        let command = |locked| Edit::DoBatchCoalesced {
            operations: vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked,
            }],
            coalesce_key: "prefix-only".into(),
        };
        assert!(matches!(
            permit.request(command(false)).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let reply = successful(
            &host.client,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: permit.pending().unwrap(),
            },
        );
        source
            .acknowledge_manual(&permit, reply.manual_ack)
            .unwrap();
        let durable = gui.portable_history().unwrap();
        permit.request(command(true)).unwrap();
        let pending = permit.pending().unwrap();
        let cut = source.freeze_host_cut().unwrap();
        host.child.kill().unwrap();
        host.child.wait().unwrap();
        if case == 3 {
            let expected_root = std::fs::read(&path).unwrap();
            std::fs::rename(root.path("drive"), root.path("old-drive")).unwrap();
            copy_tree(&root.path("old-drive"), &root.path("drive"));
            assert_eq!(
                std::fs::read(&path).unwrap(),
                expected_root,
                "the replacement has identical root bytes but a different native parent"
            );
        }
        host = ProcessHost::start(config(root.root(), Some(path.clone())));
        let mut frozen = HostSnapshot::capture(&gui).unwrap();
        let mut transmitted = pending.clone();
        if case == 2 {
            let mut forged = serde_json::to_value(&transmitted[0]).unwrap();
            forged["sequence"] = json!(99);
            transmitted[0] = serde_json::from_value(forged).unwrap();
        }
        if case == 1 {
            frozen.history_sha256 = "0".repeat(64);
        }
        let reply = host
            .client
            .request(HostCommand::BeginRecovery {
                client: identity.clone(),
                frozen: (case != 0).then_some(frozen),
                pending: if case == 0 {
                    vec![]
                } else {
                    transmitted.clone()
                },
                discard_batches: None,
            })
            .unwrap();
        if case == 0 {
            let barrier = reply.recovery.unwrap();
            assert_eq!(
                barrier.before.history, durable,
                "no GUI uses only the acknowledged prefix"
            );
            let published = successful(
                &host.client,
                HostCommand::PublishBarrier {
                    barrier: Box::new(barrier.clone()),
                },
            );
            let result = successful(
                &host.client,
                HostCommand::ConfirmBarrier {
                    barrier: Box::new(barrier.clone()),
                    installed: HostSnapshot::from_history(published.history.unwrap()).unwrap(),
                },
            );
            assert!(result.manual_lease.is_some());
            host.client = host.client.with_manual_editor(identity.clone());
        } else {
            assert!(
                reply.error.is_some(),
                "C10_ORIGINAL_NAMESPACE_AND_CHAIN_MUST_VALIDATE case={case}"
            );
            assert!(reply.status.frozen);
            assert!(
                host.client
                    .request(HostCommand::AcquireManual { client: identity })
                    .unwrap()
                    .error
                    .is_some()
            );
            assert_eq!(permit.pending().unwrap(), pending);
            assert_eq!(gui.snapshot_identity().unwrap(), cut.identity());
        }
        host.orderly();
    }
}

#[cfg(target_os = "linux")]
fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let to = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn empty_acknowledged_prefix() {
    use kinewright_project::bundle::{BundleWriteAdmission, ProjectBundle};
    let root = temp("c10-empty-prefix");
    let path = root.path("empty.kinewright");
    let core = Core::for_test(Document::default()).unwrap();
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    drop(owner);
    let mut host = ProcessHost::start(config(root.root(), Some(path.clone())));
    let identity = "a".repeat(64);
    let reply = successful(
        &host.client,
        HostCommand::BeginRecovery {
            client: identity.clone(),
            frozen: None,
            pending: vec![],
            discard_batches: None,
        },
    );
    let barrier = reply.recovery.unwrap();
    successful(
        &host.client,
        HostCommand::PublishBarrier {
            barrier: Box::new(barrier.clone()),
        },
    );
    let granted = successful(
        &host.client,
        HostCommand::ConfirmBarrier {
            barrier: Box::new(barrier.clone()),
            installed: barrier.after,
        },
    );
    assert!(granted.manual_lease.is_some());
    host.client = host.client.with_manual_editor(identity);
    host.orderly();
    let owner = kinewright_project::bundle::LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap()
    .into_writer()
    .unwrap();
    let checkpoint = owner.checkpoint(owner.manual_branch_id()).unwrap();
    assert!(
        checkpoint.install.as_ref().unwrap().stream.is_none(),
        "C10_EMPTY_PREFIX_INSTALL_HAS_NO_INVENTED_RECEIPT"
    );
    assert_eq!(checkpoint.position.position, 0);
}
