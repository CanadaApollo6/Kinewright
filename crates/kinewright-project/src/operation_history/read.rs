//! A validated cold history has a capture lease, without becoming a live task.
//! Consuming queries keep that same independent reader lease; the source Core
//! and every capture or other reader keep their own full reservations.
use super::{
    ApprovalAnswer, ApprovalLedger, Error, OwnerBoundary, OwnerHistory, OwnerOperations,
    OwnerSnapshot, Resources, snapshot::SnapshotData,
};
use crate::bundle_records::ObjectRef;

#[derive(Debug)]
pub struct OwnerRead(OwnerHistory);
impl OwnerRead {
    pub(super) const fn new(history: OwnerHistory) -> Self {
        Self(history)
    }
    #[must_use]
    pub const fn history(&self) -> &kinewright_core::undo::History {
        &self.0.core
    }
    #[must_use]
    pub const fn ledger(&self) -> &ApprovalLedger {
        &self.0.ledger
    }
    #[must_use]
    pub const fn resources(&self) -> &Resources {
        &self.0.resources
    }
    pub fn identity(&self) -> Result<super::SnapshotIdentity, Error> {
        self.0.identity()
    }
    #[must_use]
    pub fn creation_sizing(&self) -> super::CreationSizing {
        self.0.creation_sizing()
    }
    pub fn approval_answer(&self, reference: &ObjectRef) -> Result<Option<ApprovalAnswer>, Error> {
        self.0.approval_answer(reference)
    }
    pub fn into_boundary(self, depth: usize) -> Result<OwnerBoundary, Error> {
        self.0.into_boundary(depth)
    }
    pub fn into_operations(self) -> Result<OwnerOperations, Error> {
        let _window = self.0.reservation.workspace()?;
        Ok(OwnerOperations {
            operations: self.0.core.into_applied_delta(),
            _reservation: self.0.reservation,
        })
    }
    pub fn into_capture(self) -> Result<OwnerSnapshot, Error> {
        let _window = self.0.reservation.workspace()?;
        let seeds = self.0.seeds();
        let resources = self.0.resources.values().cloned().collect();
        let OwnerHistory {
            core,
            ledger,
            folded_approvals,
            origins,
            reservation,
            fold_frontier,
            terminal_facts,
            ..
        } = self.0;
        let data = SnapshotData {
            role: reservation.role(),
            sizing: reservation.sizing(),
            fold_frontier,
            terminal_facts,
            history: core.into_record(),
            seeds,
            origins: origins.0.into_iter().map(|row| row.origin).collect(),
            resources,
            ledger,
            folded_approvals,
        };
        Ok(OwnerSnapshot { data, reservation })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_history::{
        HistoryPool, PoolLimits, Role,
        tests::{document, origin},
    };
    use kinewright_core::{Command, Operation, TrackId};
    fn lock(value: bool) -> Operation {
        Operation::SetTrackSyncLock {
            track: TrackId(81),
            locked: value,
        }
    }
    fn make(pool: &HistoryPool, role: Role) -> OwnerHistory {
        OwnerHistory::new(pool, role, role.policy(), &origin(1), || Ok(document())).unwrap()
    }
    #[test]
    fn cold_queries_need_one_reader_lease_even_when_the_pool_is_full() {
        for (index, role) in [Role::Ordinary, Role::Task, Role::Candidate]
            .into_iter()
            .enumerate()
        {
            let probe = make(&HistoryPool::new(PoolLimits::default()), role);
            let bytes = probe.reservation.limits().envelope.retained;
            drop(probe);
            let mut limits = PoolLimits::default();
            limits.retained[index] = bytes * 3;
            limits.peak[index] = bytes * 3 + limits.workspace;
            let pool = HistoryPool::new(limits);
            let mut source = make(&pool, role);
            let sibling = make(&pool, role);
            source
                .execute(Command::Do(lock(true)), origin(2), vec![], None)
                .unwrap();
            source
                .execute(Command::Do(lock(false)), origin(3), vec![], None)
                .unwrap();
            source
                .execute(Command::Undo, origin(4), vec![], None)
                .unwrap();
            let expected = source.identity().unwrap();
            let reader = source
                .capture()
                .unwrap()
                .validate()
                .unwrap()
                .into_reader()
                .expect("COLD_READ_USES_THE_EXISTING_CHECKPOINT_ROOM");
            assert_eq!(reader.identity().unwrap(), expected);
            let boundary = reader
                .into_boundary(1)
                .expect("CONSUMING_BOUNDARY_NEVER_REQUESTS_A_SECOND_READER_LEASE");
            assert!(boundary.document().tracks[0].sync_lock);
            assert_eq!(
                pool.lease_counts(role).unwrap(),
                (3, 3),
                "CONSUMING_BOUNDARY_RETAINS_ITS_ONE_FULL_QUERY_LEASE"
            );
            drop(boundary);
            let reader = source
                .capture()
                .unwrap()
                .validate()
                .unwrap()
                .into_reader()
                .unwrap();
            let operations = reader.into_operations().unwrap();
            assert_eq!(
                &*operations,
                &[lock(true)],
                "CONSUMING_DELTA_EXCLUDES_THE_REDO_TAIL"
            );
            assert_eq!(
                pool.lease_counts(role).unwrap(),
                (3, 3),
                "CONSUMING_DELTA_RETAINS_ITS_ONE_FULL_QUERY_LEASE"
            );
            drop(operations);
            let reader = source
                .capture()
                .unwrap()
                .validate()
                .unwrap()
                .into_reader()
                .unwrap();
            let capture = reader.into_capture().unwrap();
            assert_eq!(
                capture.identity().unwrap(),
                expected,
                "COLD_READER_CAPTURE_PRESERVES_BOTH_STACKS_AND_OWNER_IDENTITY"
            );
            assert_eq!(pool.lease_counts(role).unwrap(), (3, 3));
            drop(capture);
            assert_eq!(
                source.identity().unwrap(),
                expected,
                "CONSUMING_READERS_NEVER_CHANGE_THE_SOURCE_CORE"
            );
            drop((source, sibling));
            assert_eq!(
                pool.usage(role).unwrap(),
                (0, 0),
                "LAST_COLD_READER_DROP_RELEASES_ITS_CAPTURE_ROOM"
            );
        }
    }
}
