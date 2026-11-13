//! Transactional manual admission and the exact cached transport JSON byte stream.
use std::{collections::VecDeque, fmt::Write as _, io::Write as _};

use super::{
    Arc, AttachmentState, Command, CoreMutationRefusal, CoreState, Document, Event, HistoryEntry,
    HistoryError, OperationIdentityEffect, TimelineRevision, execute_actor_command, manual_refusal,
};
use crate::history::HISTORY_BYTES;
use crate::{AcceptedHistory, HISTORY_COMMANDS, ManualHistoryCandidate};
use serde::Serialize;

pub(super) struct ManualIngress {
    pub(super) generation: u64,
    delta_core: bool,
    encoder: Box<dyn crate::ManualHistoryEncoder>,
    next_sequence: u64,
    predecessor: Option<String>,
    pub(super) pending: VecDeque<AcceptedHistory>,
    pub(super) bytes: usize,
    cache: HistoryJson,
}

pub(super) fn manual_message_bytes(item: &AcceptedHistory) -> Result<usize, HistoryError> {
    serde_json::to_vec(item)
        .map(|bytes| bytes.len())
        .map_err(|_| HistoryError::Encoding)
}

impl ManualIngress {
    pub(super) fn new(
        state: &mut CoreState,
        generation: u64,
        delta_core: bool,
        mut encoder: Box<dyn crate::ManualHistoryEncoder>,
    ) -> Result<Self, HistoryError> {
        if let Some(proof) =
            encoder
                .check_history(&state.history())
                .map_err(|error| match error {
                    HistoryError::MutationRefused(
                        CoreMutationRefusal::ManualHistoryCapacity
                        | CoreMutationRefusal::ManualUndoCapacity,
                    ) => {
                        HistoryError::MutationRefused(CoreMutationRefusal::ManualHistoryUnavailable)
                    }
                    other => other,
                })?
        {
            if proof.snapshot() != &state.history() {
                encoder.finish_history_check(false);
                return Err(HistoryError::BarrierInvariant);
            }
            // A lease changes no history values. It can release a holder's
            // excess allocations before the first admitted edit.
            if !keeps_checked_layout(state, &proof) {
                *state = checked_state(state, proof, false);
            }
        }
        encoder.finish_history_check(true);
        Ok(Self {
            generation,
            delta_core,
            encoder,
            next_sequence: 1,
            predecessor: None,
            pending: VecDeque::new(),
            bytes: 0,
            cache: HistoryJson::new(state)?,
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the history, cache and queue share one rollback transaction"
    )]
    pub(super) fn execute(
        &mut self,
        state: &mut CoreState,
        attachment: AttachmentState,
        command: Command,
    ) -> Event {
        if matches!(command, Command::Query(_)) || attachment != AttachmentState::Live {
            return execute_actor_command(state, attachment, command);
        }
        if self.cache.epoch != state.history_epoch {
            match HistoryJson::new(state) {
                Ok(cache) => self.cache = cache,
                Err(_) => {
                    return manual_refusal(&command, CoreMutationRefusal::ManualHistoryUnavailable);
                }
            }
        }
        // The actor is the sole reader/writer during admission. Retain only the
        // pieces this command can change, and restore them on every refusal.
        let mut delta = CandidateChange::begin(state, &command);
        let event = execute_actor_command(state, attachment, command.clone());
        if !matches!(
            &event,
            Event::DocumentChanged {
                journal_command: Some(_),
                ..
            }
        ) {
            delta.rollback(state);
            return event;
        }
        // The writer's effective policy is authoritative here as well as at
        // publication and cold open. Do not advance the JSON cache or encoder
        // until the complete prospective history passes bounded replay.
        let admission = self
            .encoder
            .check_history(&state.history())
            .and_then(|floor| {
                if self.delta_core
                    && floor
                        .as_ref()
                        .is_some_and(|proof| proof.snapshot().floor_offset != state.floor_offset)
                {
                    return Err(HistoryError::MutationRefused(
                        CoreMutationRefusal::ThreadHistoryCapacity,
                    ));
                }
                delta.install_floor(state, floor, None)
            });
        if let Err(error) = admission {
            self.encoder.finish_history_check(false);
            delta.rollback(state);
            let reason = preparation_failure(&error);
            return manual_refusal(
                &command,
                if self.delta_core && reason == CoreMutationRefusal::ManualHistoryCapacity {
                    CoreMutationRefusal::ThreadHistoryCapacity
                } else {
                    reason
                },
            );
        }
        let old_cache = if delta.floor_moved {
            let Ok(cache) = HistoryJson::new(state) else {
                self.encoder.finish_history_check(false);
                delta.rollback(state);
                return manual_refusal(&command, CoreMutationRefusal::ManualHistoryUnavailable);
            };
            Some(std::mem::replace(&mut self.cache, cache))
        } else {
            None
        };
        let (undo_prefix, redo_prefix) = delta.command.cache_prefixes(state);
        let patches = if old_cache.is_some() {
            Ok(None)
        } else {
            self.cache.update(state, undo_prefix, redo_prefix).map(Some)
        };
        let Ok(patches) = patches else {
            self.encoder.finish_history_check(false);
            delta.rollback(state);
            return manual_refusal(&command, CoreMutationRefusal::ManualHistoryUnavailable);
        };
        let admitted = (|| {
            let (after_sha256, cursor_sha256) = self.cache.identities(state)?;
            let item = self.encoder.encode(
                self.next_sequence,
                delta.command.revision,
                self.predecessor.as_deref(),
                ManualHistoryCandidate {
                    document: &state.document,
                    batch: state.batches.last().ok_or(HistoryError::Corrupt)?,
                    before_sha256: &old_cache.as_ref().unwrap_or(&self.cache).sha256,
                    after_sha256: &after_sha256,
                    cursor_sha256: &cursor_sha256,
                },
            )?;
            let bytes = manual_message_bytes(&item)?;
            let next_sequence = self
                .next_sequence
                .checked_add(1)
                .ok_or(HistoryError::CommitInvariant)?;
            if item.sequence != self.next_sequence {
                return Err(HistoryError::CommitInvariant);
            }
            if bytes > crate::MANUAL_BYTES {
                return Err(HistoryError::MutationRefused(
                    CoreMutationRefusal::ManualHistoryCapacity,
                ));
            }
            if self.pending.len() >= crate::MANUAL_COMMANDS
                || bytes > crate::MANUAL_BYTES.saturating_sub(self.bytes)
            {
                return Err(HistoryError::MutationRefused(
                    CoreMutationRefusal::ManualBackpressure,
                ));
            }
            Ok((item, bytes, next_sequence, after_sha256))
        })();
        match admitted {
            Ok((item, bytes, next_sequence, after_sha256)) => {
                self.encoder.finish_history_check(true);
                self.next_sequence = next_sequence;
                self.predecessor = Some(item.batch_id.clone());
                self.bytes += bytes;
                self.pending.push_back(item);
                // The previous after identity is the next before identity.
                self.cache.sha256 = after_sha256;
                event
            }
            Err(error) => {
                self.encoder.finish_history_check(false);
                if let Some(cache) = old_cache {
                    self.cache = cache;
                } else if let Some(patches) = patches {
                    self.cache.rollback(patches);
                }
                delta.rollback(state);
                let reason = preparation_failure(&error);
                manual_refusal(&command, reason)
            }
        }
    }
}

fn preparation_failure(error: &HistoryError) -> CoreMutationRefusal {
    match error {
        HistoryError::MutationRefused(reason) => *reason,
        HistoryError::CaptureChanged
        | HistoryError::StaleExpectation
        | HistoryError::SaveInProgress
        | HistoryError::BarrierInvariant
        | HistoryError::CommitInvariant
        | HistoryError::Capacity
        | HistoryError::Encoding
        | HistoryError::Corrupt
        | HistoryError::Disconnected => CoreMutationRefusal::ManualHistoryUnavailable,
    }
}

pub(super) fn execute_admitted(
    state: &mut CoreState,
    attachment: AttachmentState,
    command: Command,
    admission: &mut dyn crate::HistoryAdmission,
    source_bytes: Option<u64>,
    delta: bool,
) -> Event {
    if matches!(command, Command::Query(_)) || attachment != AttachmentState::Live {
        return execute_actor_command(state, attachment, command);
    }
    let mut change = CandidateChange::begin(state, &command);
    let event = execute_actor_command(state, attachment, command.clone());
    if !matches!(
        event,
        Event::DocumentChanged {
            journal_command: Some(_),
            ..
        }
    ) {
        change.rollback(state);
        return event;
    }
    match admission.check_history(&state.history()).and_then(|floor| {
        if delta
            && floor
                .as_ref()
                .is_some_and(|proof| proof.snapshot().floor_offset != state.floor_offset)
        {
            return Err(HistoryError::MutationRefused(
                CoreMutationRefusal::ThreadHistoryCapacity,
            ));
        }
        change.install_floor(state, floor, source_bytes)
    }) {
        Ok(()) => {
            admission.finish_history_check(true);
            event
        }
        Err(error) => {
            admission.finish_history_check(false);
            change.rollback(state);
            let mut reason = preparation_failure(&error);
            if delta
                && matches!(
                    reason,
                    CoreMutationRefusal::ManualHistoryCapacity
                        | CoreMutationRefusal::ManualUndoCapacity
                )
            {
                reason = CoreMutationRefusal::ThreadHistoryCapacity;
            }
            manual_refusal(&command, reason)
        }
    }
}

// A rare floor moves the old state here. Rollback restores it before undoing
// the command, including caches and discarded Undo documents, without copying it.
struct CandidateChange {
    command: CandidateDelta,
    before_floor: Option<CoreState>,
    floor_moved: bool,
}
impl CandidateChange {
    fn begin(state: &mut CoreState, command: &Command) -> Self {
        Self {
            command: CandidateDelta::begin(state, command),
            before_floor: None,
            floor_moved: false,
        }
    }
    fn install_floor(
        &mut self,
        state: &mut CoreState,
        floor: Option<crate::ResolvedHistory>,
        source_bytes: Option<u64>,
    ) -> Result<(), HistoryError> {
        let Some(floor) = floor else { return Ok(()) };
        let selected = floor.snapshot();
        if selected.floor_offset < state.floor_offset
            || selected.document != state.document
            || selected
                .floor_offset
                .checked_add(selected.batches.len() as u64)
                != state.floor_offset.checked_add(state.batches.len() as u64)
        {
            return Err(HistoryError::CommitInvariant);
        }
        let moving = selected.floor_offset > state.floor_offset;
        if !moving {
            if !state.history_matches(selected) {
                return Err(HistoryError::CommitInvariant);
            }
            if !keeps_checked_layout(state, &floor) {
                let after = checked_state(state, floor, false);
                self.before_floor = Some(std::mem::replace(state, after));
            }
            return Ok(());
        }
        let next_epoch = state
            .history_epoch
            .checked_add(1)
            .ok_or(HistoryError::Capacity)?;
        let pending_source = if let Some(source_bytes) = source_bytes {
            let mut source = crate::OrdinaryFloorSource {
                publishers: std::sync::atomic::AtomicUsize::new(0),
                history: state.history(),
                selected_offset: selected.floor_offset,
            };
            if source.retained_bytes()? > source_bytes {
                source.history = tight_history(source.history);
            }
            if source.retained_bytes()? > source_bytes {
                return Err(HistoryError::MutationRefused(
                    CoreMutationRefusal::ManualHistoryUnavailable,
                ));
            }
            // Check intrinsic preparation failures before promising that an
            // acknowledgement can make room for this exact command.
            if let Some(existing) = &state.ordinary_floor_source {
                return Err(HistoryError::MutationRefused(
                    if existing.publication_in_flight() {
                        CoreMutationRefusal::ManualBackpressure
                    } else {
                        CoreMutationRefusal::ManualSaveRequired
                    },
                ));
            }
            Some(Arc::new(source))
        } else {
            if state.ordinary_floor_source.is_some() {
                return Err(HistoryError::MutationRefused(
                    CoreMutationRefusal::ManualHistoryUnavailable,
                ));
            }
            None
        };
        if let Some(pending) = &state.pending_floor {
            return Err(HistoryError::MutationRefused(if pending.save.ready {
                CoreMutationRefusal::ManualSaveRequired
            } else {
                CoreMutationRefusal::ManualBackpressure
            }));
        }
        let mut after = checked_state(state, floor, true);
        after.ordinary_floor_source = pending_source;
        after.history_epoch = next_epoch;
        self.floor_moved = true;
        self.before_floor = Some(std::mem::replace(state, after));
        Ok(())
    }
    fn rollback(self, state: &mut CoreState) {
        if let Some(before) = self.before_floor {
            *state = before;
        }
        self.command.rollback(state);
    }
}

// A proof's document geometry and replay vector capacities bound the actor's
// actual allocations too. Otherwise adopt those allocations; never discount the
// holder's spare capacity when measuring real resident memory.
fn keeps_checked_layout(state: &CoreState, proof: &crate::ResolvedHistory) -> bool {
    let history = proof.snapshot();
    Arc::ptr_eq(&state.baseline, &history.baseline)
        && Arc::ptr_eq(&state.document, &history.document)
        && state.undo.len() == history.cursor.undo.len()
        && state.redo.len() == history.cursor.redo.len()
        && state
            .undo
            .iter()
            .chain(&state.redo)
            .zip(history.cursor.undo.iter().chain(&history.cursor.redo))
            .all(|(live, checked)| Arc::ptr_eq(&live.document, &checked.document))
        && state.batches.capacity() <= proof.checkpoint.batches_capacity
        && state.undo.capacity() <= proof.checkpoint.undo_capacity
        && state.redo.capacity() <= proof.checkpoint.redo_capacity
        && state.op_log.capacity() <= state.op_log.len().saturating_mul(2).max(4)
}
fn checked_state(state: &mut CoreState, proof: crate::ResolvedHistory, moving: bool) -> CoreState {
    let mut after = if moving {
        CoreState::adopt_resolved(proof)
    } else {
        let capacities = (
            proof.checkpoint.batches_capacity,
            proof.checkpoint.undo_capacity,
            proof.checkpoint.redo_capacity,
        );
        CoreState::adopt_snapshot_with_log(
            proof.into_unsealed_snapshot(),
            capacities,
            Some(Arc::clone(&state.op_log)),
        )
    };
    after.history_epoch = state.history_epoch;
    after.revision = state.revision;
    after.next_floor_id = state.next_floor_id;
    if !moving {
        after.pending_floor.clone_from(&state.pending_floor);
        after
            .ordinary_floor_source
            .clone_from(&state.ordinary_floor_source);
    }
    after.identity_effects.clone_from(&state.identity_effects);
    after
}
fn tight_history(mut history: crate::HistorySnapshot) -> crate::HistorySnapshot {
    let mut documents = std::collections::HashMap::new();
    let mut compact = |document: &mut Arc<Document>| {
        let address = Arc::as_ptr(document) as usize;
        let tight = documents
            .entry(address)
            .or_insert_with(|| Arc::new((**document).clone()));
        *document = Arc::clone(tight);
    };
    compact(&mut history.baseline);
    compact(&mut history.document);
    for entry in history
        .cursor
        .undo
        .iter_mut()
        .chain(&mut history.cursor.redo)
    {
        compact(&mut entry.document);
    }
    history
}

struct EntryHead {
    document: Arc<Document>,
    coalesce_key: Option<String>,
    operations: usize,
}
impl From<&HistoryEntry> for EntryHead {
    fn from(entry: &HistoryEntry) -> Self {
        Self {
            document: Arc::clone(&entry.document),
            coalesce_key: entry.coalesce_key.clone(),
            operations: entry.operations.len(),
        }
    }
}

enum StackDelta {
    Edit { redo: Vec<HistoryEntry> },
    Undo { exposed_key: Option<String> },
    Redo,
}

struct CandidateDelta {
    document: Arc<Document>,
    revision: TimelineRevision,
    undo_len: usize,
    redo_len: usize,
    undo_top: Option<EntryHead>,
    redo_top: Option<EntryHead>,
    op_log_len: usize,
    batches_len: usize,
    identity_effects: Vec<OperationIdentityEffect>,
    stacks: StackDelta,
}
impl CandidateDelta {
    fn begin(state: &mut CoreState, command: &Command) -> Self {
        let undo_len = state.undo.len();
        let redo_len = state.redo.len();
        let undo_top = state.undo.last().map(EntryHead::from);
        let redo_top = state.redo.last().map(EntryHead::from);
        let stacks = match command {
            Command::Undo => StackDelta::Undo {
                exposed_key: undo_len
                    .checked_sub(2)
                    .and_then(|i| state.undo[i].coalesce_key.clone()),
            },
            Command::Redo => StackDelta::Redo,
            _ => StackDelta::Edit {
                redo: std::mem::take(&mut state.redo),
            },
        };
        Self {
            document: Arc::clone(&state.document),
            revision: state.revision,
            undo_len,
            redo_len,
            undo_top,
            redo_top,
            op_log_len: state.op_log.len(),
            batches_len: state.batches.len(),
            identity_effects: std::mem::take(&mut state.identity_effects),
            stacks,
        }
    }

    fn cache_prefixes(&self, state: &CoreState) -> (usize, usize) {
        match &self.stacks {
            StackDelta::Edit { .. } => (
                if state.undo.len() == self.undo_len {
                    self.undo_len.saturating_sub(1)
                } else {
                    self.undo_len
                },
                0,
            ),
            StackDelta::Undo { .. } => (
                if state.undo.len() < self.undo_len {
                    self.undo_len.saturating_sub(2)
                } else {
                    self.undo_len
                },
                self.redo_len,
            ),
            StackDelta::Redo => (self.undo_len, state.redo.len()),
        }
    }

    fn rollback(self, state: &mut CoreState) {
        match self.stacks {
            StackDelta::Edit { redo } => {
                state.undo.truncate(self.undo_len);
                if let (Some(entry), Some(head)) = (state.undo.last_mut(), self.undo_top) {
                    entry.operations.truncate(head.operations);
                }
                state.redo = redo;
            }
            StackDelta::Undo { exposed_key } if state.undo.len() < self.undo_len => {
                let entry = state.redo.pop().expect("candidate Undo moved an entry");
                if let Some(exposed) = state.undo.last_mut() {
                    exposed.coalesce_key = exposed_key;
                }
                let head = self.undo_top.expect("candidate Undo had an entry");
                state.undo.push(HistoryEntry {
                    document: head.document,
                    operations: entry.operations,
                    coalesce_key: head.coalesce_key,
                });
            }
            StackDelta::Redo if state.redo.len() < self.redo_len => {
                let entry = state.undo.pop().expect("candidate Redo moved an entry");
                let head = self.redo_top.expect("candidate Redo had an entry");
                state.redo.push(HistoryEntry {
                    document: head.document,
                    operations: entry.operations,
                    coalesce_key: head.coalesce_key,
                });
            }
            StackDelta::Undo { .. } | StackDelta::Redo => {}
        }
        state.document = self.document;
        state.revision = self.revision;
        if state.op_log.len() > self.op_log_len {
            Arc::make_mut(&mut state.op_log).truncate(self.op_log_len);
        }
        state.batches.truncate(self.batches_len);
        state.identity_effects = self.identity_effects;
    }
}

/// Refuse oversized serialized chunks before extending the allocation.
struct BoundedJson {
    bytes: Vec<u8>,
    limit: usize,
}
impl BoundedJson {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}
impl std::io::Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| std::io::Error::other("history capacity"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Comma-separated canonical JSON values, without the surrounding brackets.
/// End offsets permit exact tail replacement without scanning the closed prefix.
#[derive(Default)]
#[cfg_attr(test, derive(Debug, Clone, PartialEq, Eq))]
struct JsonArray {
    bytes: Vec<u8>,
    ends: Vec<usize>,
    boundary_bytes: Vec<u64>,
}
struct ArrayPatch {
    prefix: usize,
    bytes: Vec<u8>,
    ends: Vec<usize>,
    boundary_bytes: Vec<u64>,
}
impl JsonArray {
    fn replace_tail<T: Serialize>(
        &mut self,
        prefix: usize,
        tail: &[T],
    ) -> Result<ArrayPatch, HistoryError> {
        self.replace_tail_with_cost(prefix, tail, |_| Ok(0))
    }
    fn replace_tail_with_cost<T: Serialize>(
        &mut self,
        prefix: usize,
        tail: &[T],
        cost: impl Fn(&T) -> Result<u64, HistoryError>,
    ) -> Result<ArrayPatch, HistoryError> {
        let start = if prefix == 0 {
            0
        } else {
            self.ends[prefix - 1]
        };
        let limit = usize::try_from(HISTORY_BYTES)
            .ok()
            .and_then(|limit| limit.checked_sub(start))
            .ok_or(HistoryError::Capacity)?;
        let mut bytes = BoundedJson::new(limit);
        let mut ends = Vec::new();
        let mut boundary_bytes = Vec::new();
        for value in tail {
            if prefix != 0 || !ends.is_empty() {
                bytes.write_all(b",").map_err(|_| HistoryError::Capacity)?;
            }
            serde_json::to_writer(&mut bytes, value).map_err(|_| HistoryError::Capacity)?;
            ends.push(start + bytes.bytes.len());
            boundary_bytes.push(cost(value)?);
        }
        let patch = ArrayPatch {
            prefix,
            bytes: self.bytes[start..].to_vec(),
            ends: self.ends[prefix..].to_vec(),
            boundary_bytes: self.boundary_bytes[prefix..].to_vec(),
        };
        // S2H_STALE_SEGMENT_ANCHOR: discard replaced cursor bytes before appending.
        self.bytes.truncate(start);
        self.ends.truncate(prefix);
        self.boundary_bytes.truncate(prefix);
        self.bytes.extend(bytes.bytes);
        self.ends.extend(ends);
        self.boundary_bytes.extend(boundary_bytes);
        Ok(patch)
    }

    fn rollback(&mut self, patch: ArrayPatch) {
        let start = if patch.prefix == 0 {
            0
        } else {
            self.ends[patch.prefix - 1]
        };
        self.bytes.truncate(start);
        self.ends.truncate(patch.prefix);
        self.boundary_bytes.truncate(patch.prefix);
        self.bytes.extend(patch.bytes);
        self.ends.extend(patch.ends);
        self.boundary_bytes.extend(patch.boundary_bytes);
    }
}

fn boundary_bytes(entry: &HistoryEntry) -> Result<u64, HistoryError> {
    crate::history::boundary_admission_bytes(
        &entry.document,
        &entry.operations,
        entry.coalesce_key.as_deref(),
    )
}

#[cfg_attr(test, derive(Debug, Clone, PartialEq, Eq))]
struct HistoryJson {
    epoch: u64,
    prefix: Vec<u8>,
    batches: JsonArray,
    undo: JsonArray,
    redo: JsonArray,
    sha256: String,
}
struct HistoryPatch {
    batches: ArrayPatch,
    undo: ArrayPatch,
    redo: ArrayPatch,
}
impl HistoryJson {
    fn new(state: &CoreState) -> Result<Self, HistoryError> {
        #[cfg(test)]
        if tests::cache_failure(1) {
            return Err(HistoryError::Encoding);
        }
        if state.batches.len() > HISTORY_COMMANDS {
            return Err(HistoryError::Capacity);
        }
        let mut prefix = BoundedJson::new(usize::try_from(HISTORY_BYTES).unwrap());
        prefix.write_all(b"{").map_err(|_| HistoryError::Capacity)?;
        prefix
            .write_all(b"\"baseline\":")
            .map_err(|_| HistoryError::Capacity)?;
        serde_json::to_writer(&mut prefix, &state.baseline).map_err(|_| HistoryError::Capacity)?;
        prefix
            .write_all(b",\"document\":")
            .map_err(|_| HistoryError::Capacity)?;
        let mut cache = Self {
            epoch: state.history_epoch,
            prefix: prefix.bytes,
            batches: JsonArray::default(),
            undo: JsonArray::default(),
            redo: JsonArray::default(),
            sha256: String::new(),
        };
        let batches: Vec<_> = state
            .batches
            .iter()
            .map(crate::JournalBatch::from)
            .collect();
        cache.batches.replace_tail(0, &batches)?;
        cache
            .undo
            .replace_tail_with_cost(0, &state.undo, boundary_bytes)?;
        cache
            .redo
            .replace_tail_with_cost(0, &state.redo, boundary_bytes)?;
        cache.sha256 = cache.identities(state)?.0;
        Ok(cache)
    }

    fn update(
        &mut self,
        state: &CoreState,
        undo_prefix: usize,
        redo_prefix: usize,
    ) -> Result<HistoryPatch, HistoryError> {
        #[cfg(test)]
        if tests::cache_failure(2) {
            return Err(HistoryError::Encoding);
        }
        if state.batches.len() > HISTORY_COMMANDS {
            return Err(HistoryError::Capacity);
        }
        let tail: Vec<_> = state.batches[self.batches.ends.len()..]
            .iter()
            .map(crate::JournalBatch::from)
            .collect();
        let batches = self.batches.replace_tail(self.batches.ends.len(), &tail)?;
        let undo = match self.undo.replace_tail_with_cost(
            undo_prefix,
            &state.undo[undo_prefix..],
            boundary_bytes,
        ) {
            Ok(patch) => patch,
            Err(error) => {
                self.batches.rollback(batches);
                return Err(error);
            }
        };
        let redo = match self.redo.replace_tail_with_cost(
            redo_prefix,
            &state.redo[redo_prefix..],
            boundary_bytes,
        ) {
            Ok(patch) => patch,
            Err(error) => {
                self.undo.rollback(undo);
                self.batches.rollback(batches);
                return Err(error);
            }
        };
        Ok(HistoryPatch {
            batches,
            undo,
            redo,
        })
    }

    fn rollback(&mut self, patch: HistoryPatch) {
        self.batches.rollback(patch.batches);
        self.undo.rollback(patch.undo);
        self.redo.rollback(patch.redo);
    }

    fn write_cursor(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        writer.write_all(b"{\"undo\":[")?;
        writer.write_all(&self.undo.bytes)?;
        writer.write_all(b"],\"redo\":[")?;
        writer.write_all(&self.redo.bytes)?;
        writer.write_all(b"]}")
    }

    fn write_history(
        &self,
        state: &CoreState,
        writer: &mut impl std::io::Write,
    ) -> std::io::Result<String> {
        let fixed_bytes = self
            .undo
            .boundary_bytes
            .iter()
            .chain(&self.redo.boundary_bytes)
            .try_fold(
                self.encoded_bytes(state, 0)
                    .map_err(std::io::Error::other)?,
                |total, bytes| {
                    total
                        .checked_add(*bytes)
                        .filter(|bytes| *bytes <= HISTORY_BYTES)
                        .ok_or_else(|| std::io::Error::other("history capacity"))
                },
            )?;
        let remaining = HISTORY_BYTES
            .checked_sub(fixed_bytes)
            .ok_or_else(|| std::io::Error::other("history capacity"))?;
        let mut document = BoundedJson::new(usize::try_from(remaining).unwrap());
        serde_json::to_writer(&mut document, &state.document).map_err(std::io::Error::other)?;
        writer.write_all(&self.prefix)?;
        writer.write_all(&document.bytes)?;
        write!(
            writer,
            ",\"journal\":{{\"floor_offset\":{},\"baseline_sha256\":\"{}\",\"document_sha256\":\"{}\",\"batches\":[",
            state.floor_offset,
            crate::history_document_hash(&state.baseline).map_err(std::io::Error::other)?,
            crate::history_document_hash(&state.document).map_err(std::io::Error::other)?
        )?;
        writer.write_all(&self.batches.bytes)?;
        writer.write_all(b"],\"cursor\":")?;
        let mut cursor = CursorWriter {
            history: writer,
            hash: kinewright_hash::Sha256::new(),
        };
        self.write_cursor(&mut cursor)?;
        let cursor_sha256 = hex(cursor.hash.finalize());
        writer.write_all(b"}}")?;
        Ok(cursor_sha256)
    }

    fn identities(&self, state: &CoreState) -> Result<(String, String), HistoryError> {
        let mut writer = HashWriter {
            hash: kinewright_hash::Sha256::new(),
        };
        let cursor = self
            .write_history(state, &mut writer)
            .map_err(|_| HistoryError::Capacity)?;
        Ok((hex(writer.hash.finalize()), cursor))
    }

    fn encoded_bytes(&self, state: &CoreState, document_bytes: usize) -> Result<u64, HistoryError> {
        // Every historical segment already carries its exact byte length.
        // Only the newly serialized document and changed array tails contribute
        // new measurements; admission never serializes the closed prefix again.
        [
            self.prefix.len(),
            document_bytes,
            b",\"journal\":{\"floor_offset\":,\"baseline_sha256\":\"\",\"document_sha256\":\"\",\"batches\":[".len()
                + state.floor_offset.to_string().len()
                + 128,
            self.batches.bytes.len(),
            b"],\"cursor\":".len(),
            b"{\"undo\":[".len(),
            self.undo.bytes.len(),
            b"],\"redo\":[".len(),
            self.redo.bytes.len(),
            b"]}".len(),
            b"}}".len(),
        ]
        .into_iter()
        .try_fold(0_u64, |total, bytes| {
            total
                .checked_add(bytes as u64)
                .filter(|bytes| *bytes <= HISTORY_BYTES)
                .ok_or(HistoryError::Capacity)
        })
    }
}

struct HashWriter {
    hash: kinewright_hash::Sha256,
}
impl std::io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.hash.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct CursorWriter<'a, W> {
    history: &'a mut W,
    hash: kinewright_hash::Sha256,
}
impl<W: std::io::Write> std::io::Write for CursorWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.history.write_all(bytes)?;
        self.hash.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.history.flush()
    }
}
fn hex(digest: [u8; 32]) -> String {
    let mut result = String::with_capacity(64);
    for byte in digest {
        write!(&mut result, "{byte:02x}").expect("writing a String cannot fail");
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::HistoryBatch;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{
        ActorIdentity, HistorySnapshot, ManualHistoryEncoder, Operation, Query, Track, TrackId,
        TrackKind,
    };

    #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
    struct Record {
        before: String,
        after: String,
        cursor: String,
        revision: TimelineRevision,
        predecessor: Option<String>,
        batch: HistoryBatch,
    }
    struct Encoder {
        records: Arc<Mutex<Vec<Record>>>,
        refusal: Arc<AtomicUsize>,
    }
    impl crate::HistoryAdmission for Encoder {
        fn check_history(
            &mut self,
            history: &HistorySnapshot,
        ) -> Result<Option<crate::ResolvedHistory>, HistoryError> {
            if self.refusal.load(Ordering::Relaxed) == 5 {
                Err(HistoryError::MutationRefused(
                    CoreMutationRefusal::ManualHistoryCapacity,
                ))
            } else if self.refusal.load(Ordering::Relaxed) >= 9 {
                Err(match self.refusal.load(Ordering::Relaxed) {
                    9 => HistoryError::Encoding,
                    10 => HistoryError::Corrupt,
                    11 => HistoryError::StaleExpectation,
                    12 => HistoryError::SaveInProgress,
                    _ => HistoryError::Capacity,
                })
            } else if self.refusal.load(Ordering::Relaxed) >= 6 && history.batches.len() > 1 {
                let selected = crate::HistoryTransition::floor_after(history, 1)?;
                crate::HistoryTransport::capture(&selected)?
                    .resolve_verified()
                    .map(Some)
            } else {
                Ok(None)
            }
        }
        fn finish_history_check(&mut self, _: bool) {}
    }
    impl ManualHistoryEncoder for Encoder {
        fn encode(
            &self,
            sequence: u64,
            revision: TimelineRevision,
            predecessor: Option<&str>,
            candidate: ManualHistoryCandidate<'_>,
        ) -> Result<AcceptedHistory, HistoryError> {
            let record = Record {
                before: candidate.before_sha256.to_owned(),
                after: candidate.after_sha256.to_owned(),
                cursor: candidate.cursor_sha256.to_owned(),
                revision,
                predecessor: predecessor.map(str::to_owned),
                batch: candidate.batch.clone(),
            };
            self.records.lock().unwrap().push(record.clone());
            let mode = self.refusal.load(Ordering::Relaxed);
            if mode == 1 {
                return Err(HistoryError::MutationRefused(
                    CoreMutationRefusal::ManualInputRequired,
                ));
            }
            if mode == 4 || mode == 6 {
                return Err(HistoryError::Capacity);
            }
            let envelope = if mode == 3 || mode == 7 {
                vec![255; crate::MANUAL_BYTES]
            } else {
                serde_json::to_vec(&record).unwrap()
            };
            Ok(AcceptedHistory {
                sequence: sequence + u64::from(mode == 2),
                batch_id: sequence.to_string(),
                digest: kinewright_hash::sha256_bytes(&envelope),
                envelope,
            })
        }
    }

    struct Harness {
        state: CoreState,
        ingress: ManualIngress,
        records: Arc<Mutex<Vec<Record>>>,
        refusal: Arc<AtomicUsize>,
    }
    impl Harness {
        fn new(mut state: CoreState) -> Self {
            let records = Arc::new(Mutex::new(Vec::new()));
            let refusal = Arc::new(AtomicUsize::new(0));
            let ingress = ManualIngress::new(
                &mut state,
                1,
                false,
                Box::new(Encoder {
                    records: Arc::clone(&records),
                    refusal: Arc::clone(&refusal),
                }),
            )
            .unwrap();
            let harness = Self {
                state,
                ingress,
                records,
                refusal,
            };
            harness.check_cache();
            harness
        }

        fn check_cache(&self) {
            let history = self.state.history();
            let full = serde_json::to_vec(&history).unwrap();
            // This is the unchanged project::manual_history::history_sha256 definition.
            let full_history_sha256 = kinewright_hash::sha256_bytes(&full);
            assert_eq!(self.ingress.cache.sha256, full_history_sha256);
            let mut cached = Vec::new();
            let cursor = self
                .ingress
                .cache
                .write_history(&self.state, &mut cached)
                .unwrap();
            assert_eq!(
                cached, full,
                "cached segments must reproduce every v1 JSON byte"
            );
            assert_eq!(
                cursor,
                kinewright_hash::sha256_bytes(
                    &serde_json::to_vec(&crate::HistoryJournal::capture(&history).unwrap().cursor)
                        .unwrap()
                )
            );
            let document_bytes = serde_json::to_vec(&history.document).unwrap().len();
            assert_eq!(
                self.ingress
                    .cache
                    .encoded_bytes(&self.state, document_bytes)
                    .unwrap(),
                history.encoded_bytes().unwrap()
            );
            let ordinary = self
                .ingress
                .cache
                .undo
                .boundary_bytes
                .iter()
                .chain(&self.ingress.cache.redo.boundary_bytes)
                .try_fold(history.encoded_bytes().unwrap(), |total, bytes| {
                    total.checked_add(*bytes)
                })
                .unwrap();
            assert_eq!(
                history.ordinary_admission_bytes(),
                Ok(ordinary),
                "CACHED_ORDINARY_ADMISSION_COUNTS_MATCH"
            );
        }

        fn run(&mut self, command: Command) -> Event {
            let before = self.state.history();
            let revision = self.state.revision;
            let log = self.state.op_log.clone();
            let pending = self.ingress.pending.clone();
            let sequence = self.ingress.next_sequence;
            let predecessor = self.ingress.predecessor.clone();
            let bytes = self.ingress.bytes;
            // A completed floor invalidates the old encoding independently of
            // this edit. Refusal must retain the current cut's canonical cache.
            let cache = if self.ingress.cache.epoch == self.state.history_epoch {
                self.ingress.cache.clone()
            } else {
                HistoryJson::new(&self.state).unwrap()
            };
            let event = self
                .ingress
                .execute(&mut self.state, AttachmentState::Live, command);
            if matches!(
                &event,
                Event::DocumentChanged {
                    journal_command: Some(_),
                    ..
                }
            ) {
                let records = self.records.lock().unwrap();
                let record = records.last().unwrap();
                assert_eq!(
                    record.before,
                    kinewright_hash::sha256_bytes(&serde_json::to_vec(&before).unwrap())
                );
                assert_eq!(record.after, self.ingress.cache.sha256);
                assert_eq!(record.revision, revision);
                assert_eq!(record.predecessor, predecessor);
                assert_eq!(&record.batch, self.state.batches.last().unwrap());
                assert_eq!(self.ingress.pending.len(), pending.len() + 1);
                assert!(
                    pending
                        .iter()
                        .eq(self.ingress.pending.iter().take(pending.len()))
                );
            } else {
                assert_eq!(
                    self.state.history(),
                    before,
                    "refusal must restore the exact cursor and history"
                );
                assert_eq!(self.state.revision, revision);
                assert_eq!(self.state.op_log, log);
                assert_eq!(self.ingress.pending, pending);
                assert_eq!(self.ingress.next_sequence, sequence);
                assert_eq!(self.ingress.predecessor, predecessor);
                assert_eq!(self.ingress.bytes, bytes);
                assert_eq!(
                    self.ingress.cache, cache,
                    "REFUSAL_PRESERVES_EVERY_ENCODER_CACHE_BYTE_AND_INDEX"
                );
            }
            self.check_cache();
            event
        }

        fn acknowledge(&mut self) {
            self.ingress.pending.clear();
            self.ingress.bytes = 0;
        }
    }

    thread_local! { static CACHE_FAILURE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
    pub(super) fn cache_failure(kind: u8) -> bool {
        CACHE_FAILURE.with(|value| value.get() == kind)
    }
    fn with_cache_failure<T>(kind: u8, run: impl FnOnce() -> T) -> T {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                CACHE_FAILURE.with(|v| v.set(0));
            }
        }
        CACHE_FAILURE.with(|value| {
            assert_eq!(value.replace(kind), 0);
        });
        let _reset = Reset;
        run()
    }

    #[test]
    fn delta_manual_ingress_never_discards_an_accepted_delta() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        accepted(&harness.run(add_track(1)));
        accepted(&harness.run(Command::Do(lock(false))));
        accepted(&harness.run(Command::Do(lock(true))));
        harness.acknowledge();
        let before = harness.state.history();
        let cache = harness.ingress.cache.clone();
        harness.ingress.delta_core = true;
        harness.refusal.store(8, Ordering::Relaxed);
        assert!(matches!(
            harness.run(Command::Do(lock(false))),
            Event::MutationRefused {
                reason: CoreMutationRefusal::ThreadHistoryCapacity,
                ..
            }
        ));
        assert_eq!(harness.state.history(), before);
        assert_eq!(harness.ingress.cache, cache);
        assert!(harness.ingress.pending.is_empty());
    }

    #[test]
    fn lease_bind_that_would_change_the_floor_is_an_invariant_refusal_not_an_edit_refusal() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        accepted(&harness.run(add_track(1)));
        accepted(&harness.run(Command::Do(lock(false))));
        accepted(&harness.run(Command::Do(lock(true))));
        harness.acknowledge();
        let before = harness.state.history();
        harness.refusal.store(8, Ordering::Relaxed);
        let encoder = Encoder {
            records: Arc::clone(&harness.records),
            refusal: Arc::clone(&harness.refusal),
        };
        let result = ManualIngress::new(&mut harness.state, 44, false, Box::new(encoder));
        assert!(matches!(result, Err(HistoryError::BarrierInvariant)));
        assert_eq!(harness.state.history(), before);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the failure matrix proves rollback of the same cache, cursor and queue"
    )]
    fn s2h_manual_preparation_failures_are_final_and_transactional() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        accepted(&harness.run(add_track(1)));
        accepted(&harness.run(Command::Do(lock(false))));
        accepted(&harness.run(Command::Do(lock(true))));
        for mode in [2, 4, 6, 9, 10, 11, 12, 13] {
            harness.refusal.store(mode, Ordering::Relaxed);
            for _ in 0..2 {
                assert!(
                    matches!(
                        harness.run(Command::Do(lock(false))),
                        Event::MutationRefused {
                            reason: CoreMutationRefusal::ManualHistoryUnavailable,
                            ..
                        }
                    ),
                    "NON_QUEUE_FAILURE_IS_FINAL mode={mode}"
                );
                harness.acknowledge();
            }
        }
        for mode in [9, 10, 11, 12, 13] {
            harness.refusal.store(mode, Ordering::Relaxed);
            let mut policy = Encoder {
                records: Arc::clone(&harness.records),
                refusal: Arc::clone(&harness.refusal),
            };
            let before = harness.state.history();
            assert!(
                matches!(
                    execute_admitted(
                        &mut harness.state,
                        AttachmentState::Live,
                        Command::Do(lock(false)),
                        &mut policy,
                        Some(96 << 20),
                        false
                    ),
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::ManualHistoryUnavailable,
                        ..
                    }
                ),
                "GENERIC_ADMISSION_FAILURE_IS_FINAL"
            );
            assert_eq!(harness.state.history(), before);
        }
        harness.refusal.store(8, Ordering::Relaxed);
        let mut policy = Encoder {
            records: Arc::clone(&harness.records),
            refusal: Arc::clone(&harness.refusal),
        };
        let before = harness.state.history();
        assert!(
            matches!(
                execute_admitted(
                    &mut harness.state,
                    AttachmentState::Live,
                    Command::Do(lock(false)),
                    &mut policy,
                    Some(0),
                    false
                ),
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualHistoryUnavailable,
                    ..
                }
            ),
            "INTRINSIC_SOURCE_WINDOW_FAILURE_IS_FINAL"
        );
        assert_eq!(harness.state.history(), before);
        harness.refusal.store(0, Ordering::Relaxed);
        with_cache_failure(2, || {
            assert!(matches!(
                harness.run(Command::Do(lock(false))),
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualHistoryUnavailable,
                    ..
                }
            ));
            harness.acknowledge();
            assert!(matches!(
                harness.run(Command::Do(lock(false))),
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualHistoryUnavailable,
                    ..
                }
            ));
        });
        harness.refusal.store(8, Ordering::Relaxed);
        with_cache_failure(1, || {
            assert!(
                matches!(
                    harness.run(Command::Do(lock(false))),
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::ManualHistoryUnavailable,
                        ..
                    }
                ),
                "FLOOR_CACHE_REBUILD_FAILURE_IS_FINAL"
            );
        });
        harness.refusal.store(0, Ordering::Relaxed);
        harness.state.history_epoch += 1;
        let before = harness.state.history();
        let cache = harness.ingress.cache.clone();
        with_cache_failure(1, || {
            assert!(
                matches!(
                    harness.ingress.execute(
                        &mut harness.state,
                        AttachmentState::Live,
                        Command::Do(lock(false))
                    ),
                    Event::MutationRefused {
                        reason: CoreMutationRefusal::ManualHistoryUnavailable,
                        ..
                    }
                ),
                "EPOCH_CACHE_REBUILD_FAILURE_IS_FINAL"
            );
            assert_eq!(harness.state.history(), before);
            assert_eq!(harness.ingress.cache, cache);
        });
        accepted(&harness.run(Command::Do(lock(false))));
        harness.acknowledge();
        for _ in 0..crate::MANUAL_COMMANDS {
            accepted(&harness.run(Command::Redo));
        }
        harness.refusal.store(3, Ordering::Relaxed);
        assert!(
            matches!(
                harness.run(Command::Do(lock(true))),
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualHistoryCapacity,
                    ..
                }
            ),
            "OWN_MESSAGE_SIZE_IS_FINAL_EVEN_WHEN_COUNT_WINDOW_IS_FULL"
        );
    }

    #[test]
    fn astra_review_floor_then_occupied_queue_restores_exact_state() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        accepted(&harness.run(add_track(1)));
        for index in 1..crate::MANUAL_COMMANDS {
            accepted(&harness.run(Command::Do(lock(index % 2 == 0))));
        }
        harness.refusal.store(8, Ordering::Relaxed);
        let epoch = harness.state.history_epoch;
        for _ in 0..2 {
            assert!(matches!(
                harness.run(Command::Do(lock(false))),
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualBackpressure,
                    ..
                }
            ));
            assert_eq!(harness.state.floor_offset, 0);
            assert_eq!(harness.state.history_epoch, epoch);
            assert!(harness.state.ordinary_floor_source.is_none());
        }
        harness.acknowledge();
        accepted(&harness.run(Command::Do(lock(false))));
        assert_eq!(harness.state.floor_offset, 1);
    }

    #[test]
    fn s2h_cached_floor_rolls_back_late_encoding_and_queue_refusals() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        accepted(&harness.run(add_track(1)));
        accepted(&harness.run(Command::Do(lock(false))));
        accepted(&harness.run(Command::Do(lock(true))));
        for mode in [6, 7] {
            harness.refusal.store(mode, Ordering::Relaxed);
            assert!(matches!(
                harness.run(Command::Do(lock(false))),
                Event::MutationRefused {
                    reason,
                    ..
                } if reason == if mode == 6 { CoreMutationRefusal::ManualHistoryUnavailable } else { CoreMutationRefusal::ManualHistoryCapacity }
            ));
        }
        harness.refusal.store(8, Ordering::Relaxed);
        accepted(&harness.run(Command::Do(lock(false))));
        assert_eq!(harness.state.floor_offset, 1);
        harness.refusal.store(0, Ordering::Relaxed);
        accepted(&harness.run(Command::Do(lock(true))));
    }

    fn add_track(id: u64) -> Command {
        Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        })
    }
    fn lock(locked: bool) -> Operation {
        Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }
    }
    fn drag(locked: bool, key: &str) -> Command {
        Command::DoBatchCoalesced {
            operations: vec![lock(locked)],
            coalesce_key: key.into(),
        }
    }
    fn accepted(event: &Event) {
        assert!(
            matches!(
                event,
                Event::DocumentChanged {
                    journal_command: Some(_),
                    ..
                }
            ),
            "{event:?}"
        );
    }

    #[test]
    fn s2h_cached_history_matches_full_after_every_command() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        // Empty-stack Undo/Redo still append an accepted journal batch.
        for command in [
            Command::Undo,
            Command::Redo,
            add_track(1),
            drag(false, "A"),
            drag(true, "A"),
            drag(false, "B"),
            Command::Undo,
            Command::Undo,
            Command::Redo,
            drag(true, "A"),
            Command::Undo,
            Command::Redo,
            Command::Redo,
        ] {
            accepted(&harness.run(command));
        }
        let mut random = 0xc3a5_c85c_97cb_3127_u64;
        for index in 0..240 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let command = match random % 12 {
                0 => Command::Undo,
                1 => Command::Redo,
                2 => Command::Do(lock(random & 16 != 0)),
                3 => Command::DoBatch(vec![lock(false), lock(true)]),
                4 => add_track(1), // Duplicate or a valid branch after Undo removed it.
                5 => Command::DoBatch(vec![]),
                6 => Command::DoBatchIfRevision {
                    expected: TimelineRevision(u64::MAX),
                    operations: vec![lock(true)],
                },
                7 => Command::DoIfRevision {
                    expected: TimelineRevision(u64::MAX),
                    operation: lock(true),
                    token: Some(crate::CommandToken(index)),
                },
                8 => Command::Query(Query::Document),
                9 => drag(random & 16 != 0, "B"),
                _ => drag(random & 16 != 0, "A"),
            };
            harness.run(command);
            harness.acknowledge();
        }
    }

    #[test]
    fn s2h_cached_history_rolls_back_encoder_and_admission_refusals() {
        fn roundtrip(harness: &Harness) {
            let original = harness.state.history();
            let bytes = serde_json::to_vec(&original).unwrap();
            let restored: HistorySnapshot = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(restored, original, "REFUSED_TENTATIVE_EDIT_ROUNDTRIP");
            assert_eq!(restored.applied_operations(), original.applied_operations());
        }
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        for command in [
            add_track(1),
            drag(false, "A"),
            drag(true, "A"),
            drag(false, "B"),
        ] {
            accepted(&harness.run(command));
        }
        for mode in 1..=5 {
            for command in [
                drag(true, "B"),
                Command::Undo,
                Command::Redo,
                add_track(20 + mode as u64),
            ] {
                harness.refusal.store(mode, Ordering::Relaxed);
                assert!(matches!(
                    harness.run(command.clone()),
                    Event::MutationRefused { .. }
                ));
                roundtrip(&harness);
                harness.refusal.store(0, Ordering::Relaxed);
                accepted(&harness.run(command));
                harness.acknowledge();
            }
        }
        // Preserve the detached redo branch after both invalid execution and
        // a valid candidate refused by the encoder, then successfully branch.
        accepted(&harness.run(Command::Undo));
        assert!(!harness.state.redo.is_empty());
        harness.run(Command::DoBatch(vec![]));
        harness.refusal.store(1, Ordering::Relaxed);
        harness.run(drag(true, "branch"));
        roundtrip(&harness);
        harness.refusal.store(0, Ordering::Relaxed);
        accepted(&harness.run(drag(true, "branch")));
        assert!(harness.state.redo.is_empty());
        harness.acknowledge();
        let old_sequence = harness.ingress.next_sequence;
        harness.ingress.next_sequence = u64::MAX;
        assert!(matches!(
            harness.run(Command::Undo),
            Event::MutationRefused { .. }
        ));
        harness.ingress.next_sequence = old_sequence;
        accepted(&harness.run(Command::Undo));
        harness.acknowledge();
        for _ in 0..crate::MANUAL_COMMANDS {
            accepted(&harness.run(Command::Redo));
        }
        assert!(matches!(
            harness.run(Command::Undo),
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualBackpressure,
                ..
            }
        ));
        harness.acknowledge();
        accepted(&harness.run(Command::Undo));
    }

    #[test]
    fn s2h_cached_history_rebinds_after_floor_restore_and_cursor_changes() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        for command in [
            add_track(1),
            Command::Do(lock(false)),
            drag(true, "A"),
            drag(false, "A"),
            drag(true, "B"),
            Command::Undo,
        ] {
            accepted(&harness.run(command));
        }
        let transition = crate::HistoryTransition::floor(harness.state.history(), 1).unwrap();
        assert_eq!(transition.after().floor_offset, 1);
        assert!(!transition.after().cursor.redo.is_empty());
        harness.acknowledge();
        let old_epoch = harness.ingress.cache.epoch;
        // Exercise the same settlement used by the actor's live SettleFloor
        // dispatch, retaining ingress to require its next-command invalidation.
        let ticket = harness
            .state
            .begin_floor(ActorIdentity(77), &transition)
            .unwrap();
        harness.state.settle_floor(ticket, true).unwrap();
        assert_ne!(harness.state.history_epoch, old_epoch);
        assert_eq!(harness.ingress.cache.epoch, old_epoch);
        assert_eq!(harness.state.history(), *transition.after());
        harness.refusal.store(1, Ordering::Relaxed);
        assert!(matches!(
            harness.run(Command::Redo),
            Event::MutationRefused { .. }
        ));
        assert_eq!(harness.ingress.cache.epoch, harness.state.history_epoch);
        harness.refusal.store(0, Ordering::Relaxed);
        for command in [
            Command::Redo,
            drag(false, "A"),
            Command::Undo,
            Command::Redo,
            Command::Undo,
            Command::Do(lock(true)),
        ] {
            accepted(&harness.run(command));
        }
        let wire = serde_json::to_vec(&harness.state.history()).unwrap();
        let snapshot: HistorySnapshot = serde_json::from_slice(&wire).unwrap();
        harness = Harness::new(CoreState::restore(&snapshot).unwrap());
        accepted(&harness.run(drag(false, "restored")));
    }

    #[test]
    fn ordinary_pending_floor_backpressure_clears_on_settlement() {
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        accepted(&harness.run(add_track(1)));
        accepted(&harness.run(Command::Do(lock(false))));
        accepted(&harness.run(Command::Do(lock(true))));
        harness.acknowledge();
        let before = harness.state.history();
        let revision = harness.state.revision;
        let transition = crate::HistoryTransition::floor(before.clone(), 1).unwrap();
        let ticket = harness
            .state
            .begin_floor(ActorIdentity(77), &transition)
            .unwrap();
        harness.refusal.store(8, Ordering::Relaxed);
        let mut policy = Encoder {
            records: Arc::clone(&harness.records),
            refusal: Arc::clone(&harness.refusal),
        };
        let epoch = harness.state.history_epoch;
        harness.state.history_epoch = u64::MAX;
        assert!(matches!(
            execute_admitted(
                &mut harness.state,
                AttachmentState::Live,
                Command::Do(lock(false)),
                &mut policy,
                Some(96 << 20),
                false
            ),
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualHistoryUnavailable,
                ..
            }
        ));
        assert_eq!(harness.state.history(), before);
        assert_eq!(harness.state.revision, revision);
        assert_eq!(harness.state.history_epoch, u64::MAX);
        harness.state.history_epoch = epoch;
        for (budget, reason) in [
            (0, CoreMutationRefusal::ManualHistoryUnavailable),
            (96 << 20, CoreMutationRefusal::ManualBackpressure),
        ] {
            assert!(matches!(
                execute_admitted(&mut harness.state, AttachmentState::Live, Command::Do(lock(false)), &mut policy, Some(budget), false),
                Event::MutationRefused { reason: actual, .. } if actual == reason
            ));
            assert_eq!(harness.state.history(), before);
            assert_eq!(harness.state.revision, revision);
        }
        harness.state.settle_floor(ticket, false).unwrap();
        accepted(&execute_admitted(
            &mut harness.state,
            AttachmentState::Live,
            Command::Do(lock(false)),
            &mut policy,
            Some(96 << 20),
            false,
        ));
        assert_eq!(harness.state.floor_offset, 1);
        let before = harness.state.history();
        let revision = harness.state.revision;
        assert!(matches!(
            execute_admitted(
                &mut harness.state,
                AttachmentState::Live,
                Command::Do(lock(true)),
                &mut policy,
                Some(0),
                false
            ),
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualHistoryUnavailable,
                ..
            }
        ));
        assert_eq!(harness.state.history(), before);
        assert_eq!(harness.state.revision, revision);
    }

    #[test]
    fn s2h_cached_history_rejects_exact_byte_limit_then_accepts() {
        let mut bounded = BoundedJson::new(16);
        assert!(serde_json::to_writer(&mut bounded, &"x".repeat(128)).is_err());
        assert!(
            bounded.bytes.len() <= 16,
            "serialization allocates no oversized chunk"
        );
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        accepted(&harness.run(add_track(1)));
        let attempts = harness.records.lock().unwrap().len();
        // Each individual segment fits, but the batch plus cursor together
        // exceed the exact full-history byte cap. Reject before the encoder.
        let key = "x".repeat(usize::try_from(HISTORY_BYTES / 2).unwrap());
        assert!(matches!(
            harness.run(drag(false, &key)),
            Event::MutationRefused {
                reason: CoreMutationRefusal::ManualHistoryUnavailable,
                ..
            }
        ));
        assert_eq!(harness.records.lock().unwrap().len(), attempts);
        accepted(&harness.run(drag(false, "after-byte-refusal")));
        let document_bytes = serde_json::to_vec(&harness.state.document).unwrap().len();
        let current = harness
            .ingress
            .cache
            .encoded_bytes(&harness.state, document_bytes)
            .unwrap();
        let extra = usize::try_from(HISTORY_BYTES - current).unwrap();
        assert_eq!(
            harness
                .ingress
                .cache
                .encoded_bytes(&harness.state, document_bytes + extra),
            Ok(HISTORY_BYTES)
        );
        assert_eq!(
            harness
                .ingress
                .cache
                .encoded_bytes(&harness.state, document_bytes + extra + 1),
            Err(HistoryError::Capacity)
        );
    }
}
