//! Portable history values. Restore replays through the Core's ordinary executor.
use std::{fmt::Write as _, io, sync::Arc};

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

#[derive(Debug, Clone, schemars::JsonSchema)]
#[schemars(with = "crate::HistoryTransport")]
pub struct HistorySnapshot {
    /// Runtime replay evidence; never serialized or accepted from input bytes.
    pub replay_proof: Option<Arc<HistoryReplayEvidence>>,
    pub floor_offset: u64,
    pub baseline: Arc<Document>,
    pub document: Arc<Document>,
    pub batches: Vec<HistoryBatch>,
    pub cursor: HistoryCursor,
}

// Runtime evidence does not change the portable history value.
impl PartialEq for HistorySnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.floor_offset == other.floor_offset
            && self.baseline == other.baseline
            && self.document == other.document
            && self.batches == other.batches
            && self.cursor == other.cursor
    }
}
impl Eq for HistorySnapshot {}

/// Runtime evidence for one immutable decoded cut. Weak references neither
/// retain documents nor survive a caller changing their allocation through Arc.
/// No constructor or input encoding is exposed outside Core.
#[derive(Debug)]
pub struct HistoryReplayEvidence {
    metadata_sha256: String,
    documents: Vec<std::sync::Weak<Document>>,
    pub(crate) capacities: (usize, usize, usize),
}
impl HistoryReplayEvidence {
    pub(crate) fn new(
        history: &HistorySnapshot,
        capacities: (usize, usize, usize),
    ) -> Result<Self, HistoryError> {
        Ok(Self {
            metadata_sha256: adoption_metadata_hash(history)?,
            documents: Self::documents(history),
            capacities,
        })
    }
    fn documents(history: &HistorySnapshot) -> Vec<std::sync::Weak<Document>> {
        let mut documents =
            Vec::with_capacity(2 + history.cursor.undo.len() + history.cursor.redo.len());
        documents.push(Arc::downgrade(&history.baseline));
        documents.push(Arc::downgrade(&history.document));
        documents.extend(
            history
                .cursor
                .undo
                .iter()
                .chain(&history.cursor.redo)
                .map(|entry| Arc::downgrade(&entry.document)),
        );
        documents
    }
    fn rebind(&self, history: &HistorySnapshot) -> Self {
        Self {
            metadata_sha256: self.metadata_sha256.clone(),
            documents: Self::documents(history),
            capacities: self.capacities,
        }
    }
    pub(crate) fn matches(&self, history: &HistorySnapshot) -> Result<bool, HistoryError> {
        let documents = std::iter::once(&history.baseline)
            .chain(std::iter::once(&history.document))
            .chain(
                history
                    .cursor
                    .undo
                    .iter()
                    .chain(&history.cursor.redo)
                    .map(|entry| &entry.document),
            );
        if self.documents.len() != 2 + history.cursor.undo.len() + history.cursor.redo.len()
            || !self
                .documents
                .iter()
                .zip(documents)
                .all(|(proof, current)| {
                    proof
                        .upgrade()
                        .is_some_and(|original| Arc::ptr_eq(&original, current))
                })
        {
            return Ok(false);
        }
        Ok(adoption_metadata_hash(history)? == self.metadata_sha256)
    }
}
fn adoption_metadata_hash(history: &HistorySnapshot) -> Result<String, HistoryError> {
    struct Writer(kinewright_hash::Sha256);
    impl io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    // Documents are immutable and tied by weak references above. Hash every
    // mutable non-document field, including cursor operations absent on disk.
    let undo: Vec<_> = history
        .cursor
        .undo
        .iter()
        .map(|entry| (&entry.operations, &entry.coalesce_key))
        .collect();
    let redo: Vec<_> = history
        .cursor
        .redo
        .iter()
        .map(|entry| (&entry.operations, &entry.coalesce_key))
        .collect();
    let mut writer = Writer(kinewright_hash::Sha256::new());
    serde_json::to_writer(
        &mut writer,
        &(history.floor_offset, &history.batches, undo, redo),
    )
    .map_err(|_| HistoryError::Encoding)?;
    let mut hash = String::with_capacity(64);
    for byte in writer.0.finalize() {
        write!(&mut hash, "{byte:02x}").map_err(|_| HistoryError::CommitInvariant)?;
    }
    Ok(hash)
}

/// A journal resolved through bounded replay. Its immutable snapshot cannot be
/// replaced by a caller, so adopting it does not need to execute the journal again.
#[derive(Debug, Clone)]
pub struct ResolvedHistory {
    pub(crate) history: Arc<HistorySnapshot>,
    pub(crate) checkpoint: Arc<crate::actor::ReplayCheckpoint>,
    pub(crate) peak: u64,
    pub(crate) adoption: Arc<HistoryReplayEvidence>,
}
impl ResolvedHistory {
    #[must_use]
    pub fn snapshot(&self) -> &HistorySnapshot {
        &self.history
    }
    #[must_use]
    pub fn into_snapshot(self) -> HistorySnapshot {
        let mut history = Arc::unwrap_or_clone(self.history);
        history.replay_proof = Some(self.adoption);
        history
    }
    pub(crate) fn into_unsealed_snapshot(self) -> HistorySnapshot {
        Arc::unwrap_or_clone(self.history)
    }
    #[must_use]
    pub fn replay_peak(&self) -> u64 {
        self.peak
    }
    /// Reuse equal immutable documents already resident in a live actor. A
    /// different sharing topology keeps the canonical replay allocations.
    /// # Errors
    /// Refuses a value that differs from the replayed history.
    pub fn share_documents(mut self, mut history: HistorySnapshot) -> Result<Self, HistoryError> {
        history.replay_proof = None;
        if self.snapshot() != &history {
            return Err(HistoryError::StaleExpectation);
        }
        if let Some(documents) = self
            .checkpoint
            .documents
            .rebind(self.snapshot(), &history)?
        {
            let mut checkpoint = (*self.checkpoint).clone();
            checkpoint.documents = documents;
            self.adoption = Arc::new(self.adoption.rebind(&history));
            self.history = Arc::new(history);
            self.checkpoint = Arc::new(checkpoint);
        }
        Ok(self)
    }
}
impl PartialEq for ResolvedHistory {
    fn eq(&self, other: &Self) -> bool {
        self.history == other.history
    }
}
impl Eq for ResolvedHistory {}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HistoryError {
    #[error(transparent)]
    MutationRefused(crate::CoreMutationRefusal),
    #[error("history changed during save selection; retry the save")]
    CaptureChanged,
    #[error("the expected history is stale; refresh it and retry")]
    StaleExpectation,
    #[error("another save of this history is still in progress; retry after it finishes")]
    SaveInProgress,
    #[error("internal history barrier invariant")]
    BarrierInvariant,
    #[error("internal history commit invariant; ticket released without discarding entries")]
    CommitInvariant,
    #[error("history exceeds its bounded admission")]
    Capacity,
    #[error("history could not be encoded")]
    Encoding,
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
        if self.batches.len() > HISTORY_COMMANDS {
            return Err(HistoryError::Capacity);
        }
        encoded_size(self)
    }

    /// Keep ordinary undo admission bounded independently of compact transport.
    /// Cursor document bytes are counted without storing or sending those bytes.
    /// # Errors
    /// Refuses the same 32 MiB admission ceiling as the existing manual stream.
    pub fn ordinary_admission_bytes(&self) -> Result<u64, HistoryError> {
        self.cursor.undo.iter().chain(&self.cursor.redo).try_fold(
            self.encoded_bytes()?,
            |total, entry| {
                total
                    .checked_add(boundary_admission_bytes(
                        &entry.document,
                        &entry.operations,
                        entry.coalesce_key.as_deref(),
                    )?)
                    .filter(|bytes| *bytes <= HISTORY_BYTES)
                    .ok_or(HistoryError::Capacity)
            },
        )
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

// Size accounting only: the portable writer still emits hashes for these documents.
pub(crate) fn boundary_admission_bytes(
    document: &Document,
    operations: &[Operation],
    key: Option<&str>,
) -> Result<u64, HistoryError> {
    encoded_size(&(document, operations, key))
}
pub(crate) fn encoded_size(value: &impl Serialize) -> Result<u64, HistoryError> {
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
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|_| HistoryError::Capacity)?;
    Ok(counter.0)
}
