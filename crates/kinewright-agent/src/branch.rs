use std::{collections::BTreeSet, sync::Arc};

use kinewright_core::{
    BatchError, Command, Core, CoreDisconnected, Document, Event, OpError, Operation, Query,
    QueryResult, TimelineRevision,
};
use thiserror::Error;

/// An isolated edit lineage owned by one agent thread.
#[derive(Clone)]
pub struct TimelineBranch {
    name: Arc<str>,
    base_revision: TimelineRevision,
    base_document: Arc<Document>,
    core: Core,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchComparison {
    pub name: Arc<str>,
    pub base_revision: TimelineRevision,
    pub branch_revision: TimelineRevision,
    pub base_document: Arc<Document>,
    pub document: Arc<Document>,
    pub operations: Arc<Vec<Operation>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchApplyOutcome {
    NoChanges,
    Applied {
        revision: TimelineRevision,
        document: Arc<Document>,
        operation_count: usize,
    },
    Conflict {
        expected: TimelineRevision,
        actual: TimelineRevision,
    },
    Rejected {
        operations: Vec<Operation>,
        error: BatchError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BranchError {
    #[error(transparent)]
    CoreDisconnected(#[from] CoreDisconnected),
    #[error(transparent)]
    InvalidBase(#[from] OpError),
    #[error("branch returned an unexpected response")]
    UnexpectedResponse,
    #[error("cherry-pick operation index {index} is outside the one-based range 1..={maximum}")]
    InvalidOperationIndex { index: usize, maximum: usize },
    #[error("cherry-pick operation index {0} occurs more than once")]
    DuplicateOperationIndex(usize),
}

impl TimelineBranch {
    /// Create an empty edit lineage from one immutable live-project snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if the base document is invalid.
    pub fn new(
        name: impl Into<Arc<str>>,
        base_revision: TimelineRevision,
        base_document: Arc<Document>,
    ) -> Result<Self, BranchError> {
        let core = Core::spawn((*base_document).clone())?;
        Ok(Self {
            name: name.into(),
            base_revision,
            base_document,
            core,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn base_revision(&self) -> TimelineRevision {
        self.base_revision
    }

    #[must_use]
    pub fn base_document(&self) -> Arc<Document> {
        Arc::clone(&self.base_document)
    }

    #[must_use]
    pub fn core(&self) -> Core {
        self.core.clone()
    }

    /// Return the branch head and only the edits still represented by its undo stack.
    ///
    /// # Errors
    ///
    /// Returns an error if the branch actor has stopped or returns an invalid response.
    pub fn compare(&self) -> Result<BranchComparison, BranchError> {
        let Event::QueryResult(QueryResult::Snapshot {
            revision: branch_revision,
            document,
        }) = self.core.request(Command::Query(Query::Snapshot))?
        else {
            return Err(BranchError::UnexpectedResponse);
        };
        let Event::QueryResult(QueryResult::AppliedOperations(operations)) = self
            .core
            .request(Command::Query(Query::AppliedOperations))?
        else {
            return Err(BranchError::UnexpectedResponse);
        };
        Ok(BranchComparison {
            name: Arc::clone(&self.name),
            base_revision: self.base_revision,
            branch_revision,
            base_document: Arc::clone(&self.base_document),
            document,
            operations,
        })
    }

    /// Apply every branch edit to the live project as one optimistic transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if either actor has stopped or returns an invalid response.
    pub fn merge_into(&self, live: &Core) -> Result<BranchApplyOutcome, BranchError> {
        let operations = self.compare()?.operations;
        if operations.is_empty() {
            return Ok(BranchApplyOutcome::NoChanges);
        }
        apply_to_live(live, self.base_revision, (*operations).clone())
    }

    /// Apply selected branch operations to the live project as one transaction.
    /// Indices are stable and one-based, matching the comparison view presented to users.
    ///
    /// # Errors
    ///
    /// Rejects duplicate or out-of-range indices before touching the live project.
    pub fn cherry_pick_into(
        &self,
        live: &Core,
        expected_live_revision: TimelineRevision,
        one_based_indices: &[usize],
    ) -> Result<BranchApplyOutcome, BranchError> {
        let available = self.compare()?.operations;
        let mut unique = BTreeSet::new();
        for &index in one_based_indices {
            if index == 0 || index > available.len() {
                return Err(BranchError::InvalidOperationIndex {
                    index,
                    maximum: available.len(),
                });
            }
            if !unique.insert(index) {
                return Err(BranchError::DuplicateOperationIndex(index));
            }
        }
        if unique.is_empty() {
            return Ok(BranchApplyOutcome::NoChanges);
        }
        let operations = unique
            .into_iter()
            .map(|index| available[index - 1].clone())
            .collect();
        apply_to_live(live, expected_live_revision, operations)
    }
}

fn apply_to_live(
    live: &Core,
    expected: TimelineRevision,
    operations: Vec<Operation>,
) -> Result<BranchApplyOutcome, BranchError> {
    let operation_count = operations.len();
    let outcome = match live.request(Command::DoBatchIfRevision {
        expected,
        operations,
    })? {
        Event::DocumentChanged { doc, revision, .. } => BranchApplyOutcome::Applied {
            revision,
            document: doc,
            operation_count,
        },
        Event::RevisionConflict { expected, actual } => {
            BranchApplyOutcome::Conflict { expected, actual }
        }
        Event::BatchRejected { operations, error } => {
            BranchApplyOutcome::Rejected { operations, error }
        }
        _ => return Err(BranchError::UnexpectedResponse),
    };
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use kinewright_core::{AssetId, MediaAsset, MediaKind, Rational, TimeCode};

    use super::*;

    fn asset(id: u64) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: format!("asset-{id}.mp4").into(),
            name: format!("asset {id}"),
            duration: TimeCode(120),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((1920, 1080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        }
    }

    fn live_snapshot(core: &Core) -> (TimelineRevision, Arc<Document>) {
        let Event::QueryResult(QueryResult::Snapshot { revision, document }) =
            core.request(Command::Query(Query::Snapshot)).unwrap()
        else {
            panic!("expected snapshot");
        };
        (revision, document)
    }

    #[test]
    fn merge_is_atomic_and_rejects_a_stale_base() {
        let live = Core::spawn(Document::default()).unwrap();
        let (revision, document) = live_snapshot(&live);
        let branch = TimelineBranch::new("Agent 1", revision, document).unwrap();
        branch
            .core()
            .request(Command::Do(Operation::AddAsset { asset: asset(1) }))
            .unwrap();

        let merged = branch.merge_into(&live).unwrap();
        assert!(matches!(
            merged,
            BranchApplyOutcome::Applied {
                revision: TimelineRevision(1),
                operation_count: 1,
                ..
            }
        ));
        let conflict = branch.merge_into(&live).unwrap();
        assert_eq!(
            conflict,
            BranchApplyOutcome::Conflict {
                expected: TimelineRevision(0),
                actual: TimelineRevision(1),
            }
        );
        assert_eq!(live_snapshot(&live).1.media_pool.len(), 1);
        live.request(Command::Undo).unwrap();
        assert!(live_snapshot(&live).1.media_pool.is_empty());
    }

    #[test]
    fn sibling_branches_diverge_without_touching_live_state() {
        let live = Core::spawn(Document::default()).unwrap();
        let (revision, document) = live_snapshot(&live);
        let first = TimelineBranch::new("First", revision, Arc::clone(&document)).unwrap();
        let second = TimelineBranch::new("Second", revision, document).unwrap();
        first
            .core()
            .request(Command::Do(Operation::AddAsset { asset: asset(1) }))
            .unwrap();
        second
            .core()
            .request(Command::Do(Operation::AddAsset { asset: asset(2) }))
            .unwrap();

        assert!(live_snapshot(&live).1.media_pool.is_empty());
        assert_eq!(
            first.compare().unwrap().document.media_pool[0].id,
            AssetId(1)
        );
        assert_eq!(
            second.compare().unwrap().document.media_pool[0].id,
            AssetId(2)
        );
    }

    #[test]
    fn cherry_pick_preserves_branch_order_and_validates_indices_first() {
        let live = Core::spawn(Document::default()).unwrap();
        let (revision, document) = live_snapshot(&live);
        let branch = TimelineBranch::new("Agent 1", revision, document).unwrap();
        branch
            .core()
            .request(Command::DoBatch(vec![
                Operation::AddAsset { asset: asset(1) },
                Operation::AddAsset { asset: asset(2) },
            ]))
            .unwrap();

        let error = branch
            .cherry_pick_into(&live, revision, &[2, 2])
            .unwrap_err();
        assert_eq!(error, BranchError::DuplicateOperationIndex(2));
        assert!(live_snapshot(&live).1.media_pool.is_empty());

        let outcome = branch.cherry_pick_into(&live, revision, &[2]).unwrap();
        assert!(matches!(
            outcome,
            BranchApplyOutcome::Applied {
                operation_count: 1,
                ..
            }
        ));
        let (_, document) = live_snapshot(&live);
        assert_eq!(document.media_pool.len(), 1);
        assert_eq!(document.media_pool[0].id, AssetId(2));
    }
}
