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
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAnswer {
    Kept,
    Missing,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    pub recreated: bool,
}
impl ApprovalEntry {
    #[must_use]
    pub const fn answer(&self) -> ApprovalAnswer {
        match self.fate {
            Fate::Applied | Fate::Redone | Fate::Folded => ApprovalAnswer::Kept,
            Fate::Undone | Fate::Discarded if !self.recreated => ApprovalAnswer::Missing,
            Fate::Undone | Fate::Discarded | Fate::Uncertain => ApprovalAnswer::Uncertain,
        }
    }
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApprovalLedger(pub(super) BTreeMap<CommandKey, ApprovalEntry>);
impl Serialize for ApprovalLedger {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.values())
    }
}
impl<'de> Deserialize<'de> for ApprovalLedger {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = Vec::<ApprovalEntry>::deserialize(deserializer)?;
        if entries.len() > crate::orchestration_records::ITEM_LIMIT {
            return Err(serde::de::Error::custom("approval count"));
        }
        let mut result = BTreeMap::new();
        for entry in entries {
            if result.insert(entry.key, entry).is_some() {
                return Err(serde::de::Error::custom("duplicate approval command"));
            }
        }
        Ok(Self(result))
    }
}
impl ApprovalLedger {
    pub fn entries(&self) -> impl Iterator<Item = &ApprovalEntry> {
        self.0.values()
    }
    #[must_use]
    pub fn get(&self, key: CommandKey) -> Option<&ApprovalEntry> {
        self.0.get(&key)
    }
    pub(super) fn admit(&self, approval: &Approval, key: CommandKey) -> Result<(), Error> {
        if self.0.len() >= crate::orchestration_records::ITEM_LIMIT {
            return Err(Error::Capacity("approval count"));
        }
        if self.0.contains_key(&key) || approval.targets.is_empty() {
            return Err(Error::InvalidLedger);
        }
        Ok(())
    }
    pub(super) fn insert(&mut self, key: CommandKey, digest: [u8; 32], approval: Approval) {
        self.0.insert(
            key,
            ApprovalEntry {
                key,
                payload_digest: digest,
                approval,
                fate: Fate::Applied,
                recreated: false,
            },
        );
    }
    pub(super) fn observe(&mut self, event: &Effects, created_tracks: &[TrackId]) {
        // At most one bit per approval records any later recreation. It stays
        // true even if that later command is undone or discarded, like today's audit.
        for entry in self.0.values_mut() {
            if entry.key != event.accepted {
                entry.recreated |= entry.approval.targets.iter().any(|target| match target {
                    RemovedTarget::Clip(id) => event
                        .applied
                        .iter()
                        .filter(|command| command.key == event.accepted)
                        .any(|command| {
                            command.identities.iter().any(|effect| {
                                effect.created.contains(id)
                                    || effect.split_results.iter().any(|c| c.id == *id)
                            })
                        }),
                    RemovedTarget::Track(id) => created_tracks.contains(id),
                });
            }
        }
        for key in &event.to_undo {
            if let Some(entry) = self.0.get_mut(key) {
                entry.fate = if *key == event.accepted {
                    Fate::Applied
                } else {
                    Fate::Redone
                };
            }
        }
        for key in &event.to_redo {
            if let Some(entry) = self.0.get_mut(key) {
                entry.fate = Fate::Undone;
            }
        }
        for command in &event.folded {
            if let Some(entry) = self.0.get_mut(&command.key) {
                entry.fate = Fate::Folded;
            }
        }
        for key in &event.discarded {
            if let Some(entry) = self.0.get_mut(key) {
                entry.fate = Fate::Discarded;
            }
        }
    }
}
