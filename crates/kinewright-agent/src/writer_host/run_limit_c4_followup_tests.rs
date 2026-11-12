#[cfg(test)]
mod followups {
    use super::*;
    use kinewright_core::{Command as CoreCommand, Event, Operation};
    use kinewright_project::bundle_records::SessionId;

    #[test]
    #[cfg_attr(
        all(target_os = "linux", not(feature = "slow-tests")),
        ignore = "slow tier: cargo test --features slow-tests"
    )]
    fn c4_followups_settled_pending_releases_hold_without_restart() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::followups::c4_followups_settled_pending_releases_hold_without_restart",
        ) {
            return;
        }
        let b = b21_start();
        assert!(matches!(
            kinewright_project::io_observer::fail_nth("root_before_publish", 2, || cut(&b))
                .unwrap()
                .unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
        let owner = b.f.host.owner.upgrade().unwrap();
        let Event::QueryResult(kinewright_core::QueryResult::Snapshot { revision, document }) = b
            .core
            .request(CoreCommand::Query(kinewright_core::Query::Snapshot))
            .unwrap()
        else {
            panic!("snapshot")
        };
        let error = kinewright_project::io_observer::fail_nth("root_before_publish", 1, || {
            control(&b).apply_with_edit_consent(
                &crate::ConfirmationBroker::default(),
                &b.core,
                revision,
                &document,
                &[Operation::DeleteClip {
                    clip: kinewright_core::ClipId(3),
                }],
                Some("Remove the remaining clip"),
            )
        })
        .unwrap_err();
        assert!(
            matches!(&error, HostError::Refused(refusal)
            if refusal.code == "io"
                && refusal.message.contains("This new edit was not applied")
                && refusal.message.contains("finish or Stop")
                && refusal.message.contains("injected nth root_before_publish")),
            "C4_FOLLOWUPS_RETRY_FAILURE_KEEPS_CAUSE_AND_ACTION {error}"
        );
        let pause = b.f.control(&b.f.editor, &b.run, RunAction::Pause);
        ok(
            b.f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(pause),
                })
                .unwrap(),
            "C4_FOLLOWUPS_SUPERSEDE_APPLICATION",
        );
        {
            let mut host = owner.lock().unwrap();
            host.hooks.consent_marker_hook = Some(Box::new(|| {
                Err(
                    HostRefusal::new("storage_backpressure", "fixture marker admission refused")
                        .into(),
                )
            }));
            let state = host
                .owner()
                .unwrap()
                .orchestration_projection(&b.run)
                .unwrap()
                .unwrap();
            let identity = state
                .attempts
                .iter()
                .find(|a| a.identity.task == b.child)
                .unwrap()
                .identity
                .clone();
            let producer = host.run_producer(&b.run).unwrap();
            host.publish_native(&producer, state, &identity, vec![])
                .unwrap();
            assert!(host.has_pending_edit_in_run(&b.run));
            assert!(!host.has_pending_edit_in_run(&RunId(SessionId::fresh().unwrap())));
            assert!(
                host.owner()
                    .unwrap()
                    .store()
                    .unwrap()
                    .usage()
                    .unwrap()
                    .reserved_bytes
                    > 0
            );
        }
        kept_audit::stop_with_absence_proof(&b.f.host, &b.run, b.f.temp.root());
        let page = b201_page(&b.f, &b.run);
        assert_eq!(page.projection.cleanup, RunCleanup::Settled);
        assert!(
            page.projection.edit_applications.is_empty(),
            "C4_FOLLOWUPS_NO_FABRICATED_APPLICATION"
        );
        assert_eq!(page.edit_approvals.len(), 1);
        assert!(
            !page.edit_approvals[0].applied,
            "C4_FOLLOWUPS_APPROVAL_WITHOUT_APPLICATION_IS_UNCONFIRMED"
        );
        assert!(page.missing_approved_edits.is_empty());
        {
            let mut host = owner.lock().unwrap();
            assert!(
                host.hooks.pending_edit_applications.is_empty(),
                "C4_FOLLOWUPS_SETTLED_RUN_RELEASES_PENDING"
            );
            assert_eq!(
                host.owner()
                    .unwrap()
                    .store()
                    .unwrap()
                    .usage()
                    .unwrap()
                    .reserved_bytes,
                0,
                "C4_FOLLOWUPS_SETTLED_RUN_RELEASES_HOLD"
            );
            assert!(host.message.contains("application record was not kept"));
            host.hooks.consent_marker_hook = None;
        }
        let B21 {
            _pump: pump,
            mcp,
            core,
            child: _,
            run: _,
            mut f,
        } = b;
        drop(pump);
        drop(mcp);
        drop(core);
        // The simulated launcher has no process/event reaper. Finish its real
        // fixture claim through recovery before the next run reuses that namespace.
        let mut claim = crate::execution_claim::MachineClaim::recover_fixture(
            &f.temp.path("fix4-resumed-claim"),
        )
        .unwrap();
        while let Some(identity) = claim.recovery_identities().first().cloned() {
            let proof =
                crate::guardian::RecoveryObserver::unbound_cleanup(&mut claim, &identity).unwrap();
            claim.retire_unbound(&proof).unwrap();
        }
        claim.finish().unwrap();
        {
            let mut host = owner.lock().unwrap();
            let project = host.owner_mut().unwrap();
            // Each real run gets its own candidate; never reuse a prior run's scope.
            let mut candidate = project
                .fork_durable_branch("next consent candidate")
                .unwrap();
            project.persist_durable_branch(&mut candidate).unwrap();
            f.brief.scope.base = project.candidate_binding(candidate.branch_id()).unwrap();
        }
        let next = b21_run(f, Role::Editor);
        assert!(
            matches!(
                cut(&next).unwrap().unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ),
            "C4_FOLLOWUPS_NEW_RUN_APPROVES_WITHOUT_RESTART"
        );
        assert_eq!(next.f.projection(&next.run).edit_applications.len(), 1);
        kept_audit::stop_with_absence_proof(&next.f.host, &next.run, next.f.temp.root());
    }

    fn checkpoint(b: &B21) {
        let owner = b.f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let producer = host.run_producer(&b.run).unwrap();
        host.checkpoint_task_history(&producer, &b.run, &b.child)
            .unwrap();
    }
    #[test]
    #[cfg_attr(
        all(target_os = "linux", not(feature = "slow-tests")),
        ignore = "slow tier: cargo test --features slow-tests"
    )]
    fn c4_followups_undo_and_later_removal_match_kept_targets() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::followups::c4_followups_undo_and_later_removal_match_kept_targets",
        ) {
            return;
        }
        for track in [false, true] {
            let b = b21_start();
            let remove = || {
                let Event::QueryResult(kinewright_core::QueryResult::Snapshot {
                    revision,
                    document,
                }) = b
                    .core
                    .request(CoreCommand::Query(kinewright_core::Query::Snapshot))
                    .unwrap()
                else {
                    panic!("snapshot")
                };
                let operation = if track {
                    Operation::RemoveTrack {
                        track: kinewright_core::TrackId(81),
                    }
                } else {
                    Operation::DeleteClip {
                        clip: kinewright_core::ClipId(1),
                    }
                };
                assert!(matches!(
                    control(&b)
                        .apply_with_edit_consent(
                            &crate::ConfirmationBroker::default(),
                            &b.core,
                            revision,
                            &document,
                            &[operation],
                            Some("Remove the owned target")
                        )
                        .unwrap()
                        .unwrap(),
                    Event::DocumentChanged { .. }
                ));
            };
            remove();
            assert!(matches!(
                b.core.request(CoreCommand::Undo).unwrap(),
                Event::DocumentChanged { .. }
            ));
            checkpoint(&b);
            let page = b201_page(&b.f, &b.run);
            assert!(
                !page.edit_approvals[0].applied,
                "C4_FOLLOWUPS_UNDONE_REMOVAL_IS_NOT_KEPT track={track}"
            );
            assert_eq!(page.missing_approved_edits.len(), 1);
            assert!(
                page.edit_approvals[0]
                    .issue
                    .as_deref()
                    .unwrap()
                    .contains("undone or not kept")
            );
            remove();
            checkpoint(&b);
            let page = b201_page(&b.f, &b.run);
            assert_eq!(page.edit_approvals.len(), 2);
            assert!(
                page.edit_approvals.iter().all(|edit| edit.applied),
                "C4_FOLLOWUPS_LATER_REMOVAL_SATISFIES_BOTH_APPROVALS"
            );
            assert!(page.missing_approved_edits.is_empty());
            let owner = b.f.host.owner.upgrade().unwrap();
            let host = owner.lock().unwrap();
            let kept = host
                .owner()
                .unwrap()
                .edit_audit_summary(&page.projection, &[])
                .unwrap();
            assert!(kept.approvals.iter().all(|edit| edit.applied));
            assert!(kept.missing.is_empty());
        }
    }

    #[test]
    #[cfg_attr(
        all(target_os = "linux", not(feature = "slow-tests")),
        ignore = "slow tier: cargo test --features slow-tests"
    )]
    fn c4_followups_uncertain_edits_reach_both_readers() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::followups::c4_followups_uncertain_edits_reach_both_readers",
        ) {
            return;
        }
        let b = b21_start();
        kept_audit::remove_clip(&b, 1);
        checkpoint(&b);
        let owner = b.f.host.owner.upgrade().unwrap();
        {
            let mut host = owner.lock().unwrap();
            let mut state = host
                .owner()
                .unwrap()
                .orchestration_projection(&b.run)
                .unwrap()
                .unwrap();
            let mut saved = host.saved_task_history(&state, &b.child).unwrap().unwrap();
            // Exact source binding, but no known lineage: the bounded auxiliary
            // coverage may be incomplete without refusing the real checkpoint.
            saved.edit_coverage.cuts.clear();
            saved.edit_coverage.incomplete = true;
            let sealed = host
                .owner_mut()
                .unwrap()
                .admit_scheduler_record(OrchestrationRecord {
                    retention: kinewright_project::orchestration_retention::Retention::Complete,
                    version: 1,
                    payload: OrchestrationPayload::TaskHistory(Box::new(saved)),
                })
                .unwrap();
            state
                .tasks
                .iter_mut()
                .find(|task| task.id == b.child)
                .unwrap()
                .progress
                .decisions
                .push(sealed.reference.clone());
            let identity = state
                .attempts
                .iter()
                .find(|a| a.identity.task == b.child)
                .unwrap()
                .identity
                .clone();
            let producer = host.run_producer(&b.run).unwrap();
            host.publish_native(&producer, state, &identity, vec![])
                .unwrap();
            // The reader no longer owns this actor, as after task retirement;
            // it must use the kept checkpoint rather than live coordinates.
            drop(
                host.hooks
                    .runs
                    .get_mut(&b.run)
                    .unwrap()
                    .tasks
                    .remove(&b.child),
            );
        }
        let page = b201_page(&b.f, &b.run);
        assert!(!page.edit_approvals[0].applied);
        assert!(page.missing_approved_edits.is_empty());
        assert_eq!(
            page.uncertain_approved_edits.len(),
            1,
            "C4_FOLLOWUPS_UNCERTAIN_TYPED_RUN_VIEW"
        );
        let words = page.uncertain_approved_edits[0].disclosure();
        assert!(
            words.contains("clip 1 on track 81")
                && words.contains("Review the kept branch")
                && words.contains("continue the task")
        );
        kept_audit::stop_with_absence_proof(&b.f.host, &b.run, b.f.temp.root());
        let host = owner.lock().unwrap();
        let retained = host.owner().unwrap().retained_task_work().unwrap();
        let work = retained.iter().find(|work| work.task == b.child).unwrap();
        assert_eq!(
            work.uncertain_approved_edits.len(),
            1,
            "C4_FOLLOWUPS_UNCERTAIN_TYPED_RETAINED_WORK"
        );
        assert!(
            work.disclosure.contains(&words),
            "C4_FOLLOWUPS_RETAINED_UNCERTAIN_WORDS"
        );
    }
}
