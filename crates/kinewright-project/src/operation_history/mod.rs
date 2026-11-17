//! Owner records for the operation history. Existing project readers stay on
//! their current audit, lineage, resource and stored-history implementations.
#![allow(clippy::missing_errors_doc)]

mod borrowed_identity;
mod fold;
mod identity;
pub use identity::SnapshotIdentity;
mod ledger;
mod lineage;
mod owner;
mod preview;
mod read;
pub(crate) use preview::PreparedOwner;
pub use read::OwnerRead;
mod reservation;
mod sizing;
pub(crate) use sizing::OWNER_ALLOWANCE;
pub use sizing::{CreationKind, CreationSizing, FixedLimits};
mod resources;
mod snapshot;
mod transition;
mod transport;
pub use transition::{
    Direction, TransitionApproval, TransitionCounts, TransitionFate, Uncertainty,
};
pub use transport::SnapshotTransport;
pub(crate) mod stored;
pub use ledger::{Approval, ApprovalAnswer, ApprovalEntry, ApprovalLedger, Fate, RemovedTarget};
pub use lineage::{CreationOrigin, Lineage};
pub use owner::{OwnerBoundary, OwnerCounters, OwnerHistory, OwnerOperations};
pub(crate) use reservation::LeaseHold;
pub use reservation::{Envelope, HistoryPool, HistoryReservation, PoolLimits, Role, Workspace};
pub use resources::{Resource, Resources};
pub use snapshot::{OwnerSnapshot, ValidatedOwnerSnapshot};
pub use stored::{SealedOwner, StoredOwnerRecord};

use kinewright_core::undo::{CommandKey, PreparedError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    History(kinewright_core::undo::Error),
    Capacity(&'static str),
    InvalidLedger,
    CommandConflict,
    InvalidFrontier,
    InvalidPolicy,
    NoTargetStep,
    StaleTransition,
    InvalidOrigin,
    InvalidSeeds,
    MissingResource,
    InputRequired,
    InvalidResource,
    InvalidRecord,
    IdentityMismatch,
    Reservation,
    PoolFull(Role),
    StoreCapacity(&'static str),
    StoreCorrupt(String),
    StoreUnsettled,
    StoreRunLimit,
    Store(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::History(kinewright_core::undo::Error::HistoryFull) => f.write_str("This task or candidate has reached its history limit. Finish it and continue in a new task or run."),
            Self::History(error) => error.fmt(f),
            Self::PoolFull(Role::Ordinary) => f.write_str("History memory is in use. Finish saving or close a project, then retry."),
            Self::PoolFull(Role::Task) => f.write_str("History memory is in use. Finish a task so it can be checkpointed and released, then retry."),
            Self::PoolFull(Role::Candidate) => f.write_str("History memory is in use. Finish and checkpoint a run so its candidate can be released, then retry."),
            _ => write!(f, "{self:?}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<kinewright_core::undo::Error> for Error {
    fn from(error: kinewright_core::undo::Error) -> Self {
        Self::History(error)
    }
}
impl From<PreparedError<Self>> for Error {
    fn from(error: PreparedError<Self>) -> Self {
        match error {
            PreparedError::History(error) => Self::History(error),
            PreparedError::Owner(error) => error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandOrigin {
    pub key: CommandKey,
    pub payload_digest: [u8; 32],
    pub creation: CreationOrigin,
    /// Immutable input context owned by this command, including its profile
    /// and chunk-table edges. It is independent of the current document.
    pub inputs: Vec<crate::bundle_records::ObjectRef>,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod transition_tests;

#[cfg(test)]
mod differential;

#[cfg(test)]
mod roundtrip_tests;

#[cfg(test)]
mod review_tests;

#[cfg(test)]
mod do_uncertainty_tests;

#[cfg(test)]
mod identity_tests;

#[cfg(test)]
mod fold_tests;

#[cfg(test)]
mod install_review_tests;

#[cfg(test)]
mod sizing_tests;

#[cfg(test)]
mod metadata_replay_tests;

/// Remedy shared by native task admission and its callers.
pub const TASK_LIMIT_REASON: &str =
    "Task history limit reached. Finish or freeze this task before making more edits.";
