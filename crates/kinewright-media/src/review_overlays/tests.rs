#![allow(clippy::too_many_lines)]

use std::{collections::BTreeMap, fs::File, path::Path, sync::Arc};

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
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(clippy::cast_precision_loss)]
fn s5_c6_fix1_caption_heavy_1080p_cost_and_bounded_memory() {
    use std::time::Instant;

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
    let rss_after_kib = current_rss_kib().unwrap_or(0);
    let rss_delta_mib = rss_after_kib.saturating_sub(rss_before_kib) as f64 / 1024.0;

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
    assert!(
        rss_delta_mib < 260.0,
        "S5_C6_FIX1_1080P_MEMORY_BOUNDED: rss_delta_mib={rss_delta_mib:.1}"
    );
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::similar_names
)]
fn generate_moving_grainy_mp4(path: &Path, width: u32, height: u32, frames: u32) {
    use std::io::Write as _;

    let raw_path = path.with_extension("yuv");
    let w = width as usize;
    let h = height as usize;
    let cw = w / 2;
    let ch = h / 2;
    let mut file = std::io::BufWriter::new(File::create(&raw_path).unwrap());
    let mut y_plane = vec![0_u8; w * h];
    let mut u_plane = vec![128_u8; cw * ch];
    let mut v_plane = vec![128_u8; cw * ch];
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15 ^ (u64::from(width) << 32) ^ u64::from(height);
    let mut next_u32 = || -> u32 {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 32) as u32
    };

    for f in 0..frames {
        let fi = f as usize;
        // Realistic handheld camera drift + moving subject + textured mid-frequency background
        // (foliage/window gradients and patterned shirt) plus triangular camera film grain so
        // libx264 at Youtube1080p (8 Mbps) / VerticalShort (10 Mbps) hits steady-state delivery
        // compression without degenerating into uncompressible white-noise QP=51 meltdown.
        let pan_x = (fi as i64) * 5;
        let pan_y = ((fi as i64) * 3) / 2;
        let head_cx = (w / 2) as i64 + (((fi as i64) * 7) % 140) - 70;
        let head_cy = (h / 2) as i64 + (((fi as i64) * 3) % 60) - 30;
        let head_rx = (w / 6) as i64;
        let head_ry = (h / 4) as i64;
        let head_rx2 = head_rx * head_rx;
        let head_ry2 = head_ry * head_ry;
        let head_rhs = head_rx2 * head_ry2;

        // Precompute smooth periodic luma wave tables for realistic moving texture.
        let tri = |v: i64, period: i64| -> i32 {
            let m = v.rem_euclid(period);
            let half = period / 2;
            let tri_val = if m < half { m } else { period - m };
            ((tri_val * 255) / half.max(1)) as i32
        };

        for y in 0..h {
            let row = y * w;
            let sy = y as i64 + pan_y;
            let bg_v = tri(sy, 180);
            let leaf_y = tri(sy * 3 + (fi as i64) * 4, 64);
            let ry = y as i64 - head_cy;
            let ry_term = ry * ry * head_rx2;
            for x in 0..w {
                let sx = x as i64 + pan_x;
                let bg_h = tri(sx, 240);
                let leaf_x = tri(sx * 2 + sy, 72);
                // Natural mid-frequency sunlit foliage / interior lighting variation (luma ~55..195).
                let mut luma = 58 + (bg_h + bg_v) / 6 + (leaf_x + leaf_y) / 10;

                // Moving talking-head subject with subtle fabric weave over the lower-middle area.
                let rx = x as i64 - head_cx;
                if rx * rx * head_ry2 + ry_term < head_rhs {
                    let weave = tri(sx * 2 - sy, 36);
                    luma = 82 + weave / 4;
                } else if y > (h * 2) / 3 && (x as i64 - head_cx).abs() < head_rx * 2 {
                    // Patterned shirt behind the caption zone (busy area).
                    let stripe = tri((sx + sy) * 2, 28);
                    luma = 64 + stripe / 3;
                }

                // Triangular-distributed temporal+spatial camera grain (~±9 luma, sigma ~4.0).
                let g1 = (next_u32() % 11) as i32;
                let g2 = (next_u32() % 11) as i32;
                let grain = g1 - g2;
                y_plane[row + x] = (luma + grain).clamp(16, 235) as u8;
            }
        }
        for y in 0..ch {
            let row = y * cw;
            let sy = (y as i64) * 2 + pan_y;
            for x in 0..cw {
                let sx = (x as i64) * 2 + pan_x;
                let cu_wave = tri(sx + sy / 2, 200) / 8;
                let cv_wave = tri(sx / 2 + sy, 220) / 8;
                let cg1 = (next_u32() % 5) as i32 - (next_u32() % 5) as i32;
                let cg2 = (next_u32() % 5) as i32 - (next_u32() % 5) as i32;
                u_plane[row + x] = (112 + cu_wave + cg1).clamp(16, 240) as u8;
                v_plane[row + x] = (140 - cv_wave + cg2).clamp(16, 240) as u8;
            }
        }
        file.write_all(&y_plane).unwrap();
        file.write_all(&u_plane).unwrap();
        file.write_all(&v_plane).unwrap();
    }
    file.flush().unwrap();
    drop(file);

    let size = format!("{width}x{height}");
    run_ffmpeg(
        &[
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-s",
            &size,
            "-r",
            "24",
            "-i",
            raw_path.to_str().unwrap(),
            "-threads",
            "2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "12",
            "-pix_fmt",
            "yuv420p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv",
        ],
        path,
    );
    let _ = std::fs::remove_file(&raw_path);
}

fn certify_video_asset(path: &Path, asset_id: AssetId) -> (MediaAsset, CertifiedSource) {
    let bytes = std::fs::read(path).unwrap();
    let hex = crate::sha256_bytes(&bytes);
    let mut sha = [0_u8; 32];
    for (i, slot) in sha.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    let len = bytes.len() as u64;
    let mut asset = probe_path(path, asset_id).unwrap();
    asset.source_fingerprint.content_sha256 = Some(hex);
    asset.source_fingerprint.byte_len = Some(len);
    let chunks: Vec<[u8; 32]> = bytes
        .chunks(CHUNK_BYTES)
        .map(|chunk| {
            let mut h = kinewright_hash::Sha256::new();
            h.update(chunk);
            h.finalize()
        })
        .collect();
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
    (asset, certified)
}

/// Fix round 2 (Opus M1 & GLM R1) witnesses:
/// 1. A slide-and-fade lower third shown 1 second early (and 1 frame early) fails outside-cue
///    timing verification while its clean control passes.
/// 2. A caption lingering at ~30% opacity on `cue_end` fails (no longer false-passes).
/// 3. An outside-cue probe that cannot be measured (white unscrimmed title ending on a cut to
///    a pure-white frame) records `UnmeasurableBoundaryProbe` and yields `Unknown`, never `Pass`.
#[test]
fn s5_c6_fix2_outside_cue_slide_fade_early_ghost_and_unmeasurable_probe() {
    let dir = TempDirectory::new("s5-c6-fix2-outside-cue");

    // 1. Slide-and-fade lower third: 72 frames (3.0s at 24fps), cue at frames 36..64.
    let slide_transform = Effect {
        id: EffectId(10),
        name: "transform".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::from([(
            "x_percent".into(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: -25,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(12),
                        value: 0,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ],
            },
        )]),
    };
    let lower_third = Title {
        text: "Dr. Morgan Vance — Lead Architect".into(),
        font_size_token: 1,
        color_token: 0,
        position: TitlePosition::LowerThird,
        background_scrim: true,
        fade_in_frames: TimeCode(12),
        fade_out_frames: TimeCode(8),
        caption_preset: None,
    };
    let mut lower_clip = title_clip(2, 36, 28, lower_third);
    lower_clip.effects.push(slide_transform);

    let frozen_slide = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    72,
                    SolidColor {
                        r: 26,
                        g: 42,
                        b: 68,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![lower_clip],
            },
        ],
        Vec::new(),
        72,
    );
    let slide_expectations = derive_overlay_expectations_for_test(&frozen_slide, &BTreeMap::new());
    let slide_inputs = admit_inputs(&frozen_slide, BTreeMap::new());

    // 1a. Clean slide-and-fade lower third passes with zero unmeasured boundary probes.
    let (clean_slide_file, clean_slide_timing) =
        encode_doc(&frozen_slide, BTreeMap::new(), &dir.path("slide-clean.mp4"));
    let rep_clean_slide = verify_review_overlays(
        clean_slide_file,
        &clean_slide_timing,
        &slide_expectations,
        &slide_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_clean_slide.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX2_SLIDE_FADE_CLEAN_PASSES: {:?}",
        rep_clean_slide.findings
    );
    assert_eq!(
        rep_clean_slide.boundary_unmeasured_frames, 0,
        "S5_C6_FIX2_SLIDE_FADE_BOUNDARIES_MEASURED"
    );
    assert!(
        rep_clean_slide.boundary_checked_frames >= 2,
        "S5_C6_FIX2_SLIDE_FADE_BOUNDARIES_CHECKED"
    );

    // 1b. Same slide-and-fade lower third shown 1 second (24 frames) early -> MUST FAIL at frame 35.
    let mut early_1s_doc = frozen_slide.clone();
    early_1s_doc.tracks[1].clips[0].timeline_start = TimeCode(12);
    early_1s_doc.tracks[1].clips[0].source_range = TimeCode::ZERO..TimeCode(52);
    let (early_1s_file, early_1s_timing) = encode_doc(
        &early_1s_doc,
        BTreeMap::new(),
        &dir.path("slide-early-1s.mp4"),
    );
    let rep_early_1s = verify_review_overlays(
        early_1s_file,
        &early_1s_timing,
        &slide_expectations,
        &slide_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_early_1s.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX2_SLIDE_FADE_1S_EARLY_FAILS: {rep_early_1s:?}"
    );
    // With fade_in_frames=12, local frame 0 (frame 36) has title_alpha=0 (Hidden), so the visible
    // cue begins at frame 37 and its outside-cue start probe runs on frame 36.
    assert!(
        rep_early_1s.findings.iter().any(|f| {
            f.frame_index == Some(36)
                && matches!(
                    f.detail,
                    OverlayFindingKind::UnexpectedOverlayOutsideCue { .. }
                        | OverlayFindingKind::OutsideCueReferenceMismatch { .. }
                )
        }),
        "S5_C6_FIX2_SLIDE_FADE_1S_EARLY_AT_PROBE_FRAME: {:?}",
        rep_early_1s.findings
    );

    // 2. Caption lingering at ~30% opacity on `cue_end` (Opus M1 scenario) -> MUST FAIL.
    let cap_title = CaptionPreset::Clean.title("Closing line");
    let frozen_ghost = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    14,
                    SolidColor {
                        r: 22,
                        g: 34,
                        b: 56,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(2, 4, 6, cap_title.clone())],
            },
        ],
        Vec::new(),
        14,
    );
    let ghost_expectations = derive_overlay_expectations_for_test(&frozen_ghost, &BTreeMap::new());
    let ghost_inputs = admit_inputs(&frozen_ghost, BTreeMap::new());

    let (clean_ghost_file, clean_ghost_timing) =
        encode_doc(&frozen_ghost, BTreeMap::new(), &dir.path("ghost-clean.mp4"));
    let rep_clean_ghost = verify_review_overlays(
        clean_ghost_file,
        &clean_ghost_timing,
        &ghost_expectations,
        &ghost_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_clean_ghost.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX2_GHOST_CLEAN_PASSES: {:?}",
        rep_clean_ghost.findings
    );

    // Add a 1-frame 30%-opacity lingering ghost of the caption on `cue_end` (frame 10).
    let mut ghost_doc = frozen_ghost.clone();
    let mut ghost_clip = title_clip(99, 10, 1, cap_title);
    ghost_clip.effects.push(Effect {
        id: EffectId(20),
        name: "opacity".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([("percent".into(), ParamValue::Integer(30))]),
        keyframes: BTreeMap::new(),
    });
    ghost_doc.tracks[1].clips.push(ghost_clip);
    let (ghost_file, ghost_timing) =
        encode_doc(&ghost_doc, BTreeMap::new(), &dir.path("ghost-30pct.mp4"));
    let rep_ghost = verify_review_overlays(
        ghost_file,
        &ghost_timing,
        &ghost_expectations,
        &ghost_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_ghost.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX2_30PCT_GHOST_ON_CUE_END_FAILS: {rep_ghost:?}"
    );
    assert!(
        rep_ghost.findings.iter().any(|f| {
            f.frame_index == Some(10)
                && matches!(
                    f.detail,
                    OverlayFindingKind::UnexpectedOverlayOutsideCue { .. }
                )
        }),
        "S5_C6_FIX2_30PCT_GHOST_FINDING_AT_CUE_END: {:?}",
        rep_ghost.findings
    );

    // 3. Unmeasurable boundary probe: title on Track 2 over dark navy on frames 2..8 is 100%
    //    measurable in range, while an opaque full-frame solid clip on Track 3 (above Track 2)
    //    begins at frame 8 (`cue_end`). Injecting Track 2 at its document z-order on frame 8 is
    //    completely occluded by Track 3 (`ref_rms == 0`), so the `cue_end` probe is NotMeasurable
    //    and MUST yield `Unknown` with `UnmeasurableBoundaryProbe` (never `Pass`).
    let mut upper_occluder = solid_clip(
        4,
        4,
        SolidColor {
            r: 40,
            g: 46,
            b: 60,
        },
    );
    upper_occluder.timeline_start = TimeCode(8);
    let clean_title = Title {
        text: "Occluded At Cue End".into(),
        font_size_token: 1,
        color_token: 0,
        position: TitlePosition::Center,
        background_scrim: false,
        fade_in_frames: TimeCode::ZERO,
        fade_out_frames: TimeCode::ZERO,
        caption_preset: None,
    };
    let unmeas_probe_doc = document_with(
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![solid_clip(
                    1,
                    12,
                    SolidColor {
                        r: 18,
                        g: 30,
                        b: 54,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(3, 2, 6, clean_title)],
            },
            Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![upper_occluder],
            },
        ],
        Vec::new(),
        12,
    );
    let unmeas_expectations =
        derive_overlay_expectations_for_test(&unmeas_probe_doc, &BTreeMap::new());
    let unmeas_inputs = admit_inputs(&unmeas_probe_doc, BTreeMap::new());
    let (unmeas_file, unmeas_timing) = encode_doc(
        &unmeas_probe_doc,
        BTreeMap::new(),
        &dir.path("unmeas-probe.mp4"),
    );
    let rep_unmeas = verify_review_overlays(
        unmeas_file,
        &unmeas_timing,
        &unmeas_expectations,
        &unmeas_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        rep_unmeas.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_FIX2_UNMEASURABLE_PROBE_IS_UNKNOWN: {rep_unmeas:?}"
    );
    assert_eq!(
        rep_unmeas.overlays[0].status,
        OverlayElementStatus::Unknown,
        "S5_C6_FIX2_UNMEASURABLE_PROBE_ELEMENT_UNKNOWN"
    );
    assert_eq!(
        rep_unmeas.overlays[0].measurable_frames, 6,
        "S5_C6_FIX2_IN_RANGE_ALL_MEASURED"
    );
    assert_eq!(
        rep_unmeas.boundary_unmeasured_frames, 1,
        "S5_C6_FIX2_ONE_UNMEASURED_BOUNDARY"
    );
    assert!(
        rep_unmeas.findings.iter().any(|f| {
            f.frame_index == Some(8)
                && matches!(
                    f.detail,
                    OverlayFindingKind::UnmeasurableBoundaryProbe { .. }
                )
        }),
        "S5_C6_FIX2_UNMEASURABLE_BOUNDARY_FINDING: {:?}",
        rep_unmeas.findings
    );
}

/// Fix round 2 (Opus M2 & GLM R2) delivery-bitrate calibration witness:
/// Procedurally generated moving grainy footage encoded at the product's `Youtube1080p` (8 Mbps,
/// 1920x1080) and `VerticalShort` (10 Mbps, 1080x1920) profiles with an outline caption over the
/// busy moving area, a faint/translucent card, and a slide-and-fade lower third.
/// Clean encodes must Pass; seeded missing caption, one-digit change, early start, and lingering
/// 30% ghost on the same `Youtube1080p` footage must all Fail.
#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(clippy::cast_precision_loss)]
fn s5_c6_fix2_delivery_bitrate_moving_grain_calibration() {
    let dir = TempDirectory::new("s5-c6-fix2-delivery-calib");

    // Create a faint/translucent badge card PNG (45% alpha fill + 55% alpha border) so it tests
    // translucent overlays at delivery bitrate while leaving moving-grain tiles outside its box.
    let card_w = 480_u32;
    let card_h = 270_u32;
    let mut card_rgba = vec![0_u8; (card_w * card_h * 4) as usize];
    for y in 0..card_h {
        for x in 0..card_w {
            let i = ((y * card_w + x) * 4) as usize;
            let border = x < 8 || x >= card_w - 8 || y < 8 || y >= card_h - 8;
            if border {
                card_rgba[i] = 240;
                card_rgba[i + 1] = 220;
                card_rgba[i + 2] = 120;
                card_rgba[i + 3] = 148; // ~58% alpha crisp border
            } else {
                card_rgba[i] = 20;
                card_rgba[i + 1] = 36;
                card_rgba[i + 2] = 74;
                card_rgba[i + 3] = 115; // ~45% alpha translucent body
            }
        }
    }
    let card_path = dir.path("translucent-badge.png");
    write_png_rgba(&card_path, card_w, card_h, &card_rgba);
    let (card_asset, card_cert, card_sha, card_len) = certify_image_asset(&card_path, AssetId(2));

    let make_card_clip = |start: i64, dur: i64| Clip {
        id: ClipId(20),
        asset: AssetId(2),
        timeline_start: TimeCode(start),
        source_range: TimeCode::ZERO..TimeCode(dur),
        content: ClipContent::Freeze(kinewright_core::FreezeFrame {
            source_frame: TimeCode::ZERO,
        }),
        effects: vec![Effect {
            id: EffectId(21),
            name: "transform".into(),
            enabled: true,
            enabled_curve: None,
            parameters: BTreeMap::from([
                ("scale_percent".into(), ParamValue::Integer(30)),
                ("x_percent".into(), ParamValue::Integer(28)),
                ("y_percent".into(), ParamValue::Integer(-28)),
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

    let make_lower_third = |start: i64, dur: i64| {
        let mut c = title_clip(
            30,
            start,
            dur,
            Title {
                text: "Field Dispatch — Sector 4".into(),
                font_size_token: 1,
                color_token: 0,
                position: TitlePosition::Top,
                background_scrim: true,
                fade_in_frames: TimeCode(2),
                fade_out_frames: TimeCode(2),
                caption_preset: None,
            },
        );
        c.effects.push(Effect {
            id: EffectId(31),
            name: "transform".into(),
            enabled: true,
            enabled_curve: None,
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([(
                "x_percent".into(),
                AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode(0),
                            value: -10,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                        Keyframe {
                            at: TimeCode(2),
                            value: 0,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                    ],
                },
            )]),
        });
        c
    };

    // --- Lane 1: Youtube1080p (1920x1080, 8 Mbps) over 12 frames (0.5s) of moving grain ---
    let yt_frames = 12_u32;
    let yt_src_path = dir.path("moving-grain-1080p.mp4");
    generate_moving_grainy_mp4(&yt_src_path, 1920, 1080, yt_frames);
    let (yt_asset, yt_cert) = certify_video_asset(&yt_src_path, AssetId(1));

    let yt_bg_clip = Clip {
        id: ClipId(1),
        asset: AssetId(1),
        timeline_start: TimeCode::ZERO,
        source_range: TimeCode::ZERO..TimeCode(i64::from(yt_frames)),
        content: ClipContent::Media,
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
    let yt_doc = document_with_raster(
        (1920, 1080),
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![yt_bg_clip.clone()],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![make_card_clip(6, 2)],
            },
            Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(
                    10,
                    6,
                    2,
                    CaptionPreset::Social.title("Total revenue: 1,250"),
                )],
            },
            Track {
                id: TrackId(4),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![make_lower_third(5, 4)],
            },
        ],
        vec![yt_asset.clone(), card_asset.clone()],
        i64::from(yt_frames),
    );
    let pinned_cards = BTreeMap::from([(AssetId(2), (card_sha, card_len))]);
    let yt_expectations = derive_overlay_expectations_for_test(&yt_doc, &pinned_cards);
    let yt_sources = || {
        BTreeMap::from([
            (AssetId(1), yt_cert.clone()),
            (AssetId(2), card_cert.clone()),
        ])
    };
    let yt_inputs = admit_inputs(&yt_doc, yt_sources());
    let yt_settings = DeliveryProfile::Youtube1080p.export_settings(
        &yt_doc,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );

    let yt_clean_path = dir.path("yt-1080p-clean.mp4");
    let (yt_clean_file, yt_clean_timing) =
        encode_doc_with_settings(&yt_doc, yt_sources(), &yt_settings, &yt_clean_path);
    let yt_clean_bytes = std::fs::metadata(&yt_clean_path).unwrap().len();
    let yt_clean_mbps = (yt_clean_bytes as f64 * 8.0) / (f64::from(yt_frames) / 24.0) / 1_000_000.0;
    let rep_yt_clean = verify_review_overlays(
        yt_clean_file,
        &yt_clean_timing,
        &yt_expectations,
        &yt_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let yt_clean_cap = rep_yt_clean
        .overlays
        .iter()
        .find(|o| o.element == ClipId(10))
        .unwrap();
    let yt_clean_card = rep_yt_clean
        .overlays
        .iter()
        .find(|o| o.element == ClipId(20))
        .unwrap();
    let yt_clean_lt = rep_yt_clean
        .overlays
        .iter()
        .find(|o| o.element == ClipId(30))
        .unwrap();
    println!(
        "S5_C6_FIX2_YT1080P_CLEAN mbps={yt_clean_mbps:.2} global_noise={:.2} tile_p99={:.2} \
         cap(with={:.2},tile={:.2},bnd={:.2},min_wo={:.2}) \
         card(with={:.2},tile={:.2},bnd={:.2},min_wo={:.2}) \
         lt(with={:.2},tile={:.2},bnd={:.2},min_wo={:.2}) status={:?}",
        rep_yt_clean.max_frame_global_noise_rms,
        rep_yt_clean.max_frame_tile_noise_p99,
        yt_clean_cap.worst_with_rms,
        yt_clean_cap.worst_tile_rms,
        yt_clean_cap.worst_boundary_without_rms,
        yt_clean_cap.min_without_rms,
        yt_clean_card.worst_with_rms,
        yt_clean_card.worst_tile_rms,
        yt_clean_card.worst_boundary_without_rms,
        yt_clean_card.min_without_rms,
        yt_clean_lt.worst_with_rms,
        yt_clean_lt.worst_tile_rms,
        yt_clean_lt.worst_boundary_without_rms,
        yt_clean_lt.min_without_rms,
        rep_yt_clean.row_result,
    );
    assert_eq!(
        rep_yt_clean.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX2_YT1080P_CLEAN_MUST_PASS: {:?}",
        rep_yt_clean.findings
    );
    assert_eq!(rep_yt_clean.boundary_unmeasured_frames, 0);
    for ov in &rep_yt_clean.overlays {
        assert_eq!(
            ov.status,
            OverlayElementStatus::Passed,
            "S5_C6_FIX2_YT1080P_ELEMENT_PASSED: {ov:?}"
        );
    }

    // Seeded caption defects run on the same Youtube1080p moving-grain footage with a 5-frame
    // window (cue at frame 2, boundaries at frames 1 and 3) to keep debug runtime bounded.
    let yt_cap_frames = 5_i64;
    let mut yt_cap_bg = yt_bg_clip;
    yt_cap_bg.source_range = TimeCode::ZERO..TimeCode(yt_cap_frames);
    let yt_cap_doc = document_with_raster(
        (1920, 1080),
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![yt_cap_bg],
            },
            Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(
                    10,
                    2,
                    1,
                    CaptionPreset::Social.title("Total revenue: 1,250"),
                )],
            },
        ],
        vec![yt_asset],
        yt_cap_frames,
    );
    let yt_cap_expectations = derive_overlay_expectations_for_test(&yt_cap_doc, &BTreeMap::new());
    let yt_cap_sources = || BTreeMap::from([(AssetId(1), yt_cert.clone())]);
    let yt_cap_inputs = admit_inputs(&yt_cap_doc, yt_cap_sources());
    let yt_cap_settings = DeliveryProfile::Youtube1080p.export_settings(
        &yt_cap_doc,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );

    // Seeded defect 1 on Youtube1080p: missing caption.
    let mut yt_missing = yt_cap_doc.clone();
    yt_missing.tracks[1].clips[0].enabled = false;
    let (file_miss, timing_miss) = encode_doc_with_settings(
        &yt_missing,
        yt_cap_sources(),
        &yt_cap_settings,
        &dir.path("yt-missing.mp4"),
    );
    let rep_miss = verify_review_overlays(
        file_miss,
        &timing_miss,
        &yt_cap_expectations,
        &yt_cap_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let miss_cap = rep_miss
        .overlays
        .iter()
        .find(|o| o.element == ClipId(10))
        .unwrap();
    println!(
        "S5_C6_FIX2_YT1080P_MISSING status={:?} cap_with={:.2} cap_tile={:.2}",
        rep_miss.row_result, miss_cap.worst_with_rms, miss_cap.worst_tile_rms
    );
    assert_eq!(
        rep_miss.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX2_YT1080P_MISSING_FAILS"
    );

    // Seeded defect 2 on Youtube1080p: one-digit change ("1,250" -> "1,850").
    let mut yt_digit = yt_cap_doc.clone();
    yt_digit.tracks[1].clips[0].content =
        ClipContent::Title(CaptionPreset::Social.title("Total revenue: 1,850"));
    let (file_digit, timing_digit) = encode_doc_with_settings(
        &yt_digit,
        yt_cap_sources(),
        &yt_cap_settings,
        &dir.path("yt-digit.mp4"),
    );
    let rep_digit = verify_review_overlays(
        file_digit,
        &timing_digit,
        &yt_cap_expectations,
        &yt_cap_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let digit_cap = rep_digit
        .overlays
        .iter()
        .find(|o| o.element == ClipId(10))
        .unwrap();
    println!(
        "S5_C6_FIX2_YT1080P_DIGIT status={:?} cap_with={:.2} cap_tile={:.2}",
        rep_digit.row_result, digit_cap.worst_with_rms, digit_cap.worst_tile_rms
    );
    assert_eq!(
        rep_digit.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX2_YT1080P_ONE_DIGIT_FAILS"
    );

    // Seeded defect 3 on Youtube1080p: caption starts 1 frame early (frame 1).
    let mut yt_early = yt_cap_doc.clone();
    yt_early.tracks[1].clips[0].timeline_start = TimeCode(1);
    yt_early.tracks[1].clips[0].source_range = TimeCode::ZERO..TimeCode(2);
    let (file_early, timing_early) = encode_doc_with_settings(
        &yt_early,
        yt_cap_sources(),
        &yt_cap_settings,
        &dir.path("yt-early.mp4"),
    );
    let rep_early = verify_review_overlays(
        file_early,
        &timing_early,
        &yt_cap_expectations,
        &yt_cap_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let early_cap = rep_early
        .overlays
        .iter()
        .find(|o| o.element == ClipId(10))
        .unwrap();
    println!(
        "S5_C6_FIX2_YT1080P_EARLY status={:?} cap_bnd_wo={:.2}",
        rep_early.row_result, early_cap.worst_boundary_without_rms
    );
    assert_eq!(
        rep_early.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX2_YT1080P_EARLY_FAILS"
    );

    // Seeded defect 4 on Youtube1080p: caption lingers at 30% opacity on `cue_end` (frame 3).
    let mut yt_ghost = yt_cap_doc.clone();
    let mut ghost = title_clip(
        99,
        3,
        1,
        CaptionPreset::Social.title("Total revenue: 1,250"),
    );
    ghost.effects.push(Effect {
        id: EffectId(98),
        name: "opacity".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([("percent".into(), ParamValue::Integer(30))]),
        keyframes: BTreeMap::new(),
    });
    yt_ghost.tracks[1].clips.push(ghost);
    let (file_ghost, timing_ghost) = encode_doc_with_settings(
        &yt_ghost,
        yt_cap_sources(),
        &yt_cap_settings,
        &dir.path("yt-ghost.mp4"),
    );
    let rep_ghost = verify_review_overlays(
        file_ghost,
        &timing_ghost,
        &yt_cap_expectations,
        &yt_cap_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let ghost_cap = rep_ghost
        .overlays
        .iter()
        .find(|o| o.element == ClipId(10))
        .unwrap();
    println!(
        "S5_C6_FIX2_YT1080P_LINGER_30PCT status={:?} cap_bnd_wo={:.2}",
        rep_ghost.row_result, ghost_cap.worst_boundary_without_rms
    );
    assert_eq!(
        rep_ghost.row_result,
        ReviewRowResult::Failed,
        "S5_C6_FIX2_YT1080P_LINGER_FAILS"
    );

    // --- Lane 2: VerticalShort (1080x1920, 10 Mbps) over 12 frames (0.5s) of moving grain ---
    let vs_frames = 12_u32;
    let vs_src_path = dir.path("moving-grain-vertical.mp4");
    generate_moving_grainy_mp4(&vs_src_path, 1080, 1920, vs_frames);
    let (vs_asset, vs_cert) = certify_video_asset(&vs_src_path, AssetId(1));

    let vs_bg_clip = Clip {
        id: ClipId(1),
        asset: AssetId(1),
        timeline_start: TimeCode::ZERO,
        source_range: TimeCode::ZERO..TimeCode(i64::from(vs_frames)),
        content: ClipContent::Media,
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
    let vs_doc = document_with_raster(
        (1080, 1920),
        vec![
            Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![vs_bg_clip],
            },
            Track {
                id: TrackId(2),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![make_card_clip(6, 2)],
            },
            Track {
                id: TrackId(3),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![title_clip(
                    10,
                    6,
                    2,
                    CaptionPreset::Social.title("Vertical 1,250"),
                )],
            },
            Track {
                id: TrackId(4),
                kind: TrackKind::Video,
                name: None,
                sync_lock: false,
                clips: vec![make_lower_third(5, 4)],
            },
        ],
        vec![vs_asset, card_asset],
        i64::from(vs_frames),
    );
    let vs_expectations = derive_overlay_expectations_for_test(&vs_doc, &pinned_cards);
    let vs_sources = BTreeMap::from([(AssetId(1), vs_cert), (AssetId(2), card_cert)]);
    let vs_inputs = admit_inputs(&vs_doc, vs_sources.clone());
    let vs_settings = DeliveryProfile::VerticalShort.export_settings(
        &vs_doc,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    );
    let vs_clean_path = dir.path("vs-1080x1920-clean.mp4");
    let (vs_clean_file, vs_clean_timing) =
        encode_doc_with_settings(&vs_doc, vs_sources, &vs_settings, &vs_clean_path);
    let vs_clean_bytes = std::fs::metadata(&vs_clean_path).unwrap().len();
    let vs_clean_mbps = (vs_clean_bytes as f64 * 8.0) / (f64::from(vs_frames) / 24.0) / 1_000_000.0;
    let rep_vs_clean = verify_review_overlays(
        vs_clean_file,
        &vs_clean_timing,
        &vs_expectations,
        &vs_inputs,
        fallback_gpu().context(),
        &OverlayVerificationConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    let vs_cap = rep_vs_clean
        .overlays
        .iter()
        .find(|o| o.element == ClipId(10))
        .unwrap();
    let vs_card = rep_vs_clean
        .overlays
        .iter()
        .find(|o| o.element == ClipId(20))
        .unwrap();
    let vs_lt = rep_vs_clean
        .overlays
        .iter()
        .find(|o| o.element == ClipId(30))
        .unwrap();
    println!(
        "S5_C6_FIX2_VERTICAL_CLEAN mbps={vs_clean_mbps:.2} global_noise={:.2} tile_p99={:.2} \
         cap(with={:.2},tile={:.2},bnd={:.2},min_wo={:.2}) \
         card(with={:.2},tile={:.2},bnd={:.2},min_wo={:.2}) \
         lt(with={:.2},tile={:.2},bnd={:.2},min_wo={:.2}) status={:?}",
        rep_vs_clean.max_frame_global_noise_rms,
        rep_vs_clean.max_frame_tile_noise_p99,
        vs_cap.worst_with_rms,
        vs_cap.worst_tile_rms,
        vs_cap.worst_boundary_without_rms,
        vs_cap.min_without_rms,
        vs_card.worst_with_rms,
        vs_card.worst_tile_rms,
        vs_card.worst_boundary_without_rms,
        vs_card.min_without_rms,
        vs_lt.worst_with_rms,
        vs_lt.worst_tile_rms,
        vs_lt.worst_boundary_without_rms,
        vs_lt.min_without_rms,
        rep_vs_clean.row_result,
    );
    assert_eq!(
        rep_vs_clean.row_result,
        ReviewRowResult::Pass,
        "S5_C6_FIX2_VERTICAL_CLEAN_MUST_PASS: {:?}",
        rep_vs_clean.findings
    );
    assert_eq!(rep_vs_clean.boundary_unmeasured_frames, 0);
    for ov in &rep_vs_clean.overlays {
        assert_eq!(
            ov.status,
            OverlayElementStatus::Passed,
            "S5_C6_FIX2_VERTICAL_ELEMENT_PASSED: {ov:?}"
        );
    }
}
