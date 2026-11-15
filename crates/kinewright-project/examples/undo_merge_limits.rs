//! Size qualification for the task -> candidate -> ordinary merge path.
use kinewright_core::{
    ClipId, Command, Operation, TimeCode,
    undo::{History, RetentionPolicy, RoleBounds, document_capacity_bytes},
};
use kinewright_project::{
    bundle_records::OccurrenceId,
    operation_history::{CreationOrigin, Lineage},
};

#[path = "../../kinewright-core/tests/support/merge_fixture.rs"]
mod fixture;
use fixture::{document, removals};

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
