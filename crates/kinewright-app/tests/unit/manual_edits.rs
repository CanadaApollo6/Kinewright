//! Astra's real-Core missed-wake-up and duplicate-Undo probes, retained in the App.
use super::ManualEdits;
use kinewright_core::*;
use kinewright_project::operation_actor::Core;
use std::time::{Duration, Instant};
pub(crate) struct Proof;
use kinewright_project::{
    bundle_records::{HeadId, ProjectId},
    operation_actor::{Attachment, Error, ManualPermit, Refusal},
    operation_inputs::ManualInputs,
    operation_protocol::{Lease, PENDING_COMMANDS, Receipt, Seed, StreamState},
    receipts::IssuerId,
};
use std::sync::{Arc, atomic::AtomicBool};
pub(crate) fn fixture() -> (Core, Attachment<Proof>, ManualPermit, ManualEdits) {
    fixture_with_document(Document::default())
}
pub(crate) fn fixture_with_document(
    document: Document,
) -> (Core, Attachment<Proof>, ManualPermit, ManualEdits) {
    let core = Core::for_test(document).unwrap();
    let owner = core.attach(&Proof).unwrap();
    let wire = core.portable_history().unwrap();
    let identity = wire.identity();
    let inputs = ManualInputs::fixture();
    let lease = Lease {
        state: StreamState::installed(Seed {
            project: ProjectId::fresh().unwrap(),
            branch: wire.branch.clone(),
            attachment_epoch: 1,
            installed: identity,
        })
        .unwrap(),
        snapshot: identity,
        next_key: core.change_id().unwrap().position,
        issuer: serde_json::from_value::<IssuerId>(serde_json::json!("01".repeat(16))).unwrap(),
        next_issuer_sequence: 1,
        generation: 1,
        client: "gui".into(),
        token: "test-authority".into(),
        switch_generation: 1,
        owner_epoch: 1,
        anchor_head: HeadId::fresh().unwrap(),
        inputs: inputs.reference().clone(),
        parent_identity: None,
        namespace_name: None,
    };
    drop(wire);
    let permit = owner
        .manual_permit(&Proof, None, lease, inputs, Arc::new(AtomicBool::new(true)))
        .unwrap();
    // Leave one slot. The fixture's first real edit fills the actual protocol window.
    for _ in 1..PENDING_COMMANDS {
        permit.close_gesture().unwrap();
    }
    (core, owner, permit, ManualEdits::default())
}
pub(crate) fn add(id: u64) -> Command {
    Command::Do(Operation::AddTrack {
        track: Track {
            name: None,
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        },
    })
}
pub(crate) fn collect(edits: &mut ManualEdits) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while edits.attempt_running() {
        edits.collect_ready();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}
pub(crate) fn ack(owner: &Attachment<Proof>, permit: &ManualPermit, edits: &mut ManualEdits) {
    owner
        .acknowledge_manual(
            &Proof,
            permit,
            permit
                .pending()
                .unwrap()
                .into_iter()
                .map(|encoded| {
                    let message = encoded.decode().unwrap();
                    Receipt {
                        additional_batches: message.folded.len() as u64,
                        id: message.id.clone(),
                        sequence: message.sequence,
                        accepted: message.accepted,
                        tip: message.tip,
                        command: message.digest().unwrap(),
                    }
                })
                .collect(),
        )
        .unwrap();
    edits.acknowledged();
}
#[test]
fn refusal_collected_after_curing_ack_retries_without_another_ack() {
    let (core, owner, permit, mut edits) = fixture();
    assert!(matches!(
        permit.request(add(1)).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let events = core.subscribe().unwrap();
    core.export_history().unwrap();
    for _ in events.try_iter() {}
    edits.submit(add(2), &permit).unwrap();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(10)).unwrap(),
        Err(Refusal {
            error: Error::Protocol(kinewright_project::operation_protocol::Conflict::Backpressure),
            ..
        })
    ));
    ack(&owner, &permit, &mut edits);
    collect(&mut edits);
    assert!(!edits.waiting_for_ack(), "ASTRA_MISSED_WAKEUP");
    edits.start_next(&permit);
    collect(&mut edits);
    edits.start_next(&permit);
    assert!(!edits.pending());
    assert_eq!(
        core.export_history()
            .unwrap()
            .history()
            .document
            .tracks
            .len(),
        2
    );
}
#[test]
fn accepted_command_never_replayed_by_extra_acks_or_polls() {
    let (core, owner, permit, mut edits) = fixture();
    permit.request(add(1)).unwrap();
    ack(&owner, &permit, &mut edits);
    permit.request(add(2)).unwrap();
    ack(&owner, &permit, &mut edits);
    edits.submit(Command::Undo, &permit).unwrap();
    collect(&mut edits);
    for _ in 0..5 {
        ack(&owner, &permit, &mut edits);
        edits.start_next(&permit);
        collect(&mut edits);
    }
    let h = core.export_history().unwrap();
    assert_eq!(
        h.history().document.tracks.len(),
        1,
        "UNDO_MUST_EXECUTE_EXACTLY_ONCE"
    );
    assert_eq!(h.history().document.tracks[0].id, TrackId(1));
    assert!(!edits.pending());
    assert_eq!(edits.message(), None);
}

#[test]
fn full_notice_clears_as_soon_as_the_queue_has_room() {
    let (core, owner, permit, mut edits) = fixture();
    permit.request(add(1)).unwrap();
    for id in 2..=17 {
        edits.submit(add(id), &permit).unwrap();
    }
    collect(&mut edits);
    assert_eq!(edits.action_count(), super::PENDING_ACTIONS);
    assert_eq!(
        edits.submit(add(18), &permit),
        Err(super::ManualSendError::PendingCapacity)
    );
    assert_eq!(edits.notice(), Some(super::FULL_WORDS));
    ack(&owner, &permit, &mut edits);
    edits.start_next(&permit);
    collect(&mut edits);
    edits.start_next(&permit);
    assert!(edits.action_count() < super::PENDING_ACTIONS);
    assert_ne!(
        edits.notice(),
        Some(super::FULL_WORDS),
        "MANUAL_FULL_NOTICE_CLEARS_WITH_ROOM"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while edits.pending() {
        assert!(Instant::now() < deadline);
        collect(&mut edits);
        ack(&owner, &permit, &mut edits);
        edits.start_next(&permit);
    }
    assert_eq!(
        edits.message(),
        None,
        "MANUAL_DRAINED_QUEUE_HAS_NO_FULL_NOTICE"
    );
    assert_eq!(
        core.export_history()
            .unwrap()
            .history()
            .document
            .tracks
            .len(),
        17
    );
}

#[test]
fn abandoned_acceptance_invalidates_a_stale_numeric_return() {
    let (core, owner, permit, mut edits) = fixture();
    let Event::DocumentChanged { revision, .. } = permit.request(add(1)).unwrap() else {
        panic!("track")
    };
    ack(&owner, &permit, &mut edits);
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    edits.after_send = Some(Box::new(move || {
        held_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(120)).unwrap();
    }));
    edits
        .submit(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }),
            &permit,
        )
        .unwrap();
    held_rx.recv_timeout(Duration::from_secs(120)).unwrap();
    edits.abandon_unaccepted();
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while edits.worker_owned() {
        assert!(Instant::now() < deadline);
        edits.collect_ready();
        std::thread::yield_now();
    }
    ack(&owner, &permit, &mut edits);
    edits
        .submit_value(
            Command::DoBatch(vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            }]),
            Some(revision),
            &permit,
        )
        .unwrap();
    collect(&mut edits);
    edits.start_next(&permit);
    assert!(
        core.export_history().unwrap().history().document.tracks[0].sync_lock,
        "ABANDONED_ACCEPTANCE_INVALIDATES_STALE_RETURN"
    );
    assert!(!edits.pending());
    assert_eq!(edits.notice(), None);
}
#[test]
fn unconfirmed_abandoned_worker_preserves_the_unapplied_count() {
    for panicked in [false, true] {
        let mut edits = ManualEdits::default();
        edits.note_unapplied(3);
        let worker = std::thread::spawn(move || -> Result<Event, Refusal> {
            assert!(!panicked, "injected abandoned worker panic");
            Err(Refusal {
                error: Error::Disconnected,
                token: None,
            })
        });
        edits.in_flight = Some(super::InFlight {
            command: super::PendingValue {
                command: add(1),
                unchanged_at: None,
            },
            acknowledgement: 0,
            worker: crate::joined_thread::JoinedThread::new(worker),
            disposition: super::Disposition::Abandoned {
                count_if_refused: true,
            },
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while edits.worker_owned() {
            assert!(Instant::now() < deadline);
            edits.collect_ready();
            std::thread::yield_now();
        }
        let notice = edits.notice().unwrap();
        assert!(
            notice.contains("3 hand edits were not applied."),
            "ABANDONED_UNKNOWN_KEEPS_COUNT: {notice}"
        );
        assert!(
            notice.contains("outcome could not be confirmed"),
            "{notice}"
        );
        assert!(!edits.pending());
    }
}
