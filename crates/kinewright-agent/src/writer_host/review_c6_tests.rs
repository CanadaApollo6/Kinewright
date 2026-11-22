fn s5_c6_admit_embedded_font(
    bundle: &mut ProjectBundle,
) -> kinewright_project::bundle_records::ObjectRef {
    let font = kinewright_core::title_font_bytes();
    let reference = kinewright_project::bundle_records::ObjectRef {
        sha256: kinewright_media::sha256_bytes(font),
        byte_length: font.len() as u64,
        record_kind: kinewright_project::bundle_records::RecordKind::OwnedFont,
    };
    let _blob = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut &font[..], &reference, &[])
        .unwrap();
    let manifest = kinewright_project::owned_inputs::OwnedInputManifest {
        project_id: Some(bundle.project_id().clone()),
        session_id: None,
        owner_epoch: bundle.store().owner_epoch(),
        entries: vec![kinewright_project::owned_inputs::OwnedInputEntry {
            entry_id: "font:Inter".into(),
            object: reference,
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        }],
    };
    bundle.admit_inputs(&manifest).unwrap().reference
}

fn s5_c6_solid_clip(
    id: u64,
    start: i64,
    duration: i64,
    color: kinewright_core::SolidColor,
) -> kinewright_core::Clip {
    kinewright_core::Clip {
        id: kinewright_core::ClipId(id),
        asset: kinewright_core::AssetId(0),
        timeline_start: kinewright_core::TimeCode(start),
        source_range: kinewright_core::TimeCode::ZERO..kinewright_core::TimeCode(duration),
        content: kinewright_core::ClipContent::Solid(color),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: kinewright_core::TimeCode::ZERO,
        audio_fade_out_frames: kinewright_core::TimeCode::ZERO,
        audio_gain_curve: None,
        speed_percent: 100,
        enabled: true,
        enabled_curve: None,
        blend_mode: kinewright_core::BlendMode::Normal,
    }
}

fn s5_c6_title_clip(
    id: u64,
    start: i64,
    duration: i64,
    title: kinewright_core::Title,
    effects: Vec<kinewright_core::Effect>,
) -> kinewright_core::Clip {
    kinewright_core::Clip {
        id: kinewright_core::ClipId(id),
        asset: kinewright_core::AssetId(0),
        timeline_start: kinewright_core::TimeCode(start),
        source_range: kinewright_core::TimeCode::ZERO..kinewright_core::TimeCode(duration),
        content: kinewright_core::ClipContent::Title(title),
        effects,
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: kinewright_core::TimeCode::ZERO,
        audio_fade_out_frames: kinewright_core::TimeCode::ZERO,
        audio_gain_curve: None,
        speed_percent: 100,
        enabled: true,
        enabled_curve: None,
        blend_mode: kinewright_core::BlendMode::Normal,
    }
}

fn s5_c6_clean_overlay_doc() -> Document {
    use kinewright_core::{
        AutomationCurve, CaptionPreset, Effect, EffectId, Keyframe, KeyframeInterpolation,
        Rational, SolidColor, TimeCode, Title, TitlePosition, Track, TrackId, TrackKind,
    };
    let bg_a = s5_c6_solid_clip(
        1,
        0,
        7,
        SolidColor {
            r: 28,
            g: 48,
            b: 84,
        },
    );
    let bg_b = s5_c6_solid_clip(
        2,
        7,
        7,
        SolidColor {
            r: 64,
            g: 38,
            b: 78,
        },
    );
    let cap_clean = s5_c6_title_clip(3, 0, 7, CaptionPreset::Clean.title("Hello world"), vec![]);
    let cap_social = s5_c6_title_clip(4, 7, 7, CaptionPreset::Social.title("Second beat"), vec![]);
    let anim_title = Title {
        text: "Chapter One".into(),
        font_size_token: 1,
        color_token: 2,
        position: TitlePosition::Top,
        background_scrim: true,
        fade_in_frames: TimeCode(2),
        fade_out_frames: TimeCode(2),
        caption_preset: None,
    };
    let transform = Effect {
        id: EffectId(1),
        name: "transform".into(),
        enabled: true,
        enabled_curve: None,
        parameters: std::collections::BTreeMap::new(),
        keyframes: std::collections::BTreeMap::from([(
            "x_percent".into(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: -18,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(13),
                        value: 18,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ],
            },
        )]),
    };
    let title_clip = s5_c6_title_clip(5, 0, 14, anim_title, vec![transform]);

    let doc = Document {
        fps: Rational::new(24, 1).unwrap(),
        resolution: (480, 270),
        duration: TimeCode(14),
        tracks: vec![
            Track {
                id: TrackId(1),
                name: Some("Video".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![bg_a, bg_b],
            },
            Track {
                id: TrackId(2),
                name: Some("Captions".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![cap_clean, cap_social],
            },
            Track {
                id: TrackId(3),
                name: Some("Titles".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![title_clip],
            },
        ],
        ..Document::default()
    };
    doc.validate().unwrap();
    doc
}

fn s5_c6_make_step_doc(text: &str) -> Document {
    use kinewright_core::{
        CaptionPreset, Rational, SolidColor, TimeCode, Track, TrackId, TrackKind,
    };
    let doc = Document {
        fps: Rational::new(24, 1).unwrap(),
        resolution: (480, 270),
        duration: TimeCode(8),
        tracks: vec![
            Track {
                id: TrackId(1),
                name: Some("Bg".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_solid_clip(
                    1,
                    0,
                    8,
                    SolidColor {
                        r: 36,
                        g: 56,
                        b: 92,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                name: Some("Captions".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_title_clip(
                    2,
                    0,
                    8,
                    CaptionPreset::Clean.title(text),
                    vec![],
                )],
            },
        ],
        ..Document::default()
    };
    doc.validate().unwrap();
    doc
}

fn s5_c6_publish_version(
    f: &Fixture,
    parent_version: Option<VersionId>,
    revision_refs: Vec<kinewright_project::bundle_records::ObjectRef>,
) -> kinewright_project::studio_records::SnapshotKey {
    let host = f.host.owner.upgrade().unwrap();
    let mut host = host.lock().unwrap();
    let owner = host.owner_mut().unwrap();
    let issuer = owner.reuse_metadata_issuer_for("fixture-s5-c6").unwrap();
    let envelope = owner
        .metadata_envelope(CommandId {
            sequence: owner.metadata_issuer_sequence(&issuer).unwrap(),
            issuer,
        })
        .unwrap();
    let update = MetadataUpdate {
        expected_root_generation: owner.metadata_generation().unwrap(),
        change: MetadataChange::PublishSnapshot(PublishSnapshot {
            version: VersionId::fresh().unwrap(),
            expected_head: f.brief.scope.base.head.clone(),
            checkpoint_ref: f.brief.scope.base.checkpoint.clone(),
            parent_version,
            run_provenance: None,
            revision_refs,
        }),
    };
    let kinewright_project::receipts::ReceiptLookup::Committed(receipt) = owner
        .metadata_command(
            &envelope,
            kinewright_project::studio_records::MetadataScope::Snapshot,
            &update,
        )
        .unwrap()
    else {
        panic!("S5_C6_VERSION_COMMITTED");
    };
    let ReceiptResult::Metadata {
        result: MetadataResult::Snapshot(key),
    } = receipt.result
    else {
        panic!("S5_C6_VERSION_RECEIPT");
    };
    key
}

fn s5_c6_start_and_finish_encode(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
) -> ReviewToken {
    let token = s5_review_start(&f.editor, key);
    let status = s5_review_wait(&f.editor, |phase| {
        matches!(
            phase,
            ReviewPhase::Checking(_) | ReviewPhase::Paused { .. } | ReviewPhase::Stopped
        )
    });
    assert!(
        matches!(status.phase, ReviewPhase::Checking(_)),
        "S5_C6_ENCODE_EXPECTED_CHECKING, got {:?}",
        status.phase
    );
    let shared = f.host.owner.upgrade().unwrap();
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_none_or(|(_, done)| done)
        },
        "S5_C6_ENCODE_WORKER_DONE",
    );
    s5_review_call(&f.editor, ReviewControl::Status).unwrap();
    token
}

fn s5_c6_read_all_stills(
    reviewer: &C4Mcp,
    page_size: u8,
) -> (
    Vec<super::super::review_stills::ServedReviewStill>,
    Option<String>,
    usize,
) {
    use base64::Engine as _;
    let mut cursor: Option<String> = None;
    let mut first_cursor: Option<String> = None;
    let mut pages = 0_usize;
    let mut collected = Vec::new();
    loop {
        let mut args = serde_json::json!({ "page_size": page_size });
        if let Some(ref c) = cursor {
            args["cursor"] = serde_json::json!(c);
        }
        let reply = reviewer.wire("read_review_stills", args);
        assert_eq!(reply.is_error, Some(false), "S5_C6_READ_STILLS {reply:?}");
        let page: super::super::review_stills::ReviewStillPage =
            serde_json::from_value(reply.structured_content.as_ref().unwrap()["outcome"].clone())
                .unwrap();
        pages += 1;
        if first_cursor.is_none() {
            first_cursor = page.next_cursor.clone();
        }
        assert_eq!(
            reply.content.len(),
            1 + page.stills.len(),
            "S5_C6_IMAGE_BLOCK_COUNT"
        );
        for (still, block) in page.stills.iter().zip(reply.content.iter().skip(1)) {
            let image = block.as_image().expect("S5_C6_CONTENT_BLOCK_IMAGE");
            assert_eq!(image.mime_type, "image/png", "S5_C6_PNG_MIME");
            let png = base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .expect("S5_C6_VALID_BASE64");
            assert_eq!(
                png.len() as u64,
                still.png_byte_length,
                "S5_C6_PNG_BYTE_LEN_BOUND"
            );
            assert!(
                png.len() <= kinewright_media::MAX_STILL_PNG_BYTES,
                "S5_C6_PNG_CAP"
            );
            assert_eq!(
                kinewright_media::sha256_bytes(&png),
                still.png_sha256,
                "S5_C6_PNG_SHA256_BOUND"
            );
            assert_eq!(still.plan_sha256, page.plan_sha256, "S5_C6_PLAN_SHA_BOUND");
            let decoded = image::load_from_memory(&png).expect("S5_C6_VALID_PNG_BYTES");
            assert_eq!(
                (decoded.width(), decoded.height()),
                (still.width, still.height),
                "S5_C6_PNG_DIMENSIONS_MATCH"
            );
            assert!(
                decoded.width().max(decoded.height()) <= kinewright_media::MAX_STILL_LONG_EDGE,
                "S5_C6_LONG_EDGE_CAP"
            );
        }
        collected.extend(page.stills);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    (collected, first_cursor, pages)
}

fn s5_c6_generate_bt709_clip(path: &std::path::Path, frames: u32) {
    let duration = format!("{:.3}", f64::from(frames) / 24.0 + 0.1);
    let frames_s = frames.to_string();
    kinewright_media::test_support::run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("color=c=0x5a2c44:s=320x180:r=24:d={duration}"),
            "-frames:v",
            &frames_s,
            "-threads",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
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
}

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_overlay_verification_records_pass_and_ignores_working_edit() {
    use super::super::review_overlays::C6CheckConfiguration;
    use kinewright_core::output_review::{RequiredReviewRow, ReviewRowResult};
    use kinewright_media::{OverlayElementStatus, OverlayVerificationConfig};

    let doc = s5_c6_clean_overlay_doc();
    let mut f = Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font);
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    let token = s5_c6_start_and_finish_encode(&f, &key);

    // Mutate the working edit after freeze by deleting the caption clip and saving.
    {
        let shared = f.host.owner.upgrade().unwrap();
        let owner = shared.lock().unwrap();
        owner
            .core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::DeleteClip {
                    clip: kinewright_core::ClipId(3),
                },
            ))
            .unwrap();
    }
    ok(
        f.editor
            .request(HostCommand::Save {
                snapshot: host_snapshot_for_test(&f.host),
            })
            .unwrap(),
        "S5_C6_WORKING_EDIT_SAVED",
    );

    let shared = f.host.owner.upgrade().unwrap();
    let config = C6CheckConfiguration::default();
    let config_sha = config.configuration_sha256();
    assert_eq!(
        config.overlay.sha256(),
        OverlayVerificationConfig::default().sha256(),
        "S5_C6_OVERLAY_CONFIG_SHA256_STABLE"
    );
    let mut stricter = config.clone();
    stricter.overlay.max_codec_error_rms -= 1.0;
    assert_ne!(
        config.overlay.sha256(),
        stricter.overlay.sha256(),
        "S5_C6_OVERLAY_CONFIG_SHA256_SENSITIVE"
    );
    let mut smaller_stills = config.clone();
    smaller_stills.stills.max_long_edge -= 1;
    assert_ne!(
        config.stills.sha256(),
        smaller_stills.stills.sha256(),
        "S5_C6_STILLS_CONFIG_SHA256_SENSITIVE"
    );
    let binding = ReviewCheckBinding {
        configuration_sha256: config_sha,
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding.clone()))
        .expect("S5_C6_BIND_CHECKS");

    let (report, status) = super::super::review_overlays::run_overlay_verification(
        &shared,
        &token,
        binding,
        &config,
        &kinewright_core::ExportCancellation::default(),
    )
    .expect("S5_C6_OVERLAY_VERIFICATION_RUN");

    assert_eq!(
        report.row_result,
        ReviewRowResult::Pass,
        "S5_C6_CLEAN_HOST_PASS: {:?}",
        report.findings
    );
    assert_eq!(report.overlays.len(), 3, "S5_C6_ALL_THREE_OVERLAYS");
    assert!(
        report.animated_extreme_frames >= 2,
        "S5_C6_ANIMATED_EXTREMES_CHECKED"
    );
    for overlay in &report.overlays {
        assert_eq!(
            overlay.status,
            OverlayElementStatus::Passed,
            "S5_C6_OVERLAY_PASSED {:?}",
            overlay.element
        );
        assert!(
            overlay.checked_frames > 0,
            "S5_C6_OVERLAY_FRAMES_CHECKED {:?}",
            overlay.element
        );
        if overlay.element == kinewright_core::ClipId(5) {
            assert!(
                overlay.animated_extreme_frames >= 2,
                "S5_C6_EXTREMES_CHECKED"
            );
        }
    }
    let ReviewPhase::Checking(checks) = status.phase else {
        panic!("expected Checking phase, got {:?}", status.phase);
    };
    assert_eq!(
        checks
            .rows
            .get(&RequiredReviewRow::OverlayVerification)
            .map(|entry| entry.result),
        Some(ReviewRowResult::Pass),
        "S5_C6_RECORDED_THROUGH_RECORD_REVIEW_CHECK"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_overlay_defects_and_unresolved_expectations_record_through_host() {
    use super::super::review_overlays::C6CheckConfiguration;
    use kinewright_core::{
        CaptionPreset, Rational, SolidColor, TimeCode, Track, TrackId, TrackKind,
        output_review::{RequiredReviewRow, ReviewInventory, ReviewRowResult},
    };
    use kinewright_media::{OverlayElementStatus, OverlayFindingKind, OverlayVerificationConfig};

    let config = C6CheckConfiguration::default();

    // Part A: Occluded caption (upper solid layer covers caption) cannot silently Pass through
    // the full ReviewJob host path: the compositor's with/without references are identical under
    // the full opaque occluder so L14 explicitly flags UnmeasurableOverlay -> ReviewRowResult::Unknown.
    let occluded_doc = Document {
        fps: Rational::new(24, 1).unwrap(),
        resolution: (480, 270),
        duration: TimeCode(8),
        tracks: vec![
            Track {
                id: TrackId(1),
                name: Some("Bg".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_solid_clip(
                    1,
                    0,
                    8,
                    SolidColor {
                        r: 24,
                        g: 48,
                        b: 84,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                name: Some("Captions".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_title_clip(
                    2,
                    0,
                    8,
                    CaptionPreset::Clean.title("Step 1"),
                    vec![],
                )],
            },
            Track {
                id: TrackId(3),
                name: Some("Occluder".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_solid_clip(
                    3,
                    0,
                    8,
                    SolidColor {
                        r: 112,
                        g: 64,
                        b: 52,
                    },
                )],
            },
        ],
        ..Document::default()
    };
    occluded_doc.validate().unwrap();
    let mut f = Fixture::with_inputs(
        false,
        occluded_doc,
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    let token = s5_c6_start_and_finish_encode(&f, &key);
    let shared = f.host.owner.upgrade().unwrap();
    let binding = ReviewCheckBinding {
        configuration_sha256: config.configuration_sha256(),
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding.clone()))
        .expect("S5_C6_OCCLUDED_BIND");
    let (report, status) = super::super::review_overlays::run_overlay_verification(
        &shared,
        &token,
        binding,
        &config,
        &kinewright_core::ExportCancellation::default(),
    )
    .expect("S5_C6_OCCLUDED_RUN");
    assert_ne!(
        report.row_result,
        ReviewRowResult::Pass,
        "S5_C6_OCCLUDED_CAPTION_NEVER_PASSES"
    );
    assert!(
        report.overlays.iter().any(|o| matches!(
            o.status,
            OverlayElementStatus::Failed | OverlayElementStatus::Unknown
        )),
        "S5_C6_OCCLUDED_ELEMENT_NOT_PASSED: {:?}",
        report.overlays
    );
    let ReviewPhase::Checking(checks) = status.phase else {
        panic!("expected Checking");
    };
    assert_eq!(
        checks
            .rows
            .get(&RequiredReviewRow::OverlayVerification)
            .map(|r| r.result),
        Some(report.row_result),
        "S5_C6_OCCLUDED_RECORDED_NON_PASS"
    );

    // Part B: Unresolved overlay expectation records Unknown (never Pass) through record_review_check.
    let clean_doc = s5_c6_clean_overlay_doc();
    let mut f2 = Fixture::with_inputs(
        false,
        clean_doc,
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    holder(&mut f2);
    let key2 = s5_c6_publish_version(&f2, None, vec![]);
    let token2 = s5_c6_start_and_finish_encode(&f2, &key2);
    let shared2 = f2.host.owner.upgrade().unwrap();
    let binding2 = ReviewCheckBinding {
        configuration_sha256: config.configuration_sha256(),
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    let (artifact_file, artifact_timing, mut expectations, inputs, data_dir) = {
        let mut owner = shared2.lock().unwrap();
        owner
            .record_review_check(&token2, UpdateReviewJob::BindChecks(binding2.clone()))
            .expect("S5_C6_UNKNOWN_BIND");
        let data_dir = owner.config.data_dir.clone();
        let bundle = owner
            .owner()
            .unwrap()
            .review_source_bundle(&token2.project)
            .unwrap();
        let job = bundle.review_job().unwrap().unwrap();
        let req = bundle
            .store()
            .read_record::<kinewright_project::review_job::ReviewRequest>(&job.request_ref)
            .unwrap();
        let exp = bundle
            .store()
            .read_record::<kinewright_project::review_job::StoredReviewExpectations>(
                &req.expectations_ref,
            )
            .unwrap();
        let snap = bundle.read_snapshot(&token2.snapshot).unwrap();
        let ctx = bundle
            .prepare_frozen_context_descriptor(&snap.descriptor)
            .unwrap();
        let (artifact, file) = bundle.open_review_artifact(&token2).unwrap();
        let inputs = Arc::new(ctx.open().unwrap()).playback_inputs().unwrap();
        (
            file,
            artifact.output.clone(),
            exp.0.clone(),
            inputs,
            data_dir,
        )
    };
    expectations.overlays.captions = ReviewInventory::NotDetermined { known: Vec::new() };
    let engine = super::super::review_job::review_media_engine_for_data_dir(data_dir).unwrap();
    let unknown_report = engine
        .verify_review_overlays(
            artifact_file,
            &artifact_timing,
            &expectations,
            &inputs,
            &OverlayVerificationConfig::default(),
            &kinewright_core::ExportCancellation::default(),
        )
        .unwrap();
    assert_eq!(
        unknown_report.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_UNRESOLVED_RETURNS_UNKNOWN"
    );
    let unknown_status = shared2
        .lock()
        .unwrap()
        .record_overlay_verification(&token2, binding2, &config, &unknown_report)
        .expect("S5_C6_RECORD_UNKNOWN");
    let ReviewPhase::Checking(checks2) = unknown_status.phase else {
        panic!("expected Checking");
    };
    assert_eq!(
        checks2
            .rows
            .get(&RequiredReviewRow::OverlayVerification)
            .map(|r| r.result),
        Some(ReviewRowResult::Unknown),
        "S5_C6_UNRESOLVED_RECORDED_UNKNOWN"
    );

    // Part C: Substituted output text after preflight ("Step 1" -> "Step 2") fails and records Failed.
    let step1_doc = s5_c6_make_step_doc("Step 1");
    let step2_doc = s5_c6_make_step_doc("Step 2");
    let mut f_expected = Fixture::with_inputs(
        false,
        step1_doc,
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    holder(&mut f_expected);
    let key_expected = s5_c6_publish_version(&f_expected, None, vec![]);
    let token_expected = s5_c6_start_and_finish_encode(&f_expected, &key_expected);
    let shared_expected = f_expected.host.owner.upgrade().unwrap();
    let binding_sub = ReviewCheckBinding {
        configuration_sha256: config.configuration_sha256(),
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    let (frozen_step1_exp, step1_inputs, step1_data_dir) = {
        let mut owner = shared_expected.lock().unwrap();
        owner
            .record_review_check(
                &token_expected,
                UpdateReviewJob::BindChecks(binding_sub.clone()),
            )
            .expect("S5_C6_SUB_BIND");
        let data_dir = owner.config.data_dir.clone();
        let bundle = owner
            .owner()
            .unwrap()
            .review_source_bundle(&token_expected.project)
            .unwrap();
        let job = bundle.review_job().unwrap().unwrap();
        let req = bundle
            .store()
            .read_record::<kinewright_project::review_job::ReviewRequest>(&job.request_ref)
            .unwrap();
        let exp = bundle
            .store()
            .read_record::<kinewright_project::review_job::StoredReviewExpectations>(
                &req.expectations_ref,
            )
            .unwrap()
            .0
            .clone();
        let snap = bundle.read_snapshot(&token_expected.snapshot).unwrap();
        let ctx = bundle
            .prepare_frozen_context_descriptor(&snap.descriptor)
            .unwrap();
        let inputs = Arc::new(ctx.open().unwrap()).playback_inputs().unwrap();
        (exp, inputs, data_dir)
    };
    let mut f_tampered = Fixture::with_inputs(
        false,
        step2_doc,
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    holder(&mut f_tampered);
    let key_tampered = s5_c6_publish_version(&f_tampered, None, vec![]);
    let token_tampered = s5_c6_start_and_finish_encode(&f_tampered, &key_tampered);
    let (tampered_file, tampered_timing) = {
        let shared_tampered = f_tampered.host.owner.upgrade().unwrap();
        let owner = shared_tampered.lock().unwrap();
        let bundle = owner
            .owner()
            .unwrap()
            .review_source_bundle(&token_tampered.project)
            .unwrap();
        let (artifact, file) = bundle.open_review_artifact(&token_tampered).unwrap();
        (file, artifact.output.clone())
    };
    let step1_engine =
        super::super::review_job::review_media_engine_for_data_dir(step1_data_dir).unwrap();
    let sub_report = step1_engine
        .verify_review_overlays(
            tampered_file,
            &tampered_timing,
            &frozen_step1_exp,
            &step1_inputs,
            &OverlayVerificationConfig::default(),
            &kinewright_core::ExportCancellation::default(),
        )
        .unwrap();
    assert_eq!(
        sub_report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_SUBSTITUTED_TEXT_FAILS_HOST"
    );
    assert!(
        sub_report.findings.iter().any(|f| matches!(
            f.detail,
            OverlayFindingKind::CompositorReferenceMismatch { .. }
        )),
        "S5_C6_SUBSTITUTED_FINDING_TYPED: {:?}",
        sub_report.findings
    );
    let sub_status = shared_expected
        .lock()
        .unwrap()
        .record_overlay_verification(&token_expected, binding_sub, &config, &sub_report)
        .expect("S5_C6_RECORD_SUBSTITUTED_FAILED");
    let ReviewPhase::Checking(checks_sub) = sub_status.phase else {
        panic!("expected Checking");
    };
    assert_eq!(
        checks_sub
            .rows
            .get(&RequiredReviewRow::OverlayVerification)
            .map(|r| r.result),
        Some(ReviewRowResult::Failed),
        "S5_C6_SUBSTITUTED_RECORDED_FAILED"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_scripted_reviewer_stills_mcp_and_judgment_gates() {
    use super::super::review_overlays::C6CheckConfiguration;
    use super::super::review_stills::{
        VisualFinding, VisualFindingSeverity, VisualJudgment, VisualVerdict, parse_visual_judgment,
    };
    use kinewright_core::{
        CaptionPreset, Rational, SolidColor, TimeCode, Title, TitlePosition, Track, TrackId,
        TrackKind,
        output_review::{RequiredReviewRow, ReviewRowResult},
    };
    use kinewright_media::{ReviewStillKind, ReviewStillReason};

    let config = C6CheckConfiguration::default();

    let title = Title {
        text: "Closing Beat".into(),
        font_size_token: 1,
        color_token: 0,
        position: TitlePosition::Top,
        background_scrim: true,
        fade_in_frames: TimeCode::ZERO,
        fade_out_frames: TimeCode::ZERO,
        caption_preset: None,
    };
    let doc = Document {
        fps: Rational::new(24, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(18),
        tracks: vec![
            Track {
                id: TrackId(1),
                name: Some("Video".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![
                    s5_c6_solid_clip(
                        1,
                        0,
                        9,
                        SolidColor {
                            r: 30,
                            g: 64,
                            b: 102,
                        },
                    ),
                    s5_c6_solid_clip(
                        2,
                        9,
                        9,
                        SolidColor {
                            r: 90,
                            g: 40,
                            b: 62,
                        },
                    ),
                ],
            },
            Track {
                id: TrackId(2),
                name: Some("Captions".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_title_clip(
                    3,
                    0,
                    9,
                    CaptionPreset::Clean.title("Opening line"),
                    vec![],
                )],
            },
            Track {
                id: TrackId(3),
                name: Some("Titles".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_title_clip(4, 9, 9, title, vec![])],
            },
        ],
        ..Document::default()
    };
    doc.validate().unwrap();

    let mut f = Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font);
    let cut_path = f.temp.path("cut_clip.mp4");
    s5_c6_generate_bt709_clip(&cut_path, 12);
    {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let (asset, _) = host
            .owner_mut()
            .unwrap()
            .import_owned_media(&cut_path)
            .unwrap();
        host.core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::DeleteClip {
                    clip: kinewright_core::ClipId(2),
                },
            ))
            .unwrap();
        host.core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::AddClip {
                    track: TrackId(1),
                    asset: asset.id,
                    at: TimeCode(9),
                    source: TimeCode::ZERO..TimeCode(9),
                },
            ))
            .unwrap();
    }
    ok(
        f.editor
            .request(HostCommand::Save {
                snapshot: host_snapshot_for_test(&f.host),
            })
            .unwrap(),
        "S5_C6_SAVE_CUT_CARD",
    );
    {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        let mut candidate = owner
            .fork_durable_branch(
                "candidate-cut",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        f.brief.inputs = owner.current_inputs_ref().clone();
        f.brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
    }
    let (run, parent, director) = c4_director(&f);
    let (editor_attempt, _, editor) = s5_child(&f, &director, &parent, Role::Editor);
    let (attempt_a, _, reviewer_a) = s5_child(&f, &director, &parent, Role::Reviewer);

    let v1 = s5_c6_publish_version(&f, None, vec![]);
    let audit_args = serde_json::json!({
        "version": v1.version,
        "snapshot": v1,
        "rulebook_ref": null,
        "entries": [
            {
                "line_or_beat": "Fix beat",
                "output": {"start_micros": 500_000, "end_micros": 625_000},
                "text": "Missing required beat before repair.",
                "kind": "retention",
                "finding": "missing_required_beat"
            },
            {
                "line_or_beat": "Hook",
                "output": {"start_micros": 0, "end_micros": 100_000},
                "text": "Hook works.",
                "kind": "hook",
                "finding": "resolved"
            },
            {
                "line_or_beat": "Story",
                "output": {"start_micros": 100_000, "end_micros": 200_000},
                "text": "Story works.",
                "kind": "story",
                "finding": "resolved"
            }
        ]
    });
    s5_accepted(&reviewer_a, "record_editorial_audit", audit_args);
    let v2 = s5_c6_publish_version(&f, Some(v1.version), vec![]);

    holder(&mut f);
    let token = s5_c6_start_and_finish_encode(&f, &v2);
    let shared = f.host.owner.upgrade().unwrap();
    let binding_a = ReviewCheckBinding {
        configuration_sha256: config.configuration_sha256(),
        reviewer_attempt: attempt_a.attempt.0.clone(),
    };
    shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding_a.clone()))
        .expect("S5_C6_BIND_ATTEMPT_A");

    // 1. Role enforcement: Director and Editor cannot list or invoke read_review_stills.
    let list_tools = |mcp: &C4Mcp| -> Vec<String> {
        mcp.runtime
            .block_on(mcp.client.list_all_tools())
            .unwrap()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    };
    assert!(
        list_tools(&reviewer_a)
            .iter()
            .any(|n| n == "read_review_stills"),
        "S5_C6_REVIEWER_LISTS_STILLS"
    );
    assert!(
        !list_tools(&director)
            .iter()
            .any(|n| n == "read_review_stills"),
        "S5_C6_DIRECTOR_HIDES_STILLS"
    );
    assert!(
        !list_tools(&editor)
            .iter()
            .any(|n| n == "read_review_stills"),
        "S5_C6_EDITOR_HIDES_STILLS"
    );
    assert!(
        s5_refusal(
            &director,
            "read_review_stills",
            serde_json::json!({}),
            "tool hidden for durable task role",
        )
        .is_none(),
        "S5_C6_DIRECTOR_CALL_REFUSED"
    );
    assert!(
        s5_refusal(
            &editor,
            "read_review_stills",
            serde_json::json!({}),
            "tool hidden for durable task role",
        )
        .is_none(),
        "S5_C6_EDITOR_CALL_REFUSED"
    );
    {
        let state = f.projection(&run);
        let editor_record = state
            .tasks
            .iter()
            .find(|task| task.id == editor_attempt.task)
            .unwrap();
        let mut owner = shared.lock().unwrap();
        let editor_ep = owner.hooks.runs[&run].tasks[&editor_attempt.task].endpoint();
        let direct_err = owner
            .prepare_review_stills_read(
                editor_record,
                editor_ep.authority(),
                &super::super::review_stills::ReviewStillPageRequest::default(),
            )
            .err()
            .expect("S5_C6_HOST_GUARD_REFUSES_EDITOR");
        assert!(
            direct_err.to_string().contains("Role::Reviewer"),
            "S5_C6_HOST_GUARD_MSG: {direct_err}"
        );
    }

    drop(editor);
    {
        let producer = f.producer_client(&run);
        let mut state = f.projection(&run);
        for attempt in &mut state.attempts {
            if attempt.identity.task == editor_attempt.task {
                attempt.state = AttemptState::Retired(AttemptOutcome::Cancelled);
            }
        }
        if let Some(task) = state
            .tasks
            .iter_mut()
            .find(|task| task.id == editor_attempt.task)
        {
            task.state = TaskState::Cancelled;
            task.progress.capacity = DependencyReservation::Free;
        }
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        publish_projection(&f, &producer, &run, state, None);
    }

    // 2. Scripted Reviewer A reads all pages over real HTTP MCP with real PNG image blocks.
    let (stills_a, first_cursor_a, pages_a) = s5_c6_read_all_stills(&reviewer_a, 2);
    assert!(pages_a >= 2, "S5_C6_MULTI_PAGE_READ");
    assert!(first_cursor_a.is_some(), "S5_C6_HAS_NEXT_CURSOR");
    assert!(
        stills_a
            .iter()
            .any(|s| s.kind == ReviewStillKind::DecodedOutput),
        "S5_C6_HAS_DECODED_STILLS"
    );
    assert!(
        stills_a
            .iter()
            .any(|s| s.kind == ReviewStillKind::OverlayReference),
        "S5_C6_HAS_REFERENCE_STILLS"
    );
    for required_reason in [
        ReviewStillReason::Opening,
        ReviewStillReason::Middle,
        ReviewStillReason::End,
        ReviewStillReason::Cut,
        ReviewStillReason::OverlayTransition,
        ReviewStillReason::RepairedRegion,
    ] {
        assert!(
            stills_a
                .iter()
                .any(|s| s.reasons.contains(&required_reason)),
            "S5_C6_COVERS_{required_reason:?}"
        );
    }

    // 3. Missing planned still is refused and does not record the row.
    let all_ids_a: Vec<_> = stills_a.iter().map(|s| s.still_id.clone()).collect();
    let missing_one = VisualJudgment {
        verdict: VisualVerdict::Pass,
        reviewed_still_ids: all_ids_a[..all_ids_a.len() - 1].to_vec(),
        findings: vec![],
    };
    let missing_err = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding_a.clone(), &config, &missing_one)
        .expect_err("S5_C6_MISSING_STILL_REFUSED");
    assert!(
        missing_err
            .to_string()
            .contains("missing planned review still"),
        "S5_C6_MISSING_STILL_MSG: {missing_err}"
    );

    // 4. Contradictory verdicts are refused.
    let contradictory = VisualJudgment {
        verdict: VisualVerdict::Pass,
        reviewed_still_ids: all_ids_a.clone(),
        findings: vec![VisualFinding {
            still_id: all_ids_a[0].clone(),
            severity: VisualFindingSeverity::Blocking,
            summary: "Caption clipped".into(),
        }],
    };
    let contra_err = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding_a.clone(), &config, &contradictory)
        .expect_err("S5_C6_CONTRADICTORY_REFUSED");
    assert!(
        contra_err.to_string().contains("contradictory"),
        "S5_C6_CONTRADICTORY_MSG: {contra_err}"
    );

    // 5. Valid Pass covering all planned stills parses cleanly and records Pass via record_review_check.
    let pass_json = serde_json::to_vec(&VisualJudgment {
        verdict: VisualVerdict::Pass,
        reviewed_still_ids: all_ids_a.clone(),
        findings: vec![],
    })
    .unwrap();
    let parsed_pass = parse_visual_judgment(&pass_json).unwrap();
    let (validated_pass, status_pass) = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding_a, &config, &parsed_pass)
        .expect("S5_C6_VALID_PASS_RECORDED");
    assert_eq!(validated_pass.row_result, ReviewRowResult::Pass);
    let ReviewPhase::Checking(checks_pass) = status_pass.phase else {
        panic!("expected Checking");
    };
    assert_eq!(
        checks_pass
            .rows
            .get(&RequiredReviewRow::IndependentVisualReview)
            .map(|r| r.result),
        Some(ReviewRowResult::Pass),
        "S5_C6_VISUAL_PASS_PERSISTED"
    );

    // 6. Unbind Attempt A and bind fresh Attempt B on the exact same artifact.
    shared
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .update_review_job(&token, UpdateReviewJob::UnbindChecks)
        .expect("S5_C6_UNBIND_ATTEMPT_A");
    let (attempt_b, _, reviewer_b) = s5_child(&f, &director, &parent, Role::Reviewer);
    let binding_b = ReviewCheckBinding {
        configuration_sha256: config.configuration_sha256(),
        reviewer_attempt: attempt_b.attempt.0.clone(),
    };
    shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding_b.clone()))
        .expect("S5_C6_BIND_ATTEMPT_B");

    assert!(
        s5_refusal(
            &reviewer_a,
            "read_review_stills",
            serde_json::json!({}),
            "not bound to the active output review check",
        )
        .is_none(),
        "S5_C6_OLD_ATTEMPT_READ_REFUSED"
    );
    assert!(
        s5_refusal(
            &reviewer_b,
            "read_review_stills",
            serde_json::json!({ "cursor": first_cursor_a.unwrap() }),
            "does not match the active review artifact, plan, or reviewer attempt",
        )
        .is_none(),
        "S5_C6_OTHER_ATTEMPT_CURSOR_REFUSED"
    );

    let (stills_b, _, _) = s5_c6_read_all_stills(&reviewer_b, 4);
    let mut mixed_ids: Vec<_> = stills_b.iter().map(|s| s.still_id.clone()).collect();
    mixed_ids[0] = all_ids_a[0].clone();
    let stale_attempt_judgment = VisualJudgment {
        verdict: VisualVerdict::Pass,
        reviewed_still_ids: mixed_ids,
        findings: vec![],
    };
    let stale_err = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding_b.clone(), &config, &stale_attempt_judgment)
        .expect_err("S5_C6_OTHER_ATTEMPT_STILL_ID_REFUSED");
    assert!(
        stale_err
            .to_string()
            .contains("belongs to reviewer attempt"),
        "S5_C6_OTHER_ATTEMPT_MSG: {stale_err}"
    );

    let all_ids_b: Vec<_> = stills_b.iter().map(|s| s.still_id.clone()).collect();
    let blocking_judgment = VisualJudgment {
        verdict: VisualVerdict::Findings,
        reviewed_still_ids: all_ids_b.clone(),
        findings: vec![VisualFinding {
            still_id: all_ids_b[0].clone(),
            severity: VisualFindingSeverity::Blocking,
            summary: "Lower-third caption overlaps graphic edge.".into(),
        }],
    };
    let (validated_fail, status_fail) = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding_b.clone(), &config, &blocking_judgment)
        .expect("S5_C6_BLOCKING_FINDING_RECORDED");
    assert_eq!(validated_fail.row_result, ReviewRowResult::Failed);
    assert_eq!(validated_fail.blocking_findings, 1);
    let ReviewPhase::Checking(checks_fail) = status_fail.phase else {
        panic!("expected Checking");
    };
    assert_eq!(
        checks_fail
            .rows
            .get(&RequiredReviewRow::IndependentVisualReview)
            .map(|r| (r.attempt.clone(), r.result)),
        Some((attempt_b.attempt.0, ReviewRowResult::Failed)),
        "S5_C6_BLOCKING_RECORDED_FOR_ATTEMPT_B"
    );

    // 7. Stop fence refuses subsequent read_review_stills calls over MCP AND refuses direct
    // record_visual_review_judgment (Opus m7 / GLM 7).
    s5_review_call(&f.editor, ReviewControl::Stop(token.clone())).unwrap();
    let after_stop = reviewer_b.wire("read_review_stills", serde_json::json!({}));
    assert_eq!(
        after_stop.is_error,
        Some(true),
        "S5_C6_STOP_FENCES_STILLS {after_stop:?}"
    );
    let after_stop_record = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding_b, &config, &blocking_judgment)
        .expect_err("S5_C6_STOP_FENCES_RECORD_VISUAL_JUDGMENT");
    assert!(
        !after_stop_record.to_string().is_empty(),
        "S5_C6_STOP_RECORD_REFUSAL_REASON"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_fix1_config_binding_and_validation_enforced() {
    use super::super::review_overlays::C6CheckConfiguration;
    use super::super::review_stills::{VisualJudgment, VisualVerdict};

    let doc = s5_c6_make_step_doc("Step 1");
    let mut f = Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font);
    let (_, parent, director) = c4_director(&f);
    let (attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    let token = s5_c6_start_and_finish_encode(&f, &key);
    let shared = f.host.owner.upgrade().unwrap();

    // 1. Invalid overlay/stills configs refused before any binding or decode.
    let mut nan_cfg = C6CheckConfiguration::default();
    nan_cfg.overlay.max_codec_error_rms = f32::NAN;
    assert!(nan_cfg.validate().is_err(), "S5_C6_FIX1_NAN_CONFIG_REFUSED");
    let mut zero_pages = C6CheckConfiguration::default();
    zero_pages.stills.max_pages_per_attempt = 0;
    assert!(
        zero_pages.validate().is_err(),
        "S5_C6_FIX1_ZERO_PAGES_REFUSED"
    );
    assert!(
        shared
            .lock()
            .unwrap()
            .set_review_stills_config(zero_pages.stills.clone())
            .is_err(),
        "S5_C6_FIX1_SET_INVALID_STILLS_CONFIG_REFUSED"
    );

    // 2. Combined C5+C6 digest seam changes configuration_sha256 deterministically.
    let c5_digest = [0x5A_u8; 32];
    let config = C6CheckConfiguration {
        other_checks_sha256: Some(c5_digest),
        ..C6CheckConfiguration::default()
    };
    assert_ne!(
        config.configuration_sha256(),
        C6CheckConfiguration::default().configuration_sha256(),
        "S5_C6_FIX1_COMBINED_C5_SEAM_CHANGES_DIGEST"
    );
    let binding = ReviewCheckBinding {
        configuration_sha256: config.configuration_sha256(),
        reviewer_attempt: attempt.attempt.0,
    };
    {
        let mut owner = shared.lock().unwrap();
        owner
            .record_review_check(&token, UpdateReviewJob::BindChecks(binding.clone()))
            .expect("S5_C6_FIX1_BIND_COMBINED");
        owner
            .register_bound_c6_configuration(&token, &binding, &config)
            .expect("S5_C6_FIX2_REGISTER_COMBINED_BOUND_CONFIG");
    }

    // 3. Mismatched overlay config refused on run_overlay_verification.
    let mut wrong_overlay = config.clone();
    wrong_overlay.overlay.max_codec_error_rms -= 1.0;
    let overlay_err = super::super::review_overlays::run_overlay_verification(
        &shared,
        &token,
        binding.clone(),
        &wrong_overlay,
        &kinewright_core::ExportCancellation::default(),
    )
    .expect_err("S5_C6_FIX1_MISMATCHED_OVERLAY_CONFIG_REFUSED");
    assert!(
        overlay_err
            .to_string()
            .contains("does not match the bound configuration digest"),
        "S5_C6_FIX1_OVERLAY_DIGEST_ERR: {overlay_err}"
    );

    // 4. Mismatched stills config refused on record_visual_review_judgment.
    let (stills, _, _) = s5_c6_read_all_stills(&reviewer, 4);
    let judgment = VisualJudgment {
        verdict: VisualVerdict::Pass,
        reviewed_still_ids: stills.iter().map(|s| s.still_id.clone()).collect(),
        findings: vec![],
    };
    let mut wrong_stills = config.clone();
    wrong_stills.stills.max_long_edge -= 1;
    let visual_err = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding.clone(), &wrong_stills, &judgment)
        .expect_err("S5_C6_FIX1_MISMATCHED_STILLS_CONFIG_REFUSED");
    assert!(
        visual_err
            .to_string()
            .contains("does not match the bound configuration digest"),
        "S5_C6_FIX1_VISUAL_DIGEST_ERR: {visual_err}"
    );

    // 5. Matching combined config passes cleanly on both rows.
    let (report, _) = super::super::review_overlays::run_overlay_verification(
        &shared,
        &token,
        binding.clone(),
        &config,
        &kinewright_core::ExportCancellation::default(),
    )
    .expect("S5_C6_FIX1_MATCHING_OVERLAY_CONFIG_PASSES");
    assert_eq!(
        report.row_result,
        kinewright_core::output_review::ReviewRowResult::Pass
    );
    let (validated, _) = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding, &config, &judgment)
        .expect("S5_C6_FIX1_MATCHING_VISUAL_CONFIG_PASSES");
    assert_eq!(
        validated.row_result,
        kinewright_core::output_review::ReviewRowResult::Pass
    );
}

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_fix1_off_lock_artifact_verification_and_stop_cancellation() {
    use super::super::review_overlays::C6CheckConfiguration;
    use std::sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    };

    let doc = s5_c6_clean_overlay_doc();
    let mut f = Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font);
    let (_, parent, director) = c4_director(&f);
    let (attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    let token = s5_c6_start_and_finish_encode(&f, &key);
    let shared = f.host.owner.upgrade().unwrap();

    // Limit per-attempt page budget to 3 (so 2 pages succeed, 3rd hits hook/Stop, or test budget first).
    let config = C6CheckConfiguration {
        stills: kinewright_media::ReviewStillsConfig {
            max_pages_per_attempt: 3,
            ..kinewright_media::ReviewStillsConfig::default()
        },
        ..C6CheckConfiguration::default()
    };
    let (binding, _) = shared
        .lock()
        .unwrap()
        .bind_c6_review_checks(&token, attempt.attempt.0, &config)
        .unwrap();

    // Part A: Prove host writer lock is NOT held while overlay artifact verification & decode run.
    let entered_overlay = Arc::new(Barrier::new(2));
    let release_overlay = Arc::new(Barrier::new(2));
    {
        let entered = Arc::clone(&entered_overlay);
        let release = Arc::clone(&release_overlay);
        shared
            .lock()
            .unwrap()
            .set_overlay_verify_hook(Some(Arc::new(move |_| {
                entered.wait();
                release.wait();
            })));
    }
    let worker_shared = Arc::clone(&shared);
    let worker_token = token.clone();
    let worker_binding = binding.clone();
    let worker_config = config.clone();
    let overlay_thread = std::thread::spawn(move || {
        super::super::review_overlays::run_overlay_verification(
            &worker_shared,
            &worker_token,
            worker_binding,
            &worker_config,
            &kinewright_core::ExportCancellation::default(),
        )
    });
    entered_overlay.wait();
    // Host lock MUST be free while overlay verification is about to hash/decode off-lock (Opus M4).
    {
        let guard = shared
            .try_lock()
            .expect("S5_C6_FIX1_HOST_LOCK_FREE_DURING_OVERLAY_HASH");
        assert!(guard.review_still_plan(&token, &config.stills).is_ok());
    }
    release_overlay.wait();
    let (report, _) = overlay_thread.join().unwrap().unwrap();
    assert_eq!(
        report.row_result,
        kinewright_core::output_review::ReviewRowResult::Pass
    );
    shared.lock().unwrap().set_overlay_verify_hook(None);

    // Part B: Enforce per-attempt page budget (max_pages_per_attempt = 3 -> 2 pages ok here, 3rd
    // tested with in-flight Stop cancellation, and fresh attempt proves budget exhaustion cleanly).
    let r1 = reviewer.wire("read_review_stills", serde_json::json!({ "page_size": 1 }));
    assert_eq!(r1.is_error, Some(false), "S5_C6_FIX1_PAGE_1_OK");
    let r2 = reviewer.wire("read_review_stills", serde_json::json!({ "page_size": 1 }));
    assert_eq!(r2.is_error, Some(false), "S5_C6_FIX1_PAGE_2_OK");

    // Part C: Prove host lock is FREE during read_review_stills off-lock hashing/decoding AND
    // ReviewControl::Stop cancels the in-flight read_review_stills call deterministically (Opus M4 + M5).
    let entered_stills = Arc::new(Barrier::new(2));
    let release_stills = Arc::new(Barrier::new(2));
    let observed_cancelled = Arc::new(AtomicBool::new(false));
    {
        let entered = Arc::clone(&entered_stills);
        let release = Arc::clone(&release_stills);
        let cancelled_flag = Arc::clone(&observed_cancelled);
        shared
            .lock()
            .unwrap()
            .set_review_stills_render_hook(Some(Arc::new(move |cancel| {
                entered.wait();
                release.wait();
                cancelled_flag.store(cancel.is_cancelled(), Ordering::SeqCst);
            })));
    }
    // Drive the 3rd MCP page call from a worker thread while we inspect the host lock & fire Stop.
    let stills_thread = std::thread::spawn(move || {
        let reply = reviewer.wire("read_review_stills", serde_json::json!({ "page_size": 1 }));
        (reviewer, reply)
    });
    entered_stills.wait();
    assert!(
        shared.try_lock().is_ok(),
        "S5_C6_FIX1_HOST_LOCK_FREE_DURING_STILLS_HASH"
    );
    // Fire Stop while read_review_stills is suspended off-lock!
    s5_review_call(&f.editor, ReviewControl::Stop(token.clone())).unwrap();
    release_stills.wait();
    let (_reviewer, cancelled_reply) = stills_thread.join().unwrap();
    assert!(
        observed_cancelled.load(Ordering::SeqCst),
        "S5_C6_FIX1_STOP_CANCELLED_INFLIGHT_STILLS_TOKEN"
    );
    assert_eq!(
        cancelled_reply.is_error,
        Some(true),
        "S5_C6_FIX1_INFLIGHT_STILLS_REFUSED_ON_STOP: {cancelled_reply:?}"
    );
    shared.lock().unwrap().set_review_stills_render_hook(None);

    // Part D: Direct per-attempt page budget exhaustion witness on a fresh review job (max 2 pages).
    let doc2 = s5_c6_clean_overlay_doc();
    let mut f2 = Fixture::with_inputs(false, doc2, "project.kinewright", s5_c6_admit_embedded_font);
    let (_, parent2, director2) = c4_director(&f2);
    let (attempt2, _, reviewer2) = s5_child(&f2, &director2, &parent2, Role::Reviewer);
    holder(&mut f2);
    let key2 = s5_c6_publish_version(&f2, None, vec![]);
    let token2 = s5_c6_start_and_finish_encode(&f2, &key2);
    let shared2 = f2.host.owner.upgrade().unwrap();
    let budget_cfg = C6CheckConfiguration {
        stills: kinewright_media::ReviewStillsConfig {
            max_pages_per_attempt: 2,
            ..kinewright_media::ReviewStillsConfig::default()
        },
        ..C6CheckConfiguration::default()
    };
    shared2
        .lock()
        .unwrap()
        .bind_c6_review_checks(&token2, attempt2.attempt.0, &budget_cfg)
        .unwrap();
    assert_eq!(
        reviewer2
            .wire("read_review_stills", serde_json::json!({ "page_size": 1 }))
            .is_error,
        Some(false)
    );
    assert_eq!(
        reviewer2
            .wire("read_review_stills", serde_json::json!({ "page_size": 1 }))
            .is_error,
        Some(false)
    );
    let third = reviewer2.wire("read_review_stills", serde_json::json!({ "page_size": 1 }));
    assert_eq!(
        third.is_error,
        Some(true),
        "S5_C6_FIX1_PAGE_BUDGET_REFUSES_THIRD_PAGE: {third:?}"
    );
    assert!(
        format!("{third:?}").contains("page budget exhausted"),
        "S5_C6_FIX1_PAGE_BUDGET_REASON: {third:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_fix1_cross_version_clock_mapping_and_uncovered_downgrades_to_unknown() {
    use super::super::review_overlays::C6CheckConfiguration;
    use super::super::review_stills::{VisualJudgment, VisualVerdict};
    use kinewright_core::{
        Rational, TimeCode, Track, TrackId, TrackKind,
        output_review::{RequiredReviewRow, ReviewRowResult},
    };
    use kinewright_media::ReviewStillReason;
    use kinewright_project::studio_records::{
        CommentId, FeedbackAnchor, FeedbackRevision, FeedbackTarget, FrameRate, MetadataScope,
        Orientation,
    };

    let base_doc = Document {
        fps: Rational::new(24, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode::ZERO,
        tracks: vec![Track {
            id: TrackId(1),
            name: Some("Video".into()),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    };
    base_doc.validate().unwrap();
    let mut f = Fixture::with_inputs(
        false,
        base_doc,
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    let clip_path = f.temp.path("shift_clip.mp4");
    s5_c6_generate_bt709_clip(&clip_path, 24);
    let clip_id = {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let (asset, _) = host
            .owner_mut()
            .unwrap()
            .import_owned_media(&clip_path)
            .unwrap();
        host.core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::AddClip {
                    track: TrackId(1),
                    asset: asset.id,
                    at: TimeCode(0),
                    source: TimeCode::ZERO..TimeCode(24),
                },
            ))
            .unwrap();
        (0..16)
            .rev()
            .find_map(|depth| {
                host.core()
                    .boundary(depth)
                    .ok()
                    .and_then(|b| b.document().tracks[0].clips.first().map(|c| c.id))
            })
            .expect("S5_C6_FIX1_CLIP_ADDED")
    };
    ok(
        f.editor
            .request(HostCommand::Save {
                snapshot: host_snapshot_for_test(&f.host),
            })
            .unwrap(),
        "S5_C6_FIX1_SAVE_V1",
    );
    let base_v1 = {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        let mut cand = owner
            .fork_durable_branch(
                "cand-v1",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut cand).unwrap();
        let base_v1 = owner.candidate_binding(cand.branch_id()).unwrap();
        f.brief.inputs = owner.current_inputs_ref().clone();
        f.brief.scope.base = base_v1.clone();
        base_v1
    };
    let (_, parent, director) = c4_director(&f);
    let (attempt_v1, _, reviewer_v1) = s5_child(&f, &director, &parent, Role::Reviewer);
    let v1 = s5_c6_publish_version(&f, None, vec![]);

    // Record blocking editorial audit on v1 at frames 14..16 (583_333..666_667 us).
    let audit_args = serde_json::json!({
        "version": v1.version,
        "snapshot": v1,
        "rulebook_ref": null,
        "entries": [
            {
                "line_or_beat": "Shifted beat",
                "output": {"start_micros": 583_333, "end_micros": 666_667},
                "text": "Check repair after ripple trim.",
                "kind": "retention",
                "finding": "missing_required_beat"
            },
            {
                "line_or_beat": "Hook",
                "output": {"start_micros": 0, "end_micros": 100_000},
                "text": "Hook works.",
                "kind": "hook",
                "finding": "resolved"
            },
            {
                "line_or_beat": "Story",
                "output": {"start_micros": 100_000, "end_micros": 200_000},
                "text": "Story works.",
                "kind": "story",
                "finding": "resolved"
            }
        ]
    });
    s5_accepted(&reviewer_v1, "record_editorial_audit", audit_args);

    // Save two FeedbackRevisions anchored on v1:
    // (1) Mappable revision targeting clip_id at v1 frame 14 (source frame 14).
    // (2) Unmappable revision with target: None (cannot resolve temporally onto a shifted v2 cut).
    let (mappable_rev_ref, unmappable_rev_ref) = {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        let bundle = owner.review_bundle().unwrap();
        let snap = bundle.read_snapshot(&v1).unwrap();
        let cp = bundle
            .store()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &snap.descriptor.checkpoint_ref,
            )
            .unwrap();
        let reader = cp
            .open(bundle.store(), bundle.history_pool())
            .unwrap()
            .into_reader()
            .unwrap();
        let depth = reader.history().undo().len();
        let boundary = reader.into_boundary(depth).unwrap();
        let occurrence = boundary.lineage().occurrences[&clip_id].clone();
        drop(boundary);
        drop(cp);
        drop(snap);

        let text_ref = owner
            .admit_host_grant(b"Check frame 14 after trim")
            .unwrap();
        let strokes_ref = owner.admit_host_grant(b"[[0.5,0.5]]").unwrap();
        let proof_ref = owner.admit_host_grant(b"proof-v1-14").unwrap();

        let save_rev = |owner: &mut Ja1EditingSession, target: Option<FeedbackTarget>| {
            let issuer = owner.reuse_metadata_issuer_for("fixture-s5-c6-fb").unwrap();
            let envelope = owner
                .metadata_envelope(CommandId {
                    sequence: owner.metadata_issuer_sequence(&issuer).unwrap(),
                    issuer,
                })
                .unwrap();
            let rev = FeedbackRevision {
                comment_id: CommentId::fresh().unwrap(),
                revision: 1,
                predecessor: None,
                anchor: FeedbackAnchor {
                    snapshot: v1.clone(),
                    frame: 14,
                    fps: FrameRate {
                        numerator: 24,
                        denominator: 1,
                    },
                    raster_size: [320, 180],
                    orientation: Orientation::Upright,
                    target,
                    source_ref: None,
                    script_beat_id: None,
                },
                original_text_ref: text_ref.clone(),
                strokes_ref: strokes_ref.clone(),
                frame_proof_ref: proof_ref.clone(),
                voice_ref: None,
                transcript_revision: None,
            };
            let update = MetadataUpdate {
                expected_root_generation: owner.metadata_generation().unwrap(),
                change: MetadataChange::SaveFeedback(rev),
            };
            let kinewright_project::receipts::ReceiptLookup::Committed(receipt) = owner
                .metadata_command(&envelope, MetadataScope::Feedback, &update)
                .unwrap()
            else {
                panic!("feedback committed");
            };
            let ReceiptResult::Metadata {
                result: MetadataResult::Feedback(reference),
            } = receipt.result
            else {
                panic!("feedback ref");
            };
            reference
        };
        let mappable = save_rev(
            owner,
            Some(FeedbackTarget {
                local_clip_id: clip_id,
                occurrence_id: Some(occurrence),
                local_frame: Some(14),
            }),
        );
        let unmappable = save_rev(owner, None);
        (mappable, unmappable)
    };

    // Edit timeline for v2: trim 8 frames off the front of clip_id and move it to timeline_start=0,
    // so source frame 14 (previously output frame 14 in v1) now sits at output frame 6 in v2!
    {
        let shared = f.host.owner.upgrade().unwrap();
        let host = shared.lock().unwrap();
        host.core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::TrimClip {
                    clip: clip_id,
                    new_source: TimeCode(8)..TimeCode(24),
                },
            ))
            .unwrap();
        host.core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::MoveClip {
                    clip: clip_id,
                    to_track: TrackId(1),
                    to: TimeCode(0),
                },
            ))
            .unwrap();
    }
    ok(
        f.editor
            .request(HostCommand::Save {
                snapshot: host_snapshot_for_test(&f.host),
            })
            .unwrap(),
        "S5_C6_FIX1_SAVE_V2",
    );
    {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        let mut cand = owner
            .fork_durable_branch(
                "cand-v2",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut cand).unwrap();
        f.brief.inputs = owner.current_inputs_ref().clone();
        f.brief.scope.base = owner.candidate_binding(cand.branch_id()).unwrap();
    }

    // Publish v2 with parent=v1 and the mappable cross-version feedback revision.
    let v2 = s5_c6_publish_version(&f, Some(v1.version.clone()), vec![mappable_rev_ref]);
    holder(&mut f);
    let token_v2 = s5_c6_start_and_finish_encode(&f, &v2);
    let shared = f.host.owner.upgrade().unwrap();
    let config = C6CheckConfiguration::default();
    let plan_v2 = shared
        .lock()
        .unwrap()
        .review_still_plan(&token_v2, &config.stills)
        .expect("S5_C6_FIX1_V2_PLAN");
    assert_eq!(
        plan_v2.uncovered_regions_total, 0,
        "S5_C6_FIX1_V2_ZERO_UNCOVERED: {:?}",
        plan_v2.uncovered_regions
    );
    // FeedbackRevision anchored at v1 frame 14 MUST map to v2 frame 6 (NOT stale frame 14!).
    let commented_frames: Vec<u64> = plan_v2
        .samples
        .iter()
        .filter(|s| s.reasons.contains(&ReviewStillReason::CommentedRegion))
        .map(|s| s.frame_index)
        .collect();
    assert!(
        commented_frames.contains(&6),
        "S5_C6_FIX1_FEEDBACK_SHIFTED_TO_FRAME_6: got {commented_frames:?}"
    );
    assert!(
        !commented_frames.contains(&14),
        "S5_C6_FIX1_NO_STALE_V1_FRAME_14: got {commented_frames:?}"
    );
    // Parent blocking audit at v1 frames 14..16 MUST map to v2 frame 6..8 (midpoint frame 7, NOT 14..16!).
    let repaired_frames: Vec<u64> = plan_v2
        .samples
        .iter()
        .filter(|s| s.reasons.contains(&ReviewStillReason::RepairedRegion))
        .map(|s| s.frame_index)
        .collect();
    assert!(
        repaired_frames.iter().any(|&fr| (6..=8).contains(&fr)),
        "S5_C6_FIX1_AUDIT_SHIFTED_TO_V2_FRAMES_6_TO_8: got {repaired_frames:?}"
    );
    assert!(
        repaired_frames.iter().all(|&fr| fr < 12),
        "S5_C6_FIX1_NO_STALE_V1_AUDIT_CLOCK: got {repaired_frames:?}"
    );

    // Stop v2 review and publish v3 carrying the unmappable feedback revision -> uncovered_regions_total >= 1
    // -> Reviewer Pass MUST downgrade to ReviewRowResult::Unknown (L12, Opus M6).
    s5_review_call(&f.editor, ReviewControl::Stop(token_v2)).unwrap();
    f.brief.scope.base = base_v1;
    let v3 = s5_c6_publish_version(&f, None, vec![unmappable_rev_ref]);
    let token_v3 = s5_c6_start_and_finish_encode(&f, &v3);
    let binding_v3 = ReviewCheckBinding {
        configuration_sha256: config.configuration_sha256(),
        reviewer_attempt: attempt_v1.attempt.0,
    };
    shared
        .lock()
        .unwrap()
        .record_review_check(&token_v3, UpdateReviewJob::BindChecks(binding_v3.clone()))
        .unwrap();
    let plan_v3 = shared
        .lock()
        .unwrap()
        .review_still_plan(&token_v3, &config.stills)
        .unwrap();
    assert!(
        plan_v3.uncovered_regions_total >= 1,
        "S5_C6_FIX1_UNMAPPABLE_FEEDBACK_RECORDED_UNCOVERED"
    );
    let (stills_v3, _, _) = s5_c6_read_all_stills(&reviewer_v1, 4);
    let pass_judgment = VisualJudgment {
        verdict: VisualVerdict::Pass,
        reviewed_still_ids: stills_v3.iter().map(|s| s.still_id.clone()).collect(),
        findings: vec![],
    };
    let (validated_v3, status_v3) = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token_v3, binding_v3, &config, &pass_judgment)
        .expect("S5_C6_FIX1_RECORD_UNCOVERED_UNKNOWN");
    assert_eq!(
        validated_v3.row_result,
        ReviewRowResult::Unknown,
        "S5_C6_FIX1_UNCOVERED_DOWNGRADES_PASS_TO_UNKNOWN"
    );
    assert!(validated_v3.uncovered_regions_total >= 1);
    let ReviewPhase::Checking(checks_v3) = status_v3.phase else {
        panic!("expected Checking");
    };
    assert_eq!(
        checks_v3
            .rows
            .get(&RequiredReviewRow::IndependentVisualReview)
            .map(|r| r.result),
        Some(ReviewRowResult::Unknown),
        "S5_C6_FIX1_UNKNOWN_PERSISTED_FOR_UNCOVERED_REGION"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_fix1_live_reviewer_grant_gate_refuses_without_lead_grant() {
    use super::super::review_live_grant::{
        REVIEW_LIVE_WITNESS_NAME, ReviewLiveGrant, consume_from_account_home, consume_from_dir,
        open_private_dir_for_test,
    };
    use std::os::unix::fs::PermissionsExt;

    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "s5-c6-grant",
    );
    let fake_guardian = temp.path("kinewright-job-guardian");
    std::fs::write(&fake_guardian, b"guardian-binary-bytes").unwrap();
    let guardian_sha = kinewright_media::sha256_file(&fake_guardian).unwrap();

    // 1. Account-home gate refuses cleanly without a lead grant file.
    let err = consume_from_account_home(&fake_guardian)
        .expect_err("S5_C6_FIX1_ACCOUNT_HOME_REFUSES_WITHOUT_GRANT");
    assert!(
        err.to_string().contains("S5_C6_NO_LEAD_GRANT"),
        "S5_C6_FIX1_REFUSAL_CODE: {err}"
    );

    // 2. Private directory grant consumption: valid once, refuses replay, shared run_identity,
    // expiry, wrong guardian hash, and non-private permissions.
    let grants_dir = temp.path("grants");
    std::fs::create_dir(&grants_dir).unwrap();
    std::fs::set_permissions(&grants_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir_fd = open_private_dir_for_test(&grants_dir).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let write_grant = |id: &str, grant: &ReviewLiveGrant, mode: u32| {
        let p = grants_dir.join(format!("{id}.json"));
        std::fs::write(&p, serde_json::to_vec(grant).unwrap()).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let valid = ReviewLiveGrant {
        version: 1,
        id: "grant-ok-1".into(),
        witness: REVIEW_LIVE_WITNESS_NAME.into(),
        provider: "codex".into(),
        model: "gpt-6.1-sol".into(),
        options: vec![],
        wall_millis: 120_000,
        tokens_per_task: 50_000,
        run_identity: "0123456789abcdef0123456789abcdef".into(),
        expires_at_unix_seconds: now + 300,
        guardian_sha256: guardian_sha.clone(),
    };
    write_grant("grant-ok-1", &valid, 0o600);
    let consumed =
        consume_from_dir(&dir_fd, "grant-ok-1", &guardian_sha).expect("S5_C6_FIX1_GRANT_ONCE");
    assert_eq!(consumed, valid);

    // Replay of same id is refused.
    assert!(
        consume_from_dir(&dir_fd, "grant-ok-1", &guardian_sha)
            .expect_err("S5_C6_FIX1_REPLAY_REFUSED")
            .to_string()
            .contains("already consumed")
    );

    // Different id reusing same run_identity is refused.
    let mut dup_run = valid.clone();
    dup_run.id = "grant-dup-run".into();
    write_grant("grant-dup-run", &dup_run, 0o600);
    assert!(
        consume_from_dir(&dir_fd, "grant-dup-run", &guardian_sha)
            .expect_err("S5_C6_FIX1_DUP_RUN_ID_REFUSED")
            .to_string()
            .contains("already consumed")
    );

    // Expired grant is refused.
    let mut expired = valid.clone();
    expired.id = "grant-expired".into();
    expired.run_identity = "fedcba9876543210fedcba9876543210".into();
    expired.expires_at_unix_seconds = now.saturating_sub(10);
    write_grant("grant-expired", &expired, 0o600);
    assert!(
        consume_from_dir(&dir_fd, "grant-expired", &guardian_sha)
            .expect_err("S5_C6_FIX1_EXPIRED_REFUSED")
            .to_string()
            .contains("expired")
    );

    // Wrong guardian digest is refused.
    let mut wrong_g = valid.clone();
    wrong_g.id = "grant-wrong-guardian".into();
    wrong_g.run_identity = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    wrong_g.guardian_sha256 = "0".repeat(64);
    write_grant("grant-wrong-guardian", &wrong_g, 0o600);
    assert!(
        consume_from_dir(&dir_fd, "grant-wrong-guardian", &guardian_sha)
            .expect_err("S5_C6_FIX1_GUARDIAN_MISMATCH_REFUSED")
            .to_string()
            .contains("guardian SHA-256 differs")
    );

    // World-readable grant file is refused.
    let mut world = valid;
    world.id = "grant-world".into();
    world.run_identity = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    write_grant("grant-world", &world, 0o644);
    assert!(
        consume_from_dir(&dir_fd, "grant-world", &guardian_sha)
            .expect_err("S5_C6_FIX1_WORLD_READABLE_REFUSED")
            .to_string()
            .contains("private owned regular file")
    );
}

#[cfg(target_os = "linux")]
fn s5_c6_extract_json_object(raw: &[u8]) -> &[u8] {
    let text = std::str::from_utf8(raw).unwrap_or("");
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}'))
        && start < end
    {
        return &text.as_bytes()[start..=end];
    }
    raw
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "on-demand: live reviewer vision model witness; requires a lead-owned one-use private grant file in ~/.local/share/kinewright-eval/grants/"]
fn s5_c6_live_reviewer_identifies_decoded_only_discrepancy() {
    use super::super::review_live_grant::consume_from_account_home;
    use super::super::review_overlays::C6CheckConfiguration;
    use super::super::review_stills::{VisualVerdict, parse_visual_judgment};
    use kinewright_core::output_review::ReviewRowResult;

    // 1. Refuse before starting any provider unless a valid one-use lead grant file is consumed.
    let exe = std::env::current_exe().expect("current_exe");
    let exe_dir = exe.parent().expect("exe parent");
    let bin_dir = if exe_dir.file_name().is_some_and(|n| n == "deps") {
        exe_dir.parent().expect("target dir")
    } else {
        exe_dir
    };
    let guardian = bin_dir.join("kinewright-job-guardian");
    let grant = consume_from_account_home(&guardian).expect("S5_C6_NO_LEAD_GRANT");

    // 2. Encode a real decoded-only discrepancy MP4 ("WRONG TEXT") while the frozen project and
    // expectations expect "Step 1".
    let step1_doc = s5_c6_make_step_doc("Step 1");
    let wrong_doc = s5_c6_make_step_doc("WRONG TEXT");
    let mut f_wrong = Fixture::with_inputs(
        false,
        wrong_doc,
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    holder(&mut f_wrong);
    let key_wrong = s5_c6_publish_version(&f_wrong, None, vec![]);
    let token_wrong = s5_c6_start_and_finish_encode(&f_wrong, &key_wrong);
    let (mut wrong_file, wrong_media_ref, wrong_timing) = {
        let shared = f_wrong.host.owner.upgrade().unwrap();
        let owner = shared.lock().unwrap();
        let bundle = owner
            .owner()
            .unwrap()
            .review_source_bundle(&token_wrong.project)
            .unwrap();
        let (artifact, file) = bundle.open_review_artifact(&token_wrong).unwrap();
        (file, artifact.media_ref.clone(), artifact.output.clone())
    };
    drop(f_wrong);

    // 3. Create the frozen "Step 1" project on disk and configure the native Reviewer selection
    // strictly from the consumed lead grant.
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "s5-c6-live-reviewer",
    );
    let settings_dir = temp.path("scheduler");
    std::fs::create_dir_all(&settings_dir).unwrap();
    let selection = grant.selection();
    write_director_setting(Some(&settings_dir), &selection).expect("S5_C6_LIVE_WRITE_DIRECTOR");
    write_specialist_setting(Some(&settings_dir), &SpecialistChoice::Specific(selection))
        .expect("S5_C6_LIVE_WRITE_SPECIALIST");

    let bundle_path = temp.path("live-review.kinewright");
    {
        let core = Core::for_test(step1_doc).unwrap();
        let mut bundle = ProjectBundle::activate(
            &bundle_path,
            BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
        let inputs_ref = s5_c6_admit_embedded_font(&mut bundle);
        bundle.save(&core, &inputs_ref).unwrap();
    }

    let host = WriterHost::start_inner(
        HostConfig {
            data_dir: Some(temp.path("voice-data")),
            desktop_notifications: false,
            scheduler_settings: Some(settings_dir),
            project: "S5 C6 live reviewer witness".into(),
            path: Some(bundle_path),
            roots: SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        None,
        studio_runtime::RuntimePorts {
            guardian: guardian.clone(),
            checks: Arc::new(crate::codex_qualification::InstalledNativeChecks { guardian }),
            ceiling: std::num::NonZeroU64::new(grant.tokens_per_task),
            fixture_claim: Some(temp.path("machine")),
            fixture_autostart: true,
            ..Default::default()
        },
    )
    .expect("S5_C6_LIVE_HOST_START");
    wait_runtime_recovery(&host);

    // 4. Publish the frozen "Step 1" snapshot, queue a ReviewJob, and publish the tampered
    // "WRONG TEXT" MP4 as its review artifact (creating a genuine decoded-only discrepancy).
    let shared = host.owner.upgrade().unwrap();
    let client = host.client();
    let config = C6CheckConfiguration::default();
    let (snapshot_key, checkpoint_head, token) = {
        let mut owner_guard = shared.lock().unwrap();
        let owner = owner_guard.owner_mut().unwrap();
        let (checkpoint_ref, checkpoint) = owner.manual_checkpoint().unwrap();
        let issuer = owner.reuse_metadata_issuer_for("live-s5-c6").unwrap();
        let envelope = owner
            .metadata_envelope(CommandId {
                sequence: owner.metadata_issuer_sequence(&issuer).unwrap(),
                issuer,
            })
            .unwrap();
        let update = MetadataUpdate {
            expected_root_generation: owner.metadata_generation().unwrap(),
            change: MetadataChange::PublishSnapshot(PublishSnapshot {
                version: VersionId::fresh().unwrap(),
                expected_head: checkpoint.head_id.clone(),
                checkpoint_ref,
                parent_version: None,
                run_provenance: None,
                revision_refs: vec![],
            }),
        };
        let kinewright_project::receipts::ReceiptLookup::Committed(receipt) = owner
            .metadata_command(
                &envelope,
                kinewright_project::studio_records::MetadataScope::Snapshot,
                &update,
            )
            .unwrap()
        else {
            panic!("S5_C6_LIVE_SNAPSHOT_COMMITTED");
        };
        let ReceiptResult::Metadata {
            result: MetadataResult::Snapshot(key),
        } = receipt.result
        else {
            panic!("S5_C6_LIVE_SNAPSHOT_KEY");
        };
        let frozen = owner.read_snapshot(&key).unwrap();
        let inputs = frozen.open_review_inputs().unwrap();
        let settings = kinewright_core::DeliveryProfile::SourceMaster.export_settings(
            &inputs.document,
            kinewright_core::DeliveryEncodeDepth::Eight,
            kinewright_core::ExportCancellation::default(),
        );
        let empty = kinewright_project::review_job::empty_annotations(&frozen.descriptor);
        let expectations = super::super::review_expectations::derive_review_expectations(
            &super::super::review_expectations::ReviewExpectationInput {
                document: &inputs.document,
                snapshot: frozen.descriptor.identity,
                input_manifest: &inputs.manifest,
                settings: &settings,
                transcripts: &[],
            },
            &empty,
        )
        .unwrap();
        drop(inputs);
        let token = owner
            .queue_review(&frozen.descriptor, None, settings, 64 << 20, expectations)
            .unwrap();
        let stage = kinewright_project::review_job::ReviewStage {
            id: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
            boot: crate::guardian::boot_id().unwrap(),
            pid: std::process::id(),
            process_start: super::super::review_job::process_start(std::process::id())
                .unwrap()
                .unwrap(),
            byte_limit: 64 << 20,
        };
        owner
            .update_review_job(&token, UpdateReviewJob::BeginEncoding(stage))
            .unwrap();
        let bundle = owner.review_source_bundle_mut(&token.project).unwrap();
        bundle
            .publish_review_artifact(&token, &mut wrong_file, &wrong_media_ref, wrong_timing)
            .unwrap();
        owner_guard.review_fixture_mark_verified(token.clone());
        (key, checkpoint.head_id, token)
    };

    // 5. Launch a real ReviseCut run through the production native runtime asking the Director to
    // delegate independent visual review to one Role::Reviewer specialist over loopback MCP.
    let prompt_text = concat!(
        "Assign one Role::Reviewer specialist to visually inspect the active output review artifact. ",
        "Instruct the Reviewer to call read_review_stills repeatedly (following next_cursor until null) ",
        "to inspect EVERY DecodedOutput PNG and OverlayReference PNG, compare the rendered caption text ",
        "against the OverlayReference PNG, and call finish_task whose explanation is ONLY a single ",
        "VisualJudgment JSON object: ",
        "{\"verdict\":\"findings\"|\"pass\",\"reviewed_still_ids\":[...],",
        "\"findings\":[{\"still_id\":\"...\",\"severity\":\"blocking\"|\"advisory\",\"summary\":\"...\"}]}. ",
        "Any text mismatch between DecodedOutput and OverlayReference must be flagged as blocking."
    );
    let run_id: RunId = {
        use kinewright_project::studio_records::{
            CommentId, FeedbackAnchor, FeedbackRevision, FrameRate, Orientation, ReviseResult,
        };
        let (text_ref, empty_ref) = {
            let mut locked = shared.lock().unwrap();
            let owner = locked.owner_mut().unwrap();
            let t = owner.admit_host_grant(prompt_text.as_bytes()).unwrap();
            let e = owner.admit_host_grant(b"[]").unwrap();
            (t, e)
        };
        let issue = |client: &HostClient| {
            client
                .request(HostCommand::IssueMetadataIssuer { issuer: None })
                .unwrap()
                .metadata_issuer
                .unwrap()
        };
        let issued = issue(&client);
        let saved = client
            .request(HostCommand::Metadata {
                envelope: issued.envelope,
                update: MetadataUpdate {
                    expected_root_generation: issued.root_generation,
                    change: MetadataChange::SaveFeedback(FeedbackRevision {
                        comment_id: CommentId::fresh().unwrap(),
                        revision: 1,
                        predecessor: None,
                        anchor: FeedbackAnchor {
                            snapshot: snapshot_key,
                            frame: 0,
                            fps: FrameRate {
                                numerator: 24,
                                denominator: 1,
                            },
                            raster_size: [480, 270],
                            orientation: Orientation::Upright,
                            target: None,
                            source_ref: None,
                            script_beat_id: None,
                        },
                        original_text_ref: text_ref.clone(),
                        strokes_ref: empty_ref,
                        frame_proof_ref: text_ref,
                        voice_ref: None,
                        transcript_revision: None,
                    }),
                },
            })
            .unwrap()
            .metadata
            .unwrap();
        let ReceiptResult::Metadata {
            result: MetadataResult::Feedback(rev_ref),
        } = saved.result
        else {
            panic!("S5_C6_LIVE_SAVE_FEEDBACK");
        };
        let issued = issue(&client);
        let batch = FeedbackBatchId::fresh().unwrap();
        let request = ReviseRequest {
            command_id: issued.envelope.id.clone(),
            batch_id: batch.clone(),
            expected_candidate_head: checkpoint_head,
        };
        client
            .request(HostCommand::Metadata {
                envelope: issued.envelope,
                update: MetadataUpdate {
                    expected_root_generation: issued.root_generation,
                    change: MetadataChange::SubmitBatch {
                        batch: FeedbackBatch {
                            batch_id: batch,
                            revision_refs: vec![rev_ref],
                        },
                        request: request.clone(),
                    },
                },
            })
            .unwrap();
        let revised = client
            .request(HostCommand::ReviseCut { request })
            .unwrap()
            .revise_result
            .unwrap();
        let ReviseResult::Accepted { run_id, .. } = revised else {
            panic!("S5_C6_LIVE_REVISE_CUT: {revised:?}");
        };
        serde_json::from_value(serde_json::to_value(run_id).unwrap()).unwrap()
    };

    let run_deadline = Instant::now() + Duration::from_millis(grant.wall_millis);
    let mut bound_binding: Option<ReviewCheckBinding> = None;
    let (result_ref, binding) = loop {
        host.runtime.update();
        let mut terminal_result = None;
        {
            let mut owner = shared.lock().unwrap();
            if let Some(proj) = owner
                .owner()
                .unwrap()
                .orchestration_projection(&run_id)
                .unwrap()
            {
                if bound_binding.is_none()
                    && let Some(reviewer_task) =
                        proj.tasks.iter().find(|t| t.brief.role == Role::Reviewer)
                    && let Some(attempt) = proj
                        .attempts
                        .iter()
                        .find(|a| a.identity.task == reviewer_task.id)
                {
                    let (b, _) = owner
                        .bind_c6_review_checks(&token, attempt.identity.attempt.0.clone(), &config)
                        .expect("S5_C6_LIVE_BIND_REVIEWER");
                    bound_binding = Some(b);
                }
                if let Some(reviewer_task) =
                    proj.tasks.iter().find(|t| t.brief.role == Role::Reviewer)
                    && let TaskState::Terminal(reference) = &reviewer_task.state
                {
                    terminal_result = Some(reference.clone());
                }
            }
        }
        if let Some(reference) = terminal_result {
            break (reference, bound_binding.expect("S5_C6_LIVE_REVIEWER_BOUND"));
        }
        assert!(
            Instant::now() < run_deadline,
            "S5_C6_LIVE_REVIEWER_WALL_DEADLINE"
        );
        std::thread::sleep(Duration::from_millis(50));
    };

    // 6. Parse the live Reviewer's VisualJudgment from its published TaskResult explanation,
    // validate against the served still ledger, and record through record_review_check.
    let explanation_bytes = {
        let owner = shared.lock().unwrap();
        let task_result = owner
            .read_task_reference(&result_ref)
            .expect("S5_C6_LIVE_READ_TASK_RESULT");
        let explanation_ref: kinewright_project::bundle_records::ObjectRef =
            serde_json::from_value(task_result["explanation"].clone())
                .expect("S5_C6_LIVE_EXPLANATION_REF");
        owner
            .owner()
            .unwrap()
            .review_bundle()
            .unwrap()
            .store()
            .read_evidence(&explanation_ref)
            .expect("S5_C6_LIVE_READ_EXPLANATION")
    };
    let json_slice = s5_c6_extract_json_object(&explanation_bytes);
    let judgment = parse_visual_judgment(json_slice).expect("S5_C6_LIVE_PARSE_VISUAL_JUDGMENT");
    assert_eq!(
        judgment.verdict,
        VisualVerdict::Findings,
        "S5_C6_LIVE_REVIEWER_MUST_FLAG_DISCREPANCY"
    );
    assert!(
        judgment.findings.iter().any(|finding| {
            finding.severity == super::super::review_stills::VisualFindingSeverity::Blocking && {
                let lower = finding.summary.to_ascii_lowercase();
                lower.contains("wrong text")
                    || (lower.contains("wrong") && lower.contains("step 1"))
            }
        }),
        "S5_C6_LIVE_REVIEWER_MUST_NAME_DECODED_ONLY_DISCREPANCY: {:?}",
        judgment.findings
    );
    let (validated, _) = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding, &config, &judgment)
        .expect("S5_C6_LIVE_RECORD_VISUAL_JUDGMENT");
    assert_eq!(
        validated.row_result,
        ReviewRowResult::Failed,
        "S5_C6_LIVE_REVIEWER_RECORDS_FAILED"
    );
    assert!(
        validated.blocking_findings >= 1,
        "S5_C6_LIVE_REVIEWER_BLOCKING_FINDING"
    );
}

/// Fix round 2 (Opus Q5) witness: `read_review_stills` must use the bound stills configuration,
/// not the host-wide setting.
/// 1. When no bound `C6CheckConfiguration` is registered and host-wide `set_review_stills_config`
///    differs from the active `ReviewCheckBinding::configuration_sha256` (e.g., 64 px vs 960 px),
///    `read_review_stills` is explicitly refused.
/// 2. When a `C6CheckConfiguration` is registered for the active binding, mutating the host-wide
///    stills config (64 px, 1 page) is completely ignored: served stills use the bound 960 px /
///    16-page configuration, and their IDs bind the exact `stills_config_sha256`.
#[test]
#[cfg(target_os = "linux")]
fn s5_c6_fix2_stills_read_uses_bound_config_not_host_wide() {
    use super::super::review_stills::{C6CheckConfiguration, VisualJudgment, VisualVerdict};
    use kinewright_core::output_review::ReviewRowResult;
    use kinewright_media::ReviewStillsConfig;

    let doc = s5_c6_clean_overlay_doc();
    let mut f = Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font);
    let (_, parent, director) = c4_director(&f);
    let (attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    let token = s5_c6_start_and_finish_encode(&f, &key);
    let shared = f.host.owner.upgrade().unwrap();

    // Active binding commits to default 960 px / 16-page stills config.
    let bound_cfg = C6CheckConfiguration::default();
    let binding = ReviewCheckBinding {
        configuration_sha256: bound_cfg.configuration_sha256(),
        reviewer_attempt: attempt.attempt.0,
    };
    // Host-wide stills config is set to an incompatible tiny 64 px / 1-page config.
    let rogue_host_cfg = ReviewStillsConfig {
        max_long_edge: 64,
        max_pages_per_attempt: 1,
        ..ReviewStillsConfig::default()
    };
    {
        let mut owner = shared.lock().unwrap();
        owner
            .set_review_stills_config(rogue_host_cfg.clone())
            .expect("S5_C6_FIX2_SET_ROGUE_HOST_STILLS_CONFIG");
        owner
            .record_review_check(&token, UpdateReviewJob::BindChecks(binding.clone()))
            .expect("S5_C6_FIX2_BIND_960_CONFIG");
    }

    // 1. Without registering the bound C6CheckConfiguration, the mismatched host-wide 64 px config
    //    MUST be refused by `read_review_stills` (cannot serve 64 px stills under a 960 px binding).
    let refused_read = reviewer.wire("read_review_stills", serde_json::json!({ "page_size": 2 }));
    assert_eq!(
        refused_read.is_error,
        Some(true),
        "S5_C6_FIX2_MISMATCHED_HOST_STILLS_CONFIG_REFUSED: {refused_read:?}"
    );
    assert!(
        format!("{refused_read:?}").contains("does not match the bound check configuration digest"),
        "S5_C6_FIX2_MISMATCHED_HOST_STILLS_CONFIG_REASON: {refused_read:?}"
    );

    // 2. Registering a mismatched C6CheckConfiguration is also refused.
    let wrong_c6 = C6CheckConfiguration {
        stills: rogue_host_cfg.clone(),
        ..C6CheckConfiguration::default()
    };
    assert!(
        shared
            .lock()
            .unwrap()
            .register_bound_c6_configuration(&token, &binding, &wrong_c6)
            .is_err(),
        "S5_C6_FIX2_REGISTER_WRONG_BOUND_CONFIG_REFUSED"
    );

    // 3. Register the exact bound 960 px config, then keep/re-set the rogue 64 px / 1-page
    //    host-wide config. `read_review_stills` MUST ignore the host-wide setting and serve
    //    stills bounded by the bound config across multiple pages: the default review
    //    encodes 1920x1080 (JA-1 S6 R1), so 960x540.
    {
        let mut owner = shared.lock().unwrap();
        owner
            .register_bound_c6_configuration(&token, &binding, &bound_cfg)
            .expect("S5_C6_FIX2_REGISTER_BOUND_CONFIG");
        owner
            .set_review_stills_config(rogue_host_cfg)
            .expect("S5_C6_FIX2_MUTATE_HOST_CONFIG_AFTER_BIND");
    }
    let (stills, _, pages) = s5_c6_read_all_stills(&reviewer, 2);
    assert!(
        pages >= 2,
        "S5_C6_FIX2_HOST_1PAGE_LIMIT_IGNORED: pages={pages}"
    );
    assert!(
        stills.iter().all(|s| (s.width, s.height)
            == kinewright_media::bounded_still_dimensions(
                kinewright_core::OUTPUT_REVIEW_DEFAULT_DELIVERY.resolution((480, 270)),
                bound_cfg.stills.max_long_edge,
            )),
        "S5_C6_FIX2_HOST_64PX_LIMIT_IGNORED: {:?}",
        stills
            .iter()
            .map(|s| (s.width, s.height))
            .collect::<Vec<_>>()
    );
    let expected_stills_hex = {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(64);
        for b in bound_cfg.stills.sha256() {
            let _ = write!(&mut s, "{b:02x}");
        }
        s
    };
    assert!(
        stills
            .iter()
            .all(|s| s.stills_config_sha256 == expected_stills_hex),
        "S5_C6_FIX2_STILLS_CONFIG_DIGEST_BOUND"
    );

    // 4. Judgment passes cleanly against the bound 960 px config.
    let judgment = VisualJudgment {
        verdict: VisualVerdict::Pass,
        reviewed_still_ids: stills.iter().map(|s| s.still_id.clone()).collect(),
        findings: vec![],
    };
    let (validated, _) = shared
        .lock()
        .unwrap()
        .record_visual_review_judgment(&token, binding, &bound_cfg, &judgment)
        .expect("S5_C6_FIX2_BOUND_JUDGMENT_PASSES");
    assert_eq!(validated.row_result, ReviewRowResult::Pass);
}
