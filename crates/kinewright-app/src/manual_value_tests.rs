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
use kinewright_project::operation_actor::Core;
use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

#[path = "../../kinewright-core/tests/support/undo_fixture.rs"]
mod undo_fixture;

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
        let mut document = Document {
            tracks: vec![Track {
                name: None,
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
        Self::with_document(document)
    }

    fn with_document(document: Document) -> Self {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "manual-final-value");
        let path = temp.path("p.kinewright");
        let before = Arc::new(document);
        let core = Core::for_test(Arc::clone(&before)).unwrap();
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
            "MANUAL_REAL_LEASE status={:?} current={}",
            attachment.host.as_ref().unwrap().status,
            attachment.owner.core().snapshot_identity().unwrap()
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
                self.app
                    .focused()
                    .ja1
                    .as_ref()
                    .and_then(Ja1Attachment::manual_edit_message)
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
        // Fill the actual command window before opening the drag. No-op closes
        // change no document or stack, so the final drag still has one Undo.
        // The new 1 MiB coalesced-step cap is smaller than the 2 MiB outbox;
        // repeatedly extending that step would test a different refusal.
        for _ in 1..kinewright_project::operation_protocol::PENDING_COMMANDS {
            self.gate().permit.close_gesture().unwrap();
        }
        self.send_value(1);
        // Already exported packets retain their identity and cannot take another sample.
        let _ = self.gate().permit.pending().unwrap();
        self.send_value(2);
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
#[path = "manual_window_tests.rs"]
mod manual_window_tests;

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

fn root_generation(path: &std::path::Path) -> u64 {
    let root: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    root["studio"]["generation"].as_u64().unwrap()
}

#[test]
fn hx_h4_one_publication_per_request() {
    use kinewright_agent::writer_host::HostCommand;
    use kinewright_project::operation_protocol::PENDING_COMMANDS;

    let f = Fixture::with_document(undo_fixture::heavy());
    let host = f.attachment().host.as_ref().unwrap();
    let path = f.temp.path("p.kinewright");
    let mut counts = Vec::new();
    for size in [1, PENDING_COMMANDS] {
        for _ in 0..size {
            f.gate().permit.close_gesture().unwrap();
        }
        let messages = f.gate().permit.pending().unwrap();
        assert_eq!(messages.len(), size);
        let before = root_generation(&path);
        let start = Instant::now();
        let reply = host
            .client
            .request(HostCommand::CommitHistory {
                client: host.manual_identity_for_test(),
                messages,
            })
            .unwrap();
        let elapsed = start.elapsed();
        let count = root_generation(&path) - before;
        println!("HX_H4_REQUEST size={size} publications={count} wall={elapsed:?}");
        assert!(reply.error.is_none(), "{:?}", reply.error);
        assert_eq!(reply.manual_ack.len(), size);
        f.attachment()
            .owner
            .acknowledge_manual(&f.gate().permit, reply.manual_ack)
            .unwrap();
        counts.push(count);
    }
    assert_eq!(counts, [1, 1], "HX_H4_ONE_ROOT_PER_REQUEST");
}

#[test]
fn hx_h4_refused_message_leaves_only_the_durable_prefix() {
    use kinewright_agent::writer_host::HostCommand;

    let f = Fixture::new();
    let attachment = f.attachment();
    let host = attachment.host.as_ref().unwrap();
    let path = f.temp.path("p.kinewright");
    let request = |messages| {
        host.client
            .request(HostCommand::CommitHistory {
                client: host.manual_identity_for_test(),
                messages,
            })
            .unwrap()
    };
    let mut identities = Vec::new();
    for value in 1..=4 {
        f.gate()
            .permit
            .request(Command::DoBatchCoalesced {
                operations: vec![param(value)],
                coalesce_key: "prefix".into(),
            })
            .unwrap();
        // Give each command a wire identity for the out-of-order prefix test.
        let _ = f.gate().permit.pending().unwrap();
        identities.push(attachment.owner.core().snapshot_identity().unwrap());
    }
    let messages = f.gate().permit.pending().unwrap();
    let before = root_generation(&path);
    let zero = request(vec![messages[1].clone(), messages[0].clone()]);
    assert_eq!(zero.error.unwrap().code, "history_conflict");
    assert_eq!(zero.manual_unattempted, Some(1));
    assert!(zero.manual_ack.is_empty());
    assert_eq!(root_generation(&path), before);

    let prefix = request(vec![
        messages[0].clone(),
        messages[1].clone(),
        messages[3].clone(),
        messages[2].clone(),
    ]);
    assert_eq!(prefix.error.unwrap().code, "history_conflict");
    assert_eq!(prefix.manual_unattempted, Some(1));
    assert_eq!(prefix.manual_ack.len(), 2);
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    assert_eq!(
        loaded.open_manual().unwrap().snapshot_identity().unwrap(),
        identities[1]
    );
    assert_eq!(
        loaded.root().unwrap().generation,
        before + 1,
        "HX_H4_ATOMIC_PREFIX"
    );
    drop(loaded);

    let mixed = request(messages);
    assert!(mixed.error.is_none(), "{:?}", mixed.error);
    assert_eq!(mixed.manual_unattempted, None);
    assert_eq!(mixed.manual_ack.len(), 4);
    assert_eq!(mixed.manual_ack[..2], prefix.manual_ack);
    assert_eq!(
        root_generation(&path),
        before + 2,
        "HX_H4_RETRY_PUBLISHES_ONLY_NEW"
    );
}

#[test]
fn hx_h4_app_retry_resends_the_unattempted_edits() {
    use kinewright_agent::writer_host::HostCommand;

    let mut f = Fixture::new();
    for value in 1..=4 {
        f.gate()
            .permit
            .request(Command::DoBatchCoalesced {
                operations: vec![param(value)],
                coalesce_key: "retained".into(),
            })
            .unwrap();
        // Each command is exported before adding the next packet to the suffix.
        let _ = f.gate().permit.pending().unwrap();
    }
    let originals = f.gate().permit.pending().unwrap();
    let host = f.attachment().host.as_ref().unwrap();
    let reply = host
        .client
        .request(HostCommand::CommitHistory {
            client: host.manual_identity_for_test(),
            messages: vec![
                originals[0].clone(),
                originals[2].clone(),
                originals[3].clone(),
            ],
        })
        .unwrap();
    assert_eq!(reply.manual_unattempted, Some(1));
    let attachment = f.app.focused_mut().ja1.as_mut().unwrap();
    assert!(attachment.accept_host_reply(reply).is_err());
    assert_eq!(f.gate().permit.pending().unwrap(), originals[1..]);
    f.release();
    f.app.start_ja1(Action::Retry);
    f.until(|f| {
        !f.attachment().read_only()
            && f.gate().permit.pending().unwrap().is_empty()
            && !f.attachment().is_dirty()
    });
    assert_eq!(f.scalar(), 4);
    let mut cold = LoadedBundle::read(
        &f.temp.path("p.kinewright"),
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    assert_eq!(
        cold.open_manual()
            .unwrap()
            .export_history()
            .unwrap()
            .history()
            .document
            .clip(ClipId(1))
            .unwrap()
            .effects[0]
            .parameters["exposure_milli_stops"],
        ParamValue::Integer(4),
    );
}

#[test]
fn hx_h4_full_distinct_window_displays_accepted_solid_color() {
    let mut f = Fixture::new();
    for _ in 0..kinewright_project::operation_protocol::PENDING_COMMANDS {
        f.gate().permit.close_gesture().unwrap();
    }
    let clip = ClipId(1);
    let operation = Operation::SetSolidColor {
        clip,
        color: SolidColor { r: 255, g: 0, b: 0 },
    };
    f.app
        .send_operations_coalesced(vec![operation.clone()], f.key.clone());
    f.attempt();
    assert!(f.edits().waiting_for_ack());
    assert_eq!(f.edits().backpressure_refusals, 1);
    assert_eq!(*f.app.focused().document, *f.before);
    assert_eq!(
        f.edits().message(),
        Some(crate::manual_edits::PENDING_WORDS)
    );
    assert_eq!(f.edits().desired_solid_color(clip), None);

    f.finish();
    let mut expected = (*f.before).clone();
    operation.apply(&mut expected).unwrap();
    assert_eq!(*f.app.focused().document, expected);
}

#[test]
fn hx_h4_pre_rename_error_retries_original_manual_packets() {
    ambiguous_root_retries_original_manual_packets("root_rename_io");
}

#[test]
fn hx_h4_parent_sync_error_retries_original_manual_packets() {
    ambiguous_root_retries_original_manual_packets("root_parent_sync");
}

fn ambiguous_root_retries_original_manual_packets(point: &'static str) {
    let mut f = Fixture::new();
    let path = f.temp.path("p.kinewright");
    let before = root_generation(&path);
    for value in [1, 119] {
        f.app
            .send_operations_coalesced(vec![param(value)], f.key.clone());
        f.attempt();
    }
    assert_eq!(f.gate().permit.pending_count().unwrap(), 1);
    f.attachment()
        .fixture_host
        .as_ref()
        .unwrap()
        .fail_manual_root_for_test(point, 1);
    f.release();
    f.until(|f| f.attachment().read_only() || f.gate().permit.pending_count().unwrap() == 0);
    assert!(
        !f.attachment().read_only(),
        "{point}: {}",
        f.attachment().message
    );
    f.finish();
    assert_eq!(f.gate().permit.pending_count().unwrap(), 0);
    assert_eq!(root_generation(&path), before + 1);
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    assert_eq!(
        loaded.open_manual().unwrap().snapshot_identity().unwrap(),
        f.attachment().owner.core().snapshot_identity().unwrap()
    );
    assert_eq!(f.scalar(), 119);
    println!("HX_H4_ROOT_RETRY {point} saved=119 publications=1 pending=0");
}

#[test]
fn hx_h4_ambiguous_root_retry_exhaustion_keeps_edits_read_only() {
    let mut f = Fixture::new();
    let path = f.temp.path("p.kinewright");
    let before = root_generation(&path);
    f.send_value(119);
    f.attachment()
        .fixture_host
        .as_ref()
        .unwrap()
        .fail_manual_root_for_test("root_rename_io", 2);
    f.release();
    f.until(|f| f.attachment().read_only());
    assert_eq!(f.attachment().message, "durability_unknown");
    assert_eq!(f.value(), 119);
    assert_eq!(f.gate().permit.pending_count().unwrap(), 1);
    assert!(f.attachment().is_dirty());
    assert_eq!(root_generation(&path), before);
}

fn strict_drag(control: &str, operation: impl Fn(i64) -> Operation) -> usize {
    let mut f = Fixture::with_document(undo_fixture::heavy());
    f.release();
    let before = Arc::clone(&f.before);
    let interval = Duration::from_millis(39);
    let mut accepted = 0;
    let mut sent = Instant::now();
    for value in 0..120 {
        let previous = f.attachment().owner.core().change_id().unwrap().position;
        sent = Instant::now();
        f.app
            .send_operations_coalesced(vec![operation(value)], f.key.clone());
        f.attempt();
        let current = f.attachment().owner.core().change_id().unwrap().position;
        if current.position == previous.position + 1 && !f.edits().waiting_for_ack() {
            accepted += 1;
        }
        if value < 119 {
            while sent.elapsed() < interval {
                f.tick();
                std::thread::park_timeout(Duration::from_millis(1));
            }
        }
    }
    println!("HX_H4_DRAG_SENT {control} accepted={accepted}/120");
    f.finish();
    println!(
        "HX_H4_DRAG {control} accepted={accepted}/120 last_ack={:?}",
        sent.elapsed()
    );
    assert_eq!(
        f.edits().backpressure_refusals,
        0,
        "HX_H4_NO_BACKPRESSURE {control}"
    );
    let mut expected = (*before).clone();
    operation(119).apply(&mut expected).unwrap();
    assert_eq!(*f.app.focused().document, expected, "HX_H4_DRAG_FINAL");
    let mut loaded = LoadedBundle::read(
        &f.temp.path("p.kinewright"),
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let cold = loaded.open_manual().unwrap().export_history().unwrap();
    assert_eq!(*cold.history().document, expected, "HX_H4_DRAG_COLD");
    drop(cold);
    drop(loaded);
    f.app.undo();
    f.finish();
    assert_eq!(*f.app.focused().document, *before, "HX_H4_DRAG_UNDO");
    accepted
}

fn held_reply_drag(control: &str, operation: impl Fn(i64) -> Operation) -> (usize, usize) {
    let mut f = Fixture::with_document(undo_fixture::heavy());
    let gate = Arc::new(SendGate::default());
    let _release = Release(Arc::clone(&gate));
    let held = Arc::clone(&gate);
    f.app
        .focused_mut()
        .ja1
        .as_mut()
        .unwrap()
        .host
        .as_mut()
        .unwrap()
        .after_first_manual_reply = Some(Box::new(move || held.hold()));
    f.release();
    let mut accepted = 0;
    let path = f.temp.path("p.kinewright");
    for value in 0..120 {
        let before = f.attachment().owner.core().change_id().unwrap().position;
        f.app
            .send_operations_coalesced(vec![operation(value)], f.key.clone());
        f.attempt();
        if value == 0 {
            f.until(|_| gate.entered());
        }
        let after = f.attachment().owner.core().change_id().unwrap().position;
        accepted += usize::from(after.position == before.position + 1);
    }
    assert_eq!(f.gate().permit.pending_count().unwrap(), 2);
    println!("HX_H4_HELD {control} issued=120 accepted={accepted} pending=2");
    let before = root_generation(&path);
    gate.release();
    f.finish();
    let publications = root_generation(&path) - before;
    let mut expected = (*f.before).clone();
    operation(119).apply(&mut expected).unwrap();
    assert_eq!(*f.app.focused().document, expected, "HX_H4_HELD_LAST_VALUE");
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    assert_eq!(
        *loaded
            .open_manual()
            .unwrap()
            .export_history()
            .unwrap()
            .history()
            .document,
        expected,
        "HX_H4_HELD_COLD"
    );
    drop(loaded);
    f.app.undo();
    f.finish();
    assert_eq!(*f.app.focused().document, *f.before, "HX_H4_HELD_ONE_UNDO");
    println!(
        "HX_H4_HELD {control} publications_after_release={publications} cold_reopen_and_undo=ok"
    );
    assert!(publications <= 2, "HX_H4_HELD_PUBLICATIONS {publications}");
    (accepted, f.edits().backpressure_refusals)
}

#[test]
fn hx_h4_held_first_reply_folds_unsent_gesture() {
    let exposure = held_reply_drag("exposure", param);
    let sync_lock = held_reply_drag("sync-lock", |value| Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: value % 2 == 0,
    });
    assert_eq!(
        [exposure, sync_lock],
        [(120, 0), (120, 0)],
        "HX_H4_HELD_NO_BACKPRESSURE"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn hx_h4_strict_drag_saves_all_sub_batches() {
    let exposure = strict_drag("exposure", param);
    let sync_lock = strict_drag("sync-lock", |value| Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: value % 2 == 0,
    });
    assert_eq!([exposure, sync_lock], [120, 120], "HX_H4_NO_BACKPRESSURE");
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
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let cold = loaded.open_manual().unwrap();
    assert_eq!(
        *cold.export_history().unwrap().history().document,
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
            Err(kinewright_project::operation_actor::Refusal {
                error: kinewright_project::operation_actor::Error::MutationRefused(reason),
                ..
            }) => Some(reason),
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
        *f.app
            .focused()
            .core
            .export_history()
            .unwrap()
            .history()
            .document,
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
    f.until(|_| gate.entered());
    assert_ne!(f.value(), -555, "HELD_FINAL_VALUE_HAS_NOT_BEEN_ACCEPTED");
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
        .history()
        .undo
        .iter()
        .rev()
        .take(15)
        .rev()
        .map(|step| match &step.commands()[0].operations[0] {
            Operation::SetEffectParam {
                value: ParamValue::Integer(v),
                ..
            } => *v,
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
        let saved = LoadedBundle::read(
            &f.temp.path("p.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap()
        .open_manual()
        .unwrap();
        assert_ne!(
            *saved.export_history().unwrap().history().document,
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
    let cold = LoadedBundle::read(
        &recovered,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap()
    .open_manual()
    .unwrap();
    assert_eq!(
        *cold.export_history().unwrap().history().document,
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
    for _ in 1..kinewright_project::operation_protocol::PENDING_COMMANDS {
        f.gate().permit.close_gesture().unwrap();
    }
    f.send_value(10);
    assert!(f.attachment().manual_edits_pending());
    assert_eq!(
        f.attachment().manual_edit_message(),
        None,
        "MANUAL_OPEN_GESTURE_IS_NOT_WAITING"
    );
    // Export the last slot before issuing a value that really needs space.
    let _ = f.gate().permit.pending().unwrap();
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
        let next = if refused {
            Operation::SetTrackSyncLock {
                track: TrackId(404),
                locked: false,
            }
        } else {
            param(20)
        };
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
        let Event::QueryResult(QueryResult::Document(accepted)) = f
            .app
            .focused()
            .core
            .request(Command::Query(Query::Document))
            .unwrap()
        else {
            panic!("LATE_RESULT_ORACLE_IS_THE_ACTORS_ACCEPTED_DOCUMENT");
        };
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

#[test]
fn manual_value_glm_prepared_media_clean_refusal_stays_editable() {
    for forged_counter in [true, false] {
        let mut f = Fixture::new();
        let source = f.temp.path("refused.wav");
        std::fs::write(
            &source,
            kinewright_media::test_support::wav_f32(
                &vec![0.25; 4800].into_boxed_slice(),
                48_000,
                1,
            ),
        )
        .unwrap();
        let before = f.app.focused().core.export_history().unwrap();
        f.app.focused_mut().ja1.as_mut().unwrap().before_host_reply = Some(if forged_counter {
            forge_prepared_media_counter
        } else {
            unbind_prepared_media_reply
        });
        let words = if forged_counter {
            kinewright_project::operation_protocol::Conflict::Snapshot.to_string()
        } else {
            CoreMutationRefusal::ManualInputRequired.to_string()
        };
        let error = f
            .app
            .focused_mut()
            .ja1
            .as_mut()
            .unwrap()
            .apply(Action::ImportMedia(source))
            .unwrap_err();
        assert!(
            if forged_counter {
                matches!(
                    error,
                    BundleError::HistoryChanged
                        | BundleError::Operation(
                            kinewright_project::operation_actor::Error::Protocol(
                                kinewright_project::operation_protocol::Conflict::Snapshot
                            )
                        )
                )
            } else {
                matches!(
                    error,
                    BundleError::CoreMutationRefused(CoreMutationRefusal::ManualInputRequired)
                )
            },
            "MANUAL_PREPARED_REAL_CORE_REFUSAL forged={forged_counter}: {error}"
        );
        assert_eq!(f.app.focused().core.export_history().unwrap(), before);
        assert!(
            f.attachment().imported_media.is_none(),
            "MANUAL_REFUSED_ASSET_NOT_ANNOUNCED"
        );
        if forged_counter {
            assert!(
                f.attachment().read_only(),
                "FORGED_PREPARED_COUNTER_FREEZES_INTO_RECOVERY"
            );
            let host = f.attachment().host.as_ref().unwrap();
            assert!(host.cut.is_some() && host.unknown_operation);
            continue;
        }
        assert!(
            !f.attachment().read_only(),
            "MANUAL_PREPARED_REFUSAL_STAYS_EDITABLE: {}",
            f.attachment().message
        );
        let host = f.attachment().host.as_ref().unwrap();
        assert!(host.cut.is_none() && !host.unknown_operation && !host.recovery_running());
        assert_eq!(f.attachment().message, words);
        f.until(|f| f.app.status.contains(&words) && !f.attachment().read_only());
        assert!(!f.attachment().read_only());
        assert!(
            f.app.status.contains(&words),
            "MANUAL_PREPARED_REASON_REMAINS_VISIBLE"
        );
        assert!(!f.app.status.contains("reply was not confirmed"));
        assert!(f.app.focused().pending_timeline_adds.is_empty());
        if !forged_counter {
            f.app.send_operation(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            });
            f.finish();
            assert!(
                !f.app.focused().document.tracks[0].sync_lock,
                "MANUAL_EDIT_AFTER_PREPARED_REFUSAL_LANDS"
            );
            assert!(f.app.focused().document.media_pool.is_empty());
            assert!(!f.attachment().read_only());
        }
    }
}

fn forge_prepared_media_counter(reply: &mut kinewright_agent::writer_host::HostReply) {
    assert!(reply.error.is_none() && reply.owned_media.is_some());
    let lease = reply.manual_lease.as_mut().expect("real manual lease");
    lease.next_key.position = lease.next_key.position.checked_add(1).unwrap();
}
fn unbind_prepared_media_reply(reply: &mut kinewright_agent::writer_host::HostReply) {
    assert!(reply.error.is_none() && reply.manual_lease.is_some());
    // Keep the authentic input manifest but ask for an uncovered asset identity.
    reply
        .owned_media
        .as_mut()
        .expect("real prepared media reply")
        .id = AssetId(999);
}

#[test]
fn manual_value_fix2_healthy_previews_wait_without_abandoning() {
    let mut f = Fixture::new();
    f.fill();
    let before = f.app.focused().core.export_history().unwrap();
    let recovery = f.attachment().host.as_ref().unwrap().status.recovery_points[0].clone();
    let actions = [
        Action::PreviewRecovery(recovery.into()),
        Action::PreviewRetirement(kinewright_project::bundle_records::BranchId::fresh().unwrap()),
        Action::PreviewForget,
        Action::PreviewRelease(f.temp.path("preview-source.kinewright")),
    ];
    for action in actions {
        let attachment = f.app.focused_mut().ja1.as_mut().unwrap();
        let result = attachment.apply(action);
        assert!(
            result.is_err() && attachment.message == crate::manual_edits::PENDING_WORDS,
            "MANUAL_HEALTHY_PREVIEW_WAITS: {result:?}; {}",
            attachment.message
        );
        assert!(attachment.host.as_ref().unwrap().cut.is_none());
        assert!(f.edits().waiting_for_ack());
        assert!(f.edits().notice().is_none());
        assert_eq!(f.app.focused().core.export_history().unwrap(), before);
    }
    {
        // Every preview has been checked against the complete owner. The
        // oracle is no longer needed while the real pending work drains.
        let pool = f.app.focused().core.history_pool();
        let leases = pool
            .lease_counts(kinewright_project::operation_history::Role::Ordinary)
            .unwrap()
            .0;
        drop(before);
        assert_eq!(
            pool.lease_counts(kinewright_project::operation_history::Role::Ordinary)
                .unwrap()
                .0,
            leases - 1,
            "MANUAL_PREVIEW_ORACLE_RELEASES_ITS_FULL_LEASE_BEFORE_DRAIN"
        );
    }
    f.finish();
    assert!(f.edits().notice().is_none());
    let Event::QueryResult(QueryResult::Snapshot { document, .. }) = f
        .app
        .focused()
        .core
        .request(Command::Query(Query::Snapshot))
        .unwrap()
    else {
        panic!("MANUAL_HEALTHY_PREVIEW_HAS_ITS_ACTUAL_DOCUMENT");
    };
    assert_eq!(
        document.clip(ClipId(1)).unwrap().effects[0].keyframes["exposure_milli_stops"].keyframes[0]
            .value,
        2,
        "MANUAL_HEALTHY_PREVIEW_DRAINS_THE_RETAINED_FINAL_VALUE"
    );
}
