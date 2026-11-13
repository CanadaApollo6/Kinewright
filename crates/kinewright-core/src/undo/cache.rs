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
    pub fn exact(&self, depth: usize) -> Option<Arc<Document>> {
        // The returned pointer is shared by history's current document and this disposable cache.
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
    pub fn insert(&mut self, depth: usize, document: Arc<Document>) {
        if self.points.is_empty() || depth == 0 {
            return;
        }
        let top = self.points.len() - 1;
        if top > 0 {
            let slot = (depth.trailing_zeros() as usize).min(top - 1);
            self.points[slot] = Some(Point {
                depth,
                document: Arc::clone(&document),
            });
        }
        self.points[top] = Some(Point { depth, document });
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
