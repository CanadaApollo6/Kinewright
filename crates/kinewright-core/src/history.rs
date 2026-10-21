//! Portable history values. Restore replays through the Core's ordinary executor.
use std::{io, sync::Arc};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Clip, ClipId, Document, JournalCommand, Operation};

pub const HISTORY_BYTES: u64 = 32 * 1024 * 1024;
pub const HISTORY_COMMANDS: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationIdentityEffect {
    pub created: Vec<ClipId>,
    pub deleted: Vec<ClipId>,
    pub retained: Vec<ClipId>,
    /// Exact source range and identity before a split, including intermediate splits.
    pub split_parent: Option<Clip>,
    /// The actual children produced by that operation, before later batch edits.
    pub split_results: Vec<Clip>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GestureBoundary {
    pub document: Arc<Document>,
    pub operations: Vec<Operation>,
    pub coalesce_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryCursor {
    /// Oldest first; the last redo entry is the next redo.
    pub undo: Vec<GestureBoundary>,
    pub redo: Vec<GestureBoundary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryBatch {
    pub order: u64,
    pub command: JournalCommand,
    pub identity_effects: Vec<OperationIdentityEffect>,
    pub undo_entries: usize,
    pub redo_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistorySnapshot {
    pub baseline: Arc<Document>,
    pub document: Arc<Document>,
    pub batches: Vec<HistoryBatch>,
    pub cursor: HistoryCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HistoryError {
    #[error("history exceeds its bounded admission")]
    Capacity,
    #[error("history replay differs from the retained checkpoint")]
    Corrupt,
    #[error("the Core actor has stopped")]
    Disconnected,
}

impl HistorySnapshot {
    /// Count encoded bytes without allocating another serialized copy.
    ///
    /// # Errors
    /// Refuses excessive commands, encoded size or serialization failure.
    pub fn encoded_bytes(&self) -> Result<u64, HistoryError> {
        struct Counter(u64);
        impl io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(bytes.len() as u64)
                    .filter(|n| *n <= HISTORY_BYTES)
                    .ok_or_else(|| io::Error::other("history capacity"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        if self.batches.len() > HISTORY_COMMANDS {
            return Err(HistoryError::Capacity);
        }
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, self).map_err(|_| HistoryError::Capacity)?;
        Ok(counter.0)
    }

    #[must_use]
    pub fn applied_operations(&self) -> Vec<Operation> {
        self.cursor
            .undo
            .iter()
            .flat_map(|e| e.operations.iter().cloned())
            .collect()
    }
}
