// JA-1 S6 C4: the notifier against a fake daemon on a private bus.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use kinewright_core::output_review::SealedReviewOutcome;
use kinewright_project::{bundle_records::RecordKind, studio_records::SnapshotKey};

use super::{
    DEFAULT_ACTION, MAX_QUEUED, Notifier, SealNotice,
    test_bus::{Peer, PrivateBus, action, click, closed, wait_for},
};

fn key(n: u8) -> SnapshotKey {
    let id = format!("{n:032x}");
    serde_json::from_value(serde_json::json!({
        "project": id,
        "version": id,
        "head": id,
        "document_hash": "00".repeat(32),
        "input_manifest_ref": {
            "sha256": "00".repeat(32),
            "byte_length": 0,
            "record_kind": RecordKind::Document,
        },
    }))
    .unwrap()
}

fn delivered() -> SealedReviewOutcome {
    SealedReviewOutcome::Delivered
}

fn facts_not_checked() -> SealedReviewOutcome {
    SealedReviewOutcome::ReviewedFactsNotChecked {
        exceptions: Vec::new(),
    }
}

fn notice(label: &str, gui: bool) -> SealNotice {
    let alive = gui.then(|| Arc::new(AtomicBool::new(true)));
    notice_for(label, alive.as_ref())
}

/// A notice whose GUI is `alive`; none means no GUI was attached.
fn notice_for(label: &str, alive: Option<&Arc<AtomicBool>>) -> SealNotice {
    SealNotice::new(
        label,
        &delivered(),
        alive.map(|_| (key(3), "ab".repeat(32))),
        alive.into_iter().map(Arc::clone).collect(),
    )
}

#[test]
fn ja1_s6_c4_a_seal_notifies_once_with_its_label_and_a_click_action() {
    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(true);
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v3", true));
    wait_for(|| daemon.calls.lock().unwrap().len() == 1, "S6C4_SENT");
    notifier.notify(SealNotice::new(
        "v4",
        &facts_not_checked(),
        None,
        Vec::new(),
    ));
    wait_for(|| daemon.calls.lock().unwrap().len() == 2, "S6C4_SENT_TWO");
    notifier.take_stop().finish();
    let calls = daemon.calls.lock().unwrap().clone();
    assert_eq!(calls[0].app_name, "Kinewright", "S6C4_APP_NAME");
    assert_eq!(calls[0].summary, "Kinewright", "S6C4_TITLE");
    // A new notification each time, no icon or hints, the daemon's own timeout.
    assert_eq!(
        (
            calls[0].replaces_id,
            calls[0].app_icon.as_str(),
            calls[0].hints.len(),
            calls[0].expire_timeout
        ),
        (0, "", 0, -1),
        "S6C4_PLAIN_NOTIFY"
    );
    assert_eq!(calls[0].body, "v3 is delivered", "S6C4_LABEL_DELIVERED");
    assert_eq!(
        calls[0].actions,
        ["default", "Show"],
        "S6C4_ACTION_WITH_GUI"
    );
    assert_eq!(
        calls[1].body, "v4 is reviewed, facts not checked",
        "S6C4_LABEL_FACTS_NOT_CHECKED"
    );
    assert!(calls[1].actions.is_empty(), "S6C4_NO_ACTION_WITHOUT_GUI");
    assert_eq!(calls.len(), 2, "S6C4_ONE_EACH");
    assert_eq!(notifier.status().error, None);
}

#[test]
fn ja1_s6_c4_no_gui_or_no_action_capability_offers_no_action() {
    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(false);
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v1", true));
    wait_for(|| daemon.calls.lock().unwrap().len() == 1, "S6C4_SENT");
    assert!(
        daemon.calls.lock().unwrap()[0].actions.is_empty(),
        "S6C4_NO_ACTION_WITHOUT_CAPABILITY"
    );
    // Nothing can be clicked, so nothing listens.
    assert!(!super::lock(&notifier.shared).listening, "S6C4_NO_LISTENER");
    notifier.take_stop().finish();
}

/// Make `id` clickable from `daemon`, then close it from `from` and wait
/// until the listener has handled that: every signal `from` sent before it
/// has been handled too.
fn handled_after(notifier: &Notifier, from: &Peer, id: u32) {
    super::lock(&notifier.shared).outstanding.insert(
        id,
        super::Outstanding {
            daemon: from.name(),
            target: super::Target {
                version: key(9),
                key_sha256: "cd".repeat(32),
            },
        },
    );
    closed(from, id);
    wait_for(
        || !super::lock(&notifier.shared).outstanding.contains_key(&id),
        "S6C4_MARKER_HANDLED",
    );
}

#[test]
fn ja1_s6_c4_one_click_on_this_hosts_notification_selects_once() {
    let bus = PrivateBus::start();
    let (daemon, server) = bus.daemon(true);
    let stranger = bus.stranger();
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v3", true));
    // A second clickable notification keeps the listener waiting throughout.
    notifier.notify(notice("v4", true));
    wait_for(|| daemon.calls.lock().unwrap().len() == 2, "S6C4_SENT");
    let id = daemon.calls.lock().unwrap()[0].id;
    wait_for(
        || super::lock(&notifier.shared).outstanding.len() == 2,
        "S6C4_OUTSTANDING",
    );
    // A foreign ID, another action key, and a click from a client that is
    // not the daemon select nothing.
    click(&server, id + 100);
    action(&server, id, "Show");
    handled_after(&notifier, &server, id + 200);
    click(&stranger, id);
    handled_after(&notifier, &stranger, id + 201);
    assert_eq!(
        notifier.status().select,
        None,
        "S6C4_FOREIGN_SELECTS_NOTHING"
    );
    click(&server, id);
    wait_for(|| notifier.status().select.is_some(), "S6C4_CLICK_SELECTS");
    let select = notifier.status().select.unwrap();
    assert_eq!(select.version, key(3), "S6C4_SELECTS_THE_SEALED_VERSION");
    assert_eq!(select.key_sha256, "ab".repeat(32), "S6C4_SELECT_DIGEST");
    notifier.acknowledge(select.nonce + 1);
    assert_eq!(
        notifier.status().select,
        Some(select.clone()),
        "S6C4_OTHER_NONCE_KEEPS"
    );
    notifier.acknowledge(select.nonce);
    assert_eq!(notifier.status().select, None, "S6C4_ACKNOWLEDGED");
    // A second click on the same notification selects nothing new.
    click(&server, id);
    handled_after(&notifier, &server, id + 202);
    assert_eq!(
        notifier.status().select,
        None,
        "S6C4_SECOND_CLICK_SELECTS_NOTHING"
    );
    assert!(
        super::lock(&notifier.shared).listening,
        "S6C4_STILL_LISTENING"
    );
    notifier.take_stop().finish();
    assert!(
        !super::lock(&notifier.shared).listening,
        "S6C4_STOP_ENDS_THE_WAIT"
    );
}

#[test]
fn ja1_s6_c4_a_closed_notification_can_no_longer_be_clicked() {
    let bus = PrivateBus::start();
    let (daemon, server) = bus.daemon(true);
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v3", true));
    wait_for(|| daemon.calls.lock().unwrap().len() == 1, "S6C4_SENT");
    let id = daemon.calls.lock().unwrap()[0].id;
    wait_for(
        || super::lock(&notifier.shared).outstanding.contains_key(&id),
        "S6C4_OUTSTANDING",
    );
    closed(&server, id);
    wait_for(
        || !super::lock(&notifier.shared).listening,
        "S6C4_CLOSED_ENDS_THE_WAIT",
    );
    click(&server, id);
    assert_eq!(
        notifier.status().select,
        None,
        "S6C4_CLOSED_SELECTS_NOTHING"
    );
    notifier.take_stop().finish();
}

#[test]
fn ja1_s6_c4_a_missing_or_refusing_bus_is_reported_in_status() {
    let mut missing = Notifier::new(Some(PrivateBus::refused()));
    missing.notify(notice("v1", true));
    wait_for(
        || missing.status().error.is_some(),
        "S6C4_MISSING_BUS_REPORTED",
    );
    missing.take_stop().finish();

    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(true);
    daemon.refuse.store(true, Ordering::SeqCst);
    let mut refusing = Notifier::new(Some(bus.connect()));
    refusing.notify(notice("v1", false));
    wait_for(
        || refusing.status().error.is_some(),
        "S6C4_REFUSAL_REPORTED",
    );
    assert!(
        refusing
            .status()
            .error
            .unwrap()
            .contains("refused by the test"),
        "S6C4_REFUSAL_TEXT"
    );
    // A later notification that is sent clears it.
    daemon.refuse.store(false, Ordering::SeqCst);
    refusing.notify(notice("v2", false));
    wait_for(
        || refusing.status().error.is_none(),
        "S6C4_SENT_CLEARS_ERROR",
    );
    refusing.take_stop().finish();
}

#[test]
fn ja1_s6_c4_seals_during_a_send_wait_in_a_bounded_queue() {
    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(true);
    daemon.hold.store(true, Ordering::SeqCst);
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v1", false));
    wait_for(
        || daemon.entered.load(Ordering::SeqCst),
        "S6C4_SEND_IN_FLIGHT",
    );
    // The first is in flight; MAX_QUEUED wait, and two more drop the oldest.
    for n in 2..=(MAX_QUEUED + 3) {
        notifier.notify(notice(&format!("v{n}"), false));
    }
    assert_eq!(notifier.status().dropped, 2, "S6C4_DROPPED_COUNTED");
    daemon.hold.store(false, Ordering::SeqCst);
    // Sent before the stop, which would drop what is still queued.
    wait_for(
        || daemon.calls.lock().unwrap().len() == MAX_QUEUED + 1,
        "S6C4_QUEUE_DRAINED",
    );
    notifier.take_stop().finish();
    let bodies: Vec<String> = daemon
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| call.body.clone())
        .collect();
    assert_eq!(
        bodies,
        ["v1", "v4", "v5", "v6", "v7"]
            .map(|label| format!("{label} is delivered"))
            .to_vec(),
        "S6C4_QUEUE_DROPS_THE_OLDEST"
    );
}

#[test]
fn ja1_s6_c4_stop_sends_what_is_queued_then_joins_and_takes_nothing_new() {
    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(true);
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v1", true));
    let stop = notifier.take_stop();
    stop.finish();
    assert_eq!(
        daemon.calls.lock().unwrap().len(),
        1,
        "S6C4_QUEUED_SENT_ON_STOP"
    );
    {
        let shared = super::lock(&notifier.shared);
        assert!(!shared.sending, "S6C4_SENDER_JOINED");
        assert!(shared.listener.is_none(), "S6C4_LISTENER_JOINED");
        assert!(shared.bus.is_none(), "S6C4_BUS_CLOSED");
    }
    notifier.notify(notice("v2", false));
    assert!(notifier.sender.is_none(), "S6C4_NOTHING_AFTER_STOP");
    assert_eq!(daemon.calls.lock().unwrap().len(), 1, "S6C4_NOTHING_NEW");
}

// JA-1 S6 C4 fix 1: a hung bus holds a stop for the one send in flight; the
// queued seals are dropped and counted.
#[test]
fn ja1_s6_c4_fix1_stop_finishes_only_the_send_in_flight() {
    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(true);
    daemon.hold.store(true, Ordering::SeqCst);
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v1", false));
    wait_for(
        || daemon.entered.load(Ordering::SeqCst),
        "S6C4F1_SEND_IN_FLIGHT",
    );
    notifier.notify(notice("v2", false));
    notifier.notify(notice("v3", false));
    let stop = notifier.take_stop();
    assert_eq!(
        notifier.status().dropped,
        2,
        "S6C4F1_QUEUED_DROPPED_ON_STOP"
    );
    daemon.hold.store(false, Ordering::SeqCst);
    stop.finish();
    let bodies: Vec<String> = daemon
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| call.body.clone())
        .collect();
    assert_eq!(
        bodies,
        vec!["v1 is delivered".to_owned()],
        "S6C4F1_ONLY_THE_SEND_IN_FLIGHT"
    );
}

// JA-1 S6 C4 fix 1: the GUI is read when the notice is sent. A notice queued
// while a GUI was attached, and sent after a detach or after that GUI is
// gone, offers no action.
#[test]
fn ja1_s6_c4_fix1_the_gui_is_read_when_the_notice_is_sent() {
    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(true);
    daemon.hold.store(true, Ordering::SeqCst);
    let mut notifier = Notifier::new(Some(bus.connect()));
    let alive = Arc::new(AtomicBool::new(true));
    notifier.notify(notice_for("v1", Some(&alive)));
    wait_for(
        || daemon.entered.load(Ordering::SeqCst),
        "S6C4F1_SEND_IN_FLIGHT",
    );
    let gone = Arc::new(AtomicBool::new(true));
    notifier.notify(notice_for("v2", Some(&alive)));
    notifier.notify(notice_for("v3", Some(&gone)));
    notifier.set_detached(true);
    // Read at v3's send too: its GUI is gone even after a new attachment.
    gone.store(false, Ordering::SeqCst);
    daemon.hold.store(false, Ordering::SeqCst);
    wait_for(|| daemon.calls.lock().unwrap().len() >= 2, "S6C4F1_V2_SENT");
    notifier.set_detached(false);
    wait_for(|| daemon.calls.lock().unwrap().len() >= 3, "S6C4F1_V3_SENT");
    let actions: Vec<(String, Vec<String>)> = daemon
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| (call.body.clone(), call.actions.clone()))
        .collect();
    let offered = vec![DEFAULT_ACTION.to_owned(), "Show".to_owned()];
    assert_eq!(
        actions,
        vec![
            ("v1 is delivered".to_owned(), offered),
            ("v2 is delivered".to_owned(), Vec::new()),
            ("v3 is delivered".to_owned(), Vec::new()),
        ],
        "S6C4F1_ACTION_ONLY_WHILE_ATTACHED_AT_SEND"
    );
    notifier.take_stop().finish();
}

#[test]
fn ja1_s6_c4_a_notifier_without_a_bus_does_nothing() {
    let mut notifier = Notifier::default();
    notifier.notify(notice("v1", true));
    assert!(notifier.sender.is_none(), "S6C4_NO_SENDER");
    assert_eq!(notifier.status(), super::NotifyStatus::default());
}
