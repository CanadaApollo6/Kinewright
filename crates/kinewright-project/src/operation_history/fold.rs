//! Sparse, fallible baseline updates. Neither the live maps nor their payloads
//! change until Core and owner admission have both succeeded.
use super::{CreationOrigin, Error, Lineage, Resource, Resources};
use crate::bundle_records::{BatchId, BranchId, OccurrenceId};
use kinewright_core::{Clip, ClipContent, ClipId, Document, Operation, OperationIdentityEffect};
use serde::{Serialize, ser::SerializeMap};
use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;

type Members = BTreeMap<u64, ClipId>;
type Batches = BTreeMap<BatchId, Members>;
fn capacity() -> Error {
    Error::Capacity("fold metadata")
}
fn add(a: u64, b: u64) -> Result<u64, Error> {
    a.checked_add(b).ok_or_else(capacity)
}
fn sub(a: u64, b: u64) -> Result<u64, Error> {
    a.checked_sub(b).ok_or(Error::InvalidSeeds)
}
// Charge a complete eleven-slot BTree node per entry; sparse roots and
// allocator overhead are included rather than normalizing capacity by copy.
fn node<K, V>() -> u64 {
    (11 * size_of::<(K, V)>() + 128) as u64
}
fn id_bytes(id: &OccurrenceId) -> u64 {
    (id.branch_origin.allocated_capacity() + id.creation_batch_id.allocated_capacity()) as u64 + 32
}
#[derive(Clone, Copy, Default, Debug)]
struct Point {
    encoded: u64,
    decoded: u64,
    live: u64,
}
impl Point {
    fn plus(self, b: Self) -> Result<Self, Error> {
        Ok(Self {
            encoded: add(self.encoded, b.encoded)?,
            decoded: add(self.decoded, b.decoded)?,
            live: add(self.live, b.live)?,
        })
    }
    fn minus(self, b: Self) -> Result<Self, Error> {
        Ok(Self {
            encoded: sub(self.encoded, b.encoded)?,
            decoded: sub(self.decoded, b.decoded)?,
            live: sub(self.live, b.live)?,
        })
    }
}
fn occurrence_point(id: ClipId, value: &OccurrenceId, limit: u64) -> Result<Point, Error> {
    struct One<'a>(ClipId, &'a OccurrenceId);
    impl Serialize for One<'_> {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut m = s.serialize_map(Some(1))?;
            m.serialize_entry(&self.0, self.1)?;
            m.end()
        }
    }
    let (encoded, decoded) = super::stored::payload_charge(&One(id, value), limit)?;
    Ok(Point {
        encoded,
        decoded,
        live: node::<ClipId, OccurrenceId>() + id_bytes(value),
    })
}
fn edge_point(
    child: &OccurrenceId,
    parent: &OccurrenceId,
    clip: &Clip,
    limit: u64,
) -> Result<Point, Error> {
    let live = add(
        id_bytes(child) + id_bytes(parent),
        kinewright_core::undo::clip_capacity_bytes(clip)?,
    )?;
    let (encoded, decoded) = super::stored::payload_charge(&(child, parent, clip), limit)?;
    Ok(Point {
        encoded,
        decoded,
        live,
    })
}
#[derive(Debug)]
struct Edge {
    slot: usize,
    users: usize,
    order: u64,
    point: Point,
}
#[derive(Debug)]
struct Seeds {
    live: BTreeMap<BranchId, Batches>,
    edges: BTreeMap<OccurrenceId, Edge>,
    roots: BTreeMap<OccurrenceId, usize>,
    point: Point,
    reverse_bytes: u64,
    edge_bytes: u64,
    array_slots: usize,
    next_order: u64,
}
impl Seeds {
    fn reverse(&self, id: &OccurrenceId) -> Option<ClipId> {
        self.live
            .get(&id.branch_origin)?
            .get(&id.creation_batch_id)?
            .get(&id.result_ordinal)
            .copied()
    }
    fn insert_live(&mut self, id: &OccurrenceId, clip: ClipId) {
        self.live
            .entry(id.branch_origin.clone())
            .or_default()
            .entry(id.creation_batch_id.clone())
            .or_default()
            .insert(id.result_ordinal, clip);
    }
    fn remove_live(&mut self, id: &OccurrenceId) {
        let branch_empty = if let Some(batches) = self.live.get_mut(&id.branch_origin) {
            let batch_empty = if let Some(members) = batches.get_mut(&id.creation_batch_id) {
                members.remove(&id.result_ordinal);
                members.is_empty()
            } else {
                false
            };
            if batch_empty {
                batches.remove(&id.creation_batch_id);
            }
            batches.is_empty()
        } else {
            false
        };
        if branch_empty {
            self.live.remove(&id.branch_origin);
        }
    }
    fn new(lineage: &Lineage, limit: u64) -> Result<Self, Error> {
        let mut out = Self {
            live: BTreeMap::new(),
            edges: BTreeMap::new(),
            roots: BTreeMap::new(),
            point: Point::default(),
            reverse_bytes: 0,
            edge_bytes: 0,
            array_slots: lineage.split_ancestry.capacity(),
            next_order: 0,
        };
        for (clip, id) in &lineage.occurrences {
            if out.reverse(id).is_some() {
                return Err(Error::InvalidSeeds);
            }
            out.point = out.point.plus(occurrence_point(*clip, id, limit)?)?;
            out.insert_live(id, *clip);
        }
        for (branch, batches) in &out.live {
            out.reverse_bytes = add(
                out.reverse_bytes,
                node::<BranchId, Batches>() + branch.allocated_capacity() as u64,
            )?;
            for (batch, members) in batches {
                out.reverse_bytes = add(
                    out.reverse_bytes,
                    node::<BatchId, Members>()
                        + batch.allocated_capacity() as u64
                        + members.len() as u64 * node::<u64, ClipId>(),
                )?;
            }
        }
        for (slot, (child, parent, clip)) in lineage.split_ancestry.iter().enumerate() {
            let point = edge_point(child, parent, clip, limit)?;
            out.point = out.point.plus(point)?;
            out.edge_bytes = add(
                out.edge_bytes,
                node::<OccurrenceId, Edge>() + id_bytes(child),
            )?;
            if out
                .edges
                .insert(
                    child.clone(),
                    Edge {
                        slot,
                        users: 0,
                        order: out.next_order,
                        point,
                    },
                )
                .is_some()
            {
                return Err(Error::InvalidSeeds);
            }
            out.next_order = out.next_order.checked_add(1).ok_or_else(capacity)?;
        }
        for id in lineage.occurrences.values() {
            if let Some(edge) = out.edges.get_mut(id) {
                edge.users += 1;
            }
        }
        for (_, parent, _) in &lineage.split_ancestry {
            if let Some(edge) = out.edges.get_mut(parent) {
                edge.users += 1;
            } else {
                *out.roots.entry(parent.clone()).or_default() += 1;
            }
        }
        for root in out.roots.keys() {
            out.edge_bytes = add(
                out.edge_bytes,
                node::<OccurrenceId, usize>() + id_bytes(root),
            )?;
        }
        if out.edges.values().any(|edge| edge.users == 0) {
            return Err(Error::InvalidSeeds);
        }
        Ok(out)
    }
    fn live_bytes(&self) -> Result<u64, Error> {
        add(
            add(add(self.point.live, self.reverse_bytes)?, self.edge_bytes)?,
            4096 + node::<ClipId, OccurrenceId>()
                + (self.array_slots * size_of::<(OccurrenceId, OccurrenceId, Clip)>()) as u64,
        )
    }
}
#[derive(Debug, Default)]
struct ResourceIndex {
    media: BTreeMap<u64, Resource>,
    luts: BTreeMap<u64, Resource>,
    titles: BTreeSet<ClipId>,
    counts: BTreeMap<Resource, u64>,
    bytes: u64,
}
fn resource_heap(r: &Resource) -> u64 {
    match r {
        Resource::Media {
            sha256: Some(s), ..
        }
        | Resource::Lut { sha256: s, .. } => s.capacity() as u64,
        _ => 0,
    }
}
fn resource_slot(r: &Resource) -> u64 {
    node::<u64, Resource>() + resource_heap(r)
}
fn resource_count(r: &Resource) -> u64 {
    node::<Resource, u64>() + resource_heap(r)
}
fn media(id: u64, fp: &kinewright_core::MediaSourceFingerprint) -> Resource {
    Resource::Media {
        asset: id,
        sha256: fp.content_sha256.clone(),
        byte_len: fp.byte_len,
    }
}
impl ResourceIndex {
    fn contribute(&mut self, r: Resource) {
        *self.counts.entry(r).or_default() += 1;
    }
    fn new(doc: &Document) -> Result<Self, Error> {
        super::owner::scan();
        super::owner::resource_document_scans(1);
        let mut out = Self::default();
        for asset in &doc.media_pool {
            let r = media(asset.id.0, &asset.source_fingerprint);
            out.bytes = add(out.bytes, resource_slot(&r))?;
            out.media.insert(asset.id.0, r.clone());
            out.contribute(r);
        }
        for asset in &doc.lut_assets {
            let r = Resource::Lut {
                sha256: asset.sha256.clone(),
                byte_len: asset.byte_len,
            };
            out.bytes = add(out.bytes, resource_slot(&r))?;
            out.luts.insert(asset.id.0, r.clone());
            out.contribute(r);
        }
        for clip in doc.tracks.iter().flat_map(|t| &t.clips) {
            if matches!(clip.content, ClipContent::Title(_)) {
                out.titles.insert(clip.id);
                out.contribute(Resource::TitleFont);
            }
        }
        out.bytes = add(out.bytes, out.titles.len() as u64 * node::<ClipId, ()>())?;
        for r in out.counts.keys() {
            out.bytes = add(out.bytes, resource_count(r))?;
        }
        Ok(out)
    }
}
#[derive(Debug)]
pub(super) struct FoldState {
    seeds: Seeds,
    resources: ResourceIndex,
}
impl FoldState {
    pub fn new(lineage: &Lineage, doc: &Document, workspace: u64) -> Result<Self, Error> {
        Ok(Self {
            seeds: Seeds::new(lineage, workspace / 4)?,
            resources: ResourceIndex::new(doc)?,
        })
    }
    pub fn costs(&self) -> Result<(u64, u64), Error> {
        costs(
            self.seeds.point,
            self.seeds.live_bytes()?,
            self.resources.bytes,
            self.resources.counts.len(),
        )
    }
    /// Captures and queries own their lineage; slot compaction never copies a
    /// live seed map on the command path. Stable order matches the old journal.
    pub fn ordered_ancestry<'a>(
        &self,
        lineage: &'a Lineage,
    ) -> Vec<&'a (OccurrenceId, OccurrenceId, Clip)> {
        let mut edges: Vec<_> = lineage.split_ancestry.iter().collect();
        edges.sort_unstable_by_key(|(child, _, _)| {
            self.seeds
                .edges
                .get(child)
                .expect("indexed live ancestor")
                .order
        });
        edges
    }
    pub fn materialize(&self, lineage: &Lineage) -> Lineage {
        let mut out = lineage.clone();
        out.split_ancestry.sort_unstable_by_key(|(child, _, _)| {
            self.seeds
                .edges
                .get(child)
                .expect("indexed live ancestor")
                .order
        });
        out
    }
}
fn costs(
    point: Point,
    live: u64,
    resource_bytes: u64,
    resources: usize,
) -> Result<(u64, u64), Error> {
    let bytes = add(
        add(
            (add(point.encoded, 128)?
                .checked_mul(4)
                .ok_or_else(capacity)?)
            .max(live),
            resource_bytes,
        )?,
        resources as u64 * 512 + 4096,
    )?;
    let decode = add(point.decoded, resources as u64 * 4096 + 8192)?;
    Ok((bytes, decode))
}
#[derive(Debug)]
struct NewEdge {
    parent: OccurrenceId,
    clip: Clip,
    order: u64,
    point: Point,
}
#[derive(Debug, Default)]
struct SeedPatch {
    occurrences: BTreeMap<ClipId, Option<OccurrenceId>>,
    reverse: BTreeMap<OccurrenceId, Option<ClipId>>,
    added: BTreeMap<OccurrenceId, NewEdge>,
    refs: BTreeMap<OccurrenceId, isize>,
    roots: BTreeMap<OccurrenceId, isize>,
    parents: BTreeSet<OccurrenceId>,
    removed: BTreeSet<OccurrenceId>,
    point: Point,
    reverse_bytes: u64,
    edge_bytes: u64,
    array_slots: usize,
    next_order: u64,
    scratch: u64,
}
impl SeedPatch {
    fn new(s: &Seeds, cap: u64) -> Self {
        Self {
            scratch: cap,
            point: s.point,
            reverse_bytes: s.reverse_bytes,
            edge_bytes: s.edge_bytes,
            array_slots: s.array_slots,
            next_order: s.next_order,
            ..Self::default()
        }
    }
    fn charge(&mut self, bytes: u64) -> Result<(), Error> {
        self.scratch = self.scratch.checked_sub(bytes).ok_or_else(capacity)?;
        Ok(())
    }
    fn current<'a>(&'a self, seeds: &'a Lineage, id: ClipId) -> Option<&'a OccurrenceId> {
        self.occurrences
            .get(&id)
            .map_or_else(|| seeds.occurrences.get(&id), Option::as_ref)
    }
    fn set(
        &mut self,
        state: &Seeds,
        seeds: &Lineage,
        id: ClipId,
        new: Option<OccurrenceId>,
    ) -> Result<(), Error> {
        let heap = self.current(seeds, id).map_or(0, id_bytes) + new.as_ref().map_or(0, id_bytes);
        // Occurrence/reverse overlays, later leaf references, grouped reverse
        // deltas and work queues. Charge before allocating any sparse node.
        self.charge(
            node::<ClipId, Option<OccurrenceId>>()
                + 3 * node::<OccurrenceId, Option<ClipId>>()
                + 12 * heap
                + 1024,
        )?;
        super::owner::folded_seed_ids(1);
        // The sparse patch owns the identities it must install after Core's
        // borrowed transaction ends; unrelated occurrences are never copied.
        if let Some(old) = self.current(seeds, id).cloned() {
            self.reverse.insert(old, None);
        }
        if let Some(value) = &new {
            let current = self
                .reverse
                .get(value)
                .copied()
                .unwrap_or_else(|| state.reverse(value));
            if current.is_some_and(|other| other != id) {
                return Err(Error::InvalidSeeds);
            }
            self.reverse.insert(value.clone(), Some(id));
        }
        self.occurrences.insert(id, new);
        Ok(())
    }
    fn apply(
        &mut self,
        state: &Seeds,
        seeds: &Lineage,
        origin: &CreationOrigin,
        effects: &[OperationIdentityEffect],
        limit: u64,
    ) -> Result<(), Error> {
        let mut ordinal = 0;
        for effect in effects {
            let heap = (origin.branch.allocated_capacity() + origin.batch.allocated_capacity())
                as u64
                + 32;
            self.charge(effect.created.len() as u64 * 2 * heap)?;
            let parent_heap = effect
                .split_parent
                .as_ref()
                .and_then(|c| self.current(seeds, c.id))
                .map_or(0, id_bytes);
            if parent_heap != 0 {
                self.charge(node::<OccurrenceId, ()>() + 4 * parent_heap)?;
            }
            let parent = effect
                .split_parent
                .as_ref()
                .and_then(|c| self.current(seeds, c.id))
                .cloned();
            for id in &effect.deleted {
                self.set(state, seeds, *id, None)?;
            }
            let split: BTreeSet<_> = effect.split_results.iter().map(|c| c.id).collect();
            for id in &effect.created {
                if !split.contains(id) {
                    self.set(state, seeds, *id, Some(occurrence(origin, ordinal)))?;
                    ordinal += 1;
                }
            }
            for clip in &effect.split_results {
                let child = occurrence(origin, ordinal);
                ordinal += 1;
                let parent = parent.as_ref().ok_or(Error::InvalidSeeds)?;
                if child == *parent
                    || state.reverse(&child).is_some()
                    || state.edges.contains_key(&child)
                    || state.roots.contains_key(&child)
                    || self.added.contains_key(&child)
                    || self.parents.contains(&child)
                {
                    return Err(Error::InvalidSeeds);
                }
                self.parents.insert(parent.clone());
                let point = edge_point(&child, parent, clip, limit)?;
                self.charge(
                    point.live
                        + node::<OccurrenceId, NewEdge>()
                        + 4 * id_bytes(&child)
                        + 4 * id_bytes(parent)
                        + 1024,
                )?;
                let order = self.next_order;
                self.next_order = self.next_order.checked_add(1).ok_or_else(capacity)?;
                // This retained split result is independent of Core's temporary observation.
                self.added.insert(
                    child.clone(),
                    NewEdge {
                        parent: parent.clone(),
                        clip: clip.clone(),
                        order,
                        point,
                    },
                );
                self.set(state, seeds, clip.id, Some(child))?;
            }
        }
        Ok(())
    }
    fn adjust(&mut self, state: &Seeds, id: &OccurrenceId, amount: isize) -> Result<(), Error> {
        if self.added.contains_key(id) || state.edges.contains_key(id) {
            let n = self.refs.entry(id.clone()).or_default();
            *n = n.checked_add(amount).ok_or_else(capacity)?;
        }
        Ok(())
    }
    fn parent_adjust(
        &mut self,
        state: &Seeds,
        id: &OccurrenceId,
        amount: isize,
    ) -> Result<(), Error> {
        if self.added.contains_key(id) || state.edges.contains_key(id) {
            self.adjust(state, id, amount)
        } else {
            let n = self.roots.entry(id.clone()).or_default();
            *n = n.checked_add(amount).ok_or_else(capacity)?;
            Ok(())
        }
    }
    fn users(&self, state: &Seeds, id: &OccurrenceId) -> Result<usize, Error> {
        let initial = if self.added.contains_key(id) {
            0
        } else {
            state.edges.get(id).ok_or(Error::InvalidSeeds)?.users
        };
        initial
            .checked_add_signed(self.refs.get(id).copied().unwrap_or(0))
            .ok_or(Error::InvalidSeeds)
    }
    fn finish(&mut self, state: &Seeds, seeds: &Lineage, limit: u64) -> Result<(), Error> {
        // All leaf changes are known before pruning, including delete/recreate
        // within one folded batch. Only causally dead parent chains are visited.
        let changes = std::mem::take(&mut self.occurrences);
        for (id, new) in &changes {
            if let Some(old) = seeds.occurrences.get(id) {
                self.point = self.point.minus(occurrence_point(*id, old, limit)?)?;
                if state.edges.contains_key(old) {
                    self.adjust(state, old, -1)?;
                }
            }
            if let Some(value) = new {
                self.point = self.point.plus(occurrence_point(*id, value, limit)?)?;
                self.adjust(state, value, 1)?;
            }
        }
        self.occurrences = changes;
        for edge in self.added.values() {
            let deltas = if self.added.contains_key(&edge.parent)
                || state.edges.contains_key(&edge.parent)
            {
                &mut self.refs
            } else {
                &mut self.roots
            };
            let n = deltas.entry(edge.parent.clone()).or_default();
            *n = n.checked_add(1).ok_or_else(capacity)?;
        }
        let mut dead: Vec<_> = self
            .refs
            .keys()
            .chain(self.added.keys())
            .filter(|id| self.users(state, id) == Ok(0))
            .cloned()
            .collect();
        while let Some(child) = dead.pop() {
            if self.removed.contains(&child) || self.users(state, &child)? != 0 {
                continue;
            }
            let parent = if let Some(edge) = self.added.get(&child) {
                &edge.parent
            } else {
                &seeds.split_ancestry[state.edges[&child].slot].1
            };
            self.charge(
                4 * node::<OccurrenceId, usize>()
                    + 12 * (id_bytes(&child) + id_bytes(parent))
                    + 1024,
            )?;
            super::owner::folded_ancestry(1);
            let parent = if let Some(edge) = self.added.remove(&child) {
                edge.parent
            } else {
                let edge = state.edges.get(&child).ok_or(Error::InvalidSeeds)?;
                self.removed.insert(child.clone());
                self.point = self.point.minus(edge.point)?;
                self.edge_bytes = sub(
                    self.edge_bytes,
                    node::<OccurrenceId, Edge>() + id_bytes(&child),
                )?;
                seeds.split_ancestry[edge.slot].1.clone()
            };
            self.parent_adjust(state, &parent, -1)?;
            if (state.edges.contains_key(&parent) || self.added.contains_key(&parent))
                && self.users(state, &parent)? == 0
            {
                dead.push(parent);
            }
        }
        for (child, edge) in &self.added {
            self.point = self.point.plus(edge.point)?;
            self.edge_bytes = add(
                self.edge_bytes,
                node::<OccurrenceId, Edge>() + id_bytes(child),
            )?;
        }
        let needed = seeds.split_ancestry.len() - self.removed.len() + self.added.len();
        self.array_slots = if needed == 0 {
            0
        } else if needed > seeds.split_ancestry.capacity() {
            self.array_slots.max(needed.saturating_mul(2).max(4))
        } else {
            self.array_slots
        };
        for (root, delta) in &self.roots {
            let old = state.roots.get(root).copied().unwrap_or(0);
            let next = old.checked_add_signed(*delta).ok_or(Error::InvalidSeeds)?;
            if old == 0 && next > 0 {
                self.edge_bytes = add(
                    self.edge_bytes,
                    node::<OccurrenceId, usize>() + id_bytes(root),
                )?;
            }
            if old > 0 && next == 0 {
                self.edge_bytes = sub(
                    self.edge_bytes,
                    node::<OccurrenceId, usize>() + id_bytes(root),
                )?;
            }
        }
        self.reverse_bytes = reverse_cost(state, &self.reverse)?;
        Ok(())
    }
    fn live_bytes(&self) -> Result<u64, Error> {
        add(
            add(add(self.point.live, self.reverse_bytes)?, self.edge_bytes)?,
            4096 + node::<ClipId, OccurrenceId>()
                + (self.array_slots * size_of::<(OccurrenceId, OccurrenceId, Clip)>()) as u64,
        )
    }
    fn commit(self, state: &mut Seeds, seeds: &mut Lineage) {
        for id in self.occurrences.keys() {
            if let Some(old) = seeds.occurrences.remove(id) {
                state.remove_live(&old);
            }
        }
        for (id, new) in self.occurrences {
            if let Some(value) = new {
                state.insert_live(&value, id);
                seeds.occurrences.insert(id, value);
            }
        }
        for (child, delta) in &self.refs {
            if !self.removed.contains(child)
                && let Some(edge) = state.edges.get_mut(child)
            {
                edge.users = edge
                    .users
                    .checked_add_signed(*delta)
                    .expect("prepared reference count");
            }
        }
        for (root, delta) in self.roots {
            let next = state
                .roots
                .get(&root)
                .copied()
                .unwrap_or(0)
                .checked_add_signed(delta)
                .expect("prepared root count");
            if next == 0 {
                state.roots.remove(&root);
            } else {
                state.roots.insert(root, next);
            }
        }
        for child in self.removed {
            let edge = state.edges.remove(&child).expect("prepared removal");
            seeds.split_ancestry.swap_remove(edge.slot);
            if let Some((moved, _, _)) = seeds.split_ancestry.get(edge.slot) {
                state
                    .edges
                    .get_mut(moved)
                    .expect("indexed moved ancestor")
                    .slot = edge.slot;
            }
        }
        let mut added: Vec<_> = self.added.into_iter().collect();
        added.sort_unstable_by_key(|(_, edge)| edge.order);
        for (child, edge) in added {
            let slot = seeds.split_ancestry.len();
            seeds
                .split_ancestry
                .push((child.clone(), edge.parent, edge.clip));
            let users = usize::try_from(self.refs.get(&child).copied().unwrap_or(0))
                .expect("prepared child references");
            state.edges.insert(
                child,
                Edge {
                    slot,
                    users,
                    order: edge.order,
                    point: edge.point,
                },
            );
        }
        if seeds.split_ancestry.is_empty() {
            seeds.split_ancestry = Vec::new();
        }
        state.point = self.point;
        state.reverse_bytes = self.reverse_bytes;
        state.edge_bytes = self.edge_bytes;
        state.array_slots = self.array_slots;
        state.next_order = self.next_order;
    }
}
fn occurrence(origin: &CreationOrigin, ordinal: u64) -> OccurrenceId {
    OccurrenceId {
        branch_origin: origin.branch.clone(),
        creation_batch_id: origin.batch.clone(),
        result_ordinal: ordinal,
    }
}
fn reverse_cost(
    state: &Seeds,
    changes: &BTreeMap<OccurrenceId, Option<ClipId>>,
) -> Result<u64, Error> {
    let mut groups = BTreeMap::<&BranchId, BTreeMap<&BatchId, isize>>::new();
    for (id, new) in changes {
        let old = state.reverse(id);
        let delta = isize::from(new.is_some()) - isize::from(old.is_some());
        *groups
            .entry(&id.branch_origin)
            .or_default()
            .entry(&id.creation_batch_id)
            .or_default() += delta;
    }
    let mut bytes = state.reverse_bytes;
    for (branch, batches) in groups {
        let old = state.live.get(branch);
        let mut count = old.map_or(0, BTreeMap::len);
        for (batch, delta) in batches {
            let n = old.and_then(|m| m.get(batch)).map_or(0, BTreeMap::len);
            let next = n.checked_add_signed(delta).ok_or(Error::InvalidSeeds)?;
            bytes = sub(bytes, n as u64 * node::<u64, ClipId>())?;
            bytes = add(bytes, next as u64 * node::<u64, ClipId>())?;
            if n == 0 && next > 0 {
                bytes = add(
                    bytes,
                    node::<BatchId, Members>() + batch.allocated_capacity() as u64,
                )?;
                count += 1;
            }
            if n > 0 && next == 0 {
                bytes = sub(
                    bytes,
                    node::<BatchId, Members>() + batch.allocated_capacity() as u64,
                )?;
                count -= 1;
            }
        }
        if old.is_none() && count > 0 {
            bytes = add(
                bytes,
                node::<BranchId, Batches>() + branch.allocated_capacity() as u64,
            )?;
        }
        if old.is_some() && count == 0 {
            bytes = sub(
                bytes,
                node::<BranchId, Batches>() + branch.allocated_capacity() as u64,
            )?;
        }
    }
    Ok(bytes)
}

#[derive(Debug, Default)]
struct ResourcePatch {
    media: BTreeMap<u64, Resource>,
    luts: BTreeMap<u64, Option<Resource>>,
    titles: BTreeMap<ClipId, bool>,
    deltas: BTreeMap<Resource, isize>,
    bytes: u64,
    unique: usize,
}
impl ResourcePatch {
    fn new(state: &ResourceIndex) -> Self {
        Self {
            bytes: state.bytes,
            unique: state.counts.len(),
            ..Self::default()
        }
    }
    fn delta(&mut self, r: Resource, amount: isize) {
        *self.deltas.entry(r).or_default() += amount;
    }
    fn title(&mut self, state: &ResourceIndex, id: ClipId, new: bool) {
        super::owner::folded_resource_ids(1);
        let old = self
            .titles
            .get(&id)
            .copied()
            .unwrap_or_else(|| state.titles.contains(&id));
        if old != new {
            self.delta(Resource::TitleFont, if new { 1 } else { -1 });
        }
        self.titles.insert(id, new);
    }
    fn apply(
        &mut self,
        state: &ResourceIndex,
        operations: &[Operation],
        effects: &[OperationIdentityEffect],
    ) -> Result<(), Error> {
        if operations.len() != effects.len() {
            return Err(Error::InvalidOrigin);
        }
        for (operation, effect) in operations.iter().zip(effects) {
            for id in &effect.deleted {
                self.title(state, *id, false);
            }
            match operation {
                Operation::AddAsset { asset } => {
                    self.asset(
                        state,
                        asset.id.0,
                        media(asset.id.0, &asset.source_fingerprint),
                    );
                }
                Operation::RelinkAsset {
                    asset, candidate, ..
                } => {
                    self.asset(state, asset.0, media(asset.0, &candidate.fingerprint));
                }
                Operation::AddLutAsset { asset } => {
                    self.lut(
                        state,
                        asset.id.0,
                        Some(Resource::Lut {
                            sha256: asset.sha256.clone(),
                            byte_len: asset.byte_len,
                        }),
                    );
                }
                Operation::RemoveLutAsset { lut_asset } => {
                    self.lut(state, lut_asset.0, None);
                }
                Operation::AddTitle { .. } => {
                    for id in &effect.created {
                        self.title(state, *id, true);
                    }
                }
                Operation::AddTrack { track } => {
                    for clip in &track.clips {
                        if matches!(clip.content, ClipContent::Title(_)) {
                            self.title(state, clip.id, true);
                        }
                    }
                }
                _ => {}
            }
            for clip in &effect.split_results {
                self.title(
                    state,
                    clip.id,
                    matches!(clip.content, ClipContent::Title(_)),
                );
            }
        }
        Ok(())
    }
    fn asset(&mut self, state: &ResourceIndex, id: u64, new: Resource) {
        super::owner::folded_resource_ids(1);
        if let Some(old) = self
            .media
            .get(&id)
            .or_else(|| state.media.get(&id))
            .cloned()
        {
            self.delta(old, -1);
        }
        self.delta(new.clone(), 1);
        self.media.insert(id, new);
    }
    fn lut(&mut self, state: &ResourceIndex, id: u64, new: Option<Resource>) {
        super::owner::folded_resource_ids(1);
        let old = self
            .luts
            .get(&id)
            .cloned()
            .unwrap_or_else(|| state.luts.get(&id).cloned());
        if let Some(old) = old {
            self.delta(old, -1);
        }
        if let Some(value) = &new {
            self.delta(value.clone(), 1);
        }
        self.luts.insert(id, new);
    }
    fn finish(&mut self, state: &ResourceIndex) -> Result<(), Error> {
        for (id, new) in &self.media {
            if let Some(old) = state.media.get(id) {
                self.bytes = sub(self.bytes, resource_slot(old))?;
            }
            self.bytes = add(self.bytes, resource_slot(new))?;
        }
        for (id, new) in &self.luts {
            if let Some(old) = state.luts.get(id) {
                self.bytes = sub(self.bytes, resource_slot(old))?;
            }
            if let Some(new) = new {
                self.bytes = add(self.bytes, resource_slot(new))?;
            }
        }
        for (id, new) in &self.titles {
            let old = state.titles.contains(id);
            if old && !*new {
                self.bytes = sub(self.bytes, node::<ClipId, ()>())?;
            }
            if !old && *new {
                self.bytes = add(self.bytes, node::<ClipId, ()>())?;
            }
        }
        for (r, delta) in &self.deltas {
            let old = state.counts.get(r).copied().unwrap_or(0);
            let next = old
                .checked_add_signed(i64::try_from(*delta).map_err(|_| capacity())?)
                .ok_or(Error::InvalidResource)?;
            if old == 0 && next > 0 {
                self.bytes = add(self.bytes, resource_count(r))?;
                self.unique += 1;
            }
            if old > 0 && next == 0 {
                self.bytes = sub(self.bytes, resource_count(r))?;
                self.unique -= 1;
            }
        }
        Ok(())
    }
    fn commit(self, state: &mut ResourceIndex, resources: &mut Resources) {
        state.media.extend(self.media);
        for (id, new) in self.luts {
            if let Some(value) = new {
                state.luts.insert(id, value);
            } else {
                state.luts.remove(&id);
            }
        }
        for (id, new) in self.titles {
            if new {
                state.titles.insert(id);
            } else {
                state.titles.remove(&id);
            }
        }
        for (r, delta) in self.deltas {
            let next = state
                .counts
                .get(&r)
                .copied()
                .unwrap_or(0)
                .checked_add_signed(i64::try_from(delta).expect("prepared resource delta"))
                .expect("prepared resource count");
            if next == 0 {
                state.counts.remove(&r);
                resources.baseline.remove(&r);
            } else {
                resources.baseline.insert(r.clone());
                state.counts.insert(r, next);
            }
        }
        state.bytes = self.bytes;
    }
}
#[derive(Debug)]
pub(super) struct FoldPatch {
    limit: u64,
    seeds: SeedPatch,
    resources: ResourcePatch,
}
impl FoldPatch {
    pub fn new(state: &FoldState, cap: u64, workspace: u64) -> Self {
        Self {
            limit: (workspace / 4).min(cap / 4),
            seeds: SeedPatch::new(&state.seeds, cap),
            resources: ResourcePatch::new(&state.resources),
        }
    }
    pub fn apply(
        &mut self,
        state: &FoldState,
        seeds: &Lineage,
        origin: &CreationOrigin,
        operations: &[Operation],
        effects: &[OperationIdentityEffect],
    ) -> Result<(), Error> {
        // At most A/2 of encoding spare capacity coexists with the A-sized
        // sparse patch. The Core transaction reserves 3A for all owner work,
        // also covering the new origin and fixed approval-update vectors.
        for operation in operations {
            if matches!(
                operation,
                Operation::AddAsset { .. }
                    | Operation::RelinkAsset { .. }
                    | Operation::AddLutAsset { .. }
                    | Operation::RemoveLutAsset { .. }
            ) {
                self.seeds.charge(8192)?;
            }
        }
        self.seeds
            .apply(&state.seeds, seeds, origin, effects, self.limit)?;
        self.resources.apply(&state.resources, operations, effects)
    }
    pub fn finish(&mut self, state: &FoldState, seeds: &Lineage) -> Result<(u64, u64), Error> {
        // The same point limit used during apply also covers removed points.
        self.seeds.finish(&state.seeds, seeds, self.limit)?;
        self.resources.finish(&state.resources)?;
        costs(
            self.seeds.point,
            self.seeds.live_bytes()?,
            self.resources.bytes,
            self.resources.unique,
        )
    }
    pub fn commit(self, state: &mut FoldState, seeds: &mut Lineage, resources: &mut Resources) {
        self.seeds.commit(&mut state.seeds, seeds);
        self.resources.commit(&mut state.resources, resources);
    }
}

#[cfg(test)]
impl FoldState {
    pub fn assert_matches(&self, seeds: &Lineage, doc: &Document, resources: &BTreeSet<Resource>) {
        let encoded = serde_json::to_vec(seeds).unwrap();
        assert!(
            self.seeds.point.encoded + 128 >= encoded.len() as u64,
            "CACHED_SEED_ENCODING_COVERS_REAL_LINEAGE"
        );
        let decoded =
            kinewright_core::undo::payload_memory_bound(&encoded).unwrap() + encoded.len() as u64;
        assert!(
            self.seeds.point.decoded + 8192 >= decoded,
            "CACHED_SEED_DECODE_COVERS_REAL_LINEAGE"
        );
        assert!(
            self.seeds.live_bytes().unwrap() >= seeds.capacity_bytes().unwrap(),
            "CACHED_SEED_LIVE_BOUND_COUNTS_REAL_CAPACITY"
        );
        assert!(
            self.resources.counts.keys().eq(resources),
            "INCREMENTAL_BASELINE_RESOURCE_SET"
        );
        assert_eq!(
            &Resources::document(doc),
            resources,
            "INCREMENTAL_BASELINE_EQUALS_DOCUMENT_REFERENCES"
        );
        assert_eq!(
            self.seeds
                .live
                .values()
                .flat_map(|b| b.values())
                .map(BTreeMap::len)
                .sum::<usize>(),
            seeds.occurrences.len(),
            "REVERSE_SEED_INDEX_SIZE"
        );
        for (clip, id) in &seeds.occurrences {
            assert_eq!(
                self.seeds.reverse(id),
                Some(*clip),
                "REVERSE_SEED_INDEX_IDENTITY"
            );
        }
        assert_eq!(
            self.seeds.edges.len(),
            seeds.split_ancestry.len(),
            "ANCESTRY_INDEX_SIZE"
        );
        assert!(
            self.seeds.array_slots >= seeds.split_ancestry.capacity(),
            "ANCESTRY_SPARE_CAPACITY_IS_CHARGED"
        );
        let mut users = BTreeMap::<&OccurrenceId, usize>::new();
        for id in seeds.occurrences.values() {
            *users.entry(id).or_default() += 1;
        }
        for (child, parent, _) in &seeds.split_ancestry {
            *users.entry(parent).or_default() += 1;
            let edge = &self.seeds.edges[child];
            assert_eq!(
                &seeds.split_ancestry[edge.slot].0, child,
                "ANCESTRY_SLOT_IDENTITY"
            );
        }
        for (child, edge) in &self.seeds.edges {
            assert_eq!(
                edge.users,
                users.get(child).copied().unwrap_or(0),
                "ANCESTRY_USERS"
            );
        }
        for (root, count) in &self.seeds.roots {
            assert_eq!(
                *count,
                users.get(root).copied().unwrap_or(0),
                "ANCESTRY_ROOT_USERS"
            );
        }
    }
}
