//! CC1/CC4 `render_color_proof` tests.

use super::*;

#[test]
#[allow(clippy::too_many_lines)]
fn cc1_color_proof_preflight_is_scoped_to_active_visual_layers() {
    let (seed_core, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let managed_source = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    };
    let mut document = (*seed).clone();
    document.media_pool[0].color_description = managed_source.clone();

    let mut later_video = document.media_pool[0].clone();
    later_video.id = AssetId(2);
    later_video.name = "later-offline-video".to_owned();
    later_video.path = PathBuf::from("later-offline.mp4");
    let mut offline_audio = later_video.clone();
    offline_audio.id = AssetId(3);
    offline_audio.name = "offline-audio".to_owned();
    offline_audio.path = PathBuf::from("offline-audio.wav");
    offline_audio.kind = MediaKind::Audio;
    let mut offline_overlay = later_video.clone();
    offline_overlay.id = AssetId(4);
    offline_overlay.name = "active-offline-overlay".to_owned();
    offline_overlay.path = PathBuf::from("active-offline-overlay.mp4");
    document
        .media_pool
        .extend([later_video, offline_audio, offline_overlay]);

    let mut later_clip = document.tracks[0].clips[0].clone();
    later_clip.id = ClipId(2);
    later_clip.asset = AssetId(2);
    later_clip.timeline_start = TimeCode(60);
    later_clip.source_range = TimeCode::ZERO..TimeCode(30);
    document.tracks[0].clips.push(later_clip);

    let mut audio_clip = document.tracks[0].clips[0].clone();
    audio_clip.id = ClipId(3);
    audio_clip.asset = AssetId(3);
    audio_clip.source_range = TimeCode::ZERO..TimeCode(60);
    document.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: vec![audio_clip],
    });
    document.duration = TimeCode(90);
    document.validate().unwrap();

    let proof_args = || RenderColorProofArgs {
        effect_id: None,
        look_comparison: None,
        matte_comparison: None,
        expected_revision: TimelineRevision(0),
        clip_id: ClipId(1),
        timecode: TimeCode(12),
        profile_assumption: None,
        parameters: BTreeMap::new(),
    };
    let offline = |reason: &str| MediaAvailabilityStatus {
        kind: MediaAvailabilityKind::OfflineMissing,
        observed_fingerprint: None,
        reason: Some(reason.to_owned()),
    };
    let media_for = |availability_by_asset| {
        Arc::new(NoopMedia {
            availability_by_asset,
            thumbnail_frames: BTreeMap::from([(
                TimeCode(12),
                RgbaImage {
                    width: 2,
                    height: 2,
                    pixels: [32, 32, 32, 255].repeat(4),
                },
            )]),
            candidate_thumbnail_frames: BTreeMap::from([(
                TimeCode(12),
                RgbaImage {
                    width: 2,
                    height: 2,
                    pixels: [96, 64, 32, 255].repeat(4),
                },
            )]),
            ..NoopMedia::default()
        })
    };

    // A later offline video and an offline audio track are irrelevant to
    // the exact frame being proven.
    let media = media_for(BTreeMap::from([
        (AssetId(2), offline("later video is offline")),
        (AssetId(3), offline("audio is offline")),
    ]));
    let service = KinewrightMcp::new(
        Core::spawn(document.clone()).unwrap(),
        playback.clone(),
        media.clone(),
        ConfirmationBroker::default(),
    );
    let result = service.render_color_proof(&proof_args()).unwrap();
    assert_eq!(result.is_error, Some(false));
    let manifest = result.structured_content.unwrap();
    assert_eq!(
        manifest["active_rendered_sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(manifest["active_rendered_sources"][0]["asset_id"], 1);

    // An offline source on a second video track is an active overlay and
    // must block even though the selected clip itself is online.
    let mut overlay_document = document.clone();
    let mut overlay_clip = overlay_document.tracks[0].clips[0].clone();
    overlay_clip.id = ClipId(4);
    overlay_clip.asset = AssetId(4);
    overlay_document.tracks.push(Track {
        id: TrackId(3),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![overlay_clip],
    });
    overlay_document.validate().unwrap();
    let media = media_for(BTreeMap::from([(
        AssetId(4),
        offline("active overlay is offline"),
    )]));
    let service = KinewrightMcp::new(
        Core::spawn(overlay_document.clone()).unwrap(),
        playback.clone(),
        media.clone(),
        ConfirmationBroker::default(),
    );
    let result = service.render_color_proof(&proof_args()).unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "media_offline");
    assert_eq!(structured["details"]["clip_id"], 4);
    assert_eq!(structured["details"]["asset_id"], 4);

    // Freeze frames are source-backed visual layers too; their held frame
    // still requires the referenced asset to be available.
    let mut freeze_document = overlay_document;
    freeze_document.tracks[2].clips[0].content =
        kinewright_core::ClipContent::Freeze(kinewright_core::FreezeFrame {
            source_frame: TimeCode(3),
        });
    freeze_document.validate().unwrap();
    let media = media_for(BTreeMap::from([(
        AssetId(4),
        offline("active freeze source is offline"),
    )]));
    let service = KinewrightMcp::new(
        Core::spawn(freeze_document).unwrap(),
        playback.clone(),
        media,
        ConfirmationBroker::default(),
    );
    let result = service.render_color_proof(&proof_args()).unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "media_offline");
    assert_eq!(structured["details"]["clip_id"], 4);

    // The selected source remains an explicit hard failure when it is the
    // active source that is offline.
    let media = media_for(BTreeMap::from([(
        AssetId(1),
        offline("selected source is offline"),
    )]));
    let service = KinewrightMcp::new(
        Core::spawn(document).unwrap(),
        playback,
        media,
        ConfirmationBroker::default(),
    );
    let result = service.render_color_proof(&proof_args()).unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "media_offline");
    assert_eq!(structured["details"]["clip_id"], 1);
    assert_eq!(structured["details"]["asset_id"], 1);
}

#[test]
fn cc1_color_proof_blocks_an_unsupported_non_selected_active_layer() {
    let (seed_core, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let managed_source = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    };
    let mut document = (*seed).clone();
    document.media_pool[0].color_description = managed_source;

    // A second, non-selected video track composites into the same proof
    // raster with a source the managed pipeline cannot classify.
    let mut overlay_asset = document.media_pool[0].clone();
    overlay_asset.id = AssetId(2);
    overlay_asset.name = "unsupported-overlay".to_owned();
    overlay_asset.path = PathBuf::from("unsupported-overlay.mp4");
    overlay_asset.color_description = ColorDescription::unknown();
    document.media_pool.push(overlay_asset);
    let mut overlay_clip = document.tracks[0].clips[0].clone();
    overlay_clip.id = ClipId(4);
    overlay_clip.asset = AssetId(2);
    // A non-blocking post-primary stage on the same refused composite. The
    // error is the only place it can still be reported, because the
    // successful payload that normally carries it is never produced.
    overlay_clip.effects.push(Effect {
        id: EffectId(41),
        name: "look_lut".to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::new(),
    });
    document.tracks.push(Track {
        id: TrackId(9),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![overlay_clip],
    });
    document.validate().unwrap();

    let media = Arc::new(NoopMedia {
        thumbnail_frames: BTreeMap::from([(
            TimeCode(12),
            RgbaImage {
                width: 2,
                height: 2,
                pixels: [32, 32, 32, 255].repeat(4),
            },
        )]),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(
        Core::spawn(document).unwrap(),
        playback,
        media,
        ConfirmationBroker::default(),
    );
    let result = service
        .render_color_proof(&RenderColorProofArgs {
            effect_id: None,
            look_comparison: None,
            matte_comparison: None,
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(12),
            profile_assumption: None,
            parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 500)]),
        })
        .unwrap();

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "active_layer_needs_color_override");
    assert_eq!(structured["details"]["clip_id"], 4);
    assert_eq!(structured["details"]["asset_id"], 2);
    assert!(structured["details"]["field"].is_string());
    assert!(structured["details"]["observed"].is_string());
    assert!(structured["details"]["allowed"].is_string());

    // Non-blocking layer warnings ride along on the refusal instead of
    // being dropped with the success payload.
    let warnings = structured["details"]["unsupported_layer_warnings"]
        .as_array()
        .expect("the refusal carries the non-blocking layer warnings");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["code"], "legacy_lut_stage");
    assert_eq!(warnings[0]["clip_id"], 4);
    assert_eq!(warnings[0]["asset_id"], 2);
    assert_eq!(warnings[0]["effect_id"], 41);
    assert_eq!(
        warnings[0]["blocking"], false,
        "the blocking source is the error, never a warning"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn render_color_proof_returns_mapped_before_after_evidence_without_mutating() {
    let (seed_core, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed).clone();
    document.media_pool[0].color_description = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    };
    document.tracks[0].clips[0].effects.push(Effect {
        id: EffectId(6),
        name: "primary_correction".to_owned(),
        parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), ParamValue::Integer(100))]),
        keyframes: BTreeMap::from([(
            "exposure_milli_stops".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 100,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                    Keyframe {
                        at: TimeCode(12),
                        value: 750,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                ],
            },
        )]),
    });
    document.tracks[0].clips[0].effects.push(Effect {
        id: EffectId(7),
        name: "look_lut".to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::new(),
    });
    document.tracks[0].clips[0].effects.push(Effect {
        id: EffectId(8),
        name: "cube_lut".to_owned(),
        parameters: BTreeMap::from([(
            "path".to_owned(),
            ParamValue::Text("fixture.cube".to_owned()),
        )]),
        keyframes: BTreeMap::new(),
    });
    document.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![Clip {
            id: ClipId(2),
            asset: AssetId::default(),
            source_range: TimeCode::ZERO..TimeCode(60),
            content: ClipContent::Title(Title {
                text: "CC1 proof overlay".to_owned(),
                font_size_token: 2,
                color_token: 1,
                position: TitlePosition::Top,
                background_scrim: false,
                fade_in_frames: TimeCode(3),
                fade_out_frames: TimeCode(4),
                caption_preset: None,
            }),
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }],
    });
    document.validate().unwrap();
    let proof_document = document.clone();
    let core = Core::spawn(document).unwrap();
    let before = RgbaImage {
        width: 2,
        height: 2,
        pixels: [32, 32, 32, 255].repeat(4),
    };
    let after = RgbaImage {
        width: 2,
        height: 2,
        pixels: [255, 64, 32, 255].repeat(4),
    };
    let media = Arc::new(NoopMedia {
        thumbnail_frames: BTreeMap::from([(TimeCode(12), before.clone())]),
        candidate_thumbnail_frames: BTreeMap::from([(TimeCode(12), after.clone())]),
        // The fixture clip already carries primary node 6, so the plan
        // corrects it in place instead of stacking a second node.
        candidate_effect_id: Some(EffectId(6)),
        candidate_primary_exposure_milli_stops: Some(1_000),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(core, playback, media.clone(), ConfirmationBroker::default());
    let before_snapshot = service.snapshot().unwrap();
    let result = service
        .call_blocking(
            CallToolRequestParams::new("render_color_proof").with_arguments(
                json!({
                    "expected_revision": 0,
                    "clip_id": 1,
                    "timecode": 12,
                    "parameters": {"exposure_milli_stops": 1_000},
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.content.len(), 2);
    let value = result.structured_content.unwrap();
    assert_eq!(value["timeline_revision"], 0);
    assert_eq!(value["clip_id"], 1);
    assert_eq!(value["asset_id"], 1);
    assert_eq!(value["project_frame"], 12);
    assert_eq!(value["render_kind"], "test_double");
    assert_eq!(value["renderer"], "analysis.monitor_proof_for_document");
    assert_eq!(value["backend"], "test_double");
    assert_eq!(value["adapter"], "test_double");
    assert_eq!(value["software_fallback"], true);
    assert_eq!(value["gpu_claim"], false);
    assert_eq!(value["full_resolution"], true);
    assert_eq!(
        value["legacy_stage_warnings"][0]["code"],
        "legacy_lut_stage"
    );
    assert_eq!(value["legacy_stage_warnings"][0]["effect_id"], 7);
    assert_eq!(
        value["legacy_stage_warnings"][1]["code"],
        "legacy_lut_stage"
    );
    assert_eq!(value["legacy_stage_warnings"][1]["effect_id"], 8);
    assert_eq!(value["cpu_reference"], false);
    assert_eq!(value["decoded_delivery"], false);
    assert_eq!(value["source_profile"], "rec709_video");
    assert_eq!(value["source"]["provenance"], "stream_metadata");
    let active_layers = value["active_rendered_layers"].as_array().unwrap();
    assert_eq!(active_layers.len(), 2);
    assert_eq!(active_layers[0]["track_id"], 1);
    assert_eq!(active_layers[0]["clip_id"], 1);
    assert_eq!(active_layers[0]["content"], "media");
    assert_eq!(active_layers[0]["asset_id"], 1);
    assert_eq!(active_layers[0]["source"]["provenance"], "stream_metadata");
    let effects = active_layers[0]["effects"].as_array().unwrap();
    assert_eq!(effects.len(), 3);
    assert_eq!(effects[0]["effect_index"], 0);
    assert_eq!(effects[0]["effect_id"], 6);
    assert_eq!(effects[0]["name"], "primary_correction");
    assert_eq!(effects[0]["parameters"]["exposure_milli_stops"], 750);
    assert_eq!(effects[0]["keyframes"], json!({}));
    assert_eq!(
        effects[0]["primary_parameters"]["exposure_milli_stops"],
        750
    );
    assert_eq!(
        effects[0]["primary_parameters"]["contrast_pivot_basis_points"],
        5_000
    );
    assert_eq!(active_layers[0]["color_nodes"].as_array().unwrap().len(), 1);
    assert_eq!(active_layers[0]["color_nodes"][0]["effect_id"], 6);
    assert_eq!(
        active_layers[0]["color_nodes"][0]["parameters"]["exposure_milli_stops"],
        750
    );
    assert_eq!(effects[1]["effect_index"], 1);
    assert_eq!(effects[1]["effect_id"], 7);
    assert_eq!(effects[1]["name"], "look_lut");
    assert_eq!(effects[2]["effect_index"], 2);
    assert_eq!(effects[2]["effect_id"], 8);
    assert_eq!(effects[2]["name"], "cube_lut");
    assert_eq!(
        active_layers[0]["availability"]["kind"],
        "online_unverified"
    );
    assert_eq!(active_layers[1]["track_id"], 2);
    assert_eq!(active_layers[1]["clip_id"], 2);
    assert_eq!(active_layers[1]["content"], "title");
    assert_eq!(active_layers[1]["title"]["text"], "CC1 proof overlay");
    assert_eq!(active_layers[1]["title"]["font_size_token"], 2);
    assert_eq!(active_layers[1]["title"]["color_token"], 1);
    assert_eq!(active_layers[1]["title"]["position"], "top");
    assert!(active_layers[1].get("asset_id").is_none());
    assert!(active_layers[1].get("source").is_none());
    assert!(active_layers[1].get("source_fingerprint").is_none());
    assert!(active_layers[1].get("availability").is_none());
    assert_eq!(
        value["active_rendered_sources"].as_array().unwrap().len(),
        1
    );
    assert_eq!(value["active_rendered_sources"][0]["track_id"], 1);
    assert_eq!(value["active_rendered_sources"][0]["clip_id"], 1);
    assert_eq!(value["active_rendered_sources"][0]["asset_id"], 1);
    assert_eq!(
        value["active_rendered_sources"][0]["source"]["provenance"],
        "stream_metadata"
    );
    assert_eq!(
        value["active_rendered_sources"][0]["availability"]["kind"],
        "online_unverified"
    );
    assert_eq!(
        value["active_rendered_sources"][0]["legacy_stage_warnings"],
        value["legacy_stage_warnings"]
    );
    assert_eq!(value["formats"]["input"]["bit_depth"], 8);
    assert_eq!(value["formats"]["output"]["bit_depth"], "rgba8");
    let resized_pixels = |image: &RgbaImage| {
        image::imageops::resize(
            &image::RgbaImage::from_raw(image.width, image.height, image.pixels.clone()).unwrap(),
            320,
            180,
            image::imageops::FilterType::Nearest,
        )
        .into_raw()
    };
    assert_eq!(
        value["hashes"]["before_rgba8_pixels_sha256"],
        kinewright_media::sha256_bytes(&resized_pixels(&before))
    );
    assert_eq!(
        value["hashes"]["after_rgba8_pixels_sha256"],
        kinewright_media::sha256_bytes(&resized_pixels(&after))
    );
    for label in [
        "before_rgba8_pixels_sha256",
        "after_rgba8_pixels_sha256",
        "before_png_bytes_sha256",
        "after_png_bytes_sha256",
        "contact_sheet_rgba8_pixels_sha256",
        "contact_sheet_png_bytes_sha256",
    ] {
        assert_eq!(
            value["hashes"][label].as_str().unwrap().len(),
            64,
            "{label}"
        );
    }
    assert_eq!(
        value["primary_correction"]["resolved_parameters"]
            .as_object()
            .unwrap()
            .len(),
        10
    );
    // The clip already carries primary node 6, so the proposal corrects it
    // in place: one SetEffectParam and no second AddEffect.
    let operations = value["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 1);
    assert!(operations[0].get("AddEffect").is_none());
    assert_eq!(operations[0]["SetEffectParam"]["effect"], 6);
    assert_eq!(
        operations[0]["SetEffectParam"]["name"],
        "exposure_milli_stops"
    );
    assert_eq!(operations[0]["SetEffectParam"]["value"], 1_000);
    assert_eq!(
        value["unsupported_layer_warnings"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "the two post-primary LUT stages must be reported: {}",
        value["unsupported_layer_warnings"]
    );
    assert_eq!(
        value["unsupported_layer_warnings"][0]["code"],
        "legacy_lut_stage"
    );
    assert_eq!(value["unsupported_layer_warnings"][0]["blocking"], false);
    assert_eq!(
        value["active_rendered_layers"][0]["source"]["status"]["status"],
        "supported"
    );
    assert_eq!(value["cells"][0]["cell"], "before");
    assert_eq!(value["cells"][1]["cell"], "after");
    assert_eq!(value["objective"]["max_channel_delta_code_values"], 223);
    assert_eq!(
        value["objective"]["mean_channel_delta_milli_code_values"],
        85_000
    );
    assert!(
        value["objective"]["clipping_basis_points"]["after"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(value["evidence_only"], true);
    assert_eq!(value["applied"], false);
    assert_eq!(service.snapshot().unwrap(), before_snapshot);

    // Freeze clips use the same source-backed production layer shape as
    // media clips. Keep an online freeze overlay in this focused manifest
    // check so its exact effect/primary fields cannot regress separately.
    let mut freeze_document = proof_document.clone();
    freeze_document.tracks[1].clips[0].asset = AssetId(1);
    freeze_document.tracks[1].clips[0].content =
        ClipContent::Freeze(kinewright_core::FreezeFrame {
            source_frame: TimeCode(3),
        });
    freeze_document.validate().unwrap();
    let freeze_service = KinewrightMcp::new(
        Core::spawn(freeze_document).unwrap(),
        Arc::new(NoopMedia::default()),
        Arc::new(NoopMedia {
            thumbnail_frames: BTreeMap::from([(TimeCode(12), before.clone())]),
            candidate_thumbnail_frames: BTreeMap::from([(TimeCode(12), after.clone())]),
            candidate_effect_id: Some(EffectId(9)),
            ..NoopMedia::default()
        }),
        ConfirmationBroker::default(),
    );
    let freeze_result = freeze_service
        .render_color_proof(&RenderColorProofArgs {
            effect_id: None,
            look_comparison: None,
            matte_comparison: None,
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(12),
            profile_assumption: None,
            parameters: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(freeze_result.is_error, Some(false));
    let freeze_manifest = freeze_result.structured_content.unwrap();
    let freeze_layers = freeze_manifest["active_rendered_layers"]
        .as_array()
        .unwrap();
    assert_eq!(freeze_layers[1]["content"], "freeze");
    assert_eq!(freeze_layers[1]["source_frame"], 3);
    assert!(freeze_layers[1]["effects"].is_array());

    let stale = service
        .call_blocking(
            CallToolRequestParams::new("render_color_proof").with_arguments(
                json!({
                    "expected_revision": 1,
                    "clip_id": 1,
                    "timecode": 12,
                    "parameters": {},
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(stale.is_error, Some(true));
    assert_eq!(
        stale.structured_content.unwrap()["code"],
        "revision_conflict"
    );

    for (kind, code) in [
        (MediaAvailabilityKind::OfflineMissing, "media_offline"),
        (MediaAvailabilityKind::Changed, "media_changed"),
    ] {
        let media = Arc::new(NoopMedia {
            availability_by_asset: BTreeMap::from([(
                AssetId(1),
                MediaAvailabilityStatus {
                    kind,
                    observed_fingerprint: None,
                    reason: Some("test proof availability".to_owned()),
                },
            )]),
            ..NoopMedia::default()
        });
        let unavailable = KinewrightMcp::new(
            Core::spawn(proof_document.clone()).unwrap(),
            Arc::new(NoopMedia::default()),
            media,
            ConfirmationBroker::default(),
        );
        let result = unavailable
            .render_color_proof(&RenderColorProofArgs {
                effect_id: None,
                look_comparison: None,
                matte_comparison: None,
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(12),
                profile_assumption: None,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["code"], code);
    }

    let mut incompatible_document = proof_document.clone();
    incompatible_document.color_context.pipeline_state =
        kinewright_core::ColorPipelineState::Legacy;
    let incompatible = KinewrightMcp::new(
        Core::spawn(incompatible_document).unwrap(),
        Arc::new(NoopMedia::default()),
        Arc::new(NoopMedia::default()),
        ConfirmationBroker::default(),
    );
    let result = incompatible
        .render_color_proof(&RenderColorProofArgs {
            effect_id: None,
            look_comparison: None,
            matte_comparison: None,
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(12),
            profile_assumption: None,
            parameters: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "unsupported_color_pipeline"
    );

    let failed_media = Arc::new(NoopMedia {
        render_error: Some("test compositor failure".to_owned()),
        ..NoopMedia::default()
    });
    let failed = KinewrightMcp::new(
        Core::spawn(proof_document.clone()).unwrap(),
        Arc::new(NoopMedia::default()),
        failed_media,
        ConfirmationBroker::default(),
    );
    let result = failed
        .render_color_proof(&RenderColorProofArgs {
            effect_id: None,
            look_comparison: None,
            matte_comparison: None,
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(12),
            profile_assumption: None,
            parameters: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "color_proof_render_failed"
    );

    let unsupported_media = Arc::new(NoopMedia {
        proof_error: Some(MediaError::UnsupportedDecoderFormat {
            path: PathBuf::from("fixture.mp4"),
            format: "yuv444p10le".to_owned(),
            declared_bit_depth: Some(8),
            decoder_bit_depth: Some(10),
            reason: "managed source depth mismatch".to_owned(),
        }),
        ..NoopMedia::default()
    });
    let unsupported = KinewrightMcp::new(
        Core::spawn(proof_document).unwrap(),
        Arc::new(NoopMedia::default()),
        unsupported_media,
        ConfirmationBroker::default(),
    );
    let result = unsupported
        .render_color_proof(&RenderColorProofArgs {
            effect_id: None,
            look_comparison: None,
            matte_comparison: None,
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(12),
            profile_assumption: None,
            parameters: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "unsupported_decoder_format");
    assert_eq!(structured["details"]["format"], "yuv444p10le");
    assert_eq!(structured["details"]["decoder_bit_depth"], 10);
}

fn probed_color_description() -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Bt2020,
        transfer: ColorTransfer::AribStdB67,
        matrix: ColorMatrix::Bt2020Ncl,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Ten,
        confidence_basis_points: 8_765,
        provenance: ColorProvenance::StreamMetadata,
    }
}

fn user_color_override() -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::DisplayP3,
        transfer: ColorTransfer::Gamma22,
        matrix: ColorMatrix::Rgb,
        range: ColorRange::Full,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Twelve,
        confidence_basis_points: 9_321,
        provenance: ColorProvenance::UserOverride,
    }
}

fn color_override_request(
    expected_revision: u64,
    description: &ColorDescription,
) -> CallToolRequestParams {
    CallToolRequestParams::new("set_asset_color_description").with_arguments(
        json!({
            "expected_revision": expected_revision,
            "asset": 1,
            "color_description": description,
        })
        .as_object()
        .unwrap()
        .clone(),
    )
}

fn source_color(service: &KinewrightMcp) -> serde_json::Value {
    service
        .source_info(&SourceInfoArgs {
            asset_id: AssetId(1),
            source_in: None,
            source_out: None,
        })
        .unwrap()
        .structured_content
        .unwrap()["asset"]["color_description"]
        .clone()
}

fn assert_wire_color(value: &serde_json::Value, confidence: u16, provenance: &str) {
    assert_eq!(value["confidence_basis_points"], confidence);
    assert_eq!(value["provenance"], provenance);
}

/// The CC3 §10.3 item 12 stack: a managed primary, a *bypassed but
/// non-neutral* wheels node, and a three-point curves node, in that
/// serialized order.
fn ordered_colour_node_document() -> Document {
    let (seed_core, _, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed).clone();
    document.media_pool[0].color_description = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    };
    let effects = &mut document.tracks[0].clips[0].effects;
    effects.push(Effect {
        id: EffectId(6),
        name: "primary_correction".to_owned(),
        parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), ParamValue::Integer(250))]),
        keyframes: BTreeMap::new(),
    });
    // Bypassed but deliberately non-neutral: CC3 §5 keeps its slot, its
    // stage index, and every stored value while it renders as the exact
    // identity.
    effects.push(Effect {
        id: EffectId(7),
        name: "color_wheels".to_owned(),
        parameters: BTreeMap::from([
            (
                "gain_red_thousandths".to_owned(),
                ParamValue::Integer(1_400),
            ),
            ("bypass".to_owned(), ParamValue::Integer(1)),
        ]),
        keyframes: BTreeMap::new(),
    });
    effects.push(Effect {
        id: EffectId(8),
        name: "color_curves".to_owned(),
        parameters: BTreeMap::from([
            ("master_point_count".to_owned(), ParamValue::Integer(3)),
            ("master_x1".to_owned(), ParamValue::Integer(5_000)),
            ("master_y1".to_owned(), ParamValue::Integer(6_000)),
        ]),
        keyframes: BTreeMap::new(),
    });
    document.validate().unwrap();
    document
}

/// CC3 §10.3 item 12, ordered-stage half: the proof manifest's colour-node
/// stack is `clip.effects` order, which is the compositor's evaluation
/// order. `kinewright-media` exposes no stage manifest, so the agent's
/// `render_color_proof` surface is where the ordering is observable.
#[test]
fn render_color_proof_reports_the_ordered_colour_node_stack_in_clip_effect_order() {
    let (_, playback, _) = fixture();
    let document = ordered_colour_node_document();
    let frame = RgbaImage {
        width: 2,
        height: 2,
        pixels: [32, 32, 32, 255].repeat(4),
    };
    let media = Arc::new(NoopMedia {
        thumbnail_frames: BTreeMap::from([(TimeCode(12), frame.clone())]),
        candidate_thumbnail_frames: BTreeMap::from([(TimeCode(12), frame)]),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(
        Core::spawn(document).unwrap(),
        playback,
        media,
        ConfirmationBroker::default(),
    );
    let result = service
        .render_color_proof(&RenderColorProofArgs {
            effect_id: None,
            look_comparison: None,
            matte_comparison: None,
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(12),
            profile_assumption: None,
            parameters: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let value = result.structured_content.unwrap();
    let nodes = value["active_rendered_layers"][0]["color_nodes"]
        .as_array()
        .expect("the proof manifest carries an ordered colour-node stack")
        .clone();
    assert_eq!(nodes.len(), 3);
    assert_eq!(
        nodes
            .iter()
            .map(|node| (
                node["stage_index"].as_u64(),
                node["kind"].as_str(),
                node["effect_id"].as_u64()
            ))
            .collect::<Vec<_>>(),
        vec![
            (Some(0), Some("primary_correction"), Some(6)),
            (Some(1), Some("color_wheels"), Some(7)),
            (Some(2), Some("color_curves"), Some(8)),
        ],
        "the manifest order must equal clip.effects order",
    );

    assert_eq!(nodes[1]["bypass"], 1);
    assert_eq!(nodes[1]["active"], false);
    assert_eq!(nodes[1]["inactive_reason"], "bypassed");
    assert_eq!(
        nodes[1]["parameters"]["gain_red_thousandths"], 1_400,
        "a bypassed node keeps every stored value",
    );

    assert_eq!(nodes[2]["active"], true);
    assert_eq!(
        nodes[2]["curves"]["master"]["points"],
        json!([[0, 0], [5_000, 6_000], [10_000, 10_000]]),
        "the omitted third point resolves to its (10000, 10000) neutral",
    );
    assert_eq!(nodes[2]["curves"]["master"]["truncated"], false);
    assert_eq!(nodes[2]["curves"]["red"]["structural_identity"], true);
}

#[test]
fn generated_color_override_is_revision_gated_and_undo_restores_probed_metadata() {
    let (seed_core, playback, analysis) = fixture();
    let Event::QueryResult(QueryResult::Document(seed_document)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let probed = probed_color_description();
    let mut document = (*seed_document).clone();
    document.media_pool[0].color_description = probed.clone();
    let core = Core::spawn(document).unwrap();
    let service = KinewrightMcp::new(
        core.clone(),
        playback,
        analysis,
        ConfirmationBroker::default(),
    );

    let override_description = user_color_override();
    let applied = service
        .call_blocking(color_override_request(0, &override_description))
        .unwrap();
    assert_eq!(applied.is_error, Some(false));
    let (revision, applied_document) = service.snapshot().unwrap();
    assert_eq!(revision, TimelineRevision(1));
    assert_eq!(
        applied_document
            .asset(AssetId(1))
            .unwrap()
            .color_description,
        override_description
    );

    assert_wire_color(&source_color(&service), 9_321, "user_override");
    let context = service
        .color_context(&ColorContextArgs::default())
        .unwrap()
        .structured_content
        .unwrap();
    assert_wire_color(
        &context["assets"][0]["source"]["raw_description"],
        9_321,
        "user_override",
    );

    let mut stale_description = override_description.clone();
    stale_description.confidence_basis_points = 9_999;
    let stale = service
        .call_blocking(color_override_request(0, &stale_description))
        .unwrap();
    assert_eq!(stale.is_error, Some(true));
    assert!(
        stale.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("revision conflict")
    );
    assert_eq!(service.snapshot().unwrap().0, TimelineRevision(1));

    let Event::DocumentChanged {
        doc,
        revision: TimelineRevision(2),
        ..
    } = core.request(Command::Undo).unwrap()
    else {
        panic!("undo should restore the probed colour description");
    };
    assert_eq!(doc.asset(AssetId(1)).unwrap().color_description, probed);
    assert_wire_color(&source_color(&service), 8_765, "stream_metadata");
}
