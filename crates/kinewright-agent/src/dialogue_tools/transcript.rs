//! Current word addresses on the same audible clock as the transcript.
use super::check::PlayedSpan;
use super::{BoundDialogue, DialogueMeasurements, Refusal, cached, request};
use kinewright_core::{AssetId, Document, ExactMicros, TimeCode, TrackId, dialogue::Word};
use std::{collections::BTreeMap, sync::Arc};

pub(crate) fn word_frames(
    doc: &Document,
    span: &PlayedSpan,
    word: &Word,
) -> Result<Option<[i64; 2]>, Refusal> {
    let start = ExactMicros::integer(word.start_micros).max(span.source.start);
    let end = ExactMicros::integer(word.end_micros).min(span.source.end);
    if start >= end {
        return Ok(None);
    }
    let grid = kinewright_core::dialogue::FrameGrid::new(span.source.start, doc.fps)
        .map_err(|e| request(e.to_string()))?;
    let frame = |t| {
        grid.frame_at(t, kinewright_core::FrameRounding::Nearest)
            .map(|f| f.0 + span.project.start.0)
            .map_err(|e| request(e.to_string()))
    };
    Ok(Some([frame(start)?, frame(end)?]))
}

type Address = (u64, usize);
#[derive(serde::Serialize)]
struct Unavailable {
    clip: u64,
    reason: Refusal,
}
#[derive(serde::Serialize)]
struct Neighbours {
    at: i64,
    left: Vec<Address>,
    right: Vec<Address>,
    not_measured: Vec<Unavailable>,
}
fn side(
    doc: &Document,
    spans: &[PlayedSpan],
    at: TimeCode,
    left: bool,
    values: &mut BTreeMap<AssetId, Arc<BoundDialogue>>,
    provider: &dyn DialogueMeasurements,
) -> (Vec<Address>, Option<Unavailable>) {
    let mut words = Vec::new();
    // Walk from the join outwards; do not skip unknown words to report farther ones.
    let mut candidates = spans.iter().filter(|s| {
        if left {
            s.project.start < at
        } else {
            s.project.end > at
        }
    });
    while let Some(span) = if left {
        candidates.next_back()
    } else {
        candidates.next()
    } {
        let found: Result<(), Refusal> = (|| {
            if !span.proven {
                return Err(Refusal::UnprovenSegment { clip: span.clip.0 });
            }
            if !values.contains_key(&span.asset) {
                let value = cached(
                    provider,
                    doc.asset(span.asset).ok_or(Refusal::Identity)?,
                    values,
                )?;
                values.insert(span.asset, value);
            }
            let value = &values[&span.asset];
            let source = value.dialogue().words();
            let mut indices = 0..source.len();
            while let Some(index) = if left {
                indices.next_back()
            } else {
                indices.next()
            } {
                if let Some([start, end]) = word_frames(doc, span, &source[index])?
                    && if left { end <= at.0 } else { start >= at.0 }
                {
                    words.push((span.clip.0, index));
                    if words.len() == 3 {
                        break;
                    }
                }
            }
            Ok(())
        })();
        if let Err(reason) = found {
            if left {
                words.reverse();
            }
            return (
                words,
                Some(Unavailable {
                    clip: span.clip.0,
                    reason,
                }),
            );
        }
        if words.len() == 3 {
            break;
        }
    }
    if left {
        words.reverse();
    }
    (words, None)
}

pub(crate) fn neighbours(
    doc: &Document,
    track: TrackId,
    joins: &[TimeCode],
    critical: &BTreeMap<AssetId, Arc<BoundDialogue>>,
    provider: &dyn DialogueMeasurements,
) -> serde_json::Value {
    let mut played = match super::spans(doc) {
        Ok(p) => p
            .into_iter()
            .filter(|s| s.track == track)
            .collect::<Vec<_>>(),
        Err(reason) => return serde_json::json!({"not_measured":reason}),
    };
    played.sort_by_key(|s| (s.project.start, s.clip));
    // Share the exact immutable proof values. Context lookups extend only this
    // readback cache and cannot replace a measurement used to authorize the edit.
    let mut values: BTreeMap<_, _> = critical
        .iter()
        .map(|(&id, v)| (id, Arc::clone(v)))
        .collect();
    let mut output = Vec::new();
    for &at in joins {
        let (left, a) = side(doc, &played, at, true, &mut values, provider);
        let (right, b) = side(doc, &played, at, false, &mut values, provider);
        output.push(Neighbours {
            at: at.0,
            left,
            right,
            not_measured: a.into_iter().chain(b).collect(),
        });
    }
    let mut bindings = BTreeMap::new();
    for row in &output {
        for &(clip, _) in row.left.iter().chain(&row.right) {
            if let Some(asset) = doc.clip(kinewright_core::ClipId(clip)).map(|c| c.asset)
                && let Some(value) = values.get(&asset)
            {
                bindings.insert(clip, value.binding());
            }
        }
    }
    serde_json::json!({"bindings":bindings,"joins":output})
}
