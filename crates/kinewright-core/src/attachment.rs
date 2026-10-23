//! Runtime attachment authority. All decisions execute on the owning actor.
#![allow(clippy::missing_errors_doc)]
use crate::{Core, HistoryError, HistorySnapshot, HistoryTransition, TimelineRevision};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActorIdentity(pub(crate) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentState {
    Live,
    Frozen,
    Sealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CoreMutationRefusal {
    #[error("editing is paused while the project is checked")]
    Frozen,
    #[error("this editing session is read-only after the handoff")]
    Sealed,
    #[error("history installation requires the bound attachment owner")]
    OwnerRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedCut {
    pub(crate) actor: ActorIdentity,
    pub(crate) revision: TimelineRevision,
    pub(crate) history: HistorySnapshot,
}
impl AcceptedCut {
    #[must_use]
    pub const fn actor(&self) -> ActorIdentity {
        self.actor
    }
    #[must_use]
    pub const fn revision(&self) -> TimelineRevision {
        self.revision
    }
    #[must_use]
    pub const fn history(&self) -> &HistorySnapshot {
        &self.history
    }
}

/// The proof type's constructor stays private to the persistence owner.
pub struct CoreAttachment<P> {
    pub(crate) core: Core,
    proof: std::marker::PhantomData<fn(P)>,
}
impl<P: 'static> CoreAttachment<P> {
    pub(crate) const fn new(core: Core) -> Self {
        Self {
            core,
            proof: std::marker::PhantomData,
        }
    }
    pub fn capture(&self, _proof: &P) -> Result<AcceptedCut, HistoryError> {
        self.core.attachment_cut::<P>(false)
    }
    pub fn freeze(&self, _proof: &P) -> Result<AcceptedCut, HistoryError> {
        self.core.attachment_cut::<P>(true)
    }
    pub fn resume(&self, _proof: &P, cut: &AcceptedCut) -> Result<(), HistoryError> {
        self.core.finish_attachment::<P>(cut.clone(), false)
    }
    pub fn seal(&self, _proof: &P, cut: &AcceptedCut) -> Result<(), HistoryError> {
        self.core.finish_attachment::<P>(cut.clone(), true)
    }
    pub fn restore_history(
        &self,
        _proof: &P,
        history: HistorySnapshot,
    ) -> Result<(), HistoryError> {
        self.core.restore_authorized::<P>(history, None)
    }
    pub fn install_checkpoint(
        &self,
        _proof: &P,
        before: HistorySnapshot,
        after: HistorySnapshot,
    ) -> Result<(), HistoryError> {
        self.core.restore_authorized::<P>(after, Some(before))
    }
    pub fn install_history_transition(
        &self,
        proof: &P,
        transition: HistoryTransition,
    ) -> Result<(), HistoryError> {
        self.install_checkpoint(proof, transition.before, transition.after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Event, HistorySaveTag, Operation, Query, Track, TrackId, TrackKind};
    struct Proof;
    struct Stranger;
    fn edit(id: u64) -> Command {
        Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        })
    }
    fn history_error(reason: CoreMutationRefusal) -> HistoryError {
        HistoryError::MutationRefused(reason)
    }
    #[test]
    fn m01_eager_binding_and_install_capability_cannot_be_bypassed() {
        let core = Core::spawn(crate::Document::default()).unwrap();
        let owner = core.history_save_owner(&Proof).unwrap();
        let empty = core.export_history().unwrap();
        assert!(core.clone().history_save_owner(&Stranger).is_err());
        core.request(edit(1)).unwrap();
        let before = core.export_history().unwrap();
        let denied = Err(history_error(CoreMutationRefusal::OwnerRequired));
        assert_eq!(core.restore_history(empty.clone()), denied);
        assert_eq!(
            core.install_checkpoint(before.clone(), empty.clone()),
            denied
        );
        assert_eq!(
            core.install_history_transition(HistoryTransition::floor(before.clone(), 1).unwrap()),
            denied
        );
        assert_eq!(core.export_history().unwrap(), before);
        assert!(
            owner
                .install_checkpoint(&Proof, empty.clone(), empty.clone())
                .is_err()
        );
        assert_eq!(core.export_history().unwrap(), before);
        owner
            .install_checkpoint(&Proof, before, empty.clone())
            .unwrap();
        assert_eq!(core.export_history().unwrap(), empty);
    }
    #[test]
    #[allow(clippy::too_many_lines)]
    fn w05_seal_checks_cut_and_all_mutation_paths() {
        let core = Core::spawn(crate::Document::default()).unwrap();
        let retained = core.clone();
        let owner = core.history_save_owner(&Proof).unwrap();
        let stale = owner.capture(&Proof).unwrap();
        retained.send(edit(1)).unwrap(); // accepted ahead of the actor barrier
        let cut = owner.freeze(&Proof).unwrap();
        assert_eq!(cut.history().batches.len(), 1);
        assert_eq!(
            owner.seal(&Proof, &stale),
            Err(HistoryError::CaptureChanged)
        );
        assert!(matches!(
            retained.request(edit(2)).unwrap(),
            Event::MutationRefused {
                reason: CoreMutationRefusal::Frozen,
                ..
            }
        ));
        let frozen = Err(history_error(CoreMutationRefusal::Frozen));
        assert_eq!(owner.restore_history(&Proof, cut.history().clone()), frozen);
        assert_eq!(
            owner.install_checkpoint(&Proof, cut.history().clone(), cut.history().clone()),
            frozen
        );
        assert_eq!(
            owner.install_history_transition(
                &Proof,
                HistoryTransition::floor(cut.history().clone(), 1).unwrap()
            ),
            frozen
        );
        owner.seal(&Proof, &cut).unwrap();
        let history = core.export_history().unwrap();
        let revision = cut.revision();
        let Command::Do(operation) = edit(2) else {
            unreachable!()
        };
        let commands = [
            edit(2),
            Command::DoBatch(vec![operation.clone()]),
            Command::DoBatchCoalesced {
                operations: vec![operation.clone()],
                coalesce_key: "gesture".into(),
            },
            Command::DoIfRevision {
                expected: revision,
                operation: operation.clone(),
                token: Some(crate::CommandToken(19)),
            },
            Command::DoBatchIfRevision {
                expected: revision,
                operations: vec![operation],
            },
            Command::Undo,
            Command::Redo,
        ];
        for command in commands {
            assert!(
                matches!(
                    retained.request(command.clone()).unwrap(),
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::Sealed,
                        ..
                    }
                ),
                "{command:?}"
            );
            retained.send(command).unwrap();
            assert_eq!(core.export_history().unwrap(), history);
        }
        let denied = Err(history_error(CoreMutationRefusal::Sealed));
        assert_eq!(core.restore_history(history.clone()), denied);
        assert_eq!(
            core.install_checkpoint(history.clone(), history.clone()),
            denied
        );
        assert_eq!(
            core.install_history_transition(HistoryTransition::floor(history.clone(), 1).unwrap()),
            denied
        );
        assert_eq!(owner.restore_history(&Proof, history.clone()), denied);
        assert_eq!(
            owner.install_checkpoint(&Proof, history.clone(), history.clone()),
            denied
        );
        assert_eq!(
            owner.install_history_transition(
                &Proof,
                HistoryTransition::floor(history.clone(), 1).unwrap()
            ),
            denied
        );
        assert!(matches!(
            owner.begin_history_floor(
                &Proof,
                HistoryTransition::floor(history.clone(), 1).unwrap()
            ),
            Err(HistoryError::MutationRefused(CoreMutationRefusal::Sealed))
        ));
        assert_eq!(owner.resume(&Proof, &cut), denied);
        assert!(matches!(
            core.request(Command::Query(Query::Snapshot)).unwrap(),
            Event::QueryResult(_)
        ));
        assert_eq!(core.pending_history_save().unwrap(), None);
        assert_eq!(core.export_history().unwrap(), history);
    }
    #[test]
    fn w09_tickets_reject_retag_and_matching_numbers_on_another_actor() {
        let a = Core::spawn(crate::Document::default()).unwrap();
        let b = Core::spawn(crate::Document::default()).unwrap();
        let owner_a = a.history_save_owner(&Proof).unwrap();
        let owner_b = b.history_save_owner(&Proof).unwrap();
        a.request(edit(1)).unwrap();
        b.request(edit(1)).unwrap();
        let mut first = owner_a
            .begin_history_floor(
                &Proof,
                HistoryTransition::floor(a.export_history().unwrap(), 1).unwrap(),
            )
            .unwrap();
        let mut second = owner_b
            .begin_history_floor(
                &Proof,
                HistoryTransition::floor(b.export_history().unwrap(), 1).unwrap(),
            )
            .unwrap();
        let tag = HistorySaveTag {
            destination: "a.kinewright".into(),
            parent_device: 1,
            parent_inode: 2,
            root_sha256: "a".repeat(64),
            previous_root_sha256: None,
        };
        first.tag_publication(&Proof, tag.clone()).unwrap();
        first.retain_until_reconcile(&Proof).unwrap();
        second.tag_publication(&Proof, tag.clone()).unwrap();
        second.retain_until_reconcile(&Proof).unwrap();
        let ticket_a = a.pending_history_save().unwrap().unwrap();
        let ticket_b = b.pending_history_save().unwrap().unwrap();
        assert_eq!(ticket_a.id, ticket_b.id);
        assert_eq!(
            owner_b.settle_history_save(&Proof, &ticket_a, true),
            Err(HistoryError::BarrierInvariant)
        );
        assert_eq!(b.pending_history_save().unwrap(), Some(ticket_b));
        let mut changed = tag;
        changed.root_sha256 = "b".repeat(64);
        assert_eq!(
            first.tag_publication(&Proof, changed),
            Err(HistoryError::BarrierInvariant)
        );
        assert_eq!(a.pending_history_save().unwrap(), Some(ticket_a));
    }

    #[test]
    fn w05_sealed_ticket_rejects_tag_commit_release_and_cross_actor_cut() {
        let core = Core::spawn(crate::Document::default()).unwrap();
        let owner = core.history_save_owner(&Proof).unwrap();
        core.request(edit(1)).unwrap();
        let before = core.export_history().unwrap();
        let mut barrier = owner
            .begin_history_floor(&Proof, HistoryTransition::floor(before, 1).unwrap())
            .unwrap();
        let tag = HistorySaveTag {
            destination: "a.kinewright".into(),
            parent_device: 1,
            parent_inode: 2,
            root_sha256: "a".repeat(64),
            previous_root_sha256: None,
        };
        barrier.tag_publication(&Proof, tag.clone()).unwrap();
        barrier.retain_until_reconcile(&Proof).unwrap();
        let pending = core.pending_history_save().unwrap();
        let cut = owner.freeze(&Proof).unwrap();
        let other = Core::spawn(crate::Document::default()).unwrap();
        let other_owner = other.history_save_owner(&Proof).unwrap();
        other.request(edit(1)).unwrap();
        let other_cut = other_owner.freeze(&Proof).unwrap();
        assert_eq!(other_cut.history(), cut.history());
        assert_eq!(other_cut.revision(), cut.revision());
        assert_eq!(
            other_owner.seal(&Proof, &cut),
            Err(HistoryError::CaptureChanged)
        );
        owner.seal(&Proof, &cut).unwrap();
        let denied = Err(history_error(CoreMutationRefusal::Sealed));
        assert_eq!(barrier.tag_publication(&Proof, tag), denied);
        assert_eq!(barrier.retain_until_reconcile(&Proof), denied);
        assert_eq!(barrier.commit(&Proof), denied);
        assert_eq!(barrier.release(), denied);
        assert_eq!(
            owner.settle_history_save(&Proof, pending.as_ref().unwrap(), true),
            denied
        );
        assert_eq!(core.pending_history_save().unwrap(), pending);
        drop(barrier);
        assert_eq!(core.pending_history_save().unwrap(), pending);
    }
}
