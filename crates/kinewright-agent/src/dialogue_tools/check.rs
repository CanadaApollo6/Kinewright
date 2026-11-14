use super::{BoundDialogue, DialogueMeasurements, Refusal, cached, clip_end, request};
use kinewright_core::{
    AudibleSpan, ClipId, Document, ExactMicros, TimeCode, TrackId, audible_span,
    dialogue::audit::{self, AuditSettings, Detail as AuditDetail, KeptRange, RecordingId, Report},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Detail {
    #[default]
    Flags,
    All,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckArgs {
    pub track_id: u64,
    #[serde(default)]
    pub detail: Detail,
    pub range: Option<[TimeCode; 2]>,
}
#[derive(Debug, Serialize)]
pub(crate) struct Segment {
    pub start_frame: i64,
    pub report: Report,
}
#[derive(Debug, Serialize)]
pub(crate) struct NotChecked {
    pub clip: u64,
    pub range: [TimeCode; 2],
    #[serde(flatten)]
    pub reason: Refusal,
}
#[derive(Debug, Serialize)]
// Owned wire provenance remains usable after the prepared-value handles are released.
pub(crate) struct Recording {
    pub asset: u64,
    pub sha256: String,
    pub stream: u32,
    pub recogniser_key: String,
    pub binding: String,
}
#[derive(Debug, Serialize)]
pub(crate) struct CheckResult {
    pub recordings: BTreeMap<RecordingId, Recording>,
    pub segments: Vec<Segment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_checked: Vec<NotChecked>,
    /// [project frame, number of other enabled, unmuted audio-bearing clips].
    pub other_audio: Vec<(i64, usize)>,
    #[serde(skip_serializing_if = "zero")]
    pub other_audio_omitted: usize,
}
#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde skip predicate")]
fn zero(value: &usize) -> bool {
    *value == 0
}
#[derive(Debug)]
pub(crate) struct PlayedSpan {
    pub clip: ClipId,
    pub track: TrackId,
    pub asset: kinewright_core::AssetId,
    pub project: std::ops::Range<TimeCode>,
    pub source: AudibleSpan,
    pub proven: bool,
}

pub(crate) fn spans(doc: &Document) -> Result<Vec<PlayedSpan>, Refusal> {
    if doc.duration.0 == 0 {
        return Ok(Vec::new());
    }
    kinewright_media::timeline_audio_segments(doc, TimeCode::ZERO..doc.duration)
        .map_err(|e| request(e.to_string()))?
        .into_iter()
        .filter(|s| doc.track_audible(s.track))
        .map(|s| {
            let asset = doc.asset(s.asset).ok_or(Refusal::Identity)?;
            let source = audible_span(
                s.source,
                asset.fps,
                TimeCode(s.project.end.0 - s.project.start.0),
                doc.fps,
            )
            .map_err(|e| request(e.to_string()))?;
            let proven = doc
                .tracks
                .iter()
                .find(|t| t.id == s.track)
                .and_then(|t| {
                    let index = t
                        .clips
                        .partition_point(|c| c.timeline_start <= s.project.start);
                    index.checked_sub(1).and_then(|i| t.clips.get(i))
                })
                .is_some_and(|c| c.id == s.clip && c.timeline_start == s.project.start);
            Ok(PlayedSpan {
                clip: s.clip,
                track: s.track,
                asset: s.asset,
                project: s.project,
                source,
                proven,
            })
        })
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "one snapshot, bound recordings and complete track audit"
)]
pub(crate) fn check(
    doc: &Document,
    args: &CheckArgs,
    provider: &dyn DialogueMeasurements,
) -> Result<CheckResult, Refusal> {
    let track = doc
        .tracks
        .iter()
        .find(|t| t.id == TrackId(args.track_id))
        .ok_or_else(|| request("dialogue track is unavailable"))?;
    if args
        .range
        .is_some_and(|r| r[0].0 < 0 || r[0] >= r[1] || r[1] > doc.duration)
    {
        return Err(request("check playback range is outside the document"));
    }
    let all = spans(doc)?;
    let mut selected: Vec<_> = all.iter().filter(|s| s.track == track.id).collect();
    selected.sort_by_key(|s| (s.project.start, s.clip));
    let mut values: BTreeMap<RecordingId, Arc<BoundDialogue>> = BTreeMap::new();
    let mut recordings = BTreeMap::new();
    let mut not_checked = Vec::new();
    for c in &track.clips {
        if c.content.is_media() && c.speed_percent != 100 {
            let range = [c.timeline_start, clip_end(doc, c)?];
            if args
                .range
                .is_some_and(|r| r[1] <= range[0] || range[1] <= r[0])
            {
                continue;
            }
            not_checked.push(NotChecked {
                clip: c.id.0,
                range,
                reason: Refusal::Retimed { clip: c.id.0 },
            });
        }
    }
    let mut pieces = Vec::new();
    for s in selected {
        if args
            .range
            .is_some_and(|r| r[1] <= s.project.start || s.project.end <= r[0])
        {
            continue;
        }
        if !s.proven {
            not_checked.push(NotChecked {
                clip: s.clip.0,
                range: [s.project.start, s.project.end],
                reason: Refusal::UnprovenSegment { clip: s.clip.0 },
            });
            continue;
        }
        if all.iter().any(|other| {
            other.track == s.track
                && other.clip != s.clip
                && other.project.start < s.project.end
                && s.project.start < other.project.end
        }) {
            not_checked.push(NotChecked {
                clip: s.clip.0,
                range: [s.project.start, s.project.end],
                reason: request("overlapping dialogue clips are not a linear cut"),
            });
            continue;
        }
        let id = RecordingId(s.asset.0);
        let result = (|| {
            if !values.contains_key(&id) {
                let value = cached(
                    provider,
                    doc.asset(s.asset).ok_or(Refusal::Identity)?,
                    &values,
                )?;
                recordings.insert(
                    id,
                    Recording {
                        asset: s.asset.0,
                        sha256: value.identity.sha256.clone(),
                        stream: value.stream,
                        recogniser_key: value.recogniser_key.clone(),
                        binding: value.binding.clone(),
                    },
                );
                values.insert(id, value);
            }
            if s.source.end > ExactMicros::integer(values[&id].dialogue.profile().duration_micros())
            {
                return Err(Refusal::NotMeasured);
            }
            Ok(())
        })();
        match result {
            Ok(()) => pieces.push((
                s,
                KeptRange {
                    recording: id,
                    source: s.source,
                },
            )),
            Err(reason) => not_checked.push(NotChecked {
                clip: s.clip.0,
                range: [s.project.start, s.project.end],
                reason,
            }),
        }
    }
    let views: BTreeMap<_, _> = values.iter().map(|(&id, v)| (id, &v.dialogue)).collect();
    let mut segments = Vec::new();
    let mut other_audio = Vec::new();
    let mut other_audio_omitted = 0;
    let mut first = 0;
    while first < pieces.len() {
        let mut end = first + 1;
        while end < pieces.len() && pieces[end - 1].0.project.end == pieces[end].0.project.start {
            end += 1;
        }
        let run = &pieces[first..end];
        let origin = run[0].0.project.start;
        let local = if let Some(range) = args.range {
            if range[1] <= origin || range[0] >= run.last().expect("nonempty").0.project.end {
                first = end;
                continue;
            }
            Some([
                ExactMicros::frames(TimeCode((range[0].0 - origin.0).max(0)), doc.fps)
                    .map_err(|e| request(e.to_string()))?
                    .floor(),
                ExactMicros::frames(TimeCode(range[1].0 - origin.0), doc.fps)
                    .map_err(|e| request(e.to_string()))?
                    .ceil(),
            ])
        } else {
            None
        };
        let settings = AuditSettings {
            detail: match args.detail {
                Detail::Flags => AuditDetail::Flags,
                Detail::All => AuditDetail::All,
            },
            playback_us: local,
            ..AuditSettings::default()
        };
        let kept: Vec<_> = run.iter().map(|(_, k)| *k).collect();
        let report = audit::audit(&views, &kept, &settings, audit::comparison_key)
            .map_err(|e| request(e.to_string()))?;
        // Count even clean boundaries; flags-only may omit their measurements.
        for at in std::iter::once(origin).chain(run.iter().map(|(s, _)| s.project.end)) {
            if args.range.is_some_and(|r| at < r[0] || at >= r[1]) {
                continue;
            }
            let count = other_audible(doc, &all, track.id, at)?;
            if other_audio.len() < 64 {
                other_audio.push((at.0, count));
            } else {
                other_audio_omitted += 1;
            }
        }
        segments.push(Segment {
            start_frame: origin.0,
            report,
        });
        first = end;
    }
    Ok(CheckResult {
        recordings,
        segments,
        not_checked,
        other_audio,
        other_audio_omitted,
    })
}
fn other_audible(
    doc: &Document,
    all: &[PlayedSpan],
    selected: TrackId,
    at: TimeCode,
) -> Result<usize, Refusal> {
    let mut count = 0;
    for s in all
        .iter()
        .filter(|s| s.track != selected && s.project.start <= at && at < s.project.end)
    {
        let offset = ExactMicros::frames(TimeCode(at.0 - s.project.start.0), doc.fps)
            .map_err(|e| request(e.to_string()))?;
        if offset
            < s.source
                .end
                .checked_sub(s.source.start)
                .map_err(|e| request(e.to_string()))?
        {
            count += 1;
        }
    }
    for track in &doc.tracks {
        if track.id == selected || !doc.track_audible(track.id) {
            continue;
        }
        for c in track
            .clips
            .iter()
            .filter(|c| c.speed_percent != 100 && c.content.is_media())
        {
            if c.timeline_start <= at
                && at < clip_end(doc, c)?
                && c.is_enabled_at(TimeCode(at.0 - c.timeline_start.0))
                && doc.asset(c.asset).is_some_and(|a| {
                    matches!(
                        a.kind,
                        kinewright_core::MediaKind::Audio | kinewright_core::MediaKind::AudioVideo
                    )
                })
            {
                count += 1;
            }
        }
    }
    Ok(count)
}

type SourceEdge = (RecordingId, ExactMicros);
type CheckedJoin = (i64, Option<SourceEdge>, Option<SourceEdge>, usize);
#[derive(Serialize)]
pub(crate) struct Created {
    pub recordings: BTreeMap<RecordingId, Recording>,
    /// [project frame, outgoing [recording, exact us], incoming [recording, exact us], other audio count].
    pub checked_at: Vec<CheckedJoin>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<(i64, audit::Join)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<audit::NamedWord>,
}
pub(crate) fn check_created(
    doc: &Document,
    track: TrackId,
    positions: &[TimeCode],
    values: &BTreeMap<kinewright_core::AssetId, Arc<BoundDialogue>>,
) -> Result<Created, Refusal> {
    let all = spans(doc)?;
    let mut recordings = BTreeMap::new();
    let mut checked_at = Vec::new();
    let mut flags = Vec::new();
    let mut words = Vec::new();
    for &at in positions {
        let out = all.iter().find(|s| s.track == track && s.project.end == at);
        let inn = all
            .iter()
            .find(|s| s.track == track && s.project.start == at);
        if out.is_none() && inn.is_none() {
            if !doc
                .tracks
                .iter()
                .any(|t| t.id == track && !t.clips.is_empty())
            {
                continue;
            }
            return Err(request(
                "the committed document has no created join at the planned position",
            ));
        }
        let mut kept = Vec::new();
        for s in [out, inn].into_iter().flatten() {
            if !s.proven {
                return Err(Refusal::UnprovenSegment { clip: s.clip.0 });
            }
            let id = RecordingId(s.asset.0);
            let v = values.get(&s.asset).ok_or(Refusal::NotMeasured)?;
            recordings.entry(id).or_insert_with(|| Recording {
                asset: s.asset.0,
                sha256: v.identity.sha256.clone(),
                stream: v.stream,
                recogniser_key: v.recogniser_key.clone(),
                binding: v.binding.clone(),
            });
            kept.push(KeptRange {
                recording: id,
                source: s.source,
            });
        }
        let join_us = out
            .map_or(Ok(ExactMicros::ZERO), |s| s.source.duration())
            .map_err(|e| request(e.to_string()))?
            .floor();
        let views: BTreeMap<_, _> = values
            .iter()
            .map(|(&id, v)| (RecordingId(id.0), &v.dialogue))
            .collect();
        let report = audit::audit(
            &views,
            &kept,
            &AuditSettings {
                detail: AuditDetail::Flags,
                playback_us: Some([join_us, join_us + 1]),
                ..AuditSettings::default()
            },
            audit::comparison_key,
        )
        .map_err(|e| request(e.to_string()))?;
        words.extend(report.words);
        for item in report.items {
            if let audit::Item::Join(join) = item
                && join.at_us == join_us
            {
                flags.push((at.0, join));
            }
        }
        checked_at.push((
            at.0,
            out.map(|s| (RecordingId(s.asset.0), s.source.end)),
            inn.map(|s| (RecordingId(s.asset.0), s.source.start)),
            other_audible(doc, &all, track, at)?,
        ));
    }
    words.sort_by_key(|w| w.key);
    words.dedup_by_key(|w| w.key);
    Ok(Created {
        recordings,
        checked_at,
        flags,
        words,
    })
}
