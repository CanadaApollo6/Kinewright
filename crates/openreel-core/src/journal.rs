use serde::{Deserialize, Serialize};

use crate::{Command, Operation};

/// An accepted, document-changing command suitable for durable replay.
///
/// This type is intentionally pure data. Persistence belongs to the app layer;
/// core only identifies the exact history command that produced an event.
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
