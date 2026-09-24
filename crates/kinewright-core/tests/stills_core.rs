//! MO1 R9 still/content integrity — core contract tests (carries reviewer-2's
//! `i_`/`j_` scenarios plus the per-op still pins).

use kinewright_core::{
    AssetId, AudioMix, ClipContent, ClipId, ColorContext, Document, MediaAsset, MediaCatalog,
    MediaKind, MediaSourceFingerprint, OpError, Operation, Rational, ThreePointMode, TimeCode,
    Title, Track, TrackId, TrackKind,
};

fn fps(n: u32) -> Rational {
    Rational::new(n, 1).unwrap()
}

fn asset(id: u64, kind: MediaKind, duration: i64, rate: Rational) -> MediaAsset {
    MediaAsset {
        id: AssetId(id),
        path: std::path::PathBuf::from(format!("a{id}.mp4")),
        name: format!("a{id}"),
        duration: TimeCode(duration),
        fps: rate,
        kind,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
        assumed_from: None,
    }
}

fn empty(project_fps: u32) -> Document {
    Document {
        investigator: None,
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps: fps(project_fps),
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    }
}

fn video_asset(doc: &mut Document) {
    Operation::AddAsset {
        asset: asset(1, MediaKind::Video, 600, fps(30)),
    }
    .apply(doc)
    .unwrap();
}

fn image_asset(doc: &mut Document) {
    Operation::AddAsset {
        asset: asset(2, MediaKind::Image, 1, Rational::default()),
    }
    .apply(doc)
    .unwrap();
}

fn media_clip(doc: &mut Document, at: i64, start: i64, end: i64) {
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(at),
        source: TimeCode(start)..TimeCode(end),
    }
    .apply(doc)
    .unwrap();
}

fn still(doc: &mut Document, at: i64, duration: i64) {
    Operation::AddFreezeFrame {
        track: TrackId(1),
        at: TimeCode(at),
        duration: TimeCode(duration),
        asset: AssetId(2),
        source_frame: TimeCode(0),
    }
    .apply(doc)
    .unwrap();
}

/// R9 (R2 `i_`): a Media range over a still is invalid — `add_clip` refuses
/// `Image` assets with the existing `InvalidSourceRange`.
#[test]
fn add_clip_over_image_is_refused() {
    let mut doc = empty(24);
    image_asset(&mut doc);
    let result = Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(1),
    }
    .apply(&mut doc);
    assert!(
        matches!(result, Err(OpError::MissingAsset(_))),
        "wrong asset still misses first, got {result:?}"
    );
    let result = Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(2),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(1),
    }
    .apply(&mut doc);
    assert!(
        matches!(result, Err(OpError::InvalidSourceRange { .. })),
        "R9 expects InvalidSourceRange, got {result:?}"
    );
}

/// R9: stills enter via `AddFreezeFrame` — any project-frame duration over
/// the single still frame, held at `source_frame: 0`.
#[test]
fn stills_enter_via_add_freeze_frame() {
    let mut doc = empty(24);
    image_asset(&mut doc);
    still(&mut doc, 0, 120);
    let clip = &doc.tracks[0].clips[0];
    assert!(matches!(
        clip.content,
        ClipContent::Freeze(ref frame) if frame.source_frame == TimeCode(0)
    ));
    assert_eq!(doc.clip_duration(clip).unwrap(), TimeCode(120));
    doc.validate().unwrap();
}

/// R9 (R2 `j_`): roll with a Freeze neighbour moves the shared edit point —
/// the media side maps through fps, the span side in project frames, and the
/// held frame never changes.
#[test]
fn roll_with_freeze_neighbour_moves_the_shared_point() {
    let mut doc = empty(30);
    video_asset(&mut doc);
    image_asset(&mut doc);
    media_clip(&mut doc, 0, 100, 160);
    still(&mut doc, 60, 60);

    Operation::RollEdit {
        left_clip: ClipId(1),
        right_clip: ClipId(2),
        to: TimeCode(50),
    }
    .apply(&mut doc)
    .expect("R9 roll with freeze neighbour");
    let left = &doc.tracks[0].clips[0];
    let right = &doc.tracks[0].clips[1];
    assert_eq!(
        left.timeline_start.0 + doc.clip_duration(left).unwrap().0,
        50
    );
    assert_eq!(right.timeline_start, TimeCode(50));
    assert_eq!(doc.clip_duration(right).unwrap(), TimeCode(70));
    assert!(matches!(
        right.content,
        ClipContent::Freeze(ref frame) if frame.source_frame == TimeCode(0)
    ));
    doc.validate().unwrap();
}

/// R9: slide moves the span window — a Freeze middle keeps its span while
/// the neighbours absorb the move, and a Freeze neighbour resizes in
/// project frames.
#[test]
fn slide_with_freeze_clips_moves_span_windows() {
    let mut doc = empty(30);
    video_asset(&mut doc);
    image_asset(&mut doc);
    media_clip(&mut doc, 0, 0, 60);
    still(&mut doc, 60, 60);
    media_clip(&mut doc, 120, 200, 260);

    Operation::SlideClip {
        clip: ClipId(2),
        to: TimeCode(45),
    }
    .apply(&mut doc)
    .expect("R9 slide of a freeze middle");
    let middle = &doc.tracks[0].clips[1];
    assert_eq!(middle.timeline_start, TimeCode(45));
    assert_eq!(middle.source_range.start, TimeCode(0));
    assert_eq!(middle.source_range.end, TimeCode(60));
    assert_eq!(doc.tracks[0].clips[0].source_range.end, TimeCode(45));
    assert_eq!(doc.tracks[0].clips[2].timeline_start, TimeCode(105));
    doc.validate().unwrap();
}

/// R9: Title neighbours roll/slide too — a behaviour change from
/// `EditorialRequiresMedia`, with a CHANGELOG line.
#[test]
fn roll_and_slide_accept_title_neighbours() {
    let mut doc = empty(30);
    video_asset(&mut doc);
    media_clip(&mut doc, 0, 0, 60);
    Operation::AddTitle {
        track: TrackId(1),
        at: TimeCode(60),
        duration: TimeCode(60),
        title: Title::default(),
    }
    .apply(&mut doc)
    .unwrap();

    Operation::RollEdit {
        left_clip: ClipId(1),
        right_clip: ClipId(2),
        to: TimeCode(50),
    }
    .apply(&mut doc)
    .expect("R9 roll onto a title neighbour");
    assert_eq!(doc.tracks[0].clips[1].timeline_start, TimeCode(50));
    assert_eq!(doc.tracks[0].clips[1].source_range.start, TimeCode(0));
    assert_eq!(
        doc.tracks[0].clips[1].source_range.end,
        TimeCode(70),
        "the span origin never moves; the window resizes around it"
    );
    doc.validate().unwrap();
}

/// R9: trim and split ride the freeze paths — spans resize, the held frame
/// stays put.
#[test]
fn trim_and_split_ride_freeze_spans() {
    let mut doc = empty(30);
    image_asset(&mut doc);
    still(&mut doc, 0, 60);

    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(10)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.tracks[0].clips[0].timeline_start, TimeCode(10));
    let head = doc.tracks[0].clips[0].clone();
    assert_eq!(doc.clip_duration(&head).unwrap(), TimeCode(50));

    Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.tracks[0].clips.len(), 2);
    for clip in &doc.tracks[0].clips {
        assert!(matches!(
            clip.content,
            ClipContent::Freeze(ref frame) if frame.source_frame == TimeCode(0)
        ));
    }
    doc.validate().unwrap();
}

/// R9: speed and audio fail on the existing Freeze arms; replace/fit still
/// require Media (replace a still by delete+add).
#[test]
fn speed_audio_and_replace_refuse_stills() {
    let mut doc = empty(30);
    video_asset(&mut doc);
    image_asset(&mut doc);
    still(&mut doc, 0, 60);

    let error = Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 200,
    }
    .apply(&mut doc)
    .unwrap_err();
    assert_eq!(error, OpError::SpeedOnNonMediaClip(ClipId(1)));

    let error = Operation::SetClipAudio {
        clip: ClipId(1),
        gain_tenth_db: 0,
        fade_in_frames: TimeCode::ZERO,
        fade_out_frames: TimeCode::ZERO,
    }
    .apply(&mut doc)
    .unwrap_err();
    assert_eq!(error, OpError::FreezeClipHasNoAudio(ClipId(1)));

    let error = Operation::ReplaceClip {
        clip: ClipId(1),
        asset: AssetId(1),
        source: TimeCode(0)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap_err();
    assert_eq!(error, OpError::EditorialRequiresMedia(ClipId(1)));
    let error = Operation::FitToFill {
        clip: ClipId(1),
        asset: AssetId(1),
        source: TimeCode(0)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap_err();
    assert_eq!(error, OpError::EditorialRequiresMedia(ClipId(1)));
}

/// R9: ripple moves stills with their tracks; three-point splits them like
/// any clip.
#[test]
fn ripple_moves_and_three_point_splits_stills() {
    let mut doc = empty(30);
    video_asset(&mut doc);
    image_asset(&mut doc);
    media_clip(&mut doc, 0, 0, 60);
    still(&mut doc, 60, 60);

    Operation::RippleDeleteClip { clip: ClipId(1) }
        .apply(&mut doc)
        .unwrap();
    assert_eq!(doc.tracks[0].clips[0].timeline_start, TimeCode(0));
    assert!(matches!(
        doc.tracks[0].clips[0].content,
        ClipContent::Freeze(_)
    ));

    let mut doc = empty(30);
    video_asset(&mut doc);
    image_asset(&mut doc);
    still(&mut doc, 0, 120);
    Operation::ThreePointEdit {
        track: TrackId(1),
        asset: AssetId(1),
        source_in: Some(TimeCode(0)),
        source_out: Some(TimeCode(30)),
        timeline_in: Some(TimeCode(60)),
        timeline_out: None,
        mode: ThreePointMode::Insert,
    }
    .apply(&mut doc)
    .unwrap();
    assert!(
        doc.tracks[0]
            .clips
            .iter()
            .any(|clip| matches!(clip.content, ClipContent::Freeze(_))
                && clip.source_range.start == TimeCode(60)),
        "the still survives split as a Freeze half"
    );
    doc.validate().unwrap();
}
