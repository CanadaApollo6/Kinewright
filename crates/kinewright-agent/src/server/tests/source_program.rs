//! Source/program edit planner tests.

use super::*;
use crate::server::delivery::paths_resolve_equal;

#[test]
fn source_path_guard_resolves_a_nonexistent_destination_through_dot_dot() {
    let directory =
        std::env::temp_dir().join(format!("kinewright-source-guard-{}", std::process::id()));
    let nested = directory.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let source = directory.join("source.mp4");
    std::fs::write(&source, b"source").unwrap();
    let aliased = nested.join("..").join("source.mp4");

    assert!(paths_resolve_equal(&aliased, &source));

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn m32_tools_expose_professional_edits_source_monitor_and_faceted_search() {
    let names = KinewrightMcp::tools()
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect::<Vec<_>>();
    for name in [
        "three_point_edit",
        "patched_three_point_edit",
        "slip_clip",
        "roll_edit",
        "slide_clip",
        "replace_clip",
        "fit_to_fill",
        "get_source_info",
        "plan_source_program_edit",
        "search_media",
    ] {
        assert!(names.iter().any(|candidate| candidate == name));
    }

    let (core, playback, _) = fixture();
    let transcript = Arc::new(AssetTranscript {
        asset: AssetId(1),
        content_sha256: "fixture".to_owned(),
        source_fps: Rational::new(30, 1).unwrap(),
        words: vec![TranscriptWord {
            text: "wedding vows".to_owned(),
            source_start: TimeCode(12),
            source_end: TimeCode(24),
            speaker: Some("Partner".to_owned()),
        }],
    });
    let analysis: Arc<dyn Analysis> = Arc::new(NoopMedia {
        transcript: Some(transcript),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());

    let source = service
        .source_info(&SourceInfoArgs {
            asset_id: AssetId(1),
            source_in: Some(TimeCode(10)),
            source_out: Some(TimeCode(30)),
        })
        .unwrap();
    assert_eq!(source.is_error, Some(false));
    let source = source.structured_content.unwrap();
    assert_eq!(source["timeline_revision"], 0);
    assert_eq!(source["destinations"]["video"][0]["track_id"], 1);
    assert!(
        source["destinations"]["audio"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(source["source_monitor"]["duration"], 20);
    assert_eq!(source["asset"]["color_description"]["primaries"], "unknown");
    assert_eq!(
        source["asset"]["color_description"]["confidence_basis_points"],
        0
    );
    assert_eq!(
        source["asset"]["color_description"]["provenance"],
        "unknown"
    );
    assert_eq!(source["words"][0]["speaker"], "Partner");

    let search = service
        .search_media(&MediaSearchArgs {
            query: Some("vows".to_owned()),
            speaker: Some("partner".to_owned()),
            kind: Some(MediaKind::Video),
            min_width: Some(320),
            min_height: Some(180),
            min_duration_frames: Some(TimeCode(60)),
            min_scene_count: None,
            min_beat_count: None,
            has_transcript: Some(true),
            limit: Some(10),
        })
        .unwrap();
    assert_eq!(search.is_error, Some(false));
    let search = search.structured_content.unwrap();
    assert_eq!(search["total_matches"], 1);
    assert_eq!(search["hits"][0]["word_matches"][0]["source_start"], 12);
}

#[test]
fn source_program_planner_honors_an_explicit_second_video_track_and_commits_revision_safely() {
    let service = source_program_service_with_second_video_track();
    let result = service
        .source_program_edit_plan(&SourceProgramEditArgs {
            expected_revision: TimelineRevision(0),
            asset: AssetId(1),
            source_in: Some(TimeCode(20)),
            source_out: Some(TimeCode(40)),
            timeline_in: Some(TimeCode(10)),
            timeline_out: None,
            mode: ThreePointMode::Overwrite,
            video_track: Some(TrackId(9)),
            audio_track: None,
        })
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["timeline_revision"], 0);
    assert_eq!(structured["destinations"]["video"]["track_id"], 9);
    assert_eq!(structured["source_range"], json!({"start": 20, "end": 40}));
    assert_eq!(
        structured["timeline_range"],
        json!({"start": 10, "end": 30})
    );
    assert_eq!(structured["linked"], false);
    let plan_id = structured["prepared_edit_plan"]["plan_id"]
        .as_u64()
        .expect("prepared plan id");
    assert_eq!(
        service
            .snapshot()
            .unwrap()
            .1
            .tracks
            .iter()
            .find(|track| track.id == TrackId(9))
            .unwrap()
            .clips[0]
            .id,
        ClipId(99)
    );

    let committed = service
        .call_blocking(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({
                    "plan_id": plan_id,
                    "expected_revision": 0,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(committed.is_error, Some(false));
    let (revision, document) = service.snapshot().unwrap();
    assert_eq!(revision, TimelineRevision(1));
    let target = document
        .tracks
        .iter()
        .find(|track| track.id == TrackId(9))
        .unwrap();
    assert_eq!(target.clips.len(), 2);
    let replacement = target
        .clips
        .iter()
        .find(|clip| clip.timeline_start == TimeCode(10))
        .expect("overwrite replacement");
    assert_eq!(replacement.source_range, TimeCode(20)..TimeCode(40));
    assert_eq!(replacement.id, ClipId(99));
    assert!(target.clips.iter().any(|clip| clip.id == ClipId(98)));
}

#[test]
#[allow(clippy::too_many_lines)]
fn source_program_planner_prepares_dual_linked_ranges_and_rejects_bad_routes_before_storage() {
    let service = source_program_av_service();
    let empty = service
        .source_program_edit_plan(&SourceProgramEditArgs {
            expected_revision: TimelineRevision(0),
            asset: AssetId(1),
            source_in: Some(TimeCode(0)),
            source_out: Some(TimeCode(10)),
            timeline_in: Some(TimeCode(20)),
            timeline_out: None,
            mode: ThreePointMode::Insert,
            video_track: None,
            audio_track: None,
        })
        .unwrap();
    assert_eq!(empty.is_error, Some(true));
    assert_eq!(
        empty.structured_content.unwrap()["code"],
        "empty_source_patch"
    );

    let wrong_kind = service
        .source_program_edit_plan(&SourceProgramEditArgs {
            expected_revision: TimelineRevision(0),
            asset: AssetId(1),
            source_in: Some(TimeCode(0)),
            source_out: Some(TimeCode(10)),
            timeline_in: Some(TimeCode(20)),
            timeline_out: None,
            mode: ThreePointMode::Insert,
            video_track: Some(TrackId(2)),
            audio_track: None,
        })
        .unwrap();
    assert_eq!(wrong_kind.is_error, Some(true));
    assert_eq!(
        wrong_kind.structured_content.unwrap()["code"],
        "invalid_source_patch_route_kind"
    );

    let stale = service
        .source_program_edit_plan(&SourceProgramEditArgs {
            expected_revision: TimelineRevision(1),
            asset: AssetId(1),
            source_in: Some(TimeCode(0)),
            source_out: Some(TimeCode(10)),
            timeline_in: Some(TimeCode(20)),
            timeline_out: None,
            mode: ThreePointMode::Insert,
            video_track: Some(TrackId(1)),
            audio_track: Some(TrackId(2)),
        })
        .unwrap();
    assert_eq!(stale.is_error, Some(true));
    assert!(
        stale.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("revision conflict")
    );

    let planned = service
        .source_program_edit_plan(&SourceProgramEditArgs {
            expected_revision: TimelineRevision(0),
            asset: AssetId(1),
            source_in: Some(TimeCode(0)),
            source_out: Some(TimeCode(10)),
            timeline_in: Some(TimeCode(20)),
            timeline_out: None,
            mode: ThreePointMode::Insert,
            video_track: Some(TrackId(1)),
            audio_track: Some(TrackId(2)),
        })
        .unwrap();
    assert_eq!(planned.is_error, Some(false), "{planned:?}");
    let structured = planned.structured_content.unwrap();
    assert_eq!(structured["timeline_revision"], 0);
    assert_eq!(structured["source_range"], json!({"start": 0, "end": 10}));
    assert_eq!(
        structured["timeline_range"],
        json!({"start": 20, "end": 30})
    );
    assert_eq!(structured["destinations"]["video"]["track_id"], 1);
    assert_eq!(structured["destinations"]["audio"]["track_id"], 2);
    assert_eq!(structured["linked"], true);
    assert_eq!(
        structured["destinations"]["video"]["link_id"],
        structured["destinations"]["audio"]["link_id"]
    );
    assert_eq!(structured["prepared_edit_plan"]["expected_revision"], 0);
    let plan_id = structured["prepared_edit_plan"]["plan_id"]
        .as_u64()
        .expect("prepared plan id");

    let committed = service
        .call_blocking(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({
                    "plan_id": plan_id,
                    "expected_revision": 0,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(committed.is_error, Some(false));
    let (revision, document) = service.snapshot().unwrap();
    assert_eq!(revision, TimelineRevision(1));
    let routed = document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .filter(|clip| clip.asset == AssetId(1) && clip.source_range == (TimeCode(0)..TimeCode(10)))
        .collect::<Vec<_>>();
    assert_eq!(routed.len(), 2);
    assert_eq!(routed[0].timeline_start, TimeCode(20));
    assert_eq!(routed[1].timeline_start, TimeCode(20));
    assert_eq!(routed[0].link, routed[1].link);

    let source = service
        .source_info(&SourceInfoArgs {
            asset_id: AssetId(1),
            source_in: None,
            source_out: None,
        })
        .unwrap();
    assert_eq!(source.structured_content.unwrap()["timeline_revision"], 1);
}

fn raw_patched_operation() -> serde_json::Value {
    json!({
        "op": "patched_three_point_edit",
        "asset": 1,
        "source_in": 0,
        "source_out": 10,
        "timeline_in": 20,
        "timeline_out": null,
        "mode": "insert",
        "video_track": 1,
        "audio_track": null,
    })
}

fn mutable_source_service() -> (
    KinewrightMcp,
    Arc<Mutex<BTreeMap<AssetId, MediaAvailabilityStatus>>>,
) {
    let (core, playback, _) = fixture();
    let statuses = Arc::new(Mutex::new(BTreeMap::from([(
        AssetId(1),
        MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OnlineVerified,
            observed_fingerprint: None,
            reason: Some("verified source fixture".to_owned()),
        },
    )])));
    let analysis = Arc::new(NoopMedia {
        availability_override: Some(Arc::clone(&statuses)),
        ..NoopMedia::default()
    });
    (
        KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default()),
        statuses,
    )
}

#[test]
fn raw_prepare_rejects_patched_source_without_verified_media() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let (before_revision, before_document) = service.snapshot().unwrap();
    let result = service
        .call_blocking(
            CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                json!({
                    "expected_revision": before_revision,
                    "operations": [raw_patched_operation()],
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
            .contains("online_verified")
    );
    let (after_revision, after_document) = service.snapshot().unwrap();
    assert_eq!(after_revision, before_revision);
    assert_eq!(after_document, before_document);
}

#[test]
fn prepared_patched_source_rechecks_media_at_commit_without_mutation() {
    let (service, statuses) = mutable_source_service();
    let (before_revision, before_document) = service.snapshot().unwrap();
    let prepared = service
        .call_blocking(
            CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                json!({
                    "expected_revision": before_revision,
                    "operations": [raw_patched_operation()],
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(prepared.is_error, Some(false));
    let plan_id = prepared.structured_content.unwrap()["plan_id"]
        .as_u64()
        .expect("prepared patch plan id");

    statuses.lock().unwrap().insert(
        AssetId(1),
        MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::Changed,
            observed_fingerprint: None,
            reason: Some("source changed after planning".to_owned()),
        },
    );
    let committed = service
        .call_blocking(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({
                    "plan_id": plan_id,
                    "expected_revision": before_revision,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(committed.is_error, Some(true));
    assert!(
        committed.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("online_verified")
    );
    let (after_revision, after_document) = service.snapshot().unwrap();
    assert_eq!(after_revision, before_revision);
    assert_eq!(after_document, before_document);
    assert!(
        service
            .prepared_plans
            .lock()
            .unwrap()
            .get(PreparedPlanId(plan_id))
            .is_some(),
        "failed commit should leave the opaque plan available for reinspection"
    );
}

#[test]
fn verified_patched_source_prepares_and_commits_atomically() {
    let (service, _statuses) = mutable_source_service();
    let prepared = service
        .call_blocking(
            CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                json!({
                    "expected_revision": 0,
                    "operations": [raw_patched_operation()],
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(prepared.is_error, Some(false));
    let plan_id = prepared.structured_content.unwrap()["plan_id"]
        .as_u64()
        .expect("prepared patch plan id");
    let committed = service
        .call_blocking(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({
                    "plan_id": plan_id,
                    "expected_revision": 0,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(committed.is_error, Some(false));
    let (revision, document) = service.snapshot().unwrap();
    assert_eq!(revision, TimelineRevision(1));
    assert!(document.tracks[0].clips.iter().any(|clip| {
        clip.asset == AssetId(1)
            && clip.source_range == (TimeCode(0)..TimeCode(10))
            && clip.timeline_start == TimeCode(20)
    }));
}
