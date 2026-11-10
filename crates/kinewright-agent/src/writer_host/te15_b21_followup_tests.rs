#[cfg(test)]
mod tests {
    use super::super::*;
    const PREFIX: &str = "tests::consent_followups::tests::";
    fn child(name: &str) -> bool {
        te15_fix_counting_child(&format!("{PREFIX}{name}"))
    }
    fn text(reply: &rmcp::model::CallToolResult) -> String {
        serde_json::to_string(reply).unwrap()
    }
    fn step(b: &B21, redo: bool) -> rmcp::model::CallToolResult {
        b.mcp.raw(
            if redo {
                "redo_task_edit"
            } else {
                "undo_task_edit"
            },
            serde_json::json!({"expected_revision":grant_test_revision(&b.core)}),
        )
    }
    fn edit(b: &B21, operations: &serde_json::Value) {
        let p=b.mcp.raw("prepare_edit_plan",serde_json::json!({"expected_revision":grant_test_revision(&b.core),"operations":operations}));
        assert_eq!(p.is_error, Some(false), "FOL_PREPARE {p:?}");
        let c = b21_commit(b, &p);
        assert_eq!(c.is_error, Some(false), "FOL_COMMIT {c:?}");
    }
    #[test]
    fn te15_b21f_legacy_history_refuses() {
        if !child("te15_b21f_legacy_history_refuses") {
            return;
        }
        for redo in [false, true] {
            let b = b21_start_options(Role::Editor, true);
            assert!(b.f.projection(&b.run).edit_consent.is_none());
            let operation = kinewright_core::Operation::TrimClip {
                clip: kinewright_core::ClipId(3),
                new_source: kinewright_core::TimeCode(0)..kinewright_core::TimeCode(8),
            };
            let state = b.f.projection(&b.run);
            let brief = &state.tasks.iter().find(|t| t.id == b.child).unwrap().brief;
            crate::orchestration::policy::TaskPolicy {
                role: brief.role,
                scope: brief.scope.clone(),
            }
            .validate_batch(
                &b.core.export_history().unwrap().document,
                std::slice::from_ref(&operation),
            )
            .expect("FOL_LEGACY_ALLOWED_BASELINE");
            assert!(matches!(
                b.core
                    .request(kinewright_core::Command::Do(operation))
                    .unwrap(),
                kinewright_core::Event::DocumentChanged { .. }
            ));
            if redo {
                b.core.request(kinewright_core::Command::Undo).unwrap();
            }
            let before = b.core.export_history().unwrap();
            let r = step(&b, redo);
            assert_eq!(r.is_error, Some(true), "FOL_LEGACY_HISTORY {r:?}");
            assert!(
                text(&r).contains("consent_missing")
                    && text(&r).contains("Stop it and start a new run."),
                "FOL_LEGACY_WORDS {r:?}"
            );
            assert_eq!(
                b.core.export_history().unwrap(),
                before,
                "FOL_LEGACY_UNCHANGED"
            );
        }
    }
    #[test]
    fn te15_b21f_history_uses_inverse_classes_and_records_application() {
        if !child("te15_b21f_history_uses_inverse_classes_and_records_application") {
            return;
        }
        let b = b21_start_role(Role::Motion);
        let initial = b.core.export_history().unwrap();
        edit(
            &b,
            &serde_json::json!([{"op":"add_solid_clip","track":81,"timeline_start":30,"duration":10,"color":{"r":0,"g":0,"b":0}}]),
        );
        let added = b.core.export_history().unwrap();
        let r = step(&b, false);
        assert_eq!(r.is_error, Some(false), "FOL_HISTORY_ALLOWED {r:?}");
        assert_eq!(b.core.export_history().unwrap().document, initial.document);
        let page = b201_page(&b.f, &b.run);
        assert!(
            page.edit_approvals
                .iter()
                .any(|r| r.removed_clips == 1 && r.applied),
            "FOL_DESTRUCTIVE_UNDO_RECORDED"
        );
        let r = step(&b, true);
        assert_eq!(r.is_error, Some(false), "FOL_REDO_ALLOWED {r:?}");
        assert_eq!(b.core.export_history().unwrap().document, added.document);
        let state = b.f.projection(&b.run);
        let owner = b.f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let bytes = host
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_evidence(state.edit_consent.as_ref().unwrap())
            .unwrap();
        let mut grant: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        grant["classes"] = serde_json::json!(["timeline_edit"]);
        host.hooks.consent_fixture = Some(
            crate::writer_host::task_mcp::consent::ConsentFixture::Record(
                host.owner_mut()
                    .unwrap()
                    .admit_host_grant(&serde_json::to_vec(&grant).unwrap())
                    .unwrap(),
            ),
        );
        drop(host);
        let before = b.core.export_history().unwrap();
        let r = step(&b, false);
        assert_eq!(r.is_error, Some(true), "FOL_INVERSE_CLASS {r:?}");
        assert_eq!(b.core.export_history().unwrap(), before);
    }
    #[test]
    fn te15_b21f_add_track_is_permitted_in_scope() {
        if !child("te15_b21f_add_track_is_permitted_in_scope") {
            return;
        }
        let b = b21_start();
        edit(
            &b,
            &serde_json::json!([{"op":"add_track","track":{"id":88,"kind":"Video","sync_lock":false,"clips":[]}}]),
        );
        assert!(
            b.core
                .export_history()
                .unwrap()
                .document
                .tracks
                .iter()
                .any(|t| t.id.0 == 88),
            "FOL_ADD_TRACK"
        );
    }
    fn busy(
        ms: u64,
    ) -> (
        B21,
        rmcp::model::CallToolResult,
        kinewright_core::HistorySnapshot,
    ) {
        let b = b21_start();
        let before = b.core.export_history().unwrap();
        let prepared = b21_prepare(&b);
        let (sent, received) = crossbeam_channel::bounded(1);
        let (release, wait) = crossbeam_channel::bounded(1);
        *b.mcp.server.confirmations().task_hook.lock().unwrap() = Some(Arc::new(move || {
            sent.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        }));
        let mcp = &b.mcp;
        let revision = grant_test_revision(&b.core);
        let plan = prepared.structured_content.unwrap()["plan_id"].clone();
        let result = std::thread::scope(|s| {
            let call = s.spawn(|| b21_commit_mcp(mcp, revision, plan));
            received.recv_timeout(Duration::from_secs(10)).unwrap();
            let owner = b.f.host.owner.upgrade().unwrap();
            let guard = owner.lock().unwrap();
            release.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(ms));
            drop(guard);
            call.join().unwrap()
        });
        (b, result, before)
    }
    #[test]
    fn te15_b21f_short_host_hold_succeeds() {
        if !child("te15_b21f_short_host_hold_succeeds") {
            return;
        }
        let (_, r, _) = busy(50);
        assert_eq!(r.is_error, Some(false), "FOL_SHORT_BUSY {r:?}");
    }
    #[test]
    fn te15_b21f_long_host_hold_refuses_without_edit() {
        if !child("te15_b21f_long_host_hold_refuses_without_edit") {
            return;
        }
        let (b, r, before) = busy(700);
        assert_eq!(r.is_error, Some(true), "FOL_LONG_BUSY {r:?}");
        assert!(text(&r).contains("task_busy"), "FOL_BUSY_WORDS {r:?}");
        assert_eq!(b.core.export_history().unwrap(), before);
    }
    #[test]
    fn te15_b21f_failed_core_is_not_applied() {
        if !child("te15_b21f_failed_core_is_not_applied") {
            return;
        }
        let b = b21_start();
        let prepared = b21_prepare(&b);
        b.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .hooks
            .consent_apply_hook = Some(Arc::new(|core| {
            core.request(kinewright_core::Command::Do(history_marker(99)))
                .unwrap();
        }));
        let r = b21_commit(&b, &prepared);
        assert_eq!(r.is_error, Some(true));
        let page = b201_page(&b.f, &b.run);
        assert_eq!(page.edit_approvals.len(), 1);
        assert!(!page.edit_approvals[0].applied, "FOL_NOT_APPLIED");
        assert!(b.f.projection(&b.run).edit_applications.is_empty());
        assert_eq!(
            b.core.export_history().unwrap().document.tracks[0]
                .clips
                .len(),
            3
        );
    }
    #[test]
    fn te15_b21f_bad_audit_keeps_run_view() {
        if !child("te15_b21f_bad_audit_keeps_run_view") {
            return;
        }
        let b = b21_start();
        let mut state = b.f.projection(&b.run);
        let reference =
            b.f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner_mut()
                .unwrap()
                .admit_host_grant(b"corrupt approval")
                .unwrap();
        state.edit_approvals.push(reference);
        publish_projection(&b.f, &b.f.producer_client(&b.run), &b.run, state, None);
        let page = b201_page(&b.f, &b.run);
        assert!(
            page.edit_approvals.iter().any(|r| r.issue.is_some()),
            "FOL_AUDIT_INTEGRITY"
        );
        assert!(
            !page.allowed_actions.is_empty(),
            "FOL_RUN_CONTROLS_RETAINED"
        );
    }
    #[test]
    fn te15_b21f_broker_capacity_has_its_reason() {
        if !child("te15_b21f_broker_capacity_has_its_reason") {
            return;
        }
        let b = b21_start();
        let prepared = b21_prepare(&b);
        let broker = b.mcp.server.confirmations();
        let held: Vec<_> = (0..ITEM_LIMIT)
            .map(|i| {
                broker
                    .request_task(&serde_json::json!({"held":i}), "held".into())
                    .unwrap()
            })
            .collect();
        let before = b.core.export_history().unwrap();
        let r = b21_commit(&b, &prepared);
        assert!(
            text(&r).contains("consent_broker_capacity")
                && text(&r).contains("confirmation capacity"),
            "FOL_BROKER_WORDS {r:?}"
        );
        assert_eq!(b.core.export_history().unwrap(), before);
        drop(held);
    }
    #[test]
    fn te15_b21f_policy_and_turn_reasons() {
        if !child("te15_b21f_policy_and_turn_reasons") {
            return;
        }
        let b = b21_start();
        let r=b.mcp.raw("prepare_edit_plan",serde_json::json!({"expected_revision":grant_test_revision(&b.core),"operations":[{"op":"remove_track","track":999}]}));
        assert!(
            text(&r).contains("consent_policy") && text(&r).contains("999"),
            "FOL_POLICY_WORDS {r:?}"
        );
        let mut state = b.f.projection(&b.run);
        state
            .tasks
            .iter_mut()
            .find(|t| t.id == b.child)
            .unwrap()
            .progress
            .turn_ended = true;
        publish_projection(&b.f, &b.f.producer_client(&b.run), &b.run, state, None);
        let r = b21_prepare(&b);
        assert!(text(&r).contains("consent_turn"), "FOL_TURN_WORDS {r:?}");
    }
    #[test]
    fn te15_b21f_scope_reason_tells_director() {
        if !child("te15_b21f_scope_reason_tells_director") {
            return;
        }
        let b = b21_start();
        let state = b.f.projection(&b.run);
        let owner = b.f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let bytes = host
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_evidence(state.edit_consent.as_ref().unwrap())
            .unwrap();
        let mut grant: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        grant["candidate"] = serde_json::json!("0".repeat(32));
        let reference = host
            .owner_mut()
            .unwrap()
            .admit_host_grant(&serde_json::to_vec(&grant).unwrap())
            .unwrap();
        host.hooks.consent_fixture =
            Some(crate::writer_host::task_mcp::consent::ConsentFixture::Record(reference));
        drop(host);
        let before = b.core.export_history().unwrap();
        let r = b21_prepare(&b);
        assert!(
            text(&r).contains("consent_scope") && text(&r).contains("Tell the Director"),
            "FOL_SCOPE_WORDS {r:?}"
        );
        assert_eq!(b.core.export_history().unwrap(), before);
    }
    fn fixed(grant: bool) {
        let b = b21_start();
        let p = b21_prepare(&b);
        assert_eq!(b21_commit(&b, &p).is_error, Some(false));
        let before = b.f.projection(&b.run);
        let mut bad = before.clone();
        if grant {
            bad.edit_consent = None;
        } else {
            bad.edit_approvals.clear();
        }
        let actor = b.f.producer_client(&b.run);
        let request = b.f.request(
            &actor,
            Some(b.run.clone()),
            OrchestrationCommand::RecordContext {
                task: bad.tasks[0].id.clone(),
                feedback: vec![],
                decisions: vec![],
            },
        );
        bad.sequence += 1;
        bad.control_revision = request.envelope.identity("control-revision").unwrap();
        let owner = b.f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let segment = EventSegment {
            run: b.run.clone(),
            first_sequence: bad.sequence,
            previous: Some(
                host.owner()
                    .unwrap()
                    .orchestration_cut(&b.run)
                    .unwrap()
                    .unwrap()
                    .segment,
            ),
            events: vec![OrchestrationEvent {
                command: request.envelope.id.clone(),
                sequence: bad.sequence,
                projection: bad.clone(),
            }],
        };
        let update = ValidatedRunUpdate::new(
            &request.envelope.project,
            request.envelope.expected_generation,
            &request.envelope,
            bad,
            segment,
            CommandDecision::Applied,
        )
        .unwrap();
        let refused = host
            .publish_orchestration_transition(&actor.bootstrap.auth, &update, None)
            .is_err();
        drop(host);
        assert!(refused, "FOL_FIXED_STORE grant={grant}");
        assert_eq!(b.f.projection(&b.run), before, "FOL_FIXED_STATE_UNCHANGED");
    }
    fn fixed_applications() {
        let b = b21_start();
        let p = b21_prepare(&b);
        assert_eq!(b21_commit(&b, &p).is_error, Some(false));
        let before = b.f.projection(&b.run);
        let mut bad = before.clone();
        bad.edit_applications.clear();
        let actor = b.f.producer_client(&b.run);
        let request = b.f.request(
            &actor,
            Some(b.run.clone()),
            OrchestrationCommand::RecordContext {
                task: bad.tasks[0].id.clone(),
                feedback: vec![],
                decisions: vec![],
            },
        );
        bad.sequence += 1;
        bad.control_revision = request.envelope.identity("control-revision").unwrap();
        let owner = b.f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let segment = EventSegment {
            run: b.run.clone(),
            first_sequence: bad.sequence,
            previous: Some(
                host.owner()
                    .unwrap()
                    .orchestration_cut(&b.run)
                    .unwrap()
                    .unwrap()
                    .segment,
            ),
            events: vec![OrchestrationEvent {
                command: request.envelope.id.clone(),
                sequence: bad.sequence,
                projection: bad.clone(),
            }],
        };
        let update = ValidatedRunUpdate::new(
            &request.envelope.project,
            request.envelope.expected_generation,
            &request.envelope,
            bad,
            segment,
            CommandDecision::Applied,
        )
        .unwrap();
        let refused = host
            .publish_orchestration_transition(&actor.bootstrap.auth, &update, None)
            .is_err();
        drop(host);
        assert!(refused, "B2B_APPLICATIONS_APPEND_ONLY");
        assert_eq!(b.f.projection(&b.run), before, "FOL_FIXED_STATE_UNCHANGED");
    }
    #[test]
    fn te15_b2b_applications_are_append_only() {
        if child("te15_b2b_applications_are_append_only") {
            fixed_applications();
        }
    }
    #[test]
    fn te15_b2b_net_zero_removal() {
        if !child("te15_b2b_net_zero_removal") {
            return;
        }
        let b = b21_start();
        let before = b.core.export_history().unwrap();
        edit(
            &b,
            &serde_json::json!([{"op":"split_clip","clip":3,"at":25},{"op":"delete_clip","clip":4},{"op":"trim_clip","clip":3,"new_source":{"start":0,"end":10}}]),
        );
        assert_eq!(b.core.export_history().unwrap().document, before.document);
        assert!(
            b.f.projection(&b.run).edit_approvals.is_empty(),
            "B2B_NO_ZERO_APPROVAL"
        );
        assert!(
            b201_page(&b.f, &b.run).edit_approvals.is_empty(),
            "B2B_NO_BLANK_REMOVAL_ROW"
        );
    }
    #[test]
    fn te15_b2b_liveness_prepare_and_commit() {
        if !child("te15_b2b_liveness_prepare_and_commit") {
            return;
        }
        for commit in [false, true] {
            let b = b21_start();
            let before = b.core.export_history().unwrap();
            let prepared = b21_prepare(&b);
            let owner = b.f.host.owner.upgrade().unwrap();
            owner.lock().unwrap().hooks.runs[&b.run].tasks[&b.child]
                .endpoint()
                .set_paused(true);
            let r = if commit {
                b21_commit(&b, &prepared)
            } else {
                b21_prepare(&b)
            };
            assert_eq!(r.is_error, Some(true));
            assert!(
                text(&r).contains("consent_liveness"),
                "B2B_LIVENESS_CODE commit={commit} {r:?}"
            );
            assert_eq!(b.core.export_history().unwrap(), before);
        }
    }
    #[test]
    fn te15_b2b_applied_marker_retries() {
        use std::os::unix::fs::PermissionsExt;
        if !child("te15_b2b_applied_marker_retries") {
            return;
        }
        let b = b21_start();
        let prepared = b21_prepare(&b);
        let owner = b.f.host.owner.upgrade().unwrap();
        let objects = owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .resolved_root()
            .unwrap()
            .with_extension("kinewright-assets")
            .join("studio/objects");
        let permissions = std::fs::metadata(&objects).unwrap().permissions();
        let held = objects.clone();
        owner.lock().unwrap().hooks.consent_apply_hook = Some(Arc::new(move |_| {
            std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o500)).unwrap();
        }));
        let result = b21_commit(&b, &prepared);
        std::fs::set_permissions(&objects, permissions).unwrap();
        assert_eq!(
            result.is_error,
            Some(false),
            "B2B_SUCCESS_SURVIVES_AUDIT_FAILURE {result:?}"
        );
        assert_eq!(
            b.core.export_history().unwrap().document.tracks[0]
                .clips
                .len(),
            1
        );
        assert!(b.f.projection(&b.run).edit_applications.is_empty());
        assert_eq!(
            owner.lock().unwrap().hooks.pending_edit_applications.len(),
            1,
            "B2B_PENDING_MARKER_RETAINED"
        );
        assert!(
            owner.lock().unwrap().message.contains("audit"),
            "B2B_PENDING_MARKER_LOGGED"
        );
        let producer = b.f.producer(&b.run);
        let state = b.f.projection(&b.run);
        let identity = state
            .attempts
            .iter()
            .find(|a| a.identity.task == b.child)
            .unwrap()
            .identity
            .clone();
        owner
            .lock()
            .unwrap()
            .publish_native(&producer, state, &identity, vec![])
            .unwrap();
        let state = b.f.projection(&b.run);
        assert_eq!(
            state.edit_applications.len(),
            1,
            "B2B_MARKER_NEXT_PUBLICATION"
        );
        owner
            .lock()
            .unwrap()
            .publish_native(&producer, state, &identity, vec![])
            .unwrap();
        assert_eq!(
            b.f.projection(&b.run).edit_applications.len(),
            1,
            "B2B_MARKER_EXACTLY_ONCE"
        );
        assert!(
            owner
                .lock()
                .unwrap()
                .hooks
                .pending_edit_applications
                .is_empty()
        );
    }
    #[test]
    fn te15_b21f_grant_is_fixed() {
        if child("te15_b21f_grant_is_fixed") {
            fixed(true);
        }
    }
    #[test]
    fn te15_b21f_approvals_are_append_only() {
        if child("te15_b21f_approvals_are_append_only") {
            fixed(false);
        }
    }
}
