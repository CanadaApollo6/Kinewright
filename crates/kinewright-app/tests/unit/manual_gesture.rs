//! Actual egui widgets and inspector builders feeding the real Core manual door.
use super::*;
use crate::manual_edits::{
    ManualEdits,
    tests::{Proof, ack, add, collect, fixture_with_document},
};
use kinewright_core::{
    Command, Core, CoreAttachment, Event, ManualPermit, Track, TrackId, TrackKind,
};

struct WheelProbe {
    core: Core,
    owner: CoreAttachment<Proof>,
    permit: ManualPermit,
    edits: ManualEdits,
    document: Arc<Document>,
    revision: kinewright_core::TimelineRevision,
    ctx: egui::Context,
    gesture: u64,
}
impl WheelProbe {
    fn new() -> Self {
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
            color: kinewright_core::SolidColor { r: 0, g: 0, b: 0 },
        }
        .apply(&mut document)
        .unwrap();
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(1),
                name: "color_wheels".into(),
                enabled: true,
                enabled_curve: None,
                parameters: std::collections::BTreeMap::default(),
                keyframes: std::collections::BTreeMap::default(),
            },
        }
        .apply(&mut document)
        .unwrap();
        let (core, owner, permit, edits) = fixture_with_document(document);
        // Exactly fill the real byte window with unrelated accepted work.
        let Event::DocumentChanged {
            doc: document,
            revision,
            ..
        } = permit.request(add(2)).unwrap()
        else {
            panic!("accepted unrelated prefix")
        };
        Self {
            core,
            owner,
            permit,
            edits,
            document,
            revision,
            ctx: egui::Context::default(),
            gesture: 0,
        }
    }
    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        let mut pending = InspectorEdits::default();
        let effect = &self.document.clip(ClipId(1)).unwrap().effects[0];
        let output = self.ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                let state = wheel_state(
                    effect,
                    ColorWheelsParams::from_effect(effect),
                    ColorWheelControl::Lift,
                );
                let response = color_wheel(ui, &state);
                apply_wheel_response(
                    ClipId(1),
                    effect,
                    ColorWheelControl::Lift,
                    &response,
                    &mut pending,
                );
            },
        );
        if pending.gesture_started {
            self.edits.end_gestures();
            self.gesture += 1;
        }
        if !pending.operations.is_empty() {
            let key = pending.coalesce_key.expect("live wheel gesture");
            self.edits
                .submit_value(
                    Command::DoBatchCoalesced {
                        operations: pending.operations,
                        coalesce_key: format!("{key}#{}", self.gesture),
                    },
                    pending.matches_document.then_some(self.revision),
                    &self.permit,
                )
                .unwrap();
        }
        if !self.ctx.input(|input| input.pointer.any_down()) {
            self.edits.end_gestures();
        }
        self.edits.start_next(&self.permit);
        output
    }
    fn centre(&mut self) -> egui::Pos2 {
        fn circle(shape: &egui::Shape) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Circle(c) if (c.radius - 51.0).abs() < 0.01 => Some(c.center),
                egui::Shape::Vec(v) => v.iter().find_map(circle),
                _ => None,
            }
        }
        self.frame(vec![])
            .shapes
            .iter()
            .find_map(|s| circle(&s.shape))
            .expect("actual painted wheel")
    }
    fn acknowledge(&mut self) {
        ack(&self.owner, &self.permit, &mut self.edits);
    }
    fn drain(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.edits.pending() {
            assert!(std::time::Instant::now() < deadline, "WIDGET_QUEUE_DRAINS");
            self.edits.collect_ready();
            self.acknowledge();
            self.edits.start_next(&self.permit);
            std::thread::yield_now();
        }
        self.acknowledge();
    }
    fn values(&self) -> [i64; 3] {
        let document = self.core.export_history().unwrap().document;
        let effect = &document.clip(ClipId(1)).unwrap().effects[0];
        let params = ColorWheelsParams::from_effect(effect);
        [
            ColorWheelChannel::Red,
            ColorWheelChannel::Green,
            ColorWheelChannel::Blue,
        ]
        .map(|channel| params.control(ColorWheelControl::Lift, channel))
    }
}
fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn manual_widget_return_queued_has_no_edit_or_undo() {
    let mut h = WheelProbe::new();
    let before = h.core.export_history().unwrap();
    let centre = h.centre();
    h.frame(vec![
        egui::Event::PointerMoved(centre),
        button(centre, true),
    ]);
    h.frame(vec![egui::Event::PointerMoved(
        centre + egui::vec2(15.0, -20.0),
    )]);
    collect(&mut h.edits);
    assert!(h.edits.waiting_for_ack(), "real manual window is full");
    // This newer away value remains queued: no second attempt without an acknowledgement.
    h.frame(vec![egui::Event::PointerMoved(
        centre + egui::vec2(20.0, -25.0),
    )]);
    assert!(!h.edits.attempt_running());
    h.frame(vec![egui::Event::PointerMoved(centre)]);
    h.frame(vec![button(centre, false)]);
    let pending_before_ack = h.edits.pending();
    h.drain();
    assert_eq!(h.values(), [0, 0, 0], "WIDGET_RETURN_QUEUED_FINAL");
    assert!(
        !pending_before_ack,
        "WIDGET_RETURN_QUEUED_CLEARS_BEFORE_ACK"
    );
    let after = h.core.export_history().unwrap();
    assert_eq!(
        after.cursor.undo.len(),
        before.cursor.undo.len(),
        "WIDGET_RETURN_QUEUED_NO_UNDO"
    );
    assert_eq!(
        *after.document, *before.document,
        "WIDGET_RETURN_QUEUED_DOCUMENT_UNCHANGED"
    );
}

struct HeldSend(Option<std::sync::mpsc::Sender<()>>);
impl HeldSend {
    fn install(edits: &mut ManualEdits) -> (Self, std::sync::mpsc::Receiver<()>) {
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        edits.before_send = Some(Box::new(move || {
            entered.send(()).unwrap();
            held.recv_timeout(std::time::Duration::from_secs(120))
                .expect("bounded widget send gate");
        }));
        (Self(Some(release)), observed)
    }
    fn release(mut self) {
        self.0.take().unwrap().send(()).unwrap();
    }
}
impl Drop for HeldSend {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}
impl WheelProbe {
    fn press(&mut self, at: egui::Pos2) {
        self.frame(vec![egui::Event::PointerMoved(at), button(at, true)]);
    }
    fn move_to(&mut self, at: egui::Pos2) {
        self.frame(vec![egui::Event::PointerMoved(at)]);
    }
    fn release(&mut self, at: egui::Pos2) {
        self.frame(vec![button(at, false)]);
    }
    fn refresh(&mut self) {
        self.document = self.core.export_history().unwrap().document;
        self.revision = self.core.history_change_id().unwrap().revision();
    }
    fn assert_one_undo(&mut self, before: &kinewright_core::HistorySnapshot) {
        let after = self.core.export_history().unwrap();
        assert_eq!(
            after.cursor.undo.len(),
            before.cursor.undo.len() + 1,
            "WIDGET_RETURN_ONE_UNDO"
        );
        self.acknowledge();
        assert!(matches!(
            self.permit.request(Command::Undo).unwrap(),
            Event::DocumentChanged { .. }
        ));
        assert_eq!(
            *self.core.export_history().unwrap().document,
            *before.document,
            "WIDGET_RETURN_UNDO_RESTORES_START"
        );
    }
}

#[test]
fn manual_widget_return_in_flight_follows_the_accepted_excursion() {
    let mut h = WheelProbe::new();
    let before = h.core.export_history().unwrap();
    h.acknowledge();
    let centre = h.centre();
    h.press(centre);
    let (held, entered) = HeldSend::install(&mut h.edits);
    h.move_to(centre + egui::vec2(15.0, -20.0));
    entered
        .recv_timeout(std::time::Duration::from_secs(120))
        .unwrap();
    assert_eq!(h.values(), [0, 0, 0], "older command is not accepted yet");
    h.move_to(centre);
    h.release(centre);
    assert!(h.edits.pending());
    held.release();
    h.drain();
    assert_eq!(h.values(), [0, 0, 0], "WIDGET_RETURN_IN_FLIGHT_FINAL");
    assert!(!h.edits.pending());
    h.assert_one_undo(&before);
}

#[test]
fn manual_widget_return_after_acceptance_is_one_gesture() {
    let mut h = WheelProbe::new();
    let before = h.core.export_history().unwrap();
    h.acknowledge();
    let centre = h.centre();
    h.press(centre);
    h.move_to(centre + egui::vec2(15.0, -20.0));
    collect(&mut h.edits);
    assert_ne!(h.values(), [0, 0, 0]);
    h.acknowledge();
    h.refresh();
    h.move_to(centre);
    h.release(centre);
    h.drain();
    assert_eq!(h.values(), [0, 0, 0], "WIDGET_RETURN_ACCEPTED_FINAL");
    h.assert_one_undo(&before);
}

#[test]
fn manual_widget_partial_return_keeps_every_owned_channel() {
    let mut h = WheelProbe::new();
    let before = h.core.export_history().unwrap();
    h.acknowledge();
    let centre = h.centre();
    h.press(centre);
    let (held, entered) = HeldSend::install(&mut h.edits);
    h.move_to(centre + egui::vec2(15.0, -20.0));
    entered
        .recv_timeout(std::time::Duration::from_secs(120))
        .unwrap();
    // The wheel's horizontal axis returns red to zero, retaining nonzero G/B.
    let last = centre + egui::vec2(15.0, 0.0);
    h.move_to(last);
    h.release(last);
    held.release();
    h.drain();
    let [r, g, b] = h.values();
    assert_eq!(r, 0, "WIDGET_PARTIAL_RETURN_ZERO_CHANNEL");
    assert_ne!(g, 0);
    assert_ne!(b, 0);
    assert_eq!(g, -b);
    h.assert_one_undo(&before);
}

#[test]
fn native_slider_return_and_relative_release_preserve_intent() {
    let ctx = egui::Context::default();
    let frame = |events: Vec<egui::Event>, show_value: bool| {
        let mut value = 0i64;
        let mut result = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                result = Some(ui.add(intent_slider(&mut value, -100..=100, |s| {
                    s.show_value(show_value).smart_aim(false)
                })));
            },
        );
        (value, result.unwrap())
    };
    let (_, response) = frame(vec![], false);
    let centre = response.rect.center();
    frame(
        vec![egui::Event::PointerMoved(centre), button(centre, true)],
        false,
    );
    let (value, away) = frame(
        vec![egui::Event::PointerMoved(centre + egui::vec2(20.0, 0.0))],
        false,
    );
    assert_ne!(value, 0);
    assert!(away.changed());
    // The final sample arrives on release and equals accepted readback.
    let (value, returned) = frame(
        vec![egui::Event::PointerMoved(centre), button(centre, false)],
        false,
    );
    assert_eq!(value, 0);
    assert!(returned.changed(), "SLIDER_EQUAL_RAIL_RELEASE_IS_INTENT");
    let (_, response) = frame(vec![], true);
    let readout = egui::pos2(response.rect.max.x - 12.0, response.rect.center().y);
    frame(
        vec![egui::Event::PointerMoved(readout), button(readout, true)],
        true,
    );
    let (value, away) = frame(
        vec![egui::Event::PointerMoved(readout + egui::vec2(20.0, 0.0))],
        true,
    );
    assert_ne!(value, 0);
    assert!(away.changed());
    let (_, release) = frame(vec![button(readout + egui::vec2(20.0, 0.0), false)], true);
    assert!(
        !release.changed(),
        "RELATIVE_RELEASE_IS_READBACK_NOT_NEW_INTENT"
    );
}

#[test]
fn curve_widget_emits_its_complete_return_to_the_document() {
    fn dot(shape: &egui::Shape) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Circle(circle)
                if (circle.radius - curve_editor_widget::POINT_RADIUS).abs() < 0.01
                    && circle.center.x > 100.0
                    && circle.center.x < 160.0 =>
            {
                Some(circle.center)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(dot),
            _ => None,
        }
    }
    let ctx = egui::Context::default();
    let points = [(0, 0), (5_000, 5_000), (10_000, 10_000)];
    let frame = |events: Vec<egui::Event>| {
        let mut result = None;
        let output = ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                result = Some(curve_editor_widget::curve_editor(
                    ui,
                    &points,
                    ColorCurveChannel::Master,
                    egui::Id::new("return-curve"),
                ));
            },
        );
        (result.unwrap(), output)
    };
    let (_, output) = frame(vec![]);
    let start = output
        .shapes
        .iter()
        .find_map(|shape| dot(&shape.shape))
        .unwrap();
    frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
    frame(vec![egui::Event::PointerMoved(
        start + egui::vec2(7.0, 0.0),
    )]);
    let (away, _) = frame(vec![egui::Event::PointerMoved(
        start + egui::vec2(20.0, 0.0),
    )]);
    assert_ne!(away.points.as_deref(), Some(points.as_slice()));
    assert!(away.points.is_some());
    let (returned, _) = frame(vec![egui::Event::PointerMoved(start), button(start, false)]);
    assert_eq!(
        returned.points.as_deref(),
        Some(points.as_slice()),
        "CURVE_EQUAL_RETURN_IS_INTENT"
    );
    assert!(returned.live);
}

#[test]
fn solid_picker_input_is_the_pending_colour_until_it_is_replaced_or_abandoned() {
    fn contains(shape: &egui::Shape, desired: kinewright_core::SolidColor) -> bool {
        match shape {
            egui::Shape::Rect(rect) => {
                rect.fill == egui::Color32::from_rgb(desired.r, desired.g, desired.b)
            }
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, desired)),
            _ => false,
        }
    }
    let mut h = WheelProbe::new();
    let desired = kinewright_core::SolidColor {
        r: 120,
        g: 60,
        b: 30,
    };
    h.edits
        .submit(
            Command::DoBatchCoalesced {
                operations: vec![Operation::SetSolidColor {
                    clip: ClipId(1),
                    color: desired,
                }],
                coalesce_key: "solid".into(),
            },
            &h.permit,
        )
        .unwrap();
    collect(&mut h.edits);
    assert!(h.edits.waiting_for_ack());
    assert_eq!(h.edits.desired_solid_color(ClipId(1)), Some(desired));
    let accepted = kinewright_core::SolidColor { r: 0, g: 0, b: 0 };
    let output = h.ctx.run_ui(egui::RawInput::default(), |ui| {
        let mut pending = InspectorEdits::default();
        solid_color_row(
            ui,
            ClipId(1),
            accepted,
            h.edits.desired_solid_color(ClipId(1)),
            &mut pending,
        );
        assert!(
            pending.operations.is_empty(),
            "painting requested input does not send"
        );
    });
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains(&shape.shape, desired)),
        "PICKER_SHOWS_REQUESTED_COLOUR"
    );
    h.acknowledge();
    h.edits.start_next(&h.permit);
    collect(&mut h.edits);
    assert_eq!(
        h.edits.desired_solid_color(ClipId(1)),
        Some(desired),
        "accepted request remains the gesture input until the document event arrives"
    );
    h.edits.abandon_unaccepted();
    assert_eq!(
        h.edits.desired_solid_color(ClipId(1)),
        None,
        "abandon exposes accepted colour"
    );
}
