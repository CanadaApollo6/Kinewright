//! The join audit (L11): every audible edge of a speech-bearing kept segment
//! is guarded on its source PCM, with the kept words' source spans as the
//! words and voice. No recognition runs; the output is not transcribed.
//!
//! Edges at output 0 and at the output's end are guarded like any other
//! (F5): what matters is whether the source is cut inside sound. A segment
//! that starts at its source's first sample or ends at its last cuts
//! nothing there.
use std::collections::BTreeMap;
use std::ops::Range;

use kinewright_core::dialogue::measure::ProfileBuilder;
use kinewright_core::dialogue::{
    AudibleSide, Dialogue, RefusalReason, Settings, SourceRange, Word,
};
use kinewright_core::{AssetId, ExactMicros, ExportCancellation, MediaError};

use super::plan::{Plan, samples_at};
use super::{MAX_JOIN_EDGES, MAX_JOIN_PROFILE_HOPS, RetainedSpeechConfig, ReviewScanInputs};
use crate::audio::AudioDecoder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JoinVerdict {
    Clean,
    InSound(String),
    Unmeasured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct JoinResult {
    pub output_us: u64,
    pub verdict: JoinVerdict,
}

#[derive(Debug, Default)]
pub(super) struct JoinScan {
    pub joins: Vec<JoinResult>,
    /// Edges found, including those not guarded.
    pub expected: u64,
    /// Edges past `MAX_JOIN_EDGES` or the profile budget.
    pub omitted: u64,
    /// Assets whose profile could not be built, with the edges it held.
    pub failed: Vec<(AssetId, String, u64)>,
}

/// One audible edge on its source clock.
struct Edge {
    source: ExactMicros,
    output_us: u64,
}

pub(super) fn guard_joins(
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    config: &RetainedSpeechConfig,
    cancellation: &ExportCancellation,
) -> Result<JoinScan, MediaError> {
    let mut scan = JoinScan::default();
    // Kept-word source spans, by asset, deduplicated (F21: one profile per
    // asset, no per-join copies).
    let mut words: BTreeMap<AssetId, Vec<(u64, u64, &str)>> = BTreeMap::new();
    for word in inputs.expectations.kept_words.known() {
        let Some((part, heard)) = plan.word_source(word) else {
            continue;
        };
        let asset = plan.audio[part].segment.asset;
        words.entry(asset).or_default().push((
            heard.start.floor(),
            heard.end.ceil(),
            word.text.as_str(),
        ));
    }
    let mut edges: BTreeMap<AssetId, Vec<Edge>> = BTreeMap::new();
    for part in &plan.audio {
        let asset = part.segment.asset;
        if part.muted || !words.contains_key(&asset) {
            continue;
        }
        let duration = part.asset.and_then(Plan::asset_duration);
        let sides = [
            (
                part.segment.source.start,
                part.segment.output.start,
                part.continued[0],
            ),
            (
                part.segment.source.end,
                part.segment.output.end,
                part.continued[1],
            ),
        ];
        for (source, output, continued) in sides {
            let uncut = source == ExactMicros::integer(0) || Some(source) == duration;
            if continued || uncut {
                continue;
            }
            scan.expected += 1;
            if usize::try_from(scan.expected).unwrap_or(usize::MAX) > MAX_JOIN_EDGES {
                scan.omitted += 1;
                continue;
            }
            edges.entry(asset).or_default().push(Edge {
                source,
                output_us: output.floor(),
            });
        }
    }
    let mut budget = MAX_JOIN_PROFILE_HOPS;
    for (asset, mut asset_edges) in edges {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        asset_edges.sort_by_key(|edge| edge.source);
        let count = u64::try_from(asset_edges.len()).unwrap_or(u64::MAX);
        let Some(media) = plan
            .document()
            .media_pool
            .iter()
            .find(|item| item.id == asset)
        else {
            scan.failed
                .push((asset, "the asset is not in the document".into(), count));
            continue;
        };
        let duration_us = Plan::asset_duration(media).map_or(u64::MAX, ExactMicros::floor);
        let windows = windows(&asset_edges, config.join_window_us, duration_us);
        let hops: u64 = windows
            .iter()
            .map(|window| (window.end - window.start).div_ceil(10_000))
            .sum();
        if hops > budget {
            scan.omitted += count;
            continue;
        }
        budget -= hops;
        let asset_words = words.remove(&asset).unwrap_or_default();
        match profile(inputs, media, &windows, &asset_words, config, cancellation) {
            Ok((dialogue, offsets)) => {
                for edge in &asset_edges {
                    let verdict = guard(
                        &dialogue,
                        config.join_profile_rate,
                        &windows,
                        &offsets,
                        edge.source,
                    );
                    scan.joins.push(JoinResult {
                        output_us: edge.output_us,
                        verdict,
                    });
                }
            }
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error) => scan.failed.push((asset, super::reason(&error), count)),
        }
    }
    Ok(scan)
}

/// Merged source windows of `reach` on each side of every edge.
fn windows(edges: &[Edge], reach: u64, duration_us: u64) -> Vec<Range<u64>> {
    let mut merged: Vec<Range<u64>> = Vec::new();
    for edge in edges {
        let at = edge.source.floor();
        let window = at.saturating_sub(reach)..at.saturating_add(reach).min(duration_us);
        match merged.last_mut() {
            Some(last) if window.start <= last.end => last.end = last.end.max(window.end),
            _ => merged.push(window),
        }
    }
    merged
}

/// The source windows decoded into one profile, with each window's first
/// sample on the profile clock.
fn profile(
    inputs: &ReviewScanInputs<'_>,
    media: &kinewright_core::MediaAsset,
    windows: &[Range<u64>],
    words: &[(u64, u64, &str)],
    config: &RetainedSpeechConfig,
    cancellation: &ExportCancellation,
) -> Result<(Dialogue, Vec<(u64, u64)>), MediaError> {
    let refuse = |error: &dyn std::fmt::Display| MediaError::Backend(super::reason(&error));
    let rate = config.join_profile_rate;
    let certified = inputs.sources.source(media.id)?;
    let mut builder = ProfileBuilder::new(rate, 1).map_err(|e| refuse(&e))?;
    // (window start sample, profile start sample) per window.
    let mut offsets = Vec::with_capacity(windows.len());
    let mut position = 0_u64;
    for window in windows {
        let start = samples_at(ExactMicros::integer(window.start), rate);
        let end = samples_at(ExactMicros::integer(window.end), rate);
        let mut decoder =
            AudioDecoder::open_with_source(&media.path, rate, 1, start, end, Some(&certified))?;
        offsets.push((start, position));
        let mut taken = 0_u64;
        while let Some(chunk) = decoder.next_chunk()? {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let keep = u64::try_from(chunk.len())
                .unwrap_or(0)
                .min(end - start - taken.min(end - start));
            builder
                .push(&chunk[..usize::try_from(keep).unwrap_or(0)])
                .map_err(|e| refuse(&e))?;
            taken += keep;
            if taken >= end - start {
                break;
            }
        }
        position += taken;
    }
    let to_profile = |at: u64| -> Option<u64> {
        let sample = samples_at(ExactMicros::integer(at), rate);
        let index = offsets
            .partition_point(|(start, _)| *start <= sample)
            .checked_sub(1)?;
        let (start, profile_start) = offsets[index];
        let window = &windows[index];
        (at < window.end).then(|| {
            let local = at - start * 1_000_000 / u64::from(rate);
            profile_start * 1_000_000 / u64::from(rate) + local
        })
    };
    let mut spoken: Vec<Word> = Vec::new();
    let mut voice: Vec<SourceRange> = Vec::new();
    let mut sorted: Vec<&(u64, u64, &str)> = words.iter().collect();
    sorted.sort_unstable();
    sorted.dedup();
    for &&(start, end, text) in &sorted {
        // Words outside every window cannot touch a guarded edge.
        let (Some(start), Some(end)) = (to_profile(start), to_profile(end.saturating_sub(1)))
        else {
            continue;
        };
        let end = end + 1;
        if start >= end || spoken.last().is_some_and(|last| start < last.end_micros) {
            continue;
        }
        spoken.push(Word {
            text: text.to_owned(),
            start_micros: start,
            end_micros: end,
        });
        match voice.last_mut() {
            Some(range) if start <= range.end_micros => {
                range.end_micros = range.end_micros.max(end);
            }
            _ => voice.push(SourceRange {
                start_micros: start,
                end_micros: end,
            }),
        }
    }
    let profile = builder.finish(Some(&voice)).map_err(|e| refuse(&e))?;
    let dialogue = Dialogue::new(profile, spoken, Settings::default()).map_err(|e| refuse(&e))?;
    Ok((dialogue, offsets))
}

fn guard(
    dialogue: &Dialogue,
    rate: u32,
    windows: &[Range<u64>],
    offsets: &[(u64, u64)],
    source: ExactMicros,
) -> JoinVerdict {
    let at = source.floor();
    let Some(index) = windows.iter().position(|window| window.contains(&at)) else {
        return JoinVerdict::Unmeasured;
    };
    let (start, profile_start) = offsets[index];
    let exact = |sample: u64| ExactMicros::new(u128::from(sample) * 1_000_000, u64::from(rate));
    let mapped = match (exact(start), exact(profile_start)) {
        (Ok(window_start), Ok(profile_at)) => source
            .checked_sub(window_start)
            .ok()
            .and_then(|local| local.checked_add(profile_at).ok()),
        _ => None,
    };
    let Some(mapped) = mapped else {
        return JoinVerdict::Unmeasured;
    };
    match dialogue.guard_exact(mapped, AudibleSide::In) {
        Ok(()) => JoinVerdict::Clean,
        Err(refusal) => match refusal.reason {
            RefusalReason::Unmeasured | RefusalReason::TimeOverflow => JoinVerdict::Unmeasured,
            reason => JoinVerdict::InSound(super::reason(&reason)),
        },
    }
}
