use super::*;
use kinewright_project::orchestration_records::{OrchestrationCommand, RunAction};
fn with_host<R>(
    attachment: &crate::ja1_attachment::Ja1Attachment,
    act: impl FnOnce(&mut HostFeedback<'_>) -> R,
) -> R {
    let client = attachment.host_client().unwrap();
    act(&mut HostFeedback {
        client: &client,
        reader: attachment,
    })
}
fn wait_fixture() -> (c7::Project, crate::ja1_attachment::Ja1Attachment, RunId) {
    let mut project = c7::project("te15-fix4-ui");
    let attachment = c7::launch(&project);
    project.feedback.issuer = None;
    with_host(&attachment, |port| {
        assert!(matches!(
            project.feedback.submit(port),
            Outcome::Submitted(_)
        ));
    });
    let run = project.feedback.rows[0].run.clone().unwrap();
    with_host(&attachment, |port| {
        let page = port.run_view(&run).unwrap();
        let mut issuer = None;
        let mut request = port
            .prepare_control(
                &run,
                RunActionKind::Stop,
                &page.projection.control_revision,
                &mut issuer,
            )
            .unwrap();
        let OrchestrationCommand::ControlRun(intent) = &mut request.envelope.command else {
            unreachable!()
        };
        intent.action = RunAction::SendMessage {
            text_ref: page.projection.tasks[0].brief.assignment.clone(),
        };
        request.envelope.digest = request.envelope.computed_digest().unwrap();
        std::fs::write(project.temp.path("scheduler/specialists.json"), b"broken").unwrap();
        assert!(matches!(
            port.send_control(&request),
            stop::ControlReply::Refused(_)
        ));
        std::fs::remove_file(project.temp.path("scheduler/specialists.json")).unwrap();
        project.feedback.observe_runs(port);
    });
    (project, attachment, run)
}
fn text_position(shapes: &[egui::epaint::ClippedShape], text: &str) -> Pos2 {
    for clipped in shapes {
        if let egui::Shape::Text(shape) = &clipped.shape
            && shape.galley.job.text == text
        {
            let at = shape.pos + shape.galley.rect.center().to_vec2();
            if clipped.clip_rect.contains(at) {
                return at;
            }
        }
    }
    panic!("FIX4_UI_LABEL_MISSING {text}");
}
fn frame(
    ctx: &egui::Context,
    run: &RunId,
    state: &RunState,
    events: Vec<egui::Event>,
) -> (Option<NoteAction>, egui::FullOutput) {
    let mut action = None;
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 200.0))),
            events,
            ..Default::default()
        },
        |ui| {
            ui.label(state.label());
            ui.horizontal(|ui| action = run_controls(ui, run, Some(state), &[]));
        },
    );
    (action, output)
}
fn click(ctx: &egui::Context, run: &RunId, state: &RunState, at: Pos2) -> Option<NoteAction> {
    frame(
        ctx,
        run,
        state,
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(
        ctx,
        run,
        state,
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    )
    .0
}
#[test]
fn te15_b1_followup_continue_only_when_waiting() {
    let (project, _attachment, run) = wait_fixture();
    let original = project.feedback.rows[0]
        .state
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap();
    for phase in [
        RunPhase::Queued,
        RunPhase::Running,
        RunPhase::Reviewing,
        RunPhase::Finished,
    ] {
        let mut state = original.clone();
        state.phase = phase;
        let (_, output) = frame(&egui::Context::default(), &run, &state, vec![]);
        assert!(!output.shapes.iter().any(|clipped| matches!(&clipped.shape,egui::Shape::Text(t) if t.galley.job.text=="Continue")),"FOLLOWUP_CONTINUE_ONLY_WAITING");
    }
}
#[test]
fn te15_b1_fix4_notes_continue_and_exact_reopen_replay() {
    let (mut project, attachment, run) = wait_fixture();
    let state = project.feedback.rows[0]
        .state
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    assert!(matches!(state.wait, RunWait::ProviderSettings(_)));
    assert!(state.can_continue, "FIX4_UI_ELIGIBLE");
    let reason = state.view.wait_reason.as_ref().unwrap();
    assert!(reason.contains("Settings"));
    assert!(
        state
            .label()
            .contains("Fix Director and Specialists in Settings, then press Continue."),
        "FIX4_UI_WAIT_REASON"
    );
    assert_eq!(state.view.wait_reason.as_ref(), Some(reason));
    if let Some(to) = std::env::var_os("KINEWRIGHT_TE15_FIX4_SHOT_PROJECT") {
        copy_tree(project.temp.root(), std::path::Path::new(&to));
    }
    let ctx = egui::Context::default();
    let (_, output) = frame(&ctx, &run, &state, vec![]);
    let pos = text_position(&output.shapes, "Continue");
    let action = click(&ctx, &run, &state, pos).expect("FIX4_UI_CONTINUE_BUTTON");
    let NoteAction::Continue(selected) = action else {
        panic!("FIX4_UI_CONTINUE_ACTION")
    };
    assert_eq!(selected, run);
    let before = with_host(&attachment, |port| {
        port.run_view(&run).unwrap().projection.sequence
    });
    let original = with_host(&attachment, |port| {
        let mut lossy = Lossy::new(port);
        lossy.crash_at_dispatch = Some(project.temp.path("state").join(format!(
            "{}.json",
            attachment.studio_project_id().unwrap().as_str()
        )));
        lossy.after = 1;
        assert!(matches!(
            project
                .feedback
                .continue_run(&mut lossy, &selected)
                .unwrap(),
            stop::ControlReply::Unknown(_)
        ));
        assert_eq!(lossy.calls, 1, "FIX4_UI_CONTINUE_ONCE");
        project
            .feedback
            .controls
            .first()
            .expect("FIX4_UI_EXACT_REQUEST")
            .request
            .clone()
    });
    assert!(
        project
            .feedback
            .close_text()
            .contains("A Continue request is not confirmed."),
        "FIX4_UI_PENDING_CONTROL_WORDS"
    );
    let after = with_host(&attachment, |port| {
        port.run_view(&run).unwrap().projection.sequence
    });
    assert_eq!(after, before + 1, "FIX4_UI_HOST_CONTINUE_ONCE");
    let state = project.feedback.rows[0]
        .state
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    assert!(!state.can_continue);
    assert!(
        click(&ctx, &run, &state, pos).is_none(),
        "FIX4_UI_NO_DOUBLE_CONTINUE"
    );
    let directory = project.temp.path("state");
    project.feedback = Feedback::default();
    project
        .feedback
        .open(attachment.studio_project_id().map_err(text), Ok(directory))
        .unwrap();
    assert_eq!(
        project.feedback.controls[0].request, original,
        "FIX4_UI_EXACT_REQUEST"
    );
    with_host(&attachment, |port| project.feedback.resume(port));
    assert!(
        project.feedback.controls.is_empty(),
        "FIX4_UI_REPLAY_CONFIRMED"
    );
    assert_eq!(
        with_host(&attachment, |port| port
            .run_view(&run)
            .unwrap()
            .projection
            .sequence),
        after,
        "FIX4_UI_REPLAY_NO_EXTRA_SEND"
    );
}
#[test]
fn te15_b1_fix4_notes_unknown_disables_continue() {
    let (mut project, attachment, run) = wait_fixture();
    let mut page = with_host(&attachment, |port| port.run_view(&run).unwrap());
    let RunWait::ProviderSettings(detail) = page.projection.wait else {
        unreachable!()
    };
    page.projection.wait = RunWait::Provider(detail);
    page.allowed_actions
        .retain(|action| *action != RunActionKind::Continue);
    page.wait_reason = Some(
        "The send outcome is unknown. Press Stop, then Reopen to review your notes before starting another run. Continue cannot retry this send."
            .into(),
    );
    let state = RunState::of(&page);
    assert!(
        state.label().contains(page.wait_reason.as_deref().unwrap()),
        "FIX4_UI_UNKNOWN_REASON"
    );
    assert!(!state.label().contains("waiting for the provider"));
    let ctx = egui::Context::default();
    let (_, output) = frame(&ctx, &run, &state, vec![]);
    let pos = text_position(&output.shapes, "Continue");
    assert!(
        click(&ctx, &run, &state, pos).is_none(),
        "FIX4_UI_UNKNOWN_DISABLED"
    );
    project.feedback.rows[0].state = Some(Ok(state));
    with_host(&attachment, |port| {
        let mut lossy = Lossy::new(port);
        assert!(project.feedback.continue_run(&mut lossy, &run).is_err());
        assert_eq!(lossy.calls, 0, "FIX4_UI_UNKNOWN_NO_SEND");
    });
}

#[test]
fn te15_b1_fix4_continue_poll_confirms_same_request() {
    let (mut project, attachment, run) = wait_fixture();
    let before = with_host(&attachment, |port| {
        port.run_view(&run).unwrap().projection.sequence
    });
    with_host(&attachment, |port| {
        let mut lossy = Lossy::new(port);
        lossy.after = 1;
        assert!(matches!(
            project.feedback.continue_run(&mut lossy, &run).unwrap(),
            stop::ControlReply::Unknown(_)
        ));
    });
    assert!(project.feedback.observing(), "FIX4_UI_CONFIRMATION_POLLED");
    with_host(&attachment, |port| {
        let mut lossy = Lossy::new(port);
        project.feedback.retry_continues(&mut lossy);
        assert_eq!(lossy.calls, 1, "FIX4_UI_POLL_REPLAY");
    });
    assert!(
        project.feedback.controls.is_empty(),
        "FIX4_UI_POLL_CONFIRMED"
    );
    assert_eq!(
        with_host(&attachment, |port| port
            .run_view(&run)
            .unwrap()
            .projection
            .sequence),
        before + 1,
        "FIX4_UI_POLL_NO_DUPLICATE"
    );
}
#[test]
fn te15_b1_fix4_waiting_revise_keeps_valid_request() {
    let mut project = c7::project("te15-fix4-revise");
    let attachment = c7::launch(&project);
    project.feedback.issuer = None;
    std::fs::write(project.temp.path("scheduler/specialists.json"), b"broken").unwrap();
    with_host(&attachment, |port| {
        assert!(matches!(
            project.feedback.submit(port),
            Outcome::Submitted(_)
        ));
    });
    assert!(
        project
            .feedback
            .message
            .as_deref()
            .is_some_and(|reason| reason.contains("Settings")),
        "FIX4_REVISE_WAIT_REASON"
    );
    assert!(
        project.feedback.revise.is_none(),
        "FIX4_WAITING_NOT_UNCONFIRMED"
    );
    let batch = project.feedback.rows[0].batch.clone();
    let request = project.feedback.rows[0].request.clone();
    with_host(&attachment, |port| {
        assert_eq!(
            port.index().unwrap().batches[&batch].request,
            request,
            "FIX4_DURABLE_REVISE"
        );
    });
    let directory = project.temp.path("state");
    project.feedback = Feedback::default();
    project
        .feedback
        .open(attachment.studio_project_id().map_err(text), Ok(directory))
        .unwrap();
    with_host(&attachment, |port| {
        project.feedback.refresh(port).unwrap();
    });
    assert_eq!(
        project.feedback.rows[0].request, request,
        "FIX4_REOPEN_REVISE"
    );
    std::fs::remove_file(project.temp.path("scheduler/specialists.json")).unwrap();
    let accepted = with_host(&attachment, |port| {
        let result = project
            .feedback
            .start_batch(port, &batch)
            .expect("FIX4_REVISE_REPAIRED");
        assert!(
            matches!(result, RunStart::Known(ReviseResult::Accepted { .. })),
            "FIX4_REVISE_REPAIRED {result:?}"
        );
        result
    });
    let replay = with_host(&attachment, |port| port.revise(&request));
    assert_eq!(replay, accepted, "FIX4_REVISE_ORIGINAL_REPLAY");
    assert!(project.feedback.revise.is_none());
    assert_eq!(project.feedback.rows.len(), 1);
    assert!(
        project.feedback.rows[0].run.is_some(),
        "FIX4_REVISE_LINKED_ONCE"
    );
}

#[test]
fn te15_b1_fix4_full_notes_continue_reaches_host() {
    for width in [340.0, 1200.0] {
        fix4_full_notes_at_width(width);
    }
}
fn fix4_full_notes_at_width(width: f32) {
    let (mut project, attachment, run) = wait_fixture();
    let client = attachment.host_client().unwrap();
    let before = client
        .request(kinewright_agent::writer_host::HostCommand::ObserveRun {
            run: same_identity(&run).unwrap(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap()
        .projection
        .sequence;
    let (mut app, _engine) = in1_harness(Document::default());
    app.projects[0].recovery.stop_for_studio();
    app.projects[0].ja1 = Some(attachment);
    let mut feedback = std::mem::take(&mut project.feedback);
    feedback.directory_override = Some(project.temp.path("state"));
    app.focused_mut().studio.feedback = feedback;
    app.feedback_frame();
    let ctx = egui::Context::default();
    let draw = |ctx: &egui::Context, app: &mut crate::app::KinewrightApp, events| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 1400.0))),
                events,
                ..Default::default()
            },
            |ui| app.studio_notes(ui),
        )
    };
    draw(&ctx, &mut app, vec![]);
    let output = draw(&ctx, &mut app, vec![]);
    let pos = text_position(&output.shapes, "Continue");
    draw(
        &ctx,
        &mut app,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    draw(
        &ctx,
        &mut app,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let after = client
        .request(kinewright_agent::writer_host::HostCommand::ObserveRun {
            run: same_identity(&run).unwrap(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap();
    assert_eq!(
        after.projection.sequence,
        before + 1,
        "FIX4_UI_REAL_HANDLER"
    );
    assert!(!after.allowed_actions.contains(&RunActionKind::Continue));
    assert_eq!(
        app.focused().studio.feedback.message.as_deref(),
        Some("Continue accepted."),
        "FIX4_UI_REAL_ACCEPTED"
    );
    in1_shutdown(&mut app);
}

#[test]
fn te15_b1_followup_disabled_continue_shows_host_reason() {
    let (project, attachment, run) = wait_fixture();
    let mut page = with_host(&attachment, |port| port.run_view(&run).unwrap());
    page.allowed_actions
        .retain(|action| *action != RunActionKind::Continue);
    let why = "A task has an unknown send outcome. Continue cannot retry this send.";
    page.continue_unavailable_reason = Some(why.into());
    let state = RunState::of(&page);
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let (_, output) = frame(&ctx, &run, &state, vec![]);
    let at = text_position(&output.shapes, "Continue");
    frame(&ctx, &run, &state, vec![egui::Event::PointerMoved(at)]);
    frame(&ctx, &run, &state, vec![]);
    let (_, output) = frame(&ctx, &run, &state, vec![]);
    assert!(output.shapes.iter().any(|clipped| matches!(&clipped.shape, egui::Shape::Text(shape) if shape.galley.job.text.contains(why))), "FOLLOWUP_DISABLED_HOST_REASON");
    assert!(click(&ctx, &run, &state, at).is_none());
    assert_eq!(
        project.feedback.rows[0]
            .state
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .phase,
        RunPhase::Interrupted
    );
}

#[test]
fn te15_b1_followup_wait_row_is_plain_and_details_keep_raw() {
    let (project, attachment, run) = wait_fixture();
    let original = with_host(&attachment, |port| port.run_view(&run).unwrap());
    for (raw, plain) in [
        (
            "silent_stall: no provider is running",
            "The run has no scheduled work. Press Stop, then Reopen to review your notes before starting another run.",
        ),
        (
            "native runtime refusal: wait write failed",
            "The run could not continue. Check Run details for the reason.",
        ),
        (
            "run_limit: private machine detail",
            "This run reached its limit. Press Stop to finish cleanup.",
        ),
        (
            "unknown_machine_reason: private detail",
            "The run needs attention. Check Run details for the reason.",
        ),
    ] {
        let mut page = original.clone();
        let RunWait::ProviderSettings(detail) = page.projection.wait else {
            unreachable!()
        };
        page.projection.wait = RunWait::Provider(detail);
        page.wait_reason = Some(raw.into());
        let state = RunState::of(&page);
        assert!(state.label().contains(plain), "FOLLOWUP_PLAIN_WAIT {raw}");
        assert!(!state.label().contains(raw));
        assert_eq!(
            state.view.wait_reason.as_deref(),
            Some(raw),
            "FOLLOWUP_RAW_DETAILS_RETAINED"
        );
    }
    assert!(
        project.feedback.rows[0]
            .label()
            .contains("then press Continue.")
    );
}

#[test]
fn te15_b201_replan_status_uses_host_reason() {
    let (project, _attachment, _) = wait_fixture();
    let mut state = project.feedback.rows[0]
        .state
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    let RunWait::ProviderSettings(detail) = state.wait else {
        unreachable!()
    };
    state.wait = RunWait::Replan(detail);
    state.phase = RunPhase::Running;
    let reason = "The task could not start because its candidate changed. This start sent nothing. The Director must replan the task; Continue cannot repair it.";
    state.view.wait_reason = Some(reason.into());
    assert_eq!(
        state.label(),
        format!("run running · {reason}"),
        "B201_UI_HOST_REASON"
    );
}

#[test]
fn te15_b201_transient_status_uses_host_reason() {
    let (project, _attachment, _) = wait_fixture();
    let mut state = project.feedback.rows[0]
        .state
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    let reason = "The writer host could not start the task yet. This start sent nothing. Continue to try again.";
    assert!(matches!(state.wait, RunWait::ProviderSettings(_)));
    state.view.wait_reason = Some(reason.into());
    assert_eq!(
        state.label(),
        format!("run interrupted · {reason}"),
        "B201_UI_HOST_RETRY"
    );
}

#[test]
fn te15_b21_run_details_lists_the_approved_private_batch() {
    let (_project, attachment, run) = wait_fixture();
    let mut page = with_host(&attachment, |port| port.run_view(&run).unwrap());
    page.edit_approvals
        .push(kinewright_agent::writer_host::EditApproval {
            task: page.projection.tasks[0].id.clone(),
            removed_clips: 2,
            removed_tracks: 0,
            applied: true,
            issue: None,
            digest: "a".repeat(64),
        });
    let state = RunState::of(&page);
    assert_eq!(
        state.view.edit_approvals,
        ["Removed 2 clips in the draft version. Your project is unchanged until you choose it."],
        "B21_PLAIN_APPROVAL_DETAILS"
    );
}

#[test]
fn te15_b21f_details_distinguish_unapplied_integrity_and_summary() {
    let (_project, attachment, run) = wait_fixture();
    let mut page = with_host(&attachment, |port| port.run_view(&run).unwrap());
    let row = kinewright_agent::writer_host::EditApproval {
        task: page.projection.tasks[0].id.clone(),
        removed_clips: 2,
        removed_tracks: 0,
        applied: true,
        issue: None,
        digest: "a".repeat(64),
    };
    page.edit_approvals = vec![row.clone(); 6];
    let mut pending = row.clone();
    pending.applied = false;
    page.edit_approvals.push(pending);
    let mut bad = row;
    bad.issue = Some("bad".into());
    bad.applied = false;
    page.edit_approvals.push(bad);
    let view = RunState::of(&page).view;
    assert_eq!(view.edit_approvals.len(), 3, "FOL_COLLAPSED_DETAILS");
    assert!(view.edit_approvals[0].contains("6 edits removed 12 clips"));
    assert!(
        view.edit_approvals[1].contains("changes are not confirmed"),
        "FOL_UNAPPLIED_WORDS"
    );
    assert!(
        view.edit_approvals[2].contains("could not be verified"),
        "FOL_INTEGRITY_WORDS"
    );
    assert!(view.edit_approvals.iter().all(|s| !s.contains("approve?")));
}
