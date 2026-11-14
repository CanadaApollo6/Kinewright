//! Consent for moving an existing step. Entries contain evidence, never fates.
use super::{ApprovalAnswer, Error, OwnerHistory};
use crate::bundle_records::ObjectRef;
use kinewright_core::{
    Command,
    undo::{CommandKey, Step, StepEvidence},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Undo,
    Redo,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionApproval {
    pub id: ObjectRef,
    pub direction: Direction,
    pub target: StepEvidence,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uncertainty {
    ShapeMismatch,
    MissingFoldProvenance,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionFate {
    Holds,
    Reversed,
    Discarded,
    Uncertain(Uncertainty),
}
impl TransitionFate {
    #[must_use]
    pub const fn answer(self) -> ApprovalAnswer {
        match self {
            Self::Holds => ApprovalAnswer::Kept,
            Self::Reversed | Self::Discarded => ApprovalAnswer::Missing,
            Self::Uncertain(_) => ApprovalAnswer::Uncertain,
        }
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TransitionCounts {
    pub holds: usize,
    pub reversed: usize,
    pub discarded: usize,
    pub uncertain: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum TerminalPresence {
    Folded,
    Discarded,
    ShapeMismatch,
}
pub(super) type TerminalFacts = BTreeMap<StepEvidence, TerminalPresence>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Presence {
    Applied,
    Undone,
    Folded,
    Absent,
    Uncertain(Uncertainty),
}

pub(super) fn presence(
    target: StepEvidence,
    undo: &VecDeque<Step>,
    redo: &[Step],
    origins: &super::owner::Origins,
    facts: &TerminalFacts,
    folded_commands: &super::ledger::FoldedApprovals,
    frontier: Option<CommandKey>,
) -> Result<Presence, Error> {
    if let Some(origin) = origins.get(&target.first_key) {
        if origin.origin.payload_digest != target.first_digest {
            return Err(Error::CommandConflict);
        }
        let first = origin.step_first;
        let index = undo.partition_point(|step| step.commands()[0].id < first);
        let found = undo
            .get(index)
            .filter(|step| step.commands()[0].id == first)
            .map(|step| (true, step))
            .or_else(|| {
                let index = redo.partition_point(|step| step.commands()[0].id > first);
                redo.get(index)
                    .filter(|step| step.commands()[0].id == first)
                    .map(|step| (false, step))
            });
        let (applied, step) = found.ok_or(Error::InvalidOrigin)?;
        if step.commands()[0].id != target.first_key
            || usize::try_from(target.commands)
                .ok()
                .and_then(|n| step.prefix_evidence(n))
                != Some(target)
        {
            return Ok(Presence::Uncertain(Uncertainty::ShapeMismatch));
        }
        return Ok(if applied {
            Presence::Applied
        } else {
            Presence::Undone
        });
    }
    Ok(match facts.get(&target) {
        Some(TerminalPresence::Folded) => Presence::Folded,
        Some(TerminalPresence::ShapeMismatch) => Presence::Uncertain(Uncertainty::ShapeMismatch),
        None if folded_match(target, folded_commands) => Presence::Folded,
        None if folded_commands
            .get(&target.first_key)
            .is_some_and(|proof| proof.prefix.first_key != target.first_key) =>
        {
            Presence::Uncertain(Uncertainty::ShapeMismatch)
        }
        None if frontier.is_some_and(|frontier| target.first_key < frontier) => {
            Presence::Uncertain(Uncertainty::MissingFoldProvenance)
        }
        Some(TerminalPresence::Discarded) | None => Presence::Absent,
    })
}
fn folded_match(target: StepEvidence, commands: &super::ledger::FoldedApprovals) -> bool {
    commands.values().any(|proof| proof.prefix == target)
}
pub(super) fn derive(direction: Direction, presence: Presence) -> TransitionFate {
    match (direction, presence) {
        (_, Presence::Uncertain(reason)) => TransitionFate::Uncertain(reason),
        (Direction::Undo, Presence::Applied | Presence::Folded)
        | (Direction::Redo, Presence::Undone) => TransitionFate::Reversed,
        (Direction::Undo, Presence::Undone | Presence::Absent)
        | (Direction::Redo, Presence::Applied | Presence::Folded) => TransitionFate::Holds,
        (Direction::Redo, Presence::Absent) => TransitionFate::Discarded,
    }
}
impl OwnerHistory {
    #[must_use]
    pub fn transition_target(&self, direction: Direction) -> Option<StepEvidence> {
        match direction {
            Direction::Undo => self.core.undo().back(),
            Direction::Redo => self.core.redo().last(),
        }
        .and_then(Step::evidence)
    }
    pub fn approve_transition(
        &mut self,
        id: ObjectRef,
        direction: Direction,
        expected: StepEvidence,
    ) -> Result<kinewright_core::undo::Effects, Error> {
        let _workspace = self.reservation.workspace()?;
        let actual = self
            .transition_target(direction)
            .ok_or(Error::NoTargetStep)?;
        if actual != expected {
            return Err(Error::StaleTransition);
        }
        id.validate().map_err(|_| Error::InvalidLedger)?;
        if self.ledger.len() >= crate::orchestration_records::ITEM_LIMIT {
            return Err(Error::Capacity("approval count"));
        }
        if self.ledger.transitions.iter().any(|entry| entry.id == id)
            || self
                .ledger
                .entries()
                .any(|entry| entry.approval.reference == id)
        {
            return Err(Error::InvalidLedger);
        }
        let bytes = transition_charge(&id)?;
        let next = self
            .owner_bytes
            .checked_add(bytes)
            .filter(|bytes| *bytes <= self.reservation.role().envelope().owners)
            .ok_or(Error::Capacity("owner metadata"))?;
        let decode = self
            .decode_bound
            .checked_add(TRANSITION_DECODE)
            .filter(|bytes| *bytes <= self.reservation.role().envelope().workspace)
            .ok_or(Error::Capacity("owner decode"))?;
        self.ledger
            .transitions
            .try_reserve(1)
            .map_err(|_| Error::Capacity("owner metadata"))?;
        let effects = self.core.execute(match direction {
            Direction::Undo => Command::Undo,
            Direction::Redo => Command::Redo,
        })?;
        self.ledger.observe(&effects, &[]);
        self.decode_bound = decode;
        self.owner_bytes = next;
        self.ledger.transitions.push(TransitionApproval {
            id,
            direction,
            target: expected,
        });
        Ok(effects)
    }
    pub fn transition_fate(&self, id: &ObjectRef) -> Result<Option<TransitionFate>, Error> {
        let Some(entry) = self.ledger.transitions.iter().find(|entry| &entry.id == id) else {
            return Ok(None);
        };
        Ok(Some(derive(
            entry.direction,
            presence(
                entry.target,
                self.core.undo(),
                self.core.redo(),
                &self.origins,
                &self.terminal_facts,
                &self.folded_approvals,
                self.fold_frontier,
            )?,
        )))
    }
    pub fn transition_counts(&self) -> Result<TransitionCounts, Error> {
        let mut counts = TransitionCounts::default();
        for entry in &self.ledger.transitions {
            match self
                .transition_fate(&entry.id)?
                .ok_or(Error::InvalidLedger)?
            {
                TransitionFate::Holds => counts.holds += 1,
                TransitionFate::Reversed => counts.reversed += 1,
                TransitionFate::Discarded => counts.discarded += 1,
                TransitionFate::Uncertain(_) => counts.uncertain += 1,
            }
        }
        Ok(counts)
    }
}
pub(super) const TRANSITION_DECODE: u64 = 16_384;
pub(super) fn transition_charge(id: &ObjectRef) -> Result<u64, Error> {
    1024_u64
        .checked_add(id.sha256.capacity() as u64)
        .ok_or(Error::Capacity("owner metadata"))
}
pub(super) fn prefix_matches(target: StepEvidence, origins: &super::owner::Origins) -> bool {
    let Ok(index) = origins
        .0
        .binary_search_by_key(&target.first_key.position, |row| row.origin.key.position)
    else {
        return false;
    };
    let Ok(count) = usize::try_from(target.commands) else {
        return false;
    };
    let mut rows = origins.0.iter().skip(index).take(count);
    let Some(first) = rows.next() else {
        return false;
    };
    if first.step_first != target.first_key {
        return false;
    }
    let rows = std::iter::once(first).chain(rows);
    let mut valid = true;
    let evidence = StepEvidence::from_commands(rows.map(|row| {
        valid &= row.step_first == target.first_key;
        (row.origin.key, row.origin.payload_digest)
    }));
    valid && evidence == Some(target)
}
pub(super) mod terminal_wire {
    use super::{Deserialize, StepEvidence, TerminalFacts, TerminalPresence};
    pub fn serialize<S: serde::Serializer>(facts: &TerminalFacts, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(facts.iter())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<TerminalFacts, D::Error> {
        let facts = Vec::<(StepEvidence, TerminalPresence)>::deserialize(d)?;
        if facts.len() > crate::orchestration_records::ITEM_LIMIT
            || facts.windows(2).any(|w| w[0].0 >= w[1].0)
        {
            return Err(serde::de::Error::custom("terminal step facts"));
        }
        Ok(facts.into_iter().collect())
    }
}

pub(super) fn validate_frontier(
    frontier: Option<CommandKey>,
    epoch: u64,
    position: u64,
    undo: &VecDeque<Step>,
    redo: &[Step],
) -> Result<(), Error> {
    if let Some(frontier) = frontier {
        let next = CommandKey { epoch, position };
        let first = undo
            .front()
            .or_else(|| redo.last())
            .and_then(|step| step.commands().first())
            .map(|command| command.id);
        if frontier.position == 0
            || frontier > next
            || frontier.position > position
            || first.is_some_and(|first| frontier > first || frontier.position > first.position)
        {
            return Err(Error::InvalidFrontier);
        }
    }
    Ok(())
}
pub(super) fn validate_entries(
    ledger: &super::ApprovalLedger,
    origins: &super::owner::Origins,
    undo: &VecDeque<Step>,
    redo: &[Step],
    facts: &TerminalFacts,
    folded_commands: &super::ledger::FoldedApprovals,
    frontier: Option<CommandKey>,
) -> Result<(), Error> {
    let mut known: BTreeMap<_, _> = ledger
        .entries()
        .map(|entry| (entry.key, entry.payload_digest))
        .collect();
    for entry in ledger.transition_entries() {
        if entry.target.commands == 0 {
            return Err(Error::InvalidLedger);
        }
        if let Some(old) = known.insert(entry.target.first_key, entry.target.first_digest)
            && old != entry.target.first_digest
        {
            return Err(Error::CommandConflict);
        }
        // Reading derives the answer; it never accepts a stored fate flag.
        let _ = presence(
            entry.target,
            undo,
            redo,
            origins,
            facts,
            folded_commands,
            frontier,
        )?;
    }
    for (target, whereabouts) in facts {
        if *whereabouts != TerminalPresence::Folded && folded_match(*target, folded_commands) {
            return Err(Error::InvalidLedger);
        }
        if !ledger
            .transition_entries()
            .any(|entry| entry.target == *target)
            || origins.get(&target.first_key).is_some()
        {
            return Err(Error::InvalidLedger);
        }
        if *whereabouts == TerminalPresence::Folded
            && !frontier.is_some_and(|frontier| {
                target.first_key < frontier
                    && target
                        .first_key
                        .position
                        .checked_add(target.commands)
                        .is_some_and(|end| end <= frontier.position)
            })
        {
            return Err(Error::InvalidFrontier);
        }
    }
    Ok(())
}
