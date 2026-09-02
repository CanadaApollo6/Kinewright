//! Caption and dialogue pacing tests.

use super::*;
use crate::server::captions::{caption_position, clamp_caption_cues_to_duration};
use crate::server::planning::{
    DialoguePacingSettings, dialogue_filler_bridges, dialogue_keep_ranges,
};

#[test]
fn caption_position_avoids_the_subject_and_honors_explicit_direction() {
    assert_eq!(
        caption_position(None, Some(50)),
        Ok(TitlePosition::LowerThird)
    );
    assert_eq!(caption_position(None, Some(75)), Ok(TitlePosition::Top));
    assert_eq!(
        caption_position(Some(TitlePosition::Top), Some(20)),
        Ok(TitlePosition::Top)
    );
    assert!(caption_position(None, Some(101)).is_err());
}

#[test]
fn authored_caption_path_reduces_the_dialogue_capability_surface() {
    let tools = KinewrightMcp::tools().unwrap();
    let open = |names: &[&str]| {
        open_capabilities(
            &tools,
            CapabilityArgs {
                name: None,
                names: names.iter().map(ToString::to_string).collect(),
            },
        )
    };
    let legacy = serde_json::to_vec(&open(&[
        "get_transcripts",
        "plan_dialogue_assembly",
        "add_styled_captions",
        "get_captions",
        "plan_caption_corrections",
        "get_dialogue_pacing",
        "get_editorial_readiness",
    ]))
    .unwrap()
    .len();
    let authored = serde_json::to_vec(&open(&[
        "get_transcripts",
        "plan_dialogue_assembly",
        "add_styled_captions",
        "get_dialogue_pacing",
        "get_editorial_readiness",
    ]))
    .unwrap()
    .len();

    println!("dialogue capability payload: legacy={legacy} B authored={authored} B");
    assert!(authored < legacy);
}

#[test]
fn dialogue_pacing_adds_a_bounded_capability_payload() {
    let tools = KinewrightMcp::tools().unwrap();
    let shared = [
        "get_transcripts",
        "plan_dialogue_assembly",
        "add_styled_captions",
        "get_captions",
        "plan_caption_corrections",
        "get_editorial_readiness",
    ];
    let open = |names: &[&str]| {
        open_capabilities(
            &tools,
            CapabilityArgs {
                name: None,
                names: names.iter().map(ToString::to_string).collect(),
            },
        )
    };
    let v3_bytes = serde_json::to_vec(&open(&shared)).unwrap().len();
    let mut v4 = shared.to_vec();
    v4.push("get_dialogue_pacing");
    let v4_bytes = serde_json::to_vec(&open(&v4)).unwrap().len();

    println!("dialogue capability payload: v3={v3_bytes} B v4={v4_bytes} B");
    assert!(v4_bytes > v3_bytes);
    assert!(v4_bytes - v3_bytes < 2_500);
    assert!(v4_bytes < 20_000);
}

#[test]
fn dialogue_keep_ranges_remove_qualified_silence_and_fillers() {
    let fps = Rational::new(30, 1).unwrap();
    let asset = MediaAsset {
        id: AssetId(1),
        path: "dialogue.mp4".into(),
        name: "dialogue".to_owned(),
        duration: TimeCode(120),
        fps,
        kind: MediaKind::AudioVideo,
        resolution: Some((320, 180)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let transcript = AssetTranscript {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        words: vec![
            TranscriptWord {
                text: "Keep".to_owned(),
                source_start: TimeCode(4),
                source_end: TimeCode(15),
                speaker: None,
            },
            TranscriptWord {
                text: "Um,".to_owned(),
                source_start: TimeCode(75),
                source_end: TimeCode(82),
                speaker: None,
            },
            TranscriptWord {
                text: "going".to_owned(),
                source_start: TimeCode(90),
                source_end: TimeCode(105),
                speaker: None,
            },
        ],
    };
    let silences = AssetSilences {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        source_frames: asset.duration,
        threshold_dbfs_hundredths: -4_000,
        window_milliseconds: 20,
        spans: vec![SilenceSpan {
            source_start: TimeCode(20),
            source_end: TimeCode(70),
        }],
    };

    assert_eq!(
        dialogue_keep_ranges(
            &asset,
            &transcript,
            &silences,
            TimeCode(20),
            true,
            DialoguePacingSettings {
                retained_pause: TimeCode::ZERO,
                filler_padding: TimeCode::ZERO,
                maximum_filler_bridge_pause: None,
            },
            TimeCode::ZERO..asset.duration,
        ),
        vec![
            TimeCode(0)..TimeCode(20),
            TimeCode(70)..TimeCode(75),
            TimeCode(82)..TimeCode(120),
        ]
    );
}

#[test]
fn dialogue_keep_ranges_retain_pause_and_pad_fillers() {
    let fps = Rational::new(30, 1).unwrap();
    let asset = MediaAsset {
        id: AssetId(1),
        path: "dialogue.mp4".into(),
        name: "dialogue".to_owned(),
        duration: TimeCode(120),
        fps,
        kind: MediaKind::AudioVideo,
        resolution: Some((320, 180)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let transcript = AssetTranscript {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        words: vec![TranscriptWord {
            text: "Um".to_owned(),
            source_start: TimeCode(75),
            source_end: TimeCode(82),
            speaker: None,
        }],
    };
    let silences = AssetSilences {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        source_frames: asset.duration,
        threshold_dbfs_hundredths: -4_000,
        window_milliseconds: 20,
        spans: vec![SilenceSpan {
            source_start: TimeCode(20),
            source_end: TimeCode(70),
        }],
    };

    assert_eq!(
        dialogue_keep_ranges(
            &asset,
            &transcript,
            &silences,
            TimeCode(20),
            true,
            DialoguePacingSettings {
                retained_pause: TimeCode(6),
                filler_padding: TimeCode(3),
                maximum_filler_bridge_pause: None,
            },
            TimeCode::ZERO..asset.duration,
        ),
        vec![TimeCode(0)..TimeCode(23), TimeCode(85)..TimeCode(120),]
    );
}

#[test]
fn dialogue_keep_ranges_never_escape_the_requested_source_envelope() {
    let fps = Rational::new(30, 1).unwrap();
    let asset = MediaAsset {
        id: AssetId(1),
        path: "dialogue.mp4".into(),
        name: "dialogue".to_owned(),
        duration: TimeCode(120),
        fps,
        kind: MediaKind::AudioVideo,
        resolution: Some((320, 180)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let transcript = AssetTranscript {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        words: vec![TranscriptWord {
            text: "Um".to_owned(),
            source_start: TimeCode(75),
            source_end: TimeCode(82),
            speaker: None,
        }],
    };
    let silences = AssetSilences {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        source_frames: asset.duration,
        threshold_dbfs_hundredths: -4_000,
        window_milliseconds: 20,
        spans: vec![SilenceSpan {
            source_start: TimeCode(20),
            source_end: TimeCode(70),
        }],
    };

    assert_eq!(
        dialogue_keep_ranges(
            &asset,
            &transcript,
            &silences,
            TimeCode(20),
            true,
            DialoguePacingSettings {
                retained_pause: TimeCode::ZERO,
                filler_padding: TimeCode::ZERO,
                maximum_filler_bridge_pause: None,
            },
            TimeCode(10)..TimeCode(100),
        ),
        vec![
            TimeCode(10)..TimeCode(20),
            TimeCode(70)..TimeCode(75),
            TimeCode(82)..TimeCode(100),
        ]
    );
}

#[test]
fn dialogue_filler_bridge_caps_long_pauses_and_preserves_shorter_ones() {
    let fps = Rational::new(30, 1).unwrap();
    let asset = MediaAsset {
        id: AssetId(1),
        path: "dialogue.mp4".into(),
        name: "dialogue".to_owned(),
        duration: TimeCode(120),
        fps,
        kind: MediaKind::AudioVideo,
        resolution: Some((320, 180)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let transcript = AssetTranscript {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        words: vec![
            TranscriptWord {
                text: "First.".to_owned(),
                source_start: TimeCode(5),
                source_end: TimeCode(15),
                speaker: None,
            },
            TranscriptWord {
                text: "Um".to_owned(),
                source_start: TimeCode(25),
                source_end: TimeCode(30),
                speaker: None,
            },
            TranscriptWord {
                text: "uh,".to_owned(),
                source_start: TimeCode(30),
                source_end: TimeCode(35),
                speaker: None,
            },
            TranscriptWord {
                text: "Then".to_owned(),
                source_start: TimeCode(50),
                source_end: TimeCode(60),
                speaker: None,
            },
        ],
    };
    let silences = AssetSilences {
        asset: asset.id,
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        source_frames: asset.duration,
        threshold_dbfs_hundredths: -4_000,
        window_milliseconds: 20,
        spans: vec![
            SilenceSpan {
                source_start: TimeCode(15),
                source_end: TimeCode(25),
            },
            SilenceSpan {
                source_start: TimeCode(35),
                source_end: TimeCode(50),
            },
        ],
    };

    let bridges = dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(12)), TimeCode(20));
    assert_eq!(bridges.len(), 1);
    assert_eq!(bridges[0].cut_start, TimeCode(21));
    assert_eq!(bridges[0].cut_end, TimeCode(44));
    assert_eq!(bridges[0].retained_pause_source_frames, TimeCode(12));
    assert_eq!(bridges[0].measurement, "acoustic_silence");
    let preserved =
        dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(30)), TimeCode(20));
    assert_eq!(preserved[0].available_pause_source_frames, TimeCode(25));
    assert_eq!(preserved[0].retained_pause_source_frames, TimeCode(25));
    assert_eq!(preserved[0].cut_start, TimeCode(25));
    assert_eq!(preserved[0].cut_end, TimeCode(35));
    assert_eq!(
        dialogue_keep_ranges(
            &asset,
            &transcript,
            &silences,
            TimeCode(5),
            true,
            DialoguePacingSettings {
                retained_pause: TimeCode(6),
                filler_padding: TimeCode(3),
                maximum_filler_bridge_pause: Some(TimeCode(12)),
            },
            TimeCode::ZERO..asset.duration,
        ),
        vec![TimeCode(0)..TimeCode(19), TimeCode(46)..TimeCode(120)]
    );
}

#[test]
fn dialogue_filler_bridge_uses_acoustic_edges_when_asr_endpoints_are_late() {
    let fps = Rational::new(30, 1).unwrap();
    let transcript = AssetTranscript {
        asset: AssetId(1),
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        words: vec![
            TranscriptWord {
                text: "rain".to_owned(),
                source_start: TimeCode(128),
                source_end: TimeCode(141),
                speaker: None,
            },
            TranscriptWord {
                text: "um".to_owned(),
                source_start: TimeCode(162),
                source_end: TimeCode(184),
                speaker: None,
            },
            TranscriptWord {
                text: "um".to_owned(),
                source_start: TimeCode(197),
                source_end: TimeCode(219),
                speaker: None,
            },
            TranscriptWord {
                text: "Neighbors".to_owned(),
                source_start: TimeCode(233),
                source_end: TimeCode(245),
                speaker: None,
            },
        ],
    };
    let silences = AssetSilences {
        asset: AssetId(1),
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        source_frames: TimeCode(331),
        threshold_dbfs_hundredths: -3_500,
        window_milliseconds: 10,
        spans: vec![
            SilenceSpan {
                source_start: TimeCode(108),
                source_end: TimeCode(162),
            },
            SilenceSpan {
                source_start: TimeCode(205),
                source_end: TimeCode(234),
            },
        ],
    };

    let bridges = dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(12)), TimeCode(20));

    assert_eq!(bridges.len(), 1);
    assert_eq!(bridges[0].source_start, TimeCode(108));
    assert_eq!(bridges[0].source_end, TimeCode(234));
    assert_eq!(bridges[0].cut_start, TimeCode(114));
    assert_eq!(bridges[0].cut_end, TimeCode(228));
    assert_eq!(bridges[0].retained_pause_source_frames, TimeCode(12));
    assert_eq!(bridges[0].measurement, "acoustic_silence");
}

#[test]
fn dialogue_filler_bridge_never_leaves_one_cuttable_acoustic_flank() {
    let fps = Rational::new(30, 1).unwrap();
    let transcript = AssetTranscript {
        asset: AssetId(1),
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        words: vec![
            TranscriptWord {
                text: "built".to_owned(),
                source_start: TimeCode(96),
                source_end: TimeCode(104),
                speaker: None,
            },
            TranscriptWord {
                text: "Um".to_owned(),
                source_start: TimeCode(162),
                source_end: TimeCode(193),
                speaker: None,
            },
            TranscriptWord {
                text: "Um".to_owned(),
                source_start: TimeCode(193),
                source_end: TimeCode(229),
                speaker: None,
            },
            TranscriptWord {
                text: "Then".to_owned(),
                source_start: TimeCode(233),
                source_end: TimeCode(237),
                speaker: None,
            },
        ],
    };
    let silences = AssetSilences {
        asset: AssetId(1),
        content_sha256: "fixture".to_owned(),
        source_fps: fps,
        source_frames: TimeCode(323),
        threshold_dbfs_hundredths: -3_500,
        window_milliseconds: 10,
        spans: vec![
            SilenceSpan {
                source_start: TimeCode(107),
                source_end: TimeCode(162),
            },
            SilenceSpan {
                source_start: TimeCode(205),
                source_end: TimeCode(234),
            },
        ],
    };

    let bridges = dialogue_filler_bridges(&transcript, &silences, Some(TimeCode(31)), TimeCode(20));

    assert_eq!(bridges.len(), 1);
    assert_eq!(
        bridges[0].maximum_contiguous_pause_source_frames,
        TimeCode(19)
    );
    assert_eq!(bridges[0].retained_pause_source_frames, TimeCode(24));
    assert_eq!(bridges[0].cut_start, TimeCode(126));
    assert_eq!(bridges[0].cut_end, TimeCode(229));
}

#[test]
fn dialogue_pacing_classifies_sentence_gaps_without_marking_word_gaps() {
    let word = |text: &str, asset: u64, start: i64, end: i64| TimelineTranscriptWord {
        text: text.to_owned(),
        speaker: None,
        asset: AssetId(asset),
        track: TrackId(1),
        clip: ClipId(asset),
        source_start: TimeCode(start),
        source_end: TimeCode(end),
        project_start: TimeCode(start),
        project_end: TimeCode(end),
    };
    let words = vec![
        word("rain", 1, 80, 100),
        word("Neighbors", 1, 112, 130),
        word("instead", 1, 180, 200),
        word("Over", 2, 212, 230),
        word("beds", 2, 280, 300),
        word("Then", 2, 307, 325),
        word("peppers.", 2, 380, 400),
        word("Now", 3, 420, 438),
        word("continues", 3, 440, 458),
    ];

    let gaps = dialogue_pacing_gaps(&words, &[], TimeCode(9), TimeCode(15), TimeCode(4));
    assert_eq!(gaps.len(), 4);
    assert_eq!(gaps[0].status, "target");
    assert_eq!(gaps[0].reason, "pause_backed_capitalization");
    assert_eq!(gaps[1].status, "target");
    assert!(gaps[1].reason.contains("asset_change"));
    assert_eq!(gaps[2].status, "short");
    assert_eq!(gaps[2].pause_frames, TimeCode(7));
    assert_eq!(gaps[3].status, "long");
    assert!(gaps[3].reason.contains("terminal_punctuation"));
}

#[test]
fn caption_hold_is_clamped_to_the_media_timeline() {
    let mut cues = vec![CaptionCue {
        start: TimeCode(90),
        end: TimeCode(115),
        text: "last line".to_owned(),
    }];
    clamp_caption_cues_to_duration(&mut cues, TimeCode(100));
    assert_eq!(cues[0].end, TimeCode(100));
}
