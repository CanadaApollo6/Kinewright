use super::Error;
use crate::bundle_records::{BatchId, BranchId, OccurrenceId};
use kinewright_core::{Clip, ClipId, Document, OperationIdentityEffect};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreationOrigin {
    pub branch: BranchId,
    pub batch: BatchId,
}
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    pub occurrences: BTreeMap<ClipId, OccurrenceId>,
    pub split_ancestry: Vec<(OccurrenceId, OccurrenceId, Clip)>,
}
impl Serialize for Lineage {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            occurrences: &'a BTreeMap<ClipId, OccurrenceId>,
            split_ancestry: &'a [(OccurrenceId, OccurrenceId, Clip)],
        }
        super::owner::lineage_encodings(1);
        Wire {
            occurrences: &self.occurrences,
            split_ancestry: &self.split_ancestry,
        }
        .serialize(s)
    }
}
impl Clone for Lineage {
    fn clone(&self) -> Self {
        super::owner::seed_map_copies(1);
        Self {
            occurrences: self.occurrences.clone(),
            split_ancestry: self.split_ancestry.clone(),
        }
    }
}
impl Lineage {
    #[must_use]
    pub fn seed(document: &Document, origin: &CreationOrigin) -> Self {
        Self {
            occurrences: document
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .enumerate()
                .map(|(ordinal, clip)| (clip.id, occurrence(origin, ordinal as u64)))
                .collect(),
            split_ancestry: Vec::new(),
        }
    }
    pub(super) fn apply(
        &mut self,
        origin: &CreationOrigin,
        effects: &[OperationIdentityEffect],
    ) -> Result<(), Error> {
        self.apply_inner(origin, effects, true)
    }
    pub(super) fn apply_occurrences(
        &mut self,
        origin: &CreationOrigin,
        effects: &[OperationIdentityEffect],
    ) -> Result<(), Error> {
        self.apply_inner(origin, effects, false)
    }
    fn apply_inner(
        &mut self,
        origin: &CreationOrigin,
        effects: &[OperationIdentityEffect],
        ancestry: bool,
    ) -> Result<(), Error> {
        let mut ordinal = 0;
        for effect in effects {
            // Capture the parent before deleting/replacing its reusable clip ID.
            let parent = effect
                .split_parent
                .as_ref()
                .and_then(|clip| self.occurrences.get(&clip.id))
                .cloned();
            for id in &effect.deleted {
                self.occurrences.remove(id);
            }
            for id in &effect.created {
                if !effect.split_results.iter().any(|clip| clip.id == *id) {
                    self.occurrences.insert(*id, occurrence(origin, ordinal));
                    ordinal += 1;
                }
            }
            for clip in &effect.split_results {
                let child = occurrence(origin, ordinal);
                ordinal += 1;
                let parent = parent.as_ref().ok_or(Error::InvalidSeeds)?;
                // The baseline/query owns its exact ancestry independently of
                // the executor's short-lived identity observation.
                if ancestry {
                    self.split_ancestry
                        .push((child.clone(), parent.clone(), clip.clone()));
                }
                self.occurrences.insert(clip.id, child);
            }
        }
        Ok(())
    }
    pub(super) fn prune(&mut self) {
        super::owner::seed_prune_scans(1);
        let mut reachable: BTreeSet<_> = self.occurrences.values().cloned().collect();
        loop {
            let before = reachable.len();
            for (child, parent, _) in &self.split_ancestry {
                if reachable.contains(child) {
                    reachable.insert(parent.clone());
                }
            }
            if before == reachable.len() {
                break;
            }
        }
        self.split_ancestry
            .retain(|(child, _, _)| reachable.contains(child));
    }
    pub(super) fn verify(&self, document: &Document) -> Result<(), Error> {
        super::owner::seed_verify_scans(1);
        let expected: BTreeSet<_> = document
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        if !self.occurrences.keys().copied().eq(expected) {
            return Err(Error::InvalidSeeds);
        }
        if self.occurrences.values().collect::<BTreeSet<_>>().len() != self.occurrences.len() {
            return Err(Error::InvalidSeeds);
        }
        let mut children = BTreeSet::new();
        if self
            .split_ancestry
            .iter()
            .any(|(child, parent, _)| child == parent || !children.insert(child))
        {
            return Err(Error::InvalidSeeds);
        }
        let parents = self
            .split_ancestry
            .iter()
            .map(|(child, parent, _)| (child, parent))
            .collect::<BTreeMap<_, _>>();
        let mut done = BTreeSet::new();
        for child in parents.keys() {
            let mut path = BTreeSet::new();
            let mut cursor = *child;
            while !done.contains(cursor) {
                if !path.insert(cursor) {
                    return Err(Error::InvalidSeeds);
                }
                let Some(parent) = parents.get(cursor) else {
                    break;
                };
                cursor = parent;
            }
            done.extend(path);
        }
        Ok(())
    }
}
fn occurrence(origin: &CreationOrigin, result_ordinal: u64) -> OccurrenceId {
    OccurrenceId {
        branch_origin: origin.branch.clone(),
        creation_batch_id: origin.batch.clone(),
        result_ordinal,
    }
}

impl Lineage {
    pub(super) fn capacity_bytes(&self) -> Result<u64, Error> {
        super::owner::seed_capacity_scans(1);
        let mut bytes = 4096_u64;
        let node = (11 * std::mem::size_of::<(ClipId, OccurrenceId)>() + 128) as u64;
        bytes = bytes
            .checked_add(
                (self.occurrences.len() as u64 + 1)
                    .checked_mul(node)
                    .ok_or(Error::Capacity("lineage"))?,
            )
            .ok_or(Error::Capacity("lineage"))?;
        for id in self.occurrences.values() {
            bytes = bytes
                .checked_add(id_bytes(id))
                .ok_or(Error::Capacity("lineage"))?;
        }
        bytes = bytes
            .checked_add(
                (self.split_ancestry.capacity()
                    * std::mem::size_of::<(OccurrenceId, OccurrenceId, Clip)>())
                    as u64,
            )
            .ok_or(Error::Capacity("lineage"))?;
        // The Vec slots above already include each inline Clip. Add only
        // its separately owned buffers here, alongside the two occurrence IDs.
        for (child, parent, clip) in &self.split_ancestry {
            bytes = bytes
                .checked_add(id_bytes(child) + id_bytes(parent))
                .and_then(|n| {
                    n.checked_add(
                        kinewright_core::undo::clip_capacity_bytes(clip).ok()?
                            - std::mem::size_of::<Clip>() as u64,
                    )
                })
                .ok_or(Error::Capacity("lineage"))?;
        }
        Ok(bytes)
    }
    pub(super) fn apply_bounded(
        &mut self,
        origin: &CreationOrigin,
        effects: &[OperationIdentityEffect],
        cap: u64,
    ) -> Result<(), Error> {
        let current = self.capacity_bytes()?;
        let ids =
            (origin.branch.allocated_capacity() + origin.batch.allocated_capacity()) as u64 + 64;
        let mut growth = 0_u64;
        let mut added_ancestry = 0_usize;
        for effect in effects {
            // BTree entries are already charged a complete node each; adding
            // an entry does not double the entire existing occurrence map.
            growth = growth.saturating_add(effect.created.len() as u64 * (2048 + ids));
            added_ancestry = added_ancestry.saturating_add(effect.split_results.len());
            for clip in &effect.split_results {
                growth = growth.saturating_add(
                    kinewright_core::undo::clip_capacity_bytes(clip)?
                        - std::mem::size_of::<Clip>() as u64
                        + 2 * ids
                        + 2048,
                );
            }
        }
        let needed = self.split_ancestry.len().saturating_add(added_ancestry);
        let mut capacity = self.split_ancestry.capacity();
        while capacity < needed {
            capacity = capacity.saturating_mul(2).max(4);
        }
        growth = growth.saturating_add(
            (capacity - self.split_ancestry.capacity()) as u64
                * std::mem::size_of::<(OccurrenceId, OccurrenceId, Clip)>() as u64,
        );
        if current.saturating_add(growth) > cap {
            return Err(Error::Capacity("lineage"));
        }
        self.apply(origin, effects)?;
        Ok(())
    }
}
fn id_bytes(id: &OccurrenceId) -> u64 {
    (id.branch_origin.allocated_capacity() + id.creation_batch_id.allocated_capacity()) as u64 + 32
}
