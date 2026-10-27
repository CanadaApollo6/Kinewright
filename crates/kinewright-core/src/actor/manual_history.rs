//! Transactional manual admission and the exact, cached v1 JSON byte stream.
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
        .map_err(|_| HistoryError::Capacity)
}

impl ManualIngress {
    pub(super) fn new(
        state: &CoreState,
        generation: u64,
        encoder: Box<dyn crate::ManualHistoryEncoder>,
    ) -> Result<Self, HistoryError> {
        Ok(Self {
            generation,
            encoder,
            next_sequence: 1,
            predecessor: None,
            pending: VecDeque::new(),
            bytes: 0,
            cache: HistoryJson::new(state)?,
        })
    }

    pub(super) fn execute(
        &mut self,
        state: &mut CoreState,
        attachment: AttachmentState,
        command: Command,
    ) -> Event {
        if matches!(command, Command::Query(_)) || attachment != AttachmentState::Live {
            return execute_actor_command(state, attachment, command);
        }
        if self.pending.len() >= crate::MANUAL_COMMANDS {
            return manual_refusal(&command, CoreMutationRefusal::ManualBackpressure);
        }
        if self.cache.epoch != state.history_epoch {
            match HistoryJson::new(state) {
                Ok(cache) => self.cache = cache,
                Err(_) => {
                    return manual_refusal(&command, CoreMutationRefusal::ManualBackpressure);
                }
            }
        }
        // The actor is the sole reader/writer during admission. Retain only the
        // pieces this command can change, and restore them on every refusal.
        let delta = CandidateDelta::begin(state, &command);
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
        let (undo_prefix, redo_prefix) = delta.cache_prefixes(state);
        let Ok(patches) = self.cache.update(state, undo_prefix, redo_prefix) else {
            delta.rollback(state);
            return manual_refusal(&command, CoreMutationRefusal::ManualBackpressure);
        };
        let admitted = (|| {
            let (after_sha256, cursor_sha256) = self.cache.identities(state)?;
            let item = self.encoder.encode(
                self.next_sequence,
                delta.revision,
                self.predecessor.as_deref(),
                ManualHistoryCandidate {
                    document: &state.document,
                    batch: state.batches.last().ok_or(HistoryError::Corrupt)?,
                    before_sha256: &self.cache.sha256,
                    after_sha256: &after_sha256,
                    cursor_sha256: &cursor_sha256,
                },
            )?;
            let bytes = manual_message_bytes(&item)?;
            let next_sequence = self
                .next_sequence
                .checked_add(1)
                .ok_or(HistoryError::Capacity)?;
            if item.sequence != self.next_sequence
                || bytes > crate::MANUAL_BYTES.saturating_sub(self.bytes)
            {
                return Err(HistoryError::Capacity);
            }
            Ok((item, bytes, next_sequence, after_sha256))
        })();
        match admitted {
            Ok((item, bytes, next_sequence, after_sha256)) => {
                self.next_sequence = next_sequence;
                self.predecessor = Some(item.batch_id.clone());
                self.bytes += bytes;
                self.pending.push_back(item);
                // The previous after identity is the next before identity.
                self.cache.sha256 = after_sha256;
                event
            }
            Err(error) => {
                self.cache.rollback(patches);
                delta.rollback(state);
                let reason = match error {
                    HistoryError::MutationRefused(reason) => reason,
                    _ => CoreMutationRefusal::ManualBackpressure,
                };
                manual_refusal(&command, reason)
            }
        }
    }
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
        state.op_log.truncate(self.op_log_len);
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
struct JsonArray {
    bytes: Vec<u8>,
    ends: Vec<usize>,
}
struct ArrayPatch {
    prefix: usize,
    bytes: Vec<u8>,
    ends: Vec<usize>,
}
impl JsonArray {
    fn replace_tail<T: Serialize>(
        &mut self,
        prefix: usize,
        tail: &[T],
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
        for value in tail {
            if prefix != 0 || !ends.is_empty() {
                bytes.write_all(b",").map_err(|_| HistoryError::Capacity)?;
            }
            serde_json::to_writer(&mut bytes, value).map_err(|_| HistoryError::Capacity)?;
            ends.push(start + bytes.bytes.len());
        }
        let patch = ArrayPatch {
            prefix,
            bytes: self.bytes[start..].to_vec(),
            ends: self.ends[prefix..].to_vec(),
        };
        // S2H_STALE_SEGMENT_ANCHOR: discard replaced cursor bytes before appending.
        self.bytes.truncate(start);
        self.ends.truncate(prefix);
        self.bytes.extend(bytes.bytes);
        self.ends.extend(ends);
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
        self.bytes.extend(patch.bytes);
        self.ends.extend(patch.ends);
    }
}

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
        if state.batches.len() > HISTORY_COMMANDS {
            return Err(HistoryError::Capacity);
        }
        let mut prefix = BoundedJson::new(usize::try_from(HISTORY_BYTES).unwrap());
        prefix.write_all(b"{").map_err(|_| HistoryError::Capacity)?;
        if state.floor_offset != 0 {
            write!(&mut prefix, "\"floor_offset\":{},", state.floor_offset)
                .map_err(|_| HistoryError::Capacity)?;
        }
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
        cache.batches.replace_tail(0, &state.batches)?;
        cache.undo.replace_tail(0, &state.undo)?;
        cache.redo.replace_tail(0, &state.redo)?;
        cache.sha256 = cache.identities(state)?.0;
        Ok(cache)
    }

    fn update(
        &mut self,
        state: &CoreState,
        undo_prefix: usize,
        redo_prefix: usize,
    ) -> Result<HistoryPatch, HistoryError> {
        if state.batches.len() > HISTORY_COMMANDS {
            return Err(HistoryError::Capacity);
        }
        let batches = self.batches.replace_tail(
            self.batches.ends.len(),
            &state.batches[self.batches.ends.len()..],
        )?;
        let undo = match self
            .undo
            .replace_tail(undo_prefix, &state.undo[undo_prefix..])
        {
            Ok(patch) => patch,
            Err(error) => {
                self.batches.rollback(batches);
                return Err(error);
            }
        };
        let redo = match self
            .redo
            .replace_tail(redo_prefix, &state.redo[redo_prefix..])
        {
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
            .encoded_bytes(0)
            .map_err(|_| std::io::Error::other("history capacity"))?;
        let remaining = HISTORY_BYTES
            .checked_sub(fixed_bytes)
            .ok_or_else(|| std::io::Error::other("history capacity"))?;
        let mut document = BoundedJson::new(usize::try_from(remaining).unwrap());
        serde_json::to_writer(&mut document, &state.document).map_err(std::io::Error::other)?;
        writer.write_all(&self.prefix)?;
        writer.write_all(&document.bytes)?;
        writer.write_all(b",\"batches\":[")?;
        writer.write_all(&self.batches.bytes)?;
        writer.write_all(b"],\"cursor\":")?;
        let mut cursor = CursorWriter {
            history: writer,
            hash: kinewright_hash::Sha256::new(),
        };
        self.write_cursor(&mut cursor)?;
        let cursor_sha256 = hex(cursor.hash.finalize());
        writer.write_all(b"}")?;
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

    fn encoded_bytes(&self, document_bytes: usize) -> Result<u64, HistoryError> {
        // Every historical segment already carries its exact byte length.
        // Only the newly serialized document and changed array tails contribute
        // new measurements; admission never serializes the closed prefix again.
        [
            self.prefix.len(),
            document_bytes,
            b",\"batches\":[".len(),
            self.batches.bytes.len(),
            b"],\"cursor\":".len(),
            b"{\"undo\":[".len(),
            self.undo.bytes.len(),
            b"],\"redo\":[".len(),
            self.redo.bytes.len(),
            b"]}".len(),
            b"}".len(),
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
            if mode == 4 {
                return Err(HistoryError::Capacity);
            }
            let envelope = if mode == 3 {
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
        fn new(state: CoreState) -> Self {
            let records = Arc::new(Mutex::new(Vec::new()));
            let refusal = Arc::new(AtomicUsize::new(0));
            let ingress = ManualIngress::new(
                &state,
                1,
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
                kinewright_hash::sha256_bytes(&serde_json::to_vec(&history.cursor).unwrap())
            );
            let document_bytes = serde_json::to_vec(&history.document).unwrap().len();
            assert_eq!(
                self.ingress.cache.encoded_bytes(document_bytes).unwrap(),
                history.encoded_bytes().unwrap()
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
            }
            self.check_cache();
            event
        }

        fn acknowledge(&mut self) {
            self.ingress.pending.clear();
            self.ingress.bytes = 0;
        }
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
        let mut harness = Harness::new(CoreState::new(Document::default(), TimelineRevision(0)));
        for command in [
            add_track(1),
            drag(false, "A"),
            drag(true, "A"),
            drag(false, "B"),
        ] {
            accepted(&harness.run(command));
        }
        for mode in 1..=4 {
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
                reason: CoreMutationRefusal::ManualBackpressure,
                ..
            }
        ));
        assert_eq!(harness.records.lock().unwrap().len(), attempts);
        accepted(&harness.run(drag(false, "after-byte-refusal")));
        let document_bytes = serde_json::to_vec(&harness.state.document).unwrap().len();
        let current = harness.ingress.cache.encoded_bytes(document_bytes).unwrap();
        let extra = usize::try_from(HISTORY_BYTES - current).unwrap();
        assert_eq!(
            harness.ingress.cache.encoded_bytes(document_bytes + extra),
            Ok(HISTORY_BYTES)
        );
        assert_eq!(
            harness
                .ingress
                .cache
                .encoded_bytes(document_bytes + extra + 1),
            Err(HistoryError::Capacity)
        );
    }
}
