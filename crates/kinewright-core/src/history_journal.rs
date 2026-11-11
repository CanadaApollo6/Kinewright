//! Compact portable history. Boundary documents remain an in-process value.
use std::{fmt::Write as _, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{
    Clip, ClipId, Document, HistoryBatch, HistoryError, HistorySnapshot, JournalCommand,
    OperationIdentityEffect,
};

/// Only identity facts not represented by the command. Surviving IDs are replayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalIdentityEffect {
    pub created: Vec<ClipId>,
    pub deleted: Vec<ClipId>,
    pub split_parent: Option<Clip>,
    pub split_results: Vec<Clip>,
}

impl From<&OperationIdentityEffect> for JournalIdentityEffect {
    fn from(effect: &OperationIdentityEffect) -> Self {
        Self {
            created: effect.created.clone(),
            deleted: effect.deleted.clone(),
            split_parent: effect.split_parent.clone(),
            split_results: effect.split_results.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalBatch {
    pub order: u64,
    pub command: JournalCommand,
    pub identity_effects: Vec<JournalIdentityEffect>,
    pub undo_entries: usize,
    pub redo_entries: usize,
}

impl From<&HistoryBatch> for JournalBatch {
    fn from(batch: &HistoryBatch) -> Self {
        Self {
            order: batch.order,
            command: batch.command.clone(),
            identity_effects: batch.identity_effects.iter().map(Into::into).collect(),
            undo_entries: batch.undo_entries,
            redo_entries: batch.redo_entries,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalCursor {
    pub undo: Vec<String>,
    pub redo: Vec<String>,
}

/// Accepted command order is independent of the active Undo/Redo path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryJournal {
    pub floor_offset: u64,
    pub baseline_sha256: String,
    pub document_sha256: String,
    pub batches: Vec<JournalBatch>,
    pub cursor: JournalCursor,
}

/// Self-contained transfer: each document is carried once, never per boundary.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryTransport {
    pub baseline: Arc<Document>,
    pub document: Arc<Document>,
    pub journal: HistoryJournal,
}
impl HistoryTransport {
    /// Retain a portable anchor without retaining the in-process undo documents.
    /// # Errors
    /// Refuses an excessive or unserializable journal.
    pub fn capture(history: &HistorySnapshot) -> Result<Self, HistoryError> {
        Ok(Self {
            baseline: Arc::clone(&history.baseline),
            document: Arc::clone(&history.document),
            journal: HistoryJournal::capture(history)?,
        })
    }
    /// Materialize and verify an anchor through ordinary Core replay.
    /// # Errors
    /// Refuses any command or retained hash mismatch.
    pub fn resolve(&self) -> Result<HistorySnapshot, HistoryError> {
        // Transports have no store ledger beside them. Refuse amplification
        // before replay; project callers also reserve their stricter role budget.
        if crate::history_size::replay_live_bound(&self.journal, &self.baseline, &self.document)?
            > crate::history_size::TRANSPORT_LIVE_BYTES
        {
            return Err(HistoryError::Capacity);
        }
        self.journal
            .resolve(Arc::clone(&self.baseline), &self.document)
    }
}

/// Hash the canonical document encoding without retaining another encoded copy.
///
/// # Errors
/// Returns corruption when document serialization fails.
pub fn history_document_hash(document: &Document) -> Result<String, HistoryError> {
    struct Writer(kinewright_hash::Sha256);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer(kinewright_hash::Sha256::new());
    serde_json::to_writer(&mut writer, document).map_err(|_| HistoryError::Corrupt)?;
    let mut hash = String::with_capacity(64);
    for byte in writer.0.finalize() {
        write!(&mut hash, "{byte:02x}").map_err(|_| HistoryError::Corrupt)?;
    }
    Ok(hash)
}

impl HistoryJournal {
    /// Materialize compact command payloads for a persistence/transport boundary.
    ///
    /// # Errors
    /// Refuses an excessive batch count or an unserializable document.
    pub fn capture(history: &HistorySnapshot) -> Result<Self, HistoryError> {
        if history.batches.len() > crate::HISTORY_COMMANDS {
            return Err(HistoryError::Capacity);
        }
        let hashes = |entries: &[crate::GestureBoundary]| {
            entries
                .iter()
                .map(|entry| history_document_hash(&entry.document))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            floor_offset: history.floor_offset,
            baseline_sha256: history_document_hash(&history.baseline)?,
            document_sha256: history_document_hash(&history.document)?,
            batches: history.batches.iter().map(Into::into).collect(),
            cursor: JournalCursor {
                undo: hashes(&history.cursor.undo)?,
                redo: hashes(&history.cursor.redo)?,
            },
        })
    }

    /// Replay through the ordinary executor, checking every retained boundary.
    ///
    /// # Errors
    /// Refuses invalid commands, identities, cursor hashes or current state.
    pub fn resolve(
        &self,
        baseline: Arc<Document>,
        document: &Document,
    ) -> Result<HistorySnapshot, HistoryError> {
        crate::actor::resolve_history_journal(self, baseline, document)
    }
}

impl Serialize for HistorySnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let journal = HistoryJournal::capture(self).map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("HistoryTransport", 3)?;
        state.serialize_field("baseline", &self.baseline)?;
        state.serialize_field("document", &self.document)?;
        state.serialize_field("journal", &journal)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for HistorySnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let transport = HistoryTransport::deserialize(deserializer)?;
        transport.resolve().map_err(serde::de::Error::custom)
    }
}
