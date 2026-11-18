use super::*;
use crate::{SolidColor, TimeCode, Track, TrackId, TrackKind};

fn document() -> Document {
    Document {
        tracks: vec![Track {
            name: None,
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    }
}
fn lock(value: bool) -> Operation {
    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: value,
    }
}
fn container_capacity(effects: &Effects) -> u64 {
    let commands = effects.applied.capacity() + effects.folded.capacity();
    let keys =
        effects.to_undo.capacity() + effects.to_redo.capacity() + effects.discarded.capacity();
    commands as u64 * std::mem::size_of::<CommandEffects>() as u64
        + keys as u64 * std::mem::size_of::<CommandKey>() as u64
        + effects
            .applied
            .iter()
            .chain(&effects.folded)
            .map(|command| {
                command.identities.capacity() as u64
                    * std::mem::size_of::<OperationIdentityEffect>() as u64
            })
            .sum::<u64>()
}
#[test]
fn observation_charge_covers_real_containers_for_batches_and_coalesced_redo() {
    for count in [1, 12, 120, 1000] {
        let mut history =
            History::new(document(), RetentionPolicy::NeverFold, RoleBounds::TASK).unwrap();
        let effects = history
            .execute(Command::DoBatch(
                (0..count).map(|i| lock(i % 2 == 0)).collect(),
            ))
            .unwrap();
        assert!(
            history.undo[0].charge.effects_bytes >= container_capacity(&effects),
            "BATCH_OBSERVATION_CHARGE_COVERS_ACTUAL_VECTOR_CAPACITIES count={count}"
        );
    }
    let mut history =
        History::new(document(), RetentionPolicy::NeverFold, RoleBounds::TASK).unwrap();
    for i in 0..120 {
        history
            .execute(Command::DoBatchCoalesced {
                operations: vec![lock(i % 2 == 0)],
                coalesce_key: "gesture".into(),
            })
            .unwrap();
    }
    let reserved = history.undo[0].charge.effects_bytes;
    let undone = history.execute(Command::Undo).unwrap();
    assert!(
        reserved >= container_capacity(&undone),
        "COALESCED_UNDO_OBSERVATION_CONTAINERS_COVERED"
    );
    let redone = history.execute(Command::Redo).unwrap();
    assert!(
        reserved >= container_capacity(&redone),
        "COALESCED_REDO_OBSERVATION_CONTAINERS_COVERED"
    );
}
#[test]
fn workspace_crossing_tightens_a_loose_document_bound_without_poisoning_retry() {
    let limits = RoleBounds {
        replay_workspace: 32 << 10,
        checkpoint_documents: 0,
        operation_bytes: 4096,
        ..RoleBounds::ORDINARY
    };
    let mut history = History::new(document(), RetentionPolicy::Fold, limits).unwrap();
    history
        .execute(Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(0),
            duration: TimeCode(10),
            color: SolidColor {
                r: 20,
                g: 40,
                b: 60,
            },
        }))
        .unwrap();
    let before = PhaseCounters::read();
    history
        .execute(Command::Do(Operation::SplitClip {
            clip: crate::ClipId(1),
            at: TimeCode(5),
        }))
        .expect("WORKSPACE_CROSSING_TIGHTENS_BOUND_AND_ACCEPTS_VALID_EDIT");
    let phase = PhaseCounters::read().since(before);
    assert_eq!(
        phase.document_copies, 1,
        "WORKSPACE_RETRY_HAS_ONE_CANDIDATE_COPY"
    );
    assert_eq!(
        phase.exact_measurements, 1,
        "EXACT_MEASUREMENT_ONLY_AT_WORKSPACE_CROSSING"
    );
    assert!(
        history.document_bound.live >= document_capacity_bytes(history.document()).unwrap(),
        "WORKSPACE_RETRY_BOUND_COVERS_REAL_CAPACITY"
    );
}
