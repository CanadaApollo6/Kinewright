//! The fixed merge-sizing fixture, shared by the measurement and admission tests.
use kinewright_core::{ClipId, Document, Operation, Rational, TimeCode, Track, TrackId, TrackKind};
pub fn document(clips: u64, tracks: u64, clip_frames: i64) -> Document {
    let per_track = clips / tracks;
    Document {
        fps: Rational::new(30, 1).unwrap(),
        duration: TimeCode(i64::try_from(per_track).unwrap() * clip_frames),
        tracks: (1..=tracks)
            .map(|track| Track {
                id: TrackId(track),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: (0..per_track)
                    .map(|clip| {
                        serde_json::from_value(serde_json::json!({
                            "id": (track - 1) * per_track + clip + 1, "asset": 0,
                            "source_range": {"start": 0, "end": clip_frames},
                            "content": {"solid": {"r": 20, "g": 40, "b": 60}},
                            "timeline_start": i64::try_from(clip).unwrap() * clip_frames,
                            "effects": [], "transition_in": null
                        }))
                        .unwrap()
                    })
                    .collect(),
            })
            .collect(),
        ..Document::default()
    }
}

// Same cut shape as transcript_edit::append_project_cut: split the primary,
// split/delete the locked secondaries, and ripple the primary exactly once.
pub fn removals(tracks: u64, count: u64) -> Vec<Operation> {
    let mut operations = Vec::new();
    for cut in 0..count {
        let start = TimeCode(i64::try_from((cut + 1) * 60 - cut * 6).unwrap());
        let end = TimeCode(start.0 + 6);
        let next = tracks + cut * tracks * 2;
        for track in 1..=tracks {
            let tail = if cut == 0 {
                track
            } else {
                tracks + (cut - 1) * tracks * 2 + track * 2
            };
            operations.push(Operation::SplitClip {
                clip: ClipId(tail),
                at: start,
            });
            operations.push(Operation::SplitClip {
                clip: ClipId(next + track * 2 - 1),
                at: end,
            });
            if track != 1 {
                operations.push(Operation::DeleteClip {
                    clip: ClipId(next + track * 2 - 1),
                });
            }
        }
        operations.push(Operation::RippleDeleteClip {
            clip: ClipId(next + 1),
        });
    }
    operations
}
