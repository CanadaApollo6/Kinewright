// Edge definitions from Scarlet evals/src/metrics.ts at 2cbab542; see LICENSES/scarlet-MIT.txt.
// Assignment deliberately maximizes cardinality before minimizing error.
use super::metrics::{Edge, EdgeMatch, EdgeSide};

type Point = (u64, usize);

fn ordered(edges: &[Edge], side: EdgeSide) -> Vec<Point> {
    // Sort numeric views, retaining the original occurrence indices.
    let mut points: Vec<_> = edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.side == side)
        .map(|(i, e)| (e.at_micros, i))
        .collect();
    points.sort_unstable();
    points
}

fn within(a: u64, b: u64, window: u64) -> bool {
    a.abs_diff(b) <= window
}

pub(super) fn match_count(candidate: &[Edge], reference: &[Edge], window: u64) -> usize {
    let mut count = 0;
    for side in [EdgeSide::In, EdgeSide::Out] {
        let c = ordered(candidate, side);
        let r = ordered(reference, side);
        let (mut i, mut j) = (0, 0);
        while i < c.len() && j < r.len() {
            if within(c[i].0, r[j].0, window) {
                count += 1;
                i += 1;
                j += 1;
            } else if c[i].0 < r[j].0 {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    count
}

#[derive(Clone, Copy, Default)]
struct Value {
    count: usize,
    error: u128,
}
impl Value {
    fn better(self, other: Self) -> bool {
        self.count > other.count || self.count == other.count && self.error < other.error
    }
    fn plus(self, other: Self) -> Self {
        Self {
            count: self.count + other.count,
            error: self.error + other.error,
        }
    }
}

fn row(c: &[Point], r: &[Point], window: u64, reverse: bool) -> Vec<Value> {
    let mut row = vec![Value::default(); r.len() + 1];
    for i in 0..c.len() {
        let c = c[if reverse { c.len() - 1 - i } else { i }].0;
        let mut diagonal = row[0];
        for j in 0..r.len() {
            let old = row[j + 1];
            let right = r[if reverse { r.len() - 1 - j } else { j }].0;
            let mut best = if row[j].better(old) { row[j] } else { old };
            if within(c, right, window) {
                let paired = diagonal.plus(Value {
                    count: 1,
                    error: u128::from(c.abs_diff(right)),
                });
                if paired.better(best) {
                    best = paired;
                }
            }
            row[j + 1] = best;
            diagonal = old;
        }
    }
    row
}

/// Reconstruct an order-preserving optimum with linear row storage.
/// Releasing the two rows before descending avoids a quadratic traceback table.
fn assignment(c: &[Point], r: &[Point], window: u64, pairs: &mut Vec<(usize, usize)>) {
    if c.is_empty() || r.is_empty() {
        return;
    }
    if c.len() == 1 {
        if let Some(&point) = r
            .iter()
            .filter(|p| within(c[0].0, p.0, window))
            .min_by_key(|p| (c[0].0.abs_diff(p.0), p.1))
        {
            pairs.push((c[0].1, point.1));
        }
        return;
    }
    if r.len() == 1 {
        if let Some(&point) = c
            .iter()
            .filter(|p| within(p.0, r[0].0, window))
            .min_by_key(|p| (p.0.abs_diff(r[0].0), p.1))
        {
            pairs.push((point.1, r[0].1));
        }
        return;
    }
    let middle = c.len() / 2;
    let left = row(&c[..middle], r, window, false);
    let right = row(&c[middle..], r, window, true);
    let mut best = Value::default();
    let mut split = 0;
    for j in 0..=r.len() {
        let value = left[j].plus(right[r.len() - j]);
        if value.better(best) {
            best = value;
            split = j;
        }
    }
    drop(left);
    drop(right);
    if best.count == 0 {
        return;
    }
    assignment(&c[..middle], &r[..split], window, pairs);
    assignment(&c[middle..], &r[split..], window, pairs);
}

pub(super) fn match_edges(candidate: &[Edge], reference: &[Edge], window: u64) -> Vec<EdgeMatch> {
    let mut pairs = Vec::new();
    for side in [EdgeSide::In, EdgeSide::Out] {
        assignment(
            &ordered(candidate, side),
            &ordered(reference, side),
            window,
            &mut pairs,
        );
    }
    pairs.sort_by_key(|&(i, j)| (candidate[i].at_micros, i, j));
    pairs
        .into_iter()
        .map(|(i, j)| EdgeMatch {
            candidate_index: i,
            reference_index: j,
            error_micros: i128::from(candidate[i].at_micros) - i128::from(reference[j].at_micros),
        })
        .collect()
}
