#![allow(clippy::too_many_lines)]

use std::{collections::BTreeMap, fs::File, path::Path, sync::Arc, time::Instant};

use kinewright_core::{
    AssetId, AutomationCurve, BlendMode, CaptionPreset, Clip, ClipContent, ClipId,
    DeliveryEncodeDepth, DeliveryProfile, Document, Effect, EffectId, ExactMicros,
    ExportCancellation, ExportSettings, FrameRounding, Keyframe, KeyframeInterpolation, MediaAsset,
    MediaKind, ParamValue, Rational, SolidColor, TimeCode, Title, TitlePosition, Track, TrackId,
    TrackKind, map_frames_with_rounding,
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
const STAGING_BYTES: u64 = 16 << 20;

#[allow(clippy::unnecessary_wraps)]
fn noop_reserve(_: u64) -> Result<LifetimeGuard, String> {
    Ok(Arc::new(()))
}

fn embedded_fonts() -> ManifestFonts {
    let bytes = kinewright_core::title_font_bytes();
    let digest = crate::sha256_bytes(bytes);
    ManifestFonts::bind([("font:Inter", digest.as_str(), bytes.len() as u64)])
}

fn admit_inputs(
    document: &Document,
    sources: BTreeMap<AssetId, CertifiedSource>,
) -> Arc<PlaybackInputs> {
    Arc::new(
        PlaybackInputs::admit(
            Arc::new(document.clone()),
            sources,
            Arc::new(LutLibrary::default()),
            Arc::new(()),
            embedded_fonts(),
        )
        .expect("admit playback inputs"),
    )
}

fn encode_doc_with_settings(
    document: &Document,
    sources: BTreeMap<AssetId, CertifiedSource>,
    settings: &ExportSettings,
    out_path: &Path,
) -> (File, ReviewOutputTiming) {
    let inputs = admit_inputs(document, sources);
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
        export_owned_document(&inputs, target, settings, &progress, gpu)
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
    encode_doc_with_settings(document, sources, &settings, out_path)
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

fn document_with_raster(
    raster: (u32, u32),
    tracks: Vec<Track>,
    assets: Vec<MediaAsset>,
    duration: i64,
) -> Document {
    Document {
        resolution: raster,
        fps: Rational::new(24, 1).unwrap(),
        duration: TimeCode(duration),
        media_pool: assets,
        tracks,
        ..Document::default()
    }
}

fn document_with(tracks: Vec<Track>, assets: Vec<MediaAsset>, duration: i64) -> Document {
    document_with_raster(RESOLUTION, tracks, assets, duration)
}

fn derive_overlay_expectations_at_raster(
    document: &Document,
    output_raster: (u32, u32),
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
            if !clip.enabled && clip.enabled_curve.is_none() {
                continue;
            }
            let (kind, text, pixels, base_bounds) = match &clip.content {
                ClipContent::Title(title) => {
                    let kind = if title.caption_preset.is_some() {
                        OverlayKind::Caption
                    } else {
                        OverlayKind::Title
                    };
                    let layout = title_layout(title, output_raster).unwrap();
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
                    let Some(&(sha256, byte_length)) = pinned_cards.get(&clip.asset) else {
                        continue;
                    };
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
                            right: i32::try_from(output_raster.0).unwrap(),
                            bottom: i32::try_from(output_raster.1).unwrap(),
                        },
                    )
                }
                _ => continue,
            };
            let dur = document.clip_duration(clip).unwrap();
            let end = TimeCode(clip.timeline_start.0 + dur.0);
            let mut visible: Vec<OverlayBoundsRun> = Vec::new();
            for frame in clip.timeline_start.0..end.0 {
                let at = map_frames_with_rounding(
                    TimeCode(frame),
                    fps,
                    document.fps,
                    FrameRounding::Floor,
                )
                .unwrap();
                let OverlayGeometry::Visible(bounds) =
                    review_overlay_geometry(document, clip, at, base_bounds, output_raster)
                        .unwrap()
                else {
                    continue;
                };
                let start_t = ExactMicros::frames(TimeCode(frame), fps).unwrap();
                let end_t = ExactMicros::frames(TimeCode(frame + 1), fps).unwrap();
                if let Some(last) = visible
                    .last_mut()
                    .filter(|l| l.bounds == Ok(bounds) && l.output.end == start_t)
                {
                    last.output.end = end_t;
                } else {
                    visible.push(OverlayBoundsRun {
                        output: start_t..end_t,
                        bounds: Ok(bounds),
                    });
                }
            }
            let cue = ExactMicros::frames(clip.timeline_start, fps).unwrap()
                ..ExactMicros::frames(end, fps).unwrap();
            let entry = ReviewOverlay {
                element: clip.id,
                kind,
                cue,
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
        raster: output_raster,
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

fn derive_overlay_expectations_for_test(
    document: &Document,
    pinned_cards: &BTreeMap<AssetId, ([u8; 32], u64)>,
) -> ReviewExpectations {
    derive_overlay_expectations_at_raster(document, document.resolution, pinned_cards)
}

fn certify_image_asset(
    path: &Path,
    asset_id: AssetId,
) -> (MediaAsset, CertifiedSource, [u8; 32], u64) {
    let bytes = std::fs::read(path).unwrap();
    let hex = crate::sha256_bytes(&bytes);
    let mut sha = [0_u8; 32];
    for (i, slot) in sha.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    let len = bytes.len() as u64;
    let mut asset = probe_path(path, asset_id).unwrap();
    let kinewright_core::Operation::SetAssetColorDescription {
        color_description, ..
    } = kinewright_core::assume_rec709_operation(asset.id, &asset.color_description)
    else {
        unreachable!()
    };
    asset.color_description = color_description;
    asset.source_fingerprint.content_sha256 = Some(hex);
    asset.source_fingerprint.byte_len = Some(len);
    let chunks: Vec<[u8; 32]> = if bytes.is_empty() {
        Vec::new()
    } else {
        bytes
            .chunks(CHUNK_BYTES)
            .map(|chunk| {
                let mut h = kinewright_hash::Sha256::new();
                h.update(chunk);
                h.finalize()
            })
            .collect()
    };
    let mut table_hash = kinewright_hash::Sha256::new();
    for chunk in &chunks {
        table_hash.update(chunk);
    }
    let certified = CertifiedSource::verify(
        File::open(path).unwrap(),
        sha,
        len,
        table_hash.finalize(),
        Arc::from(chunks),
        Arc::new(()),
        Arc::new(|_| Ok(Arc::new(()))),
    )
    .unwrap();
    (asset, certified, sha, len)
}

fn write_png_rgba(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    let raw_path = path.with_extension("raw");
    std::fs::write(&raw_path, rgba).unwrap();
    let size = format!("{width}x{height}");
    run_ffmpeg(
        &[
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &size,
            "-i",
            raw_path.to_str().unwrap(),
            "-frames:v",
            "1",
        ],
        path,
    );
    let _ = std::fs::remove_file(&raw_path);
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
    let inputs = admit_inputs(&document, BTreeMap::new());
    let report = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &inputs,
        fallback_gpu().context(),
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
        report.animated_extreme_frames >= 2,
        "animated extremes must be measured: {report:?}"
    );
    assert!(
        report.boundary_checked_frames >= 3,
        "outside-cue timing boundaries must be checked: {report:?}"
    );
    let anim_summary = report
        .overlays
        .iter()
        .find(|o| o.element == ClipId(4))
        .unwrap();
    assert_eq!(anim_summary.checked_frames, 8);
    assert_eq!(anim_summary.status, OverlayElementStatus::Passed);
    assert!(anim_summary.measured_extreme_frames >= 2);
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
    let frozen_inputs = admit_inputs(&frozen_doc, BTreeMap::new());

    // Defect 1a: caption omitted/disabled during encode.
    let mut omitted_doc = frozen_doc.clone();
    omitted_doc.tracks[1].clips[0].enabled = false;
    let (file, timing) = encode_doc(&omitted_doc, BTreeMap::new(), &dir.path("omitted.mp4"));
    let omitted_report = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &frozen_inputs,
        fallback_gpu().context(),
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
                OverlayFindingKind::CompositorReferenceMismatch { .. }
            )
    }));

    // Defect 1b: caption occluded by a bright solid layer added at encode time.
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
        &frozen_inputs,
        fallback_gpu().context(),
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

    // Defect 1c: caption occluded inside the frozen document itself -> unmeasurable -> Unknown, never Pass!
    let doc_occluded_expectations =
        derive_overlay_expectations_for_test(&occluded_doc, &BTreeMap::new());
    let doc_occluded_inputs = admit_inputs(&occluded_doc, BTreeMap::new());
    let (file, timing) = encode_doc(
        &occluded_doc,
        BTreeMap::new(),
        &dir.path("doc-occluded.mp4"),
    );
    let doc_occluded_report = verify_review_overlays(
        file,
        &timing,
        &doc_occluded_expectations,
        &doc_occluded_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        doc_occluded_report.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_DOC_OCCLUDED_IS_UNKNOWN_NEVER_PASS: {doc_occluded_report:?}"
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
    let cases = [
        ("Revenue grew ten", "Revenue grew two", "word"),
        ("Step 1", "Step 2", "digit"),
        // Opus m2: single similar digit inside a long line/number must fail localized tile check.
        ("Total revenue: 1,250", "Total revenue: 1,850", "longnum"),
    ];
    for (frozen_text, sub_text, tag) in cases {
        let frozen_caption = title_clip(2, 0, 8, CaptionPreset::Clean.title(frozen_text));
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
                    clips: vec![frozen_caption],
                },
            ],
            Vec::new(),
            8,
        );
        let expectations = derive_overlay_expectations_for_test(&frozen_doc, &BTreeMap::new());
        let frozen_inputs = admit_inputs(&frozen_doc, BTreeMap::new());

        let mut substituted_doc = frozen_doc.clone();
        if let ClipContent::Title(title) = &mut substituted_doc.tracks[1].clips[0].content {
            title.text = sub_text.into();
        }
        let (file, timing) = encode_doc(
            &substituted_doc,
            BTreeMap::new(),
            &dir.path(&format!("sub-{tag}.mp4")),
        );
        let report = verify_review_overlays(
            file,
            &timing,
            &expectations,
            &frozen_inputs,
            fallback_gpu().context(),
            &OverlayVerificationConfig::default(),
            &ExportCancellation::default(),
        )
        .unwrap();
        assert_eq!(
            report.row_result,
            ReviewRowResult::Failed,
            "S5_C6_SUBSTITUTED_TEXT_FAILS ({tag}: {frozen_text:?} -> {sub_text:?}): {report:?}"
        );
        assert!(report.findings.iter().any(|f| {
            f.element == Some(ClipId(2))
                && matches!(
                    f.detail,
                    OverlayFindingKind::CompositorReferenceMismatch { .. }
                )
        }));
    }
}

#[test]
fn s5_c6_unresolved_expectations_block_and_pinned_card_verifies_pixels() {
    let dir = TempDirectory::new("s5-c6-card-and-unresolved");
    let card_a_path = dir.path("card-a.png");
    let card_b_path = dir.path("card-b.png");

    let mut rgba_a = vec![0_u8; 480 * 270 * 4];
    let mut rgba_b = vec![0_u8; 480 * 270 * 4];
    for y in 0..270_usize {
        for x in 0..480_usize {
            let off = (y * 480 + x) * 4;
            if (100..380).contains(&x) && (70..200).contains(&y) {
                rgba_a[off..off + 4].copy_from_slice(&[220, 40, 40, 255]);
                rgba_b[off..off + 4].copy_from_slice(&[40, 200, 70, 255]);
            }
        }
    }
    write_png_rgba(&card_a_path, 480, 270, &rgba_a);
    write_png_rgba(&card_b_path, 480, 270, &rgba_b);

    let (asset_a, cert_a, sha_a, len_a) = certify_image_asset(&card_a_path, AssetId(1));
    let (asset_b, cert_b, _, _) = certify_image_asset(&card_b_path, AssetId(1));

    let bg = solid_clip(
        1,
        6,
        SolidColor {
            r: 25,
            g: 30,
            b: 40,
        },
    );
    let card_clip = Clip {
        id: ClipId(2),
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

    let doc_a = document_with(
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
                clips: vec![card_clip.clone()],
            },
        ],
        vec![asset_a],
        6,
    );
    let pinned_cards = BTreeMap::from([(AssetId(1), (sha_a, len_a))]);
    let expectations = derive_overlay_expectations_for_test(&doc_a, &pinned_cards);

    // 1. Clean pinned card passes.
    let (file_clean, timing_clean) = encode_doc(
        &doc_a,
        BTreeMap::from([(AssetId(1), cert_a)]),
        &dir.path("card-clean.mp4"),
    );
    let (_, cert_a_verify, _, _) = certify_image_asset(&card_a_path, AssetId(1));
    let inputs_a = admit_inputs(&doc_a, BTreeMap::from([(AssetId(1), cert_a_verify)]));
    let clean_report = verify_review_overlays(
        file_clean,
        &timing_clean,
        &expectations,
        &inputs_a,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        clean_report.row_result,
        ReviewRowResult::Pass,
        "S5_C6_CLEAN_CARD_PASSES: {:?}",
        clean_report.findings
    );

    // 2. Wrong card image encoded fails.
    let mut doc_b = doc_a.clone();
    doc_b.media_pool = vec![asset_b];
    let (file_wrong, timing_wrong) = encode_doc(
        &doc_b,
        BTreeMap::from([(AssetId(1), cert_b)]),
        &dir.path("card-wrong.mp4"),
    );
    let wrong_report = verify_review_overlays(
        file_wrong,
        &timing_wrong,
        &expectations,
        &inputs_a,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        wrong_report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_WRONG_CARD_FAILS: {wrong_report:?}"
    );

    // 3. Unresolved expectation (ImageNotPinned / UpperLayerGeometry) returns Unknown, never Pass.
    let mut unresolved_expectations = expectations.clone();
    if let ReviewInventory::Present { first, .. } = &mut unresolved_expectations.overlays.cards {
        first.pixels = OverlayPixels::Unknown(OverlayUnknown::ImageNotPinned);
    }
    let unresolved_report = verify_review_overlays(
        File::open(dir.path("card-clean.mp4")).unwrap(),
        &timing_clean,
        &unresolved_expectations,
        &inputs_a,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        unresolved_report.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_UNRESOLVED_BLOCKS"
    );
    assert!(
        unresolved_report
            .findings
            .iter()
            .any(|f| matches!(f.detail, OverlayFindingKind::UnresolvedExpectation { .. }))
    );
}

/// Opus M1, m7 & GLM 3, 7 witness:
/// - Social and Minimal outline captions (no scrim) over bright white and high-frequency busy backgrounds pass.
/// - Colour grade adjustment layer above a caption track passes when present and fails when caption hidden.
/// - 10-bit encode (`DeliveryEncodeDepth::Ten`) passes clean and fails substituted.
/// - Scaled output raster (`960x540` doc exported to `640x360`) passes clean and fails omitted.
#[test]
fn s5_c6_fix1_bright_busy_backgrounds_grade_ten_bit_and_scaled_raster() {
    let dir = TempDirectory::new("s5-c6-fix1-real-controls");

    // 1. Busy high-frequency + bright-white background image with Social and Minimal outline captions.
    let busy_path = dir.path("busy-bright.png");
    let mut busy_rgba = vec![0_u8; 480 * 270 * 4];
    for y in 0..270_usize {
        for x in 0..480_usize {
            let off = (y * 480 + x) * 4;
            if y >= 150 {
                // Bright white lower third with sharp alternating high-contrast stripes.
                let bright = ((x / 6) + (y / 6)) % 2 == 0;
                if bright {
                    busy_rgba[off..off + 4].copy_from_slice(&[244, 246, 248, 255]);
                } else {
                    busy_rgba[off..off + 4].copy_from_slice(&[198, 212, 232, 255]);
                }
            } else {
                // Pure bright white top/middle background.
                busy_rgba[off..off + 4].copy_from_slice(&[242, 245, 248, 255]);
            }
        }
    }
    write_png_rgba(&busy_path, 480, 270, &busy_rgba);
    let (busy_asset, busy_cert_enc, _, _) = certify_image_asset(&busy_path, AssetId(1));
    let (_, busy_cert_ver, _, _) = certify_image_asset(&busy_path, AssetId(1));

    let busy_bg_clip = Clip {
        id: ClipId(1),
        asset: AssetId(1),
        timeline_start: TimeCode::ZERO,
        source_range: TimeCode::ZERO..TimeCode(12),
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
    let social = title_clip(2, 1, 5, CaptionPreset::Social.title("Bright white scene"));
    let minimal = title_clip(3, 6, 5, CaptionPreset::Minimal.title("Busy striped shirt"));

    // Add a colour adjustment layer ABOVE both captions shifting brightness down 20% and saturation up.
    let mut grade_clip = Clip {
        id: ClipId(4),
        asset: AssetId(0),
        timeline_start: TimeCode::ZERO,
        source_range: TimeCode::ZERO..TimeCode(12),
        content: ClipContent::Adjustment,
        effects: vec![Effect {
            id: EffectId(20),
            name: "primary_correction".into(),
            enabled: true,
            enabled_curve: None,
            parameters: BTreeMap::from([
                ("exposure_milli_stops".into(), ParamValue::Integer(-500)),
                ("contrast_percent".into(), ParamValue::Integer(15)),
                ("saturation_percent".into(), ParamValue::Integer(20)),
            ]),
            keyframes: BTreeMap::new(),
        }],
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
    // Keep grade on upper track covering frames 0..12.
    grade_clip.enabled = true;

    let doc = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![busy_bg_clip],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![social, minimal],
            },
            Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![grade_clip],
            },
        ],
        vec![busy_asset],
        12,
    );
    // Background clip is NOT a card (do not pass AssetId(1) in pinned_cards so only captions are checked).
    let expectations = derive_overlay_expectations_for_test(&doc, &BTreeMap::new());
    let inputs = admit_inputs(&doc, BTreeMap::from([(AssetId(1), busy_cert_ver)]));

    // 1a. Standard 8-bit delivery-profile encode on bright/busy + upper colour grade passes.
    let (file_8, timing_8) = encode_doc(
        &doc,
        BTreeMap::from([(AssetId(1), busy_cert_enc)]),
        &dir.path("busy-grade-8bit.mp4"),
    );
    let report_8 = verify_review_overlays(
        file_8,
        &timing_8,
        &expectations,
        &inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_8.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX1_BRIGHT_BUSY_GRADED_OUTLINE_PASSES: {:?}",
        report_8.findings
    );

    // 1b. Hiding the graded Minimal caption fails.
    let mut hidden_doc = doc.clone();
    hidden_doc.tracks[1].clips[1].enabled = false;
    let (_, busy_cert_hidden, _, _) = certify_image_asset(&busy_path, AssetId(1));
    let (file_hidden, timing_hidden) = encode_doc(
        &hidden_doc,
        BTreeMap::from([(AssetId(1), busy_cert_hidden)]),
        &dir.path("busy-grade-hidden.mp4"),
    );
    let report_hidden = verify_review_overlays(
        file_hidden,
        &timing_hidden,
        &expectations,
        &inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_hidden.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_BRIGHT_BUSY_GRADED_HIDDEN_FAILS: {report_hidden:?}"
    );

    // 2. 10-bit encode (`DeliveryEncodeDepth::Ten`) passes clean and fails on text substitution.
    let settings_10 = DeliveryProfile::SourceMaster.export_settings(
        &doc,
        DeliveryEncodeDepth::Ten,
        ExportCancellation::default(),
    );
    let (_, busy_cert_10, _, _) = certify_image_asset(&busy_path, AssetId(1));
    let (file_10, timing_10) = encode_doc_with_settings(
        &doc,
        BTreeMap::from([(AssetId(1), busy_cert_10)]),
        &settings_10,
        &dir.path("busy-grade-10bit.mp4"),
    );
    let report_10 = verify_review_overlays(
        file_10,
        &timing_10,
        &expectations,
        &inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_10.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX1_10BIT_PASSES: {:?}",
        report_10.findings
    );

    let mut sub_10_doc = doc.clone();
    if let ClipContent::Title(title) = &mut sub_10_doc.tracks[1].clips[0].content {
        title.text = "Dark black scene".into();
    }
    let (_, busy_cert_10_sub, _, _) = certify_image_asset(&busy_path, AssetId(1));
    let (file_10_sub, timing_10_sub) = encode_doc_with_settings(
        &sub_10_doc,
        BTreeMap::from([(AssetId(1), busy_cert_10_sub)]),
        &settings_10,
        &dir.path("busy-grade-10bit-sub.mp4"),
    );
    let report_10_sub = verify_review_overlays(
        file_10_sub,
        &timing_10_sub,
        &expectations,
        &inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_10_sub.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_10BIT_SUBSTITUTED_FAILS: {report_10_sub:?}"
    );

    // 3. Scaled output raster: 960x540 document exported at 640x360 passes clean and fails omitted.
    let scaled_doc = document_with_raster(
        (960, 540),
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    8,
                    SolidColor {
                        r: 36,
                        g: 52,
                        b: 76,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(
                    2,
                    1,
                    6,
                    CaptionPreset::Clean.title("Scaled export raster"),
                )],
            },
        ],
        Vec::new(),
        8,
    );
    let mut scaled_settings = DeliveryProfile::SourceMaster.export_settings(
        &scaled_doc,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );
    scaled_settings.resolution = (640, 360);
    let scaled_expectations =
        derive_overlay_expectations_at_raster(&scaled_doc, (640, 360), &BTreeMap::new());
    let scaled_inputs = admit_inputs(&scaled_doc, BTreeMap::new());
    let (file_scaled, timing_scaled) = encode_doc_with_settings(
        &scaled_doc,
        BTreeMap::new(),
        &scaled_settings,
        &dir.path("scaled-640x360.mp4"),
    );
    let report_scaled = verify_review_overlays(
        file_scaled,
        &timing_scaled,
        &scaled_expectations,
        &scaled_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_scaled.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX1_SCALED_RASTER_PASSES: {:?}",
        report_scaled.findings
    );

    let mut scaled_omitted = scaled_doc.clone();
    scaled_omitted.tracks[1].clips[0].enabled = false;
    let (file_scaled_bad, timing_scaled_bad) = encode_doc_with_settings(
        &scaled_omitted,
        BTreeMap::new(),
        &scaled_settings,
        &dir.path("scaled-omitted.mp4"),
    );
    let report_scaled_bad = verify_review_overlays(
        file_scaled_bad,
        &timing_scaled_bad,
        &scaled_expectations,
        &scaled_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_scaled_bad.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_SCALED_RASTER_OMITTED_FAILS"
    );
}

/// Opus M2, m7 & GLM 1, 2, 7 witness:
/// - Translucent 50% alpha watermark card passes clean and fails when removed or swapped.
/// - Constant 70% opacity unscrimmed title over bright background passes clean and fails when hidden.
/// - Tiny scaled-down title removed after freeze NEVER passes (Failed or Unknown).
/// - Near-transparent (alpha ~2%) unmeasurable title returns Unknown, never Pass.
#[test]
fn s5_c6_fix1_translucent_partial_opacity_and_tiny_unmeasurable_overlays() {
    let dir = TempDirectory::new("s5-c6-fix1-translucent-tiny");

    // 1. Translucent 50% alpha watermark card (alpha = 128 everywhere).
    let wm_a = dir.path("watermark-a.png");
    let wm_b = dir.path("watermark-b.png");
    let mut rgba_a = vec![0_u8; 480 * 270 * 4];
    let mut rgba_b = vec![0_u8; 480 * 270 * 4];
    for y in 40..140_usize {
        for x in 140..340_usize {
            let off = (y * 480 + x) * 4;
            rgba_a[off..off + 4].copy_from_slice(&[240, 40, 40, 128]);
            rgba_b[off..off + 4].copy_from_slice(&[30, 210, 60, 128]);
        }
    }
    write_png_rgba(&wm_a, 480, 270, &rgba_a);
    write_png_rgba(&wm_b, 480, 270, &rgba_b);

    let (asset_a, cert_a_enc, sha_a, len_a) = certify_image_asset(&wm_a, AssetId(1));
    let (_, cert_a_ver, _, _) = certify_image_asset(&wm_a, AssetId(1));
    let (asset_b, swapped_card_cert, _, _) = certify_image_asset(&wm_b, AssetId(1));

    let wm_clip = Clip {
        id: ClipId(2),
        asset: AssetId(1),
        timeline_start: TimeCode(1),
        source_range: TimeCode::ZERO..TimeCode(5),
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
    let doc_wm = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    7,
                    SolidColor {
                        r: 30,
                        g: 40,
                        b: 60,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![wm_clip],
            },
        ],
        vec![asset_a],
        7,
    );
    let wm_exp = derive_overlay_expectations_for_test(
        &doc_wm,
        &BTreeMap::from([(AssetId(1), (sha_a, len_a))]),
    );
    let wm_inputs = admit_inputs(&doc_wm, BTreeMap::from([(AssetId(1), cert_a_ver)]));

    let (file_wm_ok, timing_wm_ok) = encode_doc(
        &doc_wm,
        BTreeMap::from([(AssetId(1), cert_a_enc)]),
        &dir.path("wm-ok.mp4"),
    );
    let report_wm_ok = verify_review_overlays(
        file_wm_ok,
        &timing_wm_ok,
        &wm_exp,
        &wm_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_wm_ok.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX1_TRANSLUCENT_CARD_PASSES: {:?}",
        report_wm_ok.findings
    );

    let mut doc_wm_removed = doc_wm.clone();
    doc_wm_removed.tracks[1].clips[0].enabled = false;
    let (_, cert_a_rem, _, _) = certify_image_asset(&wm_a, AssetId(1));
    let (file_wm_rem, timing_wm_rem) = encode_doc(
        &doc_wm_removed,
        BTreeMap::from([(AssetId(1), cert_a_rem)]),
        &dir.path("wm-removed.mp4"),
    );
    let report_wm_rem = verify_review_overlays(
        file_wm_rem,
        &timing_wm_rem,
        &wm_exp,
        &wm_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_wm_rem.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_TRANSLUCENT_CARD_REMOVED_FAILS"
    );

    let mut doc_wm_swapped = doc_wm.clone();
    doc_wm_swapped.media_pool = vec![asset_b];
    let (file_wm_swap, timing_wm_swap) = encode_doc(
        &doc_wm_swapped,
        BTreeMap::from([(AssetId(1), swapped_card_cert)]),
        &dir.path("wm-swapped.mp4"),
    );
    let report_wm_swap = verify_review_overlays(
        file_wm_swap,
        &timing_wm_swap,
        &wm_exp,
        &wm_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_wm_swap.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_TRANSLUCENT_CARD_SWAPPED_FAILS"
    );

    // 2. Constant 70% opacity unscrimmed title over bright background: clean passes, hidden fails.
    let mut partial_title = title_clip(2, 1, 5, CaptionPreset::Social.title("Lower third 70%"));
    partial_title.effects.push(Effect {
        id: EffectId(30),
        name: "opacity".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([("percent".into(), ParamValue::Integer(70))]),
        keyframes: BTreeMap::new(),
    });
    let doc_partial = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    7,
                    SolidColor {
                        r: 235,
                        g: 238,
                        b: 242,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![partial_title],
            },
        ],
        Vec::new(),
        7,
    );
    let partial_exp = derive_overlay_expectations_for_test(&doc_partial, &BTreeMap::new());
    let partial_inputs = admit_inputs(&doc_partial, BTreeMap::new());
    let (file_p_ok, timing_p_ok) = encode_doc(&doc_partial, BTreeMap::new(), &dir.path("p-ok.mp4"));
    let rep_p_ok = verify_review_overlays(
        file_p_ok,
        &timing_p_ok,
        &partial_exp,
        &partial_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_p_ok.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX1_PARTIAL_OPACITY_PASSES: {:?}",
        rep_p_ok.findings
    );

    let mut doc_p_hidden = doc_partial.clone();
    doc_p_hidden.tracks[1].clips[0].enabled = false;
    let (file_p_hid, timing_p_hid) =
        encode_doc(&doc_p_hidden, BTreeMap::new(), &dir.path("p-hid.mp4"));
    let rep_p_hid = verify_review_overlays(
        file_p_hid,
        &timing_p_hid,
        &partial_exp,
        &partial_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_p_hid.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_PARTIAL_OPACITY_HIDDEN_FAILS: {rep_p_hid:?}"
    );

    // 3. Tiny scaled-down title (18% scale) removed after freeze must NEVER pass (Failed or Unknown).
    let mut tiny_title = title_clip(
        2,
        1,
        5,
        Title {
            text: "Tiny corner".into(),
            font_size_token: 0,
            color_token: 0,
            position: TitlePosition::Top,
            background_scrim: false,
            fade_in_frames: TimeCode::ZERO,
            fade_out_frames: TimeCode::ZERO,
            caption_preset: None,
        },
    );
    tiny_title.effects.push(Effect {
        id: EffectId(31),
        name: "transform".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([("scale_percent".into(), ParamValue::Integer(18))]),
        keyframes: BTreeMap::new(),
    });
    let doc_tiny = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    7,
                    SolidColor {
                        r: 30,
                        g: 40,
                        b: 55,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![tiny_title],
            },
        ],
        Vec::new(),
        7,
    );
    let tiny_exp = derive_overlay_expectations_for_test(&doc_tiny, &BTreeMap::new());
    let tiny_inputs = admit_inputs(&doc_tiny, BTreeMap::new());
    let mut doc_tiny_removed = doc_tiny.clone();
    doc_tiny_removed.tracks[1].clips[0].enabled = false;
    let (file_tiny, timing_tiny) = encode_doc(
        &doc_tiny_removed,
        BTreeMap::new(),
        &dir.path("tiny-rem.mp4"),
    );
    let rep_tiny = verify_review_overlays(
        file_tiny,
        &timing_tiny,
        &tiny_exp,
        &tiny_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_ne!(
        rep_tiny.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX1_TINY_REMOVED_NEVER_PASSES: {rep_tiny:?}"
    );

    // 4. Near-transparent (2% opacity) title is unmeasurable -> Unknown, never Pass!
    let mut faint_title = title_clip(2, 1, 5, CaptionPreset::Clean.title("Almost invisible"));
    faint_title.effects.push(Effect {
        id: EffectId(32),
        name: "opacity".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([("percent".into(), ParamValue::Integer(2))]),
        keyframes: BTreeMap::new(),
    });
    let doc_faint = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    7,
                    SolidColor {
                        r: 30,
                        g: 40,
                        b: 55,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![faint_title],
            },
        ],
        Vec::new(),
        7,
    );
    let faint_exp = derive_overlay_expectations_for_test(&doc_faint, &BTreeMap::new());
    let faint_inputs = admit_inputs(&doc_faint, BTreeMap::new());
    let (file_faint, timing_faint) =
        encode_doc(&doc_faint, BTreeMap::new(), &dir.path("faint.mp4"));
    let rep_faint = verify_review_overlays(
        file_faint,
        &timing_faint,
        &faint_exp,
        &faint_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_faint.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_FIX1_FAINT_UNMEASURABLE_IS_UNKNOWN: {rep_faint:?}"
    );
    assert!(
        rep_faint
            .findings
            .iter()
            .any(|f| matches!(f.detail, OverlayFindingKind::UnmeasurableOverlay { .. }))
    );
}

/// L14 timing, Opus m1 & GLM 4 witness: a caption shown early (`start - 1`) or lingering
/// (`end`) fails outside-cue timing verification.
#[test]
fn s5_c6_fix1_early_and_lingering_overlays_fail_outside_cue_timing() {
    let dir = TempDirectory::new("s5-c6-fix1-timing");
    let bg = solid_clip(
        1,
        10,
        SolidColor {
            r: 24,
            g: 40,
            b: 64,
        },
    );
    let cue = title_clip(2, 3, 4, CaptionPreset::Clean.title("Exact cue window"));
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
                clips: vec![cue],
            },
        ],
        Vec::new(),
        10,
    );
    let expectations = derive_overlay_expectations_for_test(&frozen_doc, &BTreeMap::new());
    let frozen_inputs = admit_inputs(&frozen_doc, BTreeMap::new());

    // Early start (starts at frame 2 instead of 3, keeps frames 3..7 present).
    let mut early_doc = frozen_doc.clone();
    early_doc.tracks[1].clips[0].timeline_start = TimeCode(2);
    early_doc.tracks[1].clips[0].source_range = TimeCode::ZERO..TimeCode(5);
    let (file_early, timing_early) =
        encode_doc(&early_doc, BTreeMap::new(), &dir.path("early.mp4"));
    let report_early = verify_review_overlays(
        file_early,
        &timing_early,
        &expectations,
        &frozen_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_early.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_EARLY_CAPTION_FAILS: {report_early:?}"
    );
    assert!(report_early.findings.iter().any(|f| {
        f.frame_index == Some(2)
            && matches!(
                f.detail,
                OverlayFindingKind::UnexpectedOverlayOutsideCue { .. }
            )
    }));

    // Lingering end (extends to frame 8 instead of 7, keeps frames 3..7 present).
    let mut late_doc = frozen_doc.clone();
    late_doc.tracks[1].clips[0].source_range = TimeCode::ZERO..TimeCode(5);
    let (file_late, timing_late) = encode_doc(&late_doc, BTreeMap::new(), &dir.path("late.mp4"));
    let report_late = verify_review_overlays(
        file_late,
        &timing_late,
        &expectations,
        &frozen_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        report_late.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_LINGERING_CAPTION_FAILS: {report_late:?}"
    );
    assert!(report_late.findings.iter().any(|f| {
        f.frame_index == Some(7)
            && matches!(
                f.detail,
                OverlayFindingKind::UnexpectedOverlayOutsideCue { .. }
            )
    }));
}

/// Opus M3 & m6 witness: unknown PTS step yields Unknown, shifted/dropped frame fails, and
/// NaN or out-of-range configuration is refused.
#[test]
fn s5_c6_fix1_unknown_frame_step_and_invalid_config_never_pass() {
    let dir = TempDirectory::new("s5-c6-fix1-step-config");
    let doc = document_with(
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
                        r: 20,
                        g: 30,
                        b: 45,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(
                    2,
                    1,
                    4,
                    CaptionPreset::Clean.title("Step check"),
                )],
            },
        ],
        Vec::new(),
        6,
    );
    let expectations = derive_overlay_expectations_for_test(&doc, &BTreeMap::new());
    let inputs = admit_inputs(&doc, BTreeMap::new());
    let out_path = dir.path("step.mp4");
    let (_, timing) = encode_doc(&doc, BTreeMap::new(), &out_path);

    // 1. Unknown video_pts_step -> Unknown (never silently skips PTS verification).
    let mut unknown_step = timing.clone();
    unknown_step.video_pts_step = None;
    let rep_unknown = verify_review_overlays(
        File::open(&out_path).unwrap(),
        &unknown_step,
        &expectations,
        &inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_unknown.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_FIX1_UNKNOWN_STEP_IS_UNKNOWN: {rep_unknown:?}"
    );

    // 2. Shifted/mismatched PTS grid (simulates dropped/duplicated frame shift) -> Failed.
    let mut shifted = timing.clone();
    shifted.video_pts_step = shifted.video_pts_step.map(|s| s * 2);
    let rep_shifted = verify_review_overlays(
        File::open(&out_path).unwrap(),
        &shifted,
        &expectations,
        &inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_shifted.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX1_SHIFTED_PTS_FAILS: {rep_shifted:?}"
    );
    assert!(
        rep_shifted
            .findings
            .iter()
            .any(|f| matches!(f.detail, OverlayFindingKind::TimingMismatch { .. }))
    );

    // 3. Invalid config (NaN and out-of-range) is refused before decoding.
    let nan_cfg = OverlayVerificationConfig {
        max_codec_error_rms: f32::NAN,
        ..OverlayVerificationConfig::default()
    };
    assert!(
        verify_review_overlays(
            File::open(&out_path).unwrap(),
            &timing,
            &expectations,
            &inputs,
            fallback_gpu().context(),
            &nan_cfg,
            &ExportCancellation::default(),
        )
        .is_err(),
        "S5_C6_FIX1_NAN_CONFIG_REFUSED"
    );

    let huge_cfg = OverlayVerificationConfig {
        max_codec_error_rms: 255.0,
        ..OverlayVerificationConfig::default()
    };
    assert!(
        verify_review_overlays(
            File::open(&out_path).unwrap(),
            &timing,
            &expectations,
            &inputs,
            fallback_gpu().context(),
            &huge_cfg,
            &ExportCancellation::default(),
        )
        .is_err(),
        "S5_C6_FIX1_OUT_OF_RANGE_CONFIG_REFUSED"
    );
}

#[cfg(target_os = "linux")]
fn current_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmRSS:")?.trim();
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
}

/// Opus B1 & L14 witness: caption-heavy 1080p fixture measures wall-clock cost and proves O(1)
/// memory (no per-element full-frame buffers kept across 60 distinct 1080p captions).
#[test]
#[allow(clippy::cast_precision_loss)]
fn s5_c6_fix1_caption_heavy_1080p_cost_and_bounded_memory() {
    let dir = TempDirectory::new("s5-c6-fix1-1080p-cost");
    let caption_count = 60_usize;
    let total_frames = i64::try_from(caption_count).unwrap() + 2;
    let mut caption_clips = Vec::with_capacity(caption_count);
    for i in 0..caption_count {
        let i64_idx = i64::try_from(i).unwrap();
        let u64_idx = u64::try_from(i).unwrap();
        caption_clips.push(title_clip(
            100 + u64_idx,
            1 + i64_idx,
            1,
            CaptionPreset::Clean.title(format!("1080p caption line {}", i + 1)),
        ));
    }
    let doc = document_with_raster(
        (1920, 1080),
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    total_frames,
                    SolidColor {
                        r: 26,
                        g: 38,
                        b: 58,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: caption_clips,
            },
        ],
        Vec::new(),
        total_frames,
    );
    let expectations = derive_overlay_expectations_for_test(&doc, &BTreeMap::new());
    let out_path = dir.path("heavy-1080p.mp4");
    let (file, timing) = encode_doc(&doc, BTreeMap::new(), &out_path);
    let inputs = admit_inputs(&doc, BTreeMap::new());

    #[cfg(target_os = "linux")]
    let rss_before_kib = current_rss_kib().unwrap_or(0);
    let started = Instant::now();
    let report = verify_review_overlays(
        file,
        &timing,
        &expectations,
        &inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .expect("1080p overlay verification");
    let elapsed = started.elapsed();
    #[cfg(target_os = "linux")]
    let rss_after_kib = current_rss_kib().unwrap_or(0);
    #[cfg(target_os = "linux")]
    let rss_delta_mib = rss_after_kib.saturating_sub(rss_before_kib) as f64 / 1024.0;
    #[cfg(not(target_os = "linux"))]
    let rss_delta_mib = 0.0_f64;

    println!(
        "S5_C6_FIX1_1080P_COST captions={caption_count} frames={total_frames} checked_frames={} boundary_frames={} elapsed_ms={} ms_per_frame={:.1} rss_delta_mib={:.1}",
        report.checked_frames,
        report.boundary_checked_frames,
        elapsed.as_millis(),
        elapsed.as_secs_f64() * 1000.0 / total_frames as f64,
        rss_delta_mib,
    );
    assert_eq!(
        report.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX1_1080P_PASSES: {:?}",
        report.findings
    );
    assert_eq!(report.checked_overlays, caption_count);
    assert_eq!(report.checked_frames, u64::try_from(caption_count).unwrap());
    // 60 full 1080p RGBA+mask buffers under the old design would require ~622 MiB up front;
    // O(1) frame-by-frame rendering stays far below that ceiling.
    #[cfg(target_os = "linux")]
    assert!(
        rss_delta_mib < 260.0,
        "S5_C6_FIX1_1080P_MEMORY_BOUNDED: rss_delta_mib={rss_delta_mib:.1}"
    );
}
