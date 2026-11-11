#[cfg(test)]
mod tests {
    use super::super::*;

    fn control(b: &B21) -> crate::writer_host::task_mcp::TaskMcp {
        let owner = b.f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let runtime = &host.hooks.runs[&b.run];
        let endpoint = runtime.tasks[&b.child].endpoint();
        let auth = host
            .attachments
            .iter()
            .find(|attachment| {
                attachment.producer_scope.as_ref().is_some_and(|scope| {
                    scope.endpoint.as_ref() == Some(&endpoint.authority().endpoint)
                })
            })
            .unwrap()
            .auth
            .clone();
        crate::writer_host::task_mcp::TaskMcp::new(
            Arc::downgrade(&owner),
            auth,
            endpoint,
            #[cfg(all(feature = "te1-eval", target_os = "linux"))]
            host.evaluation_observer.clone(),
            Arc::clone(&runtime.measurements[&b.child]),
            None,
        )
    }
    fn cut(
        b: &B21,
    ) -> Result<Result<kinewright_core::Event, kinewright_core::CoreDisconnected>, HostError> {
        let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
            revision,
            document,
        }) = b
            .core
            .request(kinewright_core::Command::Query(
                kinewright_core::Query::Snapshot,
            ))
            .unwrap()
        else {
            panic!("snapshot")
        };
        control(b).apply_with_edit_consent(
            &crate::ConfirmationBroker::default(),
            &b.core,
            revision,
            &document,
            &[
                kinewright_core::Operation::DeleteClip {
                    clip: kinewright_core::ClipId(1),
                },
                kinewright_core::Operation::RippleDeleteClip {
                    clip: kinewright_core::ClipId(2),
                },
            ],
            Some("Remove the first two clips"),
        )
    }
    #[test]
    fn c4_run_limit_both_audit_cuts_fit_and_release_their_holds() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::c4_run_limit_both_audit_cuts_fit_and_release_their_holds",
        ) {
            return;
        }
        let b = b21_start();
        assert!(matches!(
            cut(&b).unwrap().unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
        let state = b.f.projection(&b.run);
        assert_eq!(state.edit_approvals.len(), 1);
        assert_eq!(state.edit_applications.len(), 1);
        assert_eq!(
            b.core.export_history().unwrap().document.tracks[0]
                .clips
                .len(),
            1
        );
        let owner = b.f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let store = host.owner().unwrap().store().unwrap();
        let usage = store.usage().unwrap();
        assert!(
            host.hooks.pending_edit_applications.is_empty(),
            "C4_SUCCESS_RELEASES_PLAN"
        );
        assert_eq!(
            usage.reserved_bytes, 0,
            "C4_SUCCESS_RELEASES_CONTROL_RESERVATIONS"
        );
        assert!(!store.run_limit_reached(), "C4_HEALTHY_RUN_REMAINS_OPEN");
        assert!(!usage.damaged_accounting && !usage.unsettled);
    }
    fn occupy_reader_memory(
        b: &B21,
        remaining: u64,
    ) -> Vec<kinewright_project::storage_budget::StorageReservation> {
        let owner = b.f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let store = host.owner().unwrap().store().unwrap();
        let usage = store.usage().unwrap();
        let ordinary = store.limits().metadata_bytes - kinewright_project::run_admission::METADATA;
        let mut occupied = ordinary
            .checked_sub(usage.metadata_bytes + usage.decoded_bytes + remaining)
            .unwrap();
        let mut readers = Vec::new();
        while occupied > 0 {
            let chunk = occupied.min(64 << 20);
            readers.push(store.reserve_probe(chunk, 0).unwrap());
            occupied -= chunk;
        }
        readers
    }
    #[test]
    fn c4_run_limit_one_cut_fits_two_refuse_without_publication_or_latch() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::c4_run_limit_one_cut_fits_two_refuse_without_publication_or_latch",
        ) {
            return;
        }
        // Occupied reader memory leaves enough ordinary staging room for one
        // real publisher, but not both held publishers. No store bound changes.
        let b = b21_start();
        let owner = b.f.host.owner.upgrade().unwrap();
        let (mut lower, mut upper) = (64 << 20, 128 << 20);
        let mut selected = None;
        for _ in 0..28 {
            if lower > upper {
                break;
            }
            let remaining = lower + (upper - lower) / 2;
            let _readers = occupy_reader_memory(&b, remaining);
            // Calibration aborts only AFTER both real preparations succeed,
            // before either root can publish. Its distinct error cannot satisfy
            // the final capacity witness, which has no hook installed.
            owner.lock().unwrap().hooks.consent_prepared_hook = Some(Box::new(|| {
                Err(
                    HostRefusal::new("fixture_fully_prepared", "Both real cuts were prepared")
                        .into(),
                )
            }));
            let (result, trace) = kinewright_project::io_observer::observe(|| cut(&b));
            let first_fit = trace
                .events
                .iter()
                .any(|event| event.operation == "consent_approval_prepared");
            let error = result.unwrap_err();
            let HostError::Refused(refusal) = &error else {
                panic!("C4_UNEXPECTED_PREPARE_ERROR {error}")
            };
            if refusal.code == "fixture_fully_prepared" {
                assert!(first_fit);
                upper = remaining - 1;
            } else {
                assert_eq!(
                    refusal.code, "consent_capacity",
                    "C4_CALIBRATION_REFUSAL {error}"
                );
                if first_fit {
                    selected = Some(remaining);
                }
                lower = remaining + 1;
            }
        }
        owner.lock().unwrap().hooks.consent_prepared_hook = None;
        let remaining = selected.expect("C4_REAL_ONE_CUT_TWO_CUT_BOUNDARY");
        let readers = occupy_reader_memory(&b, remaining);
        let history = b.core.export_history().unwrap();
        let projection = b.f.projection(&b.run);
        let root = std::fs::read(b.f.temp.path("project.kinewright")).unwrap();
        let (result, trace) = kinewright_project::io_observer::observe(|| cut(&b));
        assert!(
            trace
                .events
                .iter()
                .any(|event| event.operation == "consent_approval_prepared"),
            "C4_REAL_FIRST_CUT_FITS"
        );
        let error = result.unwrap_err();
        assert!(
            matches!(&error, HostError::Refused(refusal) if refusal.code == "consent_capacity" && refusal.message.contains("not applied") && refusal.message.contains("Director")),
            "C4_SPECIFIC_CAPACITY_REFUSAL {error}"
        );
        assert_eq!(
            std::fs::read(b.f.temp.path("project.kinewright")).unwrap(),
            root,
            "C4_H1_UNCHANGED"
        );
        assert_eq!(
            b.f.projection(&b.run),
            projection,
            "C4_NO_APPROVAL_OR_PROJECTION_CHANGE"
        );
        assert_eq!(
            b.core.export_history().unwrap(),
            history,
            "C4_NO_LIVE_HISTORY_CHANGE"
        );
        let metadata_limit = {
            let host = owner.lock().unwrap();
            let store = host.owner().unwrap().store().unwrap();
            assert!(!store.run_limit_reached(), "C4_REFUSAL_DOES_NOT_LATCH");
            store.limits().metadata_bytes
        };
        eprintln!(
            "C4_STAGING_BOUNDARY remaining={remaining} metadata_limit={metadata_limit} ordinary_metadata_limit={}",
            metadata_limit - kinewright_project::run_admission::METADATA
        );
        let finished = b
            .mcp
            .raw("finish_task", te13_control(&b.mcp, &b.child, &b.core));
        assert_eq!(
            finished.is_error,
            Some(false),
            "C4_FINISH_AFTER_REFUSAL {finished:?}"
        );
        let stop = b.f.control(&b.f.editor, &b.run, RunAction::Stop);
        ok(
            b.f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                })
                .unwrap(),
            "C4_STOP_AFTER_REFUSAL",
        );
        drop(readers);
    }
    #[test]
    fn c4_run_limit_pending_application_retries_its_held_cut_once() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::c4_run_limit_pending_application_retries_its_held_cut_once",
        ) {
            return;
        }
        let b = b21_start();
        let result =
            kinewright_project::io_observer::fail_nth("root_before_publish", 2, || cut(&b));
        assert!(matches!(
            result.unwrap().unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
        let state = b.f.projection(&b.run);
        assert!(state.edit_applications.is_empty());
        let identity = state
            .attempts
            .iter()
            .find(|attempt| attempt.identity.task == b.child)
            .unwrap()
            .identity
            .clone();
        let producer = b.f.producer(&b.run);
        let owner = b.f.host.owner.upgrade().unwrap();
        owner
            .lock()
            .unwrap()
            .publish_native(&producer, state, &identity, vec![])
            .unwrap();
        let current = b.f.projection(&b.run);
        assert_eq!(current.edit_applications.len(), 1, "C4_RETRY_EXACTLY_ONCE");
        let host = owner.lock().unwrap();
        assert!(
            host.hooks.pending_edit_applications.is_empty(),
            "C4_RETRY_RELEASES_HOLD"
        );
        let usage = host.owner().unwrap().store().unwrap().usage().unwrap();
        assert_eq!(usage.reserved_bytes, 0);
        assert!(!usage.damaged_accounting && !usage.unsettled);
    }
    #[test]
    fn c4_run_limit_uncertain_application_reconciles_and_releases_its_hold() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::c4_run_limit_uncertain_application_reconciles_and_releases_its_hold",
        ) {
            return;
        }
        let b = b21_start();
        let result = kinewright_project::io_observer::fail_nth("root_parent_sync", 2, || cut(&b));
        assert!(matches!(
            result.unwrap().unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
        let producer = b.f.producer(&b.run);
        let owner = b.f.host.owner.upgrade().unwrap();
        {
            let mut host = owner.lock().unwrap();
            assert_eq!(host.hooks.pending_edit_applications.len(), 1);
            assert!(
                host.hooks.pending.is_some(),
                "C4_UNCERTAIN_APPLICATION_IS_PENDING"
            );
            host.reconcile_native_pending(&producer).unwrap();
            assert!(host.hooks.pending.is_none());
            assert!(
                host.hooks.pending_edit_applications.is_empty(),
                "C4_RECONCILIATION_RELEASES_PLAN"
            );
            let usage = host.owner().unwrap().store().unwrap().usage().unwrap();
            assert_eq!(usage.reserved_bytes, 0);
            assert!(!usage.unsettled && !usage.damaged_accounting);
        }
        assert_eq!(b.f.projection(&b.run).edit_applications.len(), 1);
    }
    #[test]
    fn c4_run_limit_person_save_and_stop_survive_a_superseded_application_plan() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::c4_run_limit_person_save_and_stop_survive_a_superseded_application_plan",
        ) {
            return;
        }
        let b = b21_start();
        let result =
            kinewright_project::io_observer::fail_nth("root_before_publish", 2, || cut(&b));
        assert!(matches!(
            result.unwrap().unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
        let owner = b.f.host.owner.upgrade().unwrap();
        let manual = {
            let host = owner.lock().unwrap();
            person_history_redo(host.core(), 970)
        };
        // The save request owns its history; retain an independent before-save oracle.
        ok(
            b.f.editor
                .request(HostCommand::Save {
                    snapshot: HostSnapshot::from_history(manual.clone()).unwrap(),
                })
                .unwrap(),
            "C4_PERSON_SAVE_WITH_HELD_APPLICATION",
        );
        assert!(
            b.f.projection(&b.run).edit_applications.is_empty(),
            "C4_PERSON_SAVE_IS_INDEPENDENT_OF_AUDIT_RETRY"
        );
        let identity =
            b.f.projection(&b.run)
                .attempts
                .iter()
                .find(|attempt| attempt.identity.task == b.child)
                .unwrap()
                .identity
                .clone();
        let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &b.f.temp.path("c4-stop-proof"),
            identity.project.as_str(),
            b.run.0.as_str(),
        )
        .unwrap();
        let native = crate::native_launcher::native_identity(&identity);
        claim.reserve(&native).unwrap();
        let proof =
            crate::guardian::RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap();
        let stop = b.f.control(&b.f.editor, &b.run, RunAction::Stop);
        ok(
            b.f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                })
                .unwrap(),
            "C4_STOP_WITH_SUPERSEDED_PLAN",
        );
        // The scripted fixture has no process. Like the C3 cleanup witnesses,
        // deliver its independent absence proof after Stop closes the claim.
        let producer = b.f.producer(&b.run);
        owner
            .lock()
            .unwrap()
            .reconcile_native_effect(
                &producer,
                &identity,
                &crate::guardian::AttemptRetirementProof::Unbound(&proof),
            )
            .unwrap();
        claim.retire_unbound(&proof).unwrap();
        claim.finish().unwrap();
        // Stop's task checkpoint/native publication carries the marker through
        // current state, preserving the person's newer save and releasing it.
        let host = owner.lock().unwrap();
        assert_eq!(
            host.owner().unwrap().core().export_history().unwrap(),
            manual,
            "C4_STALE_ROOT_NEVER_OVERWRITES_PERSON_SAVE"
        );
        assert_eq!(
            host.owner()
                .unwrap()
                .orchestration_projection(&b.run)
                .unwrap()
                .unwrap()
                .edit_applications
                .len(),
            1,
            "C4_STOP_PUBLISHES_PENDING_APPLICATION"
        );
        assert!(
            host.hooks.pending_edit_applications.is_empty(),
            "C4_STOP_RELEASES_SUPERSEDED_PLAN"
        );
    }
    #[test]
    fn c4_run_limit_application_io_crash_child() {
        let Some(handoff) = std::env::var_os("KINEWRIGHT_C4_AUDIT_HANDOFF") else {
            return;
        };
        let b = b21_start();
        let kept = c5e_history(&b.f, &b.run, &b.child);
        let result =
            kinewright_project::io_observer::fail_nth("root_before_publish", 2, || cut(&b));
        assert!(
            matches!(
                result.unwrap().unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ),
            "C4_SUCCESS_SURVIVES_IO_FAILURE"
        );
        let state = b.f.projection(&b.run);
        assert_eq!(state.edit_approvals.len(), 1);
        assert!(
            state.edit_applications.is_empty(),
            "C4_EXISTING_PENDING_MARKER_STATE"
        );
        assert_eq!(
            b.core.export_history().unwrap().document.tracks[0]
                .clips
                .len(),
            1
        );
        let owner = b.f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        assert_eq!(
            host.hooks.pending_edit_applications.len(),
            1,
            "C4_PENDING_OWNS_RESERVATION"
        );
        assert!(
            host.owner()
                .unwrap()
                .store()
                .unwrap()
                .usage()
                .unwrap()
                .reserved_bytes
                > 0,
            "C4_SECOND_CUT_HOLD_SURVIVES_IO_ERROR"
        );
        std::fs::write(
            handoff,
            serde_json::to_vec(&serde_json::json!({
                "root":b.f.temp.root(),"run":b.run,"task":b.child,"kept":kept,
            }))
            .unwrap(),
        )
        .unwrap();
        // A real process exit skips host/task/plan Drop and all graceful checkpoints.
        std::process::exit(86);
    }
    #[test]
    fn c4_run_limit_application_io_failure_reopens_without_reservation_leak() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::c4_run_limit_application_io_failure_reopens_without_reservation_leak",
        ) {
            return;
        }
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c4-audit-handoff",
        );
        let log = std::fs::File::create(temp.path("crash.log")).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "writer_host::hooks_tests::tests::tests::storage_admission::tests::c4_run_limit_application_io_crash_child", "--nocapture"])
            .env("KINEWRIGHT_C4_AUDIT_HANDOFF", temp.path("handoff.json"))
            .stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(35);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("C4_CRASH_CHILD_TIMED_OUT");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            status.code(),
            Some(86),
            "C4_CRASH_CUT {}",
            std::fs::read_to_string(temp.path("crash.log")).unwrap()
        );
        let mut handoff: serde_json::Value =
            serde_json::from_slice(&std::fs::read(temp.path("handoff.json")).unwrap()).unwrap();
        let root: std::path::PathBuf = serde_json::from_value(handoff["root"].take()).unwrap();
        let run: RunId = serde_json::from_value(handoff["run"].take()).unwrap();
        let task: TaskId = serde_json::from_value(handoff["task"].take()).unwrap();
        let kept: kinewright_core::HistorySnapshot =
            serde_json::from_value(handoff["kept"].take()).unwrap();
        let mut reopened = kinewright_project::editing_session::Ja1EditingSession::open(
            &root.join("project.kinewright"),
        )
        .unwrap();
        let state = reopened.orchestration_projection(&run).unwrap().unwrap();
        assert_eq!(state.edit_approvals.len(), 1);
        assert!(
            state.edit_applications.is_empty(),
            "C4_REOPEN_APPROVAL_WITHOUT_APPLICATION"
        );
        let store = reopened.store().unwrap();
        let retained = state.tasks.iter().find(|record| record.id == task).unwrap()
            .progress.decisions.iter().rev().filter(|reference| reference.record_kind == kinewright_project::bundle_records::RecordKind::RunIndex)
            .find_map(|reference| {
                let record = store.read_run_record(reference).unwrap();
                match record.payload {
                    kinewright_project::orchestration_records::OrchestrationPayload::TaskHistory(history) => Some(history),
                    _ => None,
                }
            }).unwrap();
        let binding = reopened.candidate_binding(&retained.source.branch).unwrap();
        let checkpoint = store
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &binding.checkpoint,
            )
            .unwrap();
        let history = store
            .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)
            .unwrap();
        assert_eq!(&*history, &kept, "C4_REOPEN_EXACT_PREVIOUS_CHECKPOINT");
        drop(history);
        drop(checkpoint);
        let usage = reopened.store().unwrap().usage().unwrap();
        assert_eq!(usage.reserved_bytes, 0, "C4_NO_HOLD_ACROSS_REOPEN");
        assert!(!usage.damaged_accounting && !usage.unsettled);
        reopened.save_active().unwrap();
        drop(reopened);
        assert!(root.starts_with(kinewright_media::test_support::workspace_target_dir()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
