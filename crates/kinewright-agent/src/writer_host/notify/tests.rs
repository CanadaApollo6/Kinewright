// JA-1 S6 C4: the notifier against a fake daemon on a private bus.
use std::sync::atomic::Ordering;

use kinewright_core::output_review::SealedReviewOutcome;
use kinewright_project::{bundle_records::RecordKind, studio_records::SnapshotKey};

use super::{
    MAX_QUEUED, Notifier, SealNotice,
    test_bus::{PrivateBus, click, emit, wait_for},
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
    SealNotice::new(label, &delivered(), gui.then(|| (key(3), "ab".repeat(32))))
}

#[test]
fn ja1_s6_c4_a_seal_notifies_once_with_its_label_and_a_click_action() {
    let bus = PrivateBus::start();
    let (daemon, _server) = bus.daemon(true);
    let mut notifier = Notifier::new(Some(bus.connect()));
    notifier.notify(notice("v3", true));
    wait_for(|| daemon.calls.lock().unwrap().len() == 1, "S6C4_SENT");
    notifier.notify(SealNotice::new("v4", &facts_not_checked(), None));
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
fn handled_after(notifier: &Notifier, from: &zbus::blocking::Connection, id: u32) {
    super::lock(&notifier.shared).outstanding.insert(
        id,
        super::Outstanding {
            daemon: from.unique_name().unwrap().clone(),
            target: super::Target {
                version: key(9),
                key_sha256: "cd".repeat(32),
            },
        },
    );
    emit(from, "NotificationClosed", &(id, 2_u32));
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
    emit(&server, "ActionInvoked", &(id, "Show"));
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
    emit(&server, "NotificationClosed", &(id, 2_u32));
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

#[test]
fn ja1_s6_c4_a_notifier_without_a_bus_does_nothing() {
    let mut notifier = Notifier::default();
    notifier.notify(notice("v1", true));
    assert!(notifier.sender.is_none(), "S6C4_NO_SENDER");
    assert_eq!(notifier.status(), super::NotifyStatus::default());
}
