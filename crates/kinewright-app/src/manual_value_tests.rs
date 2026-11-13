//! Real App, manual permit, writer publications and cold reopen under held acknowledgements.
use super::*;
use crate::app::{
    KinewrightApp,
    in1_tests::{in1_harness, in1_shutdown},
};
use kinewright_agent::writer_host::{ProtocolFixture, ProtocolFixtureMode, SessionRoots};
use kinewright_core::*;
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::bundle::LoadedBundle;
use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

struct Fixture {
    app: KinewrightApp,
    _engine: Arc<kinewright_media::FfmpegMediaEngine>,
    temp: TempDirectory,
    ctx: egui::Context,
    key: String,
    before: Arc<Document>,
}
impl Fixture {
    fn new() -> Self {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-final-value");
        let path = temp.path("p.kinewright");
        let mut document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        };
        Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(0),
            duration: TimeCode(1_000_000),
            color: SolidColor { r: 0, g: 0, b: 0 },
        }
        .apply(&mut document)
        .unwrap();
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(1),
                name: "primary_correction".into(),
                enabled: true,
                enabled_curve: None,
                parameters: BTreeMap::from([(
                    "exposure_milli_stops".into(),
                    ParamValue::Integer(0),
                )]),
                keyframes: BTreeMap::new(),
            },
        }
        .apply(&mut document)
        .unwrap();
        let before = Arc::new(document);
        let core = Core::spawn(Arc::clone(&before)).unwrap();
        let mut owner = Ja1EditingSession::activate(
            &path,
            kinewright_project::bundle::BundleWriteAdmission::new_document(),
            &core,
            None,
        )
        .unwrap();
        owner.save_active().unwrap();
        let mut attachment = Ja1Attachment::new(owner).unwrap();
        let fixture = ProtocolFixture::new(temp.root(), ProtocolFixtureMode::Dormant);
        let writer = attachment
            .launch_protocol_fixture(
                SessionRoots {
                    read: vec![temp.root().into()],
                    write: vec![temp.root().into()],
                },
                temp.path("settings"),
                &fixture,
            )
            .unwrap();
        attachment.fixture_host = Some(writer);
        assert!(
            attachment.host.as_ref().unwrap().manual.is_some(),
            "MANUAL_REAL_LEASE"
        );
        attachment.host.as_mut().unwrap().manual_publication_hold = true;
        let (mut app, engine) = in1_harness(Document::default());
        app.focused_mut().project_path = Some(temp.path("other.kinewright"));
        assert!(app.save_project(false), "MANUAL_OTHER_PROJECT_SAVED");
        app.new_project();
        app.focused_mut().ja1 = Some(attachment);
        app.focused_mut().install_ja1_view();
        let gesture = app.begin_edit_gesture();
        let key = format!("final-exposure#{gesture}");
        Self {
            app,
            _engine: engine,
            temp,
            ctx: egui::Context::default(),
            key,
            before,
        }
    }
    fn attachment(&self) -> &Ja1Attachment {
        self.app.focused().ja1.as_ref().unwrap()
    }
    fn gate(&self) -> &ManualProducerGate {
        self.attachment()
            .host
            .as_ref()
            .unwrap()
            .manual
            .as_ref()
            .unwrap()
    }
    fn edits(&self) -> &crate::manual_edits::ManualEdits {
        &self.attachment().host.as_ref().unwrap().edits
    }
    fn edits_mut(&mut self) -> &mut crate::manual_edits::ManualEdits {
        &mut self
            .app
            .focused_mut()
            .ja1
            .as_mut()
            .unwrap()
            .host
            .as_mut()
            .unwrap()
            .edits
    }
    fn tick(&mut self) {
        self.app.poll_background(&self.ctx);
        std::thread::yield_now();
    }
    fn until(&mut self, mut condition: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(45);
        while self.app.focused().ja1.is_none() || !condition(self) {
            assert!(
                Instant::now() < deadline,
                "MANUAL_TEST_BOUND status={} message={:?}",
                self.app.status,
                self.attachment().manual_edit_message()
            );
            self.tick();
        }
    }
    fn attempt(&mut self) {
        self.tick();
        self.until(|f| !f.edits().attempt_running());
    }
    fn send_value(&mut self, value: i64) {
        self.app
            .send_operations_coalesced(vec![curve(450, value)], self.key.clone());
        self.attempt();
    }
    fn fill(&mut self) {
        // Full curve values fill the real byte window; no transport acknowledgement is allowed.
        for step in 1..30 {
            self.send_value(step);
            if self.edits().waiting_for_ack() {
                break;
            }
        }
        assert!(self.edits().waiting_for_ack(), "MANUAL_FULL_WINDOW");
        let pending = self.gate().permit.pending().unwrap();
        assert!(!pending.is_empty());
        println!(
            "MANUAL_WINDOW pending={} serialized_bytes={}",
            pending.len(),
            serde_json::to_vec(&pending).unwrap().len()
        );
    }
    fn release(&mut self) {
        self.app
            .focused_mut()
            .ja1
            .as_mut()
            .unwrap()
            .host
            .as_mut()
            .unwrap()
            .manual_publication_hold = false;
    }
    fn value(&self) -> i64 {
        self.app.focused().document.clip(ClipId(1)).unwrap().effects[0].keyframes["exposure_milli_stops"].keyframes[0].value
    }
    fn scalar(&self) -> i64 {
        let ParamValue::Integer(value) = self
            .app
            .focused()
            .document
            .clip(ClipId(1))
            .unwrap()
            .effects[0]
            .parameters["exposure_milli_stops"]
        else {
            panic!("integer");
        };
        value
    }
    fn finish(&mut self) {
        self.app
            .focused_mut()
            .ja1
            .as_mut()
            .unwrap()
            .end_manual_gestures();
        self.release();
        self.until(|f| !f.attachment().manual_edits_pending() && !f.attachment().is_dirty());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        in1_shutdown(&mut self.app);
    }
}
fn param(value: i64) -> Operation {
    Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(1),
        name: "exposure_milli_stops".into(),
        value: ParamValue::Integer(value),
    }
}
fn curve(count: i64, value: i64) -> Operation {
    Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(1),
        name: "exposure_milli_stops".into(),
        curve: AutomationCurve {
            keyframes: (0..count)
                .map(|at| Keyframe {
                    at: TimeCode(at),
                    value,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect(),
        },
    }
}

#[test]
fn manual_value_final_drag_is_saved_and_one_undo() {
    let mut f = Fixture::new();
    let reservation = crate::media_workflow::LutImportReservation {
        session_id: f.app.focused().id,
        asset: LutAssetId(4_000),
    };
    f.app.lut_import_reservation = Some(reservation);
    f.fill();
    assert_eq!(
        f.app.lut_import_reservation,
        Some(reservation),
        "MANUAL_RETRY_RESERVATION"
    );
    f.app.lut_import_reservation = None;
    let accepted = f.value();
    assert_ne!(accepted, -555);
    for value in [-111, -222, -555] {
        f.send_value(value);
        assert_eq!(f.edits().action_count(), 1, "MANUAL_LATEST_ONLY");
    }
    f.app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .end_manual_gestures();
    f.finish();
    assert_eq!(f.value(), -555, "MANUAL_FINAL_VALUE_LANDS");
    let final_document = Arc::clone(&f.app.focused().document);
    f.app.undo();
    f.finish();
    assert_eq!(*f.app.focused().document, *f.before, "MANUAL_ONE_UNDO");
    f.app.redo();
    f.finish();
    assert_eq!(*f.app.focused().document, *final_document);
    let path = f.temp.path("p.kinewright");
    f.app.focused_mut().ja1.as_mut().unwrap().close();
    f.app.focused_mut().ja1 = None;
    let loaded = LoadedBundle::read(&path).unwrap();
    let cold = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&cold).unwrap();
    assert_eq!(
        *cold.export_history().unwrap().document,
        *final_document,
        "MANUAL_FINAL_COLD_VALUE"
    );
}
#[test]
fn manual_value_final_capacity_refusal_is_truthful() {
    let mut f = Fixture::new();
    f.fill();
    let accepted = Arc::clone(&f.app.focused().document);
    let events = f.app.focused().core.subscribe().unwrap();
    f.app
        .send_operations_coalesced(vec![curve(150_000, -555)], f.key.clone());
    f.attempt();
    // Acknowledge the full window, so this final value reaches its own history predicate.
    f.release();
    f.until(|f| !f.edits().attempt_running() && f.gate().permit.pending().unwrap().is_empty());
    f.attempt();
    let reasons = events
        .try_iter()
        .filter_map(|event| match event {
            Event::MutationRefused { reason, .. } => Some(reason),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        reasons.contains(&CoreMutationRefusal::ManualHistoryCapacity),
        "MANUAL_SPECIFIC_FINAL_CAPACITY {reasons:?}"
    );
    assert!(
        !f.attachment().manual_edits_pending(),
        "MANUAL_FINAL_REFUSAL_DROPS_VALUE"
    );
    assert_eq!(
        f.app.status,
        CoreMutationRefusal::ManualHistoryCapacity.to_string(),
        "MANUAL_FINAL_OWN_WORDS"
    );
    assert_eq!(
        *f.app.focused().document,
        *accepted,
        "MANUAL_REFUSED_SCREEN_VALUE"
    );
    assert_eq!(
        *f.app.focused().core.export_history().unwrap().document,
        *accepted
    );
}
#[derive(Default)]
struct SendGate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
impl SendGate {
    fn hold(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.changed.notify_all();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(180), |s| !s.1)
            .unwrap();
        assert!(state.1, "MANUAL_SEND_GATE_BOUND");
    }
    fn entered(&self) -> bool {
        self.state.lock().unwrap().0
    }
    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
}
struct Release(Arc<SendGate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
#[test]
fn manual_value_close_and_quit_ask_for_unapplied_value() {
    let mut f = Fixture::new();
    f.fill();
    f.send_value(-555);
    let gate = Arc::new(SendGate::default());
    let _release = Release(Arc::clone(&gate));
    let held = Arc::clone(&gate);
    f.edits_mut().before_send = Some(Box::new(move || held.hold()));
    f.release();
    f.until(|f| gate.entered() && f.gate().permit.pending().unwrap().is_empty());
    assert!(
        f.attachment()
            .host
            .as_ref()
            .unwrap()
            .status
            .saved_digest
            .is_some()
    );
    let id = f.app.focused().id;
    let count = f.app.projects.len();
    f.app.request_close_project(f.app.focused_project);
    assert_eq!(
        f.app.projects.len(),
        count,
        "MANUAL_CLOSE_KEEPS_PENDING_PROJECT"
    );
    assert!(f.app.projects.iter().any(|p| p.id == id));
    assert!(
        f.attachment()
            .close_text()
            .contains("final value is still waiting"),
        "MANUAL_PENDING_CLOSE_WORDS"
    );
    f.app.request_exit(&f.ctx);
    assert!(f.app.exit_waits_for_choice(), "MANUAL_QUIT_ASKS");
    assert!(!f.app.exit_allowed());
    gate.release();
    f.finish();
    assert_eq!(f.value(), -555, "MANUAL_AFTER_CANCELLED_CLOSE_LANDS");
}
#[test]
fn manual_value_discrete_edits_are_ordered_and_bounded() {
    let mut f = Fixture::new();
    f.fill();
    // End the drag before the discrete actions; it remains the first pending action.
    f.app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .end_manual_gestures();
    for value in 1..crate::manual_edits::PENDING_ACTIONS {
        f.app
            .focused_mut()
            .send_manual_command(Command::Do(param(-i64::try_from(value).unwrap())))
            .unwrap();
    }
    let error = f
        .app
        .focused_mut()
        .send_manual_command(Command::Do(param(-999)))
        .unwrap_err();
    assert_eq!(
        error,
        crate::manual_edits::ManualSendError::PendingCapacity,
        "MANUAL_BOUNDED_REFUSAL"
    );
    assert_eq!(
        f.edits().action_count(),
        crate::manual_edits::PENDING_ACTIONS
    );
    assert!(
        f.attachment()
            .manual_edit_message()
            .unwrap()
            .contains("Too many edits")
    );
    f.finish();
    assert_eq!(f.scalar(), -15, "MANUAL_DISCRETE_FINAL_ORDER");
    let history = f.app.focused().core.export_history().unwrap();
    let values = history
        .batches
        .iter()
        .rev()
        .take(15)
        .rev()
        .map(|b| match &b.command {
            JournalCommand::Do(Operation::SetEffectParam {
                value: ParamValue::Integer(v),
                ..
            }) => *v,
            other => panic!("{other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        (1..=15).map(|v| -v).collect::<Vec<_>>(),
        "MANUAL_DISCRETE_ALL_KEPT"
    );
}

#[test]
fn manual_value_release_frame_submits_before_finishing_gesture() {
    let mut f = Fixture::new();
    let pos = egui::pos2(40.0, 40.0);
    let _ = f.ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |_| {},
    );
    assert!(f.ctx.input(|i| i.pointer.any_down()));
    f.send_value(10);
    f.release();
    f.until(|f| f.gate().permit.pending().unwrap().is_empty() && !f.edits().attempt_running());
    assert!(
        f.attachment().manual_edits_pending(),
        "MANUAL_GESTURE_STILL_OPEN"
    );
    let _ = f.ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |_| {},
    );
    assert!(!f.ctx.input(|i| i.pointer.any_down()));
    // Background work precedes widgets on the release frame. It must not hand
    // the attachment to preview preparation before the widget submits its last value.
    f.app.poll_background(&f.ctx);
    assert!(
        f.app.focused().ja1.is_some(),
        "MANUAL_RELEASE_BEFORE_WIDGETS"
    );
    f.app
        .send_operations_coalesced(vec![curve(450, -555)], f.key.clone());
    f.app.finish_manual_frame(&f.ctx);
    f.finish();
    assert_eq!(f.value(), -555, "MANUAL_RELEASE_FINAL_VALUE");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one ordered real-host refusal, recovery and cold-copy witness"
)]
fn manual_value_fix1_paused_recovery_keeps_retry_and_accepted_edits() {
    let mut f = Fixture::new();
    f.fill();
    f.send_value(-555);
    let accepted = Arc::clone(&f.app.focused().document);
    assert_ne!(f.value(), -555);
    assert!(!f.gate().permit.pending().unwrap().is_empty());
    {
        let saved = Core::spawn(Document::default()).unwrap();
        LoadedBundle::read(&f.temp.path("p.kinewright"))
            .unwrap()
            .restore_manual(&saved)
            .unwrap();
        assert_ne!(
            *saved.export_history().unwrap().document,
            *accepted,
            "MANUAL_RECOVERY_PREFIX_WAS_NOT_SAVED"
        );
    }
    let proxy = f
        .app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .host
        .as_mut()
        .unwrap()
        .drop_http_replies(vec!["commit_history", "publish_barrier"], true);
    f.release();
    f.until(|f| {
        proxy.lost.lock().unwrap().len() == 2
            && f.attachment().read_only()
            && !f.attachment().host.as_ref().unwrap().recovery_running()
    });
    assert!(proxy.dropped.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(
        proxy.lost.lock().unwrap().as_slice(),
        &["commit_history", "publish_barrier"]
    );
    let host = f.attachment().host.as_ref().unwrap();
    let lease_probe = host
        .client
        .request(kinewright_agent::writer_host::HostCommand::CommitHistory {
            client: host.manual_identity_for_test(),
            messages: vec![],
        })
        .unwrap();
    assert_eq!(
        lease_probe.error.as_ref().map(|error| error.code.as_str()),
        Some("recovery_frozen"),
        "MANUAL_RECOVERY_REFUSES_OLD_LEASE_STREAM"
    );
    assert!(
        !f.attachment().manual_edits_pending(),
        "MANUAL_RECOVERY_DROPS_UNACCEPTED"
    );
    assert!(
        f.attachment()
            .manual_edit_message()
            .unwrap()
            .contains("1 hand edit was not applied."),
        "MANUAL_RECOVERY_NOTICE_BEFORE_RETRY"
    );
    assert!(
        f.app.status.contains("host reply was not confirmed"),
        "MANUAL_NOTICE_KEEPS_HOST_ERROR"
    );
    // The pending-value gate must not hide rescue. The pre-existing unresolved-barrier
    // guard may still refuse it; Retry must first finish that authenticated handoff.
    f.app.start_ja1(Action::PreviewRescue);
    assert!(
        f.app.focused().ja1_job.is_some(),
        "MANUAL_RECOVERY_RESCUE_IS_AVAILABLE"
    );
    f.until(|f| f.app.focused().ja1_job.is_none());
    assert_ne!(f.attachment().message, crate::manual_edits::PENDING_WORDS);
    let preview = Arc::new(SendGate::default());
    let _release = Release(Arc::clone(&preview));
    let held = Arc::clone(&preview);
    f.app.focused_mut().studio.manual_inputs = None;
    f.app.focused_mut().studio.failure = None;
    f.app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .before_manual_playback = Some(Box::new(move || held.hold()));
    f.app.start_ja1(Action::Retry);
    assert!(
        f.app.focused().ja1_job.is_some(),
        "MANUAL_RECOVERY_RETRY_IS_AVAILABLE"
    );
    let deadline = Instant::now() + Duration::from_secs(45);
    while !preview.entered() {
        assert!(
            Instant::now() < deadline,
            "MANUAL_RECOVERY_PREVIEW_ENTERED {}",
            f.app.status
        );
        f.tick();
    }
    assert!(f.app.focused().ja1.is_none() && f.app.focused().ja1_job.is_some());
    assert!(
        f.app.status.contains("1 hand edit was not applied."),
        "MANUAL_NOTICE_DURING_PREVIEW {}",
        f.app.status
    );
    preview.release();
    f.until(|f| !f.attachment().read_only());
    assert_eq!(
        *f.app.focused().document,
        *accepted,
        "MANUAL_RECOVERY_KEEPS_ACCEPTED"
    );
    let notice = f.attachment().manual_edit_message().unwrap();
    assert!(
        notice.contains("1 hand edit was not applied."),
        "MANUAL_RECOVERY_NOTICE_SURVIVES {notice}"
    );
    let recovered = f.temp.path("recovered.kinewright");
    f.app.start_ja1(Action::SaveAs(recovered.clone()));
    f.until(|f| {
        f.app.focused().ja1_job.is_none()
            && f.attachment().project_path().as_ref() == Some(&recovered)
    });
    let cold = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(&recovered)
        .unwrap()
        .restore_manual(&cold)
        .unwrap();
    assert_eq!(
        *cold.export_history().unwrap().document,
        *accepted,
        "MANUAL_RECOVERY_COPY_KEEPS_ACCEPTED"
    );
    assert!(
        f.attachment()
            .manual_edit_message()
            .unwrap()
            .contains("1 hand edit was not applied.")
    );
}

#[test]
fn manual_value_fix1_acknowledgement_during_attempt_is_not_lost() {
    let mut f = Fixture::new();
    f.send_value(10);
    let gate = Arc::new(SendGate::default());
    let _release = Release(Arc::clone(&gate));
    let held = Arc::clone(&gate);
    f.edits_mut().after_send = Some(Box::new(move || held.hold()));
    f.app
        .send_operations_coalesced(vec![curve(450, -555)], f.key.clone());
    f.until(|_| gate.entered());
    // Exercise the queue's correlation contract separately from the poll exclusion:
    // the actual publication is acknowledged after the refused send, before its result is collected.
    let attachment = f.app.focused_mut().ja1.as_mut().unwrap();
    let host = attachment.host.as_mut().unwrap();
    let messages = host.manual.as_ref().unwrap().permit.pending().unwrap();
    assert!(!messages.is_empty());
    let reply = host
        .client
        .request(kinewright_agent::writer_host::HostCommand::CommitHistory {
            client: host.manual_identity_for_test(),
            messages,
        })
        .unwrap();
    assert!(reply.error.is_none(), "{reply:?}");
    host.accept_manual_ack(&reply, &attachment.owner).unwrap();
    gate.release();
    f.until(|f| !f.edits().attempt_running());
    assert!(
        !f.edits().waiting_for_ack(),
        "MANUAL_SEND_TIME_ACKNOWLEDGEMENT"
    );
    f.finish();
    assert_eq!(f.value(), -555);
}

#[test]
fn manual_value_fix1_poll_keeps_publication_behind_attempt() {
    let mut f = Fixture::new();
    f.send_value(10);
    let gate = Arc::new(SendGate::default());
    let _release = Release(Arc::clone(&gate));
    let held = Arc::clone(&gate);
    f.edits_mut().after_send = Some(Box::new(move || held.hold()));
    f.app
        .send_operations_coalesced(vec![curve(450, -555)], f.key.clone());
    f.until(|_| gate.entered());
    f.release();
    f.tick();
    assert!(
        !f.attachment()
            .host
            .as_ref()
            .unwrap()
            .manual_stream_inflight_for_test(),
        "MANUAL_POLL_WAIT_FOR_ATTEMPT"
    );
    assert!(!f.gate().permit.pending().unwrap().is_empty());
    gate.release();
    f.finish();
    assert_eq!(f.value(), -555);
}

#[test]
fn manual_value_fix1_revision_conflict_notice_survives_later_acceptance() {
    let mut f = Fixture::new();
    let expected = f.app.focused().revision;
    f.app
        .focused_mut()
        .send_manual_command(Command::Do(param(1)))
        .unwrap();
    f.attempt();
    f.app
        .focused_mut()
        .send_manual_command(Command::DoIfRevision {
            expected,
            operation: param(-10),
            token: None,
        })
        .unwrap();
    f.app
        .focused_mut()
        .send_manual_command(Command::Do(param(20)))
        .unwrap();
    f.until(|f| !f.attachment().manual_edits_pending());
    assert_eq!(f.scalar(), 20);
    let words = "The document changed before this edit could be applied. Check its current value and try again.";
    assert_eq!(
        f.attachment().manual_edit_message(),
        Some(words),
        "MANUAL_REVISION_CONFLICT_NOTICE"
    );
    f.finish();
    f.tick();
    assert_eq!(
        f.app.status, words,
        "MANUAL_NOTICE_SURVIVES_ACCEPTANCE_AND_SAVE"
    );
    f.app.start_ja1(Action::DismissManualNotice);
    assert!(f.edits().notice().is_none());
    assert_ne!(f.app.status, words, "MANUAL_DISMISS_REMOVES_NOTICE");
    f.app
        .focused_mut()
        .send_manual_command(Command::DoIfRevision {
            expected,
            operation: param(-30),
            token: None,
        })
        .unwrap();
    f.until(|f| !f.attachment().manual_edits_pending());
    assert_eq!(f.edits().notice(), Some(words));
    f.app
        .focused_mut()
        .send_manual_command(Command::Do(param(40)))
        .unwrap();
    assert!(
        f.edits().notice().is_none(),
        "MANUAL_NEXT_PERSON_EDIT_DISMISSES_NOTICE"
    );
    f.finish();
    assert_eq!(f.scalar(), 40);
}

#[test]
fn manual_value_fix1_open_gesture_does_not_claim_backpressure() {
    let mut f = Fixture::new();
    f.send_value(10);
    assert!(f.attachment().manual_edits_pending());
    assert_eq!(
        f.attachment().manual_edit_message(),
        None,
        "MANUAL_OPEN_GESTURE_IS_NOT_WAITING"
    );
    f.send_value(-555);
    assert!(f.edits().waiting_for_ack());
    assert_eq!(
        f.attachment().manual_edit_message(),
        Some(crate::manual_edits::PENDING_WORDS)
    );
    let close = f.attachment().close_text();
    assert!(
        close.contains("Cancel keeps this project open")
            && close.contains("Discard closes the project")
    );
    assert!(
        close.contains("waiting for a durable acknowledgement"),
        "MANUAL_CLOSE_DISCLOSES_BOTH"
    );
    f.finish();
}

#[test]
fn manual_value_fix1_waiting_value_does_not_hide_a_live_host_refusal() {
    let mut f = Fixture::new();
    f.fill();
    let attachment = f.app.focused_mut().ja1.as_mut().unwrap();
    let host = attachment.host.as_mut().unwrap();
    let messages = host.manual.as_ref().unwrap().permit.pending().unwrap();
    let reply = host
        .client
        .request(kinewright_agent::writer_host::HostCommand::CommitHistory {
            client: host.manual_identity_for_test(),
            messages,
        })
        .unwrap();
    assert!(reply.error.is_none(), "{reply:?}");
    host.accept_manual_ack(&reply, &attachment.owner).unwrap();
    // This refusal is final for that session command, but the working editor is healthy.
    let absent = kinewright_project::bundle_records::BranchId::fresh().unwrap();
    let Err(BundleError::Identity(error)) = attachment.apply(Action::SaveSession(absent)) else {
        panic!("MANUAL_TYPED_SESSION_REFUSAL");
    };
    assert!(error.contains("history_changed"), "{error}");
    assert!(
        !attachment.read_only(),
        "MANUAL_HOST_REFUSAL_IS_NOT_A_FREEZE"
    );
    assert!(attachment.manual_edits_pending());
    assert!(
        attachment.manual_edit_message().is_none(),
        "MANUAL_WAIT_DOES_NOT_HIDE_HOST_ERROR"
    );
    f.tick();
    assert_eq!(
        f.app.status,
        f.attachment().message,
        "MANUAL_HOST_ERROR_STATUS_PRIORITY"
    );
    f.finish();
}

#[test]
fn manual_value_fix1_project_switch_keeps_pending_value_with_its_project() {
    let mut f = Fixture::new();
    f.fill();
    f.send_value(-555);
    let target = f.app.focused_project;
    let other = Arc::clone(&f.app.projects[0].document);
    f.app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .end_manual_gestures();
    f.release();
    f.app.focus_project(0);
    let deadline = Instant::now() + Duration::from_secs(45);
    while f.app.projects[target]
        .ja1
        .as_ref()
        .is_none_or(|a| a.manual_edits_pending() || a.is_dirty())
    {
        assert!(Instant::now() < deadline, "MANUAL_BACKGROUND_PROJECT_LANDS");
        f.tick();
    }
    assert_eq!(f.app.focused_project, 0);
    assert_eq!(
        *f.app.projects[0].document, *other,
        "MANUAL_OTHER_PROJECT_UNCHANGED"
    );
    f.app.focus_project(target);
    assert_eq!(f.value(), -555, "MANUAL_PENDING_VALUE_STAYS_WITH_PROJECT");
}

#[test]
fn manual_value_fix1_recovery_owns_late_results_and_counts_only_refusals() {
    for (refused, before_send) in [(false, false), (true, false), (false, true)] {
        let mut f = Fixture::new();
        let first = if refused { curve(450, 10) } else { param(10) };
        f.app.send_operations_coalesced(vec![first], f.key.clone());
        f.attempt();
        let gate = Arc::new(SendGate::default());
        let _release = Release(Arc::clone(&gate));
        let held = Arc::clone(&gate);
        if before_send {
            f.edits_mut().before_send = Some(Box::new(move || held.hold()));
        } else {
            f.edits_mut().after_send = Some(Box::new(move || held.hold()));
        }
        let next = if refused { curve(450, 20) } else { param(20) };
        f.app.send_operations_coalesced(vec![next], f.key.clone());
        f.until(|_| gate.entered());
        if before_send {
            // Two distinct click commands wait behind the in-flight gesture value.
            for value in [30, 40] {
                f.app
                    .focused_mut()
                    .send_manual_command(Command::Do(param(value)))
                    .unwrap();
            }
        }
        let accepted = f.app.focused().core.export_history().unwrap().document;
        let attachment = f.app.focused_mut().ja1.as_mut().unwrap();
        attachment
            .host
            .as_mut()
            .unwrap()
            .begin_manual_recovery(&mut attachment.owner)
            .unwrap();
        assert!(
            !attachment.manual_edits_pending(),
            "MANUAL_LATE_RESULT_IS_NOT_A_RECOVERY_LOCK"
        );
        f.release();
        f.until(|f| !f.attachment().read_only());
        assert!(f.edits().worker_owned(), "MANUAL_LATE_WORKER_STILL_OWNED");
        assert_eq!(
            *f.app.focused().document,
            *accepted,
            "MANUAL_LATE_ACCEPTED_PREFIX_KEPT"
        );
        gate.release();
        f.until(|f| !f.edits().worker_owned());
        assert_eq!(
            *f.app.focused().document,
            *accepted,
            "MANUAL_OLD_PERMIT_CANNOT_APPLY_AFTER_RECOVERY"
        );
        if refused || before_send {
            let words = if before_send {
                "3 hand edits were not applied."
            } else {
                "1 hand edit was not applied."
            };
            assert!(
                f.edits().notice().unwrap().contains(words),
                "MANUAL_UNAPPLIED_COUNT_INCLUDES_LATE_REFUSAL"
            );
        } else {
            assert!(
                f.edits().notice().is_none(),
                "MANUAL_ACCEPTED_IS_NOT_REPORTED_UNAPPLIED"
            );
        }
        f.finish();
    }
}
