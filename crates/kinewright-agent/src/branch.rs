use std::{collections::BTreeSet, sync::Arc};

use kinewright_core::{
    BatchError, Command, Core, CoreDisconnected, Document, Event, IncidentCode, IncidentEvidence,
    IncidentObservation, IncidentSubject, OpError, Operation, Query, QueryResult,
    RejectionIncident, TimelineRevision,
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

impl BranchError {
    /// The incident code this branch refusal carries (`IN1b` §6.3 rule 6).
    ///
    /// Declared in the agent crate rather than in core, because `IN1b` §2.2
    /// rule 8 keeps the four out-of-core typed enums where they are: core owns
    /// the code and the policy class, not the code's trigger.
    ///
    /// `InvalidBase(OpError)` **delegates** to [`OpError::incident_code`]: a
    /// rejected base document is a rejected operation, and telling the person
    /// *"start a new agent thread"* for a duplicate clip id would be false.
    /// The other four are the branch's own refusal — a stopped branch actor, a
    /// reply the branch did not expect, and the two cherry-pick index
    /// refusals — and take `agent_branch_rejected`.
    #[must_use]
    pub const fn incident_code(&self) -> IncidentCode {
        match self {
            Self::InvalidBase(error) => error.incident_code(),
            Self::CoreDisconnected(_)
            | Self::UnexpectedResponse
            | Self::InvalidOperationIndex { .. }
            | Self::DuplicateOperationIndex(_) => {
                IncidentCode::Rejection(RejectionIncident::AgentBranch)
            }
        }
    }

    /// An observation from this refusal, with the caller's subject.
    ///
    /// **Evidence follows the code** (`IN1b` §3.4 rule 24): the delegating arm
    /// supplies [`IncidentEvidence::OpError`] with the inner family and
    /// **never** a `reason` string, and the other four supply
    /// [`IncidentEvidence::Branch`].
    ///
    /// The subject is the caller's because the error does not know it: the four
    /// branch refusals are the chat panel's own, `IncidentSubject::Agent`
    /// (Appendix B rows 41 and 45), while `InvalidBase` carries the rejected
    /// operation's subject, which only the caller holding that operation can
    /// name.
    #[must_use]
    pub fn incident_observation(
        &self,
        subject: IncidentSubject,
        revision: TimelineRevision,
    ) -> IncidentObservation {
        let evidence = match self {
            Self::InvalidBase(error) => IncidentEvidence::OpError {
                family: error.incident_family(),
                op_number: None,
                message: error.to_string(),
            },
            Self::CoreDisconnected(_)
            | Self::UnexpectedResponse
            | Self::InvalidOperationIndex { .. }
            | Self::DuplicateOperationIndex(_) => IncidentEvidence::Branch {
                reason: self.to_string(),
            },
        };
        IncidentObservation {
            code: self.incident_code(),
            subject,
            observed: self.to_string(),
            allowed: None,
            evidence,
            revision,
        }
    }
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

    /// The same lineage, with the branch core's revision counter **seeded at
    /// the live revision** (IN2 §3.3 rules 15–16).
    ///
    /// [`Self::new`] spawns at `TimelineRevision::default()`, so a branch's
    /// counter is independent of live's and `BranchComparison.branch_revision`
    /// counts edits rather than naming a revision. That is the right shape for
    /// the chat panel and is left exactly as it is: `new` is unchanged and no
    /// chat-panel assertion moves. An investigator session needs the other
    /// shape, because the operations it proves on the branch are proved
    /// against the document an approval will apply them to, and its proposal's
    /// `base_revision` must be a **live** revision a reader can compare with
    /// `ProjectSession.revision`.
    ///
    /// Its limit, stated: the seed is live's revision at *session start*,
    /// which equals the incident's own revision only if live has not moved
    /// since the incident opened. IN2 §4.4's single retry is what handles the
    /// case where it has.
    ///
    /// # Errors
    ///
    /// Returns an error if the base document is invalid.
    pub fn new_at(
        name: impl Into<Arc<str>>,
        base_revision: TimelineRevision,
        base_document: Arc<Document>,
    ) -> Result<Self, BranchError> {
        let core = Core::spawn_at((*base_document).clone(), base_revision)?;
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

/// Apply `operations` to `live` as one optimistic, revision-gated transaction.
///
/// Public since IN2 §4.4 rule 16: it is already the single application path
/// for both merge and cherry-pick, and the application's approve-a-proposal
/// path is the third caller of exactly the same transaction. It stays
/// **synchronous**, so the caller that holds the `IncidentId` holds the
/// answer, and it returns a `Result` rather than an outcome by value because
/// `live.request` can fail and `BranchError::{CoreDisconnected,
/// UnexpectedResponse}` is a real arm an approval path must handle.
///
/// The whole batch becomes **one** core history entry, which is the undo entry
/// a person gets for pressing Approve once.
///
/// # Errors
///
/// Returns an error if the live actor has stopped or answers something this
/// transaction cannot be.
pub fn apply_to_live(
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
        Event::RevisionConflict {
            expected, actual, ..
        } => BranchApplyOutcome::Conflict { expected, actual },
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

    /// `IN1b` §6.3 rule 6, §3.4 rule 24 and §7 item 10's agent half: every
    /// `BranchError` names a code, and `InvalidBase` **delegates** rather than
    /// falling back to a `reason` string.
    ///
    /// Core's `in1b_evidence_follows_the_code_for_every_delegating_producer`
    /// covers `BatchError`'s two delegating arms; the `BranchError` half lives
    /// here, because the enum lives in this crate (§2.2 rule 8).
    #[test]
    fn in1b_branch_error_names_a_code_and_the_base_rejection_delegates() {
        use kinewright_core::{ClipId, IncidentFamily};

        let agent_branch = IncidentCode::Rejection(RejectionIncident::AgentBranch);
        for error in [
            BranchError::CoreDisconnected(CoreDisconnected),
            BranchError::UnexpectedResponse,
            BranchError::InvalidOperationIndex {
                index: 4,
                maximum: 2,
            },
            BranchError::DuplicateOperationIndex(2),
        ] {
            assert_eq!(error.incident_code(), agent_branch, "{error}");
            assert_eq!(error.incident_code().code(), "agent_branch_rejected");
            let observation =
                error.incident_observation(IncidentSubject::Agent, TimelineRevision(3));
            assert_eq!(observation.subject, IncidentSubject::Agent);
            assert_eq!(observation.observed, error.to_string());
            assert_eq!(
                observation.evidence,
                IncidentEvidence::Branch {
                    reason: error.to_string()
                },
                "the branch's own refusals carry Branch evidence"
            );
        }

        // The delegating arm: the code, the family and the evidence are the
        // inner rejection's, and a `Branch { reason }` never appears.
        let inner = OpError::MissingClip(ClipId(9));
        let invalid = BranchError::InvalidBase(inner.clone());
        assert_eq!(invalid.incident_code(), inner.incident_code());
        assert_ne!(invalid.incident_code(), agent_branch);
        let observation =
            invalid.incident_observation(IncidentSubject::Clip(ClipId(9)), TimelineRevision(3));
        assert_eq!(observation.code, inner.incident_code());
        assert_eq!(
            observation.evidence,
            IncidentEvidence::OpError {
                family: inner.incident_family(),
                op_number: None,
                message: inner.to_string(),
            }
        );
        assert_ne!(inner.incident_family(), IncidentFamily::Internal);
        assert!(
            !matches!(observation.evidence, IncidentEvidence::Branch { .. }),
            "a delegating variant never falls back to a reason string"
        );
    }

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
            assumed_from: None,
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

    /// IN2 §3.3 rule 16 and reviewers' S5: `new_at` seeds the branch core at
    /// the **live** revision, and `new` is unchanged.
    ///
    /// The branch-side twin of core's `in2_a_core_spawned_at_a_revision_reports_that_revision`.
    /// What it adds over core's is the wrapper's two consequences: the struct
    /// records `base_revision`, and `compare()`'s `branch_revision` reads the
    /// **seeded** counter rather than a private one starting at zero — which is
    /// exactly the difference the chat panel must not see and the investigator
    /// must.
    #[test]
    fn in2_a_branch_spawned_at_a_revision_reports_the_seed_and_leaves_new_alone() {
        const SEED: TimelineRevision = TimelineRevision(41);
        let live = Core::spawn(Document::default()).unwrap();
        let (_, document) = live_snapshot(&live);

        let seeded = TimelineBranch::new_at("investigator", SEED, Arc::clone(&document)).unwrap();
        assert_eq!(seeded.base_revision(), SEED);
        let before = seeded.compare().unwrap();
        assert_eq!(before.base_revision, SEED);
        assert_eq!(
            before.branch_revision, SEED,
            "the branch counter starts at live's, not at zero"
        );
        assert!(
            before.operations.is_empty(),
            "spawn_at starts with an empty op_log, so 'since spawn_at' is the whole list"
        );

        seeded
            .core()
            .request(Command::Do(Operation::AddAsset { asset: asset(7) }))
            .unwrap();
        let after = seeded.compare().unwrap();
        assert_eq!(after.branch_revision, TimelineRevision(42));
        assert_eq!(
            after.base_revision, SEED,
            "the seed does not move with an edit"
        );
        assert_eq!(after.operations.len(), 1);

        // `TimelineBranch::new` is untouched, so no chat-panel assertion moves.
        let plain = TimelineBranch::new("Agent 1", SEED, document).unwrap();
        assert_eq!(plain.base_revision(), SEED);
        assert_eq!(
            plain.compare().unwrap().branch_revision,
            TimelineRevision::default(),
            "new still counts edits from zero"
        );
    }
}
