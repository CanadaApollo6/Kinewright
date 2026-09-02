//! Planner tests: beat montage, music structure, and music fit.

use super::*;

#[test]
fn music_fit_schema_exposes_bounded_end_anchor_pair() {
    let tool = KinewrightMcp::capability_tools()
        .unwrap()
        .into_iter()
        .find(|tool| tool.name == "plan_music_fit")
        .expect("music fit capability is registered");
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("music fit schema properties");

    for property in ["preferred_source_end", "maximum_end_drift_frames"] {
        assert!(properties.contains_key(property), "missing {property}");
    }
    assert!(
        tool.description
            .as_deref()
            .unwrap_or_default()
            .contains("fails closed")
    );
}

#[test]
fn music_fit_end_anchor_returns_resolved_endpoint_evidence() {
    let (core, analysis) = end_anchored_music_fit_fixture();
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );

    let result = service
        .plan_music_fit(&MusicFitPlanArgs {
            track_id: TrackId(2),
            asset_id: AssetId(9),
            timeline_range: TranscriptRangeArgs {
                start: TimeCode::ZERO,
                end: TimeCode(700),
            },
            preferred_source_start: Some(TimeCode(5_161)),
            preferred_source_end: Some(TimeCode(6_000)),
            maximum_end_drift_frames: Some(TimeCode::ZERO),
            min_strength: Some(0.0),
            mode: ThreePointMode::Overwrite,
        })
        .unwrap();

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["plan"]["strategy"], "end_anchored_straight_cut");
    assert_eq!(structured["plan"]["source_range"]["start"], 5_160);
    assert_eq!(structured["plan"]["source_range"]["end"], 6_000);
    assert_eq!(
        structured["plan"]["end_anchor"],
        json!({
            "target_source_end": 6_000,
            "resolved_source_end": 6_000,
            "signed_offset_frames": 0,
            "maximum_drift_frames": 0,
        })
    );
}

#[test]
fn music_fit_requires_complete_and_nonnegative_end_anchor_arguments() {
    let (core, analysis) = end_anchored_music_fit_fixture();
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let base = || MusicFitPlanArgs {
        track_id: TrackId(2),
        asset_id: AssetId(9),
        timeline_range: TranscriptRangeArgs {
            start: TimeCode::ZERO,
            end: TimeCode(700),
        },
        preferred_source_start: Some(TimeCode(5_160)),
        preferred_source_end: Some(TimeCode(6_000)),
        maximum_end_drift_frames: Some(TimeCode::ZERO),
        min_strength: Some(0.0),
        mode: ThreePointMode::Overwrite,
    };

    let missing_drift = service
        .plan_music_fit(&MusicFitPlanArgs {
            maximum_end_drift_frames: None,
            ..base()
        })
        .unwrap();
    assert_eq!(missing_drift.is_error, Some(true));
    assert!(
        missing_drift.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("requires maximum_end_drift_frames")
    );

    let negative_drift = service
        .plan_music_fit(&MusicFitPlanArgs {
            maximum_end_drift_frames: Some(TimeCode(-1)),
            ..base()
        })
        .unwrap();
    assert_eq!(negative_drift.is_error, Some(true));
    assert!(
        negative_drift.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("maximum drift cannot be negative")
    );
}

#[test]
fn beat_montage_returns_an_inspectable_ready_plan_without_mutating() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core.clone(),
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );

    let result = service.plan_beat_montage(&montage_plan_args()).unwrap();
    assert_eq!(result.is_error, Some(false));
    let result = result.structured_content.unwrap();
    assert_eq!(result["timeline_revision"], 0);
    assert_eq!(result["plan"]["shots"].as_array().unwrap().len(), 2);
    assert_eq!(
        result["plan"]["cut_anchors"][0]["beat"]["project_frame"],
        30
    );
    assert_eq!(
        result["plan"]["shots"][0]["source_range"],
        json!({"start": 10, "end": 40})
    );
    assert_eq!(
        result["prepared_edit_plan"]["preview"]["operation_count"],
        2
    );

    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query");
    };
    assert!(document.tracks[0].clips.is_empty());
}

#[test]
fn beat_montage_validates_optional_cadence_before_preparing() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let mut args = montage_plan_args();
    args.cadence = Some(BeatMontageCadenceContract {
        minimum_duration_buckets: 1,
        duration_bucket_frames: TimeCode(20),
        maximum_similar_run: 2,
        similar_tolerance_frames: TimeCode(8),
    });
    let result = service
        .plan_beat_montage(&args)
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(result["cadence"]["distinct_buckets"], json!([2]));
    assert_eq!(result["cadence"]["longest_similar_run"], 2);

    args.cadence = Some(BeatMontageCadenceContract {
        minimum_duration_buckets: 3,
        duration_bucket_frames: TimeCode(20),
        maximum_similar_run: 2,
        similar_tolerance_frames: TimeCode(8),
    });
    let rejected = service.plan_beat_montage(&args).unwrap();
    assert_eq!(rejected.is_error, Some(true));
    let message = rejected.content[0].as_text().unwrap().text.as_str();
    assert!(message.contains("beat montage cadence contract rejected prepared plan"));
    assert!(message.contains("requires at least 3 distinct buckets"));
}

#[test]
fn beat_montage_schema_exposes_optional_cadence_and_anchor_repair_contracts() {
    let tool = KinewrightMcp::capability_tools()
        .unwrap()
        .into_iter()
        .find(|tool| tool.name == "plan_beat_montage")
        .expect("beat montage capability is registered");
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("beat montage schema properties");
    assert!(properties.contains_key("cadence"));
    assert!(properties.contains_key("anchor_repair"));
    let schema = serde_json::to_string(&tool.input_schema).unwrap();
    assert!(
        tool.description
            .as_deref()
            .is_some_and(|description| description.contains("cadence contract")
                && description.contains("remain exact unless anchor_repair"))
    );
    for field in [
        "minimum_duration_buckets",
        "duration_bucket_frames",
        "maximum_similar_run",
        "similar_tolerance_frames",
    ] {
        assert!(schema.contains(field), "cadence schema omitted {field}");
    }
    for field in ["maximum_movement_frames", "locked_anchor_indices"] {
        assert!(
            schema.contains(field),
            "anchor repair schema omitted {field}"
        );
    }
    let repair_schema = tool.input_schema["$defs"]
        .as_object()
        .and_then(|definitions| {
            definitions.values().find(|schema| {
                schema["properties"]
                    .as_object()
                    .is_some_and(|properties| properties.contains_key("maximum_movement_frames"))
            })
        })
        .expect("anchor repair definition");
    let repair_required = repair_schema["required"]
        .as_array()
        .expect("anchor repair required fields");
    assert!(
        repair_required
            .iter()
            .any(|field| field == "maximum_movement_frames")
    );
    assert!(
        repair_required
            .iter()
            .all(|field| field != "locked_anchor_indices")
    );
    assert!(
        !tool
            .input_schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|required| required.iter().any(|field| field == "cadence"))
    );
    assert!(
        !tool
            .input_schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|required| required.iter().any(|field| field == "anchor_repair"))
    );
}

#[test]
fn beat_montage_preserves_explicit_model_selected_anchor() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let mut args = montage_plan_args();
    args.cut_anchor_frames = Some(vec![TimeCode(30)]);
    let result = service
        .plan_beat_montage(&args)
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(
        result["plan"]["cut_anchors"][0]["beat"]["project_frame"],
        30
    );
    assert!(result["anchor_repair"].is_null());

    args.cut_anchor_frames = Some(vec![TimeCode(31)]);
    let rejected = service.plan_beat_montage(&args).unwrap();
    assert_eq!(rejected.is_error, Some(true));
    assert!(
        rejected.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("not an eligible beat for music asset")
    );
}

#[test]
fn beat_montage_repairs_preferred_anchor_with_bounded_inspectable_evidence() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core.clone(),
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let mut args = montage_plan_args();
    args.cut_anchor_frames = Some(vec![TimeCode(31)]);
    args.anchor_repair = Some(BeatMontageAnchorRepairArgs {
        maximum_movement_frames: TimeCode(2),
        locked_anchor_indices: Vec::new(),
    });

    let result = service
        .plan_beat_montage(&args)
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(
        result["plan"]["cut_anchors"][0]["beat"]["project_frame"],
        30
    );
    assert_eq!(result["plan"]["shots"][0]["asset"], 1);
    assert_eq!(result["plan"]["shots"][1]["asset"], 2);
    assert_eq!(
        result["plan"]["shots"][0]["source_envelope"],
        json!({"start": 10, "end": 100})
    );
    assert_eq!(
        result["plan"]["shots"][1]["source_envelope"],
        json!({"start": 20, "end": 110})
    );
    assert_eq!(result["anchor_repair"]["repaired"], true);
    assert_eq!(
        result["anchor_repair"]["preferred_anchor_frames"],
        json!([31])
    );
    assert_eq!(
        result["anchor_repair"]["resolved_anchor_frames"],
        json!([30])
    );
    assert_eq!(result["anchor_repair"]["signed_delta_frames"], json!([-1]));
    assert_eq!(result["anchor_repair"]["absolute_delta_frames"], json!([1]));
    assert_eq!(result["anchor_repair"]["maximum_absolute_delta_frames"], 1);
    assert_eq!(result["anchor_repair"]["total_absolute_delta_frames"], 1);
    assert_eq!(result["anchor_repair"]["maximum_movement_frames"], 2);
    assert_eq!(
        result["prepared_edit_plan"]["preview"]["operation_count"],
        2
    );

    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query");
    };
    assert!(document.tracks[0].clips.is_empty());
}

#[test]
fn beat_montage_anchor_repair_enforces_opt_in_bounds_and_locks() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let mut args = montage_plan_args();
    args.anchor_repair = Some(BeatMontageAnchorRepairArgs {
        maximum_movement_frames: TimeCode(2),
        locked_anchor_indices: Vec::new(),
    });
    let missing_anchors = service.plan_beat_montage(&args).unwrap();
    assert_eq!(missing_anchors.is_error, Some(true));
    assert!(
        missing_anchors.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("anchor_repair requires explicit cut_anchor_frames")
    );

    args.cut_anchor_frames = Some(vec![TimeCode(31)]);
    args.anchor_repair.as_mut().unwrap().maximum_movement_frames = TimeCode(-1);
    let negative = service.plan_beat_montage(&args).unwrap();
    assert_eq!(negative.is_error, Some(true));
    assert!(
        negative.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("must be non-negative")
    );

    args.anchor_repair.as_mut().unwrap().maximum_movement_frames = TimeCode::ZERO;
    let bounded = service.plan_beat_montage(&args).unwrap();
    assert_eq!(bounded.is_error, Some(true));
    assert!(
        bounded.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("within maximum_movement_frames=0")
    );
    let settings = args.anchor_repair.as_mut().unwrap();
    settings.maximum_movement_frames = TimeCode(2);
    settings.locked_anchor_indices = vec![0];
    let locked = service.plan_beat_montage(&args).unwrap();
    assert_eq!(locked.is_error, Some(true));
    assert!(
        locked.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("not an eligible beat for music asset")
    );

    args.anchor_repair.as_mut().unwrap().locked_anchor_indices = vec![0, 0];
    let duplicates = service.plan_beat_montage(&args).unwrap();
    assert_eq!(duplicates.is_error, Some(true));
    assert!(
        duplicates.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("must be strictly increasing and unique")
    );

    args.anchor_repair.as_mut().unwrap().locked_anchor_indices = vec![1];
    let out_of_range_lock = service.plan_beat_montage(&args).unwrap();
    assert_eq!(out_of_range_lock.is_error, Some(true));
    assert!(
        out_of_range_lock.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("invalid beat montage anchor-repair settings")
    );
}

#[test]
fn beat_montage_bounded_failure_returns_one_exact_feasible_retry() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let mut args = montage_plan_args();
    args.cut_anchor_frames = Some(vec![TimeCode(31)]);
    args.anchor_repair = Some(BeatMontageAnchorRepairArgs {
        maximum_movement_frames: TimeCode::ZERO,
        locked_anchor_indices: Vec::new(),
    });

    let rejected = service.plan_beat_montage(&args).unwrap();
    assert_eq!(rejected.is_error, Some(true));
    assert!(
        rejected.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("reuse it instead of guessing")
    );
    let recovery = rejected.structured_content.unwrap();
    assert_eq!(recovery["status"], "bounded_anchor_repair_infeasible");
    let feasible = &recovery["nearest_globally_feasible"];
    assert_eq!(feasible["cut_anchor_frames"], json!([30]));
    assert_eq!(feasible["shot_durations"], json!([30, 30]));
    assert_eq!(
        feasible["exact_retry_patch"],
        json!({
            "cut_anchor_frames": [30],
            "anchor_repair": {
                "maximum_movement_frames": 0,
                "locked_anchor_indices": [],
            },
        })
    );

    args.cut_anchor_frames = Some(vec![TimeCode(30)]);
    let exact_retry = service.plan_beat_montage(&args).unwrap();
    assert_eq!(exact_retry.is_error, Some(false));
    assert_eq!(
        exact_retry.structured_content.unwrap()["plan"]["cut_anchors"][0]["beat"]["project_frame"],
        30
    );
}

#[test]
fn beat_montage_surfaces_source_capacity_and_repair_hint() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let mut args = montage_plan_args();
    args.selects[0].source_range = TranscriptRangeArgs {
        start: TimeCode::ZERO,
        end: TimeCode(20),
    };

    let rejected = service.plan_beat_montage(&args).unwrap();
    assert_eq!(rejected.is_error, Some(true));
    let message = rejected.content[0].as_text().unwrap().text.as_str();
    assert!(message.contains("can supply at most 20 project frames"));
    assert!(
        message
            .contains("reassign this select to a shorter slot or select a larger source envelope")
    );
}

#[test]
fn beat_montage_reports_music_analysis_pending_and_failure_explicitly() {
    let (pending_core, pending_analysis) = montage_fixture(BeatStatus::NotRequested);
    let pending_service = KinewrightMcp::new(
        pending_core,
        pending_analysis.clone(),
        pending_analysis.clone(),
        ConfirmationBroker::default(),
    );
    let pending = pending_service
        .plan_beat_montage(&montage_plan_args())
        .unwrap();
    assert_eq!(pending.is_error, Some(true));
    assert!(
        pending.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("still pending for assets [AssetId(9)]")
    );
    assert_eq!(
        *pending_analysis.beat_requests.lock().unwrap(),
        vec![AssetId(9)]
    );

    let (failed_core, failed_analysis) =
        montage_fixture(BeatStatus::Failed("decoder stopped".to_owned()));
    let failed_service = KinewrightMcp::new(
        failed_core,
        failed_analysis.clone(),
        failed_analysis.clone(),
        ConfirmationBroker::default(),
    );
    let failed = failed_service
        .plan_beat_montage(&montage_plan_args())
        .unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert!(
        failed.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("beat analysis failed: decoder stopped")
    );
    assert!(failed_analysis.beat_requests.lock().unwrap().is_empty());
}

#[test]
fn beat_montage_is_internal_and_invocable_through_the_compact_dispatcher() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    assert!(registry.iter().any(|tool| tool.name == "plan_beat_montage"));
    assert!(
        KinewrightMcp::served_tools()
            .unwrap()
            .iter()
            .all(|tool| tool.name != "plan_beat_montage")
    );

    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let invoked = service
        .call_exposed_blocking(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({
                    "name": "plan_beat_montage",
                    "arguments": {
                        "target_track_id": 1,
                        "music_asset_id": 9,
                        "timeline_range": {"start": 0, "end": 60},
                        "selects": [
                            {"asset_id": 1, "source_range": {"start": 10, "end": 100}},
                            {"asset_id": 2, "source_range": {"start": 20, "end": 110}}
                        ],
                        "cut_anchor_frames": [31],
                        "anchor_repair": {
                            "maximum_movement_frames": 2,
                            "locked_anchor_indices": []
                        },
                        "cadence": {
                            "minimum_duration_buckets": 1,
                            "duration_bucket_frames": 20,
                            "maximum_similar_run": 2,
                            "similar_tolerance_frames": 8
                        }
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(invoked.is_error, Some(false));
    let invoked = invoked.structured_content.unwrap();
    assert_eq!(invoked["plan"]["shots"].as_array().unwrap().len(), 2);
    assert_eq!(invoked["anchor_repair"]["repaired"], true);
    assert_eq!(
        invoked["anchor_repair"]["preferred_anchor_frames"],
        json!([31])
    );
    assert_eq!(
        invoked["anchor_repair"]["resolved_anchor_frames"],
        json!([30])
    );
}

#[test]
fn beat_montage_prepared_plan_commits_gaplessly() {
    let (core, analysis) = montage_fixture(ready_montage_status());
    let service = KinewrightMcp::new(
        core.clone(),
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let planned = service
        .plan_beat_montage(&montage_plan_args())
        .unwrap()
        .structured_content
        .unwrap();

    let committed = commit_prepared_plan(&service, &planned, TimelineRevision::default());
    assert_eq!(committed.is_error, Some(false));
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query");
    };
    let clips = &document.tracks[0].clips;
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].timeline_start, TimeCode::ZERO);
    assert_eq!(clips[1].timeline_start, TimeCode(30));
    assert_eq!(clips[0].source_range, TimeCode(10)..TimeCode(40));
    assert_eq!(clips[1].source_range, TimeCode(20)..TimeCode(50));
}

#[test]
fn music_structure_is_ready_filtered_and_does_not_mutate() {
    let beats = vec![
        TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode::ZERO,
            project_frame: TimeCode::ZERO,
            strength_basis_points: 9_000,
            estimated_bpm_milli: 120_000,
        },
        TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode(30),
            project_frame: TimeCode(30),
            strength_basis_points: 4_000,
            estimated_bpm_milli: 120_000,
        },
        TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode(60),
            project_frame: TimeCode(60),
            strength_basis_points: 8_000,
            estimated_bpm_milli: 120_000,
        },
        TimelineBeat {
            asset: AssetId(1),
            track: TrackId(1),
            clip: ClipId(1),
            source_frame: TimeCode(30),
            project_frame: TimeCode(30),
            strength_basis_points: 10_000,
            estimated_bpm_milli: 120_000,
        },
    ];
    let (core, analysis) = music_structure_fixture(ready_music_structure_status(), beats);
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let before = service.document().unwrap();
    let result = service
        .music_structure(&MusicStructureArgs {
            music_asset_id: AssetId(9),
            range: Some(TranscriptRangeArgs {
                start: TimeCode::ZERO,
                end: TimeCode(100),
            }),
            min_strength: Some(50.0),
            meter_beats: Some(4),
            phrase_bars: Some(2),
            structural_only: false,
        })
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["analysis_status"], "ready");
    assert_eq!(structured["heuristic"], true);
    assert!(
        structured["disclaimer"]
            .as_str()
            .unwrap()
            .contains("not guaranteed music theory")
    );
    assert_eq!(structured["parameters"]["meter_beats"], 4);
    assert_eq!(structured["parameters"]["phrase_bars"], 2);
    assert_eq!(
        structured["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|candidate| candidate["project_frame"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![0, 60]
    );
    assert!(
        structured["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate["asset"] == 9)
    );
    assert_eq!(&*service.document().unwrap(), &*before);
    assert!(
        service
            .prepared_plans
            .lock()
            .unwrap()
            .get(PreparedPlanId(1))
            .is_none()
    );
}

#[test]
fn music_structure_structural_only_compacts_ordinary_candidates_and_reports_counts() {
    let beats = vec![
        TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode::ZERO,
            project_frame: TimeCode::ZERO,
            strength_basis_points: 9_000,
            estimated_bpm_milli: 120_000,
        },
        TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode(30),
            project_frame: TimeCode(30),
            strength_basis_points: 4_000,
            estimated_bpm_milli: 120_000,
        },
        TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode(60),
            project_frame: TimeCode(60),
            strength_basis_points: 8_000,
            estimated_bpm_milli: 120_000,
        },
    ];
    let (core, analysis) = music_structure_fixture(ready_music_structure_status(), beats);
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let result = service
        .music_structure(&MusicStructureArgs {
            music_asset_id: AssetId(9),
            range: Some(TranscriptRangeArgs {
                start: TimeCode::ZERO,
                end: TimeCode(100),
            }),
            min_strength: Some(0.0),
            meter_beats: Some(4),
            phrase_bars: Some(2),
            structural_only: true,
        })
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["structural_only"], true);
    assert_eq!(structured["total_candidate_count"], 3);
    assert_eq!(structured["returned_candidate_count"], 1);
    assert_eq!(structured["omitted_ordinary_candidate_count"], 2);
    assert!(
        structured["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate["role"] != "beat")
    );
}

#[test]
fn music_structure_reports_pending_and_failed_analysis_lifecycle() {
    let (pending_core, pending_analysis) = music_structure_fixture(
        BeatStatus::NotRequested,
        vec![TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode(30),
            project_frame: TimeCode(30),
            strength_basis_points: 9_000,
            estimated_bpm_milli: 120_000,
        }],
    );
    let pending_service = KinewrightMcp::new(
        pending_core,
        pending_analysis.clone(),
        pending_analysis.clone(),
        ConfirmationBroker::default(),
    );
    let pending = pending_service
        .music_structure(&MusicStructureArgs {
            music_asset_id: AssetId(9),
            range: None,
            min_strength: None,
            meter_beats: None,
            phrase_bars: None,
            structural_only: false,
        })
        .unwrap();
    assert_eq!(pending.is_error, Some(true));
    assert!(
        pending.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("still pending for assets [AssetId(9)]")
    );
    assert_eq!(
        *pending_analysis.beat_requests.lock().unwrap(),
        vec![AssetId(9)]
    );

    let (failed_core, failed_analysis) =
        music_structure_fixture(BeatStatus::Failed("decoder stopped".to_owned()), Vec::new());
    let failed_service = KinewrightMcp::new(
        failed_core,
        failed_analysis.clone(),
        failed_analysis.clone(),
        ConfirmationBroker::default(),
    );
    let failed = failed_service
        .music_structure(&MusicStructureArgs {
            music_asset_id: AssetId(9),
            range: None,
            min_strength: None,
            meter_beats: None,
            phrase_bars: None,
            structural_only: false,
        })
        .unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert!(
        failed.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("beat analysis failed: decoder stopped")
    );
    assert!(failed_analysis.beat_requests.lock().unwrap().is_empty());
}

#[test]
fn music_structure_is_internal_and_invocable_through_compact_dispatcher() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    let tool = registry
        .iter()
        .find(|tool| tool.name == "get_music_structure")
        .expect("music structure is registered");
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("music structure schema properties");
    assert!(properties.contains_key("structural_only"));
    assert!(
        KinewrightMcp::served_tools()
            .unwrap()
            .iter()
            .all(|tool| tool.name != "get_music_structure")
    );

    let (core, analysis) = music_structure_fixture(
        ready_music_structure_status(),
        vec![TimelineBeat {
            asset: AssetId(9),
            track: TrackId(2),
            clip: ClipId(90),
            source_frame: TimeCode(30),
            project_frame: TimeCode(30),
            strength_basis_points: 9_000,
            estimated_bpm_milli: 120_000,
        }],
    );
    let service = KinewrightMcp::new(
        core,
        analysis.clone(),
        analysis,
        ConfirmationBroker::default(),
    );
    let invoked = service
        .call_exposed_blocking(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({
                    "name": "get_music_structure",
                    "arguments": {
                        "music_asset_id": 9,
                        "range": {"start": 0, "end": 90},
                        "min_strength": 0,
                        "meter_beats": 4,
                        "phrase_bars": 4,
                        "structural_only": true
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(invoked.is_error, Some(false));
    let structured = invoked.structured_content.unwrap();
    assert_eq!(structured["structural_only"], true);
    assert_eq!(structured["total_candidate_count"], 1);
    assert_eq!(structured["returned_candidate_count"], 1);
    assert_eq!(structured["omitted_ordinary_candidate_count"], 0);
    assert_eq!(structured["candidates"].as_array().unwrap().len(), 1);
}
