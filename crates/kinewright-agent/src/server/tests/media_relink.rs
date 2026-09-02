//! M41 media status, cache, and relink tests.

use super::*;

#[test]
fn m41_media_status_reports_dynamic_availability_jobs_and_preview_limits() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let result = service
        .call_blocking(CallToolRequestParams::new("get_media_status"))
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let value = result.structured_content.unwrap();
    assert_eq!(value["timeline_revision"], 0);
    assert_eq!(value["preview"]["mode"], "in_memory");
    assert_eq!(value["preview"]["max_width"], 1_280);
    assert_eq!(value["preview"]["persistent"], false);
    assert_eq!(value["preview"]["generated_proxy_supported"], false);
    assert_eq!(value["assets"].as_array().unwrap().len(), 1);
    assert_eq!(value["assets"][0]["path"], "fixture.mp4");
    assert_eq!(
        value["assets"][0]["availability"]["kind"],
        "online_unverified"
    );
    assert_eq!(
        value["assets"][0]["analysis_jobs"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn m41_timeline_proofs_ignore_offline_media_pool_assets_until_referenced() {
    let (seed_core, _, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed)) =
        seed_core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed).clone();
    let mut unused = document.media_pool[0].clone();
    unused.id = AssetId(2);
    unused.path = PathBuf::from("unused-offline.mp4");
    document.media_pool.push(unused);
    document.validate().unwrap();

    let offline = MediaAvailabilityStatus {
        kind: MediaAvailabilityKind::OfflineMissing,
        observed_fingerprint: None,
        reason: Some("test source is offline".to_owned()),
    };
    let media = Arc::new(NoopMedia {
        availability_by_asset: BTreeMap::from([(AssetId(2), offline)]),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(
        Core::spawn(document.clone()).unwrap(),
        media.clone(),
        media,
        ConfirmationBroker::default(),
    );
    assert!(
        service
            .document_availability_error(&document, "frame proof")
            .is_none(),
        "an unused offline bin item must not block a timeline proof"
    );

    document.tracks[0].clips[0].asset = AssetId(2);
    assert!(
        service
            .document_availability_error(&document, "frame proof")
            .is_some(),
        "a referenced offline source must block the proof explicitly"
    );
}

#[test]
fn m41_cache_status_and_scoped_clear_are_typed_and_proxy_failure_is_explicit() {
    let (core, playback, _) = fixture();
    let media = Arc::new(NoopMedia {
        cache_inventory: Some(MediaCacheInventory {
            families: vec![kinewright_core::MediaCacheFamilyStatus {
                family: MediaCacheFamily::VisualAssets,
                supported: true,
                root: Some(PathBuf::from("visual-assets/v1")),
                file_count: 3,
                bytes: 120,
                may_repopulate: true,
                note: Some("test inventory".to_owned()),
            }],
        }),
        clear_cache_result: Some(MediaCacheClearResult {
            family: MediaCacheFamily::VisualAssets,
            supported: true,
            removed_file_count: 3,
            removed_bytes: 120,
            may_repopulate: true,
            note: Some("test clear".to_owned()),
        }),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(core, playback, media, ConfirmationBroker::default());

    let status = service
        .call_blocking(CallToolRequestParams::new("get_cache_status"))
        .unwrap();
    assert_eq!(status.is_error, Some(false));
    assert_eq!(
        status.structured_content.unwrap()["families"][0]["file_count"],
        3
    );

    let clear = service
        .call_blocking(
            CallToolRequestParams::new("clear_media_cache").with_arguments(
                json!({"family": "visual_assets"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .unwrap();
    assert_eq!(clear.is_error, Some(false));
    assert_eq!(clear.structured_content.unwrap()["removed_bytes"], 120);

    let unsupported = service
        .call_blocking(
            CallToolRequestParams::new("clear_media_cache").with_arguments(
                json!({"family": "generated_proxy"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .unwrap();
    assert_eq!(unsupported.is_error, Some(true));
    let value = unsupported.structured_content.unwrap();
    assert_eq!(value["family"], "generated_proxy");
    assert_eq!(value["code"], "unsupported_generated_proxy");
    assert_eq!(value["supported"], false);
}

#[test]
fn m41_relink_probes_applies_one_undoable_operation_and_rejects_known_mismatch() {
    let known = fingerprint(8, 'a');
    let (service, core, media) = relink_service(known.clone(), known.clone());
    let applied = service
        .call_blocking(relink_request(0, 1, "moved/replacement.mp4", false))
        .unwrap();
    assert_eq!(applied.is_error, Some(false));
    assert_eq!(
        media.probe_paths.lock().unwrap().as_slice(),
        [PathBuf::from("moved/replacement.mp4")]
    );
    let (revision, document) = service.snapshot().unwrap();
    assert_eq!(revision, TimelineRevision(1));
    assert_eq!(
        document.asset(AssetId(1)).unwrap().path,
        PathBuf::from("moved/replacement.mp4")
    );

    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("relink should be undoable");
    };
    assert_eq!(
        doc.asset(AssetId(1)).unwrap().path,
        PathBuf::from("fixture.mp4")
    );

    let (service, _, _) = relink_service(known, fingerprint(8, 'b'));
    let mismatch = service
        .call_blocking(relink_request(0, 1, "wrong-content.mp4", false))
        .unwrap();
    assert_eq!(mismatch.is_error, Some(true));
    assert!(
        mismatch.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("fingerprint")
    );
    assert_eq!(service.snapshot().unwrap().0, TimelineRevision(0));
    assert_eq!(
        service
            .snapshot()
            .unwrap()
            .1
            .asset(AssetId(1))
            .unwrap()
            .path,
        PathBuf::from("fixture.mp4")
    );

    let mut metadata_mismatch = relink_probe_asset(fingerprint(8, 'a'));
    metadata_mismatch.duration = TimeCode(59);
    let (service, _, _) = relink_service_with_probe(fingerprint(8, 'a'), metadata_mismatch);
    let mismatch = service
        .call_blocking(relink_request(0, 1, "wrong-duration.mp4", false))
        .unwrap();
    assert_eq!(mismatch.is_error, Some(true));
    assert!(
        mismatch.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("duration")
    );
    assert_eq!(service.snapshot().unwrap().0, TimelineRevision(0));
}

#[test]
fn m41_relink_requires_legacy_opt_in_and_stale_revision_preflights_before_probe() {
    let candidate_fingerprint = fingerprint(8, 'a');
    let (service, _, media) = relink_service(
        MediaSourceFingerprint::unknown(),
        candidate_fingerprint.clone(),
    );
    let refused = service
        .call_blocking(relink_request(0, 1, "legacy-replacement.mp4", false))
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
    assert!(
        refused.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("allow_unverified_source")
    );
    assert_eq!(service.snapshot().unwrap().0, TimelineRevision(0));

    let accepted = service
        .call_blocking(relink_request(0, 1, "legacy-replacement.mp4", true))
        .unwrap();
    assert_eq!(accepted.is_error, Some(false));
    assert!(
        service
            .snapshot()
            .unwrap()
            .1
            .asset(AssetId(1))
            .unwrap()
            .source_fingerprint
            .is_verified()
    );

    let before_probe_count = media.probe_paths.lock().unwrap().len();
    let stale = service
        .call_blocking(relink_request(0, 1, "stale.mp4", true))
        .unwrap();
    assert_eq!(stale.is_error, Some(true));
    assert!(
        stale.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("revision conflict")
    );
    assert_eq!(media.probe_paths.lock().unwrap().len(), before_probe_count);
}

#[test]
fn m41_relink_is_not_available_through_generated_operation_or_edit_plan() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    assert!(registry.iter().any(|tool| tool.name == "relink_media"));
    assert!(registry.iter().all(|tool| tool.name != "relink_asset"));
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let result = service
        .call_blocking(
            CallToolRequestParams::new("apply_edit_plan").with_arguments(
                json!({
                    "expected_revision": 0,
                    "operations": [{
                        "op": "relink_asset",
                        "asset": 1,
                        "candidate": {
                            "path": "bypass.mp4",
                            "fingerprint": {},
                            "kind": "Video",
                            "fps": {"numerator": 30, "denominator": 1},
                            "duration": 60,
                            "resolution": [320, 180]
                        },
                        "allow_unverified_source": true
                    }]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("relink_media")
    );
}
