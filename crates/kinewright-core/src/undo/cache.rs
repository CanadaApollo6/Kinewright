use super::Error;
use crate::Document;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Point {
    depth: usize,
    document: Arc<Document>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Cache {
    points: Vec<Option<Point>>,
}
impl Cache {
    pub fn new(documents: usize, metadata_limit: u64) -> Result<Self, Error> {
        let bytes = documents
            .checked_mul(std::mem::size_of::<Option<Point>>())
            .ok_or(Error::Capacity)?;
        if bytes as u64 > metadata_limit {
            return Err(Error::Capacity);
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(documents)
            .map_err(|_| Error::Capacity)?;
        points.resize_with(documents, || None);
        Ok(Self { points })
    }
    pub fn clear(&mut self) {
        self.points.fill(None);
    }
    pub fn depths(&self) -> Vec<usize> {
        self.points.iter().flatten().map(|p| p.depth).collect()
    }
    pub fn documents(&self) -> impl Iterator<Item = &Arc<Document>> {
        self.points.iter().flatten().map(|p| &p.document)
    }
    pub fn storage_bytes(&self) -> u64 {
        (self.points.capacity() * std::mem::size_of::<Option<Point>>()) as u64
    }
    pub fn exact(&self, depth: usize) -> Option<Arc<Document>> {
        // A cached pointer is shared with the history's current document or a capture.
        self.points
            .iter()
            .flatten()
            .find(|p| p.depth == depth)
            .map(|p| Arc::clone(&p.document))
    }
    pub fn nearest(&self, depth: usize) -> Option<(usize, &Arc<Document>)> {
        self.points
            .iter()
            .flatten()
            .filter(|p| p.depth <= depth)
            .max_by_key(|p| p.depth)
            .map(|p| (p.depth, &p.document))
    }
    fn grid_slots(&self) -> usize {
        self.points.len().saturating_sub(1) / 2
    }
    fn replay_slot(&self, depth: usize) -> Option<usize> {
        let start = self.grid_slots();
        let count = self.points.len().saturating_sub(start + 1);
        (count > 0).then(|| start + (depth.trailing_zeros() as usize).min(count - 1))
    }
    pub fn insert(&mut self, depth: usize, document: Arc<Document>) {
        if depth == 0 || self.points.is_empty() {
            return;
        }
        // Periodic anchors serve short replay gaps. The remaining slots
        // subdivide replays as the cursor walks back.
        let grid = self.grid_slots();
        if grid > 0 && depth.is_multiple_of(32) {
            self.points[(depth / 32) % grid] = Some(Point {
                depth,
                document: Arc::clone(&document),
            });
        }
        if let Some(slot) = self.replay_slot(depth) {
            self.points[slot] = Some(Point {
                depth,
                document: Arc::clone(&document),
            });
        }
        let top = self.points.len() - 1;
        self.points[top] = Some(Point { depth, document });
    }
    // Plan only the final survivor in each near slot. Replay creates those
    // documents as it passes their boundaries; no intermediate snapshots survive.
    pub fn warm_depths(&self, start: usize, end: usize) -> Vec<usize> {
        let mut depths = vec![None; self.points.len()];
        for depth in start + 1..end {
            let grid = self.grid_slots();
            if grid > 0 && depth.is_multiple_of(32) {
                depths[(depth / 32) % grid] = Some(depth);
            }
            if let Some(slot) = self.replay_slot(depth) {
                depths[slot] = Some(depth);
            }
        }
        let mut selected: Vec<_> = depths.into_iter().flatten().collect();
        selected.sort_unstable();
        selected.dedup();
        selected
    }
    pub fn insert_replayed(&mut self, depth: usize, document: &Arc<Document>) {
        if self.points.is_empty() || depth == 0 {
            return;
        }
        let grid = self.grid_slots();
        if grid > 0 && depth.is_multiple_of(32) {
            self.points[(depth / 32) % grid] = Some(Point {
                depth,
                document: Arc::clone(document),
            });
        }
        // Preserve the dedicated pre-top pointer across on-demand queries.
        // Warmed points behind the cursor service a continuing Undo walk.
    }
    pub fn insert_warm(&mut self, depth: usize, document: Arc<Document>) {
        let grid = self.grid_slots();
        if grid > 0 && depth.is_multiple_of(32) {
            self.points[(depth / 32) % grid] = Some(Point {
                depth,
                document: Arc::clone(&document),
            });
        }
        if let Some(slot) = self.replay_slot(depth) {
            self.points[slot] = Some(Point { depth, document });
        }
    }
    pub fn truncate(&mut self, depth: usize) {
        for point in &mut self.points {
            if point.as_ref().is_some_and(|p| p.depth > depth) {
                *point = None;
            }
        }
    }
    pub fn fold(&mut self, steps: usize) {
        for point in &mut self.points {
            if let Some(p) = point {
                if p.depth <= steps {
                    *point = None;
                } else {
                    p.depth -= steps;
                }
            }
        }
    }
}
