use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kinewright_core::{
    AssetId, AudioBus, AudioBusId, AutomationCurve, BinId, Clip, ClipContent, ClipId, Command,
    Core, Document, Effect, EffectId, Event, FreezeFrame, Keyframe, KeyframeInterpolation, LinkId,
    Marker, MarkerId, MediaAsset, MediaBin, MediaKind, OpError, Operation, ParamValue, Rational,
    SourceSelect, StringOut, StringOutId, SyncGroup, SyncGroupId, SyncGroupMember,
    TRANSITION_DESCRIPTORS, ThreePointMode, TimeCode, Title, TitlePosition, Track, TrackId,
    TrackKind, Transition, transition_descriptor,
};
use proptest::prelude::*;

fn asset(id: u64, fps: Rational, duration: i64) -> MediaAsset {
    MediaAsset {
        id: AssetId(id),
        path: PathBuf::from(format!("asset-{id}.mp4")),
        name: format!("asset-{id}"),
        duration: TimeCode(duration),
        fps,
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
    }
}

fn empty_timeline(fps: Rational) -> Document {
    Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    }
}

fn document_with_one_clip() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddAsset {
        asset: asset(1, fps, 300),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(10),
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

fn document_with_three_clips() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddAsset {
        asset: asset(1, fps, 300),
    }
    .apply(&mut doc)
    .unwrap();
    for (at, source) in [
        (0, TimeCode(0)..TimeCode(10)),
        (20, TimeCode(10)..TimeCode(30)),
        (50, TimeCode(30)..TimeCode(40)),
    ] {
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(at),
            source,
        }
        .apply(&mut doc)
        .unwrap();
    }
    doc
}

fn document_with_butt_joined_clips() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddAsset {
        asset: asset(1, fps, 600),
    }
    .apply(&mut doc)
    .unwrap();
    for (at, source) in [
        (0, TimeCode(50)..TimeCode(100)),
        (50, TimeCode(150)..TimeCode(180)),
        (80, TimeCode(220)..TimeCode(270)),
    ] {
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(at),
            source,
        }
        .apply(&mut doc)
        .unwrap();
    }
    doc
}

#[test]
#[allow(clippy::too_many_lines)]
fn document_and_every_operation_variant_round_trip_through_json() {
    let doc = document_with_one_clip();
    let encoded = serde_json::to_string(&doc).unwrap();
    let decoded: Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, doc);

    let operations = vec![
        Operation::AddAsset {
            asset: asset(2, Rational::new(24_000, 1_001).unwrap(), 240),
        },
        Operation::UpsertBin {
            bin: MediaBin {
                id: BinId(1),
                name: "Ceremony".to_owned(),
                parent: None,
                assets: vec![AssetId(1)],
            },
        },
        Operation::RemoveBin { bin: BinId(1) },
        Operation::SetAssetBin {
            asset: AssetId(1),
            bin: Some(BinId(1)),
        },
        Operation::UpsertStringOut {
            string_out: StringOut {
                id: StringOutId(1),
                name: "Vows".to_owned(),
                selects: vec![SourceSelect {
                    asset: AssetId(1),
                    source: TimeCode(10)..TimeCode(20),
                    label: "Promise".to_owned(),
                }],
            },
        },
        Operation::RemoveStringOut {
            string_out: StringOutId(1),
        },
        Operation::UpsertSyncGroup {
            sync_group: SyncGroup {
                id: SyncGroupId(1),
                name: "Ceremony angles".to_owned(),
                members: vec![
                    SyncGroupMember {
                        asset: AssetId(1),
                        offset: TimeCode::ZERO,
                        angle_name: "Wide".to_owned(),
                    },
                    SyncGroupMember {
                        asset: AssetId(2),
                        offset: TimeCode(3),
                        angle_name: "Close".to_owned(),
                    },
                ],
            },
        },
        Operation::RemoveSyncGroup {
            sync_group: SyncGroupId(1),
        },
        Operation::UpsertAudioBus {
            bus: AudioBus {
                id: AudioBusId(1),
                name: "Dialogue".to_owned(),
                tracks: vec![TrackId(1)],
                effects: vec![Effect {
                    id: EffectId(1),
                    name: "audio_gain".to_owned(),
                    parameters: BTreeMap::from([(
                        "gain_tenth_db".to_owned(),
                        ParamValue::Integer(-30),
                    )]),
                    keyframes: BTreeMap::new(),
                }],
                ducking_sidechain_tracks: Vec::new(),
            },
        },
        Operation::RemoveAudioBus { bus: AudioBusId(1) },
        Operation::AddTrack {
            track: Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            },
        },
        Operation::RemoveTrack { track: TrackId(2) },
        Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(100),
            source: TimeCode(5)..TimeCode(25),
        },
        Operation::AddTitle {
            track: TrackId(1),
            at: TimeCode(100),
            duration: TimeCode(90),
            title: Title::default(),
        },
        Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(25),
        },
        Operation::TrimClip {
            clip: ClipId(1),
            new_source: TimeCode(1)..TimeCode(20),
        },
        Operation::MoveClip {
            clip: ClipId(1),
            to_track: TrackId(1),
            to: TimeCode(60),
        },
        Operation::ThreePointEdit {
            track: TrackId(1),
            asset: AssetId(1),
            source_in: Some(TimeCode(10)),
            source_out: Some(TimeCode(40)),
            timeline_in: Some(TimeCode(60)),
            timeline_out: None,
            mode: ThreePointMode::Insert,
        },
        Operation::SlipClip {
            clip: ClipId(1),
            new_source_in: TimeCode(10),
        },
        Operation::RollEdit {
            left_clip: ClipId(1),
            right_clip: ClipId(2),
            to: TimeCode(30),
        },
        Operation::SlideClip {
            clip: ClipId(2),
            to: TimeCode(40),
        },
        Operation::ReplaceClip {
            clip: ClipId(1),
            asset: AssetId(1),
            source: TimeCode(20)..TimeCode(50),
        },
        Operation::FitToFill {
            clip: ClipId(1),
            asset: AssetId(1),
            source: TimeCode(20)..TimeCode(80),
        },
        Operation::DeleteClip { clip: ClipId(1) },
        Operation::RippleDeleteClip { clip: ClipId(1) },
        Operation::RippleInsertGap {
            track: TrackId(1),
            at: TimeCode(90),
            duration: TimeCode(15),
        },
        Operation::LinkClips {
            clips: vec![ClipId(1), ClipId(2)],
        },
        Operation::UnlinkClips {
            clips: vec![ClipId(1)],
        },
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(1),
                position: TimeCode(12),
                label: "Review".to_owned(),
                color_token: 0,
            },
        },
        Operation::RemoveMarker {
            marker: MarkerId(1),
        },
        Operation::MoveMarker {
            marker: MarkerId(1),
            to: TimeCode(24),
        },
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(1),
                name: "brightness".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
        },
        Operation::RemoveEffect {
            clip: ClipId(1),
            effect: EffectId(1),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "percent".to_owned(),
            value: ParamValue::Integer(25),
        },
        Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "percent".to_owned(),
            curve: AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value: 25,
                    interpolation: KeyframeInterpolation::Linear,
                }],
            },
        },
        Operation::ClearEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "percent".to_owned(),
        },
        Operation::SetTitleParam {
            clip: ClipId(1),
            name: "text".to_owned(),
            value: ParamValue::Text("Chapter one".to_owned()),
        },
        Operation::SetClipAudio {
            clip: ClipId(1),
            gain_tenth_db: -60,
            fade_in_frames: TimeCode(6),
            fade_out_frames: TimeCode(9),
        },
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "crossfade".to_owned(),
                duration: TimeCode(10),
            },
        },
        Operation::RemoveTransition { clip: ClipId(1) },
        Operation::SetMarkerParam {
            marker: MarkerId(1),
            name: "label".to_owned(),
            value: ParamValue::Text("Review this".to_owned()),
        },
        Operation::AddFreezeFrame {
            track: TrackId(1),
            at: TimeCode(100),
            duration: TimeCode(60),
            asset: AssetId(1),
            source_frame: TimeCode(24),
        },
        Operation::SetClipSpeed {
            clip: ClipId(1),
            speed_percent: 200,
        },
    ];

    for operation in operations {
        let encoded = serde_json::to_string(&operation).unwrap();
        let decoded: Operation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, operation);
    }
}

#[test]
fn pre_m15_project_json_defaults_additive_fields_without_changing_legacy_shape() {
    let document: Document =
        serde_json::from_str(include_str!("fixtures/pre_m13_project.json")).unwrap();
    document.validate().unwrap();
    assert!(document.markers.is_empty());
    assert!(document.tracks[0].sync_lock);
    assert_eq!(document.clip(ClipId(1)).unwrap().link, None);
    assert_eq!(document.clip(ClipId(1)).unwrap().audio_gain_tenth_db, 0);
    assert_eq!(
        document.clip(ClipId(1)).unwrap().audio_fade_in_frames,
        TimeCode::ZERO
    );
    assert_eq!(
        document.clip(ClipId(1)).unwrap().audio_fade_out_frames,
        TimeCode::ZERO
    );
    assert_eq!(
        document.clip(ClipId(1)).unwrap().content,
        ClipContent::Media
    );

    let encoded = serde_json::to_string(&document).unwrap();
    assert!(!encoded.contains("\"markers\""));
    assert!(!encoded.contains("\"sync_lock\""));
    assert!(!encoded.contains("\"link\""));
    assert!(!encoded.contains("\"content\""));
    assert!(!encoded.contains("\"audio_gain_tenth_db\""));
    assert!(!encoded.contains("\"audio_fade_in_frames\""));
    assert!(!encoded.contains("\"audio_fade_out_frames\""));

    let mut unlocked = document;
    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }
    .apply(&mut unlocked)
    .unwrap();
    let unlocked_json = serde_json::to_string(&unlocked).unwrap();
    assert!(unlocked_json.contains("\"sync_lock\":false"));
    assert_eq!(
        serde_json::from_str::<Document>(&unlocked_json).unwrap(),
        unlocked
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn title_clips_reuse_move_trim_split_ripple_link_and_undo_contracts() {
    let fps = Rational::new(30, 1).unwrap();
    let mut document = empty_timeline(fps);
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddTitle {
        track: TrackId(1),
        at: TimeCode(30),
        duration: TimeCode(90),
        title: Title {
            text: "Lower third".to_owned(),
            position: TitlePosition::LowerThird,
            fade_in_frames: TimeCode(10),
            fade_out_frames: TimeCode(12),
            ..Title::default()
        },
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(document.duration, TimeCode(120));
    assert!(matches!(
        document.clip(ClipId(1)).unwrap().content,
        ClipContent::Title(_)
    ));

    Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(75),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(
        document
            .clip_duration(document.clip(ClipId(1)).unwrap())
            .unwrap(),
        TimeCode(45)
    );
    assert_eq!(
        document
            .clip_duration(document.clip(ClipId(2)).unwrap())
            .unwrap(),
        TimeCode(45)
    );
    let ClipContent::Title(left) = &document.clip(ClipId(1)).unwrap().content else {
        panic!("left split must remain a title");
    };
    let ClipContent::Title(right) = &document.clip(ClipId(2)).unwrap().content else {
        panic!("right split must remain a title");
    };
    assert_eq!(left.fade_out_frames, TimeCode::ZERO);
    assert_eq!(right.fade_in_frames, TimeCode::ZERO);

    Operation::MoveClip {
        clip: ClipId(2),
        to_track: TrackId(2),
        to: TimeCode(90),
    }
    .apply(&mut document)
    .unwrap();
    Operation::TrimClip {
        clip: ClipId(2),
        new_source: TimeCode(50)..TimeCode(80),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(
        document.clip(ClipId(2)).unwrap().timeline_start,
        TimeCode(95)
    );
    Operation::LinkClips {
        clips: vec![ClipId(1), ClipId(2)],
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(
        document.clip(ClipId(1)).unwrap().link,
        document.clip(ClipId(2)).unwrap().link
    );
    Operation::RippleDeleteClip { clip: ClipId(1) }
        .apply(&mut document)
        .unwrap();
    assert!(document.clip(ClipId(1)).is_none());

    let core = Core::spawn(document.clone()).unwrap();
    core.request(Command::Do(Operation::SetTitleParam {
        clip: ClipId(2),
        name: "text".to_owned(),
        value: ParamValue::Text("Updated".to_owned()),
    }))
    .unwrap();
    let undone = match core.request(Command::Undo).unwrap() {
        Event::DocumentChanged { doc, .. } => doc,
        other => panic!("unexpected event: {other:?}"),
    };
    let ClipContent::Title(title) = &undone.clip(ClipId(2)).unwrap().content else {
        panic!("clip must remain a title");
    };
    assert_eq!(title.text, "Lower third");
}

#[test]
fn add_freeze_frame_validates_track_asset_frame_and_duration() {
    let fps = Rational::new(30, 1).unwrap();
    let mut document = empty_timeline(fps);
    document.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: Vec::new(),
    });
    document.media_pool.extend([
        asset(1, fps, 120),
        MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("audio.wav"),
            name: "audio".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::Audio,
            resolution: None,
        },
    ]);
    document.validate().unwrap();

    let operation = |track, asset, duration, source_frame| Operation::AddFreezeFrame {
        track,
        at: TimeCode(10),
        duration,
        asset,
        source_frame,
    };
    for (operation, expected) in [
        (
            operation(TrackId(2), AssetId(1), TimeCode(30), TimeCode(10)),
            OpError::FreezeOnAudioTrack(TrackId(2)),
        ),
        (
            operation(TrackId(1), AssetId(99), TimeCode(30), TimeCode(10)),
            OpError::MissingAsset(AssetId(99)),
        ),
        (
            operation(TrackId(1), AssetId(2), TimeCode(30), TimeCode(10)),
            OpError::IncompatibleTrack {
                asset: AssetId(2),
                track: "video",
                track_id: TrackId(1),
            },
        ),
        (
            operation(TrackId(1), AssetId(1), TimeCode(30), TimeCode(-1)),
            OpError::FreezeSourceFrameOutOfRange {
                asset: AssetId(1),
                source_frame: TimeCode(-1),
                duration: TimeCode(120),
            },
        ),
        (
            operation(TrackId(1), AssetId(1), TimeCode(30), TimeCode(120)),
            OpError::FreezeSourceFrameOutOfRange {
                asset: AssetId(1),
                source_frame: TimeCode(120),
                duration: TimeCode(120),
            },
        ),
        (
            operation(TrackId(1), AssetId(1), TimeCode::ZERO, TimeCode(10)),
            OpError::InvalidFreezeDuration(TimeCode::ZERO),
        ),
    ] {
        let mut candidate = document.clone();
        assert_eq!(operation.apply(&mut candidate).unwrap_err(), expected);
        assert_eq!(candidate, document);
    }
    let mut candidate = document.clone();
    assert_eq!(
        Operation::AddFreezeFrame {
            track: TrackId(1),
            at: TimeCode(-1),
            duration: TimeCode(30),
            asset: AssetId(1),
            source_frame: TimeCode(10),
        }
        .apply(&mut candidate)
        .unwrap_err(),
        OpError::NegativeTimelinePosition(TimeCode(-1))
    );
    assert_eq!(candidate, document);

    Operation::AddFreezeFrame {
        track: TrackId(1),
        at: TimeCode(10),
        duration: TimeCode(30),
        asset: AssetId(1),
        source_frame: TimeCode(119),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(document.duration, TimeCode(40));
}

#[test]
fn freeze_content_round_trips_with_freeze_tag_and_hand_edits_are_validated() {
    let fps = Rational::new(30, 1).unwrap();
    let mut document = empty_timeline(fps);
    document.media_pool.push(asset(1, fps, 120));
    Operation::AddFreezeFrame {
        track: TrackId(1),
        at: TimeCode(5),
        duration: TimeCode(30),
        asset: AssetId(1),
        source_frame: TimeCode(42),
    }
    .apply(&mut document)
    .unwrap();

    let encoded = serde_json::to_string(&document).unwrap();
    assert!(encoded.contains("\"content\":{\"freeze\""));
    let loaded: Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(loaded, document);

    let mut hand_edited = serde_json::to_value(&document).unwrap();
    hand_edited["tracks"][0]["clips"][0]["content"]["freeze"]["source_frame"] =
        serde_json::json!(120);
    let invalid: Document = serde_json::from_value(hand_edited).unwrap();
    assert!(matches!(
        invalid.validate(),
        Err(OpError::FreezeSourceFrameOutOfRange { .. })
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn freeze_clips_reuse_move_trim_split_ripple_link_and_undo_contracts() {
    let fps = Rational::new(30, 1).unwrap();
    let mut document = empty_timeline(fps);
    document.media_pool.push(asset(1, fps, 300));
    document.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: Vec::new(),
    });
    Operation::AddFreezeFrame {
        track: TrackId(1),
        at: TimeCode(30),
        duration: TimeCode(90),
        asset: AssetId(1),
        source_frame: TimeCode(77),
    }
    .apply(&mut document)
    .unwrap();
    Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(75),
    }
    .apply(&mut document)
    .unwrap();
    for id in [ClipId(1), ClipId(2)] {
        let clip = document.clip(id).unwrap();
        assert_eq!(document.clip_duration(clip).unwrap(), TimeCode(45));
        assert!(matches!(
            clip.content,
            ClipContent::Freeze(FreezeFrame {
                source_frame: TimeCode(77)
            })
        ));
    }
    Operation::MoveClip {
        clip: ClipId(2),
        to_track: TrackId(2),
        to: TimeCode(90),
    }
    .apply(&mut document)
    .unwrap();
    Operation::TrimClip {
        clip: ClipId(2),
        new_source: TimeCode(50)..TimeCode(80),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(
        document.clip(ClipId(2)).unwrap().timeline_start,
        TimeCode(95)
    );
    Operation::LinkClips {
        clips: vec![ClipId(1), ClipId(2)],
    }
    .apply(&mut document)
    .unwrap();
    Operation::RippleInsertGap {
        track: TrackId(1),
        at: TimeCode(30),
        duration: TimeCode(10),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(
        document.clip(ClipId(1)).unwrap().timeline_start,
        TimeCode(40)
    );
    assert_eq!(
        document.clip(ClipId(2)).unwrap().timeline_start,
        TimeCode(105)
    );

    let core = Core::spawn(document.clone()).unwrap();
    core.request(Command::Do(Operation::MoveClip {
        clip: ClipId(2),
        to_track: TrackId(2),
        to: TimeCode(120),
    }))
    .unwrap();
    let undone = match core.request(Command::Undo).unwrap() {
        Event::DocumentChanged { doc, .. } => doc,
        other => panic!("unexpected event: {other:?}"),
    };
    assert_eq!(undone.as_ref(), &document);

    assert_eq!(
        Operation::SetClipAudio {
            clip: ClipId(1),
            gain_tenth_db: 0,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
        }
        .apply(&mut document)
        .unwrap_err(),
        OpError::FreezeClipHasNoAudio(ClipId(1))
    );
    assert_eq!(
        Operation::SetTitleParam {
            clip: ClipId(1),
            name: "text".to_owned(),
            value: ParamValue::Text("no".to_owned()),
        }
        .apply(&mut document)
        .unwrap_err(),
        OpError::NotTitleClip(ClipId(1))
    );
}

#[test]
fn title_and_marker_parameter_validation_is_atomic() {
    let fps = Rational::new(30, 1).unwrap();
    let mut document = empty_timeline(fps);
    Operation::AddTitle {
        track: TrackId(1),
        at: TimeCode::ZERO,
        duration: TimeCode(30),
        title: Title::default(),
    }
    .apply(&mut document)
    .unwrap();
    let before = document.clone();
    let error = Operation::SetTitleParam {
        clip: ClipId(1),
        name: "fade_in_frames".to_owned(),
        value: ParamValue::Integer(31),
    }
    .apply(&mut document)
    .unwrap_err();
    assert!(matches!(error, OpError::TitleFadeTooLong { .. }));
    assert_eq!(document, before);

    Operation::AddMarker {
        marker: Marker {
            id: MarkerId(1),
            position: TimeCode(5),
            label: "Review".to_owned(),
            color_token: 0,
        },
    }
    .apply(&mut document)
    .unwrap();
    Operation::SetMarkerParam {
        marker: MarkerId(1),
        name: "color_token".to_owned(),
        value: ParamValue::Integer(3),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(document.marker(MarkerId(1)).unwrap().color_token, 3);
}

#[test]
#[allow(clippy::too_many_lines)]
fn ripple_operations_shift_all_markers_independently_of_sync_locks() {
    let mut document = document_with_three_clips();
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(2),
        asset: AssetId(1),
        at: TimeCode(25),
        source: TimeCode(70)..TimeCode(80),
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(2),
        asset: AssetId(1),
        at: TimeCode(35),
        source: TimeCode(80)..TimeCode(90),
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(2),
        asset: AssetId(1),
        at: TimeCode(70),
        source: TimeCode(90)..TimeCode(100),
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddTrack {
        track: Track {
            id: TrackId(3),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut document)
    .unwrap();
    Operation::SetTrackSyncLock {
        track: TrackId(3),
        locked: false,
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(3),
        asset: AssetId(1),
        at: TimeCode(70),
        source: TimeCode(100)..TimeCode(110),
    }
    .apply(&mut document)
    .unwrap();
    for (id, position) in [(1, 30), (2, 39), (3, 40), (4, 70)] {
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(id),
                position: TimeCode(position),
                label: format!("Marker {id}"),
                color_token: 0,
            },
        }
        .apply(&mut document)
        .unwrap();
    }

    Operation::RippleDeleteClip { clip: ClipId(2) }
        .apply(&mut document)
        .unwrap();
    assert_eq!(document.tracks[0].clips[1].id, ClipId(3));
    assert_eq!(document.tracks[0].clips[1].timeline_start, TimeCode(30));
    assert_eq!(
        document.clip(ClipId(4)).unwrap().timeline_start,
        TimeCode(25)
    );
    assert_eq!(
        document.clip(ClipId(5)).unwrap().timeline_start,
        TimeCode(35)
    );
    assert_eq!(
        document.clip(ClipId(6)).unwrap().timeline_start,
        TimeCode(50)
    );
    assert_eq!(
        document.clip(ClipId(7)).unwrap().timeline_start,
        TimeCode(70)
    );
    assert_eq!(document.marker(MarkerId(1)).unwrap().position, TimeCode(30));
    assert_eq!(document.marker(MarkerId(2)).unwrap().position, TimeCode(39));
    assert_eq!(document.marker(MarkerId(3)).unwrap().position, TimeCode(20));
    assert_eq!(document.marker(MarkerId(4)).unwrap().position, TimeCode(50));

    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }
    .apply(&mut document)
    .unwrap();
    Operation::RippleInsertGap {
        track: TrackId(1),
        at: TimeCode(30),
        duration: TimeCode(7),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(document.tracks[0].clips[1].timeline_start, TimeCode(37));
    assert_eq!(
        document.clip(ClipId(4)).unwrap().timeline_start,
        TimeCode(25)
    );
    assert_eq!(
        document.clip(ClipId(5)).unwrap().timeline_start,
        TimeCode(42)
    );
    assert_eq!(
        document.clip(ClipId(6)).unwrap().timeline_start,
        TimeCode(57)
    );
    assert_eq!(
        document.clip(ClipId(7)).unwrap().timeline_start,
        TimeCode(70)
    );
    assert_eq!(document.marker(MarkerId(1)).unwrap().position, TimeCode(37));
    assert_eq!(document.marker(MarkerId(2)).unwrap().position, TimeCode(46));
    assert_eq!(document.marker(MarkerId(3)).unwrap().position, TimeCode(20));
    assert_eq!(document.marker(MarkerId(4)).unwrap().position, TimeCode(57));

    for invalid in [
        Operation::RippleDeleteClip { clip: ClipId(99) },
        Operation::RippleInsertGap {
            track: TrackId(99),
            at: TimeCode(0),
            duration: TimeCode(1),
        },
        Operation::RippleInsertGap {
            track: TrackId(1),
            at: TimeCode(0),
            duration: TimeCode(0),
        },
        Operation::RippleInsertGap {
            track: TrackId(1),
            at: TimeCode(-1),
            duration: TimeCode(1),
        },
        Operation::SetTrackSyncLock {
            track: TrackId(99),
            locked: false,
        },
    ] {
        let before = document.clone();
        assert!(invalid.apply(&mut document).is_err());
        assert_eq!(document, before);
    }
}

#[test]
fn ripple_delete_rejects_straddling_boundary_overlap_atomically() {
    let fps = Rational::new(30, 1).unwrap();
    let mut document = empty_timeline(fps);
    Operation::AddAsset {
        asset: asset(1, fps, 300),
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(10),
        source: TimeCode(0)..TimeCode(10),
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(30),
        source: TimeCode(10)..TimeCode(20),
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(2),
        asset: AssetId(1),
        at: TimeCode(15),
        source: TimeCode(20)..TimeCode(30),
    }
    .apply(&mut document)
    .unwrap();
    Operation::AddClip {
        track: TrackId(2),
        asset: AssetId(1),
        at: TimeCode(30),
        source: TimeCode(30)..TimeCode(40),
    }
    .apply(&mut document)
    .unwrap();

    let before = document.clone();
    assert_eq!(
        Operation::RippleDeleteClip { clip: ClipId(1) }.apply(&mut document),
        Err(OpError::ClipOverlap {
            track: TrackId(2),
            clip: ClipId(3),
            with: ClipId(4),
        })
    );
    assert_eq!(document, before);
}

proptest! {
    #[test]
    fn ripple_delete_shift_math_matches_removed_project_duration(
        first_duration in 1_i64..80,
        removed_duration in 1_i64..80,
        first_gap in 0_i64..20,
        second_gap in 0_i64..20,
        source_locked in any::<bool>(),
        secondary_locked in any::<bool>(),
    ) {
        let fps = Rational::new(30, 1).unwrap();
        let mut document = empty_timeline(fps);
        Operation::AddAsset { asset: asset(1, fps, 300) }
            .apply(&mut document)
            .unwrap();
        Operation::AddTrack {
            track: Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        }
        .apply(&mut document)
        .unwrap();
        Operation::SetTrackSyncLock {
            track: TrackId(2),
            locked: secondary_locked,
        }
        .apply(&mut document)
        .unwrap();
        Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: source_locked,
        }
        .apply(&mut document)
        .unwrap();
        let second_start = first_duration.saturating_add(first_gap);
        let third_start = second_start
            .saturating_add(removed_duration)
            .saturating_add(second_gap);
        for (at, source) in [
            (0, TimeCode(0)..TimeCode(first_duration)),
            (
                second_start,
                TimeCode(first_duration)..TimeCode(first_duration + removed_duration),
            ),
            (
                third_start,
                TimeCode(first_duration + removed_duration)
                    ..TimeCode(first_duration + removed_duration + 1),
            ),
        ] {
            Operation::AddClip {
                track: TrackId(1),
                asset: AssetId(1),
                at: TimeCode(at),
                source,
            }
            .apply(&mut document)
            .unwrap();
        }
        Operation::AddClip {
            track: TrackId(2),
            asset: AssetId(1),
            at: TimeCode(third_start),
            source: TimeCode(0)..TimeCode(1),
        }
        .apply(&mut document)
        .unwrap();
        let ripple_point = second_start + removed_duration;
        for (id, position) in [
            (1, ripple_point - 1),
            (2, ripple_point),
            (3, third_start),
        ] {
            Operation::AddMarker {
                marker: Marker {
                    id: MarkerId(id),
                    position: TimeCode(position),
                    label: String::new(),
                    color_token: 0,
                },
            }
            .apply(&mut document)
            .unwrap();
        }

        Operation::RippleDeleteClip { clip: ClipId(2) }
            .apply(&mut document)
            .unwrap();
        prop_assert_eq!(document.clip(ClipId(3)).unwrap().timeline_start, TimeCode(third_start - removed_duration));
        let expected_secondary = if secondary_locked {
            third_start - removed_duration
        } else {
            third_start
        };
        prop_assert_eq!(document.clip(ClipId(4)).unwrap().timeline_start, TimeCode(expected_secondary));
        prop_assert_eq!(document.marker(MarkerId(1)).unwrap().position, TimeCode(ripple_point - 1));
        prop_assert_eq!(document.marker(MarkerId(2)).unwrap().position, TimeCode(second_start));
        prop_assert_eq!(document.marker(MarkerId(3)).unwrap().position, TimeCode(third_start - removed_duration));
        prop_assert!(document.validate().is_ok());
    }

    #[test]
    fn title_fades_accept_exact_integer_bounds(
        duration in 1_i64..600,
        fade_in in 0_i64..600,
        fade_out in 0_i64..600,
    ) {
        let fps = Rational::new(30, 1).unwrap();
        let mut document = empty_timeline(fps);
        let operation = Operation::AddTitle {
            track: TrackId(1),
            at: TimeCode::ZERO,
            duration: TimeCode(duration),
            title: Title {
                fade_in_frames: TimeCode(fade_in),
                fade_out_frames: TimeCode(fade_out),
                ..Title::default()
            },
        };
        let before = document.clone();
        let result = operation.apply(&mut document);
        if fade_in <= duration && fade_out <= duration {
            prop_assert!(result.is_ok());
            prop_assert_eq!(document.duration, TimeCode(duration));
        } else {
            let rejected_for_fade = matches!(result, Err(OpError::TitleFadeTooLong { .. }));
            prop_assert!(rejected_for_fade);
            prop_assert_eq!(document, before);
        }
    }
}

#[test]
fn link_and_unlink_validate_selection_and_only_mutate_link_data() {
    let mut document = document_with_three_clips();
    let original_positions = document.tracks[0]
        .clips
        .iter()
        .map(|clip| clip.timeline_start)
        .collect::<Vec<_>>();
    Operation::LinkClips {
        clips: vec![ClipId(1), ClipId(3)],
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(document.clip(ClipId(1)).unwrap().link, Some(LinkId(1)));
    assert_eq!(document.clip(ClipId(3)).unwrap().link, Some(LinkId(1)));
    assert_eq!(document.clip(ClipId(2)).unwrap().link, None);
    assert_eq!(
        document.tracks[0]
            .clips
            .iter()
            .map(|clip| clip.timeline_start)
            .collect::<Vec<_>>(),
        original_positions
    );

    Operation::UnlinkClips {
        clips: vec![ClipId(1)],
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(document.clip(ClipId(1)).unwrap().link, None);
    assert_eq!(document.clip(ClipId(3)).unwrap().link, Some(LinkId(1)));

    for invalid in [
        Operation::LinkClips {
            clips: vec![ClipId(1)],
        },
        Operation::LinkClips {
            clips: vec![ClipId(1), ClipId(1)],
        },
        Operation::LinkClips {
            clips: vec![ClipId(1), ClipId(99)],
        },
        Operation::UnlinkClips { clips: Vec::new() },
        Operation::UnlinkClips {
            clips: vec![ClipId(99)],
        },
    ] {
        let before = document.clone();
        assert!(invalid.apply(&mut document).is_err());
        assert_eq!(document, before);
    }
}

#[test]
fn marker_operations_validate_sort_move_remove_and_atomic_rejection() {
    let mut document = document_with_one_clip();
    for marker in [
        Marker {
            id: MarkerId(2),
            position: TimeCode(20),
            label: "Second".to_owned(),
            color_token: 1,
        },
        Marker {
            id: MarkerId(1),
            position: TimeCode(5),
            label: "First".to_owned(),
            color_token: 0,
        },
    ] {
        Operation::AddMarker { marker }
            .apply(&mut document)
            .unwrap();
    }
    assert_eq!(
        document
            .markers
            .iter()
            .map(|marker| marker.id)
            .collect::<Vec<_>>(),
        vec![MarkerId(1), MarkerId(2)]
    );

    Operation::MoveMarker {
        marker: MarkerId(2),
        to: TimeCode(3),
    }
    .apply(&mut document)
    .unwrap();
    assert_eq!(document.markers[0].id, MarkerId(2));
    Operation::RemoveMarker {
        marker: MarkerId(1),
    }
    .apply(&mut document)
    .unwrap();
    assert!(document.marker(MarkerId(1)).is_none());

    for invalid in [
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(2),
                position: TimeCode(8),
                label: "Duplicate".to_owned(),
                color_token: 0,
            },
        },
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(3),
                position: TimeCode(-1),
                label: "Negative".to_owned(),
                color_token: 0,
            },
        },
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(3),
                position: TimeCode(8),
                label: "Bad color".to_owned(),
                color_token: 4,
            },
        },
        Operation::MoveMarker {
            marker: MarkerId(2),
            to: TimeCode(-1),
        },
        Operation::RemoveMarker {
            marker: MarkerId(99),
        },
    ] {
        let before = document.clone();
        assert!(invalid.apply(&mut document).is_err());
        assert_eq!(document, before);
    }
}

#[test]
fn effect_operations_validate_names_ids_parameters_and_are_atomic() {
    let mut doc = document_with_one_clip();
    let effect = Effect {
        id: EffectId(7),
        name: "brightness".to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::new(),
    };
    Operation::AddEffect {
        clip: ClipId(1),
        effect: effect.clone(),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.clip(ClipId(1)).unwrap().effects, vec![effect]);

    let before_duplicate = doc.clone();
    assert_eq!(
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(7),
                name: "opacity".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
        }
        .apply(&mut doc),
        Err(OpError::DuplicateEffect {
            clip: ClipId(1),
            effect: EffectId(7),
        })
    );
    assert_eq!(doc, before_duplicate);

    Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(7),
        name: "percent".to_owned(),
        value: ParamValue::Integer(40),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        doc.clip(ClipId(1)).unwrap().effects[0]
            .parameters
            .get("percent"),
        Some(&ParamValue::Integer(40))
    );

    let before_invalid = doc.clone();
    assert_eq!(
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(7),
            name: "percent".to_owned(),
            value: ParamValue::Integer(101),
        }
        .apply(&mut doc),
        Err(OpError::EffectParamOutOfRange {
            effect: "brightness".to_owned(),
            name: "percent".to_owned(),
            min: -100,
            max: 100,
            actual: 101,
        })
    );
    assert_eq!(doc, before_invalid);

    Operation::RemoveEffect {
        clip: ClipId(1),
        effect: EffectId(7),
    }
    .apply(&mut doc)
    .unwrap();
    assert!(doc.clip(ClipId(1)).unwrap().effects.is_empty());
    assert_eq!(
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(8),
                name: "blur".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
        }
        .apply(&mut doc),
        Err(OpError::UnknownEffect("blur".to_owned()))
    );
}

#[test]
fn effect_keyframes_are_exact_validated_and_atomically_clearable() {
    let mut doc = document_with_one_clip();
    Operation::AddEffect {
        clip: ClipId(1),
        effect: Effect {
            id: EffectId(7),
            name: "brightness".to_owned(),
            parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(25))]),
            keyframes: BTreeMap::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    let curve = AutomationCurve {
        keyframes: vec![
            Keyframe {
                at: TimeCode::ZERO,
                value: -100,
                interpolation: KeyframeInterpolation::Linear,
            },
            Keyframe {
                at: TimeCode(10),
                value: 100,
                interpolation: KeyframeInterpolation::EaseInOut,
            },
        ],
    };
    Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(7),
        name: "percent".to_owned(),
        curve: curve.clone(),
    }
    .apply(&mut doc)
    .unwrap();
    let effect = &doc.clip(ClipId(1)).unwrap().effects[0];
    assert_eq!(effect.integer_parameter_at("percent", TimeCode(5)), Some(0));
    assert_eq!(effect.keyframes.get("percent"), Some(&curve));

    for invalid in [
        AutomationCurve {
            keyframes: vec![Keyframe {
                at: TimeCode(30),
                value: 0,
                interpolation: KeyframeInterpolation::Linear,
            }],
        },
        AutomationCurve {
            keyframes: vec![Keyframe {
                at: TimeCode(5),
                value: 101,
                interpolation: KeyframeInterpolation::Linear,
            }],
        },
        AutomationCurve { keyframes: vec![] },
    ] {
        let before = doc.clone();
        assert!(
            Operation::SetEffectKeyframes {
                clip: ClipId(1),
                effect: EffectId(7),
                name: "percent".to_owned(),
                curve: invalid,
            }
            .apply(&mut doc)
            .is_err()
        );
        assert_eq!(doc, before);
    }

    Operation::ClearEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(7),
        name: "percent".to_owned(),
    }
    .apply(&mut doc)
    .unwrap();
    let effect = &doc.clip(ClipId(1)).unwrap().effects[0];
    assert!(effect.keyframes.is_empty());
    assert_eq!(
        effect.integer_parameter_at("percent", TimeCode(5)),
        Some(25)
    );
}

#[test]
fn cube_lut_requires_a_non_empty_text_path_and_preserves_it() {
    let mut doc = document_with_one_clip();
    let before = doc.clone();
    assert_eq!(
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(8),
                name: "cube_lut".to_owned(),
                parameters: BTreeMap::from([(
                    "intensity_percent".to_owned(),
                    ParamValue::Integer(80),
                )]),
                keyframes: BTreeMap::new(),
            },
        }
        .apply(&mut doc),
        Err(OpError::MissingCubeLutPath)
    );
    assert_eq!(doc, before);

    Operation::AddEffect {
        clip: ClipId(1),
        effect: Effect {
            id: EffectId(8),
            name: "cube_lut".to_owned(),
            parameters: BTreeMap::from([
                (
                    "path".to_owned(),
                    ParamValue::Text("looks/ceremony.cube".to_owned()),
                ),
                ("intensity_percent".to_owned(), ParamValue::Integer(80)),
            ]),
            keyframes: BTreeMap::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        doc.clip(ClipId(1)).unwrap().effects[0]
            .parameters
            .get("path"),
        Some(&ParamValue::Text("looks/ceremony.cube".to_owned()))
    );
}

#[test]
fn audio_buses_validate_routing_effect_domains_and_project_keyframes_atomically() {
    let mut doc = document_with_one_clip();
    let gain = Effect {
        id: EffectId(1),
        name: "audio_gain".to_owned(),
        parameters: BTreeMap::from([("gain_tenth_db".to_owned(), ParamValue::Integer(-30))]),
        keyframes: BTreeMap::from([(
            "gain_tenth_db".to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode(20),
                    value: -60,
                    interpolation: KeyframeInterpolation::Linear,
                }],
            },
        )]),
    };
    Operation::UpsertAudioBus {
        bus: AudioBus {
            id: AudioBusId(1),
            name: "Dialogue".to_owned(),
            tracks: vec![TrackId(1)],
            effects: vec![gain],
            ducking_sidechain_tracks: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.audio_mix.buses[0].name, "Dialogue");

    let before = doc.clone();
    let invalid_buses = [
        AudioBus {
            id: AudioBusId(2),
            name: "Duplicate route".to_owned(),
            tracks: vec![TrackId(1)],
            effects: Vec::new(),
            ducking_sidechain_tracks: Vec::new(),
        },
        AudioBus {
            id: AudioBusId(1),
            name: "Visual effect".to_owned(),
            tracks: vec![TrackId(1)],
            effects: vec![Effect {
                id: EffectId(1),
                name: "brightness".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            }],
            ducking_sidechain_tracks: Vec::new(),
        },
        AudioBus {
            id: AudioBusId(1),
            name: "Outside project".to_owned(),
            tracks: vec![TrackId(1)],
            effects: vec![Effect {
                id: EffectId(1),
                name: "audio_gain".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::from([(
                    "gain_tenth_db".to_owned(),
                    AutomationCurve {
                        keyframes: vec![Keyframe {
                            at: doc.duration,
                            value: 0,
                            interpolation: KeyframeInterpolation::Linear,
                        }],
                    },
                )]),
            }],
            ducking_sidechain_tracks: Vec::new(),
        },
    ];
    for bus in invalid_buses {
        assert!(Operation::UpsertAudioBus { bus }.apply(&mut doc).is_err());
        assert_eq!(doc, before);
    }

    Operation::RemoveAudioBus { bus: AudioBusId(1) }
        .apply(&mut doc)
        .unwrap();
    assert!(doc.audio_mix.is_empty());
}

#[test]
fn transition_operations_validate_crossfade_duration_and_are_atomic() {
    let mut doc = document_with_one_clip();
    Operation::AddTransition {
        clip: ClipId(1),
        transition: Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(12),
        },
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        doc.clip(ClipId(1)).unwrap().transition_in,
        Some(Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(12),
        })
    );

    let before_duplicate = doc.clone();
    assert_eq!(
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "crossfade".to_owned(),
                duration: TimeCode(4),
            },
        }
        .apply(&mut doc),
        Err(OpError::DuplicateTransition(ClipId(1)))
    );
    assert_eq!(doc, before_duplicate);

    Operation::RemoveTransition { clip: ClipId(1) }
        .apply(&mut doc)
        .unwrap();
    assert_eq!(
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "crossfade".to_owned(),
                duration: TimeCode(31),
            },
        }
        .apply(&mut doc),
        Err(OpError::TransitionTooLong {
            clip: ClipId(1),
            duration: TimeCode(31),
            clip_duration: TimeCode(30),
        })
    );

    let before_invalid = doc.clone();
    assert_eq!(
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "unknown".to_owned(),
                duration: TimeCode(4),
            },
        }
        .apply(&mut doc),
        Err(OpError::UnknownTransition("unknown".to_owned()))
    );
    assert_eq!(doc, before_invalid);

    assert_eq!(
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "fade_from_black".to_owned(),
                duration: TimeCode::ZERO,
            },
        }
        .apply(&mut doc),
        Err(OpError::InvalidTransitionDuration {
            clip: ClipId(1),
            duration: TimeCode::ZERO,
        })
    );
    assert_eq!(doc, before_invalid);
}

#[test]
fn transition_descriptors_validate_all_registered_names_and_document_loads() {
    assert_eq!(TRANSITION_DESCRIPTORS.len(), 3);
    assert!(transition_descriptor("crossfade").is_some());
    assert!(transition_descriptor("fade_from_black").is_some());
    assert!(transition_descriptor("fade_from_white").is_some());
    assert!(transition_descriptor("dip_to_black").is_none());

    for name in ["fade_from_black", "fade_from_white"] {
        let mut document = document_with_one_clip();
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: name.to_owned(),
                duration: TimeCode(12),
            },
        }
        .apply(&mut document)
        .unwrap();

        let encoded = serde_json::to_string(&document).unwrap();
        let loaded: Document = serde_json::from_str(&encoded).unwrap();
        loaded.validate().unwrap();
        assert_eq!(
            loaded
                .clip(ClipId(1))
                .unwrap()
                .transition_in
                .as_ref()
                .unwrap()
                .name,
            name
        );
    }
}

#[test]
fn clip_audio_operation_validates_bounds_fades_and_titles_atomically() {
    let mut document = document_with_one_clip();
    for gain_tenth_db in [-600, 120] {
        Operation::SetClipAudio {
            clip: ClipId(1),
            gain_tenth_db,
            fade_in_frames: TimeCode(10),
            fade_out_frames: TimeCode(20),
        }
        .apply(&mut document)
        .unwrap();
        assert_eq!(
            document.clip(ClipId(1)).unwrap().audio_gain_tenth_db,
            gain_tenth_db
        );
    }

    for gain_tenth_db in [-601, 121] {
        let before = document.clone();
        assert_eq!(
            Operation::SetClipAudio {
                clip: ClipId(1),
                gain_tenth_db,
                fade_in_frames: TimeCode::ZERO,
                fade_out_frames: TimeCode::ZERO,
            }
            .apply(&mut document),
            Err(OpError::AudioGainOutOfRange {
                clip: ClipId(1),
                gain_tenth_db,
            })
        );
        assert_eq!(document, before);
    }

    for (fade_in_frames, fade_out_frames, expected) in [
        (
            TimeCode(-1),
            TimeCode::ZERO,
            OpError::NegativeAudioFade {
                clip: ClipId(1),
                name: "fade-in",
                frames: TimeCode(-1),
            },
        ),
        (
            TimeCode::ZERO,
            TimeCode(-1),
            OpError::NegativeAudioFade {
                clip: ClipId(1),
                name: "fade-out",
                frames: TimeCode(-1),
            },
        ),
        (
            TimeCode(15),
            TimeCode(16),
            OpError::AudioFadesTooLong {
                clip: ClipId(1),
                fade_in_frames: TimeCode(15),
                fade_out_frames: TimeCode(16),
                fade_total: TimeCode(31),
                clip_duration: TimeCode(30),
            },
        ),
    ] {
        let before = document.clone();
        assert_eq!(
            Operation::SetClipAudio {
                clip: ClipId(1),
                gain_tenth_db: 0,
                fade_in_frames,
                fade_out_frames,
            }
            .apply(&mut document),
            Err(expected)
        );
        assert_eq!(document, before);
    }

    let mut titles = empty_timeline(Rational::new(30, 1).unwrap());
    Operation::AddTitle {
        track: TrackId(1),
        at: TimeCode::ZERO,
        duration: TimeCode(30),
        title: Title::default(),
    }
    .apply(&mut titles)
    .unwrap();
    assert_eq!(
        Operation::SetClipAudio {
            clip: ClipId(1),
            gain_tenth_db: 0,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
        }
        .apply(&mut titles),
        Err(OpError::TitleClipHasNoAudio(ClipId(1)))
    );
}

#[test]
fn hand_edited_clip_audio_values_are_rejected_during_document_validation() {
    let valid = document_with_one_clip();
    for (gain, fade_in, fade_out, expected) in [
        (
            121,
            TimeCode::ZERO,
            TimeCode::ZERO,
            OpError::AudioGainOutOfRange {
                clip: ClipId(1),
                gain_tenth_db: 121,
            },
        ),
        (
            0,
            TimeCode(-1),
            TimeCode::ZERO,
            OpError::NegativeAudioFade {
                clip: ClipId(1),
                name: "fade-in",
                frames: TimeCode(-1),
            },
        ),
        (
            0,
            TimeCode(20),
            TimeCode(11),
            OpError::AudioFadesTooLong {
                clip: ClipId(1),
                fade_in_frames: TimeCode(20),
                fade_out_frames: TimeCode(11),
                fade_total: TimeCode(31),
                clip_duration: TimeCode(30),
            },
        ),
    ] {
        let mut value = serde_json::to_value(&valid).unwrap();
        let clip = &mut value["tracks"][0]["clips"][0];
        clip["audio_gain_tenth_db"] = serde_json::json!(gain);
        clip["audio_fade_in_frames"] = serde_json::json!(fade_in.0);
        clip["audio_fade_out_frames"] = serde_json::json!(fade_out.0);
        let loaded: Document = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.validate(), Err(expected));
    }
}

#[test]
fn clip_audio_uses_exact_snapshot_undo() {
    let initial = document_with_one_clip();
    let core = Core::spawn(initial.clone()).unwrap();
    let Event::DocumentChanged { doc, .. } = core
        .request(Command::Do(Operation::SetClipAudio {
            clip: ClipId(1),
            gain_tenth_db: -60,
            fade_in_frames: TimeCode(6),
            fade_out_frames: TimeCode(9),
        }))
        .unwrap()
    else {
        panic!("clip audio operation should be accepted");
    };
    assert_eq!(doc.clip(ClipId(1)).unwrap().audio_gain_tenth_db, -60);

    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("clip audio operation should be undoable");
    };
    assert_eq!(&*doc, &initial);
}

#[test]
fn add_and_remove_track_are_validated_and_atomic() {
    let mut doc = Document::default();
    let video = Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: Vec::new(),
    };
    Operation::AddTrack {
        track: video.clone(),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.tracks, vec![video.clone()]);

    let before_duplicate = doc.clone();
    assert_eq!(
        Operation::AddTrack {
            track: video.clone(),
        }
        .apply(&mut doc),
        Err(OpError::DuplicateTrack(TrackId(1)))
    );
    assert_eq!(doc, before_duplicate);

    let non_empty = Track {
        id: TrackId(2),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![Clip {
            id: ClipId(99),
            asset: AssetId(99),
            source_range: TimeCode(0)..TimeCode(1),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }],
    };
    assert_eq!(
        Operation::AddTrack { track: non_empty }.apply(&mut doc),
        Err(OpError::NewTrackNotEmpty(TrackId(2)))
    );

    Operation::RemoveTrack { track: TrackId(1) }
        .apply(&mut doc)
        .unwrap();
    assert!(doc.tracks.is_empty());
    assert_eq!(
        Operation::RemoveTrack {
            track: TrackId(404)
        }
        .apply(&mut doc),
        Err(OpError::MissingTrack(TrackId(404)))
    );
}

#[test]
fn remove_track_cascades_clips_and_recomputes_duration() {
    let mut doc = document_with_one_clip();
    Operation::RemoveTrack { track: TrackId(1) }
        .apply(&mut doc)
        .unwrap();
    assert!(doc.tracks.is_empty());
    assert_eq!(doc.duration, TimeCode::ZERO);
    assert_eq!(doc.media_pool.len(), 1);
}

#[test]
fn left_edge_trim_moves_the_timeline_start_as_one_atomic_edit() {
    let mut doc = document_with_one_clip();
    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(5)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();

    assert_eq!(doc.tracks[0].clips[0].timeline_start, TimeCode(15));
    assert_eq!(
        doc.tracks[0].clips[0].source_range,
        TimeCode(5)..TimeCode(30)
    );
    assert_eq!(doc.duration, TimeCode(40));
}

#[test]
fn project_json_round_trip_preserves_exact_document_equality() {
    let doc = document_with_one_clip();
    let json = serde_json::to_string_pretty(&doc).unwrap();
    let loaded: Document = serde_json::from_str(&json).unwrap();
    loaded.validate().unwrap();
    assert_eq!(loaded, doc);
}

#[test]
fn track_sync_lock_uses_snapshot_undo_and_redo() {
    let core = Core::spawn(empty_timeline(Rational::new(30, 1).unwrap())).unwrap();
    let Event::DocumentChanged { doc, .. } = core
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }))
        .unwrap()
    else {
        panic!("sync-lock operation should be accepted");
    };
    assert!(!doc.tracks[0].sync_lock);

    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("sync-lock operation should be undoable");
    };
    assert!(doc.tracks[0].sync_lock);

    let Event::DocumentChanged { doc, .. } = core.request(Command::Redo).unwrap() else {
        panic!("sync-lock operation should be redoable");
    };
    assert!(!doc.tracks[0].sync_lock);
}

#[test]
fn ripple_marker_shifts_restore_exact_positions_on_snapshot_undo() {
    let mut initial = document_with_three_clips();
    for (id, position) in [(1, 40), (2, 70)] {
        Operation::AddMarker {
            marker: Marker {
                id: MarkerId(id),
                position: TimeCode(position),
                label: format!("Marker {id}"),
                color_token: 0,
            },
        }
        .apply(&mut initial)
        .unwrap();
    }
    let core = Core::spawn(initial.clone()).unwrap();

    let Event::DocumentChanged { doc, .. } = core
        .request(Command::Do(Operation::RippleDeleteClip { clip: ClipId(2) }))
        .unwrap()
    else {
        panic!("ripple delete should be accepted");
    };
    assert_eq!(doc.marker(MarkerId(1)).unwrap().position, TimeCode(20));
    assert_eq!(doc.marker(MarkerId(2)).unwrap().position, TimeCode(50));
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("ripple delete should be undoable");
    };
    assert_eq!(&*doc, &initial);

    let Event::DocumentChanged { doc, .. } = core
        .request(Command::Do(Operation::RippleInsertGap {
            track: TrackId(1),
            at: TimeCode(40),
            duration: TimeCode(5),
        }))
        .unwrap()
    else {
        panic!("ripple insert should be accepted");
    };
    assert_eq!(doc.marker(MarkerId(1)).unwrap().position, TimeCode(45));
    assert_eq!(doc.marker(MarkerId(2)).unwrap().position, TimeCode(75));
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("ripple insert should be undoable");
    };
    assert_eq!(&*doc, &initial);
}

#[test]
fn public_actor_builds_undoes_redoes_saves_and_reopens_a_rough_cut() {
    let core = Core::spawn(Document::default()).unwrap();
    let events = core.subscribe().unwrap();
    let _initial = events.recv_timeout(Duration::from_secs(1)).unwrap();
    let fps = Rational::new(30, 1).unwrap();
    let operations = [
        Operation::AddTrack {
            track: Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            },
        },
        Operation::AddAsset {
            asset: asset(1, fps, 120),
        },
        Operation::AddAsset {
            asset: asset(2, fps, 120),
        },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(0),
            source: TimeCode(0)..TimeCode(120),
        },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(2),
            at: TimeCode(120),
            source: TimeCode(0)..TimeCode(120),
        },
        Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(30),
        },
        Operation::TrimClip {
            clip: ClipId(1),
            new_source: TimeCode(5)..TimeCode(30),
        },
        Operation::MoveClip {
            clip: ClipId(2),
            to_track: TrackId(1),
            to: TimeCode(130),
        },
        Operation::DeleteClip { clip: ClipId(3) },
    ];

    let mut latest = None;
    for operation in operations {
        core.send(Command::Do(operation)).unwrap();
        let Event::DocumentChanged { doc, .. } =
            events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected accepted edit");
        };
        latest = Some(doc);
    }
    core.send(Command::Undo).unwrap();
    let Event::DocumentChanged { doc: restored, .. } =
        events.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("expected undo snapshot");
    };
    assert!(restored.clip(ClipId(3)).is_some());
    core.send(Command::Redo).unwrap();
    let Event::DocumentChanged { doc: redone, .. } =
        events.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("expected redo snapshot");
    };
    assert_eq!(redone, latest.unwrap());
    redone.validate().unwrap();

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("kinewright-m2-{nonce}.kinewright"));
    fs::write(&path, serde_json::to_string_pretty(&*redone).unwrap()).unwrap();
    let reopened: Document = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let _ = fs::remove_file(path);
    reopened.validate().unwrap();
    assert_eq!(reopened, *redone);
}

#[test]
fn missing_asset_is_rejected_atomically() {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    let before = doc.clone();
    let error = Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(404),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(10),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert_eq!(error, OpError::MissingAsset(AssetId(404)));
    assert_eq!(doc, before);
}

#[test]
fn overlapping_clip_is_rejected_atomically() {
    let mut doc = document_with_one_clip();
    let before = doc.clone();
    let error = Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(20),
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert!(matches!(error, OpError::ClipOverlap { .. }));
    assert_eq!(doc, before);
}

#[test]
fn unsorted_input_document_is_rejected() {
    let fps = Rational::new(30, 1).unwrap();
    let media = asset(1, fps, 300);
    let later = Clip {
        id: ClipId(1),
        asset: AssetId(1),
        source_range: TimeCode(0)..TimeCode(10),
        content: ClipContent::Media,
        timeline_start: TimeCode(20),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
    };
    let earlier = Clip {
        id: ClipId(2),
        timeline_start: TimeCode(0),
        ..later.clone()
    };
    let doc = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![later, earlier],
        }],
        media_pool: vec![media],
        markers: Vec::new(),
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode(30),
    };

    assert!(matches!(doc.validate(), Err(OpError::ClipsUnsorted { .. })));
}

#[test]
fn out_of_bounds_trim_is_rejected_atomically() {
    let mut doc = document_with_one_clip();
    let before = doc.clone();
    let error = Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(0)..TimeCode(301),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert!(matches!(error, OpError::SourceOutOfBounds { .. }));
    assert_eq!(doc, before);
}

#[test]
fn slip_preserves_the_timeline_slot_and_moves_equal_source_handles() {
    let mut doc = document_with_butt_joined_clips();
    let before_duration = doc.duration;

    Operation::SlipClip {
        clip: ClipId(2),
        new_source_in: TimeCode(170),
    }
    .apply(&mut doc)
    .unwrap();

    let clip = doc.clip(ClipId(2)).unwrap();
    assert_eq!(clip.timeline_start, TimeCode(50));
    assert_eq!(clip.source_range, TimeCode(170)..TimeCode(200));
    assert_eq!(doc.duration, before_duration);
}

#[test]
fn roll_moves_only_the_shared_boundary_and_preserves_outer_duration() {
    let mut doc = document_with_butt_joined_clips();

    Operation::RollEdit {
        left_clip: ClipId(1),
        right_clip: ClipId(2),
        to: TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();

    let left = doc.clip(ClipId(1)).unwrap();
    let right = doc.clip(ClipId(2)).unwrap();
    assert_eq!(left.source_range, TimeCode(50)..TimeCode(110));
    assert_eq!(right.source_range, TimeCode(160)..TimeCode(180));
    assert_eq!(right.timeline_start, TimeCode(60));
    assert_eq!(doc.duration, TimeCode(130));
}

#[test]
fn slide_keeps_middle_source_and_sequence_outer_boundaries_fixed() {
    let mut doc = document_with_butt_joined_clips();
    let middle_source = doc.clip(ClipId(2)).unwrap().source_range.clone();

    Operation::SlideClip {
        clip: ClipId(2),
        to: TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();

    assert_eq!(doc.clip(ClipId(1)).unwrap().source_range.end, TimeCode(110));
    let middle = doc.clip(ClipId(2)).unwrap();
    assert_eq!(middle.timeline_start, TimeCode(60));
    assert_eq!(middle.source_range, middle_source);
    let right = doc.clip(ClipId(3)).unwrap();
    assert_eq!(right.timeline_start, TimeCode(90));
    assert_eq!(right.source_range.start, TimeCode(230));
    assert_eq!(doc.duration, TimeCode(130));
}

#[test]
fn replace_and_fit_to_fill_preserve_the_clip_slot() {
    let mut replaced = document_with_butt_joined_clips();
    Operation::ReplaceClip {
        clip: ClipId(2),
        asset: AssetId(1),
        source: TimeCode(300)..TimeCode(330),
    }
    .apply(&mut replaced)
    .unwrap();
    let clip = replaced.clip(ClipId(2)).unwrap();
    assert_eq!(clip.timeline_start, TimeCode(50));
    assert_eq!(clip.source_range, TimeCode(300)..TimeCode(330));
    assert_eq!(clip.speed_percent, 100);

    let mut fitted = document_with_butt_joined_clips();
    Operation::FitToFill {
        clip: ClipId(2),
        asset: AssetId(1),
        source: TimeCode(300)..TimeCode(360),
    }
    .apply(&mut fitted)
    .unwrap();
    let clip = fitted.clip(ClipId(2)).unwrap();
    assert_eq!(clip.timeline_start, TimeCode(50));
    assert_eq!(fitted.clip_duration(clip).unwrap(), TimeCode(30));
    assert_eq!(clip.speed_percent, 200);
    assert_eq!(fitted.duration, TimeCode(130));
}

#[test]
fn three_point_insert_and_overwrite_derive_the_unmarked_boundary() {
    let original = document_with_butt_joined_clips();
    let mut inserted = original.clone();
    Operation::ThreePointEdit {
        track: TrackId(1),
        asset: AssetId(1),
        source_in: Some(TimeCode(300)),
        source_out: Some(TimeCode(330)),
        timeline_in: Some(TimeCode(50)),
        timeline_out: None,
        mode: ThreePointMode::Insert,
    }
    .apply(&mut inserted)
    .unwrap();
    assert_eq!(
        inserted.clip(ClipId(4)).unwrap().timeline_start,
        TimeCode(50)
    );
    assert_eq!(
        inserted.clip(ClipId(2)).unwrap().timeline_start,
        TimeCode(80)
    );
    assert_eq!(inserted.duration, TimeCode(160));

    let mut overwritten = original;
    Operation::ThreePointEdit {
        track: TrackId(1),
        asset: AssetId(1),
        source_in: Some(TimeCode(300)),
        source_out: None,
        timeline_in: Some(TimeCode(50)),
        timeline_out: Some(TimeCode(80)),
        mode: ThreePointMode::Overwrite,
    }
    .apply(&mut overwritten)
    .unwrap();
    assert!(overwritten.clip(ClipId(2)).is_none());
    let replacement = overwritten.clip(ClipId(4)).unwrap();
    assert_eq!(replacement.timeline_start, TimeCode(50));
    assert_eq!(replacement.source_range, TimeCode(300)..TimeCode(330));
    assert_eq!(overwritten.duration, TimeCode(130));
}

#[test]
fn three_point_insert_splits_a_target_clip_at_the_record_point() {
    let mut doc = document_with_butt_joined_clips();

    Operation::ThreePointEdit {
        track: TrackId(1),
        asset: AssetId(1),
        source_in: Some(TimeCode(300)),
        source_out: Some(TimeCode(310)),
        timeline_in: Some(TimeCode(25)),
        timeline_out: None,
        mode: ThreePointMode::Insert,
    }
    .apply(&mut doc)
    .unwrap();

    let inserted = doc
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .find(|clip| clip.source_range == (TimeCode(300)..TimeCode(310)))
        .unwrap();
    assert_eq!(inserted.timeline_start, TimeCode(25));
    let right_half = doc
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .find(|clip| clip.source_range.start == TimeCode(75))
        .unwrap();
    assert_eq!(right_half.timeline_start, TimeCode(35));
    assert_eq!(doc.duration, TimeCode(140));
}

#[test]
fn three_point_insert_splits_every_sync_locked_straddler_before_ripple() {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddAsset {
        asset: asset(1, fps, 300),
    }
    .apply(&mut doc)
    .unwrap();
    for track in [TrackId(1), TrackId(2)] {
        Operation::AddClip {
            track,
            asset: AssetId(1),
            at: TimeCode::ZERO,
            source: TimeCode::ZERO..TimeCode(100),
        }
        .apply(&mut doc)
        .unwrap();
    }

    Operation::ThreePointEdit {
        track: TrackId(1),
        asset: AssetId(1),
        source_in: Some(TimeCode(200)),
        source_out: Some(TimeCode(210)),
        timeline_in: Some(TimeCode(50)),
        timeline_out: None,
        mode: ThreePointMode::Insert,
    }
    .apply(&mut doc)
    .unwrap();

    assert_eq!(doc.tracks[0].clips.len(), 3);
    assert_eq!(doc.tracks[1].clips.len(), 2);
    assert_eq!(doc.tracks[0].clips[2].timeline_start, TimeCode(60));
    assert_eq!(doc.tracks[1].clips[1].timeline_start, TimeCode(60));
    assert_eq!(doc.duration, TimeCode(110));
}

#[test]
fn professional_edits_reject_missing_handles_atomically() {
    let mut doc = document_with_butt_joined_clips();
    let before = doc.clone();

    assert!(matches!(
        Operation::SlipClip {
            clip: ClipId(2),
            new_source_in: TimeCode(590),
        }
        .apply(&mut doc),
        Err(OpError::SourceOutOfBounds { .. })
    ));
    assert_eq!(doc, before);

    assert!(matches!(
        Operation::ThreePointEdit {
            track: TrackId(1),
            asset: AssetId(1),
            source_in: Some(TimeCode(10)),
            source_out: Some(TimeCode(20)),
            timeline_in: Some(TimeCode(30)),
            timeline_out: Some(TimeCode(40)),
            mode: ThreePointMode::Overwrite,
        }
        .apply(&mut doc),
        Err(OpError::InvalidThreePointSelection)
    ));
    assert_eq!(doc, before);
}

#[test]
fn bins_string_outs_and_sync_groups_are_validated_undoable_catalog_data() {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    for id in [1, 2] {
        Operation::AddAsset {
            asset: asset(id, fps, 300),
        }
        .apply(&mut doc)
        .unwrap();
    }
    Operation::UpsertBin {
        bin: MediaBin {
            id: BinId(1),
            name: "Ceremony".to_owned(),
            parent: None,
            assets: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::SetAssetBin {
        asset: AssetId(1),
        bin: Some(BinId(1)),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.catalog.bins[0].assets, vec![AssetId(1)]);

    Operation::UpsertStringOut {
        string_out: StringOut {
            id: StringOutId(1),
            name: "Best vows".to_owned(),
            selects: vec![SourceSelect {
                asset: AssetId(1),
                source: TimeCode(30)..TimeCode(90),
                label: "Partner A".to_owned(),
            }],
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::UpsertSyncGroup {
        sync_group: SyncGroup {
            id: SyncGroupId(1),
            name: "Ceremony angles".to_owned(),
            members: vec![
                SyncGroupMember {
                    asset: AssetId(1),
                    offset: TimeCode::ZERO,
                    angle_name: "Wide".to_owned(),
                },
                SyncGroupMember {
                    asset: AssetId(2),
                    offset: TimeCode(-3),
                    angle_name: "Close".to_owned(),
                },
            ],
        },
    }
    .apply(&mut doc)
    .unwrap();
    doc.validate().unwrap();
    let reopened: Document = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
    assert_eq!(reopened, doc);

    let core = Core::spawn(doc.clone()).unwrap();
    assert!(matches!(
        core.request(Command::Do(Operation::RemoveStringOut {
            string_out: StringOutId(1),
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let Event::DocumentChanged { doc: restored, .. } = core.request(Command::Undo).unwrap() else {
        panic!("catalog undo must restore the exact document");
    };
    assert_eq!(*restored, doc);
}

#[test]
fn catalog_rejects_cycles_duplicate_members_and_invalid_selects_atomically() {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddAsset {
        asset: asset(1, fps, 300),
    }
    .apply(&mut doc)
    .unwrap();
    let before = doc.clone();

    assert!(matches!(
        Operation::UpsertBin {
            bin: MediaBin {
                id: BinId(1),
                name: "Loop".to_owned(),
                parent: Some(BinId(1)),
                assets: Vec::new(),
            },
        }
        .apply(&mut doc),
        Err(OpError::BinSelfParent(BinId(1)))
    ));
    assert_eq!(doc, before);

    assert!(matches!(
        Operation::UpsertStringOut {
            string_out: StringOut {
                id: StringOutId(1),
                name: "Bad".to_owned(),
                selects: vec![SourceSelect {
                    asset: AssetId(1),
                    source: TimeCode(290)..TimeCode(310),
                    label: String::new(),
                }],
            },
        }
        .apply(&mut doc),
        Err(OpError::SourceOutOfBounds { .. })
    ));
    assert_eq!(doc, before);

    assert!(matches!(
        Operation::UpsertSyncGroup {
            sync_group: SyncGroup {
                id: SyncGroupId(1),
                name: "Bad".to_owned(),
                members: vec![
                    SyncGroupMember {
                        asset: AssetId(1),
                        offset: TimeCode::ZERO,
                        angle_name: "A".to_owned(),
                    },
                    SyncGroupMember {
                        asset: AssetId(1),
                        offset: TimeCode(1),
                        angle_name: "B".to_owned(),
                    },
                ],
            },
        }
        .apply(&mut doc),
        Err(OpError::DuplicateSyncGroupAsset { .. })
    ));
    assert_eq!(doc, before);
}

#[test]
fn mixed_rate_split_preserves_the_clip_boundary_and_total_duration() {
    let source_fps = Rational::new(24_000, 1_001).unwrap();
    let project_fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(project_fps);
    Operation::AddAsset {
        asset: asset(1, source_fps, 48),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(1)..TimeCode(48),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.duration, TimeCode(59));

    Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(29),
    }
    .apply(&mut doc)
    .unwrap();

    assert_eq!(doc.tracks[0].clips.len(), 2);
    assert_eq!(doc.tracks[0].clips[0].source_range.end, TimeCode(24));
    assert_eq!(doc.tracks[0].clips[1].source_range.start, TimeCode(24));
    assert_eq!(doc.tracks[0].clips[1].timeline_start, TimeCode(29));
    assert_eq!(doc.duration, TimeCode(59));
    doc.validate().unwrap();
}

#[test]
fn mixed_rate_split_rejects_a_non_source_frame_boundary() {
    let source_fps = Rational::new(24_000, 1_001).unwrap();
    let project_fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(project_fps);
    Operation::AddAsset {
        asset: asset(1, source_fps, 48),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(48),
    }
    .apply(&mut doc)
    .unwrap();
    let before = doc.clone();

    let error = Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(2),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert_eq!(
        error,
        OpError::UnrepresentableSplit {
            clip: ClipId(1),
            at: TimeCode(2),
        }
    );
    assert_eq!(doc, before);
}

#[test]
fn clip_speed_scales_duration_exactly_and_validates_bounds() {
    let mut doc = document_with_one_clip();
    let clip = doc.clip(ClipId(1)).unwrap().clone();
    assert_eq!(doc.clip_duration(&clip).unwrap(), TimeCode(30));

    Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 50,
    }
    .apply(&mut doc)
    .unwrap();
    let clip = doc.clip(ClipId(1)).unwrap().clone();
    assert_eq!(doc.clip_duration(&clip).unwrap(), TimeCode(60));

    Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 200,
    }
    .apply(&mut doc)
    .unwrap();
    let clip = doc.clip(ClipId(1)).unwrap().clone();
    assert_eq!(doc.clip_duration(&clip).unwrap(), TimeCode(15));

    let before = doc.clone();
    for out_of_range in [0, 9, 1001, u32::MAX] {
        assert!(matches!(
            Operation::SetClipSpeed {
                clip: ClipId(1),
                speed_percent: out_of_range,
            }
            .apply(&mut doc),
            Err(OpError::ClipSpeedOutOfRange(value)) if value == out_of_range
        ));
        assert_eq!(doc, before);
    }
}

#[test]
fn slowing_a_clip_into_a_neighbor_is_rejected_atomically() {
    let mut doc = document_with_three_clips();
    let before = doc.clone();
    assert!(matches!(
        Operation::SetClipSpeed {
            clip: ClipId(1),
            speed_percent: 25,
        }
        .apply(&mut doc),
        Err(OpError::ClipOverlap { .. })
    ));
    assert_eq!(doc, before);

    Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 50,
    }
    .apply(&mut doc)
    .unwrap();
    let clip = doc.clip(ClipId(1)).unwrap().clone();
    assert_eq!(doc.clip_duration(&clip).unwrap(), TimeCode(20));
}

#[test]
fn non_media_clips_reject_speed_everywhere() {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddTitle {
        track: TrackId(1),
        at: TimeCode(0),
        duration: TimeCode(30),
        title: Title::default(),
    }
    .apply(&mut doc)
    .unwrap();
    assert!(matches!(
        Operation::SetClipSpeed {
            clip: ClipId(1),
            speed_percent: 50,
        }
        .apply(&mut doc),
        Err(OpError::SpeedOnNonMediaClip(ClipId(1)))
    ));

    let mut hand_edited = doc.clone();
    hand_edited.tracks[0].clips[0].speed_percent = 50;
    assert!(matches!(
        hand_edited.validate(),
        Err(OpError::SpeedOnNonMediaClip(ClipId(1)))
    ));
}

#[test]
fn split_preserves_project_adjacency_and_total_duration_at_any_speed() {
    for speed in [10_u32, 33, 50, 100, 150, 1000] {
        let project_fps = Rational::new(30, 1).unwrap();
        let source_fps = Rational::new(24_000, 1_001).unwrap();
        let mut doc = empty_timeline(project_fps);
        Operation::AddAsset {
            asset: asset(1, source_fps, 480),
        }
        .apply(&mut doc)
        .unwrap();
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(0),
            source: TimeCode(0)..TimeCode(480),
        }
        .apply(&mut doc)
        .unwrap();
        Operation::SetClipSpeed {
            clip: ClipId(1),
            speed_percent: speed,
        }
        .apply(&mut doc)
        .unwrap();
        let original = doc.clip(ClipId(1)).unwrap().clone();
        let total = doc.clip_duration(&original).unwrap();
        assert!(
            total > TimeCode(1),
            "speed {speed} produced a degenerate clip"
        );

        // Not every project frame maps to an exact source boundary at odd
        // speeds; scan outward from the midpoint for a representable split,
        // mirroring how interactive splitting snaps.
        let mid = total.0 / 2;
        let mut split_at = None;
        for offset in 0..total.0 / 2 {
            for candidate in [mid - offset, mid + offset] {
                if candidate <= 0 || candidate >= total.0 {
                    continue;
                }
                let mut attempt = doc.clone();
                let split = Operation::SplitClip {
                    clip: ClipId(1),
                    at: TimeCode(candidate),
                };
                if split.apply(&mut attempt).is_ok() {
                    doc = attempt;
                    split_at = Some(candidate);
                    break;
                }
            }
            if split_at.is_some() {
                break;
            }
        }
        let split_at =
            split_at.unwrap_or_else(|| panic!("no representable split at speed {speed}"));

        let left = doc.clip(ClipId(1)).unwrap().clone();
        let right = doc.clip(ClipId(2)).unwrap().clone();
        assert_eq!(left.speed_percent, speed);
        assert_eq!(
            right.speed_percent, speed,
            "split right half must inherit speed"
        );
        let left_duration = doc.clip_duration(&left).unwrap();
        let right_duration = doc.clip_duration(&right).unwrap();
        assert_eq!(left.timeline_start, TimeCode(0));
        assert_eq!(
            left_duration,
            TimeCode(split_at),
            "left half must end at the split"
        );
        assert_eq!(right.timeline_start, TimeCode(split_at));
        assert_eq!(
            left_duration.0 + right_duration.0,
            total.0,
            "speed {speed}: split must conserve total project duration"
        );
        doc.validate().unwrap();
    }
}

#[test]
fn clip_speed_serde_defaults_skips_and_round_trips() {
    let doc = document_with_one_clip();
    let encoded = serde_json::to_string(&doc).unwrap();
    assert!(
        !encoded.contains("speed_percent"),
        "real-time speed must not serialize"
    );
    let legacy: Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(legacy.clip(ClipId(1)).unwrap().speed_percent, 100);

    let mut speeded = doc.clone();
    Operation::SetClipSpeed {
        clip: ClipId(1),
        speed_percent: 150,
    }
    .apply(&mut speeded)
    .unwrap();
    let encoded = serde_json::to_string(&speeded).unwrap();
    assert!(encoded.contains("\"speed_percent\":150"));
    let round: Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(round, speeded);
}

#[test]
fn clip_speed_change_is_one_undo_step() {
    let original = document_with_one_clip();
    let core = Core::spawn(original.clone()).unwrap();
    assert!(matches!(
        core.request(Command::Do(Operation::SetClipSpeed {
            clip: ClipId(1),
            speed_percent: 400,
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("undo must return the restored document");
    };
    assert_eq!(*doc, original);
}
