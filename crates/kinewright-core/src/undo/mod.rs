//! Operation-bearing Undo history. The actor continues to use its existing history.
#![allow(clippy::missing_errors_doc)]

use crate::history_size::ReplayBudget;
use crate::{BatchError, Command, Document, Operation, OperationIdentityEffect};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};

mod cache;
mod record;
use cache::Cache;
pub use record::{Record, ValidatedSnapshot};

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
    /// Proposed ordinary profile: `Fold`, with a 32 MiB retained envelope.
    /// Owners reserve their ledgers and vacant stack storage separately.
    pub const ORDINARY: Self = Self {
        operation_bytes: 512 << 10,
        step_cap: 16_384,
        checkpoint_documents: 4,
        replay_workspace: 32 << 20,
        document_live_bytes: 7 << 19,
        document_bytes: 512 << 10,
        decoded_history_bytes: 5 << 20,
    };
    /// Proposed task profile: `NeverFold`. Below the current 41 MiB resident cap.
    pub const TASK: Self = Self {
        operation_bytes: 24 << 10,
        step_cap: 1024,
        checkpoint_documents: 4,
        replay_workspace: 64 << 20,
        document_live_bytes: 4 << 20,
        document_bytes: 256 << 10,
        decoded_history_bytes: 512 << 10,
    };
    /// Proposed candidate profile: `NeverFold`, within the current 2 MiB cap.
    pub const CANDIDATE: Self = Self {
        operation_bytes: 64 << 10,
        step_cap: 512,
        checkpoint_documents: 0,
        replay_workspace: 64 << 20,
        document_live_bytes: 512 << 10,
        document_bytes: 64 << 10,
        decoded_history_bytes: 512 << 10,
    };
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StepCharge {
    operations: Charge,
    effects_bytes: u64,
    replay_peak: u64,
    key_bytes: u64,
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
    document: Arc<Document>,
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
}

impl History {
    pub fn new(
        document: Document,
        policy: RetentionPolicy,
        bounds: RoleBounds,
    ) -> Result<Self, Error> {
        document.validate().map_err(|error| {
            Error::Batch(BatchError::OperationFailed {
                op_number: 1,
                error,
            })
        })?;
        check_document_value(&document, bounds)?;
        let mut budget =
            ReplayBudget::workspace(bounds.replay_workspace).map_err(|_| Error::Capacity)?;
        budget.check_clone(&document).map_err(|_| Error::Capacity)?;
        // The immutable baseline/current share compact owned storage. A caller's
        // nested Vec/String headroom does not become retained history memory.
        let compact = document.clone();
        drop(document);
        check_document(&compact, bounds)?;
        let document = Arc::new(compact);
        Ok(Self {
            baseline: Arc::clone(&document),
            document,
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
        })
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
    #[must_use]
    pub const fn position(&self) -> CommandKey {
        CommandKey {
            epoch: self.epoch,
            position: self.position,
        }
    }
    #[must_use]
    pub const fn charge(&self) -> Charge {
        Charge {
            encoded: self.undo_charge.encoded + self.redo_charge.encoded,
            decoded: self.undo_charge.decoded + self.redo_charge.decoded,
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
                .checked_add(
                    crate::history_size::document_live_bytes(document)
                        .map_err(|_| Error::Capacity)?,
                )
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

    #[expect(
        clippy::too_many_lines,
        reason = "one atomic edit plans all folds before committing any state"
    )]
    fn edit(&mut self, operations: Vec<Operation>, key: Option<String>) -> Result<Effects, Error> {
        if operations.is_empty() {
            return Err(Error::Batch(BatchError::Empty));
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
            return Err(Error::StepTooLarge);
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
            return Err(Error::HistoryFull);
        }
        let mut charge = operation_charge(&operations)?;
        let normalization_bytes = charge
            .decoded
            .checked_mul(1 + u64::from(self.recorder.is_some()))
            .ok_or(Error::Capacity)?;
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
            return Err(Error::Capacity);
        }
        let mut retained = self.undo_charge.add(charge)?;
        let mut budget =
            ReplayBudget::workspace(self.bounds.replay_workspace).map_err(|_| Error::Capacity)?;
        budget
            .reserve_input(normalization_bytes)
            .map_err(|_| Error::Capacity)?;
        // Retained operations own compact nested Vec/String allocations. Moving
        // caller payloads directly could retain arbitrary spare capacity that
        // the shared structural estimator intentionally does not count.
        let normalized = operations.clone();
        drop(operations);
        let operations = normalized;
        budget
            .check_clone(&self.document)
            .map_err(|_| Error::Capacity)?;
        let mut candidate = (*self.document).clone();
        let identities = apply(&mut candidate, &operations, self.bounds, &mut budget)?;
        let mut summary = replay_charge(top, &budget, identities.len(), self.bounds, 0)?;
        summary.operations = top.operations.add(charge)?;
        summary.key_bytes = key_bytes;
        let accepted = self.position();
        let digest = digest_operations(&operations)?;
        let mut effects = Effects::new(accepted);
        effects.applied.push(CommandEffects {
            key: accepted,
            identities,
        });
        let mut folded_count = 0;
        let mut folded_baseline = None;
        if retained.encoded > self.bounds.operation_bytes || steps > self.bounds.step_cap {
            budget
                .set_document_charge(
                    crate::history_size::document_live_bytes(&candidate)
                        .map_err(|_| Error::Capacity)?,
                )
                .map_err(|_| Error::Capacity)?;
            budget
                .check_clone(&self.baseline)
                .map_err(|_| Error::Capacity)?;
            let mut baseline = (*self.baseline).clone();
            for step in &self.undo {
                if retained.encoded <= self.bounds.operation_bytes && steps <= self.bounds.step_cap
                {
                    break;
                }
                for command in &step.commands {
                    let identities = replay_observed(
                        &mut baseline,
                        &command.operations,
                        self.bounds,
                        &mut budget,
                    )?;
                    effects.folded.push(CommandEffects {
                        key: command.id,
                        identities,
                    });
                }
                retained.subtract(step.charge.operations);
                steps -= 1;
                folded_count += 1;
            }
            folded_baseline = Some(Arc::new(baseline));
        }
        let fold_new = steps > self.bounds.step_cap;
        if fold_new {
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
            return Err(Error::Capacity);
        }
        // Returning discarded keys is part of this transaction's workspace too.
        for step in &self.redo {
            budget
                .reserve_input(
                    ((step.commands.len() + 4) * 2 * std::mem::size_of::<CommandKey>()) as u64,
                )
                .map_err(|_| Error::Capacity)?;
        }
        // Everything fallible happens above; commit metadata, documents and cache together.
        effects
            .discarded
            .extend(self.redo.iter().flat_map(Step::keys));
        effects.to_undo.push(accepted);
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
        if let Some(baseline) = folded_baseline {
            self.baseline = baseline;
        }
        if !extends && !fold_new {
            self.cache
                .insert(self.undo.len() - 1, Arc::clone(&self.document));
        }
        self.document = Arc::new(candidate);
        if fold_new {
            self.baseline = Arc::clone(&self.document);
        }
        self.undo_charge = retained;
        self.position += 1;
        Ok(effects)
    }

    fn undo_step(&mut self) -> Result<Effects, Error> {
        let mut effects = Effects::new(self.position());
        if !self.undo.is_empty() {
            let target = self.undo.len() - 1;
            let document = self.replay_to(target)?;
            let mut step = self.undo.pop_back().unwrap();
            effects.to_redo.extend(step.keys());
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
                .check_clone(&self.document)
                .map_err(|_| Error::Capacity)?;
            let mut document = (*self.document).clone();
            for command in &step.commands {
                let identities =
                    replay_observed(&mut document, &command.operations, self.bounds, &mut budget)?;
                effects.applied.push(CommandEffects {
                    key: command.id,
                    identities,
                });
            }
            let mut step = self.redo.pop().unwrap();
            effects.to_undo.extend(step.keys());
            self.redo_charge.subtract(step.charge.operations);
            self.undo_charge = self
                .undo_charge
                .add(step.charge.operations)
                .expect("unchanged total");
            step.coalesce_key = None;
            self.cache
                .insert(self.undo.len(), Arc::clone(&self.document));
            self.undo.push_back(step);
            self.document = Arc::new(document);
        }
        self.position += 1;
        Ok(effects)
    }
    pub fn boundary_document(&mut self, depth: usize) -> Result<Arc<Document>, Error> {
        if depth > self.undo.len() {
            return Err(Error::InvalidDepth);
        }
        self.replay_to(depth)
    }
    fn replay_to(&mut self, depth: usize) -> Result<Arc<Document>, Error> {
        if depth == self.undo.len() {
            return Ok(Arc::clone(&self.document));
        }
        if depth == 0 {
            return Ok(Arc::clone(&self.baseline));
        }
        if let Some(document) = self.cache.exact(depth) {
            return Ok(document);
        }
        let (start, source) = self.cache.nearest(depth).unwrap_or((0, &self.baseline));
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
            budget.check_clone(source).map_err(|_| Error::Capacity)?;
            let mut document = (**source).clone();
            let mut checkpoints = Vec::new();
            let mut held = 0_u64;
            for (index, step) in self.undo.iter().enumerate().take(depth).skip(start) {
                for command in &step.commands {
                    replay(&mut document, &command.operations, self.bounds, &mut budget)?;
                }
                let boundary = index + 1;
                if warm.contains(&boundary) {
                    let bytes = crate::history_size::document_live_bytes(&document)
                        .map_err(|_| Error::Capacity)?;
                    held = held.checked_add(bytes).ok_or(Error::Capacity)?;
                    budget
                        .set_document_charge(held)
                        .map_err(|_| Error::Capacity)?;
                    budget.check_clone(&document).map_err(|_| Error::Capacity)?;
                    // The scratch remains mutable. This copy belongs only to
                    // the prospective cache, and is committed after success.
                    checkpoints.push((boundary, Arc::new(document.clone())));
                }
            }
            Ok((Arc::new(document), checkpoints))
        };
        let result = rebuild(&warm);
        let (document, checkpoints) = if matches!(&result, Err(Error::Capacity)) && !warm.is_empty()
        {
            rebuild(&[])?
        } else {
            result?
        };
        for (boundary, checkpoint) in checkpoints {
            self.cache.insert_warm(boundary, checkpoint);
        }
        self.cache.insert_replayed(depth, &document);
        Ok(document)
    }
}

fn close_key(step: &mut Step) -> u64 {
    step.coalesce_key = None;
    let released = std::mem::take(&mut step.charge.key_bytes);
    step.charge.operations.decoded -= released;
    released
}
fn check_document_value(document: &Document, bounds: RoleBounds) -> Result<(), Error> {
    if encoded_bytes(document)? > bounds.document_bytes
        || crate::history_size::document_bytes(document, true).map_err(|_| Error::Capacity)?
            > bounds.document_live_bytes
    {
        return Err(Error::DocumentCapacity);
    }
    Ok(())
}
fn check_document(document: &Document, bounds: RoleBounds) -> Result<(), Error> {
    if encoded_bytes(document)? > bounds.document_bytes {
        return Err(Error::DocumentCapacity);
    }
    if crate::history_size::document_live_bytes(document).map_err(|_| Error::Capacity)?
        > bounds.document_live_bytes
    {
        return Err(Error::DocumentCapacity);
    }
    Ok(())
}
fn apply_mode(
    document: &mut Document,
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
        let observed = if observe {
            crate::operation::apply_batch_observed_replay(
                document,
                std::slice::from_ref(operation),
                budget,
            )
        } else {
            crate::operation::apply_batch_replay(document, std::slice::from_ref(operation), budget)
                .map(|()| Vec::new())
        }
        .map_err(|error| match error {
            BatchError::OperationFailed {
                error: crate::OpError::HistoryReplayCapacity,
                ..
            } => Error::Capacity,
            BatchError::OperationFailed { error, .. } => {
                Error::Batch(BatchError::OperationFailed {
                    op_number: index + 1,
                    error,
                })
            }
            BatchError::Empty => unreachable!(),
        })?;
        check_document(document, bounds)?;
        identities.extend(observed);
    }
    Ok(identities)
}

fn observation_bytes(operations: usize) -> Result<u64, Error> {
    // Include transient observation vectors and the minimum/growth capacities of
    // Effects vectors. The same charge is reused when admitting future Redo.
    let bytes = operations
        .checked_mul(std::mem::size_of::<OperationIdentityEffect>())
        .and_then(|value| {
            value.checked_add(
                std::mem::size_of::<CommandEffects>() + 4 * std::mem::size_of::<CommandKey>(),
            )
        })
        .and_then(|value| value.checked_mul(4))
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
    let extra_document = bounds
        .document_live_bytes
        .checked_add(bounds.document_live_bytes.div_ceil(10))
        .ok_or(Error::Capacity)?;
    let replay_peak = previous
        .effects_bytes
        .checked_add(peak)
        .and_then(|value| value.checked_add(extra_document))
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
    })
}
fn apply(
    document: &mut Document,
    operations: &[Operation],
    bounds: RoleBounds,
    budget: &mut ReplayBudget,
) -> Result<Vec<OperationIdentityEffect>, Error> {
    apply_mode(document, operations, bounds, budget, true)
}
fn canonical_workspace(document: &mut Document, budget: &mut ReplayBudget) -> Result<(), Error> {
    let held = budget.document_charge();
    let source = crate::history_size::document_live_bytes(document).map_err(|_| Error::Capacity)?;
    budget
        .set_document_charge(held.checked_add(source).ok_or(Error::Capacity)?)
        .map_err(|_| Error::Capacity)?;
    budget.check_clone(document).map_err(|_| Error::Capacity)?;
    // Every accepted command originally received a fresh scratch allocation. Reproduce that
    // allocation shape so retained Vec capacity cannot make replay exceed admission bounds.
    *document = document.clone();
    budget
        .set_document_charge(held)
        .map_err(|_| Error::Capacity)
}
fn replay_observed(
    document: &mut Document,
    operations: &[Operation],
    bounds: RoleBounds,
    budget: &mut ReplayBudget,
) -> Result<Vec<OperationIdentityEffect>, Error> {
    canonical_workspace(document, budget)?;
    apply(document, operations, bounds, budget)
}
fn replay(
    document: &mut Document,
    operations: &[Operation],
    bounds: RoleBounds,
    budget: &mut ReplayBudget,
) -> Result<(), Error> {
    canonical_workspace(document, budget)?;
    apply_mode(document, operations, bounds, budget, false).map(|_| ())
}
fn operation_charge(operations: &[Operation]) -> Result<Charge, Error> {
    operations
        .iter()
        .try_fold(Charge::default(), |charge, operation| {
            charge.add(Charge {
                encoded: encoded_bytes(operation)?,
                decoded: (std::mem::size_of::<Operation>() as u64)
                    .checked_add(
                        crate::history_size::operation_owned_bytes(operation)
                            .map_err(|_| Error::Capacity)?,
                    )
                    .ok_or(Error::Capacity)?,
            })
        })
}
fn encoded_bytes<T: Serialize + ?Sized>(value: &T) -> Result<u64, Error> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
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
