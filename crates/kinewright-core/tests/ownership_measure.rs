//! On-demand measurement for the ownership fixes (H1): the cost of one edit in
//! a coalesced live gesture on a document with thousands of clips.
//!
//! Run with `cargo test -p kinewright-core --release --test ownership_measure
//! -- --ignored --nocapture`. It prints one `H1_EDIT` line with the wall time
//! per edit (request to `DocumentChanged`, so the actor's work is included).

use std::time::Instant;

use kinewright_core::{
    AssetId, AudioMix, BlendMode, Clip, ClipContent, ClipId, ColorContext, Command, Core, Document,
    Event, MediaAsset, MediaCatalog, MediaKind, MediaSourceFingerprint, Operation, Rational,
    TimeCode, Track, TrackId, TrackKind,
};

const TRACKS: u64 = 4;
const CLIPS_PER_TRACK: u64 = 750;
const CLIP_FRAMES: i64 = 30;
const EDITS: u64 = 300;

fn document() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let tracks = (1..=TRACKS)
        .map(|track| Track {
            id: TrackId(track),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: (0..CLIPS_PER_TRACK)
                .map(|index| Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(track * 10_000 + index + 1),
                    asset: AssetId(1),
                    source_range: TimeCode(0)..TimeCode(CLIP_FRAMES),
                    content: ClipContent::Media,
                    timeline_start: TimeCode(i64::try_from(index).unwrap() * CLIP_FRAMES),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                    blend_mode: BlendMode::Normal,
                })
                .collect(),
        })
        .collect();
    Document {
        investigator: None,
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks,
        media_pool: vec![MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from("measure.mp4"),
            name: "measure".to_owned(),
            duration: TimeCode(600),
            fps,
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        }],
        markers: Vec::new(),
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode(i64::try_from(CLIPS_PER_TRACK).unwrap() * CLIP_FRAMES),
    }
}

fn slip(frame: u64) -> Command {
    Command::DoBatchCoalesced {
        operations: vec![Operation::SlipClip {
            clip: ClipId(10_001 + CLIPS_PER_TRACK / 2),
            new_source_in: TimeCode(i64::try_from(frame % 500).unwrap()),
        }],
        coalesce_key: "measure-slip-drag".to_owned(),
    }
}

#[test]
#[ignore = "on-demand measurement"]
fn h1_coalesced_drag_edit_cost() {
    let core = Core::spawn(document()).unwrap();
    for frame in 0..20 {
        assert!(matches!(
            core.request(slip(frame)).unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let started = Instant::now();
    for frame in 20..20 + EDITS {
        assert!(matches!(
            core.request(slip(frame)).unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let elapsed = started.elapsed();
    println!(
        "H1_EDIT clips={} edits={EDITS} per_edit_us={:.1}",
        TRACKS * CLIPS_PER_TRACK,
        elapsed.as_secs_f64() * 1e6 / f64::from(u32::try_from(EDITS).unwrap()),
    );
}
