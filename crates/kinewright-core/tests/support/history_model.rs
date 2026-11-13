#![allow(
    dead_code,
    reason = "each integration binary exercises a subset of the shared reference model"
)]
use std::collections::BTreeMap;

use kinewright_core::{BatchError, Command, Document, OpError, Operation};

type CommandId = (u64, u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCommand {
    pub id: CommandId,
    pub position: u64,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub commands: Vec<StepCommand>,
    pub coalesce_key: Option<String>,
}
impl Step {
    pub fn operations(&self) -> impl Iterator<Item = &Operation> {
        self.commands.iter().flat_map(|command| &command.operations)
    }
    fn cost(&self) -> usize {
        self.commands
            .iter()
            .map(|command| Model::step_cost(&command.operations))
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Fold,
    NeverFold,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    Undo,
    Redo,
    Folded,
    Discarded,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Operation(OpError),
    Batch(BatchError),
    StepTooLarge,
    HistoryFull,
    DocumentTooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub baseline: Document,
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
    pub budget: usize,
    pub document_limit: usize,
    pub step_cap: usize,
    pub policy: Policy,
    pub position: u64,
    pub epoch: u64,
    pub fates: BTreeMap<CommandId, Fate>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallRefusal {
    UndoReplay(OpError),
    DocumentMismatch,
    RedoReplay(OpError),
    EpochExhausted,
}

pub struct Snapshot {
    pub state: Model,
    pub document: Document,
}

impl Model {
    pub fn new(baseline: Document, budget: usize, policy: Policy) -> Self {
        Self {
            baseline,
            undo: Vec::new(),
            redo: Vec::new(),
            budget,
            document_limit: usize::MAX,
            step_cap: usize::MAX,
            policy,
            position: 0,
            epoch: 0,
            fates: BTreeMap::new(),
        }
    }

    pub fn at_depth(&self, depth: usize) -> Document {
        // This copy is independent replay state, never a cached checkpoint.
        let mut document = self.baseline.clone();
        for operation in self.undo[..depth].iter().flat_map(Step::operations) {
            operation
                .apply(&mut document)
                .expect("MODEL_REPLAY_ACCEPTED_OPERATION");
        }
        document
    }
    pub fn document(&self) -> Document {
        self.at_depth(self.undo.len())
    }
    pub fn applied_operations(&self) -> Vec<Operation> {
        self.undo
            .iter()
            .flat_map(Step::operations)
            .cloned()
            .collect()
    }
    pub fn step_cost(operations: &[Operation]) -> usize {
        operations
            .iter()
            .map(|operation| serde_json::to_vec(operation).unwrap().len())
            .sum()
    }
    pub fn cost(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(Step::cost).sum()
    }

    pub fn snapshot(&self) -> Snapshot {
        // Snapshot ownership is independent of subsequent execution and folding.
        Snapshot {
            state: self.clone(),
            document: self.document(),
        }
    }
    pub fn install_snapshot(&mut self, snapshot: Snapshot) -> Result<(), InstallRefusal> {
        let mut replayed = snapshot.state.baseline.clone();
        for operation in snapshot.state.undo.iter().flat_map(Step::operations) {
            operation
                .apply(&mut replayed)
                .map_err(InstallRefusal::UndoReplay)?;
        }
        if replayed != snapshot.document {
            return Err(InstallRefusal::DocumentMismatch);
        }
        let mut redone = snapshot.document;
        for operation in snapshot.state.redo.iter().rev().flat_map(Step::operations) {
            operation
                .apply(&mut redone)
                .map_err(InstallRefusal::RedoReplay)?;
        }
        let epoch = self
            .epoch
            .max(snapshot.state.epoch)
            .checked_add(1)
            .ok_or(InstallRefusal::EpochExhausted)?;
        *self = snapshot.state;
        self.epoch = epoch;
        Ok(())
    }
    pub fn close_gesture(&mut self) {
        if self
            .undo
            .last()
            .is_some_and(|step| step.coalesce_key.is_some())
        {
            // Today's recovery Undo/Redo also closes the temporarily exposed entry.
            for step in self.undo.iter_mut().rev().take(2) {
                step.coalesce_key = None;
            }
        }
        self.position += 1;
    }
    fn mark(&mut self, step: &Step, fate: Fate) {
        for command in &step.commands {
            self.fates.insert(command.id, fate);
        }
    }
    pub fn execute(&mut self, command: &Command) -> Result<usize, Refusal> {
        let folded = match command {
            Command::Undo => {
                if let Some(mut step) = self.undo.pop() {
                    self.mark(&step, Fate::Redo);
                    step.coalesce_key = None;
                    self.redo.push(step);
                    if let Some(exposed) = self.undo.last_mut() {
                        exposed.coalesce_key = None;
                    }
                }
                0
            }
            Command::Redo => {
                if let Some(mut step) = self.redo.pop() {
                    let mut document = self.document();
                    for operation in step.operations() {
                        operation
                            .apply(&mut document)
                            .expect("MODEL_REDO_ACCEPTED_OPERATION");
                    }
                    self.mark(&step, Fate::Undo);
                    step.coalesce_key = None;
                    self.undo.push(step);
                }
                0
            }
            Command::Do(operation) => self.do_(std::slice::from_ref(operation), None, true)?,
            Command::DoBatch(operations) => self.do_(operations, None, false)?,
            Command::DoBatchCoalesced {
                operations,
                coalesce_key,
            } => self.do_(operations, Some(coalesce_key), false)?,
            _ => unreachable!("history model command"),
        };
        self.position += 1;
        Ok(folded)
    }
    fn do_(
        &mut self,
        operations: &[Operation],
        key: Option<&str>,
        single: bool,
    ) -> Result<usize, Refusal> {
        if operations.is_empty() {
            return Err(Refusal::Batch(BatchError::Empty));
        }
        let extends = key.is_some()
            && self
                .undo
                .last()
                .is_some_and(|step| step.coalesce_key.as_deref() == key);
        let added_cost = Self::step_cost(operations);
        let top_cost = if extends {
            self.undo.last().unwrap().cost()
        } else {
            0
        };
        if top_cost + added_cost > self.budget {
            return Err(Refusal::StepTooLarge);
        }
        let undo_cost: usize = self.undo.iter().map(Step::cost).sum();
        if self.policy == Policy::NeverFold
            && (undo_cost + added_cost > self.budget
                || self.undo.len() + usize::from(!extends) > self.step_cap)
        {
            return Err(Refusal::HistoryFull);
        }
        let mut document = self.document();
        for (index, operation) in operations.iter().enumerate() {
            operation.apply(&mut document).map_err(|error| {
                if single {
                    Refusal::Operation(error)
                } else {
                    Refusal::Batch(BatchError::OperationFailed {
                        op_number: index + 1,
                        error,
                    })
                }
            })?;
            if serde_json::to_vec(&document).unwrap().len() > self.document_limit {
                return Err(Refusal::DocumentTooLarge);
            }
        }
        let command = StepCommand {
            id: (self.epoch, self.position),
            position: self.position,
            operations: operations.into(),
        };
        self.fates.insert(command.id, Fate::Undo);
        if extends {
            self.undo.last_mut().unwrap().commands.push(command);
        } else {
            self.undo.push(Step {
                commands: vec![command],
                coalesce_key: key.map(String::from),
            });
        }
        for step in std::mem::take(&mut self.redo) {
            self.mark(&step, Fate::Discarded);
        }
        let mut folded = 0;
        while self.cost() > self.budget || self.undo.len() > self.step_cap {
            let before = self.document();
            let step = self.undo.remove(0);
            for operation in step.operations() {
                operation
                    .apply(&mut self.baseline)
                    .expect("MODEL_FOLD_ACCEPTED_OPERATION");
            }
            self.mark(&step, Fate::Folded);
            assert_eq!(self.document(), before, "EACH_FOLD_PRESERVES_DOCUMENT");
            folded += 1;
        }
        Ok(folded)
    }
}
