//! `NeverFold` admission also reserves the eventual one-batch destination replay.
//! Per-step observations are additive; the largest recorded kernel scratch
//! excludes those observations, so forwarding a batch does not charge them twice.
use super::{Error, Step, StepCharge};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MergeGuard {
    pub input: u64,
    pub undo_effects: u64,
    pub redo_effects: u64,
    pub peak: u64,
}
impl MergeGuard {
    pub fn from_steps(
        input: u64,
        undo: &std::collections::VecDeque<Step>,
        redo: &[Step],
        limit: u64,
    ) -> Result<Self, Error> {
        let mut guard = Self {
            input,
            undo_effects: 0,
            redo_effects: 0,
            peak: 0,
        };
        for step in undo {
            guard.undo_effects = guard
                .undo_effects
                .checked_add(step.charge.effects_bytes)
                .ok_or(Error::Capacity)?;
            guard.peak = guard.peak.max(step.charge.merge_scratch);
        }
        for step in redo {
            guard.redo_effects = guard
                .redo_effects
                .checked_add(step.charge.effects_bytes)
                .ok_or(Error::Capacity)?;
            guard.peak = guard.peak.max(step.charge.merge_scratch);
        }
        guard.check(limit)?;
        Ok(guard)
    }
    fn check(self, limit: u64) -> Result<(), Error> {
        let peak = self
            .input
            .checked_add(self.input.div_ceil(10))
            .and_then(|n| n.checked_add(self.undo_effects))
            .and_then(|n| n.checked_add(self.redo_effects))
            .and_then(|n| n.checked_add(self.peak))
            .ok_or(Error::Capacity)?;
        if peak > limit {
            Err(Error::HistoryFull)
        } else {
            Ok(())
        }
    }
    pub fn after_edit(
        self,
        previous: StepCharge,
        next: StepCharge,
        limit: u64,
    ) -> Result<Self, Error> {
        let guard = Self {
            undo_effects: self
                .undo_effects
                .checked_sub(previous.effects_bytes)
                .and_then(|n| n.checked_add(next.effects_bytes))
                .ok_or(Error::Capacity)?,
            redo_effects: 0,
            peak: self.peak.max(next.merge_scratch),
            ..self
        };
        guard.check(limit)?;
        Ok(guard)
    }
}
