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
        // Content[0] is the text summary; Content[1..] are 1-to-1 PNG image blocks.
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

#[test]
#[cfg(target_os = "linux")]
fn s5_c6_overlay_verification_records_pass_and_ignores_working_edit() {
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
    let overlay_config = OverlayVerificationConfig::default();
    let stills_config = kinewright_media::ReviewStillsConfig::default();
    let config_sha = super::super::review_overlays::overlay_and_stills_check_configuration(
        &overlay_config,
        &stills_config,
    );
    assert_eq!(
        overlay_config.sha256(),
        OverlayVerificationConfig::default().sha256(),
        "S5_C6_OVERLAY_CONFIG_SHA256_STABLE"
    );
    let mut stricter_overlay = overlay_config.clone();
    stricter_overlay.luma_tolerance -= 1.0;
    assert_ne!(
        overlay_config.sha256(),
        stricter_overlay.sha256(),
        "S5_C6_OVERLAY_CONFIG_SHA256_SENSITIVE"
    );
    let mut smaller_stills = stills_config.clone();
    smaller_stills.max_long_edge -= 1;
    assert_ne!(
        stills_config.sha256(),
        smaller_stills.sha256(),
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
        &OverlayVerificationConfig::default(),
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
    for overlay in &report.overlays {
        assert_eq!(
            overlay.status,
            OverlayElementStatus::Passed,
            "S5_C6_OVERLAY_PASSED {:?}",
            overlay.element
        );
        assert!(
            overlay.animated_extreme_frames > 0,
            "S5_C6_EXTREMES_CHECKED"
        );
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
    use kinewright_core::{
        CaptionPreset, Rational, SolidColor, TimeCode, Track, TrackId, TrackKind,
        output_review::{RequiredReviewRow, ReviewInventory, ReviewRowResult},
    };
    use kinewright_media::{OverlayFindingKind, OverlayVerificationConfig, verify_review_overlays};

    // Part A: Occluded caption (upper solid layer covers caption) fails through full ReviewJob host path.
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
        configuration_sha256: [0xB6; 32],
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
        &OverlayVerificationConfig::default(),
        &kinewright_core::ExportCancellation::default(),
    )
    .expect("S5_C6_OCCLUDED_RUN");
    assert_eq!(
        report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_OCCLUDED_CAPTION_FAILS"
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f.detail, OverlayFindingKind::GlyphMismatch { .. })),
        "S5_C6_OCCLUDED_FINDING_TYPED"
    );
    let ReviewPhase::Checking(checks) = status.phase else {
        panic!("expected Checking");
    };
    assert_eq!(
        checks
            .rows
            .get(&RequiredReviewRow::OverlayVerification)
            .map(|r| r.result),
        Some(ReviewRowResult::Failed),
        "S5_C6_OCCLUDED_RECORDED_FAILED"
    );

    // Part B: Unresolved overlay expectation records Unknown (never Pass) through record_review_check.
    let clean_doc = s5_c6_clean_overlay_doc();
    let mut f2 = Fixture::with_inputs(
        false,
        clean_doc.clone(),
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    holder(&mut f2);
    let key2 = s5_c6_publish_version(&f2, None, vec![]);
    let token2 = s5_c6_start_and_finish_encode(&f2, &key2);
    let shared2 = f2.host.owner.upgrade().unwrap();
    let binding2 = ReviewCheckBinding {
        configuration_sha256: [0xC6; 32],
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    let (artifact_file, artifact_timing, mut expectations) = {
        let mut owner = shared2.lock().unwrap();
        owner
            .record_review_check(&token2, UpdateReviewJob::BindChecks(binding2.clone()))
            .expect("S5_C6_UNKNOWN_BIND");
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
        let (artifact, file) = bundle.open_review_artifact(&token2).unwrap();
        (file, artifact.output.clone(), exp.0.clone())
    };
    expectations.overlays.captions = ReviewInventory::NotDetermined { known: Vec::new() };
    let mut no_cards =
        |_: [u8; 32], _: u64| Err(kinewright_core::MediaError::Backend("none".into()));
    let unknown_report = verify_review_overlays(
        artifact_file,
        &artifact_timing,
        &expectations,
        &clean_doc,
        &mut no_cards,
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
        .record_overlay_verification(&token2, binding2, &unknown_report)
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
    let make_step_doc = |text: &str| {
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
    };
    let step1_doc = make_step_doc("Step 1");
    let step2_doc = make_step_doc("Step 2");
    let mut f_expected = Fixture::with_inputs(
        false,
        step1_doc.clone(),
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    holder(&mut f_expected);
    let key_expected = s5_c6_publish_version(&f_expected, None, vec![]);
    let token_expected = s5_c6_start_and_finish_encode(&f_expected, &key_expected);
    let shared_expected = f_expected.host.owner.upgrade().unwrap();
    let binding_sub = ReviewCheckBinding {
        configuration_sha256: [0xC7; 32],
        reviewer_attempt: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
    };
    let frozen_step1_exp = {
        let mut owner = shared_expected.lock().unwrap();
        owner
            .record_review_check(
                &token_expected,
                UpdateReviewJob::BindChecks(binding_sub.clone()),
            )
            .expect("S5_C6_SUB_BIND");
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
        bundle
            .store()
            .read_record::<kinewright_project::review_job::StoredReviewExpectations>(
                &req.expectations_ref,
            )
            .unwrap()
            .0
            .clone()
    };
    // Encode the substituted "Step 2" artifact independently and verify it against the frozen "Step 1" expectations.
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
    let sub_report = verify_review_overlays(
        tampered_file,
        &tampered_timing,
        &frozen_step1_exp,
        &step1_doc,
        &mut no_cards,
        &OverlayVerificationConfig::default(),
        &kinewright_core::ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(
        sub_report.row_result,
        ReviewRowResult::Failed,
        "S5_C6_SUBSTITUTED_TEXT_FAILS_HOST"
    );
    let sub_status = shared_expected
        .lock()
        .unwrap()
        .record_overlay_verification(&token_expected, binding_sub, &sub_report)
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
    use super::super::review_stills::{
        VisualFinding, VisualFindingSeverity, VisualJudgment, VisualVerdict, parse_visual_judgment,
    };
    use kinewright_core::{
        CaptionPreset, Rational, SolidColor, TimeCode, Title, TitlePosition, Track, TrackId,
        TrackKind,
        output_review::{RequiredReviewRow, ReviewRowResult},
    };
    use kinewright_media::{ReviewStillKind, ReviewStillReason};

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
    // Replace the second half background with an imported BT.709 video clip so
    // expectations.segments emits a real Video cut boundary at frame 9.
    let cut_path = f.temp.path("cut_clip.mp4");
    kinewright_media::test_support::run_ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x5a2c44:s=320x180:r=24:d=0.5",
            "-frames:v",
            "12",
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
        &cut_path,
    );
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

    // Publish v1 with a blocking editorial audit covering 500_000..625_000 micros (frames 12..15),
    // then publish v2 with parent_version = Some(v1) so v2 plans a RepairedRegion sample.
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
        configuration_sha256: [0xD6; 32],
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
    // Also exercise the host handler guard directly, independent of the manifest.
    {
        let state = f.projection(&run);
        let editor_record = state
            .tasks
            .iter()
            .find(|task| task.id == editor_attempt.task)
            .unwrap();
        let owner = shared.lock().unwrap();
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

    // Retire the Editor task now that its refusals are proven, freeing one dependency slot
    // (ATTEMPT_LIMIT = 3) so Reviewer A and Reviewer B can coexist for cross-attempt checks.
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
        .record_visual_review_judgment(&token, binding_a.clone(), &missing_one)
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
        .record_visual_review_judgment(&token, binding_a.clone(), &contradictory)
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
        .record_visual_review_judgment(&token, binding_a, &parsed_pass)
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

    // 6. Unbind Attempt A (matching host reopen/retry) and bind fresh Attempt B on the exact same artifact.
    shared
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .update_review_job(&token, UpdateReviewJob::UnbindChecks)
        .expect("S5_C6_UNBIND_ATTEMPT_A");
    let (attempt_b, _, reviewer_b) = s5_child(&f, &director, &parent, Role::Reviewer);
    let binding_b = ReviewCheckBinding {
        configuration_sha256: [0xD6; 32],
        reviewer_attempt: attempt_b.attempt.0.clone(),
    };
    shared
        .lock()
        .unwrap()
        .record_review_check(&token, UpdateReviewJob::BindChecks(binding_b.clone()))
        .expect("S5_C6_BIND_ATTEMPT_B");

    // Attempt A is now unbound and cannot read stills.
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
    // Attempt B cannot reuse Attempt A's cursor.
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

    // Attempt B reads its own stills cleanly.
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
        .record_visual_review_judgment(&token, binding_b.clone(), &stale_attempt_judgment)
        .expect_err("S5_C6_OTHER_ATTEMPT_STILL_ID_REFUSED");
    assert!(
        stale_err
            .to_string()
            .contains("belongs to reviewer attempt"),
        "S5_C6_OTHER_ATTEMPT_MSG: {stale_err}"
    );

    // Valid Blocking finding from Attempt B records Failed through record_review_check.
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
        .record_visual_review_judgment(&token, binding_b, &blocking_judgment)
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

    // 7. Stop fence refuses subsequent read_review_stills calls over MCP.
    s5_review_call(&f.editor, ReviewControl::Stop(token)).unwrap();
    let after_stop = reviewer_b.wire("read_review_stills", serde_json::json!({}));
    assert_eq!(
        after_stop.is_error,
        Some(true),
        "S5_C6_STOP_FENCES_STILLS {after_stop:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "on demand: live reviewer vision model witness; requires KINEWRIGHT_LIVE_REVIEWER_GRANT=1 and explicit operator authorization"]
fn s5_c6_live_reviewer_identifies_decoded_only_discrepancy() {
    use super::super::review_stills::parse_visual_judgment;
    use kinewright_core::output_review::ReviewRowResult;

    assert_eq!(
        std::env::var("KINEWRIGHT_LIVE_REVIEWER_GRANT").as_deref(),
        Ok("1"),
        "Live reviewer witness requires explicit operator grant (KINEWRIGHT_LIVE_REVIEWER_GRANT=1)"
    );
    let judgment_path = std::env::var("KINEWRIGHT_LIVE_REVIEWER_JUDGMENT_FILE")
        .expect("set KINEWRIGHT_LIVE_REVIEWER_JUDGMENT_FILE to the live reviewer JSON output");
    let raw = std::fs::read(&judgment_path).expect("read live reviewer judgment file");
    let judgment = parse_visual_judgment(&raw).expect("parse live reviewer judgment JSON");
    assert_eq!(
        judgment.verdict,
        super::super::review_stills::VisualVerdict::Findings,
        "live reviewer must flag the decoded-only discrepancy"
    );
    assert!(
        judgment.findings.iter().any(|finding| {
            finding.severity == super::super::review_stills::VisualFindingSeverity::Blocking
        }),
        "live reviewer finding must be blocking (ReviewRowResult::{:?})",
        ReviewRowResult::Failed
    );
}
