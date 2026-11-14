use super::Error;
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
    pub(super) commands: BTreeMap<CommandKey, ApprovalEntry>,
    pub(super) transitions: Vec<super::transition::TransitionApproval>,
}
impl Serialize for ApprovalLedger {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct Commands<'a>(&'a BTreeMap<CommandKey, ApprovalEntry>);
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
            if result.insert(entry.key, entry).is_some() {
                return Err(serde::de::Error::custom("duplicate approval command"));
            }
        }
        let ledger = Self {
            commands: result,
            transitions,
        };
        if !ledger.references_unique() {
            return Err(serde::de::Error::custom("duplicate approval reference"));
        }
        Ok(ledger)
    }
}
impl ApprovalLedger {
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
        self.transitions.iter()
    }
    pub fn entries(&self) -> impl Iterator<Item = &ApprovalEntry> {
        self.commands.values()
    }
    #[must_use]
    pub fn get(&self, key: CommandKey) -> Option<&ApprovalEntry> {
        self.commands.get(&key)
    }
    pub(super) fn admit(&self, approval: &Approval, key: CommandKey) -> Result<(), Error> {
        approval
            .reference
            .validate()
            .map_err(|_| Error::InvalidLedger)?;
        if self.len() >= crate::orchestration_records::ITEM_LIMIT {
            return Err(Error::Capacity("approval count"));
        }
        if self.commands.contains_key(&key)
            || approval.targets.is_empty()
            || self
                .entries()
                .any(|entry| entry.approval.reference == approval.reference)
            || self
                .transitions
                .iter()
                .any(|entry| entry.id == approval.reference)
        {
            return Err(Error::InvalidLedger);
        }
        Ok(())
    }
    pub(super) fn insert(&mut self, key: CommandKey, digest: [u8; 32], approval: Approval) {
        self.commands.insert(
            key,
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
        for key in &event.to_undo {
            if let Some(entry) = self.commands.get_mut(key) {
                entry.fate = if *key == event.accepted {
                    Fate::Applied
                } else {
                    Fate::Redone
                };
            }
        }
        for key in &event.to_redo {
            if let Some(entry) = self.commands.get_mut(key) {
                entry.fate = Fate::Undone;
            }
        }
        for command in &event.folded {
            if let Some(entry) = self.commands.get_mut(&command.key) {
                entry.fate = Fate::Folded;
            }
        }
        for key in &event.discarded {
            if let Some(entry) = self.commands.get_mut(key) {
                entry.fate = Fate::Discarded;
            }
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
