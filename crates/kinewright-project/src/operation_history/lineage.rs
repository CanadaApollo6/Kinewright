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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    pub occurrences: BTreeMap<ClipId, OccurrenceId>,
    pub split_ancestry: Vec<(OccurrenceId, OccurrenceId, Clip)>,
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
                self.split_ancestry
                    .push((child.clone(), parent.clone(), clip.clone()));
                self.occurrences.insert(clip.id, child);
            }
        }
        Ok(())
    }
    pub(super) fn prune(&mut self) {
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
        let expected: BTreeSet<_> = document
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        if !self.occurrences.keys().copied().eq(expected) {
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
