//! Owner records for the operation history. Existing project readers stay on
//! their current audit, lineage, resource and stored-history implementations.
#![allow(clippy::missing_errors_doc)]

mod ledger;
mod lineage;
mod owner;
mod reservation;
mod resources;
mod snapshot;
mod transition;
pub use transition::{
    Direction, TransitionApproval, TransitionCounts, TransitionFate, Uncertainty,
};
pub(crate) mod stored;
pub use ledger::{Approval, ApprovalAnswer, ApprovalEntry, ApprovalLedger, Fate, RemovedTarget};
pub use lineage::{CreationOrigin, Lineage};
pub use owner::{OwnerBoundary, OwnerCounters, OwnerHistory};
pub use reservation::{Envelope, HistoryPool, HistoryReservation, PoolLimits, Role, Workspace};
pub use resources::{Resource, Resources};
pub use snapshot::{OwnerSnapshot, ValidatedOwnerSnapshot};
pub use stored::StoredOwnerRecord;

use kinewright_core::undo::{CommandKey, PreparedError};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq)]
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
    InvalidResource,
    InvalidRecord,
    IdentityMismatch,
    Reservation,
    Store(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
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
