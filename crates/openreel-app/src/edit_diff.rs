//! Changed-range computation between document snapshots (M24 phase 2).
//!
//! The session stream turns each agent edit into a watchable diff: the range
//! returned here is what the monitor cues and what an operation card plays.

use std::collections::BTreeMap;

use openreel_core::{Clip, ClipId, Document, TimeCode};

/// The project-frame span in the NEW document that a change affects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChangedRange {
    pub start: TimeCode,
    pub end: TimeCode,
}

/// Compare two documents and return the span covering real content changes.
///
/// A ripple edit shifts every downstream clip, which a naive field diff would
/// report as "everything after the cut changed". Clips that are identical
/// when aligned from the document END (same content, same distance from the
/// end) are treated as an unchanged, shifted suffix and excluded, so the
/// range covers the seams a reviewer actually needs to watch. Returns `None`
/// when nothing content-visible changed (for example marker or sync-lock
/// edits).
pub(crate) fn changed_project_range(old: &Document, new: &Document) -> Option<ChangedRange> {
    let old_clips = clip_index(old);
    let new_clips = clip_index(new);

    let mut start: Option<i64> = None;
    let mut end: Option<i64> = None;
    let mut cover = |from: i64, to: i64| {
        start = Some(start.map_or(from, |value| value.min(from)));
        end = Some(end.map_or(to, |value| value.max(to)));
    };

    for (id, (old_clip, old_end)) in &old_clips {
        match new_clips.get(id) {
            None => {
                // Removed: the seam sits where the clip used to start, which
                // in the new document is the same frame (content before it is
                // unchanged or handled by its own entry).
                cover(old_clip.timeline_start.0, old_clip.timeline_start.0);
            }
            Some((new_clip, new_end)) => {
                let content_equal = clip_content_equal(old_clip, new_clip);
                if content_equal && old_clip.timeline_start == new_clip.timeline_start {
                    continue; // identical in place
                }
                let old_tail = old.duration.0 - old_end;
                let new_tail = new.duration.0 - new_end;
                if content_equal && old_tail == new_tail {
                    continue; // unchanged suffix, merely shifted by the edit
                }
                cover(new_clip.timeline_start.0, *new_end);
            }
        }
    }
    for (id, (new_clip, new_end)) in &new_clips {
        if !old_clips.contains_key(id) {
            cover(new_clip.timeline_start.0, *new_end);
        }
    }

    let (start, end) = (start?, end?);
    let maximum = new.duration.0.max(0);
    Some(ChangedRange {
        start: TimeCode(start.clamp(0, maximum)),
        end: TimeCode(end.clamp(0, maximum).max(start.clamp(0, maximum))),
    })
}

fn clip_index(document: &Document) -> BTreeMap<ClipId, (&Clip, i64)> {
    let mut index = BTreeMap::new();
    for clip in document.tracks.iter().flat_map(|track| &track.clips) {
        let end = document
            .clip_duration(clip)
            .map_or(clip.timeline_start.0, |duration| {
                clip.timeline_start.0.saturating_add(duration.0)
            });
        index.insert(clip.id, (clip, end));
    }
    index
}

/// Everything that affects rendered output, ignoring absolute position.
fn clip_content_equal(left: &Clip, right: &Clip) -> bool {
    left.asset == right.asset
        && left.source_range == right.source_range
        && left.content == right.content
        && left.effects == right.effects
        && left.transition_in == right.transition_in
        && left.speed_percent == right.speed_percent
        && left.audio_gain_tenth_db == right.audio_gain_tenth_db
        && left.audio_fade_in_frames == right.audio_fade_in_frames
        && left.audio_fade_out_frames == right.audio_fade_out_frames
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use openreel_core::{
        AssetId, ClipContent, MediaAsset, MediaKind, Operation, Rational, Track, TrackId, TrackKind,
    };

    use super::*;

    fn fixture() -> Document {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("diff.mp4"),
            name: "diff.mp4".to_owned(),
            duration: TimeCode(600),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
        };
        let clips = [(1, 0, 0..100), (2, 100, 100..200), (3, 200, 200..300)]
            .into_iter()
            .map(|(id, at, source)| Clip {
                id: ClipId(id),
                asset: AssetId(1),
                source_range: TimeCode(source.start)..TimeCode(source.end),
                content: ClipContent::Media,
                timeline_start: TimeCode(at),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            })
            .collect();
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips,
            }],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(300),
        }
    }

    #[test]
    fn identical_documents_have_no_changed_range() {
        let document = fixture();
        assert_eq!(changed_project_range(&document, &document.clone()), None);
    }

    #[test]
    fn marker_only_changes_are_not_content_changes() {
        let old = fixture();
        let mut new = old.clone();
        new.markers.push(openreel_core::Marker {
            id: openreel_core::MarkerId(1),
            position: TimeCode(50),
            label: "note".to_owned(),
            color_token: 0,
        });
        assert_eq!(changed_project_range(&old, &new), None);
    }

    #[test]
    fn ripple_delete_covers_the_seam_not_the_shifted_tail() {
        let old = fixture();
        let mut new = old.clone();
        Operation::RippleDeleteClip { clip: ClipId(2) }
            .apply(&mut new)
            .unwrap();
        // Clip 3 shifts 100..200 but is end-aligned identical; the seam is
        // where clip 2 was removed.
        let range = changed_project_range(&old, &new).unwrap();
        assert_eq!(range.start, TimeCode(100));
        assert_eq!(range.end, TimeCode(100));
    }

    #[test]
    fn an_effect_change_covers_exactly_that_clip() {
        let old = fixture();
        let mut new = old.clone();
        Operation::AddEffect {
            clip: ClipId(2),
            effect: openreel_core::Effect {
                id: openreel_core::EffectId(1),
                name: "brightness".to_owned(),
                parameters: [("percent".to_owned(), openreel_core::ParamValue::Integer(25))]
                    .into_iter()
                    .collect(),
            },
        }
        .apply(&mut new)
        .unwrap();
        let range = changed_project_range(&old, &new).unwrap();
        assert_eq!(range.start, TimeCode(100));
        assert_eq!(range.end, TimeCode(200));
    }

    #[test]
    fn a_middle_trim_covers_from_the_trim_to_the_last_moved_content() {
        let old = fixture();
        let mut new = old.clone();
        // Trim clip 2's tail by 40 source frames; ripple is not involved so
        // clip 3 stays put and only clip 2's span changes.
        Operation::TrimClip {
            clip: ClipId(2),
            new_source: TimeCode(100)..TimeCode(160),
        }
        .apply(&mut new)
        .unwrap();
        let range = changed_project_range(&old, &new).unwrap();
        assert_eq!(range.start, TimeCode(100));
        assert_eq!(range.end, TimeCode(160));
    }
}
