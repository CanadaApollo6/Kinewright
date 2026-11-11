use super::*;
use crate::{
    editing_session::Ja1EditingSession,
    orchestration_accounting::AccountingRecord,
    orchestration_commit::{CommandDecision, ValidatedRunUpdate, commit_orchestration},
    orchestration_records::*,
    receipts::{CommandId, IssuerId},
    storage_budget::StorageLimits,
};
use kinewright_core::{Core, Document};
use kinewright_media::test_support::TempDirectory;
fn id<T: serde::de::DeserializeOwned>(n: u64) -> T {
    serde_json::from_value(serde_json::json!(format!("{n:032x}"))).unwrap()
}
// Test-only oracle for the previous construction: both persisted objects carry
// the complete state, and reopen chooses the current cumulative replacement.
fn old_cut_projection(update: &ValidatedRunUpdate) -> RunProjection {
    let snapshot: RunProjection =
        serde_json::from_slice(&serde_json::to_vec(update.projection()).unwrap()).unwrap();
    let segment: EventSegment =
        serde_json::from_slice(&serde_json::to_vec(update.segment()).unwrap()).unwrap();
    assert_eq!(segment.events.last().unwrap().projection, snapshot);
    snapshot
}
struct Fixture {
    temp: TempDirectory,
    owner: Ja1EditingSession,
    _core: Core,
    issuer: IssuerId,
    grant: ObjectRef,
    state: RunProjection,
}
impl Fixture {
    fn new() -> Self {
        let temp = TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "linear-run",
        );
        let core = Core::spawn(Document::default()).unwrap();
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(&temp.path("linear.kinewright"), StorageLimits::default())
            .unwrap();
        let mut candidate = owner.fork_durable_branch("linear-candidate").unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        let base = owner.candidate_binding(candidate.branch_id()).unwrap();
        let (issuer, grant) = owner
            .issue_metadata_authority("linear", None, b"linear authority")
            .unwrap();
        let evidence = owner.admit_host_grant(b"linear brief").unwrap();
        let brief = TaskBrief {
            plan: evidence.clone(),
            script: evidence.clone(),
            assignment: evidence,
            inputs: owner.current_inputs_ref().clone(),
            role: Role::Director,
            provider: ProviderSelectionRecord {
                connector: "scripted".into(),
                model: "fixture".into(),
                options: vec![],
            },
            scope: TaskScope {
                operations: std::collections::BTreeSet::default(),
                objects: std::collections::BTreeSet::default(),
                fields: std::collections::BTreeSet::default(),
                tracks: std::collections::BTreeSet::default(),
                bus_sidechain_closure: std::collections::BTreeSet::default(),
                asset_roots: vec![],
                base,
                consent_refs: vec![],
            },
        };
        let state = RunProjection {
            run: id(1),
            sequence: 1,
            control_revision: id(2),
            phase: RunPhase::Queued,
            outcome: RunOutcome::Pending,
            wait: RunWait::None,
            cleanup: RunCleanup::Settled,
            edit_consent: None,
            edit_approvals: vec![],
            edit_applications: vec![],
            tasks: vec![TaskRecord {
                id: id(3),
                parent: None,
                brief,
                state: TaskState::Queued,
                progress: TaskProgress::default(),
            }],
            effects: vec![],
            attempts: vec![],
            deliveries: vec![],
            requests: vec![],
            results: vec![],
            usage: vec![],
        };
        let mut f = Self {
            temp,
            owner,
            _core: core,
            issuer,
            grant,
            state,
        };
        let mut accounting =
            AccountingRecord::new(f.owner.project_id().unwrap(), f.state.run.0.clone(), 1).unwrap();
        accounting
            .add_task(f.state.tasks[0].id.0.clone(), None)
            .unwrap();
        let seal = f
            .owner
            .active_bundle_mut()
            .unwrap()
            .admit_owned_scheduler_record(OrchestrationRecord {
                version: 1,
                retention: Retention::Complete,
                payload: OrchestrationPayload::Accounting(Box::new(accounting)),
            })
            .unwrap();
        f.state.usage.push(seal.reference.clone());
        let update = f.update();
        commit_orchestration(&mut f.owner, update, None).unwrap();
        f
    }
    fn update(&self) -> ValidatedRunUpdate {
        let mut envelope = OrchestrationEnvelope {
            id: CommandId {
                issuer: self.issuer.clone(),
                sequence: self.state.sequence,
            },
            digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
            project: self.owner.project_id().unwrap(),
            expected_generation: self.owner.metadata_generation().unwrap(),
            authority: AuthorityBinding {
                grant_ref: self.grant.clone(),
                owner_epoch: self.owner.store().unwrap().owner_epoch(),
                endpoint: None,
                attempt: None,
            },
            command: OrchestrationCommand::RecordContext {
                task: self.state.tasks[0].id.clone(),
                feedback: vec![],
                decisions: vec![],
            },
        };
        envelope.digest = envelope.computed_digest().unwrap();
        let previous = self
            .owner
            .orchestration_cut(&self.state.run)
            .unwrap()
            .map(|c| c.segment);
        let segment = EventSegment {
            run: self.state.run.clone(),
            first_sequence: self.state.sequence,
            previous,
            events: vec![OrchestrationEvent {
                command: envelope.id.clone(),
                sequence: self.state.sequence,
                projection: self.state.clone(),
            }],
        };
        ValidatedRunUpdate::new(
            &envelope.project,
            envelope.expected_generation,
            &envelope,
            self.state.clone(),
            segment,
            CommandDecision::Applied,
        )
        .unwrap()
    }
    fn add_audit(&mut self) -> ValidatedRunUpdate {
        let reference = self
            .owner
            .admit_host_grant(format!("audit {}", self.state.sequence).as_bytes())
            .unwrap();
        self.state.sequence += 1;
        self.state.edit_approvals.push(reference.clone());
        self.state.edit_applications.push(reference);
        self.update()
    }
    fn attach_one_send(&mut self) -> crate::store::SealedObject {
        use crate::orchestration_accounting::{
            ProviderMetadata, ProviderSelection, SendConsumption,
        };
        let record = self
            .owner
            .store()
            .unwrap()
            .read_run_record(&self.state.usage[0])
            .unwrap();
        let OrchestrationPayload::Accounting(before) = &record.payload else {
            unreachable!()
        };
        let mut next = before.successor().unwrap();
        let selection = ProviderSelection {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        };
        let task = next.task_mut(&self.state.tasks[0].id.0).unwrap();
        task.select_provider(ProviderMetadata {
            attempt: id(4),
            requested: selection.clone(),
            resolved: selection,
            cli_version: "fixture".into(),
        })
        .unwrap();
        let payload = self.owner.admit_host_grant(b"one durable send").unwrap();
        task.consume_send(SendConsumption {
            command: CommandId {
                issuer: self.issuer.clone(),
                sequence: self.state.sequence,
            },
            attempt: id(4),
            turn: 1,
            payload,
        })
        .unwrap();
        next.validate_successor(before).unwrap();
        let seal = self
            .owner
            .admit_scheduler_record(OrchestrationRecord {
                version: 1,
                retention: Retention::Complete,
                payload: OrchestrationPayload::Accounting(Box::new(next)),
            })
            .unwrap();
        self.state.usage[0] = seal.reference.clone();
        seal
    }
    fn reopen(&mut self) {
        let core = Core::spawn(Document::default()).unwrap();
        let replacement =
            Ja1EditingSession::new(self.temp.root(), &core, StorageLimits::default()).unwrap();
        drop(std::mem::replace(&mut self.owner, replacement));
        self.owner = Ja1EditingSession::open(&self.temp.path("linear.kinewright")).unwrap();
    }
}
#[test]
fn linear_run_admission_recomputes_missing_and_extra_introductions() {
    let mut f = Fixture::new();
    let update = f.add_audit();
    let bundle = f.owner.active_bundle_mut().unwrap();
    let base = bundle.semantic_snapshot(&update.projection().run).unwrap();
    let record = OrchestrationRecord::retained(
        OrchestrationPayload::Snapshot(Box::new(update.projection().clone())),
        base,
        &bundle.store,
    )
    .unwrap();
    let mut unanchored = record.clone();
    unanchored.retention = Retention::Complete;
    assert!(matches!(
        bundle.store.admit_record(&unanchored),
        Err(BundleError::Corrupt(_))
    ));
    let mut missing = record.clone();
    let Retention::Introduced { references, .. } = &mut missing.retention else {
        unreachable!()
    };
    assert!(!references.is_empty());
    references.clear();
    assert!(matches!(
        bundle.store.admit_record(&missing),
        Err(BundleError::Corrupt(_))
    ));
    let mut extra = record.clone();
    let Retention::Introduced { references, .. } = &mut extra.retention else {
        unreachable!()
    };
    references.push(update.projection().tasks[0].brief.plan.clone());
    references.sort();
    references.dedup();
    assert!(matches!(
        bundle.store.admit_record(&extra),
        Err(BundleError::Corrupt(_))
    ));
    bundle.store.admit_record(&record).unwrap();
}
#[test]
fn linear_run_full_projection_oracle_survives_cumulative_collection_and_reopen() {
    let mut f = Fixture::new();
    // The old construction's oracle is the complete immutable projection carried
    // by every event; dependency ownership never changes the projected state.
    let mut oracle = vec![f.state.clone()];
    for _ in 0..4 {
        let update = f.add_audit();
        oracle.push(old_cut_projection(&update));
        commit_orchestration(&mut f.owner, update, None).unwrap();
        let mut accounting = {
            let r = f
                .owner
                .store()
                .unwrap()
                .read_run_record(&f.state.usage[0])
                .unwrap();
            let OrchestrationPayload::Accounting(a) = r.payload else {
                unreachable!()
            };
            *a
        };
        for _ in 0..3 {
            let mut next = accounting.successor().unwrap();
            next.observe_clock(accounting.last_unix_ms + 1, 1, true)
                .unwrap();
            f.owner.publish_run_accounting(&f.state.run, &next).unwrap();
            let mut expected = f.state.clone();
            let current = f
                .owner
                .orchestration_projection(&f.state.run)
                .unwrap()
                .unwrap();
            let actual = f
                .owner
                .store()
                .unwrap()
                .read_run_record(&current.usage[0])
                .unwrap();
            let OrchestrationPayload::Accounting(actual) = &actual.payload else {
                unreachable!()
            };
            assert_eq!(
                **actual, next,
                "the exact clock/usage successor is the effective value"
            );
            expected.usage[0].clone_from(&current.usage[0]);
            assert_eq!(current, expected);
            accounting = next;
            f.state = current;
            f.owner
                .active_bundle_mut()
                .unwrap()
                .collect_after_ack()
                .unwrap();
            f.reopen();
            assert_eq!(
                f.owner
                    .orchestration_projection(&f.state.run)
                    .unwrap()
                    .unwrap(),
                f.state
            );
        }
    }
    let bundle = f.owner.active_bundle().unwrap();
    let index = bundle.run_index().unwrap();
    for (cut, expected) in index.orchestration[&f.state.run].iter().zip(oracle) {
        assert_eq!(
            bundle
                .orchestration_receipt_projection(&cut.receipt)
                .unwrap(),
            expected
        );
        let snapshot = bundle
            .store
            .read_record::<OrchestrationRecord>(&cut.snapshot)
            .unwrap();
        for r in snapshot.full_dependencies().unwrap() {
            bundle.store.verify(&r).unwrap();
        }
    }
    let update = f.add_audit();
    let mut rewritten = update.projection().clone();
    rewritten.edit_approvals.remove(0);
    assert_eq!(
        crate::orchestration_commit::validate_projection_successor(&f.state, &rewritten),
        Err(Refusal::ScopeConflict)
    );
    let binding = update.segment().binding().unwrap();
    assert_ne!(binding.previous, None);
    let bytes = serde_json::to_vec(&update).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(wire["segment"].get("events").is_none());
    assert!(wire.get("dependencies").is_none());
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one cut is retried at every publication boundary"
)]
fn linear_run_each_publication_write_and_root_boundary_is_replayable() {
    let mut clean = Fixture::new();
    clean.add_audit();
    let _send_seal = clean.attach_one_send();
    let update = clean.update();
    let (_, trace) = crate::io_observer::observe(|| {
        commit_orchestration(&mut clean.owner, update, None).unwrap()
    });
    assert!(!trace.overflowed);
    let writes = trace
        .events
        .iter()
        .filter(|e| e.operation == "object_file_sync")
        .count();
    assert!(
        writes >= 4,
        "snapshot, segment, index, receipts and manifest must be written: {trace:?}"
    );
    let mut boundaries = (1..=writes)
        .map(|n| ("object_file_sync", n))
        .collect::<Vec<_>>();
    boundaries.extend([
        ("root_before_publish", 1),
        ("floor_tag_prepare", 1),
        ("floor_tag_complete", 1),
        ("root_file_sync", 1),
        ("root_rename_io", 1),
        ("root_parent_sync", 1),
    ]);
    for (point, occurrence) in boundaries {
        let mut f = Fixture::new();
        let before = f.state.clone();
        f.add_audit();
        let send_seal = f.attach_one_send();
        let update = f.update();
        let expected = f.state.clone();
        let receipt = update.receipt().clone();
        let result = crate::io_observer::fail_nth(point, occurrence, || {
            commit_orchestration(&mut f.owner, update.clone(), None)
        });
        assert!(result.is_err(), "injection {point}/{occurrence} must fire");
        drop(send_seal);
        f.reopen();
        let recovered = f
            .owner
            .orchestration_projection(&f.state.run)
            .unwrap()
            .unwrap();
        assert!(
            recovered == before || recovered == expected,
            "one root at {point}/{occurrence}"
        );
        if recovered == before {
            // The reopened owner reissues an uncommitted intent with its current
            // epoch/generation. No committed publication identity is replayed.
            f.state = before.clone();
            f.add_audit();
            let _retry_seal = f.attach_one_send();
            let retry = f.update();
            commit_orchestration(&mut f.owner, retry, None).unwrap();
        } else {
            assert_eq!(
                commit_orchestration(&mut f.owner, update, None).unwrap(),
                receipt
            );
        }
        assert_eq!(
            f.owner
                .orchestration_projection(&f.state.run)
                .unwrap()
                .unwrap(),
            expected
        );
        f.owner
            .active_bundle_mut()
            .unwrap()
            .collect_after_ack()
            .unwrap();
        f.reopen();
        let final_state = f
            .owner
            .orchestration_projection(&f.state.run)
            .unwrap()
            .unwrap();
        assert_eq!(final_state.edit_approvals.len(), 1);
        assert_eq!(final_state.edit_applications.len(), 1);
        assert_eq!(final_state, expected);
        let accounting = f
            .owner
            .store()
            .unwrap()
            .read_run_record(&final_state.usage[0])
            .unwrap();
        let OrchestrationPayload::Accounting(accounting) = &accounting.payload else {
            unreachable!()
        };
        assert_eq!(
            accounting.tasks[0].sends.len(),
            1,
            "durable send charged once"
        );
        assert_eq!(
            f.owner
                .active_bundle()
                .unwrap()
                .run_index()
                .unwrap()
                .orchestration[&f.state.run]
                .len(),
            2
        );
        eprintln!("C5_LINEAR_CRASH {point}/{occurrence} old_or_new=true exact_retry=true");
    }
}

#[test]
fn linear_run_warm_ownership_reuse_preserves_cold_reopen_state() {
    let mut f = Fixture::new();
    for _ in 0..3 {
        let update = f.add_audit();
        commit_orchestration(&mut f.owner, update, None).unwrap();
    }
    let current = f
        .owner
        .store()
        .unwrap()
        .read_run_record(&f.state.usage[0])
        .unwrap();
    let OrchestrationPayload::Accounting(accounting) = &current.payload else {
        unreachable!()
    };
    let mut next = accounting.successor().unwrap();
    next.observe_clock(accounting.last_unix_ms + 1, 1, true)
        .unwrap();
    drop(current);
    let (published, trace) =
        crate::io_observer::observe(|| f.owner.publish_run_accounting(&f.state.run, &next));
    published.unwrap();
    assert!(
        trace
            .events
            .iter()
            .any(|event| event.operation == "run_ownership_memo_hit"),
        "a warm publication must reuse earlier immutable run records"
    );
    let expected = f
        .owner
        .orchestration_projection(&f.state.run)
        .unwrap()
        .unwrap();
    f.owner
        .verify_retained_run_after_collection(&f.state.run)
        .unwrap();
    f.reopen();
    assert_eq!(
        f.owner
            .orchestration_projection(&f.state.run)
            .unwrap()
            .unwrap(),
        expected
    );
}

#[test]
fn linear_run_receipt_binds_predecessor_and_refuses_rewritten_past() {
    let mut f = Fixture::new();
    let update = f.add_audit();
    let receipt = commit_orchestration(&mut f.owner, update.clone(), None).unwrap();
    assert_eq!(
        commit_orchestration(&mut f.owner, update, None).unwrap(),
        receipt
    );
    let next = f.add_audit();
    let mut forged = next.segment().clone();
    forged.previous.as_mut().unwrap().sha256 = "0".repeat(64);
    let wrong = ValidatedRunUpdate::new(
        &next.envelope().project,
        next.envelope().expected_generation,
        next.envelope(),
        next.projection().clone(),
        forged,
        next.receipt().decision.clone(),
    )
    .unwrap();
    assert_ne!(wrong.receipt().cut_digest, next.receipt().cut_digest);
    let metadata = f
        .owner
        .metadata_envelope(next.envelope().id.clone())
        .unwrap();
    assert!(
        f.owner
            .active_bundle()
            .unwrap()
            .validate_run_publication_binding(&metadata, &wrong)
            .is_err()
    );
    let before = f.owner.root_sha256().unwrap();
    f.state.edit_approvals.remove(0);
    let rewritten = f.update();
    let error = commit_orchestration(&mut f.owner, rewritten, None).unwrap_err();
    assert!(
        matches!(error, BundleError::Identity(ref text) if text.contains("ScopeConflict")),
        "{error}"
    );
    assert_eq!(f.owner.root_sha256().unwrap(), before);
    f.reopen();
    assert_eq!(
        f.owner
            .orchestration_projection(&f.state.run)
            .unwrap()
            .unwrap()
            .edit_approvals
            .len(),
        1
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one cumulative publication is retried at every boundary"
)]
fn linear_run_cumulative_writes_and_root_retry_preserve_exact_projection() {
    let next_accounting = |f: &Fixture| {
        let record = f
            .owner
            .store()
            .unwrap()
            .read_run_record(&f.state.usage[0])
            .unwrap();
        let OrchestrationPayload::Accounting(before) = &record.payload else {
            unreachable!()
        };
        let mut next = before.successor().unwrap();
        next.observe_clock(2, 1, true).unwrap();
        next
    };
    let mut clean = Fixture::new();
    let next = next_accounting(&clean);
    let ((), trace) = crate::io_observer::observe(|| {
        clean
            .owner
            .publish_run_accounting(&clean.state.run, &next)
            .unwrap();
    });
    assert!(!trace.overflowed);
    let writes = trace
        .events
        .iter()
        .filter(|e| e.operation == "object_file_sync")
        .count();
    assert!(writes >= 4, "accounting, snapshot, index and manifest");
    let mut boundaries = (1..=writes)
        .map(|n| ("object_file_sync", n))
        .collect::<Vec<_>>();
    boundaries.extend([
        ("root_before_publish", 1),
        ("floor_tag_prepare", 1),
        ("floor_tag_complete", 1),
        ("root_file_sync", 1),
        ("root_rename_io", 1),
        ("root_parent_sync", 1),
    ]);
    for (point, occurrence) in boundaries {
        let mut f = Fixture::new();
        let next = next_accounting(&f);
        let result = crate::io_observer::fail_nth(point, occurrence, || {
            f.owner.publish_run_accounting(&f.state.run, &next)
        });
        assert!(result.is_err(), "injection {point}/{occurrence} must fire");
        f.reopen();
        let recovered = f
            .owner
            .orchestration_projection(&f.state.run)
            .unwrap()
            .unwrap();
        let record = f
            .owner
            .store()
            .unwrap()
            .read_run_record(&recovered.usage[0])
            .unwrap();
        let OrchestrationPayload::Accounting(accounting) = &record.payload else {
            unreachable!()
        };
        assert!(
            recovered == f.state || **accounting == next,
            "old or new accounting at {point}/{occurrence}"
        );
        let root = f.owner.root_sha256().unwrap();
        f.owner.publish_run_accounting(&f.state.run, &next).unwrap();
        if **accounting == next {
            assert_eq!(
                f.owner.root_sha256().unwrap(),
                root,
                "an acknowledged retry writes no second root"
            );
        }
        let after = f
            .owner
            .orchestration_projection(&f.state.run)
            .unwrap()
            .unwrap();
        crate::orchestration_cumulative::validate_snapshot(&f.state, &after, |r| {
            Ok(*f.owner.store()?.read_run_record(r)?)
        })
        .unwrap();
        f.owner
            .active_bundle_mut()
            .unwrap()
            .collect_after_ack()
            .unwrap();
        f.reopen();
        assert_eq!(
            f.owner
                .orchestration_projection(&f.state.run)
                .unwrap()
                .unwrap(),
            after
        );
        let record = f
            .owner
            .store()
            .unwrap()
            .read_run_record(&after.usage[0])
            .unwrap();
        let OrchestrationPayload::Accounting(accounting) = &record.payload else {
            unreachable!()
        };
        assert_eq!(**accounting, next);
        eprintln!(
            "C5_LINEAR_CUMULATIVE_CRASH {point}/{occurrence} old_or_new=true exact_retry=true"
        );
    }
}
