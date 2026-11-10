fn b202_fault(point: super::super::task_preparation::PreparationCheckpoint) {
    let (f, _) = te15_fix_setup("codex");
    let r = c5_fix10_run_from(f);
    let (f, run, parent, director, first, attempt, mcp) = (
        r.f, r.run, r.parent, r.director, r.child, r.attempt, r.editor,
    );
    let (reference, result) = b20_publish(&f, &run, &first, &mcp);
    b20_retire(&f, &run, &attempt);
    assert_eq!(
        b20_merge(&director, &first, &reference, &result).is_error,
        Some(false)
    );
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    b20_retire(&f, &run, &parent);
    let effect_count = f.projection(&run).effects.len();
    let original = f
        .projection(&run)
        .effects
        .into_iter()
        .find(|e| e.task == child)
        .unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    owner.lock().unwrap().preparation_fault = Some(point);
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    assert!(pump.tick().is_ok(), "B202_FIRST_RETRY");
    let state = f.projection(&run);
    {
        let host = owner.lock().unwrap();
        assert!(
            !host.hooks.runs[&run].tasks.contains_key(&child),
            "B202_UNPUBLISHED_ROLLED_BACK"
        );
        assert!(
            host.saved_task_history(&state, &child).unwrap().is_none(),
            "B202_NO_UNPUBLISHED_HISTORY"
        );
    }
    assert!(state.effects.iter().any(|e| e == &original));
    assert!(sent.lock().unwrap().is_empty());
    let fork = owner
        .lock()
        .unwrap()
        .retained_preparation_fork(&state, &child)
        .unwrap()
        .expect("B2B_FORK_RETAINED_BEFORE_HISTORY");
    fix3_continue(&f, &run);
    pump.tick().unwrap();
    let current = f.projection(&run);
    let history = owner
        .lock()
        .unwrap()
        .saved_task_history(&current, &child)
        .unwrap()
        .expect("B2B_FORK_REUSED_AFTER_PUBLICATION_FAILURE: retry history must exist");
    assert_eq!(
        history.source.branch, fork,
        "B2B_FORK_REUSED_AFTER_PUBLICATION_FAILURE"
    );
    let state = f.projection(&run);
    assert_ne!(
        state.tasks.iter().find(|t| t.id == child).unwrap().state,
        TaskState::Cancelled,
        "B202_NO_CANCEL"
    );
    assert!(!matches!(state.wait, RunWait::Replan(_)), "B202_NO_REPLAN");
    let sends = sent.lock().unwrap();
    assert_eq!(sends.len(), 1, "B202_ONE_SEND");
    assert_eq!(sends[0].task, child, "B202_ONLY_CHILD_SEND");
    assert_eq!(state.effects.len(), effect_count, "B202_NO_NOTIFICATION");
    assert!(
        state
            .effects
            .iter()
            .any(|e| e.id == original.id && matches!(e.state, EffectState::Claimed(_))),
        "B202_SAME_EFFECT"
    );
}
#[test]
fn te15_b202_after_history() {
    if !te15_fix_counting_child("te15_b202_after_history") {
        return;
    }
    b202_fault(super::super::task_preparation::PreparationCheckpoint::History);
}
#[test]
fn te15_b202_setup_publish() {
    if !te15_fix_counting_child("te15_b202_setup_publish") {
        return;
    }
    b202_fault(super::super::task_preparation::PreparationCheckpoint::Publish);
}
#[test]
fn te15_b202_history_base_guard() {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, _, _mcp) = b20_start(&f, &run, &child);
    let mut state = f.projection(&run);
    let task = state.tasks.iter_mut().find(|t| t.id == child).unwrap();
    task.progress.decisions.clear();
    task.brief.scope.base.head = kinewright_project::bundle_records::HeadId::fresh().unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let err = owner
        .lock()
        .unwrap()
        .persist_runtime_task(&mut state, &attempt, false);
    assert!(
        matches!(err,Err(HostError::Refused(ref e)) if e.code=="task_preparation_history_base"),
        "B202_MIXED_BASE_REFUSED"
    );
}
fn b202_permanent(code: &str) {
    use super::super::task_preparation::{PreparationRefusal, preparation_refusal};
    let (f, _) = te15_fix_setup("codex");
    let run = f.start().envelope.identity("run").unwrap();
    let (mut pump, _) = fix4_pump(&f, &run, false);
    pump.before_preparation = Some(Arc::new(|_| {
        Err(std::io::Error::other("fixture store failure").into())
    }));
    pump.tick().unwrap();
    let mut before = f.projection(&run);
    // This witness installs the permanent reason with no prior wait. The B2b
    // precedence witness separately preserves the fixture's transient wait.
    before.wait = RunWait::None;
    before.phase = RunPhase::Running;
    publish_projection(&f, &f.producer_client(&run), &run, before.clone(), None);
    let child = before.tasks[0].id.clone();
    let attempt = before.attempts[0].identity.clone();
    let error =
        preparation_refusal(HostRefusal::new(code, "fixture typed permanent refusal").into());
    let kind = error
        .get_ref()
        .unwrap()
        .downcast_ref::<PreparationRefusal>()
        .unwrap()
        .kind;
    assert!(kind.permanent(), "B202_PERMANENT_KIND");
    let auth = f.producer(&run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .preparation_failed(&auth, &attempt, kind)
        .unwrap();
    let page = b201_page(&f, &run);
    assert_eq!(page.projection.phase, RunPhase::Interrupted);
    assert!(
        matches!(page.projection.wait, RunWait::Provider(_)),
        "B202_PERMANENT_NO_RETRY"
    );
    assert!(
        !page.allowed_actions.contains(&RunActionKind::Continue),
        "B202_PERMANENT_CONTINUE_DISABLED"
    );
    assert_eq!(page.projection.effects, before.effects);
    assert_eq!(
        page.projection
            .tasks
            .iter()
            .find(|t| t.id == child)
            .unwrap()
            .state,
        before.tasks.iter().find(|t| t.id == child).unwrap().state
    );
    assert_eq!(page.wait_reason.as_deref(), Some(kind.words()));
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(f.control(&f.editor, &run, RunAction::Continue)),
        })
        .unwrap();
    assert!(reply.error.is_some(), "B202_CONTINUE_REFUSED");
    assert_eq!(f.projection(&run).attempts.len(), before.attempts.len());
    if code == "task_preparation_frozen" {
        drop(pump);
        let root = f.temp.path("b202-restart");
        let f = c4_restart(f, &root);
        let reply = f
            .editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(f.control(&f.editor, &run, RunAction::Continue)),
            })
            .unwrap();
        assert!(reply.error.is_some(), "B202_PERMANENT_AFTER_RESTART");
    }
}
#[test]
fn te15_b202_permanent_frozen() {
    if !te15_fix_counting_child("te15_b202_permanent_frozen") {
        return;
    }
    b202_permanent("task_preparation_frozen");
}
#[test]
fn te15_b202_permanent_history() {
    if !te15_fix_counting_child("te15_b202_permanent_history") {
        return;
    }
    b202_permanent("task_preparation_missing_history");
}
#[test]
fn te15_b202_permanent_legacy() {
    if !te15_fix_counting_child("te15_b202_permanent_legacy") {
        return;
    }
    b202_permanent("task_preparation_legacy");
}
#[test]
fn te15_b202_permanent_base() {
    if !te15_fix_counting_child("te15_b202_permanent_base") {
        return;
    }
    b202_permanent("task_preparation_history_base");
}
fn b202_sent_wait(kind: &str) {
    let (f, _) = te15_fix_setup("codex");
    let r = c5_fix10_run_from(f);
    fix4_prior_work(&r.f, &r.run);
    let mut state = r.f.projection(&r.run);
    state
        .effects
        .iter_mut()
        .find(|e| e.task == r.child)
        .unwrap()
        .state = EffectState::Pending;
    publish_projection(&r.f, &r.f.producer_client(&r.run), &r.run, state, None);
    let (mut pump, sent) = fix4_pump(&r.f, &r.run, false);
    pump.tick().unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1);
    fix4_prior_work(&r.f, &r.run);
    let mut state = r.f.projection(&r.run);
    let identity = state
        .attempts
        .iter()
        .find(|a| a.identity.task == r.child)
        .unwrap()
        .identity
        .clone();
    let reason =
        r.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .admit_host_grant(b"existing external wait")
            .unwrap();
    let detail = WaitDetail {
        reason,
        missing: vec![],
        attempt: None,
    };
    state.wait = match kind {
        "files" => RunWait::Files(detail),
        "provider" => RunWait::Provider(detail),
        _ => RunWait::ProviderSettings(detail),
    };
    let wait = state.wait.clone();
    let effects = state.effects.clone();
    publish_projection(&r.f, &r.f.producer_client(&r.run), &r.run, state, None);
    b201_failed(&r.f, &r.run, &identity);
    assert_eq!(r.f.projection(&r.run).wait, wait, "B202_SENT_WAIT_PRIORITY");
    assert_eq!(r.f.projection(&r.run).effects, effects);
}
#[test]
fn te15_b202_sent_files() {
    if te15_fix_counting_child("te15_b202_sent_files") {
        b202_sent_wait("files");
    }
}
#[test]
fn te15_b202_sent_provider() {
    if te15_fix_counting_child("te15_b202_sent_provider") {
        b202_sent_wait("provider");
    }
}
#[test]
fn te15_b202_sent_settings() {
    if te15_fix_counting_child("te15_b202_sent_settings") {
        b202_sent_wait("settings");
    }
}
#[test]
fn te15_b202_permanent_replay() {
    if !te15_fix_counting_child("te15_b202_permanent_replay") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run = f.start().envelope.identity("run").unwrap();
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    pump.before_preparation = Some(Arc::new(|_| {
        Err(std::io::Error::other("fixture store failure").into())
    }));
    pump.tick().unwrap();
    let request = f.control(&f.editor, &run, RunAction::Continue);
    let original = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request.clone()),
        })
        .unwrap();
    assert!(original.error.is_none());
    let before = f.projection(&run);
    let auth = f.producer(&run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .preparation_failed(
            &auth,
            &before.attempts[0].identity,
            super::super::task_preparation::PreparationKind::FrozenCheckpoint,
        )
        .unwrap();
    let replay = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert!(replay.error.is_none(), "B202_ACCEPTED_REPLAY {replay:?}");
    assert_eq!(
        replay.orchestration, original.orchestration,
        "B202_IMMUTABLE_REPLAY"
    );
    assert_eq!(f.projection(&run).attempts.len(), before.attempts.len());
    assert!(sent.lock().unwrap().is_empty());
}
