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
    assert!(state.label().contains(reason), "FIX4_UI_WAIT_REASON");
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
        "The send outcome is unknown. Stop this run and use recovery; Continue cannot retry it."
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
