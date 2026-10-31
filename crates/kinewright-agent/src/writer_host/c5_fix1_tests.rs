fn c5_fix1_limited_fixture() -> Fixture {
    c5_fix1_fixture_limits(kinewright_project::storage_budget::StorageLimits {
        receipts: 128,
        ..Default::default()
    })
}
fn c5_fix1_fixture_limits(limits: kinewright_project::storage_budget::StorageLimits) -> Fixture {
    use kinewright_project::bundle::LoadedBundle;
    let f = Fixture::new();
    f.host.runtime.stop();
    let host = f.host.owner.upgrade().unwrap();
    let mut host = host.lock().unwrap();
    let old = host.owner.take().unwrap();
    let history = old.core().export_history().unwrap();
    let inputs = old.current_inputs_ref().clone();
    drop(old);
    let bundle = LoadedBundle::read(&f.temp.path("project.kinewright"))
        .unwrap()
        .into_writer_with_limits_probe(limits)
        .unwrap();
    let core = Core::spawn((*history.baseline).clone()).unwrap();
    core.restore_history(history).unwrap();
    host.owner = Some(Ja1EditingSession::from_bundle(bundle, &core, inputs).unwrap());
    host.owner_mut()
        .unwrap()
        .admit_host_grant(b"hook plan, script and assignment")
        .unwrap();
    drop(host);
    f
}
fn c5_fix1_occupy(f: &Fixture, count: usize) {
    let publisher = f.actor(AttachmentRole::Producer, "preexisting-snapshots");
    for _ in 0..count {
        let (checkpoint_ref, checkpoint) = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .manual_checkpoint()
            .unwrap();
        let command = metadata_request(
            f,
            &publisher,
            MetadataChange::PublishSnapshot(PublishSnapshot {
                version: VersionId::fresh().unwrap(),
                expected_head: checkpoint.head_id.clone(),
                checkpoint_ref,
                parent_version: None,
                run_provenance: None,
                revision_refs: vec![],
            }),
        );
        ok(
            publisher.request(command).unwrap(),
            "C5_FIX1_PREEXISTING_CAPACITY",
        );
    }
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"hook plan, script and assignment")
        .unwrap();
}
#[test]
fn s4b_c5_fix1_start_refuses_nearly_full_store() {
    let f = c5_fix1_limited_fixture();
    c5_fix1_occupy(&f, 97);
    let request = f.start_request(&f.editor);
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert_eq!(
        reply.error.as_ref().map(|e| e.code.as_str()),
        Some("run_limit"),
        "C5_FIX1_NEARLY_FULL_START {reply:?}"
    );
    assert!(
        reply
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("start a run")
    );
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_runs()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn s4b_c5_fix1_start_refuses_small_terminal_vector() {
    use kinewright_project::storage_budget::StorageLimits;
    for field in [
        "control", "decoded", "graph", "pending", "issuers", "versions",
    ] {
        let mut limits = StorageLimits::default();
        match field {
            "control" => limits.control_bytes = 8 << 20,
            "decoded" => limits.decoded_record_bytes = 32 << 20,
            "graph" => limits.graph_depth = 15,
            "pending" => limits.pending_commands = 1,
            "issuers" => limits.issuers = 8,
            "versions" => limits.versions = 1,
            _ => unreachable!(),
        }
        let f = c5_fix1_fixture_limits(limits);
        let request = f.start_request(&f.editor);
        let reply = f
            .editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request),
            })
            .unwrap();
        assert_eq!(
            reply.error.as_ref().map(|e| e.code.as_str()),
            Some("run_limit"),
            "C5_FIX1_SMALL_TERMINAL_VECTOR {field} {reply:?}"
        );
    }
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one occupied-store witness holds exact state through refusal, cleanup and reopen"
)]
fn s4b_c5_fix1_occupied_churn_reserves_stop_cleanup_reopen() {
    use super::super::native_invocation::SpendingChange;
    let f = c5_fix1_limited_fixture();
    c5_fix1_occupy(&f, 80);
    let manual = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .core()
        .export_history()
        .unwrap();
    let (run, execution, _) = reserve(&f);
    let run_auth = f.producer(&run);
    let now = native_now(&f, &run);
    let sequence = f.projection(&run).sequence;
    let mut deliveries = vec![];
    for n in 0..80 {
        spending_change(
            &f,
            &execution,
            SpendingChange::Wait(Some(format!("rate {n}"))),
            now + n * 6000,
            0,
            true,
        )
        .unwrap();
        spending_change(
            &f,
            &execution,
            SpendingChange::Wait(None),
            now + n * 6000 + 5000,
            0,
            true,
        )
        .unwrap();
        if n < 16 {
            let delivery: kinewright_project::bundle_records::SessionId = identity();
            deliveries.push(delivery.clone());
            spending_change(
                &f,
                &execution,
                SpendingChange::Repair {
                    delivery,
                    failure: kinewright_media::sha256_bytes(&n.to_le_bytes()),
                },
                now + n * 6000 + 5000,
                0,
                true,
            )
            .unwrap();
        }
        assert_eq!(
            f.projection(&run).sequence,
            sequence,
            "C5_FIX1_WAIT_REPAIR_REPLACE"
        );
    }
    let accounting = spending_record(&f, &run);
    assert_eq!(accounting.tasks[0].repairs.len(), 16);
    for delivery in &deliveries {
        assert!(
            accounting.tasks[0]
                .repairs
                .iter()
                .any(|r| &r.delivery == delivery && r.attempt == execution.attempt.0)
        );
    }
    let host = f.host.owner.upgrade().unwrap();
    let mut refused = false;
    for _ in 0..128 {
        let mut owner = host.lock().unwrap();
        let state = owner.spending_scope(&run_auth, &run).unwrap();
        match owner.publish_native(&run_auth, state, &execution, vec![]) {
            Ok(()) => {}
            Err(error) => {
                assert!(
                    error.to_string().contains("Run limit"),
                    "C5_FIX1_RUN_LIMIT_REASON {error}"
                );
                refused = true;
                break;
            }
        }
    }
    assert!(refused, "C5_FIX1_RESERVE_ORDINARY_REFUSAL");
    let page = ok(
        f.editor
            .request(HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            })
            .unwrap(),
        "C5_FIX1_RUN_VIEW",
    )
    .run_view
    .unwrap();
    assert!(
        page.runtime_status
            .as_deref()
            .is_some_and(|reason| reason.contains("Run limit")),
        "C5_FIX1_RUN_LIMIT_IN_RUN_VIEW"
    );
    let before = f.projection(&run);
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_FIX1_RESERVED_STOP",
    );
    let claim_root = f.temp.path("reserve-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        execution.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    crate::effect_pump::reconcile_attempt(&host, &mut claim, &run_auth, &execution).unwrap();
    claim.finish().unwrap();
    let state = f.projection(&run);
    assert!(state.sequence > before.sequence);
    assert_eq!(state.cleanup, RunCleanup::Settled);
    assert_eq!(
        spending_record(&f, &run),
        accounting,
        "C5_FIX1_RESERVE_ACCOUNTING_EXACT"
    );
    drop(host);
    let f = c4_restart(f, &claim_root);
    assert_eq!(
        f.projection(&run),
        state,
        "C5_FIX1_RESERVE_REOPEN_PROJECTION"
    );
    assert_eq!(
        spending_record(&f, &run),
        accounting,
        "C5_FIX1_RESERVE_REOPEN_ACCOUNTING"
    );
    assert_eq!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .core()
            .export_history()
            .unwrap(),
        manual,
        "C5_FIX1_RESERVE_REOPEN_MANUAL"
    );
}

#[test]
fn s4b_c5_fix1_objects_and_decoded_pressure_leave_terminal_capacity() {
    use kinewright_project::storage_budget::StorageLimits;
    for metadata in [false, true] {
        let limits = if metadata {
            StorageLimits {
                metadata_bytes: 384 << 20,
                ..Default::default()
            }
        } else {
            StorageLimits {
                objects: 384,
                ..Default::default()
            }
        };
        let f = c5_fix1_fixture_limits(limits);
        let (run, execution, _) = reserve(&f);
        let producer = f.producer(&run);
        let host = f.host.owner.upgrade().unwrap();
        // A retained reader already owns this measured decoded charge.
        let held = metadata.then(|| {
            host.lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .reserve_probe(64 << 20, 0)
                .unwrap()
        });
        let mut refused = false;
        for _ in 0..128 {
            let mut owner = host.lock().unwrap();
            let state = owner.spending_scope(&producer, &run).unwrap();
            match owner.publish_native(&producer, state, &execution, vec![]) {
                Ok(()) => {}
                Err(error) => {
                    assert!(
                        error.to_string().contains("Run limit"),
                        "C5_FIX1_RESOURCE_REFUSAL {error}"
                    );
                    refused = true;
                    break;
                }
            }
        }
        assert!(
            refused,
            "C5_FIX1_SHARED_RESOURCE_RESERVE metadata={metadata}"
        );
        let stop = f.control(&f.editor, &run, RunAction::Stop);
        ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                })
                .unwrap(),
            "C5_FIX1_RESOURCE_STOP",
        );
        let claim_root = f.temp.path("resource-machine");
        let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &claim_root,
            execution.project.as_str(),
            run.0.as_str(),
        )
        .unwrap();
        crate::effect_pump::reconcile_attempt(&host, &mut claim, &producer, &execution).unwrap();
        claim.finish().unwrap();
        assert_eq!(f.projection(&run).cleanup, RunCleanup::Settled);
        let state = f.projection(&run);
        drop(held);
        drop(host);
        let f = c4_restart(f, &claim_root);
        assert_eq!(f.projection(&run), state, "C5_FIX1_RESOURCE_REOPEN");
    }
}
