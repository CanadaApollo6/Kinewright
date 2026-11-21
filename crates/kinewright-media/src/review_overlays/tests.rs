#![allow(clippy::too_many_lines)]

use std::{collections::BTreeMap, fs::File, path::Path, sync::Arc};

use kinewright_core::{
    AssetId, AutomationCurve, BlendMode, CaptionPreset, Clip, ClipContent, ClipId,
    DeliveryEncodeDepth, DeliveryProfile, Document, Effect, EffectId, ExactMicros,
    ExportCancellation, Keyframe, KeyframeInterpolation, MediaAsset, MediaKind, Operation,
    Rational, SolidColor, TimeCode, Title, TitlePosition, Track, TrackId, TrackKind,
    output_review::{
        AbsenceProof, OverlayBoundsRun, OverlayExpectations, OverlayKind, OverlayPixels,
        OverlayText, OverlayUnknown, ReviewCutIdentity, ReviewExpectations, ReviewInventory,
        ReviewOverlay, ReviewRowResult, ReviewSnapshotId,
    },
    title_layout,
};

use super::*;
use crate::{
    LutLibrary,
    cc1_fixtures::fallback_gpu,
    decode::probe_path,
    export::{OwnedExport, OwnedExportTarget, export_owned_document},
    owned_io::{CHUNK_BYTES, CertifiedSource, LifetimeGuard, ManifestFonts, PlaybackInputs},
    review_geometry::{OverlayGeometry, review_overlay_geometry},
    review_output::inspect_review_output,
    test_support::{TempDirectory, run_ffmpeg},
};

const RESOLUTION: (u32, u32) = (480, 270);
const STAGING_BYTES: u64 = 4 << 20;

#[allow(clippy::unnecessary_wraps)]
fn noop_reserve(_: u64) -> Result<LifetimeGuard, String> {
    Ok(Arc::new(()))
}

fn embedded_fonts() -> ManifestFonts {
    let bytes = kinewright_core::title_font_bytes();
    let digest = crate::sha256_bytes(bytes);
    ManifestFonts::bind([("font:Inter", digest.as_str(), bytes.len() as u64)])
}

fn encode_doc(
    document: &Document,
    sources: BTreeMap<AssetId, CertifiedSource>,
    out_path: &Path,
) -> (File, ReviewOutputTiming) {
    let settings = DeliveryProfile::SourceMaster.export_settings(
        document,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );
    let inputs = Arc::new(
        PlaybackInputs::admit(
            Arc::new(document.clone()),
            sources,
            Arc::new(LutLibrary::default()),
            Arc::new(()),
            embedded_fonts(),
        )
        .expect("admit playback inputs"),
    );
    let commit = |install: &mut dyn FnMut() -> Result<(), MediaError>| install();
    let target = OwnedExportTarget {
        destination: out_path,
        staging_bytes: STAGING_BYTES,
        reserve_staging: &noop_reserve,
        reserve_working: &noop_reserve,
        commit: &commit,
    };
    let (progress, _) = crossbeam_channel::bounded(0);
    let gpu = fallback_gpu().context();
    let OwnedExport::Clean(_receipt) =
        export_owned_document(&inputs, target, &settings, &progress, gpu)
            .expect("fixture owned export")
    else {
        panic!("expected clean owned export");
    };
    let timing = inspect_review_output(
        File::open(out_path).unwrap(),
        &ExportCancellation::default(),
    )
    .expect("inspect review output");
    (File::open(out_path).unwrap(), timing)
}

fn solid_clip(id: u64, duration: i64, color: SolidColor) -> Clip {
    Clip {
        id: ClipId(id),
        asset: AssetId(0),
        timeline_start: TimeCode::ZERO,
        source_range: TimeCode::ZERO..TimeCode(duration),
        content: ClipContent::Solid(color),
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
    }
}

fn title_clip(id: u64, start: i64, duration: i64, title: Title) -> Clip {
    Clip {
        id: ClipId(id),
        asset: AssetId(0),
        timeline_start: TimeCode(start),
        source_range: TimeCode::ZERO..TimeCode(duration),
        content: ClipContent::Title(title),
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
    }
}

fn document_with(tracks: Vec<Track>, assets: Vec<MediaAsset>, duration: i64) -> Document {
    Document {
        resolution: RESOLUTION,
        fps: Rational::new(24, 1).unwrap(),
        duration: TimeCode(duration),
        media_pool: assets,
        tracks,
        ..Document::default()
    }
}

fn derive_overlay_expectations_for_test(
    document: &Document,
    pinned_cards: &BTreeMap<AssetId, ([u8; 32], u64)>,
) -> ReviewExpectations {
    let fps = document.fps;
    let total = document.duration;
    let mut captions = Vec::new();
    let mut titles = Vec::new();
    let mut cards = Vec::new();

    for track in document
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
    {
        for clip in &track.clips {
            let (kind, text, pixels, base_bounds) = match &clip.content {
                ClipContent::Title(title) => {
                    let kind = if title.caption_preset.is_some() {
                        OverlayKind::Caption
                    } else {
                        OverlayKind::Title
                    };
                    let layout = title_layout(title, document.resolution).unwrap();
                    (
                        kind,
                        OverlayText::Expected(title.text.clone()),
                        OverlayPixels::RenderedText,
                        layout.visual_bounds,
                    )
                }
                ClipContent::Media | ClipContent::Freeze(_)
                    if document
                        .asset(clip.asset)
                        .is_some_and(|a| a.kind == MediaKind::Image) =>
                {
                    let (sha256, byte_length) = pinned_cards[&clip.asset];
                    (
                        OverlayKind::Card,
                        OverlayText::Unknown(OverlayUnknown::OpaqueCard),
                        OverlayPixels::PinnedImage {
                            sha256,
                            byte_length,
                        },
                        TitlePixelBounds {
                            left: 0,
                            top: 0,
                            right: i32::try_from(document.resolution.0).unwrap(),
                            bottom: i32::try_from(document.resolution.1).unwrap(),
                        },
                    )
                }
                _ => continue,
            };
            let dur = document.clip_duration(clip).unwrap();
            let end = TimeCode(clip.timeline_start.0 + dur.0);
            let mut visible: Vec<OverlayBoundsRun> = Vec::new();
            for frame in clip.timeline_start.0..end.0 {
                let at = TimeCode(frame);
                let OverlayGeometry::Visible(bounds) =
                    review_overlay_geometry(document, clip, at, base_bounds, document.resolution)
                        .unwrap()
                else {
                    continue;
                };
                let span = ExactMicros::frames(TimeCode(frame), fps).unwrap()
                    ..ExactMicros::frames(TimeCode(frame + 1), fps).unwrap();
                if let Some(last) = visible
                    .last_mut()
                    .filter(|last| last.bounds == Ok(bounds) && last.output.end == span.start)
                {
                    last.output.end = span.end;
                } else {
                    visible.push(OverlayBoundsRun {
                        output: span,
                        bounds: Ok(bounds),
                    });
                }
            }
            let entry = ReviewOverlay {
                element: clip.id,
                kind,
                cue: ExactMicros::frames(clip.timeline_start, fps).unwrap()
                    ..ExactMicros::frames(end, fps).unwrap(),
                visible,
                text,
                pixels,
                sourced: false,
            };
            match kind {
                OverlayKind::Caption => captions.push(entry),
                OverlayKind::Title => titles.push(entry),
                OverlayKind::Card => cards.push(entry),
            }
        }
    }

    let to_inv = |mut v: Vec<ReviewOverlay>| {
        if v.is_empty() {
            ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput)
        } else {
            let first = v.remove(0);
            ReviewInventory::Present { first, rest: v }
        }
    };

    ReviewExpectations {
        identity: ReviewCutIdentity {
            snapshot: ReviewSnapshotId([1; 32]),
            input_manifest_sha256: [2; 32],
            export_settings_sha256: [3; 32],
        },
        project_fps: fps,
        output_fps: fps,
        raster: document.resolution,
        output_frames: total,
        transcripts: Vec::new(),
        segments: Vec::new(),
        kept_words: ReviewInventory::NoneProven(AbsenceProof::NoAudibleSourceSegments),
        speech_windows: ReviewInventory::NoneProven(AbsenceProof::NoAudibleSourceSegments),
        uncovered_speech_segments: Vec::new(),
        intended_pauses: ReviewInventory::NoneProven(AbsenceProof::NoAuthorPauseDeclarations),
        av_landmarks: ReviewInventory::NoneProven(AbsenceProof::NoPairedAudioVideo),
        overlays: OverlayExpectations {
            annotated_off_output: Vec::new(),
            captions: to_inv(captions),
            titles: to_inv(titles),
            cards: to_inv(cards),
        },
    }
}

fn no_cards(_: [u8; 32], _: u64) -> Result<Box<dyn ReviewReadSeek>, MediaError> {
    Err(MediaError::Backend("no pinned card in fixture".into()))
}

#[test]
fn s5_c6_clean_captions_and_titles_pass_including_animated_extremes() {
    let dir = TempDirectory::new("s5-c6-clean-overlays");
    let bg = solid_clip(
        1,
        14,
        SolidColor {
            r: 28,
            g: 48,
            b: 84,
        },
    );
    let clean_caption = title_clip(2, 0, 7, CaptionPreset::Clean.title("Clean caption text"));
    let social_caption = title_clip(3, 7, 7, CaptionPreset::Social.title("Social line"));
    let mut animated_title = title_clip(
        4,
        2,
        10,
        Title {
            text: "Moving Title".into(),
            font_size_token: 1,
            color_token: 2,
            position: TitlePosition::Top,
            background_scrim: true,
            fade_in_frames: TimeCode(3),
            fade_out_frames: TimeCode(3),
            caption_preset: None,
        },
    );
    animated_title.effects.push(Effect {
        id: EffectId(10),
        name: "transform".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::from([
            (
                "x_percent".into(),
                AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode(0),
                            value: -20,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                        Keyframe {
                            at: TimeCode(9),
                            value: 20,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                    ],
                },
            ),
            (
                "scale_percent".into(),
                AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode(0),
                            value: 85,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                        Keyframe {
                            at: TimeCode(9),
                            value: 112,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                    ],
                },
            ),
        ]),
    });

    let document = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![bg],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![clean_caption],
            },
            Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![social_caption],
            },
            Track {
                id: TrackId(4),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![animated_title],
            },
        ],
        Vec::new(),
        14,
    );

    let expectations = derive_overlay_expectations_for_test(&document, &BTreeMap::new());
    let (file, timing) = encode_doc(&document, BTreeMap::new(), &dir.path("clean.mp4"));
    let report = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &document,
        &mut no_cards,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .expect("overlay verification run");

    assert_eq!(
        report.row_result,
        ReviewRowResult::Pass,
        "S5_C6_CLEAN_CONTROLS_PASS: {:?}",
        report.findings
    );
    assert_eq!(report.checked_overlays, 3);
    assert!(
        report.animated_extreme_frames >= 4,
        "animated extremes must be checked: {report:?}"
    );
    let anim_summary = report
        .overlays
        .iter()
        .find(|o| o.element == ClipId(4))
        .unwrap();
    // Fade-in frame 0 (frame 2) has alpha 0 (hidden), fade-out last frame (frame 11) has alpha 0 -> 8 visible frames.
    assert_eq!(anim_summary.checked_frames, 8);
    assert_eq!(anim_summary.status, OverlayElementStatus::Passed);
    assert!(anim_summary.animated_extreme_frames >= 2);
}

#[test]
fn s5_c6_hidden_caption_and_occluded_caption_independently_fail() {
    let dir = TempDirectory::new("s5-c6-hidden-caption");
    let bg = solid_clip(
        1,
        8,
        SolidColor {
            r: 30,
            g: 45,
            b: 70,
        },
    );
    let caption = title_clip(2, 0, 8, CaptionPreset::Clean.title("Visible spoken line"));
    let frozen_doc = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![bg.clone()],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![caption.clone()],
            },
        ],
        Vec::new(),
        8,
    );
    let expectations = derive_overlay_expectations_for_test(&frozen_doc, &BTreeMap::new());

    // Defect 1a: caption omitted/disabled during encode.
    let mut omitted_doc = frozen_doc.clone();
    omitted_doc.tracks[1].clips[0].enabled = false;
    let (file, timing) = encode_doc(&omitted_doc, BTreeMap::new(), &dir.path("omitted.mp4"));
    let omitted_report = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &frozen_doc,
        &mut no_cards,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        omitted_report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_HIDDEN_CAPTION_OMITTED_FAILS"
    );
    assert!(omitted_report.findings.iter().any(|f| {
        f.element == Some(ClipId(2))
            && matches!(
                f.detail,
                OverlayFindingKind::GlyphMismatch { .. }
                    | OverlayFindingKind::ScrimOrOutlineMismatch { .. }
            )
    }));

    // Defect 1b: caption occluded by a bright solid layer placed above it (proves expected glyph
    // mask is built before occluding layers).
    let mut occluded_doc = frozen_doc.clone();
    occluded_doc.tracks.push(Track {
        id: TrackId(3),
        kind: TrackKind::Video,
        name: None,
        sync_lock: false,
        clips: vec![solid_clip(
            3,
            8,
            SolidColor {
                r: 230,
                g: 236,
                b: 242,
            },
        )],
    });
    let (file, timing) = encode_doc(&occluded_doc, BTreeMap::new(), &dir.path("occluded.mp4"));
    let occluded_report = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &frozen_doc,
        &mut no_cards,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        occluded_report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_OCCLUDED_CAPTION_FAILS"
    );
    assert!(
        occluded_report
            .findings
            .iter()
            .any(|f| f.element == Some(ClipId(2)))
    );
}

#[test]
fn s5_c6_substituted_output_text_after_preflight_fails() {
    let dir = TempDirectory::new("s5-c6-substituted-text");
    let bg = solid_clip(
        1,
        8,
        SolidColor {
            r: 22,
            g: 30,
            b: 44,
        },
    );
    let frozen_caption = title_clip(2, 0, 8, CaptionPreset::Clean.title("Revenue grew ten"));
    let frozen_doc = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![bg],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![frozen_caption],
            },
        ],
        Vec::new(),
        8,
    );
    let expectations = derive_overlay_expectations_for_test(&frozen_doc, &BTreeMap::new());

    // Substitute only the last word after preflight, keeping identical styling, scrim and position.
    let mut substituted_doc = frozen_doc.clone();
    if let ClipContent::Title(title) = &mut substituted_doc.tracks[1].clips[0].content {
        title.text = "Revenue grew two".into();
    }
    let (file, timing) = encode_doc(
        &substituted_doc,
        BTreeMap::new(),
        &dir.path("substituted.mp4"),
    );
    let report = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &frozen_doc,
        &mut no_cards,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();

    assert_eq!(
        report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_SUBSTITUTED_TEXT_FAILS: {report:?}"
    );
    assert!(report.findings.iter().any(|f| {
        f.element == Some(ClipId(2)) && matches!(f.detail, OverlayFindingKind::GlyphMismatch { .. })
    }));
    // Also verify single-digit substitution ("Step 1" -> "Step 2") fails.
    let digit_doc = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    6,
                    SolidColor {
                        r: 22,
                        g: 30,
                        b: 44,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(2, 0, 6, CaptionPreset::Social.title("Step 1"))],
            },
        ],
        Vec::new(),
        6,
    );
    let digit_exp = derive_overlay_expectations_for_test(&digit_doc, &BTreeMap::new());
    let mut digit_sub = digit_doc.clone();
    if let ClipContent::Title(title) = &mut digit_sub.tracks[1].clips[0].content {
        title.text = "Step 2".into();
    }
    let (file, timing) = encode_doc(&digit_sub, BTreeMap::new(), &dir.path("digit-sub.mp4"));
    let digit_report = verify_review_overlays(
        file,
        &timing,
        &digit_exp,
        &digit_doc,
        &mut no_cards,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        digit_report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_SINGLE_DIGIT_SUBSTITUTION_FAILS: {digit_report:?}"
    );
}

#[test]
fn s5_c6_unresolved_expectations_block_and_pinned_card_verifies_pixels() {
    let dir = TempDirectory::new("s5-c6-card-and-unresolved");
    let card_path = dir.path("card.png");
    let wrong_card_path = dir.path("wrong-card.png");
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x2a8844:s=480x270",
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-c:v",
            "png",
        ],
        &card_path,
    );
    run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0xaa2233:s=480x270",
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-c:v",
            "png",
        ],
        &wrong_card_path,
    );

    let pin_file = |path: &Path| -> (CertifiedSource, [u8; 32], u64) {
        let bytes = std::fs::read(path).unwrap();
        let hash = |b: &[u8]| {
            let mut h = kinewright_hash::Sha256::new();
            h.update(b);
            h.finalize()
        };
        let digest = hash(&bytes);
        let chunks: Vec<_> = bytes.chunks(CHUNK_BYTES).map(hash).collect();
        let table = hash(chunks.as_flattened());
        let src = CertifiedSource::verify(
            File::open(path).unwrap(),
            digest,
            bytes.len() as u64,
            table,
            chunks.into(),
            Arc::new(()),
            Arc::new(noop_reserve),
        )
        .unwrap();
        (src, digest, bytes.len() as u64)
    };

    let (src_ok, sha_ok, len_ok) = pin_file(&card_path);
    let (src_wrong, _, _) = pin_file(&wrong_card_path);
    let mut asset = probe_path(&card_path, AssetId(1)).unwrap();
    let Operation::SetAssetColorDescription {
        color_description, ..
    } = kinewright_core::assume_rec709_operation(asset.id, &asset.color_description)
    else {
        unreachable!()
    };
    asset.color_description = color_description;

    let card_clip = Clip {
        id: ClipId(5),
        asset: AssetId(1),
        timeline_start: TimeCode::ZERO,
        source_range: TimeCode::ZERO..TimeCode(6),
        content: ClipContent::Freeze(kinewright_core::FreezeFrame {
            source_frame: TimeCode::ZERO,
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
    };
    let document = document_with(
        vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            name: None,
            sync_lock: false,
            clips: vec![card_clip],
        }],
        vec![asset],
        6,
    );
    let expectations = derive_overlay_expectations_for_test(
        &document,
        &BTreeMap::from([(AssetId(1), (sha_ok, len_ok))]),
    );

    let mut loader = |sha: [u8; 32], len: u64| -> Result<Box<dyn ReviewReadSeek>, MediaError> {
        if sha == sha_ok && len == len_ok {
            File::open(&card_path)
                .map(|file| Box::new(file) as Box<dyn ReviewReadSeek>)
                .map_err(|e| MediaError::Backend(e.to_string()))
        } else {
            Err(MediaError::Backend("unknown pinned card".into()))
        }
    };

    // Clean card passes.
    let (file, timing) = encode_doc(
        &document,
        BTreeMap::from([(AssetId(1), src_ok)]),
        &dir.path("card-ok.mp4"),
    );
    let pass = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &document,
        &mut loader,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(pass.row_result, ReviewRowResult::Pass, "S5_C6_CARD_PASSES");

    // Substituted card fails against the frozen expectations for the original pinned card.
    let mut wrong_asset = probe_path(&wrong_card_path, AssetId(1)).unwrap();
    let Operation::SetAssetColorDescription {
        color_description: wrong_color,
        ..
    } = kinewright_core::assume_rec709_operation(wrong_asset.id, &wrong_asset.color_description)
    else {
        unreachable!()
    };
    wrong_asset.color_description = wrong_color;
    let mut wrong_doc = document.clone();
    wrong_doc.media_pool = vec![wrong_asset];
    let (file, timing) = encode_doc(
        &wrong_doc,
        BTreeMap::from([(AssetId(1), src_wrong)]),
        &dir.path("card-wrong.mp4"),
    );
    let fail = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &document,
        &mut loader,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        fail.row_result,
        ReviewRowResult::Failed,
        "S5_C6_WRONG_CARD_FAILS"
    );

    // Unresolved expectation blocks (ReviewRowResult::Unknown, never Pass).
    let mut unresolved = expectations.clone();
    unresolved.overlays.captions = ReviewInventory::NotDetermined { known: Vec::new() };
    let blocked = verify_review_overlays(
        File::open(dir.path("card-ok.mp4")).unwrap(),
        &timing,
        &unresolved,
        &document,
        &mut loader,
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        blocked.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_UNRESOLVED_BLOCKS"
    );
}
