//! Size qualification for the task -> candidate -> ordinary merge path.
use kinewright_core::{
    ClipId, Command, Document, Operation, Rational, TimeCode, Track, TrackId, TrackKind,
    undo::{History, RetentionPolicy, RoleBounds, document_capacity_bytes},
};
use kinewright_project::{
    bundle_records::OccurrenceId,
    operation_history::{CreationOrigin, Lineage},
};

fn document(clips: u64, tracks: u64, clip_frames: i64) -> Document {
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
fn removals(tracks: u64, count: u64) -> Vec<Operation> {
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

fn measure_documents() {
    let origin = CreationOrigin {
        branch: serde_json::from_str("\"11111111111111111111111111111111\"").unwrap(),
        batch: serde_json::from_str("\"22222222222222222222222222222222\"").unwrap(),
    };
    for clips in [1_000, 5_000, 20_000] {
        let doc = document(clips, 8, 90);
        doc.validate().unwrap();
        let seeds = Lineage::seed(&doc, &origin);
        let seed_encoded = serde_json::to_vec(&seeds).unwrap().len();
        // The current NeverFold owner accounting charges one eleven-slot node
        // per occurrence plus the two 32-byte IDs and their allocator headers.
        let node = (11 * std::mem::size_of::<(ClipId, OccurrenceId)>() + 128) as u64;
        let seed_capacity = 4096 + (clips + 1) * node + clips * 96;
        let owner_charge = (u64::try_from(seed_encoded).unwrap() * 4).max(seed_capacity) + 4096;
        println!(
            "DOCUMENT clips={clips} tracks=8 encoded={} live={} seed_encoded={seed_encoded} owner_charge={owner_charge}",
            serde_json::to_vec(&doc).unwrap().len(),
            document_capacity_bytes(&doc).unwrap()
        );
    }
}

fn measure_dialogue(bounds: RoleBounds) {
    for tracks in [1, 4] {
        for count in [1, 32] {
            let operations = removals(tracks, count);
            let wire = serde_json::to_vec(&operations).unwrap().len();
            let mut history = History::new(
                document(tracks, tracks, 120_120),
                RetentionPolicy::NeverFold,
                bounds,
            )
            .unwrap();
            history.execute(Command::DoBatch(operations)).unwrap();
            println!(
                "DIALOGUE tracks={tracks} removals={count} wire={wire} encoded={} decoded={} document_encoded={} document_live={}",
                history.charge().encoded,
                history.charge().decoded,
                serde_json::to_vec(history.document()).unwrap().len(),
                document_capacity_bytes(history.document()).unwrap()
            );
        }
    }
    let mut task =
        History::new(document(4, 4, 120_120), RetentionPolicy::NeverFold, bounds).unwrap();
    let operations = removals(4, 2_000);
    let mut removals = operations.into_iter();
    for cut in 0..2_000 {
        let command = removals.by_ref().take(12).collect();
        task.execute(Command::DoBatch(command))
            .unwrap_or_else(|error| panic!("cut {cut}: {error:?}"));
        if (cut + 1) % 500 == 0 {
            eprintln!("DIALOGUE_PROGRESS removals={}", cut + 1);
        }
    }
    println!(
        "DIALOGUE_STREAM tracks=4 removals=2000 encoded={} decoded={} document_encoded={} document_live={}",
        task.charge().encoded,
        task.charge().decoded,
        serde_json::to_vec(task.document()).unwrap().len(),
        document_capacity_bytes(task.document()).unwrap()
    );
    let mut candidate =
        History::new(document(4, 4, 120_120), RetentionPolicy::NeverFold, bounds).unwrap();
    let delta = task.applied_delta().cloned().collect(); // Independent merge destination owns its payload.
    candidate.execute(Command::DoBatch(delta)).unwrap();
    println!(
        "DIALOGUE_MERGE tracks=4 removals=2000 encoded={} decoded={}",
        candidate.charge().encoded,
        candidate.charge().decoded
    );
}

fn measure_large_cut(bounds: RoleBounds) {
    let mut doc = document(20_000, 8, 90);
    for (index, track) in doc.tracks.iter_mut().enumerate() {
        track.sync_lock = index < 4;
        if track.sync_lock {
            track.clips[0].source_range.end = TimeCode(120_120);
            for clip in track.clips.iter_mut().skip(1) {
                clip.timeline_start.0 += 120_030;
            }
        }
    }
    doc.duration.0 += 120_030;
    let mut operations = Vec::new();
    for track in 0..4 {
        operations.push(Operation::SplitClip {
            clip: ClipId(track * 2500 + 1),
            at: TimeCode(60),
        });
        let middle = ClipId(20_001 + track * 2);
        operations.push(Operation::SplitClip {
            clip: middle,
            at: TimeCode(66),
        });
        if track != 0 {
            operations.push(Operation::DeleteClip { clip: middle });
        }
    }
    operations.push(Operation::RippleDeleteClip {
        clip: ClipId(20_001),
    });
    let mut history = History::new(doc, RetentionPolicy::NeverFold, bounds).unwrap();
    history.execute(Command::DoBatch(operations)).unwrap();
    println!(
        "DOCUMENT_AFTER_DIALOGUE initial_clips=20000 tracks=8 locked=4 clips={} encoded={} live={}",
        history
            .document()
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>(),
        serde_json::to_vec(history.document()).unwrap().len(),
        document_capacity_bytes(history.document()).unwrap()
    );
}

fn main() {
    println!(
        "LAYOUT clip={} operation={} step={} command={}",
        std::mem::size_of::<kinewright_core::Clip>(),
        std::mem::size_of::<Operation>(),
        std::mem::size_of::<kinewright_core::undo::Step>(),
        std::mem::size_of::<kinewright_core::undo::StepCommand>()
    );
    let bounds = RoleBounds {
        operation_bytes: 64 << 20,
        step_cap: 16_384,
        checkpoint_documents: 0,
        replay_workspace: 256 << 20,
        document_live_bytes: 8 << 20,
        document_bytes: 4 << 20,
        decoded_history_bytes: 256 << 20,
    };
    measure_documents();
    measure_large_cut(bounds);
    measure_dialogue(bounds);
}
