use super::Error;
use crate::bundle_records::{ObjectRef, RecordKind};
use kinewright_core::{
    ClipId, TrackId,
    undo::{CommandKey, Effects},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fate {
    Applied,
    Undone,
    Redone,
    Folded,
    Discarded,
    /// An installed sibling capture has no evidence for this absent command.
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAnswer {
    Kept,
    Missing,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RemovedTarget {
    Clip(ClipId),
    Track(TrackId),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub reference: crate::bundle_records::ObjectRef,
    pub targets: Vec<RemovedTarget>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalEntry {
    pub key: CommandKey,
    pub payload_digest: [u8; 32],
    pub approval: Approval,
    pub fate: Fate,
    /// The last installation which changed this entry's fate. No allocation.
    pub install_epoch: Option<u64>,
}
impl ApprovalEntry {
    #[must_use]
    pub const fn answer(&self) -> ApprovalAnswer {
        match self.fate {
            Fate::Applied | Fate::Redone | Fate::Folded => ApprovalAnswer::Kept,
            Fate::Discarded | Fate::Undone => ApprovalAnswer::Missing,
            Fate::Uncertain => ApprovalAnswer::Uncertain,
        }
    }
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApprovalLedger {
    pub(super) commands: BTreeMap<CommandApprovalKey, ApprovalEntry>,
    pub(super) transitions: Vec<super::transition::TransitionApproval>,
    used: std::collections::BTreeSet<ReferenceKey>,
    by_step: BTreeMap<CommandKey, Vec<usize>>,
}
// An exact fixed-size routing key avoids duplicating consent strings in the
// admission index. It preserves the ObjectRef's spelling, length and kind.
pub(super) type ReferenceKey = ([u8; 64], u64, RecordKind);
pub(super) type CommandApprovalKey = (CommandKey, [u8; 32], ReferenceKey);
const LOW_REFERENCE: ReferenceKey = ([0; 64], 0, RecordKind::Document);
const HIGH_REFERENCE: ReferenceKey = ([u8::MAX; 64], u64::MAX, RecordKind::Document);
fn key_range(key: CommandKey) -> std::ops::RangeInclusive<CommandApprovalKey> {
    (key, [0; 32], LOW_REFERENCE)..=(key, [u8::MAX; 32], HIGH_REFERENCE)
}

pub(super) fn reference_key(reference: &ObjectRef) -> Option<ReferenceKey> {
    Some((
        reference.sha256.as_bytes().try_into().ok()?,
        reference.byte_length,
        reference.record_kind,
    ))
}
impl Serialize for ApprovalLedger {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct Commands<'a>(&'a BTreeMap<CommandApprovalKey, ApprovalEntry>);
        impl Serialize for Commands<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_seq(self.0.values())
            }
        }
        #[derive(Serialize)]
        struct Wire<'a> {
            commands: Commands<'a>,
            transitions: &'a [super::transition::TransitionApproval],
        }
        Wire {
            commands: Commands(&self.commands),
            transitions: &self.transitions,
        }
        .serialize(s)
    }
}
impl<'de> Deserialize<'de> for ApprovalLedger {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            commands: Vec<ApprovalEntry>,
            transitions: Vec<super::transition::TransitionApproval>,
        }
        let Wire {
            commands,
            transitions,
        } = Wire::deserialize(d)?;
        if commands.len() + transitions.len() > crate::orchestration_records::ITEM_LIMIT {
            return Err(serde::de::Error::custom("approval count"));
        }
        let mut result = BTreeMap::new();
        for entry in commands {
            let reference = reference_key(&entry.approval.reference)
                .ok_or_else(|| serde::de::Error::custom("invalid approval reference"))?;
            if result
                .insert((entry.key, entry.payload_digest, reference), entry)
                .is_some()
            {
                return Err(serde::de::Error::custom("duplicate approval command"));
            }
        }
        let mut ledger = Self {
            commands: result,
            transitions,
            ..Self::default()
        };
        ledger.rebuild_indices();
        if !ledger.references_unique() {
            return Err(serde::de::Error::custom("duplicate approval reference"));
        }
        Ok(ledger)
    }
}
impl ApprovalLedger {
    pub(super) fn rebuild_indices(&mut self) {
        self.used.clear();
        self.by_step.clear();
        for entry in self.commands.values() {
            if let Some(key) = reference_key(&entry.approval.reference) {
                self.used.insert(key);
            }
        }
        for index in 0..self.transitions.len() {
            self.index_transition(index);
        }
    }
    pub(super) fn index_transition(&mut self, index: usize) {
        let entry = &self.transitions[index];
        if let Some(key) = reference_key(&entry.id) {
            self.used.insert(key);
        }
        self.by_step
            .entry(entry.target.first_key)
            .or_default()
            .push(index);
    }
    pub(super) fn reference_used(&self, id: &ObjectRef) -> bool {
        reference_key(id).is_some_and(|key| self.used.contains(&key))
    }
    pub(super) fn for_step(
        &self,
        key: CommandKey,
    ) -> impl Iterator<Item = &super::transition::TransitionApproval> {
        self.by_step
            .get(&key)
            .into_iter()
            .flatten()
            .map(|index| &self.transitions[*index])
            .inspect(|_| super::owner::ledger_entries(1))
    }
    pub(super) fn references_unique(&self) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        self.entries()
            .map(|entry| &entry.approval.reference)
            .chain(self.transition_entries().map(|entry| &entry.id))
            .all(|id| seen.insert(id))
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len() + self.transitions.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn transition_entries(
        &self,
    ) -> impl Iterator<Item = &super::transition::TransitionApproval> {
        self.transitions
            .iter()
            .inspect(|_| super::owner::ledger_entries(1))
    }
    pub fn entries(&self) -> impl Iterator<Item = &ApprovalEntry> {
        self.commands
            .values()
            .inspect(|_| super::owner::ledger_entries(1))
    }
    #[must_use]
    /// Command-level convenience. Different payloads sharing an absent key
    /// remain ambiguous; several approvals of the same payload share its fate.
    pub fn get(&self, key: CommandKey) -> Option<&ApprovalEntry> {
        let mut entries = self.commands.range(key_range(key)).map(|(_, entry)| entry);
        let first = entries.next()?;
        entries
            .all(|entry| entry.payload_digest == first.payload_digest)
            .then_some(first)
    }
    #[must_use]
    pub fn get_exact(&self, key: CommandKey, digest: [u8; 32]) -> Option<&ApprovalEntry> {
        self.commands
            .range((key, digest, LOW_REFERENCE)..=(key, digest, HIGH_REFERENCE))
            .next()
            .map(|(_, entry)| entry)
    }
    pub(super) fn for_command_mut(
        &mut self,
        key: CommandKey,
        digest: [u8; 32],
    ) -> impl Iterator<Item = &mut ApprovalEntry> {
        self.commands
            .range_mut((key, digest, LOW_REFERENCE)..=(key, digest, HIGH_REFERENCE))
            .map(|(_, entry)| entry)
    }
    #[must_use]
    pub fn get_approval(&self, reference: &ObjectRef) -> Option<&ApprovalEntry> {
        self.commands
            .values()
            .find(|entry| &entry.approval.reference == reference)
    }
    pub(super) fn command_fates_consistent(&self) -> bool {
        let mut previous: Option<&ApprovalEntry> = None;
        for entry in self.commands.values() {
            if previous.is_some_and(|old| {
                old.key == entry.key
                    && old.payload_digest == entry.payload_digest
                    && old.fate != entry.fate
            }) {
                return false;
            }
            previous = Some(entry);
        }
        true
    }
    fn set_fate(&mut self, key: CommandKey, fate: Fate) {
        for (_, entry) in self.commands.range_mut(key_range(key)) {
            entry.fate = fate;
        }
    }
    pub(super) fn admit(&self, approval: &Approval, key: CommandKey) -> Result<(), Error> {
        approval
            .reference
            .validate()
            .map_err(|_| Error::InvalidLedger)?;
        if self.len() >= crate::orchestration_records::ITEM_LIMIT {
            return Err(Error::Capacity("approval count"));
        }
        if self.commands.range(key_range(key)).next().is_some()
            || approval.targets.is_empty()
            || self.reference_used(&approval.reference)
        {
            return Err(Error::InvalidLedger);
        }
        Ok(())
    }
    pub(super) fn insert(&mut self, key: CommandKey, digest: [u8; 32], approval: Approval) {
        self.used
            .insert(reference_key(&approval.reference).expect("admitted consent reference"));
        self.commands.insert(
            (
                key,
                digest,
                reference_key(&approval.reference).expect("admitted consent reference"),
            ),
            ApprovalEntry {
                key,
                payload_digest: digest,
                approval,
                fate: Fate::Applied,
                install_epoch: None,
            },
        );
    }
    pub(super) fn observe(&mut self, event: &Effects) {
        // Retained-key conflicts are rejected at install/open. All approvals
        // of that exact command move together; absent sibling digests cannot
        // be named by a Core movement.
        for key in &event.to_undo {
            self.set_fate(
                *key,
                if *key == event.accepted {
                    Fate::Applied
                } else {
                    Fate::Redone
                },
            );
        }
        for key in &event.to_redo {
            self.set_fate(*key, Fate::Undone);
        }
        for command in &event.folded {
            self.set_fate(command.key, Fate::Folded);
        }
        for key in &event.discarded {
            self.set_fate(*key, Fate::Discarded);
        }
    }
}

/// One fixed prefix stamp per approved Do, captured only when its step folds.
/// It can also settle a later transition approval without retaining commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FoldedApproval {
    pub payload_digest: [u8; 32],
    pub prefix: kinewright_core::undo::StepEvidence,
}
pub(super) type FoldedApprovals = BTreeMap<CommandKey, FoldedApproval>;
impl FoldedApproval {
    pub fn valid_for(&self, key: CommandKey, digest: [u8; 32]) -> bool {
        self.payload_digest == digest
            && self.prefix.commands > 0
            && self.prefix.first_key <= key
            && self
                .prefix
                .first_key
                .position
                .checked_add(self.prefix.commands - 1)
                .is_some_and(|last| last <= key.position)
            && (self.prefix.commands != 1
                || kinewright_core::undo::StepEvidence::from_commands(std::iter::once((
                    key, digest,
                ))) == Some(self.prefix))
    }
}
// JSON objects cannot key a map by CommandKey. The baseline owns an ordered
// sequence of exact key/digest facts, distinct from the ledger's current fates.
pub(super) mod folded_wire {
    use super::{CommandKey, Deserialize, FoldedApproval, FoldedApprovals};
    pub fn serialize<S: serde::Serializer>(map: &FoldedApprovals, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(map.iter())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<FoldedApprovals, D::Error> {
        let values = Vec::<(CommandKey, FoldedApproval)>::deserialize(d)?;
        if values.len() > crate::orchestration_records::ITEM_LIMIT
            || values.windows(2).any(|w| w[0].0 >= w[1].0)
        {
            return Err(serde::de::Error::custom("baseline approval facts"));
        }
        Ok(values.into_iter().collect())
    }
}

pub(super) fn targets_removed(
    approval: &Approval,
    operations: &[kinewright_core::Operation],
    deleted: impl Fn(ClipId) -> bool,
) -> bool {
    !approval.targets.is_empty()
        && approval.targets.iter().all(|target| match target {
            RemovedTarget::Clip(id) => deleted(*id),
            RemovedTarget::Track(id) => operations.iter().any(
                |op| matches!(op,kinewright_core::Operation::RemoveTrack {track} if track==id),
            ),
        })
}
