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
        Self::with_setup(|_| {})
    }
    fn with_setup(setup: impl FnOnce(&mut Document)) -> Self {
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
        setup(&mut document);
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
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        Self {
            core,
            owner,
            permit,
            edits,
            document,
            revision,
            ctx,
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

#[test]
fn astra_matte_numeric_typed_return_keeps_final_value() {
    let mut h = WheelProbe::new();
    let ctx = egui::Context::default();
    let effect = &h.document.clip(ClipId(1)).unwrap().effects[0];
    let frame = |events: Vec<egui::Event>| {
        let mut p = InspectorEdits::default();
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
                matte_integer_control(
                    ui,
                    ClipId(1),
                    effect,
                    "matte_hue_center_centidegrees",
                    "Hue",
                    0,
                    "matte-hue",
                    &mut p,
                );
            },
        );
        (p, output)
    };
    let (_, out) = frame(vec![]);
    let pos = out
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == "0" => Some(t.pos + t.galley.size() / 2.0),
            _ => None,
        })
        .expect("numeric readout");
    frame(vec![egui::Event::PointerMoved(pos), button(pos, true)]);
    frame(vec![button(pos, false)]);
    frame(vec![]);
    let (away, _) = frame(vec![egui::Event::Text("100".into())]);
    assert!(!away.operations.is_empty(), "typed excursion emitted");
    h.edits
        .submit(Command::DoBatch(away.operations), &h.permit)
        .unwrap();
    collect(&mut h.edits);
    assert!(h.edits.waiting_for_ack());
    let mods = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    let (returned, _) = frame(vec![
        egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: mods,
        },
        egui::Event::Text("0".into()),
    ]);
    eprintln!("typed return operations: {}", returned.operations.len());
    if !returned.operations.is_empty() {
        h.edits
            .submit(Command::DoBatch(returned.operations), &h.permit)
            .unwrap();
    }
    let (release, _) = frame(vec![egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    if !release.operations.is_empty() {
        h.edits
            .submit(Command::DoBatch(release.operations), &h.permit)
            .unwrap();
    }
    h.edits.end_gestures();
    h.drain();
    assert_eq!(h.edits.message(), None);
    let doc = h.core.export_history().unwrap().document;
    let params = &doc.clip(ClipId(1)).unwrap().effects[0].parameters;
    assert_eq!(
        params["matte_hue_center_centidegrees"],
        ParamValue::Integer(0),
        "MATTE_TYPED_RETURN_FINAL"
    );
}

impl WheelProbe {
    fn numeric_frame(
        &mut self,
        events: Vec<egui::Event>,
        mut draw: impl FnMut(&mut egui::Ui, &Document, &mut InspectorEdits),
    ) -> (egui::FullOutput, usize) {
        let _ = crate::mixer_ui::take_strip_rects();
        let mut pending = InspectorEdits::default();
        let output = self.ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| {
                pending = InspectorEdits::default();
                draw(ui, &self.document, &mut pending);
            },
        );
        if pending.gesture_started {
            self.edits.end_gestures();
            self.gesture += 1;
        }
        let count = pending.operations.len();
        if count > 0 {
            let command = if let Some(key) = pending.coalesce_key {
                Command::DoBatchCoalesced {
                    operations: pending.operations,
                    coalesce_key: format!("{key}#{}", self.gesture),
                }
            } else {
                Command::DoBatch(pending.operations)
            };
            self.edits
                .submit_value(
                    command,
                    pending.matches_document.then_some(self.revision),
                    &self.permit,
                )
                .unwrap();
        }
        if !self.ctx.input(|i| i.pointer.any_down()) {
            self.edits.end_gestures();
        }
        (output, count)
    }
}

#[derive(Clone, Copy, Debug)]
enum NumericFamily {
    Matte,
    ClipFade,
    Keyframe,
    TrackGain,
    MixerEffect,
}
impl NumericFamily {
    fn setup(self, document: &mut Document) {
        document.tracks.push(Track {
            id: TrackId(3),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![],
        });
        if matches!(self, Self::ClipFade | Self::Keyframe) {
            document.media_pool.push(kinewright_core::MediaAsset {
                id: kinewright_core::AssetId(1),
                path: "numeric-fixture.wav".into(),
                name: "numeric-fixture.wav".into(),
                duration: TimeCode(1_000_000),
                fps: kinewright_core::Rational::new(30, 1).unwrap(),
                kind: MediaKind::Audio,
                resolution: None,
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
                assumed_from: None,
            });
            let mut clip = document.tracks[0].clips.remove(0);
            clip.content = ClipContent::Media;
            clip.asset = kinewright_core::AssetId(1);
            clip.effects.clear();
            document.tracks[1].clips.push(clip);
        }
        if matches!(self, Self::Keyframe) {
            document.tracks[1].clips[0].audio_gain_curve =
                Some(upsert_keyframe(None, TimeCode(0), 0));
        }
        if matches!(self, Self::MixerEffect) {
            document.audio_mix.master.effects.push(Effect {
                id: EffectId(2),
                name: "audio_compressor".into(),
                enabled: true,
                enabled_curve: None,
                parameters: std::collections::BTreeMap::from([(
                    "threshold_tenth_db".into(),
                    ParamValue::Integer(0),
                )]),
                keyframes: std::collections::BTreeMap::new(),
            });
        }
    }
    fn draw(self, ui: &mut egui::Ui, document: &Document, pending: &mut InspectorEdits) {
        let clip = document.clip(ClipId(1)).unwrap();
        match self {
            Self::Matte => matte_integer_control(
                ui,
                clip.id,
                &clip.effects[0],
                "matte_hue_center_centidegrees",
                "Hue",
                0,
                "matte-hue",
                pending,
            ),
            Self::ClipFade => clip_audio_controls(ui, document, clip, 0, pending),
            Self::Keyframe => envelope_block(
                ui,
                clip.id,
                clip.audio_gain_curve.as_ref(),
                1_000_000,
                0,
                0,
                pending,
            ),
            Self::TrackGain => {
                let mix = document.audio_mix.track(TrackId(3));
                let mut gain = mix.gain_tenth_db;
                let response = ui.add(intent_drag_value(&mut gain, |field| {
                    field
                        .range(
                            kinewright_core::TRACK_MIX_GAIN_MIN
                                ..=kinewright_core::TRACK_MIX_GAIN_MAX,
                        )
                        .custom_formatter(|v, _| crate::mixer_ui::format_gain_db(v))
                        .custom_parser(crate::mixer_ui::parse_gain_db)
                        .update_while_editing(false)
                }));
                crate::mixer_ui::record_strip_rect("typed_track_gain", response.rect);
                crate::mixer_ui::record_mix_edit(pending, &response, &mix, || {
                    kinewright_core::TrackMix {
                        gain_tenth_db: gain,
                        ..kinewright_core::TrackMix::neutral(mix.track)
                    }
                });
            }
            Self::MixerEffect => {
                let master = &document.audio_mix.master;
                let effect = &master.effects[0];
                let mut edits = crate::mixer_ui::MixerChainEdits::default();
                crate::mixer_pane_ui::mixer_parameter_control(
                    ui,
                    crate::mixer_pane_ui::MixerChain::Master(master),
                    effect,
                    "threshold_tenth_db",
                    "Threshold",
                    crate::mixer_pane_ui::MixerUnit::Decibels,
                    0,
                    &mut edits,
                );
                *pending = edits.take_operations(document);
            }
        }
    }
    fn position(self, output: &egui::FullOutput) -> egui::Pos2 {
        let rects = crate::mixer_ui::take_strip_rects();
        if let Some((_, rect)) = rects.iter().find(|(name, _)| {
            name == "typed_track_gain"
                || name == "clip_fade_in"
                || name == "param:2:threshold_tenth_db"
                || name == "keyframe_row_value"
        }) {
            return rect.center();
        }
        let label = if matches!(self, Self::ClipFade) {
            "0 f"
        } else {
            "0"
        };
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{self:?} numeric readout; rects={rects:?}"))
    }
    fn away(self) -> &'static str {
        match self {
            Self::TrackGain => "10.0",
            Self::MixerEffect => "-10.0",
            _ => "100",
        }
    }
    fn value(self, document: &Document) -> i64 {
        let clip = document.clip(ClipId(1)).unwrap();
        match self {
            Self::Matte => {
                integer_value(&clip.effects[0].parameters["matte_hue_center_centidegrees"])
            }
            Self::ClipFade => clip.audio_fade_in_frames.0,
            Self::Keyframe => clip.audio_gain_curve.as_ref().unwrap().keyframes[0].value,
            Self::TrackGain => i64::from(document.audio_mix.track(TrackId(3)).gain_tenth_db),
            Self::MixerEffect => integer_value(
                &document.audio_mix.master.effects[0].parameters["threshold_tenth_db"],
            ),
        }
    }
}
fn enter_numeric() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
fn replace_numeric(text: &str) -> Vec<egui::Event> {
    vec![
        egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
        },
        egui::Event::Text(text.into()),
    ]
}
fn focus_numeric(h: &mut WheelProbe, family: NumericFamily, position: egui::Pos2) {
    for events in [
        vec![egui::Event::PointerMoved(position), button(position, true)],
        vec![button(position, false)],
        vec![],
    ] {
        let (_, count) = h.numeric_frame(events, |ui, doc, p| family.draw(ui, doc, p));
        assert_eq!(count, 0, "NUMERIC_UNCHANGED_FOCUS_IS_NOT_INTENT {family:?}");
    }
}
#[test]
fn typed_numeric_returns_reach_every_producer_family() {
    for family in [
        NumericFamily::Matte,
        NumericFamily::ClipFade,
        NumericFamily::Keyframe,
        NumericFamily::TrackGain,
        NumericFamily::MixerEffect,
    ] {
        let mut h = WheelProbe::with_setup(|d| family.setup(d));
        let (output, _) = h.numeric_frame(vec![], |ui, doc, p| family.draw(ui, doc, p));
        let position = family.position(&output);
        focus_numeric(&mut h, family, position);
        h.numeric_frame(
            vec![egui::Event::Text(family.away().into())],
            |ui, doc, p| family.draw(ui, doc, p),
        );
        h.numeric_frame(vec![enter_numeric()], |ui, doc, p| family.draw(ui, doc, p));
        collect(&mut h.edits);
        assert!(
            h.edits.waiting_for_ack(),
            "NUMERIC_TYPED_EXCURSION_WAITS {family:?}"
        );
        focus_numeric(&mut h, family, position);
        h.numeric_frame(replace_numeric("0"), |ui, doc, p| family.draw(ui, doc, p));
        h.numeric_frame(vec![enter_numeric()], |ui, doc, p| family.draw(ui, doc, p));
        h.edits.end_gestures();
        h.drain();
        assert_eq!(
            family.value(&h.core.export_history().unwrap().document),
            0,
            "NUMERIC_TYPED_RETURN_FINAL {family:?}"
        );
        assert!(!h.edits.pending());
        assert_eq!(h.edits.message(), None);
    }
}
#[test]
fn invalid_numeric_text_and_unchanged_focus_make_no_edit() {
    for family in [
        NumericFamily::Matte,
        NumericFamily::ClipFade,
        NumericFamily::Keyframe,
        NumericFamily::TrackGain,
        NumericFamily::MixerEffect,
    ] {
        let mut h = WheelProbe::with_setup(|d| family.setup(d));
        let before = h.core.export_history().unwrap();
        let (output, _) = h.numeric_frame(vec![], |ui, doc, p| family.draw(ui, doc, p));
        let position = family.position(&output);
        focus_numeric(&mut h, family, position);
        let (_, count) =
            h.numeric_frame(vec![enter_numeric()], |ui, doc, p| family.draw(ui, doc, p));
        assert_eq!(count, 0, "NUMERIC_UNCHANGED_COMMIT {family:?}");
        focus_numeric(&mut h, family, position);
        for events in [replace_numeric("-"), vec![enter_numeric()], vec![]] {
            let (_, count) = h.numeric_frame(events, |ui, doc, p| family.draw(ui, doc, p));
            assert_eq!(count, 0, "NUMERIC_INVALID_TEXT_IS_NOT_EDIT {family:?}");
        }
        assert!(!h.edits.pending());
        assert_eq!(h.edits.message(), None);
        assert_eq!(h.core.export_history().unwrap(), before);
    }
}
#[test]
fn unchanged_slider_click_does_not_auto_key_or_add_undo() {
    fn circle(shape: &egui::Shape) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Circle(c) if c.radius > 3.0 && c.radius < 12.0 => Some(c.center),
            egui::Shape::Rect(r) if r.rect.width() <= 16.0 && r.rect.height() > 8.0 => {
                Some(r.rect.center())
            }
            egui::Shape::Vec(v) => v.iter().find_map(circle),
            _ => None,
        }
    }
    fn transform_document(d: &mut Document) {
        let effect = &mut d.tracks[0].clips[0].effects[0];
        effect.name = "transform".into();
        effect.keyframes.insert(
            "scale_x_percent".into(),
            upsert_keyframe(None, TimeCode(0), 100),
        );
    }
    let mut h = WheelProbe::with_setup(transform_document);
    let draw = |ui: &mut egui::Ui, doc: &Document, p: &mut InspectorEdits| {
        let effect = &doc.clip(ClipId(1)).unwrap().effects[0];
        let parameter = kinewright_core::effect_descriptor("transform")
            .unwrap()
            .parameter("scale_x_percent")
            .unwrap();
        motion_param_row(
            ui,
            ClipId(1),
            effect,
            parameter,
            TimeCode(50),
            TimeCode(1_000_000),
            false,
            p,
        );
    };
    let (output, _) = h.numeric_frame(vec![], draw);
    let position = output
        .shapes
        .iter()
        .find_map(|s| circle(&s.shape))
        .expect("painted slider thumb");
    let before = h.core.export_history().unwrap();
    for events in [
        vec![egui::Event::PointerMoved(position), button(position, true)],
        vec![button(position, false)],
        vec![],
    ] {
        let (_, count) = h.numeric_frame(events, draw);
        assert_eq!(count, 0, "NUMERIC_UNCHANGED_CLICK_NO_OPERATION");
    }
    h.drain();
    assert_eq!(
        h.core.export_history().unwrap(),
        before,
        "NUMERIC_UNCHANGED_CLICK_NO_UNDO"
    );
    let mut h = WheelProbe::with_setup(transform_document);
    let (output, _) = h.numeric_frame(vec![], draw);
    let position = output
        .shapes
        .iter()
        .find_map(|s| circle(&s.shape))
        .expect("painted slider thumb");
    let before = h.core.export_history().unwrap();
    h.numeric_frame(
        vec![egui::Event::PointerMoved(position), button(position, true)],
        draw,
    );
    let (_, count) = h.numeric_frame(
        vec![egui::Event::PointerMoved(position + egui::vec2(20.0, 0.0))],
        draw,
    );
    assert!(count > 0, "NUMERIC_MOVED_SLIDER_EMITS");
    collect(&mut h.edits);
    assert!(h.edits.waiting_for_ack());
    h.numeric_frame(
        vec![egui::Event::PointerMoved(position), button(position, false)],
        draw,
    );
    h.drain();
    let after = h.core.export_history().unwrap();
    let curve = &after.document.clip(ClipId(1)).unwrap().effects[0].keyframes["scale_x_percent"];
    assert_eq!(
        curve.value_at(TimeCode(50)),
        Some(100),
        "NUMERIC_MOVED_SLIDER_RETURN"
    );
    assert_eq!(
        after.cursor.undo.len(),
        before.cursor.undo.len() + 1,
        "NUMERIC_MOVED_SLIDER_ONE_UNDO"
    );
    assert!(matches!(
        h.permit.request(Command::Undo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    assert_eq!(h.core.export_history().unwrap().document, before.document);
}

fn integer_value(value: &ParamValue) -> i64 {
    let ParamValue::Integer(value) = value else {
        panic!("integer numeric field")
    };
    *value
}

#[test]
fn numeric_text_and_enter_in_one_frame_preserve_the_return() {
    for family in [NumericFamily::Matte, NumericFamily::MixerEffect] {
        let mut h = WheelProbe::with_setup(|d| family.setup(d));
        let (output, _) = h.numeric_frame(vec![], |ui, doc, p| family.draw(ui, doc, p));
        let position = family.position(&output);
        focus_numeric(&mut h, family, position);
        let mut events = replace_numeric(family.away());
        events.push(enter_numeric());
        let (_, count) = h.numeric_frame(events, |ui, doc, p| family.draw(ui, doc, p));
        assert!(count > 0, "NUMERIC_TEXT_AND_ENTER_COMMITS {family:?}");
        collect(&mut h.edits);
        assert!(h.edits.waiting_for_ack());
        focus_numeric(&mut h, family, position);
        let mut events = replace_numeric("0");
        events.push(enter_numeric());
        let (_, count) = h.numeric_frame(events, |ui, doc, p| family.draw(ui, doc, p));
        assert!(count > 0, "NUMERIC_TEXT_AND_ENTER_RETURN {family:?}");
        h.drain();
        assert_eq!(family.value(&h.core.export_history().unwrap().document), 0);
        assert!(!h.edits.pending());
        assert_eq!(h.edits.message(), None);
    }
}
