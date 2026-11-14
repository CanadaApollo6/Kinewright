use super::{
    Cache, Charge, CommandKey, DocumentBound, Error, History, RetentionPolicy, RoleBounds, Step,
    digest_operations, operation_charge,
};
use crate::{Document, history_size::ReplayBudget};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

/// Immutable capture. Documents are shared with the live history; command payloads are owned by the capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub baseline: Arc<Document>,
    pub document: Arc<Document>,
    pub undo: std::collections::VecDeque<Step>,
    pub redo: Vec<Step>,
    pub position: u64,
    pub epoch: u64,
    pub policy: RetentionPolicy,
}

/// Constructed only by bounded semantic validation. Cache entries never cross this boundary.
#[derive(Debug)]
pub struct ValidatedSnapshot {
    record: Record,
    bounds: RoleBounds,
    undo_charge: Charge,
    redo_charge: Charge,
    baseline_bound: DocumentBound,
    document_bound: DocumentBound,
}
impl Record {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let bytes = serde_json::to_vec(self).map_err(|_| Error::Encoding)?;
        super::metrics::serialized(bytes.len() as u64);
        Ok(bytes)
    }
    pub fn snapshot_identity(&self) -> Result<[u8; 32], Error> {
        super::digest(self)
    }
    pub fn decode(bytes: &[u8], bounds: RoleBounds) -> Result<ValidatedSnapshot, Error> {
        if bytes.len() as u64 > bounds.replay_workspace {
            return Err(Error::Capacity);
        }
        let input =
            crate::history_size::payload_memory_bound(bytes).map_err(|_| Error::Capacity)?;
        if input > bounds.replay_workspace {
            return Err(Error::Capacity);
        }
        let record: Self = serde_json::from_slice(bytes).map_err(|_| Error::InvalidRecord)?;
        record.validate_with_input(bounds, input)
    }
    pub fn validate(self, bounds: RoleBounds) -> Result<ValidatedSnapshot, Error> {
        self.validate_with_input(bounds, 0)
    }
    #[expect(
        clippy::too_many_lines,
        reason = "validate one record and both replay streams before constructing the installation token"
    )]
    fn validate_with_input(
        mut self,
        bounds: RoleBounds,
        input: u64,
    ) -> Result<ValidatedSnapshot, Error> {
        self.baseline.validate().map_err(Error::InvalidDocument)?;
        self.document.validate().map_err(Error::InvalidDocument)?;
        let baseline_bound = DocumentBound::measure(&self.baseline, bounds)?;
        DocumentBound::measure(&self.document, bounds)?;
        if self
            .undo
            .len()
            .checked_add(self.redo.len())
            .ok_or(Error::Capacity)?
            > bounds.step_cap
        {
            return Err(Error::Capacity);
        }
        // A typed capture may have reserved vacant queue storage. Installation
        // retains values, rather than the source owner's allocation headroom.
        self.undo.shrink_to_fit();
        self.redo.shrink_to_fit();
        for step in self.undo.iter_mut().chain(&mut self.redo) {
            step.commands.shrink_to_fit();
        }
        let mut keys = BTreeSet::new();
        let mut position = None;
        let mut previous_key = None;
        let mut undo_charge = Charge::default();
        let mut redo_charge = Charge::default();
        let undo_depth = self.undo.len();
        for (index, step) in self
            .undo
            .iter_mut()
            .chain(self.redo.iter_mut().rev())
            .enumerate()
        {
            if step.commands.is_empty() {
                return Err(Error::InvalidMetadata);
            }
            let mut total = Charge::default();
            for command in &step.commands {
                if command.operations.is_empty()
                    || command.id.position != command.position
                    || command.position >= self.position
                    || command.id.epoch > self.epoch
                    || !keys.insert(command.id)
                    || position.is_some_and(|old| command.position <= old)
                    || previous_key.is_some_and(|old| command.id <= old)
                    || digest_operations(&command.operations)? != command.payload_digest
                {
                    return Err(Error::InvalidMetadata);
                }
                position = Some(command.position);
                previous_key = Some(command.id);
                let mut command_charge = operation_charge(&command.operations)?;
                command_charge.decoded = command_charge
                    .decoded
                    .checked_add((std::mem::size_of::<super::StepCommand>() * 2) as u64)
                    .ok_or(Error::Capacity)?;
                total = total.add(command_charge)?;
            }
            step.charge.key_bytes = step
                .coalesce_key
                .as_deref()
                .map(super::encoded_bytes)
                .transpose()?
                .unwrap_or(0);
            total.decoded = total
                .decoded
                .checked_add((std::mem::size_of::<Step>() * 2) as u64 + step.charge.key_bytes)
                .ok_or(Error::Capacity)?;
            step.charge.operations = total;
            if index < undo_depth {
                undo_charge = undo_charge.add(total)?;
            } else {
                redo_charge = redo_charge.add(total)?;
            }
        }
        let total = undo_charge.add(redo_charge)?;
        if total.encoded > bounds.operation_bytes || total.decoded > bounds.decoded_history_bytes {
            return Err(Error::Capacity);
        }
        if self.redo.iter().any(|step| step.coalesce_key.is_some()) {
            return Err(Error::InvalidMetadata);
        }
        // Every untrusted record loses its runtime proof before replay, even
        // when a typed capture originally came from a live History.
        for step in self.undo.iter_mut().chain(&mut self.redo) {
            step.charge.validated = false;
        }
        drop(keys);
        let mut budget =
            ReplayBudget::workspace(bounds.replay_workspace).map_err(|_| Error::Capacity)?;
        budget.reserve_input(input).map_err(|_| Error::Capacity)?;
        budget
            .check_known(baseline_bound.live)
            .map_err(|_| Error::Capacity)?;
        let mut document = super::capacity::copy_document(&self.baseline);
        let mut document_bound = baseline_bound;
        for step in &mut self.undo {
            validate_step(&mut document, &mut document_bound, step, bounds, input).map_err(
                |error| match error {
                    Error::Batch(error) => Error::InvalidUndo(error),
                    other => other,
                },
            )?;
        }
        if document != *self.document {
            return Err(Error::DocumentMismatch);
        }
        document_bound = DocumentBound::measure(&document, bounds)?;
        self.document = Arc::new(document);
        if !self.redo.is_empty() {
            budget
                .check_known(document_bound.live)
                .map_err(|_| Error::Capacity)?;
            let mut redone = super::capacity::copy_document(&self.document);
            let mut redone_bound = document_bound;
            for step in self.redo.iter_mut().rev() {
                validate_step(&mut redone, &mut redone_bound, step, bounds, input).map_err(
                    |error| match error {
                        Error::Batch(e) => Error::InvalidRedo(e),
                        other => other,
                    },
                )?;
            }
            DocumentBound::measure(&redone, bounds)?;
        }
        Ok(ValidatedSnapshot {
            record: self,
            bounds,
            undo_charge,
            redo_charge,
            baseline_bound,
            document_bound,
        })
    }
}
fn validate_step(
    document: &mut Document,
    size: &mut DocumentBound,
    step: &mut Step,
    bounds: RoleBounds,
    input: u64,
) -> Result<(), Error> {
    let before = *size;
    let mut summary = super::StepCharge {
        operations: step.charge.operations,
        key_bytes: step.charge.key_bytes,
        ..Default::default()
    };
    for command in &step.commands {
        let mut budget =
            ReplayBudget::workspace(bounds.replay_workspace).map_err(|_| Error::Capacity)?;
        budget.reserve_input(input).map_err(|_| Error::Capacity)?;
        let effects = super::apply(document, size, &command.operations, bounds, &mut budget)?;
        summary = super::replay_charge(summary, &budget, effects.len(), bounds, input)?;
    }
    summary.before = before;
    summary.after = *size;
    summary.validated = true;
    step.charge = summary;
    Ok(())
}
impl History {
    #[must_use]
    pub fn record(&self) -> Record {
        Record {
            baseline: Arc::clone(&self.baseline),
            document: Arc::clone(&self.document),
            undo: self.undo.clone(),
            redo: self.redo.clone(),
            position: self.position,
            epoch: self.epoch,
            policy: self.policy,
        }
    }
    pub fn snapshot_identity(&self) -> Result<[u8; 32], Error> {
        super::digest(self)
    }
    pub fn install_snapshot(&mut self, snapshot: ValidatedSnapshot) -> Result<CommandKey, Error> {
        // An opt-in activity recorder belongs to this live endpoint, rather
        // than the installed record. Its existing prefix must still fit.
        if self
            .recorder_bytes
            .checked_add(snapshot.undo_charge.decoded)
            .and_then(|bytes| bytes.checked_add(snapshot.redo_charge.decoded))
            .ok_or(Error::Capacity)?
            > snapshot.bounds.decoded_history_bytes
        {
            return Err(Error::Capacity);
        }
        let epoch = self
            .epoch
            .max(snapshot.record.epoch)
            .checked_add(1)
            .ok_or(Error::CounterExhausted)?;
        let cache = Cache::new(
            snapshot.bounds.checkpoint_documents,
            snapshot.bounds.decoded_history_bytes,
        )?;
        let Record {
            baseline,
            document,
            undo,
            redo,
            position,
            policy,
            ..
        } = snapshot.record;
        self.baseline = baseline;
        self.baseline_bound = snapshot.baseline_bound;
        self.document = document;
        self.document_bound = snapshot.document_bound;
        self.undo = undo;
        self.redo = redo;
        self.position = position;
        self.epoch = epoch;
        self.policy = policy;
        self.bounds = snapshot.bounds;
        self.undo_charge = snapshot.undo_charge;
        self.redo_charge = snapshot.redo_charge;
        self.cache = cache;
        Ok(self.position())
    }
}

impl Serialize for History {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut record = serializer.serialize_struct("Record", 7)?;
        record.serialize_field("baseline", &self.baseline)?;
        record.serialize_field("document", &self.document)?;
        record.serialize_field("undo", &self.undo)?;
        record.serialize_field("redo", &self.redo)?;
        record.serialize_field("position", &self.position)?;
        record.serialize_field("epoch", &self.epoch)?;
        record.serialize_field("policy", &self.policy)?;
        record.end()
    }
}
