#![allow(clippy::too_many_lines)]

use std::{collections::BTreeMap, fs::File, sync::Arc};

use kinewright_core::{
    AssetId, BlendMode, CaptionPreset, Clip, ClipContent, ClipId, DeliveryEncodeDepth,
    DeliveryProfile, Document, ExactMicros, ExportCancellation, Rational, SolidColor, TimeCode,
    Track, TrackId, TrackKind,
    output_review::{
        AbsenceProof, OverlayBoundsRun, OverlayExpectations, OverlayKind, OverlayPixels,
        OverlayText, ReviewCutIdentity, ReviewExpectations, ReviewInventory, ReviewLane,
        ReviewOverlay, ReviewSegment, ReviewSegmentId, ReviewSnapshotId, SegmentSampling,
    },
    title_layout,
};

use super::*;
use crate::{
    LutLibrary,
    cc1_fixtures::fallback_gpu,
    export::{OwnedExport, OwnedExportTarget, export_owned_document},
    owned_io::{LifetimeGuard, ManifestFonts, PlaybackInputs},
    review_output::{ReviewOutputStream, inspect_review_output},
    test_support::TempDirectory,
};

fn fake_timing(frames: u64) -> ReviewOutputTiming {
    ReviewOutputTiming {
        decoder_version: 1,
        video: ReviewOutputStream {
            index: 0,
            decoder: "h264".into(),
            time_base: (1, 24_000),
            start_pts: 0,
            duration_pts: i64::try_from(frames).unwrap() * 1_000,
        },
        audio: None,
        video_first_pts: 0,
        video_pts_step: Some(1_000),
        video_frames: frames,
    }
}

fn sample_expectations(
    frames: i64,
    cuts: &[i64],
    overlay_span: Option<(i64, i64)>,
) -> ReviewExpectations {
    let fps = Rational::new(24, 1).unwrap();
    let t = |f: i64| ExactMicros::frames(TimeCode(f), fps).unwrap();
    let mut segments = Vec::new();
    let mut prev = 0_i64;
    for (idx, &cut) in cuts.iter().chain(std::iter::once(&frames)).enumerate() {
        segments.push(ReviewSegment {
            id: ReviewSegmentId {
                clip: ClipId(10 + idx as u64),
                lane: ReviewLane::Video,
                project_start: TimeCode(prev),
            },
            track: TrackId(1),
            asset: AssetId(1),
            source: t(0)..t(cut - prev),
            project: TimeCode(prev)..TimeCode(cut),
            output: t(prev)..t(cut),
            sampling: SegmentSampling::VideoHold {
                frame: TimeCode(0),
                source_fps: fps,
            },
        });
        prev = cut;
    }

    let captions = if let Some((s, e)) = overlay_span {
        let title = CaptionPreset::Clean.title("Sample");
        let layout = title_layout(&title, (320, 180)).unwrap();
        ReviewInventory::Present {
            first: ReviewOverlay {
                element: ClipId(2),
                kind: OverlayKind::Caption,
                cue: t(s)..t(e),
                visible: vec![OverlayBoundsRun {
                    output: t(s)..t(e),
                    bounds: Ok(layout.visual_bounds),
                }],
                text: OverlayText::Expected("Sample".into()),
                pixels: OverlayPixels::RenderedText,
                sourced: false,
            },
            rest: Vec::new(),
        }
    } else {
        ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput)
    };

    ReviewExpectations {
        identity: ReviewCutIdentity {
            snapshot: ReviewSnapshotId([1; 32]),
            input_manifest_sha256: [2; 32],
            export_settings_sha256: [3; 32],
        },
        project_fps: fps,
        output_fps: fps,
        raster: (320, 180),
        output_frames: TimeCode(frames),
        transcripts: Vec::new(),
        segments,
        kept_words: ReviewInventory::NoneProven(AbsenceProof::NoAudibleSourceSegments),
        speech_windows: ReviewInventory::NoneProven(AbsenceProof::NoAudibleSourceSegments),
        uncovered_speech_segments: Vec::new(),
        intended_pauses: ReviewInventory::NoneProven(AbsenceProof::NoAuthorPauseDeclarations),
        av_landmarks: ReviewInventory::NoneProven(AbsenceProof::NoPairedAudioVideo),
        overlays: OverlayExpectations {
            annotated_off_output: Vec::new(),
            captions,
            titles: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
            cards: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
        },
    }
}

#[test]
fn s5_c6_plan_covers_opening_middle_end_cuts_overlays_and_regions() {
    let fps = Rational::new(24, 1).unwrap();
    let t = |f: i64| ExactMicros::frames(TimeCode(f), fps).unwrap();
    let expectations = sample_expectations(48, &[16, 32], Some((6, 12)));
    let timing = fake_timing(48);
    let plan = plan_review_stills(
        &expectations,
        &timing,
        &ReviewStillPlanInput {
            repaired_regions: vec![t(20)..t(22)],
            commented_regions: vec![t(40)..t(41)],
        },
    )
    .unwrap();

    let has = |frame: u64, reason: ReviewStillReason| {
        plan.samples
            .iter()
            .any(|s| s.frame_index == frame && s.reasons.contains(&reason))
    };
    assert!(has(0, ReviewStillReason::Opening));
    assert!(has(24, ReviewStillReason::Middle));
    assert!(has(47, ReviewStillReason::End));
    assert!(has(16, ReviewStillReason::Cut));
    assert!(has(32, ReviewStillReason::Cut));
    assert!(has(6, ReviewStillReason::OverlayTransition));
    assert!(has(11, ReviewStillReason::OverlayTransition));
    assert!(has(12, ReviewStillReason::OverlayTransition));
    assert!(has(21, ReviewStillReason::RepairedRegion));
    assert!(has(40, ReviewStillReason::CommentedRegion));

    // Overlay frames must advertise the active overlay so reference stills are attached.
    let s6 = plan.samples.iter().find(|s| s.frame_index == 6).unwrap();
    assert_eq!(s6.overlays, vec![ClipId(2)]);
    assert!(s6.expects_reference());
    let s40 = plan.samples.iter().find(|s| s.frame_index == 40).unwrap();
    assert!(!s40.expects_reference());
}

#[test]
fn s5_c6_plan_caps_candidates_at_max_review_stills() {
    let cuts: Vec<i64> = (1..80).map(|i| i * 2).collect();
    let expectations = sample_expectations(200, &cuts, None);
    let timing = fake_timing(200);
    let plan =
        plan_review_stills(&expectations, &timing, &ReviewStillPlanInput::default()).unwrap();
    assert!(plan.candidate_count > MAX_REVIEW_STILLS);
    assert_eq!(plan.samples.len(), MAX_REVIEW_STILLS);
    assert!(
        plan.samples
            .iter()
            .any(|s| s.reasons.contains(&ReviewStillReason::Opening))
    );
    assert!(
        plan.samples
            .iter()
            .any(|s| s.reasons.contains(&ReviewStillReason::Middle))
    );
    assert!(
        plan.samples
            .iter()
            .any(|s| s.reasons.contains(&ReviewStillReason::End))
    );
}

#[test]
fn s5_c6_decode_stills_page_emits_decoded_and_reference_stills_with_bound_pts() {
    let dir = TempDirectory::new("s5-c6-decode-stills");
    let caption = CaptionPreset::Clean.title("Visual review text");
    let layout = title_layout(&caption, (320, 180)).unwrap();
    let doc = Document {
        resolution: (320, 180),
        fps: Rational::new(24, 1).unwrap(),
        duration: TimeCode(10),
        tracks: vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: AssetId(0),
                    timeline_start: TimeCode::ZERO,
                    source_range: TimeCode::ZERO..TimeCode(10),
                    content: ClipContent::Solid(SolidColor {
                        r: 20,
                        g: 35,
                        b: 60,
                    }),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    audio_gain_curve: None,
                    speed_percent: 100,
                    enabled: true,
                    enabled_curve: None,
                    blend_mode: BlendMode::Normal,
                }],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![Clip {
                    id: ClipId(2),
                    asset: AssetId(0),
                    timeline_start: TimeCode(2),
                    source_range: TimeCode::ZERO..TimeCode(6),
                    content: ClipContent::Title(caption.clone()),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    audio_gain_curve: None,
                    speed_percent: 100,
                    enabled: true,
                    enabled_curve: None,
                    blend_mode: BlendMode::Normal,
                }],
            },
        ],
        ..Document::default()
    };

    let settings = DeliveryProfile::SourceMaster.export_settings(
        &doc,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );
    let noop = |_: u64| -> Result<LifetimeGuard, String> { Ok(Arc::new(())) };
    let commit = |install: &mut dyn FnMut() -> Result<(), MediaError>| install();
    let out = dir.path("stills.mp4");
    let (progress, _) = crossbeam_channel::bounded(0);
    let font_bytes = kinewright_core::title_font_bytes();
    let font_digest = crate::sha256_bytes(font_bytes);
    let fonts =
        ManifestFonts::bind([("font:Inter", font_digest.as_str(), font_bytes.len() as u64)]);
    let OwnedExport::Clean(_) = export_owned_document(
        &Arc::new(
            PlaybackInputs::admit(
                Arc::new(doc.clone()),
                BTreeMap::new(),
                Arc::new(LutLibrary::default()),
                Arc::new(()),
                fonts,
            )
            .unwrap(),
        ),
        OwnedExportTarget {
            destination: &out,
            staging_bytes: 4 << 20,
            reserve_staging: &noop,
            reserve_working: &noop,
            commit: &commit,
        },
        &settings,
        &progress,
        fallback_gpu().context(),
    )
    .unwrap() else {
        panic!("clean export");
    };
    let timing =
        inspect_review_output(File::open(&out).unwrap(), &ExportCancellation::default()).unwrap();
    let fps = doc.fps;
    let t = |f: i64| ExactMicros::frames(TimeCode(f), fps).unwrap();
    let mut expectations = sample_expectations(10, &[], None);
    expectations.overlays.captions = ReviewInventory::Present {
        first: ReviewOverlay {
            element: ClipId(2),
            kind: OverlayKind::Caption,
            cue: t(2)..t(8),
            visible: vec![OverlayBoundsRun {
                output: t(2)..t(8),
                bounds: Ok(layout.visual_bounds),
            }],
            text: OverlayText::Expected(caption.text),
            pixels: OverlayPixels::RenderedText,
            sourced: false,
        },
        rest: Vec::new(),
    };

    let plan =
        plan_review_stills(&expectations, &timing, &ReviewStillPlanInput::default()).unwrap();
    let page = &plan.samples[..plan.samples.len().min(MAX_STILLS_PER_PAGE)];
    let mut no_cards = |_: [u8; 32], _: u64| Err(MediaError::Backend("none".into()));
    let stills = decode_review_stills_page(
        File::open(&out).unwrap(),
        &timing,
        &expectations,
        &doc,
        &mut no_cards,
        page,
        MAX_STILL_LONG_EDGE,
        &ExportCancellation::default(),
    )
    .unwrap();

    assert!(
        stills
            .iter()
            .any(|s| s.kind == ReviewStillKind::DecodedOutput)
    );
    assert!(
        stills
            .iter()
            .any(|s| s.kind == ReviewStillKind::OverlayReference)
    );
    for still in &stills {
        assert_eq!(still.width, 320);
        assert_eq!(still.height, 180);
        assert_eq!(still.rgba8.len(), 320 * 180 * 4);
        let planned = plan
            .samples
            .iter()
            .find(|p| p.sample_index == still.sample_index)
            .unwrap();
        assert_eq!(still.pts, planned.expected_pts);
        assert_eq!(still.frame_index, planned.frame_index);
    }
}
