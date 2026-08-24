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
    Undo,
    Redo,
}

impl From<JournalCommand> for Command {
    fn from(command: JournalCommand) -> Self {
        match command {
            JournalCommand::Do(operation) => Self::Do(operation),
            JournalCommand::DoBatch(operations) => Self::DoBatch(operations),
            JournalCommand::Undo => Self::Undo,
            JournalCommand::Redo => Self::Redo,
        }
    }
}
