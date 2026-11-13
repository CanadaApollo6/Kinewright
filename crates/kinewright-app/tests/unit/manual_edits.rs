//! Astra's real-Core missed-wake-up and duplicate-Undo probes, retained in the App.
use super::ManualEdits;
use kinewright_core::*;
use std::time::{Duration, Instant};
pub(crate) struct Proof;
struct Encoder;
impl ManualHistoryEncoder for Encoder {
    fn check_history(&mut self, _: &HistorySnapshot) -> Result<(), HistoryError> {
        Ok(())
    }
    fn finish_history_check(&mut self, _: bool) {}
    fn encode(
        &self,
        sequence: u64,
        _: TimelineRevision,
        _: Option<&str>,
        _: ManualHistoryCandidate<'_>,
    ) -> Result<AcceptedHistory, HistoryError> {
        let mut item = AcceptedHistory {
            sequence,
            batch_id: sequence.to_string(),
            digest: String::new(),
            envelope: vec![],
        };
        let size = serde_json::to_vec(&item).unwrap().len();
        item.digest = "x".repeat(MANUAL_BYTES - size);
        Ok(item)
    }
}
pub(crate) fn fixture() -> (Core, CoreAttachment<Proof>, ManualPermit, ManualEdits) {
    fixture_with_document(Document::default())
}
pub(crate) fn fixture_with_document(
    document: Document,
) -> (Core, CoreAttachment<Proof>, ManualPermit, ManualEdits) {
    let core = Core::spawn(document).unwrap();
    let owner = core.history_save_owner(&Proof).unwrap();
    let permit = owner.manual_permit(&Proof, Box::new(Encoder)).unwrap();
    (core, owner, permit, ManualEdits::default())
}
pub(crate) fn add(id: u64) -> Command {
    Command::Do(Operation::AddTrack {
        track: Track {
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
pub(crate) fn ack(owner: &CoreAttachment<Proof>, permit: &ManualPermit, edits: &mut ManualEdits) {
    owner
        .acknowledge_manual(&Proof, permit, permit.pending().unwrap())
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
        Event::MutationRefused {
            reason: CoreMutationRefusal::ManualBackpressure,
            ..
        }
    ));
    ack(&owner, &permit, &mut edits);
    collect(&mut edits);
    assert!(!edits.waiting_for_ack(), "ASTRA_MISSED_WAKEUP");
    edits.start_next(&permit);
    collect(&mut edits);
    edits.start_next(&permit);
    assert!(!edits.pending());
    assert_eq!(core.export_history().unwrap().document.tracks.len(), 2);
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
    assert_eq!(h.document.tracks.len(), 1, "UNDO_MUST_EXECUTE_EXACTLY_ONCE");
    assert_eq!(h.document.tracks[0].id, TrackId(1));
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
    assert_eq!(core.export_history().unwrap().document.tracks.len(), 17);
}
