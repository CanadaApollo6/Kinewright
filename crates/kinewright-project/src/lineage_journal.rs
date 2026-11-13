//! Compact occurrence seeds on disk; existing per-boundary maps in memory.
use crate::bundle::BundleError;
use crate::bundle_records::{BranchId, LineageRecord, OccurrenceId};
use kinewright_core::{Clip, ClipId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) fn verify_history_boundaries(
    lineage: &LineageRecord,
    history: &kinewright_core::HistorySnapshot,
) -> Result<(), BundleError> {
    let clip_keys = |document: &kinewright_core::Document| {
        document
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(|clip| clip.id))
            .collect::<std::collections::BTreeSet<_>>()
    };
    if lineage
        .baseline
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        != clip_keys(&history.baseline)
        || lineage
            .current
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            != clip_keys(&history.document)
        || lineage.undo.len() != history.cursor.undo.len()
        || lineage.redo.len() != history.cursor.redo.len()
        || lineage
            .undo
            .iter()
            .chain(&lineage.redo)
            .zip(history.cursor.undo.iter().chain(&history.cursor.redo))
            .any(|(keys, entry)| {
                keys.keys()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>()
                    != clip_keys(&entry.document)
            })
    {
        return Err(BundleError::Corrupt(
            "lineage boundary differs from history".into(),
        ));
    }
    Ok(())
}

fn map_live_bound(entries: usize) -> Result<u64, BundleError> {
    // BTree nodes have at most eleven entries and at least five, apart from
    // their root. Include edges, node headers and both fixed-width identity strings.
    let nodes = entries.div_ceil(5).saturating_add(1) as u64;
    let node = (11 * std::mem::size_of::<(ClipId, OccurrenceId)>()
        + 12 * std::mem::size_of::<usize>()
        + 32) as u64;
    nodes
        .checked_mul(node)
        .and_then(|bytes| bytes.checked_add((entries as u64).checked_mul(64)?))
        .ok_or(BundleError::Capacity("lineage heap overflow"))
}

pub(crate) fn live_bytes(lineage: &LineageRecord) -> Result<u64, BundleError> {
    let mut bytes = 2048_u64;
    for map in [&lineage.baseline, &lineage.current]
        .into_iter()
        .chain(lineage.undo.iter())
        .chain(lineage.redo.iter())
    {
        bytes = bytes
            .checked_add(map_live_bound(map.len())?)
            .ok_or(BundleError::Capacity("lineage heap overflow"))?;
    }
    bytes
        .checked_add(crate::history_size::payload(&lineage.split_ancestry)?)
        .ok_or(BundleError::Capacity("lineage heap overflow"))
}

pub(crate) fn decode_live_bound(bytes: &[u8]) -> Result<u64, BundleError> {
    let journal: LineageJournal =
        serde_json::from_slice(bytes).map_err(|error| BundleError::Corrupt(error.to_string()))?;
    let mut charge = 2048_u64
        .checked_add(map_live_bound(journal.baseline.len())?)
        .and_then(|bytes| bytes.checked_add(map_live_bound(journal.current.len()).ok()?))
        .ok_or(BundleError::Capacity("lineage heap overflow"))?;
    for steps in [&journal.undo, &journal.redo] {
        let mut entries = journal.baseline.len();
        for step in steps {
            // Replacements can only make this upper bound more conservative.
            entries = entries
                .checked_add(step.assigned.len())
                .and_then(|count| count.checked_sub(step.removed.len()))
                .ok_or(BundleError::Capacity("lineage cursor size"))?;
            charge = charge
                .checked_add(map_live_bound(entries)?)
                .ok_or(BundleError::Capacity("lineage heap overflow"))?;
        }
    }
    charge
        .checked_add(crate::history_size::payload(&journal.split_ancestry)?)
        .ok_or(BundleError::Capacity("lineage heap overflow"))
}

/// The occurrence carries its original batch ID, branch origin and ordinal.
/// Rebuilding a map never allocates a fresh identity or uses a reusable clip ID.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OccurrenceStep {
    removed: Vec<ClipId>,
    assigned: BTreeMap<ClipId, OccurrenceId>,
}

impl OccurrenceStep {
    fn between(
        before: &BTreeMap<ClipId, OccurrenceId>,
        after: &BTreeMap<ClipId, OccurrenceId>,
    ) -> Self {
        Self {
            removed: before
                .keys()
                .filter(|id| !after.contains_key(*id))
                .copied()
                .collect(),
            assigned: after
                .iter()
                .filter(|(id, occurrence)| before.get(*id) != Some(*occurrence))
                .map(|(id, occurrence)| (*id, occurrence.clone()))
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineageJournal {
    branch_origin: BranchId,
    baseline: BTreeMap<ClipId, OccurrenceId>,
    current: BTreeMap<ClipId, OccurrenceId>,
    undo: Vec<OccurrenceStep>,
    redo: Vec<OccurrenceStep>,
    split_ancestry: Vec<(OccurrenceId, OccurrenceId, Clip)>,
}

/// Recovery keeps the independently stored current occurrences without replaying
/// the unavailable Undo maps. The ordinary reader still rebuilds every map.
pub(crate) fn restore_current(
    bytes: &[u8],
    reserve: impl FnOnce(u64) -> Result<crate::storage_budget::StorageReservation, BundleError>,
) -> Result<crate::storage_budget::StorageReservation<LineageRecord>, BundleError> {
    let record: LineageJournal =
        serde_json::from_slice(bytes).map_err(|error| BundleError::Corrupt(error.to_string()))?;
    if record
        .undo
        .len()
        .checked_add(record.redo.len())
        .is_none_or(|count| count > kinewright_core::HISTORY_COMMANDS)
    {
        return Err(BundleError::Capacity("lineage cursor capacity"));
    }
    let ancestry = crate::history_size::payload(&record.split_ancestry)?;
    let live = map_live_bound(record.current.len())?
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(2048))
        .and_then(|bytes| bytes.checked_add(ancestry))
        .ok_or(BundleError::Capacity("current lineage heap"))?;
    let reservation = reserve(live)?;
    Ok(reservation.hold(LineageRecord {
        branch_origin: record.branch_origin,
        baseline: record.current.clone(),
        current: record.current,
        undo: vec![],
        redo: vec![],
        split_ancestry: record.split_ancestry,
    }))
}

fn steps(
    baseline: &BTreeMap<ClipId, OccurrenceId>,
    entries: &[BTreeMap<ClipId, OccurrenceId>],
) -> Vec<OccurrenceStep> {
    let mut previous = baseline;
    entries
        .iter()
        .map(|entry| {
            let step = OccurrenceStep::between(previous, entry);
            previous = entry;
            step
        })
        .collect()
}

fn maps(
    baseline: &BTreeMap<ClipId, OccurrenceId>,
    steps: Vec<OccurrenceStep>,
) -> Result<Vec<BTreeMap<ClipId, OccurrenceId>>, String> {
    let mut previous = baseline.clone();
    let mut maps = Vec::with_capacity(steps.len());
    for step in steps {
        for id in step.removed {
            if previous.remove(&id).is_none() {
                return Err("lineage removes an absent occurrence".into());
            }
        }
        previous.extend(step.assigned);
        // These map snapshots are the existing in-process contract, retained in stage 1.
        maps.push(previous.clone());
    }
    Ok(maps)
}

impl Serialize for LineageRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut record = serializer.serialize_struct("LineageJournal", 6)?;
        record.serialize_field("branch_origin", &self.branch_origin)?;
        record.serialize_field("baseline", &self.baseline)?;
        record.serialize_field("current", &self.current)?;
        record.serialize_field("undo", &steps(&self.baseline, &self.undo))?;
        record.serialize_field("redo", &steps(&self.baseline, &self.redo))?;
        record.serialize_field("split_ancestry", &self.split_ancestry)?;
        record.end()
    }
}

impl<'de> Deserialize<'de> for LineageRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = LineageJournal::deserialize(deserializer)?;
        if record
            .undo
            .len()
            .checked_add(record.redo.len())
            .is_none_or(|count| count > kinewright_core::HISTORY_COMMANDS)
        {
            return Err(serde::de::Error::custom("lineage cursor capacity"));
        }
        let undo = maps(&record.baseline, record.undo).map_err(serde::de::Error::custom)?;
        let redo = maps(&record.baseline, record.redo).map_err(serde::de::Error::custom)?;
        Ok(Self {
            branch_origin: record.branch_origin,
            baseline: record.baseline,
            current: record.current,
            undo,
            redo,
            split_ancestry: record.split_ancestry,
        })
    }
}
