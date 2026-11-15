//! Operation-bearing Undo history. The actor continues to use its existing history.
#![allow(clippy::missing_errors_doc)]

use crate::history_size::ReplayBudget;
use crate::{BatchError, Command, Document, Operation, OperationIdentityEffect};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};

mod cache;
mod capacity;
pub mod metrics;
mod size;
use capacity::Heap;
pub use metrics::PhaseCounters;
use size::DocumentBound;
mod fork;
mod merge;
mod record;
use cache::Cache;
pub use record::{Record, ValidatedSnapshot};

/// Structural JSON decoder ceiling, also enforced by live owner forecasts.
pub const DECODED_PAYLOAD_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandKey {
    pub epoch: u64,
    pub position: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetentionPolicy {
    Fold,
    NeverFold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleBounds {
    pub operation_bytes: u64,
    pub step_cap: usize,
    pub checkpoint_documents: usize,
    pub replay_workspace: u64,
    pub document_live_bytes: u64,
    pub document_bytes: u64,
    pub decoded_history_bytes: u64,
}

impl RoleBounds {
    /// A measurement or caller may lower fixed bounds, never enlarge its lease.
    #[must_use]
    pub fn intersect(self, maximum: Self) -> Self {
        Self {
            operation_bytes: self.operation_bytes.min(maximum.operation_bytes),
            decoded_history_bytes: self
                .decoded_history_bytes
                .min(maximum.decoded_history_bytes),
            step_cap: self.step_cap.min(maximum.step_cap),
            checkpoint_documents: self.checkpoint_documents.min(maximum.checkpoint_documents),
            replay_workspace: self.replay_workspace.min(maximum.replay_workspace),
            document_live_bytes: self.document_live_bytes.min(maximum.document_live_bytes),
            document_bytes: self.document_bytes.min(maximum.document_bytes),
        }
    }
    /// Product profile. Owners add their independently accounted 32 MiB allowance.
    pub const ORDINARY: Self = Self {
        operation_bytes: 1 << 20,
        step_cap: 16_384,
        checkpoint_documents: 4,
        replay_workspace: 168 << 20,
        document_live_bytes: 8 << 20,
        document_bytes: 8 << 20,
        decoded_history_bytes: 24 << 20,
    };
    /// Creation sizing lowers document room; history room is fixed for life.
    pub const TASK: Self = Self {
        step_cap: 4096,
        checkpoint_documents: 1,
        ..Self::ORDINARY
    };
    /// Live candidates have task history room; frozen-delta staging may lower it.
    pub const CANDIDATE: Self = Self {
        checkpoint_documents: 0,
        ..Self::TASK
    };
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Charge {
    pub encoded: u64,
    pub decoded: u64,
}
impl Charge {
    fn add(self, other: Self) -> Result<Self, Error> {
        Ok(Self {
            encoded: self
                .encoded
                .checked_add(other.encoded)
                .ok_or(Error::Capacity)?,
            decoded: self
                .decoded
                .checked_add(other.decoded)
                .ok_or(Error::Capacity)?,
        })
    }
    fn subtract(&mut self, other: Self) {
        self.encoded -= other.encoded;
        self.decoded -= other.decoded;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepCommand {
    /// Original acceptance identity; installing a snapshot never changes it.
    pub id: CommandKey,
    pub position: u64,
    /// Digest of the ordered operation array, independent of envelope receipt IDs.
    pub payload_digest: [u8; 32],
    pub operations: Box<[Operation]>,
}
/// Fixed-size identity of an exact command prefix, independent of its payload allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepEvidence {
    pub first_key: CommandKey,
    pub first_digest: [u8; 32],
    pub commands: u64,
    pub digest: [u8; 32],
}
impl StepEvidence {
    /// Hash keys and payload digests in order. No operation or document is copied.
    #[must_use]
    pub fn from_commands(commands: impl Iterator<Item = (CommandKey, [u8; 32])>) -> Option<Self> {
        let mut prefix = StepEvidenceBuilder::new();
        for (key, digest) in commands {
            prefix.push(key, digest)?;
        }
        prefix.evidence()
    }
}
/// One streaming encoder for all prefixes of a folded step.
pub struct StepEvidenceBuilder {
    first: Option<(CommandKey, [u8; 32])>,
    commands: u64,
    digest: kinewright_hash::Sha256,
}
impl Default for StepEvidenceBuilder {
    fn default() -> Self {
        Self::new()
    }
}
impl StepEvidenceBuilder {
    #[must_use]
    pub fn new() -> Self {
        let mut digest = kinewright_hash::Sha256::new();
        digest.update(b"Kinewright step commands\0");
        Self {
            first: None,
            commands: 0,
            digest,
        }
    }
    pub fn push(&mut self, key: CommandKey, payload: [u8; 32]) -> Option<()> {
        self.commands = self.commands.checked_add(1)?;
        self.first.get_or_insert((key, payload));
        self.digest.update(&key.epoch.to_le_bytes());
        self.digest.update(&key.position.to_le_bytes());
        self.digest.update(&payload);
        Some(())
    }
    #[must_use]
    pub fn evidence(&self) -> Option<StepEvidence> {
        let (first_key, first_digest) = self.first?;
        // Finalize this prefix independently, then continue the same command
        // stream for the next approval; the fixed hash state owns no payload.
        let mut digest = self.digest.clone();
        digest.update(&self.commands.to_le_bytes());
        Some(StepEvidence {
            first_key,
            first_digest,
            commands: self.commands,
            digest: digest.finalize(),
        })
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StepCharge {
    operations: Charge,
    effects_bytes: u64,
    replay_peak: u64,
    merge_scratch: u64,
    key_bytes: u64,
    after: DocumentBound,
    validated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub(crate) commands: Vec<StepCommand>,
    pub(crate) coalesce_key: Option<Box<str>>,
    #[serde(skip)]
    pub(crate) charge: StepCharge,
}
impl Step {
    #[must_use]
    pub fn evidence(&self) -> Option<StepEvidence> {
        self.prefix_evidence(self.commands.len())
    }
    #[must_use]
    pub fn prefix_evidence(&self, count: usize) -> Option<StepEvidence> {
        if count > self.commands.len() {
            return None;
        }
        StepEvidence::from_commands(
            self.commands
                .iter()
                .take(count)
                .map(|command| (command.id, command.payload_digest)),
        )
    }
    /// Build untrusted wire data. Only Record validation grants a replay proof.
    #[must_use]
    pub fn unvalidated(commands: Vec<StepCommand>, coalesce_key: Option<Box<str>>) -> Self {
        Self {
            commands,
            coalesce_key,
            charge: StepCharge::default(),
        }
    }
    #[must_use]
    pub fn commands(&self) -> &[StepCommand] {
        &self.commands
    }
    #[must_use]
    pub fn coalesce_key(&self) -> Option<&str> {
        self.coalesce_key.as_deref()
    }
    #[must_use]
    pub const fn charge(&self) -> Charge {
        self.charge.operations
    }
    pub fn operations(&self) -> impl Iterator<Item = &Operation> {
        self.commands.iter().flat_map(|c| c.operations.iter())
    }
    fn keys(&self) -> impl Iterator<Item = CommandKey> {
        self.commands.iter().map(|c| c.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEffects {
    pub key: CommandKey,
    pub identities: Vec<OperationIdentityEffect>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effects {
    pub accepted: CommandKey,
    pub to_undo: Vec<CommandKey>,
    pub to_redo: Vec<CommandKey>,
    pub discarded: Vec<CommandKey>,
    pub folded: Vec<CommandEffects>,
    pub applied: Vec<CommandEffects>,
}

/// Borrowed transaction evidence. The owner prepares its fallible metadata before
/// the executor commits, without capturing another document or operation list.
pub struct EditEvent<'a> {
    pub effects: &'a Effects,
    pub operations: &'a [Operation],
    pub payload_digest: [u8; 32],
    pub step_first: CommandKey,
    folded_steps: &'a std::collections::VecDeque<Step>,
    folded_old_steps: usize,
    folded_new: bool,
}
pub struct FoldedCommand<'a> {
    pub key: CommandKey,
    pub payload_digest: [u8; 32],
    pub operations: &'a [Operation],
    pub step_first: CommandKey,
}
impl EditEvent<'_> {
    /// Borrow exactly the folded prefix, never copy an operation for the owner.
    pub fn folded_commands(&self) -> impl Iterator<Item = FoldedCommand<'_>> {
        self.folded_steps
            .iter()
            .take(self.folded_old_steps)
            .flat_map(|step| {
                let first = step.commands[0].id;
                step.commands.iter().map(move |command| FoldedCommand {
                    key: command.id,
                    payload_digest: command.payload_digest,
                    operations: &command.operations,
                    step_first: first,
                })
            })
            .chain(self.folded_new.then_some(FoldedCommand {
                key: self.effects.accepted,
                payload_digest: self.payload_digest,
                operations: self.operations,
                step_first: self.step_first,
            }))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PreparedError<E> {
    History(Error),
    Owner(E),
}

/// Explicit whole-document capacity accounting for validation and diagnostics.
/// It measures the supplied allocation directly, without a compacting copy.
pub fn document_capacity_bytes(document: &Document) -> Result<u64, Error> {
    capacity::document_bytes(document)
}

impl<E> From<Error> for PreparedError<E> {
    fn from(error: Error) -> Self {
        Self::History(error)
    }
}
impl Effects {
    fn new(accepted: CommandKey) -> Self {
        Self {
            accepted,
            to_undo: vec![],
            to_redo: vec![],
            discarded: vec![],
            folded: vec![],
            applied: vec![],
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("one step exceeds the history budget")]
    StepTooLarge,
    #[error("the fixed-base history is full")]
    HistoryFull,
    #[error("history replay or allocation exceeds its bound")]
    Capacity,
    #[error("document exceeds its live bound")]
    DocumentCapacity,
    #[error("invalid history operation: {0}")]
    Batch(#[from] BatchError),
    #[error("accepted command counter or epoch exhausted")]
    CounterExhausted,
    #[error("unsupported history command")]
    UnsupportedCommand,
    #[error("history encoding failed")]
    Encoding,
    #[error("boundary depth is outside Undo")]
    InvalidDepth,
    #[error("invalid record encoding")]
    InvalidRecord,
    #[error("invalid history command metadata")]
    InvalidMetadata,
    #[error("invalid snapshot document: {0}")]
    InvalidDocument(crate::OpError),
    #[error("Undo replay does not equal the saved document")]
    DocumentMismatch,
    #[error("invalid Undo operations: {0}")]
    InvalidUndo(BatchError),
    #[error("invalid Redo operations: {0}")]
    InvalidRedo(BatchError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    // Baseline/current are shared with immutable captures and cache entries, never mutated through Arc.
    baseline: Arc<Document>,
    baseline_bound: DocumentBound,
    document: Arc<Document>,
    document_bound: DocumentBound,
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    position: u64,
    epoch: u64,
    policy: RetentionPolicy,
    bounds: RoleBounds,
    undo_charge: Charge,
    redo_charge: Charge,
    cache: Cache,
    recorder: Option<Vec<Operation>>,
    recorder_bytes: u64,
    merge_guard: Option<merge::MergeGuard>,
}

impl History {
    pub fn new(
        document: impl Into<Arc<Document>>,
        policy: RetentionPolicy,
        bounds: RoleBounds,
    ) -> Result<Self, Error> {
        let document = document.into();
        document.validate().map_err(|error| {
            Error::Batch(BatchError::OperationFailed {
                op_number: 1,
                error,
            })
        })?;
        let document_bound = DocumentBound::measure(&document, bounds)?;
        // A long-lived initial baseline may need replay padding. Preserve all
        // supplied spare capacity; never add a normalisation copy to an edit.
        let document = if capacity::document_bytes(&document)? < document_bound.live {
            Arc::new(capacity::padded_baseline(&document))
        } else {
            document
        };
        Ok(Self {
            baseline: Arc::clone(&document),
            baseline_bound: document_bound,
            document,
            document_bound,
            undo: VecDeque::new(),
            redo: Vec::new(),
            position: 0,
            epoch: 0,
            policy,
            bounds,
            undo_charge: Charge::default(),
            redo_charge: Charge::default(),
            cache: Cache::new(bounds.checkpoint_documents, bounds.decoded_history_bytes)?,
            recorder: None,
            recorder_bytes: 0,
            merge_guard: None,
        })
    }
    /// Configure the known fresh merge destination at construction/open only.
    /// The running guard makes admission O(1), independent of retained depth.
    pub fn with_merge_destination(mut self, input: u64) -> Result<Self, Error> {
        if self.policy != RetentionPolicy::NeverFold {
            return Err(Error::InvalidMetadata);
        }
        self.merge_guard = Some(merge::MergeGuard::from_steps(
            input,
            &self.undo,
            &self.redo,
            self.bounds.replay_workspace,
        )?);
        Ok(self)
    }
    pub fn with_recorder(
        document: Document,
        policy: RetentionPolicy,
        bounds: RoleBounds,
    ) -> Result<Self, Error> {
        let mut history = Self::new(document, policy, bounds)?;
        history.recorder = Some(Vec::new());
        Ok(history)
    }
    /// A caller explicitly opts in before the endpoint accepts any activity.
    pub fn recording(mut self) -> Result<Self, Error> {
        if self.position != 0 || !self.undo.is_empty() || !self.redo.is_empty() {
            return Err(Error::InvalidMetadata);
        }
        self.recorder = Some(Vec::new());
        Ok(self)
    }
    #[must_use]
    pub fn document(&self) -> &Arc<Document> {
        &self.document
    }
    #[must_use]
    pub fn baseline(&self) -> &Arc<Document> {
        &self.baseline
    }
    #[must_use]
    pub fn undo(&self) -> &VecDeque<Step> {
        &self.undo
    }
    #[must_use]
    pub fn redo(&self) -> &[Step] {
        &self.redo
    }
    /// Consume an independently held reader without copying operation payloads.
    #[must_use]
    pub fn into_applied_delta(self) -> Vec<Operation> {
        let count = self
            .undo
            .iter()
            .flat_map(Step::commands)
            .map(|command| command.operations.len())
            .sum();
        let mut operations = Vec::with_capacity(count);
        for step in self.undo {
            for command in step.commands {
                operations.extend(command.operations.into_vec());
            }
        }
        operations
    }
    /// Consume a private reader's already validated stacks, retaining no cache.
    #[must_use]
    pub fn into_record(self) -> Record {
        Record {
            baseline: self.baseline,
            document: self.document,
            undo: self.undo,
            redo: self.redo,
            position: self.position,
            epoch: self.epoch,
            policy: self.policy,
        }
    }
    #[must_use]
    pub const fn position(&self) -> CommandKey {
        CommandKey {
            epoch: self.epoch,
            position: self.position,
        }
    }
    /// Select the install epoch for a private publication preview of this
    /// already validated state. Retained commands keep their original keys;
    /// the peer adopts this epoch after validating the published record.
    pub fn begin_install_epoch(&mut self) -> Result<CommandKey, Error> {
        self.epoch = self.epoch.checked_add(1).ok_or(Error::CounterExhausted)?;
        Ok(self.position())
    }
    #[must_use]
    pub const fn charge(&self) -> Charge {
        Charge {
            encoded: self.undo_charge.encoded + self.redo_charge.encoded,
            decoded: self.undo_charge.decoded + self.redo_charge.decoded,
        }
    }
    #[must_use]
    pub const fn document_bound(&self) -> Charge {
        Charge {
            encoded: self.document_bound.encoded,
            decoded: self.document_bound.live,
        }
    }
    #[must_use]
    pub const fn bounds(&self) -> RoleBounds {
        self.bounds
    }
    #[must_use]
    pub const fn policy(&self) -> RetentionPolicy {
        self.policy
    }
    #[must_use]
    pub fn op_log(&self) -> Option<&[Operation]> {
        self.recorder.as_deref()
    }
    pub fn applied_delta(&self) -> impl Iterator<Item = &Operation> {
        self.undo.iter().flat_map(Step::operations)
    }
    /// A producer's encoded outbox occupies one document-anchor allowance.
    /// This is configured once at binding, before commands are accepted; it
    /// neither changes the fixed profile nor allocates another history lease.
    pub fn reserve_stream_buffer(&mut self, peak_bytes: u64) -> Result<(), Error> {
        let anchors = self
            .bounds
            .checkpoint_documents
            .checked_sub(1)
            .ok_or(Error::Capacity)?;
        if peak_bytes > self.bounds.document_live_bytes {
            return Err(Error::Capacity);
        }
        self.cache = Cache::new(anchors, self.bounds.decoded_history_bytes)?;
        Ok(())
    }
    pub fn release_stream_buffer(&mut self) -> Result<(), Error> {
        self.cache = Cache::new(
            self.bounds.checkpoint_documents,
            self.bounds.decoded_history_bytes,
        )?;
        Ok(())
    }
    pub fn drop_cache(&mut self) {
        self.cache.clear();
    }
    #[must_use]
    pub fn cache_depths(&self) -> Vec<usize> {
        self.cache.depths()
    }

    /// Conservative live allocation charge, including vacant stack capacity and
    /// distinct cached documents. Immutable captures held by callers are separate.
    pub fn retained_live_bound(&self) -> Result<u64, Error> {
        let mut documents: Vec<&Arc<Document>> = vec![&self.baseline];
        for document in std::iter::once(&self.document).chain(self.cache.documents()) {
            if !documents.iter().any(|old| Arc::ptr_eq(old, document)) {
                documents.push(document);
            }
        }
        let mut total = std::mem::size_of::<Self>() as u64
            + ((self.undo.capacity() + self.redo.capacity()) * std::mem::size_of::<Step>()) as u64
            + self.cache.storage_bytes();
        for document in documents {
            total = total
                .checked_add(capacity::document_bytes(document)?)
                .ok_or(Error::Capacity)?;
        }
        for step in self.undo.iter().chain(&self.redo) {
            let metadata = (step.commands.capacity() * std::mem::size_of::<StepCommand>()) as u64
                + step.coalesce_key.as_ref().map_or(0, |key| key.len() as u64);
            total = total.checked_add(metadata).ok_or(Error::Capacity)?;
            for command in &step.commands {
                total = total
                    .checked_add(operation_charge(&command.operations)?.decoded)
                    .ok_or(Error::Capacity)?;
            }
        }
        if let Some(recorder) = &self.recorder {
            total = total
                .checked_add(operation_charge(recorder)?.decoded)
                .and_then(|bytes| {
                    bytes.checked_add(
                        ((recorder.capacity() - recorder.len()) * std::mem::size_of::<Operation>())
                            as u64,
                    )
                })
                .ok_or(Error::Capacity)?;
        }
        Ok(total)
    }
    pub fn execute(&mut self, command: Command) -> Result<Effects, Error> {
        self.next_position()?;
        match command {
            Command::Undo => self.undo_step(),
            Command::Redo => self.redo_step(),
            Command::Do(operation) => self.edit(vec![operation], None),
            Command::DoBatch(operations) => self.edit(operations, None),
            Command::DoBatchCoalesced {
                operations,
                coalesce_key,
            } => self.edit(operations, Some(coalesce_key)),
            _ => Err(Error::UnsupportedCommand),
        }
    }
    pub fn close_gesture(&mut self) -> Result<Effects, Error> {
        self.next_position()?;
        let effects = Effects::new(self.position());
        if self
            .undo
            .back()
            .is_some_and(|step| step.coalesce_key.is_some())
        {
            // Match the old recovery Undo/Redo pair's temporarily exposed entry as well.
            for step in self.undo.iter_mut().rev().take(2) {
                self.undo_charge.decoded -= close_key(step);
            }
        }
        self.position += 1;
        Ok(effects)
    }
    fn next_position(&self) -> Result<u64, Error> {
        self.position.checked_add(1).ok_or(Error::CounterExhausted)
    }

    fn edit(&mut self, operations: Vec<Operation>, key: Option<String>) -> Result<Effects, Error> {
        match self.edit_prepared(
            operations,
            key,
            0,
            |_| Ok::<_, std::convert::Infallible>(()),
        ) {
            Ok((effects, ())) => Ok(effects),
            Err(PreparedError::History(error)) => Err(error),
            Err(PreparedError::Owner(never)) => match never {},
        }
    }

    /// Atomically prepare an owner's metadata for an edit. Undo/Redo and close
    /// report allocation-free fate moves through the existing effects API.
    pub fn execute_prepared<P, E>(
        &mut self,
        command: Command,
        owner_workspace: u64,
        prepare: impl FnOnce(EditEvent<'_>) -> Result<P, E>,
    ) -> Result<(Effects, P), PreparedError<E>> {
        self.next_position()?;
        match command {
            Command::Do(operation) => {
                self.edit_prepared(vec![operation], None, owner_workspace, prepare)
            }
            Command::DoBatch(operations) => {
                self.edit_prepared(operations, None, owner_workspace, prepare)
            }
            Command::DoBatchCoalesced {
                operations,
                coalesce_key,
            } => self.edit_prepared(operations, Some(coalesce_key), owner_workspace, prepare),
            _ => Err(PreparedError::History(Error::UnsupportedCommand)),
        }
    }

    /// Rebuild one reachable document and its owner's lineage in one replay.
    /// The caller retains only its baseline seeds, never a map per boundary.
    pub fn visit_boundary<E>(
        &self,
        depth: usize,
        owner_workspace: u64,
        mut visit: impl FnMut(&StepCommand, &[OperationIdentityEffect]) -> Result<(), E>,
    ) -> Result<Arc<Document>, PreparedError<E>> {
        if depth > self.undo.len() + self.redo.len() {
            return Err(Error::InvalidDepth.into());
        }
        let mut budget =
            ReplayBudget::workspace(self.bounds.replay_workspace).map_err(|_| Error::Capacity)?;
        budget
            .reserve_input(owner_workspace)
            .map_err(|_| Error::Capacity)?;
        budget
            .check_known(self.baseline_bound.live)
            .map_err(|_| Error::Capacity)?;
        let mut document = capacity::copy_document(&self.baseline);
        for step in self.undo.iter().chain(self.redo.iter().rev()).take(depth) {
            let effects = replay_step(&mut document, step, &mut budget, true)?;
            for (command, effect) in step.commands.iter().zip(effects) {
                visit(command, &effect.identities).map_err(PreparedError::Owner)?;
            }
        }
        Ok(Arc::new(document))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one atomic edit plans all folds before committing any state"
    )]
    fn edit_prepared<P, E>(
        &mut self,
        operations: Vec<Operation>,
        key: Option<String>,
        owner_workspace: u64,
        prepare: impl FnOnce(EditEvent<'_>) -> Result<P, E>,
    ) -> Result<(Effects, P), PreparedError<E>> {
        if operations.is_empty() {
            return Err(Error::Batch(BatchError::Empty).into());
        }
        let old_depth = self.undo.len();
        let extends = key.is_some()
            && self
                .undo
                .back()
                .is_some_and(|step| step.coalesce_key.as_deref() == key.as_deref());
        let top = if extends {
            self.undo.back().unwrap().charge
        } else {
            StepCharge::default()
        };
        let encoded = operations.iter().try_fold(0_u64, |bytes, operation| {
            bytes
                .checked_add(encoded_bytes(operation)?)
                .ok_or(Error::Capacity)
        })?;
        if top
            .operations
            .encoded
            .checked_add(encoded)
            .ok_or(Error::Capacity)?
            > self.bounds.operation_bytes
        {
            return Err(Error::StepTooLarge.into());
        }
        let mut steps = self.undo.len() + usize::from(!extends);
        if self.policy == RetentionPolicy::NeverFold
            && (self
                .undo_charge
                .encoded
                .checked_add(encoded)
                .ok_or(Error::Capacity)?
                > self.bounds.operation_bytes
                || steps > self.bounds.step_cap)
        {
            return Err(Error::HistoryFull.into());
        }
        let mut charge = operation_charge(&operations)?;
        let recorder_bytes = if self.recorder.is_some() {
            self.recorder_bytes
                .checked_add(charge.decoded)
                .and_then(|bytes| {
                    bytes.checked_add((operations.len() * std::mem::size_of::<Operation>()) as u64)
                })
                .ok_or(Error::Capacity)?
        } else {
            0
        };
        charge.decoded = charge
            .decoded
            .checked_add((std::mem::size_of::<StepCommand>() * 2) as u64)
            .ok_or(Error::Capacity)?;
        let key_bytes = if extends {
            top.key_bytes
        } else {
            key.as_deref().map(encoded_bytes).transpose()?.unwrap_or(0)
        };
        if !extends {
            charge.decoded = charge
                .decoded
                .checked_add((std::mem::size_of::<Step>() * 2) as u64 + key_bytes)
                .ok_or(Error::Capacity)?;
        }
        if top
            .operations
            .decoded
            .checked_add(charge.decoded)
            .and_then(|bytes| bytes.checked_add(recorder_bytes))
            .ok_or(Error::Capacity)?
            > self.bounds.decoded_history_bytes
        {
            return Err(Error::Capacity.into());
        }
        let mut retained = self.undo_charge.add(charge)?;
        let mut budget =
            ReplayBudget::workspace(self.bounds.replay_workspace).map_err(|_| Error::Capacity)?;
        budget
            .reserve_input(owner_workspace)
            .map_err(|_| Error::Capacity)?;
        budget
            .check_known(self.document_bound.live)
            .map_err(|_| Error::Capacity)?;
        let mut candidate = capacity::copy_document(&self.document);
        let mut candidate_bound = self.document_bound;
        let identities = apply(
            &mut candidate,
            &mut candidate_bound,
            &operations,
            self.bounds,
            &mut budget,
        )?;
        let mut summary =
            replay_charge(top, &budget, identities.len(), self.bounds, owner_workspace)?;
        summary.operations = top.operations.add(charge)?;
        summary.after = candidate_bound;
        summary.validated = true;
        summary.key_bytes = key_bytes;
        let merge_guard = self
            .merge_guard
            .map(|guard| guard.after_edit(top, summary, self.bounds.replay_workspace))
            .transpose()?;
        let accepted = self.position();
        let digest = digest_operations(&operations)?;
        let mut effects = Effects::new(accepted);
        effects.applied.push(CommandEffects {
            key: accepted,
            identities,
        });
        let mut folded_count = 0;
        let mut folded_baseline = None;
        if (retained.encoded > self.bounds.operation_bytes || steps > self.bounds.step_cap)
            && !self.undo.is_empty()
        {
            budget
                .set_document_charge(candidate_bound.live)
                .map_err(|_| Error::Capacity)?;
            budget
                .check_known(self.baseline_bound.live)
                .map_err(|_| Error::Capacity)?;
            let mut baseline = capacity::copy_document(&self.baseline);
            for step in &self.undo {
                if retained.encoded <= self.bounds.operation_bytes && steps <= self.bounds.step_cap
                {
                    break;
                }
                effects
                    .folded
                    .extend(replay_step(&mut baseline, step, &mut budget, true)?);
                // Prior folded effects remain owned by the transaction while
                // the next validated step is replayed.
                budget
                    .reserve_input(step.charge.effects_bytes)
                    .map_err(|_| Error::Capacity)?;
                retained.subtract(step.charge.operations);
                steps -= 1;
                folded_count += 1;
            }
            let baseline_bound = DocumentBound::measure(&baseline, self.bounds)?;
            folded_baseline = Some((Arc::new(baseline), baseline_bound));
        }
        let fold_new = steps > self.bounds.step_cap;
        if fold_new {
            candidate_bound = DocumentBound::measure(&candidate, self.bounds)?;
            summary.after = candidate_bound;
            retained.subtract(charge);
            folded_count += 1;
            // Current and baseline consumers receive independent identity observations.
            budget
                .reserve_input(summary.effects_bytes)
                .map_err(|_| Error::Capacity)?;
            effects.folded.push(effects.applied[0].clone());
        }
        if retained
            .decoded
            .checked_add(recorder_bytes)
            .ok_or(Error::Capacity)?
            > self.bounds.decoded_history_bytes
        {
            return Err(Error::Capacity.into());
        }
        // Returning discarded keys is part of this transaction's workspace too.
        for step in &self.redo {
            budget
                .reserve_input(
                    ((step.commands.len() + 4) * 2 * std::mem::size_of::<CommandKey>()) as u64,
                )
                .map_err(|_| Error::Capacity)?;
        }
        effects
            .discarded
            .extend(self.redo.iter().flat_map(Step::keys));
        effects.to_undo.push(accepted);
        let prepared = prepare(EditEvent {
            effects: &effects,
            operations: &operations,
            payload_digest: digest,
            step_first: if extends {
                self.undo.back().unwrap().commands[0].id
            } else {
                accepted
            },
            folded_steps: &self.undo,
            folded_old_steps: folded_count - usize::from(fold_new),
            folded_new: fold_new,
        })
        .map_err(PreparedError::Owner)?;
        // Both the executor and its owner have finished all fallible preparation.
        if let Some(recorder) = &mut self.recorder {
            recorder.extend(operations.iter().cloned());
            self.recorder_bytes = recorder_bytes;
        }
        let command = StepCommand {
            id: accepted,
            position: accepted.position,
            payload_digest: digest,
            operations: operations.into_boxed_slice(),
        };
        if extends {
            self.undo.back_mut().unwrap().commands.push(command);
            self.undo.back_mut().unwrap().charge = summary;
        } else {
            self.undo.push_back(Step {
                commands: vec![command],
                coalesce_key: key.map(String::into_boxed_str),
                charge: summary,
            });
        }
        self.redo.clear();
        self.redo_charge = Charge::default();
        for _ in 0..folded_count {
            self.undo.pop_front();
        }
        self.cache.truncate(if extends {
            old_depth.saturating_sub(1)
        } else {
            old_depth
        });
        self.cache.fold(folded_count);
        if let Some((baseline, bound)) = folded_baseline {
            self.baseline = baseline;
            self.baseline_bound = bound;
        }
        if !extends && !fold_new {
            self.cache.insert(
                self.undo.len() - 1,
                Arc::clone(&self.document),
                self.document_bound,
            );
        }
        self.document = Arc::new(candidate);
        self.document_bound = candidate_bound;
        if fold_new {
            self.baseline = Arc::clone(&self.document);
            self.baseline_bound = candidate_bound;
        }
        self.undo_charge = retained;
        self.merge_guard = merge_guard;
        self.position += 1;
        Ok((effects, prepared))
    }

    fn undo_step(&mut self) -> Result<Effects, Error> {
        let mut effects = Effects::new(self.position());
        if !self.undo.is_empty() {
            let target = self.undo.len() - 1;
            let (document, bound) = self.replay_to(target)?;
            let mut step = self.undo.pop_back().unwrap();
            effects.to_redo.extend(step.keys());
            if let Some(guard) = &mut self.merge_guard {
                guard.undo_effects -= step.charge.effects_bytes;
                guard.redo_effects += step.charge.effects_bytes;
            }
            self.undo_charge.subtract(step.charge.operations);
            close_key(&mut step);
            self.redo_charge = self
                .redo_charge
                .add(step.charge.operations)
                .expect("unchanged total");
            if let Some(exposed) = self.undo.back_mut() {
                self.undo_charge.decoded -= close_key(exposed);
            }
            self.redo.push(step);
            self.document = document;
            self.document_bound = bound;
        }
        self.position += 1;
        Ok(effects)
    }
    fn redo_step(&mut self) -> Result<Effects, Error> {
        let mut effects = Effects::new(self.position());
        if let Some(step) = self.redo.last() {
            let mut budget = ReplayBudget::workspace(self.bounds.replay_workspace)
                .map_err(|_| Error::Capacity)?;
            budget
                .check_prevalidated_peak(step.charge.replay_peak)
                .map_err(|_| Error::Capacity)?;
            let mut document = capacity::copy_document(&self.document);
            effects.applied = replay_step(&mut document, step, &mut budget, true)?;
            let bound = step.charge.after;
            let mut step = self.redo.pop().unwrap();
            effects.to_undo.extend(step.keys());
            if let Some(guard) = &mut self.merge_guard {
                guard.redo_effects -= step.charge.effects_bytes;
                guard.undo_effects += step.charge.effects_bytes;
            }
            self.redo_charge.subtract(step.charge.operations);
            self.undo_charge = self
                .undo_charge
                .add(step.charge.operations)
                .expect("unchanged total");
            step.coalesce_key = None;
            self.cache.insert(
                self.undo.len(),
                Arc::clone(&self.document),
                self.document_bound,
            );
            self.undo.push_back(step);
            self.document = Arc::new(document);
            self.document_bound = bound;
        }
        self.position += 1;
        Ok(effects)
    }
    pub fn boundary_document(&mut self, depth: usize) -> Result<Arc<Document>, Error> {
        self.boundary_document_with_bound(depth)
            .map(|(document, _)| document)
    }
    pub fn boundary_document_with_bound(
        &mut self,
        depth: usize,
    ) -> Result<(Arc<Document>, Charge), Error> {
        if depth > self.undo.len() {
            return Err(Error::InvalidDepth);
        }
        self.replay_to(depth).map(|(document, bound)| {
            (
                document,
                Charge {
                    encoded: bound.encoded,
                    decoded: bound.live,
                },
            )
        })
    }
    fn replay_to(&mut self, depth: usize) -> Result<(Arc<Document>, DocumentBound), Error> {
        if depth == self.undo.len() {
            return Ok((Arc::clone(&self.document), self.document_bound));
        }
        if depth == 0 {
            return Ok((Arc::clone(&self.baseline), self.baseline_bound));
        }
        if let Some(document) = self.cache.exact(depth) {
            return Ok(document);
        }
        let (start, source, source_bound) =
            self.cache
                .nearest(depth)
                .unwrap_or((0, &self.baseline, self.baseline_bound));
        let warm = self.cache.warm_depths(start, depth);
        // Cache planning owns its temporary snapshots until the entire replay
        // succeeds. A tight workspace retries without warming; cache policy
        // cannot turn an otherwise valid Undo into a refusal.
        let rebuild = |warm: &[usize]| -> Result<_, Error> {
            let mut budget = ReplayBudget::workspace(self.bounds.replay_workspace)
                .map_err(|_| Error::Capacity)?;
            budget
                .reserve_input(
                    (warm.len() * 2 * std::mem::size_of::<(usize, Arc<Document>)>()) as u64,
                )
                .map_err(|_| Error::Capacity)?;
            budget
                .check_known(source_bound.live)
                .map_err(|_| Error::Capacity)?;
            let mut document = capacity::copy_document(source);
            let mut checkpoints = Vec::new();
            let mut held = 0_u64;
            for (index, step) in self.undo.iter().enumerate().take(depth).skip(start) {
                replay_step(&mut document, step, &mut budget, false)?;
                let boundary = index + 1;
                if warm.contains(&boundary) {
                    let bound = DocumentBound::measure(&document, self.bounds)?;
                    let bytes = bound.live;
                    held = held.checked_add(bytes).ok_or(Error::Capacity)?;
                    budget
                        .set_document_charge(held)
                        .map_err(|_| Error::Capacity)?;
                    budget
                        .check_known(bound.live)
                        .map_err(|_| Error::Capacity)?;
                    // The scratch remains mutable. This copy belongs only to
                    // the prospective cache, and is committed after success.
                    checkpoints.push((
                        boundary,
                        Arc::new(capacity::copy_document(&document)),
                        bound,
                    ));
                }
            }
            let bound = self.undo[depth - 1].charge.after;
            Ok((Arc::new(document), bound, checkpoints))
        };
        let result = rebuild(&warm);
        let (document, bound, checkpoints) =
            if matches!(&result, Err(Error::Capacity)) && !warm.is_empty() {
                rebuild(&[])?
            } else {
                result?
            };
        for (boundary, checkpoint, bound) in checkpoints {
            self.cache.insert_warm(boundary, checkpoint, bound);
        }
        self.cache.insert_replayed(depth, &document, bound);
        Ok((document, bound))
    }
}

fn close_key(step: &mut Step) -> u64 {
    step.coalesce_key = None;
    let released = std::mem::take(&mut step.charge.key_bytes);
    step.charge.operations.decoded -= released;
    released
}
fn apply_mode(
    document: &mut Document,
    size: &mut DocumentBound,
    operations: &[Operation],
    bounds: RoleBounds,
    budget: &mut ReplayBudget,
    observe: bool,
) -> Result<Vec<OperationIdentityEffect>, Error> {
    if operations.is_empty() {
        return Err(Error::Batch(BatchError::Empty));
    }
    if observe {
        budget
            .reserve_input(observation_bytes(operations.len())?)
            .map_err(|_| Error::Capacity)?;
    }
    let mut identities = Vec::with_capacity(if observe { operations.len() } else { 0 });
    for (index, operation) in operations.iter().enumerate() {
        let growth = match size::reservation(document, operation, *size, budget) {
            Ok(growth) => growth,
            Err(Error::Capacity) => {
                // A loose document bound may cross the workspace limit before
                // the document limit. Tighten at that limit as well; never
                // refuse a source-proven merge because of spare upper bound.
                *size = DocumentBound::measure(document, bounds)?;
                size::reservation(document, operation, *size, budget)?
            }
            Err(error) => return Err(error),
        };
        let effects = if observe {
            crate::operation::apply_batch_observed_in_place(
                document,
                std::slice::from_ref(operation),
            )
        } else {
            crate::operation::apply_batch_prevalidated(document, std::slice::from_ref(operation))
                .map(|()| Vec::new())
        }
        .map_err(|error| match error {
            BatchError::OperationFailed { error, .. } => {
                Error::Batch(BatchError::OperationFailed {
                    op_number: index + 1,
                    error,
                })
            }
            BatchError::Empty => unreachable!(),
        })?;
        size.grow(growth, document, bounds)?;
        for effect in &effects {
            budget
                .retain_identity(effect)
                .map_err(|_| Error::Capacity)?;
        }
        identities.extend(effects);
    }
    Ok(identities)
}

fn observation_bytes(operations: usize) -> Result<u64, Error> {
    // apply_mode preallocates exactly N observations. Only one kernel result
    // Vec (one observation) is transient at a time. CommandEffects and fate
    // vectors grow geometrically, so reserve four slots per command/container.
    let bytes = operations
        .checked_add(1)
        .and_then(|n| n.checked_mul(std::mem::size_of::<OperationIdentityEffect>()))
        .and_then(|n| n.checked_add(4 * std::mem::size_of::<CommandEffects>()))
        .and_then(|n| n.checked_add(16 * std::mem::size_of::<CommandKey>()))
        .ok_or(Error::Capacity)?;
    Ok(bytes as u64)
}

fn replay_charge(
    previous: StepCharge,
    budget: &ReplayBudget,
    observations: usize,
    bounds: RoleBounds,
    input: u64,
) -> Result<StepCharge, Error> {
    let peak = budget
        .workspace_peak_without(input)
        .map_err(|_| Error::Capacity)?;
    let metadata = observation_bytes(observations)?;
    let effects = budget
        .identity_charge()
        .checked_add(metadata)
        .ok_or(Error::Capacity)?;
    let effects = effects
        .checked_add(effects.div_ceil(10))
        .ok_or(Error::Capacity)?;
    let replay_peak = previous
        .effects_bytes
        .checked_add(peak)
        .ok_or(Error::Capacity)?
        .max(previous.replay_peak);
    if replay_peak > bounds.replay_workspace {
        return Err(Error::Capacity);
    }
    Ok(StepCharge {
        operations: previous.operations,
        key_bytes: previous.key_bytes,
        effects_bytes: previous
            .effects_bytes
            .checked_add(effects)
            .ok_or(Error::Capacity)?,
        replay_peak,
        merge_scratch: previous.merge_scratch.max(
            budget
                .kernel_peak_with_margin()
                .map_err(|_| Error::Capacity)?,
        ),
        after: previous.after,
        validated: previous.validated,
    })
}
fn apply(
    document: &mut Document,
    size: &mut DocumentBound,
    operations: &[Operation],
    bounds: RoleBounds,
    budget: &mut ReplayBudget,
) -> Result<Vec<OperationIdentityEffect>, Error> {
    apply_mode(document, size, operations, bounds, budget, true)
}
fn replay_step(
    document: &mut Document,
    step: &Step,
    budget: &mut ReplayBudget,
    observe: bool,
) -> Result<Vec<CommandEffects>, Error> {
    assert!(step.charge.validated, "VALIDATED_REDO_ONLY");
    budget
        .check_prevalidated_peak(step.charge.replay_peak)
        .map_err(|_| Error::Capacity)?;
    let mut effects = Vec::new();
    for command in &step.commands {
        if observe {
            let identities =
                crate::operation::apply_batch_observed_in_place(document, &command.operations)?;
            effects.push(CommandEffects {
                key: command.id,
                identities,
            });
        } else {
            crate::operation::apply_batch_prevalidated(document, &command.operations)?;
        }
    }
    Ok(effects)
}
/// Exact admission charge for one plain batch, without applying it or copying a document.
pub fn batch_charge(operations: &[Operation]) -> Result<Charge, Error> {
    batch_charge_iter(operations)
}
/// Exact one-batch charge of a borrowed stream; no payload is copied for sizing.
pub fn batch_charge_iter<'a>(
    operations: impl IntoIterator<Item = &'a Operation>,
) -> Result<Charge, Error> {
    let mut charge = operation_charge_iter(operations)?;
    charge.decoded = charge
        .decoded
        .checked_add(
            (2 * std::mem::size_of::<StepCommand>() + 2 * std::mem::size_of::<Step>()) as u64,
        )
        .ok_or(Error::Capacity)?;
    Ok(charge)
}
fn operation_charge(operations: &[Operation]) -> Result<Charge, Error> {
    operation_charge_iter(operations)
}
fn operation_charge_iter<'a>(
    operations: impl IntoIterator<Item = &'a Operation>,
) -> Result<Charge, Error> {
    operations
        .into_iter()
        .try_fold(Charge::default(), |charge, operation| {
            charge.add(Charge {
                encoded: encoded_bytes(operation)?,
                decoded: (std::mem::size_of::<Operation>() as u64)
                    .checked_add(
                        crate::history_size::operation_owned_bytes(operation)
                            .map_err(|_| Error::Capacity)?
                            .max(operation.heap()?),
                    )
                    .ok_or(Error::Capacity)?,
            })
        })
}
fn encoded_bytes<T: Serialize + ?Sized>(value: &T) -> Result<u64, Error> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            metrics::serialized(bytes.len() as u64);
            self.0 = self
                .0
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| std::io::Error::other("encoded size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|_| Error::Encoding)?;
    Ok(counter.0)
}
fn digest_operations(operations: &[Operation]) -> Result<[u8; 32], Error> {
    digest(operations)
}
fn digest<T: Serialize + ?Sized>(value: &T) -> Result<[u8; 32], Error> {
    struct Writer(kinewright_hash::Sha256);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            metrics::serialized(bytes.len() as u64);
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer(kinewright_hash::Sha256::new());
    serde_json::to_writer(&mut writer, value).map_err(|_| Error::Encoding)?;
    Ok(writer.0.finalize())
}

/// Capacity of a borrowed clip, including vacant payload storage.
pub fn clip_capacity_bytes(clip: &crate::Clip) -> Result<u64, Error> {
    use capacity::Heap as _;
    clip.heap()?
        .checked_add(std::mem::size_of::<crate::Clip>() as u64)
        .ok_or(Error::Capacity)
}
/// Allocation preflight for untrusted owner bodies before typed decoding.
pub fn payload_memory_bound(bytes: &[u8]) -> Result<u64, Error> {
    crate::history_size::payload_memory_bound(bytes).map_err(|_| Error::Capacity)
}

#[cfg(test)]
mod charge_tests {
    use super::*;
    #[test]
    fn map_heavy_payload_uses_the_typed_capacity_floor() {
        let mut cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/support/history_operation_cases.json"
        ))
        .unwrap();
        let mut operation: Operation =
            serde_json::from_value(cases["AddEffect"]["operation"].take()).unwrap();
        let Operation::AddEffect { effect, .. } = &mut operation else {
            panic!("effect fixture")
        };
        effect.parameters = (0..50)
            .map(|n| (format!("parameter_{n:02}"), crate::ParamValue::Integer(n)))
            .collect();
        let structural = crate::history_size::operation_owned_bytes(&operation).unwrap();
        let typed = operation.heap().unwrap();
        assert!(
            typed > structural,
            "MAP_HEAVY_TYPED_WITNESS {typed} <= {structural}"
        );
        let charge = operation_charge(std::slice::from_ref(&operation)).unwrap();
        assert!(
            charge.decoded >= std::mem::size_of::<Operation>() as u64 + typed,
            "MAP_PAYLOAD_TYPED_CAPACITY_FLOOR"
        );
    }
}

#[cfg(test)]
mod merge_tests;

#[cfg(test)]
mod fork_tests;
