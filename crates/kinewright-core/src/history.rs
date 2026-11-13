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
    /// One unpublished floor's exact source. It is never part of the wire value.
    pub ordinary_floor_source: Option<Arc<OrdinaryFloorSource>>,
    /// Runtime replay evidence; never serialized or accepted from input bytes.
    pub replay_proof: Option<Arc<HistoryReplayEvidence>>,
    pub floor_offset: u64,
    pub baseline: Arc<Document>,
    pub document: Arc<Document>,
    pub batches: Vec<HistoryBatch>,
    pub cursor: HistoryCursor,
}

/// Kept only until the writer acknowledges this floor. The snapshot has no
/// source of its own, so unpublished floors cannot form a retained actor chain.
#[derive(Debug)]
pub struct OrdinaryFloorSource {
    pub(crate) publishers: std::sync::atomic::AtomicUsize,
    pub(crate) history: HistorySnapshot,
    pub(crate) selected_offset: u64,
}
impl OrdinaryFloorSource {
    pub(crate) fn publication_in_flight(&self) -> bool {
        self.publishers.load(std::sync::atomic::Ordering::Acquire) != 0
    }

    /// Includes the opaque wrapper and its Arc allocation header.
    /// # Errors
    /// Refuses accounting overflow.
    pub fn retained_bytes(&self) -> Result<u64, HistoryError> {
        let header = std::mem::size_of::<Self>() - std::mem::size_of::<HistorySnapshot>()
            + 2 * std::mem::size_of::<usize>();
        crate::history_size::history_live_bytes(&self.history)?
            .checked_add(header as u64)
            .ok_or(HistoryError::Capacity)
    }
    #[must_use]
    pub fn applies_to(&self, history: &HistorySnapshot) -> bool {
        history.floor_offset == self.selected_offset
            && history
                .floor_offset
                .checked_add(history.batches.len() as u64)
                .zip(
                    self.history
                        .floor_offset
                        .checked_add(self.history.batches.len() as u64),
                )
                .is_some_and(|(current, source)| current >= source)
    }
}
/// Owner-authorized scope for a real publication of one exact pending source.
/// It promises no outcome: leaving the scope removes retryable pressure even
/// when the writer has to retain an uncertain save for person reconciliation.
pub struct OrdinarySourcePublication(Arc<OrdinaryFloorSource>);
impl OrdinarySourcePublication {
    #[allow(deprecated, reason = "Rust 1.92 MSRV predates AtomicUsize::try_update")]
    pub(crate) fn begin(source: Arc<OrdinaryFloorSource>) -> Result<Self, HistoryError> {
        source
            .publishers
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |count| count.checked_add(1),
            )
            .map_err(|_| HistoryError::Capacity)?;
        Ok(Self(source))
    }
}
impl Drop for OrdinarySourcePublication {
    fn drop(&mut self) {
        self.0
            .publishers
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

impl HistorySnapshot {
    /// Recover the exact unfloored publication source, including edits accepted
    /// since the floor. The returned value is checked against this retained cut.
    /// # Errors
    /// Refuses a changed source or a non-equivalent retained floor.
    pub fn publication_history(&self) -> Result<Option<Self>, HistoryError> {
        crate::actor::ordinary_publication_history(self)
    }
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
    replay_dynamic_peak: u64,
    canonical_redo_input: std::sync::OnceLock<crate::history_size::RedoInputMax>,
    documents: Vec<std::sync::Weak<Document>>,
    pub(crate) capacities: (usize, usize, usize),
}
impl HistoryReplayEvidence {
    pub(crate) fn new(
        history: &HistorySnapshot,
        capacities: (usize, usize, usize),
        replay_dynamic_peak: u64,
    ) -> Result<Self, HistoryError> {
        Ok(Self {
            metadata_sha256: adoption_metadata_hash(history)?,
            replay_dynamic_peak,
            canonical_redo_input: std::sync::OnceLock::new(),
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
    fn redo_input_max(
        &self,
        history: &HistorySnapshot,
    ) -> Result<crate::history_size::RedoInputMax, HistoryError> {
        if let Some(bound) = self.canonical_redo_input.get() {
            return Ok(*bound);
        }
        let bound = crate::history_size::redo_input_max(history)?;
        // Immutable proof metadata may be shared between readers. A racing
        // initializer computes the same bound; no document is retained here.
        let _ = self.canonical_redo_input.set(bound);
        Ok(*self
            .canonical_redo_input
            .get()
            .ok_or(HistoryError::CommitInvariant)?)
    }
    fn rebind(&self, history: &HistorySnapshot) -> Self {
        let canonical_redo_input = std::sync::OnceLock::new();
        if let Some(bound) = self.canonical_redo_input.get() {
            let _ = canonical_redo_input.set(*bound);
        }
        Self {
            metadata_sha256: self.metadata_sha256.clone(),
            replay_dynamic_peak: self.replay_dynamic_peak,
            canonical_redo_input,
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
    // Sealed construction provenance, never serialized or supplied by a caller.
    pub(crate) canonical_allocations: bool,
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
    /// Bound every remaining Redo without executing another history replay.
    /// # Errors
    /// Refuses arithmetic or command-count overflow.
    pub fn redo_capacity_bound(
        &self,
        journal: &crate::HistoryJournal,
        input: &HistorySnapshot,
    ) -> Result<crate::history_size::RedoCapacityBound, HistoryError> {
        if self.snapshot() != input {
            return Err(HistoryError::StaleExpectation);
        }
        crate::history_size::redo_capacity_bound(
            self.snapshot(),
            self.adoption.replay_dynamic_peak,
            journal,
            input,
            self.adoption.redo_input_max(self.snapshot())?,
        )
    }
    #[must_use]
    pub fn shares_document_allocations(&self, other: &HistorySnapshot) -> bool {
        let own = self.snapshot();
        Arc::ptr_eq(&own.baseline, &other.baseline)
            && Arc::ptr_eq(&own.document, &other.document)
            && own.cursor.undo.len() == other.cursor.undo.len()
            && own.cursor.redo.len() == other.cursor.redo.len()
            && own
                .cursor
                .undo
                .iter()
                .chain(&own.cursor.redo)
                .zip(other.cursor.undo.iter().chain(&other.cursor.redo))
                .all(|(before, after)| Arc::ptr_eq(&before.document, &after.document))
    }

    /// Share a live holder's immutable documents only when their physical
    /// charges equal the canonical replay's charges, including baseline decode
    /// padding and alias topology. The replay budget and its maxima remain
    /// unchanged. A larger or smaller layout keeps the canonical documents.
    /// # Errors
    /// Refuses arithmetic or inconsistent replay evidence.
    pub fn share_canonical_documents(
        mut self,
        live: &HistorySnapshot,
    ) -> Result<Self, HistoryError> {
        fn documents(history: &HistorySnapshot) -> impl Iterator<Item = &Arc<Document>> {
            std::iter::once(&history.baseline)
                .chain(std::iter::once(&history.document))
                .chain(
                    history
                        .cursor
                        .undo
                        .iter()
                        .chain(&history.cursor.redo)
                        .map(|entry| &entry.document),
                )
        }
        if !self.canonical_allocations
            || self.snapshot() != live
            || self.shares_document_allocations(live)
        {
            return Ok(self);
        }
        let before = self.snapshot();
        let old_current = Arc::as_ptr(&before.document) as usize;
        let same_current_geometry =
            crate::history_size::same_document_geometry(&before.document, &live.document);
        let shared_boundaries = Arc::ptr_eq(&before.baseline, &live.baseline)
            && before
                .cursor
                .undo
                .iter()
                .chain(&before.cursor.redo)
                .zip(live.cursor.undo.iter().chain(&live.cursor.redo))
                .all(|(a, b)| Arc::ptr_eq(&a.document, &b.document));
        if shared_boundaries
            && same_current_geometry
            && Arc::make_mut(&mut self.checkpoint)
                .documents
                .rebind_current(old_current, &live.document)
        {
            let history = Arc::make_mut(&mut self.history);
            history.document = Arc::clone(&live.document);
            self.adoption = Arc::new(self.adoption.rebind(history));
            return Ok(self);
        }
        let before = self.snapshot();
        let mut checked = std::collections::BTreeSet::new();
        let same_geometry = documents(before)
            .zip(documents(live))
            .all(|(canonical, held)| {
                Arc::ptr_eq(canonical, held)
                    || !checked
                        .insert((Arc::as_ptr(canonical) as usize, Arc::as_ptr(held) as usize))
                    || crate::history_size::same_document_geometry(canonical, held)
            });
        if !same_geometry {
            return Ok(self);
        }
        let Some(documents) = self.checkpoint.documents.rebind_exact(before, live)? else {
            return Ok(self);
        };
        // Normal prefix extension owns these metadata buffers uniquely. Rebind
        // document handles in place; do not clone the journal or operation log.
        let history = Arc::make_mut(&mut self.history);
        history.baseline = Arc::clone(&live.baseline);
        history.document = Arc::clone(&live.document);
        for (canonical, held) in history
            .cursor
            .undo
            .iter_mut()
            .chain(&mut history.cursor.redo)
            .zip(live.cursor.undo.iter().chain(&live.cursor.redo))
        {
            canonical.document = Arc::clone(&held.document);
        }
        self.adoption = Arc::new(self.adoption.rebind(history));
        Arc::make_mut(&mut self.checkpoint).documents = documents;
        Ok(self)
    }

    /// Reuse equal immutable documents already resident in a live actor. A
    /// different sharing topology keeps the canonical replay allocations.
    /// # Errors
    /// Refuses a value that differs from the replayed history.
    pub fn share_documents(mut self, mut history: HistorySnapshot) -> Result<Self, HistoryError> {
        self.canonical_allocations = false;
        history.replay_proof = None;
        history.ordinary_floor_source = None;
        if !self.snapshot().cursor.redo.is_empty() {
            // Sharing smaller equal documents must not erase the canonical
            // replay allocation needed by a later cold-open Redo input.
            self.adoption.redo_input_max(self.snapshot())?;
        }
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

    /// Check sealed replay evidence before using its navigation reservation.
    /// # Errors
    /// Refuses invalid history, capacity or unavailable actor evidence.
    pub fn redo_capacity_bound(
        &self,
        replay_limit: u64,
        journal: &crate::HistoryJournal,
    ) -> Result<crate::history_size::RedoCapacityBound, HistoryError> {
        if let Some(proof) = &self.replay_proof
            && proof.matches(self)?
        {
            return crate::history_size::redo_capacity_bound(
                self,
                proof.replay_dynamic_peak,
                journal,
                self,
                proof.redo_input_max(self)?,
            );
        }
        let resolved = journal.resolve_verified_with_budget(
            Arc::clone(&self.baseline),
            &self.document,
            replay_limit,
            None,
        )?;
        if resolved.snapshot() != self {
            return Err(HistoryError::StaleExpectation);
        }
        resolved.redo_capacity_bound(journal, self)
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
