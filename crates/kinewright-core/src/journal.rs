use serde::{Deserialize, Serialize};

use crate::{Command, Operation};

/// An accepted, document-changing command suitable for durable replay.
///
/// This type is intentionally pure data. Persistence belongs to the app layer;
/// core only identifies the exact history command that produced an event.
/// The enum is short-lived at the journal boundary, and preserving its direct
/// serialized operation shape is more useful than adding allocation solely to
/// equalize in-memory variant sizes.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalCommand {
    Do(Operation),
    DoBatch(Vec<Operation>),
    /// One frame of a live coalescing gesture, carrying the key that decides
    /// whether it merges into the newest history entry.
    ///
    /// History granularity is part of what recovery has to restore: replaying
    /// an N-frame drag as N ordinary batches would turn one gesture into N
    /// undo steps, and a journaled [`JournalCommand::Undo`] recorded after the
    /// gesture would then unwind a single frame instead of the whole gesture.
    ///
    /// This variant was added after the original journal format shipped. It is
    /// purely additive: journals written by older builds contain only the
    /// other variants and still deserialize unchanged.
    DoBatchCoalesced {
        operations: Vec<Operation>,
        coalesce_key: String,
    },
    Undo,
    Redo,
}

impl From<JournalCommand> for Command {
    fn from(command: JournalCommand) -> Self {
        match command {
            JournalCommand::Do(operation) => Self::Do(operation),
            JournalCommand::DoBatch(operations) => Self::DoBatch(operations),
            JournalCommand::DoBatchCoalesced {
                operations,
                coalesce_key,
            } => Self::DoBatchCoalesced {
                operations,
                coalesce_key,
            },
            JournalCommand::Undo => Self::Undo,
            JournalCommand::Redo => Self::Redo,
        }
    }
}
