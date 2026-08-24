use std::{path::PathBuf, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use kinewright_agent::McpServer;
use kinewright_core::{
    Analysis, AssetId, Clip, ClipId, Command, Core, Document, Effect, EffectId, Event, Marker,
    MarkerId, MediaAsset, MediaKind, ParamValue, Query, QueryResult, Rational, TimeCode, Track,
    TrackId, TrackKind,
};
use kinewright_media::{
    FfmpegMediaEngine,
    test_support::{GeneratedMedia, single_clip_document},
};
use rmcp::{
    RoleClient, ServiceExt as _,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::StreamableHttpClientTransport,
};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn compact_runtime_applies_a_plan_and_rejects_direct_internal_tools() {
    let core = Core::spawn(Document::default()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let tools = client.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 7);
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        kinewright_agent::compact_tool_names()
    );

    let direct = client
        .call_tool(
            CallToolRequestParams::new("add_track").with_arguments(
                json!({"expected_revision": 0, "track": {"id": 7, "kind": "Video", "clips": []}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(direct.is_error, Some(true));
    assert!(
        direct.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("internal capability")
    );
    assert!(query_document(&core).tracks.is_empty());

    let prepared = prepare_plan(
        &client,
        0,
        json!([
            {"op": "add_track", "track": {"id": 7, "kind": "Video", "clips": []}},
            {"op": "set_track_sync_lock", "track": 7, "locked": false},
            {"op": "add_marker", "marker": {
                "id": 1,
                "position": 0,
                "label": "Review",
                "color_token": 0
            }}
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false));
    let result = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let outcome = &result.content[0].as_text().unwrap().text;
    assert!(outcome.contains("op 1 add_track: applied"));
    assert!(outcome.contains("tracks 0->1"));
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query result");
    };
    assert_eq!(document.tracks[0].id, TrackId(7));
    assert!(!query_document(&core).tracks[0].sync_lock);

    let timeline_state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    assert_eq!(timeline_state.is_error, Some(false));
    assert!(
        timeline_state.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("track 7 video sync_lock=false clips=0")
    );

    assert_eq!(query_document(&core).markers[0].id, MarkerId(1));

    let rejected = prepare_plan(
        &client,
        1,
        json!([{"op": "add_track", "track": {"id": 7, "kind": "Video", "clips": []}}]),
    )
    .await;
    assert_eq!(rejected.is_error, Some(true));
    assert!(
        rejected.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("track 7 occurs more than once")
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_plans_cross_the_real_mcp_server_atomically_with_one_confirmation() {
    let original = edit_plan_document();
    let core = Core::spawn(original.clone()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let confirmations = server.confirmations();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let prepared = prepare_plan(
        &client,
        0,
        json!([
            {"op": "add_track", "track": {"id": 2, "kind": "Video", "clips": []}},
            {"op": "move_clip", "clip": 1, "to_track": 2, "to": 0}
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false));
    let applied = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(applied.is_error, Some(false));
    let applied_text = &applied.content[0].as_text().unwrap().text;
    assert!(applied_text.contains("op 1 add_track: applied"));
    assert!(applied_text.contains("op 2 move_clip: applied"));
    // Every plan result self-reports remaining cuttable silence so the agent
    // cannot mistake a partial cleanup for a finished one.
    assert!(
        applied_text.contains("cuttable silence"),
        "plan result must include the silence completion footer: {applied_text}"
    );
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("one undo should restore the pre-plan document");
    };
    assert_eq!(&*doc, &original);

    let rejected = prepare_plan(
        &client,
        2,
        json!([
            {"op": "add_track", "track": {"id": 2, "kind": "Video", "clips": []}},
            {"op": "add_track", "track": {"id": 2, "kind": "Video", "clips": []}}
        ]),
    )
    .await;
    assert_eq!(rejected.is_error, Some(true));
    let rejected_text = &rejected.content[0].as_text().unwrap().text;
    assert!(rejected_text.contains("edit plan is invalid"));
    assert_eq!(query_document(&core), original);

    let destructive = prepare_plan(&client, 2, json!([{"op": "remove_track", "track": 1}])).await;
    assert_eq!(destructive.is_error, Some(false));
    let (approved, ()) = tokio::join!(
        client.call_tool(commit_request(2, &destructive)),
        resolve_plan_confirmation(confirmations.clone(), true),
    );
    let approved = approved.unwrap();
    assert_eq!(approved.is_error, Some(false));
    assert!(query_document(&core).tracks.is_empty());
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("undo should restore the approved destructive plan");
    };
    assert_eq!(&*doc, &original);

    let destructive = prepare_plan(&client, 4, json!([{"op": "remove_track", "track": 1}])).await;
    assert_eq!(destructive.is_error, Some(false));
    let (refused, ()) = tokio::join!(
        client.call_tool(commit_request(4, &destructive)),
        resolve_plan_confirmation(confirmations, false),
    );
    let refused = refused.unwrap();
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(query_document(&core), original);

    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn ripple_marker_position_renders_through_the_real_mcp_server() {
    let mut document = edit_plan_document();
    document.markers.push(Marker {
        id: MarkerId(1),
        position: TimeCode(30),
        label: "Review cut".to_owned(),
        color_token: 0,
    });
    let core = Core::spawn(document).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let prepared = prepare_plan(
        &client,
        0,
        json!([{"op": "ripple_insert_gap", "track": 1, "at": 30, "duration": 15}]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false));
    let ripple = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(ripple.is_error, Some(false));
    assert_eq!(
        query_document(&core).marker(MarkerId(1)).unwrap().position,
        TimeCode(45)
    );

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    assert_eq!(state.is_error, Some(false));
    assert!(
        state.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("marker 1 at=45f/1.500s color=0 label=\"Review cut\"")
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn visual_proof_and_analysis_lifecycle_work_on_generated_media() {
    let generated = GeneratedMedia::ffmpeg(
        "m3",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30000/1001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-frames:v",
            "60",
            "-t",
            "2.002",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "60",
            "-c:a",
            "aac",
            "-shortest",
        ],
        "mp4",
    );
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let mut document = single_clip_document(asset);
    document.tracks[0].clips[0].effects.push(Effect {
        id: EffectId(1),
        name: "opacity".to_owned(),
        parameters: std::collections::BTreeMap::from([(
            "percent".to_owned(),
            ParamValue::Integer(0),
        )]),
        keyframes: std::collections::BTreeMap::new(),
    });
    document.tracks[0].clips[0].effects.push(Effect {
        id: EffectId(2),
        name: "mask".to_owned(),
        parameters: std::collections::BTreeMap::from([
            ("center_x_percent".to_owned(), ParamValue::Integer(50)),
            ("center_y_percent".to_owned(), ParamValue::Integer(50)),
            ("width_percent".to_owned(), ParamValue::Integer(40)),
            ("height_percent".to_owned(), ParamValue::Integer(40)),
        ]),
        keyframes: std::collections::BTreeMap::new(),
    });
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core, media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let result = invoke_capability(&client, "get_frame_at", json!({"timecode": 30})).await;

    assert_eq!(result.is_error, Some(false));
    let image = result
        .content
        .iter()
        .find_map(|content| content.as_image())
        .expect("tool result must contain image content");
    assert_eq!(image.mime_type, "image/png");
    let png = BASE64.decode(&image.data).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 180));
    assert!(decoded.width() <= 512);
    assert!(
        decoded
            .to_rgba8()
            .pixels()
            .all(|pixel| pixel.0 == [0, 0, 0, 255]),
        "proof frames must use the compositor and include timeline effects"
    );

    let storyboard = invoke_capability(
        &client,
        "get_timeline_storyboard",
        json!({"frame_count": 4, "max_width": 160}),
    )
    .await;
    assert_eq!(storyboard.is_error, Some(false));
    let manifest = storyboard
        .structured_content
        .as_ref()
        .expect("storyboard must publish a machine-readable manifest");
    assert_eq!(manifest["timeline_revision"], 0);
    assert_eq!(manifest["cells"][0]["project_frame"], 0);
    assert_eq!(manifest["cells"][3]["project_frame"], 59);
    let storyboard_image = storyboard
        .content
        .iter()
        .find_map(|content| content.as_image())
        .expect("storyboard must contain image content");
    let png = BASE64.decode(&storyboard_image.data).unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (652, 90));

    let tracking = invoke_capability(
        &client,
        "track_mask_region",
        json!({
            "clip_id": 1,
            "effect_id": 2,
            "start_local_frame": 0,
            "end_local_frame": 11,
            "step_frames": 5,
            "max_width": 64
        }),
    )
    .await;
    assert_eq!(tracking.is_error, Some(false));
    let tracking = tracking
        .structured_content
        .as_ref()
        .expect("tracking must return machine-readable keyframe operations");
    assert_eq!(tracking["timeline_revision"], 0);
    assert_eq!(tracking["observations"].as_array().unwrap().len(), 3);
    assert_eq!(
        tracking["prepared_edit_plan"]["preview"]["operation_count"],
        2
    );

    let requested = invoke_capability(
        &client,
        "request_analysis",
        json!({"asset_id": 1, "kinds": ["beat"]}),
    )
    .await;
    assert_eq!(requested.is_error, Some(false));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status =
            invoke_capability(&client, "get_analysis_status", json!({"asset_id": 1})).await;
        let jobs = status.structured_content.as_ref().unwrap()["jobs"]
            .as_array()
            .unwrap();
        let beat = jobs
            .iter()
            .find(|job| job["kind"] == "beat")
            .expect("uniform lifecycle must include beat analysis");
        if beat["phase"] == "ready" {
            break;
        }
        assert_ne!(beat["phase"], "failed", "beat job failed: {beat}");
        assert!(
            tokio::time::Instant::now() < deadline,
            "beat analysis did not finish: {beat}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let beats = invoke_capability(&client, "get_timeline_beats", json!({"min_strength": 0})).await;
    assert_eq!(beats.is_error, Some(false));
    assert!(beats.structured_content.as_ref().unwrap()["beats"].is_array());

    client.cancel().await.unwrap();
    server.shutdown();
}

fn edit_plan_document() -> Document {
    let asset = MediaAsset {
        id: AssetId(1),
        path: PathBuf::from("fixture.mp4"),
        name: "fixture".to_owned(),
        duration: TimeCode(60),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((320, 180)),
        color_description: kinewright_core::ColorDescription::default(),
    };
    Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..TimeCode(60),
                content: kinewright_core::ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps: Rational::new(30, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(60),
        color_context: kinewright_core::ColorContext::default(),
    }
}

async fn invoke_capability(
    client: &RunningService<RoleClient, ()>,
    name: &str,
    arguments: serde_json::Value,
) -> CallToolResult {
    client
        .call_tool(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({"name": name, "arguments": arguments})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
}

async fn prepare_plan(
    client: &RunningService<RoleClient, ()>,
    expected_revision: u64,
    operations: serde_json::Value,
) -> CallToolResult {
    client
        .call_tool(
            CallToolRequestParams::new("prepare_edit_plan").with_arguments(
                json!({
                    "expected_revision": expected_revision,
                    "operations": operations
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap()
}

fn commit_request(expected_revision: u64, prepared: &CallToolResult) -> CallToolRequestParams {
    let plan_id = prepared
        .structured_content
        .as_ref()
        .expect("prepared plans must return structured content")["plan_id"]
        .clone();
    CallToolRequestParams::new("commit_edit_plan").with_arguments(
        json!({
            "plan_id": plan_id,
            "expected_revision": expected_revision
        })
        .as_object()
        .unwrap()
        .clone(),
    )
}

fn query_document(core: &Core) -> Document {
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query result");
    };
    (*document).clone()
}

async fn resolve_plan_confirmation(broker: kinewright_agent::ConfirmationBroker, approve: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(request) = broker.pending_requests().into_iter().next() {
            assert_eq!(request.tool_name, "apply_edit_plan");
            assert_eq!(
                request.description,
                "Plan removes 1 clip and 1 track - approve?"
            );
            if approve {
                assert!(broker.approve(request.id));
            } else {
                assert!(broker.reject(request.id, "keep the original timeline"));
            }
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "plan confirmation was not published"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
