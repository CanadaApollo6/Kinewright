//! Storyboard, cut neighbourhood, and shot board tests.

use super::*;
use crate::server::source_program::coverage_candidate_positions;
use crate::server::storyboards::{
    compose_contact_sheet, rgba_mean_absolute_difference_basis_points, storyboard_sample_frames,
};

#[test]
fn storyboard_sampling_is_bounded_uniform_and_includes_visible_edges() {
    assert_eq!(
        storyboard_sample_frames(&(TimeCode(0)..TimeCode(10)), 4),
        [TimeCode(0), TimeCode(3), TimeCode(6), TimeCode(9)]
    );
    assert_eq!(
        storyboard_sample_frames(&(TimeCode(0)..TimeCode(10)), 1),
        [TimeCode(4)]
    );
}

#[test]
fn contact_sheet_preserves_cells_and_uses_a_dark_opaque_gutter() {
    let red = kinewright_core::RgbaImage {
        width: 2,
        height: 1,
        pixels: vec![255, 0, 0, 255, 255, 0, 0, 255],
    };
    let blue = kinewright_core::RgbaImage {
        width: 2,
        height: 1,
        pixels: vec![0, 0, 255, 255, 0, 0, 255, 255],
    };
    let sheet = compose_contact_sheet(&[red, blue]).unwrap();
    assert_eq!(sheet.width, 2 * 2 + STORYBOARD_GUTTER);
    assert_eq!(sheet.height, 1);
    assert_eq!(&sheet.pixels[..4], &[255, 0, 0, 255]);
    let gutter = 2_usize * 4;
    assert_eq!(&sheet.pixels[gutter..gutter + 4], &[16, 16, 16, 255]);
    let blue_start = usize::try_from(2 + STORYBOARD_GUTTER).unwrap() * 4;
    assert_eq!(&sheet.pixels[blue_start..blue_start + 4], &[0, 0, 255, 255]);
}

#[test]
fn rgba_difference_reports_full_range_and_rejects_mismatched_images() {
    let black = kinewright_core::RgbaImage {
        width: 1,
        height: 1,
        pixels: vec![0, 0, 0, 255],
    };
    let white = kinewright_core::RgbaImage {
        width: 1,
        height: 1,
        pixels: vec![255, 255, 255, 255],
    };
    let mismatched = kinewright_core::RgbaImage {
        width: 2,
        height: 1,
        pixels: vec![0; 8],
    };
    assert_eq!(
        rgba_mean_absolute_difference_basis_points(&black, &black),
        Some(0)
    );
    assert_eq!(
        rgba_mean_absolute_difference_basis_points(&black, &white),
        Some(10_000)
    );
    assert_eq!(
        rgba_mean_absolute_difference_basis_points(&black, &mismatched),
        None
    );
}

#[test]
fn source_storyboard_maps_cells_to_exact_source_frames_without_mutating_timeline() {
    let (core, playback, analysis) = fixture();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let before = service.document().unwrap();
    let result = service
        .call_blocking(
            CallToolRequestParams::new("get_source_storyboard").with_arguments(
                json!({
                    "asset_id": 1,
                    "range": {"start": 10, "end": 50},
                    "frame_count": 4,
                    "max_width": 64
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert!(
        result
            .content
            .iter()
            .any(|block| block.as_image().is_some())
    );
    let manifest = result.structured_content.unwrap();
    assert_eq!(manifest["timeline_revision"], 0);
    assert_eq!(manifest["asset_id"], 1);
    assert_eq!(manifest["source_range"], json!({"start": 10, "end": 50}));
    assert_eq!(manifest["sheet"], json!({"width": 20, "height": 2}));
    let cells = manifest["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 4);
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell["source_frame"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [10, 23, 36, 49]
    );
    for cell in cells {
        assert_eq!(cell["asset_id"], 1);
        assert_eq!(cell["source_range"], json!({"start": 10, "end": 50}));
    }
    assert_eq!(&*service.document().unwrap(), &*before);
}

#[test]
fn source_storyboard_rejects_missing_nonvideo_and_invalid_requests() {
    let (core, playback, analysis) = fixture();
    core.request(Command::Do(Operation::AddAsset {
        asset: MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("fixture.wav"),
            name: "fixture audio".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        },
    }))
    .unwrap();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    for args in [
        SourceStoryboardArgs {
            asset_id: AssetId(999),
            range: None,
            frame_count: None,
            max_width: None,
        },
        SourceStoryboardArgs {
            asset_id: AssetId(2),
            range: None,
            frame_count: None,
            max_width: None,
        },
        SourceStoryboardArgs {
            asset_id: AssetId(1),
            range: Some(TranscriptRangeArgs {
                start: TimeCode(40),
                end: TimeCode(40),
            }),
            frame_count: None,
            max_width: None,
        },
        SourceStoryboardArgs {
            asset_id: AssetId(1),
            range: Some(TranscriptRangeArgs {
                start: TimeCode(0),
                end: TimeCode(61),
            }),
            frame_count: None,
            max_width: None,
        },
        SourceStoryboardArgs {
            asset_id: AssetId(1),
            range: None,
            frame_count: Some(STORYBOARD_MAX_FRAMES + 1),
            max_width: None,
        },
        SourceStoryboardArgs {
            asset_id: AssetId(1),
            range: None,
            frame_count: None,
            max_width: Some(THUMBNAIL_MAX_WIDTH + 1),
        },
    ] {
        let result = service.source_storyboard(&args).unwrap();
        assert_eq!(result.is_error, Some(true));
    }
}

#[test]
fn source_storyboard_is_internal_registry_capability_not_compact_tool() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    assert!(
        registry
            .iter()
            .any(|tool| tool.name == "get_source_storyboard")
    );
    let served = KinewrightMcp::served_tools().unwrap();
    assert!(
        served
            .iter()
            .all(|tool| tool.name != "get_source_storyboard")
    );
}

#[test]
fn cut_neighborhoods_maps_exact_cut_edges_and_does_not_mutate() {
    let (core, playback, analysis) = fixture();
    core.request(Command::Do(Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(20),
    }))
    .unwrap();
    core.request(Command::Do(Operation::SplitClip {
        clip: ClipId(2),
        at: TimeCode(40),
    }))
    .unwrap();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let before = service.document().unwrap();
    let result = service
        .cut_neighborhoods(&CutNeighborhoodsArgs {
            track_id: TrackId(1),
            frames_before: Some(1),
            frames_after: Some(3),
            cut_offset: None,
            cut_count: None,
            maximum_secondary_change_basis_points: None,
            max_width: Some(64),
        })
        .unwrap();

    assert_eq!(result.is_error, Some(false));
    assert!(
        result
            .content
            .iter()
            .any(|block| block.as_image().is_some())
    );
    let manifest = result.structured_content.unwrap();
    assert_eq!(manifest["timeline_revision"], 2);
    assert_eq!(manifest["track_id"], 1);
    assert_eq!(manifest["total_cut_count"], 2);
    assert_eq!(manifest["returned_cut_count"], 2);
    assert_eq!(manifest["clean"], true);
    assert_eq!(manifest["issue_count"], 0);
    assert_eq!(manifest["sheet"], json!({"width": 20, "height": 8}));
    let cells = manifest["cells"].as_array().unwrap();
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell["project_frame"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [19, 20, 21, 22, 39, 40, 41, 42]
    );
    assert_eq!(cells[0]["side"], "outgoing");
    assert_eq!(cells[4]["side"], "outgoing");
    assert!(cells[1..4].iter().all(|cell| cell["side"] == "incoming"));
    assert!(cells[5..8].iter().all(|cell| cell["side"] == "incoming"));
    assert_eq!(&*service.document().unwrap(), &*before);
}

#[test]
fn cut_neighborhoods_blocks_a_secondary_change_inside_the_incoming_handle() {
    let (core, playback, _) = fixture();
    core.request(Command::Do(Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(20),
    }))
    .unwrap();
    let black = RgbaImage {
        width: 2,
        height: 2,
        pixels: [0, 0, 0, 255].repeat(4),
    };
    let white = RgbaImage {
        width: 2,
        height: 2,
        pixels: [255, 255, 255, 255].repeat(4),
    };
    let analysis = Arc::new(NoopMedia {
        thumbnail_frames: BTreeMap::from([
            (TimeCode(20), black.clone()),
            (TimeCode(21), black),
            (TimeCode(22), white),
        ]),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let result = service
        .cut_neighborhoods(&CutNeighborhoodsArgs {
            track_id: TrackId(1),
            frames_before: Some(1),
            frames_after: Some(3),
            cut_offset: None,
            cut_count: Some(1),
            maximum_secondary_change_basis_points: Some(1_200),
            max_width: Some(64),
        })
        .unwrap();

    assert_eq!(result.is_error, Some(false));
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .starts_with("CUT EDGE REVIEW FAILED")
    );
    let manifest = result.structured_content.unwrap();
    assert_eq!(manifest["clean"], false);
    assert_eq!(manifest["issue_count"], 1);
    assert_eq!(manifest["issues"][0]["cut_frame"], 20);
    assert_eq!(manifest["issues"][0]["from_offset"], 1);
    assert_eq!(manifest["issues"][0]["to_offset"], 2);
    assert_eq!(manifest["issues"][0]["change_basis_points"], 10_000);
}

#[test]
fn cut_neighborhoods_rejects_invalid_tracks_and_bounds() {
    let (core, playback, analysis) = fixture();
    core.request(Command::Do(Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        },
    }))
    .unwrap();
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    for args in [
        CutNeighborhoodsArgs {
            track_id: TrackId(999),
            frames_before: None,
            frames_after: None,
            cut_offset: None,
            cut_count: None,
            maximum_secondary_change_basis_points: None,
            max_width: None,
        },
        CutNeighborhoodsArgs {
            track_id: TrackId(2),
            frames_before: None,
            frames_after: None,
            cut_offset: None,
            cut_count: None,
            maximum_secondary_change_basis_points: None,
            max_width: None,
        },
        CutNeighborhoodsArgs {
            track_id: TrackId(1),
            frames_before: Some(0),
            frames_after: None,
            cut_offset: None,
            cut_count: None,
            maximum_secondary_change_basis_points: None,
            max_width: None,
        },
        CutNeighborhoodsArgs {
            track_id: TrackId(1),
            frames_before: None,
            frames_after: None,
            cut_offset: None,
            cut_count: Some(13),
            maximum_secondary_change_basis_points: None,
            max_width: None,
        },
        CutNeighborhoodsArgs {
            track_id: TrackId(1),
            frames_before: None,
            frames_after: None,
            cut_offset: None,
            cut_count: None,
            maximum_secondary_change_basis_points: Some(10_001),
            max_width: None,
        },
    ] {
        assert_eq!(
            service.cut_neighborhoods(&args).unwrap().is_error,
            Some(true)
        );
    }
}

#[test]
fn cut_neighborhoods_is_internal_registry_capability_not_compact_tool() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    assert!(
        registry
            .iter()
            .any(|tool| tool.name == "get_cut_neighborhoods")
    );
    let served = KinewrightMcp::served_tools().unwrap();
    assert!(
        served
            .iter()
            .all(|tool| tool.name != "get_cut_neighborhoods")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn source_shot_board_segments_exact_scenes_pages_evidence_and_does_not_mutate() {
    let (core, playback, _) = fixture();
    let analysis = Arc::new(NoopMedia {
        scene_statuses: BTreeMap::from([(
            AssetId(1),
            SceneStatus::Ready(Arc::new(AssetSceneChanges {
                asset: AssetId(1),
                content_sha256: "fixture".to_owned(),
                source_fps: Rational::new(30, 1).unwrap(),
                source_frames: TimeCode(60),
                proxy_width: 160,
                changes: vec![
                    SceneChange {
                        source_frame: TimeCode(10),
                        confidence_basis_points: 9_100,
                    },
                    SceneChange {
                        source_frame: TimeCode(20),
                        confidence_basis_points: DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS - 1,
                    },
                    SceneChange {
                        source_frame: TimeCode(30),
                        confidence_basis_points: 8_200,
                    },
                    SceneChange {
                        source_frame: TimeCode(45),
                        confidence_basis_points: 7_300,
                    },
                ],
            })),
        )]),
        ..NoopMedia::default()
    });
    let service = KinewrightMcp::new(core, playback, analysis, ConfirmationBroker::default());
    let before = service.document().unwrap();
    let result = service
        .call_blocking(
            CallToolRequestParams::new("get_source_shot_board").with_arguments(
                json!({
                    "asset_id": 1,
                    "range": {"start": 5, "end": 50},
                    "candidate_selection": "page",
                    "candidate_offset": 1,
                    "candidate_count": 2,
                    "max_width": 64,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert!(
        result
            .content
            .iter()
            .any(|block| block.as_image().is_some())
    );
    let manifest = result.structured_content.unwrap();
    assert_eq!(manifest["timeline_revision"], 0);
    assert_eq!(manifest["status"], "ready");
    assert_eq!(
        manifest["scene_confidence_threshold_basis_points"],
        DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS
    );
    assert_eq!(manifest["total_candidates"], 4);
    assert_eq!(manifest["next_candidate_offset"], 3);
    assert_eq!(manifest["sheet"], json!({"width": 20, "height": 8}));
    assert_eq!(
        manifest["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|candidate| candidate["source_range"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!({"start": 10, "end": 30}),
            json!({"start": 30, "end": 45})
        ]
    );
    assert_eq!(
        manifest["candidates"][0]["boundary_provenance"]["start"]["confidence_basis_points"],
        9_100
    );
    assert_eq!(
        manifest["candidates"][1]["boundary_provenance"]["end"]["confidence_basis_points"],
        7_300
    );
    let cells = manifest["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 6);
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell["source_frame"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![10, 19, 29, 30, 37, 44]
    );
    assert_eq!(
        cells[0]["candidate_id"],
        manifest["candidates"][0]["candidate_id"]
    );
    assert_eq!(
        cells[3]["candidate_id"],
        manifest["candidates"][1]["candidate_id"]
    );
    assert_eq!(&*service.document().unwrap(), &*before);

    let filtered = service
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: Some(ShotBoardCandidateSelection::Page),
            candidate_offset: Some(1),
            minimum_duration_frames: Some(TimeCode(15)),
            minimum_confidence_basis_points: None,
            candidate_count: Some(1),
            max_width: Some(64),
        })
        .unwrap();
    assert_eq!(filtered.is_error, Some(false));
    let filtered_manifest = filtered.structured_content.unwrap();
    assert_eq!(filtered_manifest["minimum_duration_frames"], 15);
    assert_eq!(filtered_manifest["total_candidates"], 4);
    assert_eq!(filtered_manifest["filtered_candidates"], 3);
    assert_eq!(filtered_manifest["returned_candidates"], 1);
    assert_eq!(filtered_manifest["next_candidate_offset"], 2);
    assert_eq!(filtered_manifest["candidates"][0]["candidate_index"], 2);
    assert_eq!(
        filtered_manifest["candidates"][0]["candidate_id"],
        "asset-1-scene-30-45"
    );
    let strong_only = service
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: Some(8_000),
            candidate_count: Some(12),
            max_width: Some(64),
        })
        .unwrap();
    let strong_manifest = strong_only.structured_content.unwrap();
    assert_eq!(
        strong_manifest["scene_confidence_threshold_basis_points"],
        8_000
    );
    assert_eq!(strong_manifest["total_candidates"], 3);

    let coverage = service
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: Some(3),
            max_width: Some(64),
        })
        .unwrap();
    let coverage_manifest = coverage.structured_content.unwrap();
    assert_eq!(coverage_manifest["candidate_selection"], "coverage");
    assert_eq!(
        coverage_manifest["candidate_offset"],
        serde_json::Value::Null
    );
    assert_eq!(
        coverage_manifest["next_candidate_offset"],
        serde_json::Value::Null
    );
    assert_eq!(
        coverage_manifest["selected_eligible_candidate_positions"],
        json!([0, 1, 3])
    );
    assert_eq!(
        coverage_manifest["selected_candidate_indexes"],
        json!([0, 1, 3])
    );
    let coverage_ids = coverage_manifest["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["candidate_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        coverage_ids,
        vec![
            json!("asset-1-scene-0-10"),
            json!("asset-1-scene-10-30"),
            json!("asset-1-scene-45-60"),
        ]
    );
    let coverage_again = service
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: Some(ShotBoardCandidateSelection::Coverage),
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: Some(3),
            max_width: Some(64),
        })
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(
        coverage_again["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|candidate| candidate["candidate_id"].clone())
            .collect::<Vec<_>>(),
        coverage_ids
    );

    let single_coverage = service
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: Some(ShotBoardCandidateSelection::Coverage),
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: Some(1),
            max_width: Some(64),
        })
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(
        single_coverage["selected_eligible_candidate_positions"],
        json!([0])
    );
    assert_eq!(single_coverage["candidates"][0]["candidate_index"], 0);
}

#[test]
fn coverage_candidate_positions_span_full_range_without_duplicates() {
    assert_eq!(coverage_candidate_positions(10, 4), vec![0, 3, 6, 9]);
    assert_eq!(coverage_candidate_positions(3, 12), vec![0, 1, 2]);
    assert_eq!(coverage_candidate_positions(10, 1), vec![0]);
    assert!(coverage_candidate_positions(0, 6).is_empty());
}

#[test]
#[allow(clippy::too_many_lines)]
fn source_shot_board_requests_pending_scene_analysis_and_reports_invalid_states() {
    let (core, playback, _) = fixture();
    let analysis = Arc::new(NoopMedia::default());
    let service = KinewrightMcp::new(
        core.clone(),
        playback,
        analysis.clone(),
        ConfirmationBroker::default(),
    );
    let pending = service
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: None,
            max_width: None,
        })
        .unwrap();
    assert_eq!(pending.is_error, Some(false));
    assert_eq!(pending.structured_content.unwrap()["status"], "pending");
    assert_eq!(&*analysis.scene_requests.lock().unwrap(), &[AssetId(1)]);

    let incompatible_coverage = service
        .source_shot_board(&SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: Some(ShotBoardCandidateSelection::Coverage),
            candidate_offset: Some(0),
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: None,
            max_width: None,
        })
        .unwrap();
    assert_eq!(incompatible_coverage.is_error, Some(true));
    assert!(
        incompatible_coverage.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("candidate_offset is only supported")
    );

    core.request(Command::Do(Operation::AddAsset {
        asset: MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("fixture.wav"),
            name: "fixture audio".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        },
    }))
    .unwrap();
    for args in [
        SourceShotBoardArgs {
            asset_id: AssetId(2),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: None,
            max_width: None,
        },
        SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: Some(TranscriptRangeArgs {
                start: TimeCode(10),
                end: TimeCode(10),
            }),
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: None,
            max_width: None,
        },
        SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: None,
            candidate_count: Some(SHOT_BOARD_MAX_CANDIDATES + 1),
            max_width: None,
        },
        SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: Some(TimeCode(0)),
            minimum_confidence_basis_points: None,
            candidate_count: None,
            max_width: None,
        },
        SourceShotBoardArgs {
            asset_id: AssetId(1),
            range: None,
            candidate_selection: None,
            candidate_offset: None,
            minimum_duration_frames: None,
            minimum_confidence_basis_points: Some(10_001),
            candidate_count: None,
            max_width: None,
        },
    ] {
        assert_eq!(
            service.source_shot_board(&args).unwrap().is_error,
            Some(true)
        );
    }

    let failed_analysis = Arc::new(NoopMedia {
        scene_statuses: BTreeMap::from([(
            AssetId(1),
            SceneStatus::Failed("decoder error".to_owned()),
        )]),
        ..NoopMedia::default()
    });
    let failed = KinewrightMcp::new(
        core,
        Arc::new(NoopMedia::default()),
        failed_analysis,
        ConfirmationBroker::default(),
    )
    .source_shot_board(&SourceShotBoardArgs {
        asset_id: AssetId(1),
        range: None,
        candidate_selection: None,
        candidate_offset: None,
        minimum_duration_frames: None,
        minimum_confidence_basis_points: None,
        candidate_count: None,
        max_width: None,
    })
    .unwrap();
    assert_eq!(failed.is_error, Some(true));
}

#[test]
fn source_shot_board_is_internal_registry_capability_not_compact_tool() {
    let registry = KinewrightMcp::capability_tools().unwrap();
    let tool = registry
        .iter()
        .find(|tool| tool.name == "get_source_shot_board")
        .expect("source shot board is registered");
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("source shot board schema properties");
    assert!(properties.contains_key("candidate_selection"));
    assert!(properties.contains_key("minimum_duration_frames"));
    assert!(properties.contains_key("minimum_confidence_basis_points"));
    let served = KinewrightMcp::served_tools().unwrap();
    assert!(
        served
            .iter()
            .all(|tool| tool.name != "get_source_shot_board")
    );
}
