#![allow(
    clippy::too_many_lines,
    reason = "complete real process recovery matrices retain their independent checks together"
)]
use super::*;

#[cfg(target_os = "linux")]
pub(super) fn restart_case(committed: bool, floor: bool, lose_transition_reply: bool, suffix: u8) {
    use kinewright_core::{
        Command as Edit, Event, HistoryTransition, Operation, Track, TrackId, TrackKind,
    };
    use kinewright_project::{
        editing_session::Ja1EditingSession, receipts::ReceiptLookup, storage_budget::StorageLimits,
    };
    let root = temp("c10-restart");
    let path = root.path("manual.kinewright");
    let gui = Core::spawn(Document {
        duration: kinewright_core::TimeCode(90),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![serde_json::from_value(json!({"id":1,"asset":0,"source_range":{"start":0,"end":90},"content":{"solid":{"r":20,"g":40,"b":60}},"timeline_start":0,"effects":[],"transition_in":null})).unwrap()],
        }],
        ..Document::default()
    })
    .unwrap();
    let mut source = Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
    source.first_save(&path, StorageLimits::default()).unwrap();
    source.prepare_host_attachment().unwrap();
    let initial = source.freeze_host_cut().unwrap();
    source.complete_host_cut(&initial, None, None).unwrap();
    let (original_lineage, original_inputs) = {
        let owner = kinewright_project::bundle::LoadedBundle::read(&path)
            .unwrap()
            .into_writer()
            .unwrap();
        let branch = owner.manual_branch_id();
        let lineage = (*owner.branch_lineage(branch).unwrap()).clone();
        assert_eq!(lineage.current.len(), 1);
        (
            lineage,
            owner.checkpoint(branch).unwrap().input_manifest_ref.clone(),
        )
    };
    let mut host = ProcessHost::start(config(root.root(), Some(path.clone())));
    let identity = "e".repeat(64);
    let lease = successful(
        &host.client,
        HostCommand::AcquireManual {
            client: identity.clone(),
        },
    );
    let permit = source
        .bind_manual_producer(lease.manual_lease.unwrap(), lease.manual_inputs.unwrap())
        .unwrap();
    let client = host.client.with_manual_editor(identity.clone());
    // A closed command gives the floor a legal prefix, then B and C share a gesture.
    assert!(matches!(
        permit
            .request(Edit::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false
            }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let accepted = permit.pending().unwrap();
    let mut original_batches: Vec<String> = accepted.iter().map(|m| m.batch_id.clone()).collect();
    let reply = successful(
        &client,
        HostCommand::CommitHistory {
            client: identity.clone(),
            messages: accepted,
        },
    );
    source
        .acknowledge_manual(&permit, reply.manual_ack)
        .unwrap();
    let drag = |locked| Edit::DoBatchCoalesced {
        operations: vec![Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }],
        coalesce_key: "A-B-C".into(),
    };
    assert!(matches!(
        permit.request(drag(true)).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let durable_drag = permit.pending().unwrap();
    original_batches.extend(durable_drag.iter().map(|m| m.batch_id.clone()));
    let reply = successful(
        &client,
        HostCommand::CommitHistory {
            client: identity.clone(),
            messages: durable_drag,
        },
    );
    source
        .acknowledge_manual(&permit, reply.manual_ack)
        .unwrap();
    let command = match suffix {
        1 | 2 => Edit::Undo,
        _ => drag(false),
    };
    assert!(matches!(
        permit.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
    if suffix == 2 {
        assert!(matches!(
            permit.request(Edit::Redo).unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let pending = permit.pending().unwrap();
    original_batches.extend(pending.iter().map(|m| m.batch_id.clone()));
    let accepted_cut = gui.export_history().unwrap();
    if committed {
        // The publication really completes, but the original-envelope reply is dropped.
        let dropped = successful(
            &client,
            HostCommand::CommitHistory {
                client: identity.clone(),
                messages: pending.clone(),
            },
        );
        assert_eq!(dropped.manual_ack, pending);
    }
    let cut = source.freeze_host_cut().unwrap();
    assert_eq!(cut.history(), &accepted_cut);
    let death = Instant::now();
    host.child.kill().unwrap();
    host.child.wait().unwrap();
    host = ProcessHost::start(config(root.root(), Some(path.clone())));
    let successor = "a".repeat(64);
    let client = host.client.with_manual_editor(successor.clone());
    let prepared = successful(
        &client,
        HostCommand::BeginRecovery {
            client: successor.clone(),
            frozen: Some(HostSnapshot::from_history(accepted_cut.clone()).unwrap()),
            pending: pending.clone(),
            discard_batches: floor.then_some(1),
        },
    );
    assert!(prepared.status.frozen);
    let mut barrier = prepared.recovery.unwrap();
    let mut local_cut = cut;
    let mut still_pending = pending.clone();
    loop {
        assert!(
            host.client
                .request(HostCommand::ConfirmBarrier {
                    barrier: Box::new(barrier.clone()),
                    installed: barrier.after.clone()
                })
                .unwrap()
                .error
                .is_some(),
            "no lease before durable publication"
        );
        let publication = successful(
            &host.client,
            HostCommand::PublishBarrier {
                barrier: Box::new(barrier.clone()),
            },
        );
        assert!(publication.manual_lease.is_none());
        assert_eq!(publication.history.as_ref(), Some(&barrier.after.history));
        assert_eq!(
            successful(&host.client, HostCommand::ReadHistory)
                .history
                .as_ref(),
            Some(&barrier.after.history),
            "the host actor itself installed the result before restart"
        );
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
        local_cut = source
            .install_manual_barrier(&local_cut, barrier.after.history.clone(), still_pending)
            .unwrap();
        still_pending = vec![];
        assert!(matches!(
            permit.request(Edit::Undo).unwrap(),
            Event::MutationRefused { .. }
        ));
        let reply = successful(
            &host.client,
            HostCommand::ConfirmBarrier {
                barrier: Box::new(barrier.clone()),
                installed: HostSnapshot::capture(&gui).unwrap(),
            },
        );
        if let Some(next) = reply.recovery {
            assert!(reply.manual_lease.is_none());
            barrier = next;
        } else {
            let lease = reply.manual_lease.unwrap();
            source.resume_manual_barrier(&local_cut).unwrap();
            let fresh = source
                .bind_manual_producer(lease, reply.manual_inputs.unwrap())
                .unwrap();
            assert!(matches!(
                permit.request(drag(true)).unwrap(),
                Event::MutationRefused { .. }
            ));
            let elapsed = death.elapsed().as_secs_f64() * 1000.0;
            println!(
                "C10_RECOVERY_MS committed={committed} floor={floor} dropped={lose_transition_reply} elapsed={elapsed:.3}"
            );
            assert_eq!(gui.export_history().unwrap(), barrier.after.history);
            let expected = if floor {
                HistoryTransition::floor(accepted_cut.clone(), 1)
                    .unwrap()
                    .after()
                    .clone()
            } else {
                accepted_cut.clone()
            };
            assert_eq!(
                barrier.after.history,
                *HistoryTransition::recovery_close(expected.clone(), &expected)
                    .unwrap()
                    .after()
            );
            // A new gesture cannot continue the recovered drag's original undo target.
            assert!(matches!(
                fresh.request(drag(true)).unwrap(),
                Event::DocumentChanged { .. }
            ));
            assert!(matches!(
                fresh.request(Edit::Undo).unwrap(),
                Event::DocumentChanged { .. }
            ));
            assert_eq!(
                gui.export_history().unwrap().document,
                accepted_cut.document
            );
            break;
        }
    }
    let stale_client = host.client.with_manual_editor(identity.clone());
    assert!(
        stale_client
            .request(HostCommand::ReadHistory)
            .unwrap()
            .error
            .is_none()
    );
    assert!(
        stale_client
            .request(HostCommand::CommitHistory {
                client: identity,
                messages: pending.clone()
            })
            .unwrap()
            .error
            .is_some(),
        "the former editor stays a reader after lease transfer"
    );
    host.client = host.client.with_manual_editor(successor);
    host.orderly();
    let owner = kinewright_project::bundle::LoadedBundle::read(&path)
        .unwrap()
        .into_writer()
        .unwrap();
    let checkpoint = owner.checkpoint(owner.manual_branch_id()).unwrap();
    let retained = &original_batches[usize::from(floor)..];
    assert_eq!(
        checkpoint.batch_ids[..retained.len()]
            .iter()
            .map(kinewright_project::bundle_records::BatchId::as_str)
            .collect::<Vec<_>>(),
        retained.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(checkpoint.floor_generation, u64::from(floor));
    assert_eq!(checkpoint.input_manifest_ref, original_inputs);
    assert_eq!(
        checkpoint.applied_operations_digest,
        kinewright_media::sha256_bytes(
            &serde_json::to_vec(&barrier.after.history.applied_operations()).unwrap()
        )
    );
    let lineage = owner.branch_lineage(owner.manual_branch_id()).unwrap();
    for map in std::iter::once(&lineage.baseline)
        .chain(std::iter::once(&lineage.current))
        .chain(&lineage.undo)
        .chain(&lineage.redo)
    {
        assert_eq!(
            map.get(&kinewright_core::ClipId(1)),
            original_lineage.current.get(&kinewright_core::ClipId(1)),
            "occurrence lineage survives floor, restart and gesture close"
        );
    }
    let original = kinewright_project::manual_history::AcceptedManualCommand::decode(&pending[0])
        .unwrap()
        .envelope;
    let receipt = owner.receipt_lookup(&original).unwrap();
    let ReceiptLookup::Committed(receipt) = receipt else {
        panic!("{receipt:?}")
    };
    assert_eq!(receipt.recovery_origin.is_some(), !committed);
    for accepted in pending.iter().skip(1) {
        let original = kinewright_project::manual_history::AcceptedManualCommand::decode(accepted)
            .unwrap()
            .envelope;
        let ReceiptLookup::Committed(receipt) = owner.receipt_lookup(&original).unwrap() else {
            panic!("missing original receipt")
        };
        assert_eq!(receipt.recovery_origin.is_some(), !committed);
    }
}

#[cfg(all(target_os = "linux", feature = "eval-harness"))]
pub(super) fn pressure_floor() {
    use kinewright_core::{
        Command as Edit, Event, HistoryTransition, Operation, Track, TrackId, TrackKind,
    };
    use kinewright_project::{
        bundle::LoadedBundle,
        bundle_commit::{bound_history_probe, history_cost_probe},
        editing_session::Ja1EditingSession,
        manual_history::AcceptedManualCommand,
        receipts::ReceiptLookup,
        storage_budget::StorageLimits,
    };
    let root = temp("c10-pressure");
    let path = root.path("pressure.kinewright");
    let gui = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let mut source = Ja1EditingSession::new(root.root(), &gui, StorageLimits::default()).unwrap();
    source.first_save(&path, StorageLimits::default()).unwrap();
    source.prepare_host_attachment().unwrap();
    let initial = source.freeze_host_cut().unwrap();
    source.complete_host_cut(&initial, None, None).unwrap();
    let mut host = HostCommandOwner::open(config(root.root(), Some(path.clone()))).unwrap();
    let old_client = "c".repeat(64);
    let grant = host.execute(
        HostCommand::AcquireManual {
            client: old_client.clone(),
        },
        roots(root.root()),
    );
    assert!(grant.error.is_none(), "{:?}", grant.error);
    let permit = source
        .bind_manual_producer(grant.manual_lease.unwrap(), grant.manual_inputs.unwrap())
        .unwrap();
    for index in 0..12 {
        assert!(matches!(
            permit
                .request(Edit::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: index % 2 == 0
                }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let mut original_batches: Vec<String> = permit
        .pending()
        .unwrap()
        .iter()
        .map(|m| m.batch_id.clone())
        .collect();
    let reply = host.execute(
        HostCommand::CommitHistory {
            client: old_client,
            messages: permit.pending().unwrap(),
        },
        roots(root.root()),
    );
    assert!(reply.error.is_none(), "{:?}", reply.error);
    source
        .acknowledge_manual(&permit, reply.manual_ack)
        .unwrap();
    let prefix = gui.export_history().unwrap();
    for index in 0..8 {
        assert!(matches!(
            permit
                .request(Edit::DoBatchCoalesced {
                    operations: vec![Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: index % 2 == 0
                    }],
                    coalesce_key: "pressure-drag".into()
                })
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let pending = permit.pending().unwrap();
    assert_eq!(pending.len(), 8);
    original_batches.extend(pending.iter().map(|m| m.batch_id.clone()));
    let accepted = gui.export_history().unwrap();
    let limits = StorageLimits {
        branch_history_bytes: history_cost_probe(&prefix, &StorageLimits::default()).unwrap() + 512,
        ..StorageLimits::default()
    };
    assert!(history_cost_probe(&accepted, &limits).unwrap() > limits.branch_history_bytes);
    let selected = bound_history_probe(&accepted, &limits).unwrap();
    assert!(selected.floor_offset > accepted.floor_offset);
    let expected_close = HistoryTransition::recovery_close(selected.clone(), &selected)
        .unwrap()
        .after()
        .clone();
    assert!(history_cost_probe(&expected_close, &limits).unwrap() <= limits.branch_history_bytes);
    let mut cut = source.freeze_host_cut().unwrap();
    drop(host);
    let mut host = HostCommandOwner::open(config(root.root(), Some(path.clone()))).unwrap();
    host.restrict_manual_limits_probe(limits).unwrap();
    let client = "d".repeat(64);
    let reply = host.execute(
        HostCommand::BeginRecovery {
            client: client.clone(),
            frozen: Some(HostSnapshot::from_history(accepted.clone()).unwrap()),
            pending: pending.clone(),
            discard_batches: None,
        },
        roots(root.root()),
    );
    assert!(reply.error.is_none(), "{:?}", reply.error);
    let mut barrier = reply.recovery.unwrap();
    assert!(
        barrier.envelope.manual.is_some(),
        "the batch root selects its shared floor before close"
    );
    assert_eq!(barrier.after.history, selected);
    let mut expected_pending = pending.clone();
    loop {
        let published = host.execute(
            HostCommand::PublishBarrier {
                barrier: Box::new(barrier.clone()),
            },
            roots(root.root()),
        );
        assert!(published.error.is_none(), "{:?}", published.error);
        assert!(published.manual_lease.is_none());
        assert_eq!(
            host.execute(HostCommand::ReadHistory, roots(root.root()))
                .history
                .as_ref(),
            Some(&barrier.after.history)
        );
        cut = source
            .install_manual_barrier(&cut, barrier.after.history.clone(), expected_pending)
            .unwrap();
        expected_pending = vec![];
        let reply = host.execute(
            HostCommand::ConfirmBarrier {
                barrier: Box::new(barrier.clone()),
                installed: HostSnapshot::capture(&gui).unwrap(),
            },
            roots(root.root()),
        );
        assert!(reply.error.is_none(), "{:?}", reply.error);
        if let Some(next) = reply.recovery {
            assert!(reply.manual_lease.is_none());
            barrier = next;
        } else {
            source.resume_manual_barrier(&cut).unwrap();
            let fresh = source
                .bind_manual_producer(reply.manual_lease.unwrap(), reply.manual_inputs.unwrap())
                .unwrap();
            assert_eq!(gui.export_history().unwrap(), expected_close);
            assert!(matches!(
                permit.request(Edit::Undo).unwrap(),
                Event::MutationRefused { .. }
            ));
            assert!(matches!(
                fresh.request(Edit::Undo).unwrap(),
                Event::DocumentChanged { .. }
            ));
            break;
        }
    }
    drop(host);
    let owner = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    let checkpoint = owner.checkpoint(owner.manual_branch_id()).unwrap();
    let retained = usize::try_from(selected.floor_offset - accepted.floor_offset).unwrap();
    assert_eq!(
        checkpoint.batch_ids[..selected.batches.len()]
            .iter()
            .map(kinewright_project::bundle_records::BatchId::as_str)
            .collect::<Vec<_>>(),
        original_batches[retained..]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "batch IDs survive the automatic floor"
    );
    assert!(retained > 0);
    for original in &pending {
        let envelope = AcceptedManualCommand::decode(original).unwrap().envelope;
        let ReceiptLookup::Committed(receipt) = owner.receipt_lookup(&envelope).unwrap() else {
            panic!("missing original link")
        };
        assert!(receipt.recovery_origin.is_some());
        let kinewright_project::receipts::ReceiptResult::Edit { batch_id, .. } = receipt.result
        else {
            panic!("edit receipt required")
        };
        assert_eq!(
            batch_id
                .as_ref()
                .map(kinewright_project::bundle_records::BatchId::as_str),
            Some(original.batch_id.as_str())
        );
    }
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
    use kinewright_project::{
        bundle::LoadedBundle, editing_session::Ja1EditingSession, storage_budget::StorageLimits,
    };
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
        let gui = Core::spawn(Document {
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
        source.prepare_host_attachment().unwrap();
        let initial = source.freeze_host_cut().unwrap();
        source.complete_host_cut(&initial, None, None).unwrap();
        let mut host = ProcessHost::start(config(root.root(), Some(path.clone())));
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
        let durable = gui.export_history().unwrap();
        permit.request(command(true)).unwrap();
        let pending = permit.pending().unwrap();
        let cut = source.freeze_host_cut().unwrap();
        host.child.kill().unwrap();
        host.child.wait().unwrap();
        if case == 2 {
            let mut owner = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
            let restored = Core::spawn(Document::default()).unwrap();
            owner
                .attach_core(&restored)
                .unwrap()
                .restore_history(durable.clone())
                .unwrap();
            let inputs = owner
                .checkpoint(owner.manual_branch_id())
                .unwrap()
                .input_manifest_ref
                .clone();
            owner.clear_current_receipt_details_probe();
            owner.save(&restored, &inputs).unwrap();
        }
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
        let mut frozen = HostSnapshot::from_history(cut.history().clone()).unwrap();
        if case == 1 {
            frozen.history_sha256 = "0".repeat(64);
        }
        let reply = host
            .client
            .request(HostCommand::BeginRecovery {
                client: identity.clone(),
                frozen: (case != 0).then_some(frozen),
                pending: if case == 0 { vec![] } else { pending.clone() },
                discard_batches: None,
            })
            .unwrap();
        if case == 0 {
            let barrier = reply.recovery.unwrap();
            assert_eq!(
                barrier.before.history, durable,
                "no GUI uses only the acknowledged prefix"
            );
            // Confirmation before publication cannot grant a lease, even for an identity close.
            assert!(
                host.client
                    .request(HostCommand::ConfirmBarrier {
                        barrier: Box::new(barrier.clone()),
                        installed: barrier.after.clone()
                    })
                    .unwrap()
                    .error
                    .is_some()
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
            assert!(reply.error.is_some());
            assert!(reply.status.frozen);
            assert!(
                host.client
                    .request(HostCommand::AcquireManual { client: identity })
                    .unwrap()
                    .error
                    .is_some()
            );
            assert_eq!(permit.pending().unwrap(), pending);
            assert_eq!(gui.export_history().unwrap(), *cut.history());
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
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
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
    assert!(
        host.client
            .request(HostCommand::ConfirmBarrier {
                barrier: Box::new(barrier.clone()),
                installed: barrier.after.clone()
            })
            .unwrap()
            .error
            .is_some()
    );
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
    let owner = kinewright_project::bundle::LoadedBundle::read(&path)
        .unwrap()
        .into_writer()
        .unwrap();
    let reference = owner
        .evidence_refs()
        .iter()
        .find(|r| r.record_kind == kinewright_project::bundle_records::RecordKind::RecoveryClose)
        .unwrap();
    let record = owner
        .store()
        .read_record::<kinewright_project::bundle_records::RecoveryCloseCheckpoint>(reference)
        .unwrap();
    assert!(record.reconciled_cut.is_none());
}
