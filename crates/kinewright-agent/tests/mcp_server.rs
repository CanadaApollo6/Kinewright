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
    let mut arguments = vec![
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
    ];
    arguments.extend(MANAGED_BT709_ENCODE_ARGUMENTS);
    arguments.extend(["-c:a", "aac", "-shortest"]);
    let generated = GeneratedMedia::ffmpeg("m3", &arguments, "mp4");
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
    // CC5 §5.2: the prepared values are *layer*-space and are asserted as
    // values, not only counted. The clip carries an opacity-0 node, so every
    // composited thumbnail is uniform and the tracker holds its seeded centre:
    // the 50 percent seed is pixel 32 of the 64-wide thumbnail and pixel 18 of
    // the 36-tall one, which as fractions of the extent are
    // round(32.5 * 100 / 64) = 51 and round(18.5 * 100 / 36) = 51. The layer
    // transform is the identity here, so the conversion is that read alone.
    assert_eq!(tracking["coordinate_space"]["thumbnail"]["width"], 64);
    assert_eq!(tracking["coordinate_space"]["thumbnail"]["height"], 36);
    assert_eq!(tracking["coordinate_space"]["samples"][0]["scale"], 1.0);
    assert_eq!(tracking["coordinate_space"]["samples"][0]["offset_x"], 0.0);
    assert_eq!(tracking["coordinate_space"]["box_percent"], json!([40, 40]));
    assert_eq!(
        tracking["curves"]["center_x_percent"]["keyframes"][0]["value"],
        51
    );
    assert_eq!(
        tracking["curves"]["center_y_percent"]["keyframes"][0]["value"],
        51
    );
    assert_eq!(tracking["observations"][0]["layer_center_x_percent"], 51);
    assert_eq!(tracking["observations"][0]["center_x_percent"], 51);
    // The composite provenance rides alongside, on the same fraction-of-extent
    // convention the response's `coordinate_space.pixel_to_unit` declares.
    assert_eq!(
        tracking["observations"][0]["composite_center_x_percent"],
        51
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

/// The encoder settings and complete BT.709 source-colour tagging every
/// managed fixture in this file shares.
///
/// The colour tools classify a source from exactly these fields, so the
/// fixtures have to agree on them exactly; only the inputs and the container
/// tail differ between them.
const MANAGED_BT709_ENCODE_ARGUMENTS: [&str; 16] = [
    "-c:v",
    "libx264",
    "-vf",
    "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
    "-pix_fmt",
    "yuv420p",
    "-g",
    "60",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-colorspace",
    "bt709",
    "-color_range",
    "tv",
];

/// Generate one managed BT.709 fixture clip for the colour tools.
fn managed_color_media() -> GeneratedMedia {
    let mut arguments = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30",
        "-frames:v",
        "60",
    ];
    arguments.extend(MANAGED_BT709_ENCODE_ARGUMENTS);
    GeneratedMedia::ffmpeg("cc-color", &arguments, "mp4")
}

/// Split the single fixture clip into two shots so `plan_shot_match` has one
/// explicit reference and one explicit candidate.
fn two_shot_color_document(asset: &MediaAsset) -> Document {
    let mut document = single_clip_document(asset.clone());
    let half = TimeCode(asset.duration.0 / 2);
    document.tracks[0].clips[0].source_range = TimeCode::ZERO..half;
    let mut second = document.tracks[0].clips[0].clone();
    second.id = ClipId(2);
    second.source_range = half..asset.duration;
    second.timeline_start = half;
    document.tracks[0].clips.push(second);
    document
}

#[tokio::test(flavor = "multi_thread")]
async fn cc1_color_context_plan_and_commit_advance_the_revision_exactly_once() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    assert_eq!(context.is_error, Some(false));
    let context = context
        .structured_content
        .as_ref()
        .expect("get_color_context must publish machine-readable status");
    let revision = context["timeline_revision"].as_u64().unwrap();
    assert_eq!(revision, 0);
    // CC1 §5: video-only layers, ordered chain, source raster, sampling marker.
    assert_eq!(context["sampling_region"], json!(null));
    assert_eq!(context["layer_scope"], "video_tracks_only");
    assert_eq!(context["clips"][0]["z_order"], 0);
    assert!(context["clips"][0]["effects"].is_array());
    assert_eq!(
        context["assets"][0]["source"]["formats"]["input"]["raster"],
        json!([320, 180])
    );
    assert_eq!(
        context["assets"][0]["source"]["status"]["status"],
        "supported"
    );

    let plan = invoke_capability(
        &client,
        "plan_primary_correction",
        json!({
            "expected_revision": revision,
            "clip_id": 1,
            "parameters": {"exposure_milli_stops": 500}
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false));
    let plan = plan
        .structured_content
        .as_ref()
        .expect("plan_primary_correction must publish exact operations")
        .clone();
    assert_eq!(plan["applied"], false);
    assert_eq!(plan["no_change"], false);
    assert_eq!(plan["created_new_node"], true);
    assert_eq!(plan["existing_primary_node_count"], 0);
    let target_effect_id = plan["target_effect_id"].as_u64().unwrap();
    assert_eq!(query_document(&core).tracks[0].clips[0].effects.len(), 0);

    let prepared = prepare_plan(&client, revision, plan["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    let committed = client
        .call_tool(commit_request(revision, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false));

    let after = query_document(&core);
    let effects = &after.tracks[0].clips[0].effects;
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].name, "primary_correction");
    assert_eq!(effects[0].id.0, target_effect_id);
    assert_eq!(
        effects[0].parameters["exposure_milli_stops"],
        ParamValue::Integer(500)
    );

    let after_context = invoke_capability(&client, "get_color_context", json!({})).await;
    assert_eq!(
        after_context.structured_content.as_ref().unwrap()["timeline_revision"]
            .as_u64()
            .unwrap(),
        revision + 1,
        "one committed plan must advance the revision exactly once"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC5 §7 / §9.2.15: `plan_secondary_correction` → `prepare_edit_plan` →
/// `commit_edit_plan` lands the exact `matte_*` parameters across the live
/// transport, and the plan itself applies nothing.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc5_secondary_plan_and_commit_land_the_matte_parameters() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let revision = context.structured_content.as_ref().unwrap()["timeline_revision"]
        .as_u64()
        .unwrap();
    assert_eq!(revision, 0);

    // A matte on a node that does not exist yet: the planner allocates it and
    // inserts it at the stage-legal index.
    let plan = invoke_capability(
        &client,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision,
            "clip_id": 1,
            "node_kind": "color_wheels",
            "windows": [{
                "shape": "ellipse",
                "center_x": 6_000,
                "center_y": 4_000,
                "half_width": 1_500,
                "half_height": 2_000,
                "feather": 1_200,
            }],
            "qualifier": {
                "saturation_low": 3_000,
                "saturation_high": 9_000,
            },
            "mix_basis_points": 7_500,
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false));
    let plan = plan
        .structured_content
        .as_ref()
        .expect("plan_secondary_correction must publish exact operations")
        .clone();
    assert_eq!(plan["applied"], false);
    assert_eq!(plan["evidence_only"], true);
    assert_eq!(plan["kind"], "color_wheels");
    assert_eq!(plan["created_new_node"], true);
    assert_eq!(plan["insert_index"], 0);
    let target_effect_id = plan["target_effect_id"].as_u64().unwrap();
    // Every requested integer is echoed back under its generated name.
    assert_eq!(plan["requested_parameters"]["matte_enabled"], 1);
    assert_eq!(plan["requested_parameters"]["matte_window_count"], 1);
    assert_eq!(plan["requested_parameters"]["matte_window0_shape_token"], 2);
    assert_eq!(
        plan["requested_parameters"]["matte_window0_feather_basis_points"],
        1_200
    );
    assert_eq!(plan["requested_parameters"]["matte_qualifier_enabled"], 1);
    assert_eq!(
        plan["requested_parameters"]["matte_mix_basis_points"],
        7_500
    );
    // The proposal's own matte, in the CC5 §7 manifest shape.
    assert_eq!(plan["matte"]["enabled"], true);
    assert_eq!(plan["matte"]["window_count"], 1);
    assert_eq!(plan["matte"]["combine"], "union");
    assert_eq!(plan["matte"]["mix_basis_points"], 7_500);
    assert_eq!(plan["matte"]["windows"].as_array().unwrap().len(), 1);
    assert_eq!(plan["matte"]["windows"][0]["shape"], "ellipse");
    // Nothing is applied by the plan itself.
    assert_eq!(query_document(&core).tracks[0].clips[0].effects.len(), 0);

    // A stale revision fails closed before anything is prepared.
    let stale = invoke_capability(
        &client,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision + 9,
            "clip_id": 1,
            "node_kind": "color_wheels",
            "windows": [{"center_x": 6_000}],
        }),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));

    // CC5 §2.1: a technical input transform cannot carry a matte.
    let technical = invoke_capability(
        &client,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision,
            "clip_id": 1,
            "node_kind": "technical_lut",
            "windows": [{"center_x": 6_000}],
        }),
    )
    .await;
    assert_eq!(technical.is_error, Some(true));
    assert_eq!(
        technical.structured_content.as_ref().unwrap()["code"],
        "matte_unsupported_node_kind"
    );

    let prepared = prepare_plan(&client, revision, plan["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    let committed = client
        .call_tool(commit_request(revision, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false));

    // The exact `matte_*` integers landed on the stored node, and nothing the
    // caller did not ask for was written.
    let after = query_document(&core);
    let effects = &after.tracks[0].clips[0].effects;
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].name, "color_wheels");
    assert_eq!(effects[0].id.0, target_effect_id);
    for (name, value) in [
        ("matte_enabled", 1),
        ("matte_window_count", 1),
        ("matte_mix_basis_points", 7_500),
        ("matte_qualifier_enabled", 1),
        ("matte_saturation_low_basis_points", 3_000),
        ("matte_saturation_high_basis_points", 9_000),
        ("matte_window0_shape_token", 2),
        ("matte_window0_center_x_basis_points", 6_000),
        ("matte_window0_center_y_basis_points", 4_000),
        ("matte_window0_half_width_basis_points", 1_500),
        ("matte_window0_half_height_basis_points", 2_000),
        ("matte_window0_feather_basis_points", 1_200),
    ] {
        assert_eq!(
            effects[0].parameters.get(name),
            Some(&ParamValue::Integer(value)),
            "committed node must carry {name} = {value}"
        );
    }
    // CC5 §2.2: an omitted control resolves to its neutral and is not stored.
    for name in [
        "matte_invert",
        "matte_combine_token",
        "matte_window0_invert",
        "matte_window0_rotation_centidegrees",
        "matte_window1_center_x_basis_points",
    ] {
        assert_eq!(
            effects[0].parameters.get(name),
            None,
            "a neutral control must not be stored"
        );
    }

    // The manifest now carries the CC5 §7 `matte` object.
    let after_context = invoke_capability(&client, "get_color_context", json!({})).await;
    let after_context = after_context.structured_content.as_ref().unwrap();
    assert_eq!(
        after_context["timeline_revision"].as_u64().unwrap(),
        revision + 1,
        "one committed plan must advance the revision exactly once"
    );
    let nodes = after_context["clips"][0]["color_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    let matte = &nodes[0]["matte"];
    assert_eq!(matte["enabled"], true);
    assert_eq!(matte["active"], true);
    assert_eq!(matte["window_count"], 1);
    assert_eq!(matte["mix_basis_points"], 7_500);
    assert_eq!(matte["qualifier"]["enabled"], true);
    assert_eq!(matte["qualifier"]["saturation_low_basis_points"], 3_000);
    assert_eq!(matte["qualifier"]["saturation_high_basis_points"], 9_000);
    // The hue leg stays at its 180 degree neutral, which disables it, so a
    // qualifier that names only saturation does not drop every grey pixel.
    assert_eq!(matte["qualifier"]["hue_leg_disabled"], true);
    let windows = matte["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["shape"], "ellipse");
    assert_eq!(windows[0]["center_x_basis_points"], 6_000);
    assert_eq!(windows[0]["feather_basis_points"], 1_200);

    // A node whose colour controls are all neutral is the exact identity, so
    // CC5 §2.6 reports it inactive and there is no coverage to inspect however
    // capable the renderer is. Give the node something to do first, so the
    // inspection below is a real measurement rather than a refusal the test
    // would have to accept either way.
    let wheels = invoke_capability(
        &client,
        "plan_color_wheels",
        json!({
            "expected_revision": revision + 1,
            "clip_id": 1,
            "parameters": {"gain_red_thousandths": 1_200},
        }),
    )
    .await;
    assert_eq!(wheels.is_error, Some(false));
    let wheels = wheels.structured_content.as_ref().unwrap().clone();
    assert_eq!(
        wheels["target_effect_id"].as_u64().unwrap(),
        target_effect_id,
        "the grade must land on the matted node, not on a second one"
    );
    let prepared = prepare_plan(&client, revision + 1, wheels["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    assert_eq!(
        client
            .call_tool(commit_request(revision + 1, &prepared))
            .await
            .unwrap()
            .is_error,
        Some(false)
    );

    // CC5 §7: the matte-scoped surfaces refuse honestly while this build's
    // renderer cannot proof a matte, rather than inventing coverage.
    let inspect = invoke_capability(
        &client,
        "inspect_grade_matte",
        json!({
            "expected_revision": revision + 2,
            "clip_id": 1,
            "effect_id": target_effect_id,
            "timecode": 0,
        }),
    )
    .await;
    let inspect_body = inspect.structured_content.as_ref().unwrap();
    if inspect.is_error == Some(true) {
        // A test that accepts both branches asserts nothing: a renderer that
        // silently stopped producing matte proofs would look exactly like a
        // green run. Refusing is a *skip*, and skipping is opt-in, on the same
        // workspace variable the media crate's GPU tests use.
        assert!(
            std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
                .ok()
                .as_deref()
                == Some("1"),
            "inspect_grade_matte refused: {inspect_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 \
             to accept an unavailable matte proof on a machine with no usable adapter."
        );
        assert_eq!(inspect_body["code"], "matte_proof_unavailable");
        assert_eq!(inspect_body["applied"], false);
        assert_eq!(inspect_body["details"]["observed"]["has_matte"], true);
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a matte proof; \
             inspect_grade_matte's measured statistics were not exercised."
        );
    } else {
        // Once the engine lands matte proofs, the statistics must describe the
        // same node the manifest just published.
        assert_eq!(inspect_body["effect_id"], target_effect_id);
        assert_eq!(inspect_body["kind"], "color_wheels");
        assert_eq!(
            inspect_body["matte_threshold"],
            "coverage_greater_than_zero"
        );
        let total = inspect_body["statistics"]["total_pixel_count"]
            .as_u64()
            .unwrap();
        let covered = inspect_body["statistics"]["covered_pixel_count"]
            .as_u64()
            .unwrap();
        // One ellipse well inside the frame plus a saturation band: the matte
        // must select a strict, non-empty subset of the frame. An empty matte
        // and a matte that degenerated to the whole frame both fail here.
        assert!(total > 0);
        assert!(covered > 0, "the matte covered nothing: {inspect_body}");
        assert!(
            covered < total,
            "the matte covered the whole frame: {inspect_body}"
        );
        assert_eq!(inspect_body["covered_pixel_count"], covered);
    }

    // Read-only either way: the committed revision did not move.
    assert_eq!(
        invoke_capability(&client, "get_color_context", json!({}))
            .await
            .structured_content
            .as_ref()
            .unwrap()["timeline_revision"]
            .as_u64()
            .unwrap(),
        revision + 2
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC3 §8/§10.3 fixture 11: `plan_color_wheels` is evidence-only over the real
/// transport, its exact operations survive prepare/commit, and the resulting
/// node is visible in the ordered `color_nodes` manifest afterwards.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc3_color_wheels_plan_and_commit_create_the_ordered_node() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    let revision = context["timeline_revision"].as_u64().unwrap();
    assert_eq!(revision, 0);
    assert_eq!(
        context["clips"][0]["color_nodes"].as_array().unwrap().len(),
        0
    );

    let plan = invoke_capability(
        &client,
        "plan_color_wheels",
        json!({
            "expected_revision": revision,
            "clip_id": 1,
            "parameters": {"gain_red_thousandths": 1_200, "lift_master_basis_points": -500}
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false));
    let plan = plan
        .structured_content
        .as_ref()
        .expect("plan_color_wheels must publish exact operations")
        .clone();
    assert_eq!(plan["applied"], false);
    assert_eq!(plan["evidence_only"], true);
    assert_eq!(plan["no_change"], false);
    assert_eq!(plan["created_new_node"], true);
    assert_eq!(plan["existing_color_node_count"], 0);
    assert_eq!(plan["kind"], "color_wheels");
    let target_effect_id = plan["target_effect_id"].as_u64().unwrap();
    assert_eq!(query_document(&core).tracks[0].clips[0].effects.len(), 0);

    // A stale revision fails closed before anything is prepared.
    let stale = invoke_capability(
        &client,
        "plan_color_wheels",
        json!({
            "expected_revision": revision + 9,
            "clip_id": 1,
            "parameters": {"gain_red_thousandths": 1_200}
        }),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));

    let prepared = prepare_plan(&client, revision, plan["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    let committed = client
        .call_tool(commit_request(revision, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false));

    let after = query_document(&core);
    let effects = &after.tracks[0].clips[0].effects;
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].name, "color_wheels");
    assert_eq!(effects[0].id.0, target_effect_id);
    assert_eq!(
        effects[0].parameters["gain_red_thousandths"],
        ParamValue::Integer(1_200)
    );
    assert_eq!(
        effects[0].parameters["lift_master_basis_points"],
        ParamValue::Integer(-500)
    );

    let after_context = invoke_capability(&client, "get_color_context", json!({})).await;
    let after_context = after_context.structured_content.as_ref().unwrap();
    assert_eq!(
        after_context["timeline_revision"].as_u64().unwrap(),
        revision + 1,
        "one committed plan must advance the revision exactly once"
    );
    let nodes = after_context["clips"][0]["color_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["stage_index"], 0);
    assert_eq!(nodes[0]["kind"], "color_wheels");
    assert_eq!(nodes[0]["effect_id"], target_effect_id);
    assert_eq!(nodes[0]["bypass"], 0);
    assert_eq!(nodes[0]["active"], true);
    assert_eq!(nodes[0]["inactive_reason"], json!(null));
    assert_eq!(nodes[0]["parameters"]["gain_red_thousandths"], 1_200);
    assert_eq!(nodes[0]["parameters"]["gamma_master_thousandths"], 1_000);

    // The same clip now takes a curves node in place beside the wheels node.
    let curves = invoke_capability(
        &client,
        "plan_color_curves",
        json!({
            "expected_revision": revision + 1,
            "clip_id": 1,
            "curves": {"master": [[0, 0], [5_000, 6_000], [10_000, 10_000]]}
        }),
    )
    .await;
    assert_eq!(curves.is_error, Some(false));
    let curves = curves.structured_content.as_ref().unwrap().clone();
    assert_eq!(curves["existing_color_node_count"], 1);
    assert_eq!(curves["created_new_node"], true);
    let prepared = prepare_plan(&client, revision + 1, curves["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    let committed = client
        .call_tool(commit_request(revision + 1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false));

    let final_context = invoke_capability(&client, "get_color_context", json!({})).await;
    let final_context = final_context.structured_content.as_ref().unwrap();
    let nodes = final_context["clips"][0]["color_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[1]["stage_index"], 1);
    assert_eq!(nodes[1]["kind"], "color_curves");
    assert_eq!(
        nodes[1]["curves"]["master"]["points"],
        json!([[0, 0], [5_000, 6_000], [10_000, 10_000]])
    );
    assert_eq!(nodes[1]["curves"]["master"]["truncated"], false);
    assert_eq!(nodes[1]["active"], true);

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC4 §8, §10.3.14: the look planner is evidence-only over the live
/// transport, binds to the analyzed revision, and lands the ordered node
/// through the ordinary prepare/commit path.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc4_creative_look_plan_and_commit_create_the_ordered_node() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    // A built-in generated look is `verified` from the binary's own bake, so
    // the store never has to be touched to prove the agent surface (CC4 §2.6).
    let warm = kinewright_media::BuiltinLook::Warm;
    let mut document = single_clip_document(asset);
    document.lut_assets = vec![warm.to_lut_asset(kinewright_core::LutAssetId(1))];
    document.tracks[0].clips[0].effects = vec![Effect {
        id: EffectId(1),
        name: "primary_correction".to_owned(),
        parameters: [("exposure_milli_stops".to_owned(), ParamValue::Integer(250))]
            .into_iter()
            .collect(),
        keyframes: std::collections::BTreeMap::default(),
    }];
    document.validate().expect("the seeded CC4 stack is valid");

    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    server.set_project_path(Some(std::env::temp_dir().join(format!(
        "kinewright-cc4-live-{}.kinewright",
        std::process::id()
    ))));
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let listed = invoke_capability(&client, "list_look_assets", json!({})).await;
    assert_eq!(listed.is_error, Some(false));
    let listed = listed.structured_content.as_ref().unwrap().clone();
    let revision = listed["timeline_revision"].as_u64().unwrap();
    assert_eq!(revision, 0);
    assert_eq!(listed["store_root_known"], true);
    assert_eq!(listed["assets"][0]["lut_asset_id"], 1);
    assert_eq!(listed["assets"][0]["sha256"], warm.pinned_sha256());
    assert_eq!(listed["assets"][0]["provenance"]["kind"], "builtin");
    assert_eq!(listed["assets"][0]["availability"]["kind"], "verified");
    assert_eq!(listed["assets"][0]["referenced_by"], json!([]));

    // A stale revision fails closed before anything is prepared.
    let stale = invoke_capability(
        &client,
        "plan_creative_look",
        json!({"expected_revision": revision + 9, "clip_id": 1, "lut_asset_id": 1}),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));

    let plan = invoke_capability(
        &client,
        "plan_creative_look",
        json!({
            "expected_revision": revision,
            "clip_id": 1,
            "lut_asset_id": 1,
            "mix_basis_points": 6_500
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false));
    let plan = plan
        .structured_content
        .as_ref()
        .expect("plan_creative_look must publish exact operations")
        .clone();
    assert_eq!(plan["applied"], false);
    assert_eq!(plan["evidence_only"], true);
    assert_eq!(plan["kind"], "creative_look");
    assert_eq!(plan["role"], "creative");
    assert_eq!(plan["color_stage"], "look");
    assert_eq!(plan["created_new_node"], true);
    // The clip already carries one correction node, so the look goes after it.
    assert_eq!(plan["insert_index"], 1);
    assert_eq!(plan["existing_color_node_count"], 1);
    assert_eq!(plan["lut_asset"]["title"], warm.title());
    assert_eq!(plan["lut_asset"]["sha256"], warm.pinned_sha256());
    assert_eq!(plan["lut_asset"]["availability"]["kind"], "verified");
    let target_effect_id = plan["target_effect_id"].as_u64().unwrap();
    assert_eq!(
        query_document(&core).tracks[0].clips[0].effects.len(),
        1,
        "planning must not apply anything"
    );

    let prepared = prepare_plan(&client, revision, plan["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    let committed = client
        .call_tool(commit_request(revision, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false));

    let after = query_document(&core);
    let effects = &after.tracks[0].clips[0].effects;
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0].name, "primary_correction");
    assert_eq!(effects[1].name, "creative_look");
    assert_eq!(effects[1].id.0, target_effect_id);
    assert_eq!(
        effects[1].parameters["lut_asset_id"],
        ParamValue::Integer(1)
    );
    assert_eq!(
        effects[1].parameters["mix_basis_points"],
        ParamValue::Integer(6_500)
    );

    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    assert_eq!(context["timeline_revision"].as_u64().unwrap(), revision + 1);
    let nodes = context["clips"][0]["color_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[1]["kind"], "creative_look");
    assert_eq!(nodes[1]["stage_index"], 1);
    assert_eq!(nodes[1]["lut_asset_id"], 1);
    assert_eq!(nodes[1]["lut_sha256"], warm.pinned_sha256());
    assert_eq!(nodes[1]["lut_availability"]["kind"], "verified");
    assert_eq!(nodes[1]["input_encoding"], "display709");
    assert_eq!(nodes[1]["mix_basis_points"], 6_500);
    assert_eq!(nodes[1]["active"], true);

    // The asset now reports the node that references it.
    let listed = invoke_capability(&client, "list_look_assets", json!({})).await;
    let listed = listed.structured_content.as_ref().unwrap();
    assert_eq!(
        listed["assets"][0]["referenced_by"],
        json!([{"clip_id": 1, "effect_id": target_effect_id}])
    );

    // `AddLutAsset` is unreachable through the plan path over the wire, and
    // the refusal names the one capability that can register a record.
    let refused = prepare_plan(
        &client,
        revision + 1,
        json!([{"add_lut_asset": {"asset": warm.to_lut_asset(kinewright_core::LutAssetId(2))}}]),
    )
    .await;
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(
        refused.content[0].as_text().unwrap().text,
        "edit plan contains an unsupported operation: AddLutAsset is only available through import_lut_asset, which parses, hashes, and stores the .cube bytes before registering the record"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC4 §8, §9: `convert_legacy_look` is the submittable form of the
/// `legacy_look_conversions` evidence, over the live transport.
///
/// The published batch opens with `AddLutAsset` whenever the built-in is not
/// registered yet, and `AddLutAsset` is refused on every plan path by design,
/// so before this capability existed the `ready` status named a batch no agent
/// could send.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc4_convert_legacy_look_submits_the_batch_the_evidence_publishes() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let mut document = single_clip_document(asset);
    let legacy_look = |id: u64, intensity: i64| Effect {
        id: EffectId(id),
        name: "look_lut".to_owned(),
        parameters: [
            ("preset_token".to_owned(), ParamValue::Integer(2)),
            (
                "intensity_percent".to_owned(),
                ParamValue::Integer(intensity),
            ),
        ]
        .into_iter()
        .collect(),
        keyframes: std::collections::BTreeMap::default(),
    };
    document.tracks[0].clips[0].effects = vec![legacy_look(1, 75), legacy_look(2, 100)];
    document
        .validate()
        .expect("the seeded legacy stack is valid");

    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap().clone();
    let revision = context["timeline_revision"].as_u64().unwrap();
    let conversion = &context["legacy_look_conversions"][0];
    assert_eq!(conversion["status"], "ready");
    assert_eq!(conversion["builtin_name"], "cool");
    assert_eq!(conversion["mix_basis_points"], 7_500);
    assert_eq!(conversion["operations"].as_array().unwrap().len(), 2);
    assert!(
        conversion["recovery_action"]
            .as_str()
            .unwrap()
            .contains("convert_legacy_look"),
        "{conversion}"
    );

    // The published batch is still refused on the plan path, which is exactly
    // why the capability exists.
    let refused = prepare_plan(&client, revision, conversion["operations"].clone()).await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        refused.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("import_lut_asset")
    );

    // A stale revision fails closed, structurally.
    let stale = invoke_capability(
        &client,
        "convert_legacy_look",
        json!({"expected_revision": revision + 9, "clip_id": 1, "effect_id": 1}),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));
    assert_eq!(
        stale.structured_content.as_ref().unwrap()["code"],
        "revision_conflict"
    );
    assert!(query_document(&core).lut_assets.is_empty());

    let converted = invoke_capability(
        &client,
        "convert_legacy_look",
        json!({"expected_revision": revision, "clip_id": 1, "effect_id": 1}),
    )
    .await;
    assert_eq!(converted.is_error, Some(false), "{:?}", converted.content);
    let converted = converted.structured_content.as_ref().unwrap();
    assert_eq!(converted["applied"], true);
    assert_eq!(converted["bit_identical_to_legacy"], false);
    assert_eq!(converted["conversion"]["source"], "builtin");
    assert_eq!(converted["conversion"]["reused_existing_asset"], false);
    assert_eq!(converted["timeline_revision"], revision + 1);
    assert_eq!(converted["lut_asset"]["availability"]["kind"], "verified");

    let after = query_document(&core);
    assert_eq!(after.lut_assets.len(), 1);
    assert_eq!(
        after.lut_assets[0].sha256,
        kinewright_media::BuiltinLook::Cool.pinned_sha256()
    );
    let effects = &after.tracks[0].clips[0].effects;
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0].name, "creative_look");
    assert_eq!(effects[0].id, EffectId(1));
    assert_eq!(
        effects[0].parameters["mix_basis_points"],
        ParamValue::Integer(7_500)
    );

    // Nothing new is blocked: with the asset registered, the second node's
    // batch is a lone `ConvertLegacyLook`, which the ordinary plan path still
    // accepts. Only `AddLutAsset` was ever refused there.
    let plain = prepare_plan(
        &client,
        revision + 1,
        json!([{"convert_legacy_look": {
            "clip": 1,
            "effect": 2,
            "lut_asset": 1,
            "mix_basis_points": 10_000
        }}]),
    )
    .await;
    assert_eq!(plain.is_error, Some(false), "{:?}", plain.content);
    let committed = client
        .call_tool(commit_request(revision + 1, &plain))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false));

    // Nothing is left to convert, and the tool refuses a managed node.
    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    assert_eq!(
        context["legacy_look_conversions"],
        json!([]),
        "the legacy stages are gone"
    );
    let again = invoke_capability(
        &client,
        "convert_legacy_look",
        json!({"expected_revision": revision + 2, "clip_id": 1, "effect_id": 1}),
    )
    .await;
    assert_eq!(again.is_error, Some(true));
    assert_eq!(
        again.structured_content.as_ref().unwrap()["code"],
        "not_a_legacy_look"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC4 §2.2, §8: a branch server started with the project session's
/// saved-project-path handle can resolve an imported asset's availability.
///
/// A branch started with a fresh `None` handle is store-blind on a saved
/// project: every imported asset reports `unknown_no_store`.
#[tokio::test(flavor = "multi_thread")]
async fn cc4_branch_server_with_the_project_path_handle_resolves_imported_availability() {
    let directory =
        std::env::temp_dir().join(format!("kinewright-cc4-branch-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let project = directory.join("branch.kinewright");
    let source = directory.join("warm.cube");
    std::fs::write(
        &source,
        kinewright_media::BuiltinLook::Warm.canonical_text(),
    )
    .unwrap();
    let store = kinewright_media::LutStore::for_project(&project).unwrap();
    let imported = store.import_lut_asset(&source).unwrap();

    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let mut document = single_clip_document(asset);
    document.lut_assets = vec![imported.into_lut_asset(kinewright_core::LutAssetId(1))];
    document.validate().expect("the imported record is valid");

    // The regression: a branch server with its own `None` handle cannot see
    // the store even though the project is saved.
    let blind_core = Core::spawn(document.clone()).unwrap();
    let blind = McpServer::start_isolated(blind_core, media.clone(), media.clone()).unwrap();
    let blind_client =
        ().serve(StreamableHttpClientTransport::from_uri(blind.endpoint()))
            .await
            .unwrap();
    let listed = invoke_capability(&blind_client, "list_look_assets", json!({})).await;
    let listed = listed.structured_content.as_ref().unwrap();
    assert_eq!(listed["store_root_known"], false);
    assert_eq!(
        listed["assets"][0]["availability"]["kind"],
        "unknown_no_store"
    );
    blind_client.cancel().await.unwrap();
    blind.shutdown();

    // Sharing the session's handle resolves it.
    let handle = Arc::new(std::sync::RwLock::new(Some(project.clone())));
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start_isolated_with_project_path(
        core,
        media.clone(),
        media,
        Arc::clone(&handle),
    )
    .unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let listed = invoke_capability(&client, "list_look_assets", json!({})).await;
    let listed = listed.structured_content.as_ref().unwrap();
    assert_eq!(listed["store_root_known"], true);
    assert_eq!(listed["assets"][0]["availability"]["kind"], "verified");
    assert_eq!(
        listed["assets"][0]["recovery_action"],
        json!(null),
        "a verified asset needs no recovery"
    );

    // The handle is shared, so a later Save As reaches the branch with no
    // republishing.
    *handle.write().unwrap() = None;
    let listed = invoke_capability(&client, "list_look_assets", json!({})).await;
    assert_eq!(
        listed.structured_content.as_ref().unwrap()["store_root_known"],
        false
    );

    client.cancel().await.unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC4 §8: a LUT node is proofed by the real managed renderer, which refuses
/// with a typed `missing_lut_asset` when the asset's bytes are not published
/// rather than rendering a look-free frame.
///
/// The asset is deliberately *imported*: a built-in is baked in this binary and
/// resolves from the document alone, so only an imported asset — whose bytes
/// live in the project store the application publishes — can still be
/// unresolvable here. The agent server never publishes them, so this is the
/// honest failure an agent sees, and it is a render-stage refusal rather than a
/// pre-render short circuit.
#[tokio::test(flavor = "multi_thread")]
async fn cc4_render_color_proof_reports_the_unpublished_lut_asset_from_the_real_renderer() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let warm = kinewright_media::BuiltinLook::Warm;
    // Valid, self-consistent metadata from a real bake, recorded as an
    // imported asset so its bytes have to come from a store nobody published.
    let mut unpublished = warm.to_lut_asset(kinewright_core::LutAssetId(1));
    unpublished.title = "Unpublished look".to_owned();
    unpublished.source = kinewright_core::LutAssetSource::Imported {
        source_path: "/looks/unpublished.cube".to_owned(),
    };
    let mut document = single_clip_document(asset);
    document.lut_assets = vec![unpublished];
    document.tracks[0].clips[0].effects = vec![Effect {
        id: EffectId(1),
        name: "creative_look".to_owned(),
        parameters: [("lut_asset_id".to_owned(), ParamValue::Integer(1))]
            .into_iter()
            .collect(),
        keyframes: std::collections::BTreeMap::default(),
    }];
    document
        .validate()
        .expect("the CC4 stack is a valid document");

    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core, media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let refused = invoke_capability(
        &client,
        "render_color_proof",
        json!({
            "expected_revision": 0,
            "clip_id": 1,
            "timecode": 5,
            "effect_id": 1,
            "look_comparison": "after"
        }),
    )
    .await;
    assert_eq!(refused.is_error, Some(true));
    let structured = refused.structured_content.as_ref().unwrap();
    // CC4 §2.3, §8: the refusal is typed and names the asset, not a prose
    // `render_failed` message an agent would have to parse.
    assert_eq!(structured["code"], "missing_lut_asset");
    let details = &structured["details"];
    assert_eq!(details["field"], "lut_asset_id");
    assert_eq!(details["observed"], 1);
    assert_eq!(details["lut_asset_id"], 1);
    assert_eq!(details["effect_id"], 1);
    assert_eq!(details["lut_title"], "Unpublished look");
    assert_eq!(details["lut_sha256"], warm.pinned_sha256());
    assert!(
        details["allowed"].is_string(),
        "the refusal names what would have been accepted: {details}"
    );
    assert!(
        details["recovery_action"]
            .as_str()
            .is_some_and(|action| action.contains("list_look_assets")),
        "{details}"
    );
    // The BEFORE cell removes the node, so only the AFTER render needs the
    // asset. That ordering is the document's, never the adapter's.
    assert_eq!(details["stage"], "after");
    assert!(
        details["availability"].is_object(),
        "the live availability travels with the refusal: {details}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn cc2_scope_tools_are_read_only_over_the_live_endpoint() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(two_shot_color_document(&asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let before = query_document(&core);
    let revision = invoke_capability(&client, "get_color_context", json!({}))
        .await
        .structured_content
        .as_ref()
        .unwrap()["timeline_revision"]
        .as_u64()
        .unwrap();

    let scopes = invoke_capability(
        &client,
        "get_video_scopes_v2",
        json!({"expected_revision": revision, "timecode": 5}),
    )
    .await;
    assert_eq!(scopes.is_error, Some(false));
    let scopes = scopes.structured_content.as_ref().unwrap();
    assert_eq!(scopes["full_resolution"], true);
    assert_eq!(scopes["stage"], "monitoring_post_composite");
    // The typed core evidence is the single source of truth for grids.
    assert!(scopes["core_evidence"]["waveform"].is_object());
    assert!(scopes.get("waveform").is_none());

    let analysis = invoke_capability(
        &client,
        "analyze_color_shot",
        json!({"expected_revision": revision, "clip_id": 1}),
    )
    .await;
    assert_eq!(analysis.is_error, Some(false));
    let analysis = analysis.structured_content.as_ref().unwrap();
    assert_eq!(analysis["applied"], false);
    assert_eq!(analysis["full_resolution"], true);
    assert_eq!(analysis["grids_omitted"], true);
    assert!(
        serde_json::to_vec(analysis).unwrap().len() < 20_000,
        "analyze_color_shot must stay compact by default"
    );

    let matched = invoke_capability(
        &client,
        "plan_shot_match",
        json!({
            "expected_revision": revision,
            "reference_clip_id": 1,
            "candidate_clip_ids": [2]
        }),
    )
    .await;
    assert_eq!(matched.is_error, Some(false));
    let matched = matched.structured_content.as_ref().unwrap();
    assert_eq!(matched["applied"], false);
    assert_eq!(matched["full_resolution"], true);
    assert_eq!(matched["candidate_limit"], 16);
    assert!(matched["editable_operations"].is_array());

    assert_eq!(
        query_document(&core),
        before,
        "CC2 evidence tools must never mutate the timeline"
    );
    assert_eq!(
        invoke_capability(&client, "get_color_context", json!({}))
            .await
            .structured_content
            .as_ref()
            .unwrap()["timeline_revision"]
            .as_u64()
            .unwrap(),
        revision,
        "read-only scope tools must leave the revision unchanged"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC6 §11.2.18: `get_color_qc` measures the working stage, publishes evidence
/// only, is revision-gated without *requiring* a revision, refuses a skin check
/// with no region, refuses a frame the project does not have, and offers no
/// resolution knob of any spelling.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc6_get_color_qc_is_evidence_only_and_revision_gated() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    // The project is exactly the asset's frames, so `duration` below is the
    // half-open project range `get_color_qc` has to enforce.
    let duration = asset.duration.0;
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let before = query_document(&core);
    let revision = invoke_capability(&client, "get_color_context", json!({}))
        .await
        .structured_content
        .as_ref()
        .unwrap()["timeline_revision"]
        .as_u64()
        .unwrap();

    // CC6 R13: the published schema carries no `resolution`, `proxy_sampling`,
    // or `max_width`. A working-stage measurement is full-resolution or it is
    // refused, so there is nothing for a caller to turn down.
    let opened = client
        .call_tool(
            CallToolRequestParams::new("get_capability")
                .with_arguments(json!({"name": "get_color_qc"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(opened.is_error, Some(false));
    let opened = opened.structured_content.as_ref().unwrap();
    assert_eq!(opened["capability"]["kind"], "inspector");
    let properties = opened["input_schema"]["properties"].as_object().unwrap();
    for absent in ["resolution", "proxy_sampling", "max_width"] {
        assert!(
            !properties.contains_key(absent),
            "get_color_qc must not carry {absent}: {opened}"
        );
    }
    for present in ["roi", "matte_region", "checks", "delivery_bit_depth"] {
        assert!(properties.contains_key(present), "missing {present}");
    }

    // A stale revision is the uniform envelope, refused before any render.
    let stale = invoke_capability(
        &client,
        "get_color_qc",
        json!({"expected_revision": revision + 7, "timecode": 5}),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));
    let stale_body = stale.structured_content.as_ref().unwrap();
    assert_eq!(stale_body["code"], "stale_revision");
    assert_eq!(stale_body["applied"], false);
    assert_eq!(stale_body["evidence_only"], true);
    assert_eq!(stale_body["details"]["expected_revision"], revision + 7);
    assert_eq!(stale_body["details"]["actual_revision"], revision);

    // CC6 §3.5: skin is a diagnostic of a region the operator chose, so it is
    // refused without one, before any render.
    let unscoped_skin = invoke_capability(
        &client,
        "get_color_qc",
        json!({"timecode": 5, "checks": ["skin"]}),
    )
    .await;
    assert_eq!(unscoped_skin.is_error, Some(true));
    let unscoped_body = unscoped_skin.structured_content.as_ref().unwrap();
    assert_eq!(unscoped_body["code"], "color_qc_region_required");
    assert_eq!(unscoped_body["details"]["field"], "checks");

    // CC6 §7 / errata E32: a frame the project does not have is refused, not
    // measured. Both directions, before any render: the compositor would
    // happily return its cleared target and the report would read as a clean
    // legal-range pass over opaque black that no export will ever contain.
    assert!(duration > 0);
    for (field, request) in [
        ("timecode", json!({"timecode": -1})),
        ("timecode", json!({"timecode": duration})),
        ("frame", json!({"frame": duration + 1_000})),
    ] {
        let refused = invoke_capability(&client, "get_color_qc", request.clone()).await;
        assert_eq!(refused.is_error, Some(true), "{request} must be refused");
        let body = refused.structured_content.as_ref().unwrap();
        assert_eq!(body["code"], "color_qc_frame_out_of_range", "{body}");
        assert_eq!(body["applied"], false);
        assert_eq!(body["evidence_only"], true);
        assert_eq!(body["details"]["field"], field, "{body}");
        assert_eq!(body["details"]["allowed"], format!("0..{duration}"));
    }
    // The last frame the project has is inside the half-open range, so the
    // guard cannot be passing by refusing everything.
    let last = invoke_capability(&client, "get_color_qc", json!({"timecode": duration - 1})).await;
    assert_ne!(
        last.structured_content.as_ref().unwrap()["code"],
        json!("color_qc_frame_out_of_range"),
        "the last project frame must not be refused as out of range"
    );

    // CC6 §7: `max_nodes` is validated on every call, not only when `per_node`
    // is asked for - an out-of-range budget is a malformed request whether or
    // not this call would have spent it. Refused before any render.
    for budget in [0, 17] {
        let refused = invoke_capability(
            &client,
            "get_color_qc",
            json!({"timecode": 5, "max_nodes": budget}),
        )
        .await;
        assert_eq!(refused.is_error, Some(true), "max_nodes={budget}");
        let body = refused.structured_content.as_ref().unwrap();
        assert_eq!(body["code"], "color_qc_node_budget_exceeded", "{body}");
        assert_eq!(body["details"]["field"], "max_nodes");
        assert_eq!(body["details"]["observed"], budget.to_string());
    }

    // The measurement itself. `expected_revision` is deliberately absent: this
    // is an inspector, not a planner.
    let report = invoke_capability(&client, "get_color_qc", json!({"timecode": 5})).await;
    let body = report.structured_content.as_ref().unwrap();
    if report.is_error == Some(true) {
        // A test that accepts both branches asserts nothing: a renderer that
        // silently stopped producing working proofs would look exactly like a
        // green run. Refusing is a *skip*, and skipping is opt-in.
        assert!(
            std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
                .ok()
                .as_deref()
                == Some("1"),
            "get_color_qc refused: {body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to accept an \
             unavailable working proof on a machine with no usable adapter."
        );
        // Even the skip branch asserts a typed code, so a refusal for the
        // wrong reason still fails.
        assert_eq!(body["code"], "working_proof_unavailable");
        assert_eq!(body["applied"], false);
        assert_eq!(body["evidence_only"], true);
        assert_eq!(body["details"]["field"], "working_proof");
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a working \
             proof; get_color_qc's measured report was not exercised."
        );
    } else {
        // The human-readable line reads the envelope's typed values rather
        // than `Value`'s Display, so the stage is not quoted and the frame is
        // not a JSON number rendering.
        let text = report.content[0].as_text().unwrap().text.clone();
        assert!(
            text.contains("stage=working_linear_post_composite,"),
            "the stage must not arrive quoted: {text}"
        );
        assert!(text.contains("project frame 5;"), "{text}");
        assert!(!text.contains('"'), "{text}");

        assert_eq!(body["evidence_only"], true);
        assert_eq!(body["applied"], false);
        assert_eq!(body["stage"], "working_linear_post_composite");
        assert_eq!(body["full_resolution"], true);
        assert_eq!(body["timeline_revision"], revision);
        let qc = &body["report"];
        assert_eq!(qc["stage"], "working_linear_post_composite");
        assert_eq!(qc["full_resolution"], true);
        assert_eq!(qc["evidence_only"], true);
        assert_eq!(qc["project_frame"], 5);
        assert_eq!(qc["delivery_bit_depth"], 8);
        // §3.1: the composite target is opaque by construction at this stage.
        assert_eq!(qc["transparent_pixel_count"], 0);
        // Exact, not merely self-consistent: an unscoped measurement is every
        // pixel of the 320x180 raster, and alpha is 1 everywhere at this
        // stage, so both counts are the full raster and neither can drift
        // without failing here.
        assert_eq!(qc["raster"], json!([320, 180]));
        assert_eq!(qc["visible_pixel_count"], json!(320 * 180));
        assert_eq!(qc["region"]["region_pixel_count"], json!(320 * 180));
        // The default checks produce range, gamut, and a pre-export tag check,
        // and never the optional sections.
        assert!(qc["range"].is_object());
        assert!(qc["gamut"].is_object());
        assert_eq!(
            qc["tags"]["tag_source"], "materialised_export_settings",
            "get_color_qc is always pre-export tag mode: {qc}"
        );
        assert!(
            qc["tags"]["not_representable"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(qc["tags"]["conforming"], true);
        assert_eq!(qc["skin"], json!(null));
        assert_eq!(qc["nodes"], json!(null));
        // The default `checks` publish exactly six assumptions: the four that
        // hold for every measurement, the pre-export tag note, and the
        // evidence-only boundary. The skin and per-node notes are absent
        // because those checks did not run - a `>= 4` bound would pass even if
        // the tool started describing a skin population it never measured.
        let assumptions = body["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|assumption| assumption.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(assumptions.len(), 6, "{assumptions:#?}");
        assert!(assumptions[0].starts_with("Measured at working_linear_post_composite"));
        assert!(assumptions[1].starts_with("Always full resolution."));
        assert!(assumptions[2].contains("alpha is 1 everywhere"));
        assert!(
            assumptions[3].contains("eight lane (8 bits)"),
            "{assumptions:#?}"
        );
        assert!(assumptions[4].contains("pre-export mode"));
        assert!(assumptions[5].starts_with("Evidence only."));
        assert!(
            !assumptions
                .iter()
                .any(|assumption| assumption.contains("Per-node attribution")),
            "per_node is never a default: {assumptions:#?}"
        );
        assert!(
            !assumptions
                .iter()
                .any(|assumption| assumption.contains("skin")
                    && !assumption.contains("pre-export mode")),
            "no skin assumption without a skin check: {assumptions:#?}"
        );
        assert!(body["exceptions"].is_array());
        assert_eq!(qc["provenance"]["engine"], "kinewright_color_qc_v1");

        // A region makes the skin check legal, and it measures the region the
        // caller named rather than the whole raster.
        let scoped = invoke_capability(
            &client,
            "get_color_qc",
            json!({
                "timecode": 5,
                "checks": ["range", "gamut", "skin"],
                "roi": {
                    "x_basis_points": 2_500,
                    "y_basis_points": 2_500,
                    "width_basis_points": 5_000,
                    "height_basis_points": 5_000
                }
            }),
        )
        .await;
        assert_eq!(
            scoped.is_error,
            Some(false),
            "{:?}",
            scoped.structured_content
        );
        let scoped = scoped.structured_content.as_ref().unwrap();
        assert!(scoped["report"]["skin"].is_object());
        assert_eq!(scoped["report"]["tags"], json!(null));
        // 2500..7500 basis points of 320x180 is x 80..240 and y 45..135 by
        // CC2's floor/ceil rule: exactly 160 x 90 pixels, not merely "fewer".
        assert_eq!(scoped["report"]["visible_pixel_count"], json!(160 * 90));
        assert_eq!(
            scoped["report"]["region"]["region_pixel_count"],
            json!(160 * 90)
        );
        // Dropping `tags` and adding `skin` swaps exactly one assumption.
        let scoped_assumptions = scoped["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|assumption| assumption.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(scoped_assumptions.len(), 6, "{scoped_assumptions:#?}");
        assert!(
            !scoped_assumptions
                .iter()
                .any(|assumption| assumption.contains("pre-export mode")),
            "no tag assumption without a tag check: {scoped_assumptions:#?}"
        );
        assert_eq!(
            scoped_assumptions[4],
            kinewright_core::SKIN_DIAGNOSTIC_BOUNDARY
        );
    }

    // Whatever branch ran, nothing moved.
    assert_eq!(
        query_document(&core),
        before,
        "get_color_qc must never mutate the timeline"
    );
    assert_eq!(
        invoke_capability(&client, "get_color_context", json!({}))
            .await
            .structured_content
            .as_ref()
            .unwrap()["timeline_revision"]
            .as_u64()
            .unwrap(),
        revision,
        "an evidence-only measurement must leave the revision unchanged"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC6 §11.2.16 (agent half): `get_video_scopes_v2` publishes a typed pointer
/// at `get_color_qc` where it used to publish a fabricated zero, and the
/// working stage is refused by the CC2 scope engine.
#[tokio::test(flavor = "multi_thread")]
async fn cc6_video_scopes_v2_points_at_get_color_qc_instead_of_a_fabricated_zero() {
    let generated = managed_color_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let scopes = invoke_capability(&client, "get_video_scopes_v2", json!({"timecode": 5})).await;
    assert_eq!(
        scopes.is_error,
        Some(false),
        "{:?}",
        scopes.structured_content
    );
    let scopes = scopes.structured_content.as_ref().unwrap();
    assert_eq!(
        scopes["provenance"]["stage_measured"],
        "monitoring_post_composite"
    );
    let gamut = &scopes["gamut"];
    assert_eq!(gamut["measured"], false);
    assert_eq!(gamut["code"], "gamut_requires_working_stage");
    assert_eq!(gamut["stage_required"], "working_linear_post_composite");
    assert_eq!(gamut["tool"], "get_color_qc");
    assert!(
        gamut["definition"]
            .as_str()
            .is_some_and(|definition| definition.contains("display-clamped")),
        "{gamut}"
    );
    // The fabricated zero is the actual defect: it reads as "measured, none
    // found". Both keys must be absent, not zero.
    let gamut = gamut.as_object().unwrap();
    assert!(!gamut.contains_key("out_of_range_pixels"));
    assert!(!gamut.contains_key("out_of_range_basis_points"));

    // CC6 §2.1: the working stage is a real name in one shared vocabulary, and
    // the CC2 scope engine fails closed on it rather than falling back to
    // monitoring evidence.
    let working = invoke_capability(
        &client,
        "get_video_scopes_v2",
        json!({"stage": "working_linear_post_composite", "timecode": 5}),
    )
    .await;
    assert_eq!(working.is_error, Some(true));
    let working = working.structured_content.as_ref().unwrap();
    assert_eq!(working["code"], "unsupported_stage");
    assert_eq!(working["applied"], false);
    assert_eq!(working["details"]["stage"], "working_linear_post_composite");
    assert_eq!(
        working["details"]["supported_stages"][0],
        "monitoring_post_composite"
    );

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
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
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
        lut_assets: Vec::new(),
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

// ===========================================================================
// CC7 §5 — the six scripted agent end-to-end tests.
//
// One `cc7_` test per scenario, driving the *real* MCP endpoint over
// `McpServer::start` + `StreamableHttpClientTransport` with scripted tool
// calls. There is no LLM here and no `AgentDriver`: every number these tests
// assert comes from `kinewright_core::cc7_scenarios` (the scenario authority,
// CC7 §2) or from `kinewright_media::cc7_sources` (the one raster generator,
// CC7 §3), never from a literal restated at this call site.
//
// CC7 §5.1's uniform assertions run in every one of the six:
//   1. every planner/inspector response carries `evidence_only: true` and
//      `applied: false`, and the document is unchanged after planning;
//   2. a stale `expected_revision` returns the typed `stale_revision`;
//   3. one commit advances `timeline_revision` exactly once;
//   4. the committed document EQUALS `cc7_canonical_operations` applied to the
//      same base document — a regression pin on `match_parameters`, whose
//      values were measured by an independent f64 transcription (R-M8);
//   5. the same integers are re-read from `get_color_context`'s `color_nodes`.
// ===========================================================================

use kinewright_core::{
    NormalizedRoi, Operation, SCOPE_BASIS_POINTS, apply_batch,
    cc7_scenarios::{
        CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS, CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED,
        CC7_C2_OVER_RANGE_PIXELS_REPORTED, CC7_CANDIDATE_CLIP_ID, CC7_CHART_BAND_ROI,
        CC7_DEEP_SHADOW_RECT, CC7_DEEP_SHADOW_ROI, CC7_F_KEYFRAMED_PARAMETERS, CC7_LOG_CUBE_SIZE,
        CC7_LOG_FIRST_PERCENTILE_MIN_CODE16, CC7_LOG_P99_MAX_CODE16,
        CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS, CC7_LOOK_MIX_BASIS_POINTS, CC7_LUT_ASSET_ID,
        CC7_MATCH_PROPOSAL_B, CC7_MATCH_PROPOSAL_C1, CC7_MATCH_PROPOSAL_C2,
        CC7_PRODUCT_PATCH_PIXEL_COUNT, CC7_PRODUCT_RED_ROI, CC7_REFERENCE_CLIP_ID,
        CC7_SECONDARY_SATURATION_PERCENT, CC7_SINGLE_CLIP_ID, CC7_SKIN_BAND_ROI,
        CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS, CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS,
        CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES, CC7_TRACK_F2_SAMPLE_FRAMES,
        CC7_TRACK_F2_STEP_FRAMES, CC7_TRACK_MAX_WIDTH, CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS, CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS,
        CC7_TRACK_OCCLUDED_CONFIDENCE_ON_THE_RECIPE_REPORTED, CC7_TRACK_RANGE_END_LOCAL_FRAME,
        CC7_TRACK_RANGE_START_LOCAL_FRAME, CC7_TRACK_SEARCH_RADIUS_PERCENT,
        CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS,
        CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS, CC7_TRACK_STEP_FRAMES,
        CC7_TRACK_SURVIVING_SAMPLE_FRAMES, CC7_TRACK_TOLERANCE_BASIS_POINTS, Cc7Camera,
        Cc7Scenario, cc7_canonical_operations, cc7_lut_backed_canonical_operations,
        cc7_track_keyframe_centres, cc7_tracking_sample_frames,
    },
};
use kinewright_media::cc7_sources::{
    cc7_camera_source, cc7_log_source, cc7_tracked_source, write_log_like_inverse_cube,
};

/// The default `MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS` (`server.rs:11192-11204`,
/// `pub(crate)` in the agent crate and therefore unreachable from an
/// integration test). Restated here with its owner, exactly as CC7 §2.7's
/// R-M2 transcription rule allows, because CC7 §4(f)(1) asserts the CC7 floor
/// is **not** this number and that this number drops nothing.
const CC7_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS: i64 = 5_000;

/// `MATTE_TRACK_MINIMUM_SAMPLES` (`server.rs:11204`), the number of surviving
/// observations `track_matte_window` needs before it will publish a curve.
/// Private to the agent crate, so — like the floor above — it is restated here
/// with its owner rather than written as a bare literal inside the (f2) gate.
const CC7_MATTE_TRACK_MINIMUM_SAMPLES: i64 = 2;

/// The descriptor bound one CC1 `primary_correction` control offers, which is
/// exactly what `primary_parameter_bounds` (`color_scopes.rs:1786-1794`) reads
/// and therefore what `proposal_details[..].{min,max}` must publish. CC7 §4(b)(2)
/// asks for `-100 / 100` on `temperature_percent`; taking it from the
/// descriptor rather than from the clamped value means a descriptor change and
/// a clamp change cannot move together and cancel (R2 minor 1/2).
fn cc7_primary_bounds(name: &str) -> (i64, i64) {
    let parameter = kinewright_core::effect_descriptor("primary_correction")
        .and_then(|descriptor| descriptor.parameter(name))
        .unwrap_or_else(|| panic!("{name} is a registered primary_correction control"));
    (parameter.min, parameter.max)
}

/// CC7 §2.3.4: two clips on one video track referencing **two distinct
/// encodes**, never one asset split in half. `two_shot_color_document`
/// (`:489`) is colorimetrically vacuous and CC7 does not reuse it.
fn cc7_two_clip_document(reference: MediaAsset, candidate: MediaAsset) -> Document {
    let reference_duration = reference.duration;
    let candidate_duration = candidate.duration;
    let mut document = single_clip_document(reference);
    let mut second = document.tracks[0].clips[0].clone();
    second.id = CC7_CANDIDATE_CLIP_ID;
    second.asset = candidate.id;
    second.source_range = TimeCode::ZERO..candidate_duration;
    second.timeline_start = reference_duration;
    document.tracks[0].clips.push(second);
    document.media_pool.push(candidate);
    document.duration = TimeCode(reference_duration.0 + candidate_duration.0);
    document
        .validate()
        .expect("the CC7 two-clip document is valid");
    document
}

/// CC7 §5.1(4): the canonical document is `operations` applied to the same
/// base the live server started from, by Core's own `apply_batch`.
fn cc7_canonical_document(base: &Document, operations: &[Operation]) -> Document {
    let mut expected = base.clone();
    apply_batch(&mut expected, operations)
        .expect("the canonical batch is accepted by core in order");
    expected
}

/// CC7 errata D-E1: `plan_primary_correction` emits `AddEffect` carrying
/// **all ten** non-matte CC1 controls at their descriptor neutrals
/// (`color_status.rs:1529-1540`, `:1598-1610`) and only then `SetEffectParam`
/// for the ones it moved, so a node the CC1/CC2 planners create stores seven
/// neutral controls §2.5's `InsertEffect` canonical batch does not — and it is
/// an `AddEffect`, not an `InsertEffect`. CC7 §5.1(4)'s equality is therefore
/// taken against the canonical document with exactly those descriptor neutrals
/// filled in. Nothing else is forgiven: every parameter the canonical batch
/// names still has to match exactly, and a stored value that is *not* the
/// descriptor's neutral still fails the comparison.
fn cc7_with_cc1_neutral_fill(mut document: Document, clip: ClipId) -> Document {
    let descriptor = kinewright_core::effect_descriptor("primary_correction")
        .expect("primary_correction is a registered effect");
    for track in &mut document.tracks {
        for target in track.clips.iter_mut().filter(|target| target.id == clip) {
            for effect in target
                .effects
                .iter_mut()
                .filter(|effect| effect.name == "primary_correction")
            {
                for parameter in descriptor
                    .parameters
                    .iter()
                    .filter(|parameter| !kinewright_core::is_matte_parameter(parameter.name))
                {
                    effect
                        .parameters
                        .entry(parameter.name.to_owned())
                        .or_insert(ParamValue::Integer(parameter.neutral));
                }
            }
        }
    }
    document
}

/// The current `timeline_revision`, read through `get_color_context`.
async fn cc7_revision(client: &RunningService<RoleClient, ()>) -> u64 {
    invoke_capability(client, "get_color_context", json!({}))
        .await
        .structured_content
        .as_ref()
        .expect("get_color_context publishes machine-readable status")["timeline_revision"]
        .as_u64()
        .unwrap()
}

/// CC7 §5.1(1): evidence-only, and nothing applied.
fn cc7_assert_evidence_only(body: &serde_json::Value, tool: &str) {
    assert_eq!(body["applied"], false, "{tool} must apply nothing: {body}");
    assert_eq!(
        body["evidence_only"], true,
        "{tool} must publish evidence_only: {body}"
    );
}

/// CC7 §5.1(2): a stale `expected_revision` is the typed refusal, before any
/// render.
async fn cc7_assert_stale_revision(
    client: &RunningService<RoleClient, ()>,
    tool: &str,
    arguments: serde_json::Value,
    revision: u64,
    stale: u64,
) {
    let refused = invoke_capability(client, tool, arguments).await;
    assert_eq!(
        refused.is_error,
        Some(true),
        "{tool} must refuse a stale revision"
    );
    let body = refused
        .structured_content
        .as_ref()
        .expect("a typed refusal carries structured content");
    assert_eq!(body["code"], "stale_revision", "{body}");
    assert_eq!(body["applied"], false, "{body}");
    assert_eq!(body["details"]["expected_revision"], stale, "{body}");
    assert_eq!(body["details"]["actual_revision"], revision, "{body}");
}

/// CC7 errata D-E2: the **CC4/CC5** planners (`plan_technical_lut`,
/// `plan_creative_look`, `plan_secondary_correction`, `track_matte_window`)
/// publish a revision conflict as `revision_conflict_text`
/// (`server.rs:13255-13259`), which is `error_text` — prose, no structured
/// body and no `code`. Only CC2's scope planners and `get_color_qc` publish
/// the typed `stale_revision` §5.1(2) names. This helper asserts the strongest
/// claim those tools actually make: `is_error == true`, both revisions named
/// in the message, and nothing applied.
async fn cc7_assert_stale_revision_prose(
    client: &RunningService<RoleClient, ()>,
    core: &Core,
    tool: &str,
    arguments: serde_json::Value,
    revision: u64,
    stale: u64,
) {
    let before = query_document(core);
    let refused = invoke_capability(client, tool, arguments).await;
    assert_eq!(
        refused.is_error,
        Some(true),
        "{tool} must refuse a stale revision"
    );
    let text = refused.content[0].as_text().unwrap().text.clone();
    assert!(
        text.contains("timeline revision conflict")
            && text.contains(&format!("expected {stale}"))
            && text.contains(&format!("actual {revision}")),
        "{tool} must name both revisions: {text}"
    );
    assert_eq!(&query_document(core), &before, "a refusal changes nothing");
}

/// A `plan_shot_match` / `analyze_color_shot` ROI, which is normalized
/// `0..=1` floats rather than basis points (`color_scopes.rs:175-191`).
fn cc7_scope_roi(roi: NormalizedRoi) -> serde_json::Value {
    let scale = f64::from(SCOPE_BASIS_POINTS);
    json!({
        "x": f64::from(roi.x_basis_points) / scale,
        "y": f64::from(roi.y_basis_points) / scale,
        "width": f64::from(roi.width_basis_points) / scale,
        "height": f64::from(roi.height_basis_points) / scale,
    })
}

/// A `get_color_qc` ROI, which *is* basis points (`color_qc_tool.rs`).
fn cc7_qc_roi(roi: NormalizedRoi) -> serde_json::Value {
    json!({
        "x_basis_points": roi.x_basis_points,
        "y_basis_points": roi.y_basis_points,
        "width_basis_points": roi.width_basis_points,
        "height_basis_points": roi.height_basis_points,
    })
}

/// The approval loop's own bookkeeping, read back on the test's thread.
///
/// A panic inside a `tokio::spawn`ed task whose `JoinHandle` is only ever
/// `abort()`ed is swallowed, so the loop asserts nothing itself: it *counts*
/// what it approved and what it refused to recognise, and the test asserts
/// both after the awaited call returns (R2 minor 7).
struct Cc7Approvals {
    task: tokio::task::JoinHandle<()>,
    approved: Arc<std::sync::atomic::AtomicUsize>,
    foreign: Arc<std::sync::atomic::AtomicUsize>,
}

impl Cc7Approvals {
    /// Assert the confirmation really was raised and approved — and that no
    /// *other* tool asked for one — then stop the loop.
    fn assert_approved_and_stop(&self, tool_name: &str) {
        use std::sync::atomic::Ordering;
        assert_eq!(
            self.foreign.load(Ordering::SeqCst),
            0,
            "only {tool_name} may raise a confirmation in this scenario"
        );
        assert!(
            self.approved.load(Ordering::SeqCst) >= 1,
            "{tool_name} must have raised a confirmation that this test approved"
        );
        self.task.abort();
    }
}

/// Approve every destructive-tool confirmation this scenario raises until the
/// task is aborted. `import_lut_asset` blocks on the broker
/// (`server.rs:1912`), so the approval has to run beside the awaited call.
fn cc7_approve_confirmations(
    broker: kinewright_agent::ConfirmationBroker,
    tool_name: &'static str,
) -> Cc7Approvals {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let approved = Arc::new(AtomicUsize::new(0));
    let foreign = Arc::new(AtomicUsize::new(0));
    let approved_counter = Arc::clone(&approved);
    let foreign_counter = Arc::clone(&foreign);
    let task = tokio::spawn(async move {
        loop {
            for request in broker.pending_requests() {
                if request.tool_name == tool_name {
                    if broker.approve(request.id) {
                        approved_counter.fetch_add(1, Ordering::SeqCst);
                    }
                } else {
                    foreign_counter.fetch_add(1, Ordering::SeqCst);
                    let _ = broker.approve(request.id);
                }
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    });
    Cc7Approvals {
        task,
        approved,
        foreign,
    }
}

/// CC7 §5.1(3)/(4): prepare the planner's exact operations, commit them, and
/// assert the revision advanced exactly once and the document is canonical.
async fn cc7_prepare_commit_and_compare(
    client: &RunningService<RoleClient, ()>,
    core: &Core,
    revision: u64,
    operations: serde_json::Value,
    expected: &Document,
) {
    let prepared = prepare_plan(client, revision, operations).await;
    assert_eq!(
        prepared.is_error,
        Some(false),
        "{:?}",
        prepared.structured_content
    );
    let committed = client
        .call_tool(commit_request(revision, &prepared))
        .await
        .unwrap();
    assert_eq!(
        committed.is_error,
        Some(false),
        "{:?}",
        committed.structured_content
    );
    assert_eq!(
        cc7_revision(client).await,
        revision + 1,
        "one committed plan must advance the revision exactly once"
    );
    assert_eq!(
        &query_document(core),
        expected,
        "the committed document must equal cc7_canonical_operations applied to the same base"
    );
}

/// CC7 §5.4: CC7 adds no tool, so the served surface, the internal registry
/// and `INSPECTOR_TOOL_NAMES` are byte-for-byte what CC6 published.
#[tokio::test(flavor = "multi_thread")]
async fn cc7_the_agent_surface_is_unchanged_by_this_slice() {
    let core = Core::spawn(Document::default()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core, media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // The served surface, over the live endpoint.
    let tools = client.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 7, "CC7 adds no served tool");
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        kinewright_agent::compact_tool_names()
    );

    // The internal registry: 124 tools, of which `INSPECTOR_TOOL_NAMES` is 75.
    let registry = kinewright_agent::capability_tool_names().unwrap();
    let operations = kinewright_agent::operation_tools().unwrap();
    assert_eq!(registry.len(), 124, "CC7 adds no registry tool");
    assert_eq!(
        registry.len() - operations.len(),
        75,
        "INSPECTOR_TOOL_NAMES stays at 75"
    );

    // The served byte counts CC6 recorded, asserted byte-identically.
    let metrics = server.tool_surface_metrics();
    assert_eq!(metrics.tool_count, 7);
    assert_eq!(metrics.serialized_bytes, 5_660, "{metrics:?}");
    assert_eq!(metrics.input_schema_bytes, 3_510, "{metrics:?}");
    assert_eq!(metrics.description_bytes, 998, "{metrics:?}");

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC7 §4(a)(1)'s failing-direction fixture, under the contract's own name.
///
/// The reference may not also be a candidate, so "the reference was retained"
/// cannot be satisfied by matching it against itself
/// (`color_scopes.rs:793-796`).
///
/// It is a **named function asserted inside** the (a) script rather than a
/// `#[test]` of its own: reaching `plan_shot_match` needs the two-clip
/// document, the FFV1 sources and a live MCP endpoint, and a second
/// `#[tokio::test]` would pay for all three again to make one refused call.
/// The name resolves, and the assertion runs on every run of the script.
async fn cc7_a_reference_retention_fails_when_the_reference_is_also_a_candidate(
    client: &RunningService<RoleClient, ()>,
    revision: u64,
) {
    let self_match = invoke_capability(
        client,
        "plan_shot_match",
        json!({
            "expected_revision": revision,
            "reference_clip_id": CC7_REFERENCE_CLIP_ID.0,
            "candidate_clip_ids": [CC7_REFERENCE_CLIP_ID.0],
        }),
    )
    .await;
    assert_eq!(self_match.is_error, Some(true));
    let self_match_body = self_match.structured_content.as_ref().unwrap();
    assert_eq!(
        self_match_body["code"], "invalid_request",
        "{self_match_body}"
    );
}

/// CC7 §5.2 (a) — mixed-camera interview.
///
/// `analyze_color_shot` ×2 → `plan_shot_match` → `prepare_edit_plan` →
/// `commit_edit_plan` → `get_color_qc` → `render_color_proof`. The reference
/// clip keeps **zero** effects, `saturation_percent` is proposed nowhere, and
/// the committed document equals `cc7_canonical_operations(MixedCamera)`.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc7_a_mixed_camera_match_retains_the_reference_and_lands_the_canonical_grade() {
    let reference_media = cc7_camera_source(Cc7Camera::A);
    let candidate_media = cc7_camera_source(Cc7Camera::B);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let reference = media.probe(reference_media.path()).unwrap();
    let candidate = media.probe(candidate_media.path()).unwrap();
    let candidate_start = reference.duration.0;
    let base = cc7_two_clip_document(reference, candidate);
    let expected = cc7_with_cc1_neutral_fill(
        cc7_canonical_document(&base, &cc7_canonical_operations(Cc7Scenario::MixedCamera)),
        CC7_CANDIDATE_CLIP_ID,
    );
    let core = Core::spawn(base.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let revision = cc7_revision(&client).await;
    assert_eq!(revision, 0);

    // Both shots, read-only.
    for clip in [CC7_REFERENCE_CLIP_ID, CC7_CANDIDATE_CLIP_ID] {
        let analysis = invoke_capability(
            &client,
            "analyze_color_shot",
            json!({"expected_revision": revision, "clip_id": clip.0}),
        )
        .await;
        assert_eq!(
            analysis.is_error,
            Some(false),
            "{:?}",
            analysis.structured_content
        );
        cc7_assert_evidence_only(
            analysis.structured_content.as_ref().unwrap(),
            "analyze_color_shot",
        );
    }

    // CC7 §5.1(2): the planner fails closed on a stale snapshot.
    cc7_assert_stale_revision(
        &client,
        "plan_shot_match",
        json!({
            "expected_revision": revision + 7,
            "reference_clip_id": CC7_REFERENCE_CLIP_ID.0,
            "candidate_clip_ids": [CC7_CANDIDATE_CLIP_ID.0],
        }),
        revision,
        revision + 7,
    )
    .await;

    cc7_a_reference_retention_fails_when_the_reference_is_also_a_candidate(&client, revision).await;

    // The match itself, over CC7 §2.3.3's twelve-patch achromatic chart band.
    let matched = invoke_capability(
        &client,
        "plan_shot_match",
        json!({
            "expected_revision": revision,
            "reference_clip_id": CC7_REFERENCE_CLIP_ID.0,
            "candidate_clip_ids": [CC7_CANDIDATE_CLIP_ID.0],
            "roi": cc7_scope_roi(CC7_CHART_BAND_ROI),
        }),
    )
    .await;
    assert_eq!(
        matched.is_error,
        Some(false),
        "{:?}",
        matched.structured_content
    );
    let matched = matched.structured_content.as_ref().unwrap().clone();
    cc7_assert_evidence_only(&matched, "plan_shot_match");
    // `reference_retained` is a hardcoded literal (`color_scopes.rs:906`) and
    // is asserted **present**, never as the evidence of retention (R-M19).
    assert_eq!(matched["reference_retained"], true);
    let candidates = matched["editable_operations"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    let proposal = &candidates[0];
    assert_eq!(proposal["clip_id"], CC7_CANDIDATE_CLIP_ID.0);
    cc7_assert_evidence_only(proposal, "plan_shot_match candidate");

    // CC7 §5.1(4): the regression pin. These integers are exactly what
    // `match_parameters` produced when probe-2 transcribed it in f64.
    assert_eq!(
        proposal["parameters"]["exposure_milli_stops"], CC7_MATCH_PROPOSAL_B.exposure_milli_stops,
        "{proposal}"
    );
    assert_eq!(
        proposal["parameters"]["temperature_percent"], CC7_MATCH_PROPOSAL_B.temperature_percent,
        "{proposal}"
    );
    assert_eq!(
        proposal["parameters"]["tint_percent"], CC7_MATCH_PROPOSAL_B.tint_percent,
        "{proposal}"
    );
    // CC7 §4(a)(4): the intentional desaturation is not corrected away, so no
    // saturation term is proposed anywhere in the response.
    assert!(
        proposal["parameters"].get("saturation_percent").is_none(),
        "no saturation term may be proposed: {proposal}"
    );
    assert!(
        proposal["proposal_details"]
            .get("saturation_percent")
            .is_none(),
        "no saturation control may appear in proposal_details: {proposal}"
    );
    // CC7 §4(b)(1)'s absent-key rule, in the passing direction here: every
    // control the planner *did* propose is unclamped for cam B.
    for name in [
        "exposure_milli_stops",
        "temperature_percent",
        "tint_percent",
    ] {
        assert_eq!(
            proposal["proposal_details"][name]["clamped"], false,
            "cam B is inside the planner's authority: {proposal}"
        );
    }

    // CC7 §5.1(1): planning applied nothing.
    assert_eq!(query_document(&core), base);

    cc7_prepare_commit_and_compare(
        &client,
        &core,
        revision,
        proposal["operations"].clone(),
        &expected,
    )
    .await;

    // CC7 §4(a)(1): the reference clip carries zero effects, and its
    // serialized form is byte-identical to its pre-commit form.
    let after = query_document(&core);
    assert!(
        after.tracks[0].clips[0].effects.is_empty(),
        "the reference clip must carry zero effects"
    );
    assert_eq!(
        serde_json::to_string(&after.tracks[0].clips[0]).unwrap(),
        serde_json::to_string(&base.tracks[0].clips[0]).unwrap(),
        "the reference clip must be byte-identical to its pre-commit form"
    );
    let effects = &after.tracks[0].clips[1].effects;
    assert_eq!(effects.len(), 1);
    let effect_id = effects[0].id.0;

    // CC7 §5.1(5): the agent-visible manifest carries the same integers.
    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    assert!(
        context["clips"][0]["color_nodes"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the reference clip publishes no colour node: {context}"
    );
    let node = &context["clips"][1]["color_nodes"][0];
    assert_eq!(node["kind"], "primary_correction");
    assert_eq!(
        node["parameters"]["exposure_milli_stops"], CC7_MATCH_PROPOSAL_B.exposure_milli_stops,
        "{node}"
    );
    assert_eq!(
        node["parameters"]["temperature_percent"], CC7_MATCH_PROPOSAL_B.temperature_percent,
        "{node}"
    );
    assert_eq!(
        node["parameters"]["tint_percent"], CC7_MATCH_PROPOSAL_B.tint_percent,
        "{node}"
    );

    // CC7 §4(a)(4): the skin band on the matched candidate.
    let qc = invoke_capability(
        &client,
        "get_color_qc",
        json!({
            "timecode": candidate_start,
            "checks": ["skin"],
            "roi": cc7_qc_roi(CC7_SKIN_BAND_ROI),
        }),
    )
    .await;
    let qc_body = qc.structured_content.as_ref().unwrap();
    if qc.is_error == Some(true) {
        assert!(
            std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
                .ok()
                .as_deref()
                == Some("1"),
            "get_color_qc refused: {qc_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to accept an \
             unavailable working proof on a machine with no usable adapter."
        );
        assert_eq!(qc_body["code"], "working_proof_unavailable");
        assert_eq!(qc_body["applied"], false);
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a working \
             proof; scenario (a)'s skin band was not measured."
        );
    } else {
        let skin = &qc_body["report"]["skin"];
        assert!(skin.is_object(), "{qc_body}");
        assert_eq!(
            skin["in_band_basis_points"], CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS,
            "the matched candidate's skin band is exact: {skin}"
        );
        assert!(
            skin["mean_hue_centidegrees"].is_i64(),
            "mean_hue_centidegrees must be Some on a chromatic skin band: {skin}"
        );
        assert!(
            qc_body["exceptions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|exception| exception["code"] != "skin_region_outside_band"),
            "a skin band at 10000 raises no Info exception: {qc_body}"
        );
    }

    // CC7 §5.2 (a)'s last call: the AFTER proof of the stored node.
    let proof = invoke_capability(
        &client,
        "render_color_proof",
        json!({
            "expected_revision": revision + 1,
            "clip_id": CC7_CANDIDATE_CLIP_ID.0,
            "timecode": candidate_start,
            "effect_id": effect_id,
            "look_comparison": "after",
        }),
    )
    .await;
    let proof_body = proof.structured_content.as_ref().unwrap();
    if proof.is_error == Some(true) {
        assert!(
            std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
                .ok()
                .as_deref()
                == Some("1"),
            "render_color_proof refused: {proof_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to \
             accept an unavailable proof on a machine with no usable adapter."
        );
        assert_eq!(proof_body["code"], "color_proof_render_failed");
        assert_eq!(proof_body["applied"], false);
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a colour \
             proof; scenario (a)'s AFTER proof was not exercised."
        );
    } else {
        cc7_assert_evidence_only(proof_body, "render_color_proof");
        assert_eq!(proof_body["look_comparison"]["effect_id"], effect_id);
        assert_eq!(proof_body["look_comparison"]["variant"], "after");
        assert_ne!(
            proof_body["hashes"]["before_rgba8_pixels_sha256"],
            proof_body["hashes"]["after_rgba8_pixels_sha256"],
            "the matched grade must change the picture: {proof_body}"
        );
    }

    // Whatever branch ran, the revision moved exactly once in this test.
    assert_eq!(cc7_revision(&client).await, revision + 1);

    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC7 §5.2 (b) — wrong white balance and underexposure.
///
/// `plan_shot_match` on the recoverable C1 document, then on the C2 document
/// that is beyond the planner's authority → prepare/commit (C2) →
/// `get_color_qc` with `range`, `gamut`, `tags` and `per_node`. (b1) publishes
/// **no** clamp; (b2) publishes the `temperature_percent` clamp at the
/// descriptor bound and one `delivery_range_excursion` **Warning** whose
/// per-node attribution names the primary node alone.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc7_b_wrong_balance_publishes_the_clamp_and_the_range_warning() {
    let reference_media = cc7_camera_source(Cc7Camera::A);
    let recoverable_media = cc7_camera_source(Cc7Camera::C1);
    let unrecoverable_media = cc7_camera_source(Cc7Camera::C2);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());

    // ---------------------------------------------------------------- (b1)
    let reference = media.probe(reference_media.path()).unwrap();
    let recoverable = media.probe(recoverable_media.path()).unwrap();
    let candidate_start = reference.duration.0;
    let base = cc7_two_clip_document(reference, recoverable);
    let core = Core::spawn(base.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media.clone()).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let revision = cc7_revision(&client).await;
    cc7_assert_stale_revision(
        &client,
        "plan_shot_match",
        json!({
            "expected_revision": revision + 3,
            "reference_clip_id": CC7_REFERENCE_CLIP_ID.0,
            "candidate_clip_ids": [CC7_CANDIDATE_CLIP_ID.0],
        }),
        revision,
        revision + 3,
    )
    .await;

    let matched = invoke_capability(
        &client,
        "plan_shot_match",
        json!({
            "expected_revision": revision,
            "reference_clip_id": CC7_REFERENCE_CLIP_ID.0,
            "candidate_clip_ids": [CC7_CANDIDATE_CLIP_ID.0],
            "roi": cc7_scope_roi(CC7_CHART_BAND_ROI),
        }),
    )
    .await;
    assert_eq!(
        matched.is_error,
        Some(false),
        "{:?}",
        matched.structured_content
    );
    let matched = matched.structured_content.as_ref().unwrap().clone();
    cc7_assert_evidence_only(&matched, "plan_shot_match");
    let recoverable_proposal = &matched["editable_operations"][0];
    // CC7 §4(b)(1), R-M19: an absent `proposal_details` key means *not
    // proposed*, never *zero* (`color_scopes.rs:1897-1903`), so the gate
    // iterates the controls that ARE present — and separately asserts
    // `temperature_percent` is one of them, so a run in which the planner
    // proposed nothing at all cannot pass by vacuous iteration.
    let details = recoverable_proposal["proposal_details"]
        .as_object()
        .unwrap();
    assert!(
        details.contains_key("temperature_percent"),
        "the recoverable candidate must propose a temperature: {recoverable_proposal}"
    );
    let mut present = Vec::new();
    for name in [
        "exposure_milli_stops",
        "temperature_percent",
        "tint_percent",
    ] {
        if let Some(control) = details.get(name) {
            present.push(name);
            // `cc7_b_c1_publishes_no_clamp`, inline: C1 is recoverable, so
            // every control it *did* propose is inside its descriptor bound,
            // and (b2)'s clamp assertion below is therefore not tautological.
            assert_eq!(
                control["clamped"], false,
                "C1 is inside the planner's authority: {control}"
            );
            assert_eq!(
                control["requested"], control["value"],
                "an unclamped control writes exactly what it requested: {control}"
            );
        }
    }
    assert!(
        !present.is_empty(),
        "the planner must propose something for C1: {recoverable_proposal}"
    );
    // CC7 §5.1(4), R2-MAJ-1: `CC7_MATCH_PROPOSAL_C1` is a **regression pin on
    // the live planner**, taken here against the real `match_parameters`
    // (`color_scopes.rs:1860-1965`) rather than only against the media crate's
    // independent f64 replica. Without these three lines a planner that
    // stopped proposing exposure, or moved `+1 465`, would still pass the
    // iteration above — which is the vacuity R-M19 exists to close.
    assert_eq!(
        recoverable_proposal["parameters"]["exposure_milli_stops"],
        CC7_MATCH_PROPOSAL_C1.exposure_milli_stops,
        "{recoverable_proposal}"
    );
    assert_eq!(
        recoverable_proposal["parameters"]["temperature_percent"],
        CC7_MATCH_PROPOSAL_C1.temperature_percent,
        "{recoverable_proposal}"
    );
    // Errata D-E5: C1's tint delta rounds to `0`, so the control is omitted
    // entirely — the absent-key rule (R-M19) exercised by a real measurement.
    // `CC7_MATCH_PROPOSAL_C1.tint_percent == 0` *means* "not proposed".
    assert_eq!(CC7_MATCH_PROPOSAL_C1.tint_percent, 0);
    assert!(
        !details.contains_key("tint_percent"),
        "C1's tint rounds to zero, so the control is not proposed: {details:?}"
    );
    assert!(
        recoverable_proposal["parameters"]
            .get("tint_percent")
            .is_none(),
        "a control that is not proposed writes no parameter: {recoverable_proposal}"
    );
    assert_eq!(
        present,
        vec!["exposure_milli_stops", "temperature_percent"],
        "C1 proposes exactly two controls: {recoverable_proposal}"
    );
    const {
        assert!(!CC7_MATCH_PROPOSAL_C1.temperature_clamped);
    }
    eprintln!(
        "CC7 (b1) measured on the amended twelve-patch band: present={present:?} parameters={} details={}",
        recoverable_proposal["parameters"], recoverable_proposal["proposal_details"],
    );
    // §5.2's (b) script commits C2, never C1: (b1)'s canonical document is
    // proved by the media fixtures, so nothing is committed on this server and
    // the revision must not have moved.
    assert_eq!(query_document(&core), base);
    assert_eq!(cc7_revision(&client).await, revision);
    client.cancel().await.unwrap();
    server.shutdown();

    // ---------------------------------------------------------------- (b2)
    let reference = media.probe(reference_media.path()).unwrap();
    let unrecoverable = media.probe(unrecoverable_media.path()).unwrap();
    let base = cc7_two_clip_document(reference, unrecoverable);
    let expected = cc7_with_cc1_neutral_fill(
        cc7_canonical_document(&base, &cc7_canonical_operations(Cc7Scenario::WhiteBalance)),
        CC7_CANDIDATE_CLIP_ID,
    );
    let core = Core::spawn(base.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let revision = cc7_revision(&client).await;

    let matched = invoke_capability(
        &client,
        "plan_shot_match",
        json!({
            "expected_revision": revision,
            "reference_clip_id": CC7_REFERENCE_CLIP_ID.0,
            "candidate_clip_ids": [CC7_CANDIDATE_CLIP_ID.0],
            "roi": cc7_scope_roi(CC7_CHART_BAND_ROI),
        }),
    )
    .await;
    assert_eq!(
        matched.is_error,
        Some(false),
        "{:?}",
        matched.structured_content
    );
    let matched = matched.structured_content.as_ref().unwrap().clone();
    let proposal = &matched["editable_operations"][0];
    assert_eq!(
        proposal["parameters"]["exposure_milli_stops"], CC7_MATCH_PROPOSAL_C2.exposure_milli_stops,
        "{proposal}"
    );
    assert_eq!(
        proposal["parameters"]["temperature_percent"], CC7_MATCH_PROPOSAL_C2.temperature_percent,
        "{proposal}"
    );
    assert_eq!(
        proposal["parameters"]["tint_percent"], CC7_MATCH_PROPOSAL_C2.tint_percent,
        "{proposal}"
    );
    // CC7 §4(b)(2): the clamp is published, with its bound and the raw term.
    let temperature = &proposal["proposal_details"]["temperature_percent"];
    assert_eq!(
        temperature["clamped"], CC7_MATCH_PROPOSAL_C2.temperature_clamped,
        "{temperature}"
    );
    // R2 minor 1/2: the published bound is the **descriptor's**, read from the
    // same place `primary_parameter_bounds` reads it. Comparing `max` against
    // the clamped value would let a descriptor change and a clamp change move
    // together and cancel; a bare `-100` would restate a CC1 fact at the call
    // site (§2.1).
    let (temperature_min, temperature_max) = cc7_primary_bounds("temperature_percent");
    assert_eq!(temperature["min"], temperature_min, "{temperature}");
    assert_eq!(temperature["max"], temperature_max, "{temperature}");
    assert_eq!(
        CC7_MATCH_PROPOSAL_C2.temperature_percent, temperature_max,
        "C2's published value IS the descriptor's upper bound"
    );
    // `requested` is `current + delta`, i.e. the **rounded** first-order term
    // for a non-composed proposal (`color_scopes.rs:1918-1924`).
    // `CC7_MATCH_PROPOSAL_C2.temperature_unrounded_delta` is that rounded
    // number despite its name (R2 minor 3), so the response's real `f64`
    // `unrounded_delta` is read here too and asserted to round onto it — the
    // one place the two quantities are tied together.
    let requested = CC7_MATCH_PROPOSAL_C2
        .temperature_unrounded_delta
        .expect("C2's temperature clamps from a measured raw delta");
    assert_eq!(temperature["requested"], requested, "{temperature}");
    let unrounded = temperature["unrounded_delta"]
        .as_f64()
        .unwrap_or_else(|| panic!("a clamped control publishes its raw term: {temperature}"));
    #[allow(clippy::cast_possible_truncation)]
    let rounded = unrounded.round() as i64;
    assert_eq!(
        rounded, requested,
        "the published unrounded_delta must round onto requested: {temperature}"
    );
    assert!(
        requested > temperature_max,
        "the clamp is only meaningful if the raw term left the bound: {temperature}"
    );
    assert_eq!(
        proposal["proposal_details"]["exposure_milli_stops"]["clamped"], false,
        "exposure stays inside its bound while temperature clamps: {proposal}"
    );

    // CC7 §5.1(1), R2 minor 9: planning applied nothing on the (b2) leg
    // either — the same check (a), (c), (d) and (e) make before their commits.
    assert_eq!(query_document(&core), base, "planning must apply nothing");

    cc7_prepare_commit_and_compare(
        &client,
        &core,
        revision,
        proposal["operations"].clone(),
        &expected,
    )
    .await;

    // CC7 §5.1(5), R2-MAJ-2: the same three integers, re-read from
    // `get_color_context`'s `color_nodes` manifest, so the committed document
    // and the agent-visible manifest cannot disagree.
    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    assert!(
        context["clips"][0]["color_nodes"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the reference clip publishes no colour node: {context}"
    );
    let node = &context["clips"][1]["color_nodes"][0];
    assert_eq!(node["kind"], "primary_correction", "{node}");
    assert_eq!(
        node["parameters"]["exposure_milli_stops"], CC7_MATCH_PROPOSAL_C2.exposure_milli_stops,
        "{node}"
    );
    assert_eq!(
        node["parameters"]["temperature_percent"], CC7_MATCH_PROPOSAL_C2.temperature_percent,
        "{node}"
    );
    assert_eq!(
        node["parameters"]["tint_percent"], CC7_MATCH_PROPOSAL_C2.tint_percent,
        "{node}"
    );
    // The manifest publishes the clamped value, never the raw term the planner
    // asked for.
    assert_ne!(
        node["parameters"]["temperature_percent"], requested,
        "{node}"
    );

    // CC6 §7: `max_nodes` is validated on every call, not only when `per_node`
    // is asked for.
    let over_budget = invoke_capability(
        &client,
        "get_color_qc",
        json!({"timecode": candidate_start, "max_nodes": 17}),
    )
    .await;
    assert_eq!(over_budget.is_error, Some(true));
    assert_eq!(
        over_budget.structured_content.as_ref().unwrap()["code"],
        "color_qc_node_budget_exceeded"
    );

    // CC7 §4(b)(3): the compromise is visible and typed.
    let qc = invoke_capability(
        &client,
        "get_color_qc",
        json!({
            "timecode": candidate_start,
            "checks": ["range", "gamut", "tags", "per_node"],
            "max_nodes": 16,
        }),
    )
    .await;
    let qc_body = qc.structured_content.as_ref().unwrap();
    if qc.is_error == Some(true) {
        assert!(
            std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
                .ok()
                .as_deref()
                == Some("1"),
            "get_color_qc refused: {qc_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to accept an \
             unavailable working proof on a machine with no usable adapter."
        );
        assert_eq!(qc_body["code"], "working_proof_unavailable");
        assert_eq!(qc_body["applied"], false);
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a working \
             proof; scenario (b2)'s range Warning was not measured."
        );
    } else {
        cc7_assert_evidence_only(qc_body, "get_color_qc");
        let report = &qc_body["report"];
        // A Warning is not an Error: the encode still passes technically.
        assert_eq!(report["technical_pass"], true, "{report}");
        let exceptions = qc_body["exceptions"].as_array().unwrap();
        let excursions = exceptions
            .iter()
            .filter(|exception| exception["code"] == "delivery_range_excursion")
            .collect::<Vec<_>>();
        assert_eq!(
            excursions.len(),
            1,
            "exactly one range excursion is expected: {exceptions:#?}"
        );
        assert_eq!(excursions[0]["severity"], "warning", "{:#?}", excursions[0]);
        assert_eq!(
            excursions[0]["field"], "blue.over_basis_points",
            "{:#?}",
            excursions[0]
        );
        assert_eq!(excursions[0]["allowed"], "< 10", "{:#?}", excursions[0]);
        assert_eq!(
            excursions[0]["observed"],
            CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED.to_string(),
            "{:#?}",
            excursions[0]
        );
        // CC7 §4(b)(3), A16: the excursion is on the blue channel alone.
        assert_eq!(
            report["range"]["blue"]["over_pixel_count"], CC7_C2_OVER_RANGE_PIXELS_REPORTED,
            "{report}"
        );
        assert_eq!(report["range"]["red"]["over_pixel_count"], 0, "{report}");
        assert_eq!(report["range"]["green"]["over_pixel_count"], 0, "{report}");
        // R2 minor 5: §4(b)(3)'s `maximum_over_excursion_millionths` was
        // printed and never read. The magnitude now has its constant —
        // `CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS` — so the depth of the
        // excursion is gated here rather than merely asserted non-zero, and
        // §2.1's ban on restating a number in the fixture is honoured by
        // naming the constant.
        assert_eq!(
            report["range"]["blue"]["maximum_over_excursion_millionths"]
                .as_i64()
                .unwrap_or_else(|| panic!("the range report publishes a depth: {report}")),
            CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS,
            "{report}"
        );
        assert_eq!(
            report["range"]["red"]["maximum_over_excursion_millionths"], 0,
            "{report}"
        );
        assert_eq!(
            report["range"]["green"]["maximum_over_excursion_millionths"], 0,
            "{report}"
        );
        // CC6's `qc_per_node_truncated` Info must be absent at 16 nodes.
        assert!(
            exceptions
                .iter()
                .all(|exception| exception["code"] != "qc_per_node_truncated"),
            "{exceptions:#?}"
        );
        // CC7 §4(b)(3): per-node attribution names the primary alone.
        let nodes = &report["nodes"];
        assert_eq!(nodes["attribution"], "node_removed", "{nodes}");
        assert_eq!(nodes["truncated"], false, "{nodes}");
        let contributions = nodes["nodes"].as_array().unwrap();
        assert_eq!(contributions.len(), 1, "{nodes}");
        assert_eq!(contributions[0]["node_kind"], "primary_correction");
        assert_eq!(contributions[0]["clip"], CC7_CANDIDATE_CLIP_ID.0);
        assert_eq!(contributions[0]["gamut_basis_points_delta"], 0, "{nodes}");
        assert!(
            contributions[0]["range_basis_points_delta"]
                .as_i64()
                .unwrap()
                > 0,
            "the primary node is the sole cause of the excursion: {nodes}"
        );
        eprintln!(
            "CC7 (b2) measured: blue.over_basis_points={} over_pixel_count={} \
             maximum_over_excursion_millionths={} red.over_pixel_count={} \
             green.over_pixel_count={} range_basis_points_delta={}",
            report["range"]["blue"]["over_basis_points"],
            report["range"]["blue"]["over_pixel_count"],
            report["range"]["blue"]["maximum_over_excursion_millionths"],
            report["range"]["red"]["over_pixel_count"],
            report["range"]["green"]["over_pixel_count"],
            contributions[0]["range_basis_points_delta"],
        );
    }

    assert_eq!(cc7_revision(&client).await, revision + 1);
    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC7 §5.2 (c) — log-like input normalised by an imported technical LUT.
///
/// The server carries the project session's saved-project-path handle, exactly
/// as `cc4_branch_server_with_the_project_path_handle_resolves_imported_availability`
/// (`:1351`) does, because `import_lut_asset` reports `project_not_saved`
/// without one (`server.rs:352`). Script: `analyze_color_shot` →
/// `import_lut_asset` → `list_look_assets` → `plan_technical_lut` →
/// prepare/commit → `get_color_qc` → `render_color_proof`.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc7_c_log_like_input_is_normalised_by_an_imported_technical_lut() {
    let directory = std::env::temp_dir().join(format!(
        "kinewright-cc7-c-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let project = directory.join("log-like.kinewright");
    let cube = write_log_like_inverse_cube(&directory, CC7_LOG_CUBE_SIZE);
    let cube_bytes = std::fs::read(&cube).unwrap();
    let asset_record = kinewright_core::cc7_scenarios::cc7_log_lut_asset(
        &kinewright_media::sha256_bytes(&cube_bytes),
        cube_bytes.len() as u64,
        &cube.display().to_string(),
    );

    let generated = cc7_log_source();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let carrier = media.probe(generated.path()).unwrap();
    let base = single_clip_document(carrier);
    let expected = cc7_canonical_document(
        &base,
        &cc7_lut_backed_canonical_operations(Cc7Scenario::LogLike, asset_record),
    );
    let core = Core::spawn(base.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let approvals = cc7_approve_confirmations(server.confirmations(), "import_lut_asset");
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let revision = cc7_revision(&client).await;
    assert_eq!(revision, 0);

    // CC7 §4(c)(1), A21: the log signature, in the 16-bit unit the tool
    // publishes. `mean_code_values.luma` is an 8-bit mean and is the wrong
    // field; these two are `ChannelStatistics` percentiles (`scopes.rs:576`).
    let analysis = invoke_capability(
        &client,
        "analyze_color_shot",
        json!({"expected_revision": revision, "clip_id": CC7_SINGLE_CLIP_ID.0}),
    )
    .await;
    assert_eq!(
        analysis.is_error,
        Some(false),
        "{:?}",
        analysis.structured_content
    );
    let analysis = analysis.structured_content.as_ref().unwrap();
    cc7_assert_evidence_only(analysis, "analyze_color_shot");
    let luma = &analysis["shot"]["scope_statistics"]["luma"];
    let first_percentile = luma["first_percentile"].as_i64().unwrap();
    let ninety_ninth = luma["ninety_ninth_percentile"].as_i64().unwrap();
    eprintln!(
        "CC7 (c) measured carrier luma percentiles (16-bit): p1={first_percentile} p99={ninety_ninth}"
    );
    assert!(
        first_percentile >= CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
        "the carrier's shadows must sit off the floor: {first_percentile}"
    );
    assert!(
        ninety_ninth <= CC7_LOG_P99_MAX_CODE16,
        "the carrier's highlights must sit off the ceiling: {ninety_ninth}"
    );

    // CC7 §4(c)(5): the import needs a saved project, and says so.
    let unsaved = invoke_capability(
        &client,
        "import_lut_asset",
        json!({"expected_revision": revision, "path": cube.display().to_string()}),
    )
    .await;
    assert_eq!(unsaved.is_error, Some(true));
    assert_eq!(
        unsaved.structured_content.as_ref().unwrap()["code"],
        "project_not_saved"
    );
    assert_eq!(
        query_document(&core),
        base,
        "a refused import changes nothing"
    );

    // The saved-project handle, shared exactly as the session publishes it.
    server.set_project_path(Some(project.clone()));
    let imported = invoke_capability(
        &client,
        "import_lut_asset",
        json!({"expected_revision": revision, "path": cube.display().to_string()}),
    )
    .await;
    assert_eq!(
        imported.is_error,
        Some(false),
        "{:?}",
        imported.structured_content
    );
    let imported = imported.structured_content.as_ref().unwrap().clone();
    assert_eq!(imported["reused_existing_asset"], false);
    let lut_asset_id = imported["lut_asset"]["lut_asset_id"].as_u64().unwrap();
    assert_eq!(lut_asset_id, CC7_LUT_ASSET_ID.0);
    assert_eq!(imported["lut_asset"]["size"], CC7_LOG_CUBE_SIZE);
    // `import_lut_asset` applies its own `AddLutAsset`, so the revision has
    // already moved once before the node is planned.
    let after_import = cc7_revision(&client).await;
    assert_eq!(after_import, revision + 1);

    let listed = invoke_capability(&client, "list_look_assets", json!({})).await;
    let listed = listed.structured_content.as_ref().unwrap();
    assert_eq!(listed["store_root_known"], true);
    assert_eq!(listed["assets"][0]["availability"]["kind"], "verified");
    assert_eq!(
        listed["assets"][0]["sha256"],
        kinewright_media::sha256_bytes(&cube_bytes)
    );

    // CC7 §5.5: an unregistered asset id is the typed look refusal.
    let missing = invoke_capability(
        &client,
        "plan_technical_lut",
        json!({
            "expected_revision": after_import,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "lut_asset_id": lut_asset_id + 98,
        }),
    )
    .await;
    assert_eq!(missing.is_error, Some(true));
    assert_eq!(
        missing.structured_content.as_ref().unwrap()["code"],
        "missing_lut_asset"
    );

    cc7_assert_stale_revision_prose(
        &client,
        &core,
        "plan_technical_lut",
        json!({
            "expected_revision": after_import + 5,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "lut_asset_id": lut_asset_id,
        }),
        after_import,
        after_import + 5,
    )
    .await;

    let plan = invoke_capability(
        &client,
        "plan_technical_lut",
        json!({
            "expected_revision": after_import,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "lut_asset_id": lut_asset_id,
            "input_encoding_token": 0,
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false), "{:?}", plan.structured_content);
    let plan = plan.structured_content.as_ref().unwrap().clone();
    cc7_assert_evidence_only(&plan, "plan_technical_lut");
    assert_eq!(plan["kind"], "technical_lut");
    assert_eq!(plan["color_stage"], "input");
    assert_eq!(plan["insert_index"], 0);
    assert_eq!(plan["created_new_node"], true);
    let effect_id = plan["target_effect_id"].as_u64().unwrap();
    assert!(
        query_document(&core).tracks[0].clips[0].effects.is_empty(),
        "planning must not apply anything"
    );

    cc7_prepare_commit_and_compare(
        &client,
        &core,
        after_import,
        plan["operations"].clone(),
        &expected,
    )
    .await;

    // CC7 §4(c)(4): node order, on the agent-visible manifest.
    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    let nodes = context["clips"][0]["color_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["kind"], "technical_lut");
    assert_eq!(nodes[0]["color_stage"], "input");
    assert_eq!(nodes[0]["stage_index"], 0);
    assert_eq!(nodes[0]["lut_asset_id"], lut_asset_id);
    // `input_encoding_token = 0` is the descriptor neutral and is therefore
    // not stored, but the manifest still resolves it (§2.5).
    assert_eq!(nodes[0]["input_encoding"], "display709");
    assert_eq!(nodes[0]["mix_basis_points"], CC7_LOOK_MIX_BASIS_POINTS);
    // R2 minor 6: the ordering loop that used to stand here could never run —
    // `nodes.len() == 1` above — so it read as a gate and was not one. §4(c)(4)
    // on a one-node stack is exactly the two facts asserted above: the node is
    // at the **input** stage and at `stage_index 0`, so nothing precedes it.
    assert!(
        nodes
            .iter()
            .all(|node| node["color_stage"] != "correction" && node["color_stage"] != "look"),
        "(c) commits one input-stage node and nothing else: {nodes:#?}"
    );

    // CC7 errata D-E3: the agent server never publishes an imported LUT's
    // bytes to the renderer — the boundary
    // `cc4_render_color_proof_reports_the_unpublished_lut_asset_from_the_real_renderer`
    // (`:1439`) already pins — so (c)'s proof-side calls cannot render, and
    // both refuse **deterministically and typed**. That is not a GPU-
    // availability question, so §5.3's skip branch does not apply and these
    // are asserted unconditionally: accepting either branch would assert
    // nothing.
    let qc = invoke_capability(&client, "get_color_qc", json!({"timecode": 0})).await;
    let qc_body = qc.structured_content.as_ref().unwrap();
    assert_eq!(qc.is_error, Some(true), "{qc_body}");
    assert_eq!(qc_body["code"], "working_proof_unavailable", "{qc_body}");
    assert_eq!(qc_body["applied"], false, "{qc_body}");
    assert_eq!(qc_body["evidence_only"], true, "{qc_body}");
    assert_eq!(qc_body["details"]["field"], "working_proof", "{qc_body}");
    assert!(
        qc_body["details"]["observed"]
            .as_str()
            .is_some_and(|observed| observed.contains("missing_lut_asset")),
        "the refusal names the unpublished asset, not a GPU adapter: {qc_body}"
    );

    let proof = invoke_capability(
        &client,
        "render_color_proof",
        json!({
            "expected_revision": after_import + 1,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "timecode": 0,
            "effect_id": effect_id,
            "look_comparison": "after",
        }),
    )
    .await;
    let proof_body = proof.structured_content.as_ref().unwrap();
    assert_eq!(proof.is_error, Some(true), "{proof_body}");
    assert_eq!(proof_body["code"], "missing_lut_asset", "{proof_body}");
    assert_eq!(proof_body["details"]["lut_asset_id"], lut_asset_id);
    assert_eq!(proof_body["details"]["effect_id"], effect_id);
    assert_eq!(proof_body["details"]["stage"], "after");
    assert_eq!(
        proof_body["details"]["lut_sha256"],
        kinewright_media::sha256_bytes(&cube_bytes)
    );

    assert_eq!(cc7_revision(&client).await, after_import + 1);
    approvals.assert_approved_and_stop("import_lut_asset");
    client.cancel().await.unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

/// CC7 §5.2 (d) — product and skin.
///
/// `plan_secondary_correction` derives the qualifier from the `product_red`
/// patch, `plan_primary_correction` writes the `saturation_percent = 40` the
/// secondary planner has no field for (errata D-E4), and the committed node is
/// `cc7_canonical_operations(ProductAndSkin)` exactly. `inspect_grade_matte`
/// then measures `covered == full == 192`, `partial == 0`, and the skin band's
/// hue is unchanged by the grade.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc7_d_product_qualifier_selects_its_patch_and_leaves_skin_alone() {
    let generated = cc7_camera_source(Cc7Camera::A);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let base = single_clip_document(asset);
    let expected = cc7_canonical_document(
        &base,
        &cc7_canonical_operations(Cc7Scenario::ProductAndSkin),
    );
    let core = Core::spawn(base.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let revision = cc7_revision(&client).await;
    assert_eq!(revision, 0);

    // CC7 §4(d)(3): the skin band before the grade, so "unchanged" is a
    // measured difference rather than a single reading.
    let skin_before = invoke_capability(
        &client,
        "get_color_qc",
        json!({
            "timecode": 0,
            "checks": ["skin"],
            "roi": cc7_qc_roi(CC7_SKIN_BAND_ROI),
        }),
    )
    .await;
    let skin_before_body = skin_before.structured_content.as_ref().unwrap().clone();
    let gpu_may_skip = std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
        .ok()
        .as_deref()
        == Some("1");
    if skin_before.is_error == Some(true) {
        assert!(
            gpu_may_skip,
            "get_color_qc refused: {skin_before_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to \
             accept an unavailable working proof on a machine with no usable adapter."
        );
        assert_eq!(skin_before_body["code"], "working_proof_unavailable");
        assert_eq!(skin_before_body["applied"], false, "{skin_before_body}");
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a working \
             proof; scenario (d)'s skin hue was not measured."
        );
    }

    // CC7 §5.5: a technical input transform carries no matte.
    let technical = invoke_capability(
        &client,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "node_kind": "technical_lut",
            "sample_roi": cc7_scope_roi(CC7_PRODUCT_RED_ROI),
            "derive_qualifier_from_sample": true,
        }),
    )
    .await;
    assert_eq!(technical.is_error, Some(true));
    assert_eq!(
        technical.structured_content.as_ref().unwrap()["code"],
        "matte_unsupported_node_kind"
    );

    // CC7 §5.5: a skin check needs a region, and is refused before any render.
    let unscoped = invoke_capability(
        &client,
        "get_color_qc",
        json!({"timecode": 0, "checks": ["skin"]}),
    )
    .await;
    assert_eq!(unscoped.is_error, Some(true));
    assert_eq!(
        unscoped.structured_content.as_ref().unwrap()["code"],
        "color_qc_region_required"
    );

    cc7_assert_stale_revision_prose(
        &client,
        &core,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision + 4,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "node_kind": "primary_correction",
            "sample_roi": cc7_scope_roi(CC7_PRODUCT_RED_ROI),
            "derive_qualifier_from_sample": true,
        }),
        revision,
        revision + 4,
    )
    .await;

    let plan = invoke_capability(
        &client,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "node_kind": "primary_correction",
            "sample_roi": cc7_scope_roi(CC7_PRODUCT_RED_ROI),
            "derive_qualifier_from_sample": true,
            "timecode": 0,
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false), "{:?}", plan.structured_content);
    let plan = plan.structured_content.as_ref().unwrap().clone();
    cc7_assert_evidence_only(&plan, "plan_secondary_correction");
    assert_eq!(plan["kind"], "primary_correction");
    assert_eq!(plan["created_new_node"], true);
    let effect_id = plan["target_effect_id"].as_u64().unwrap();
    assert!(
        query_document(&core).tracks[0].clips[0].effects.is_empty(),
        "planning must not apply anything"
    );

    let prepared = prepare_plan(&client, revision, plan["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    assert_eq!(
        client
            .call_tool(commit_request(revision, &prepared))
            .await
            .unwrap()
            .is_error,
        Some(false)
    );
    assert_eq!(cc7_revision(&client).await, revision + 1);

    // CC7 errata D-E4: `SecondaryCorrectionPlanArgs` has no `saturation_percent`
    // field (`color_status.rs:4326-4374`), so §5.2's (d) call is two calls: the
    // matte through the secondary planner, then the grade through the CC1
    // primary planner, which retargets the same node in place and therefore
    // emits `SetEffectParam` alone — no second `AddEffect` and no neutral fill.
    let grade = invoke_capability(
        &client,
        "plan_primary_correction",
        json!({
            "expected_revision": revision + 1,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "parameters": {"saturation_percent": CC7_SECONDARY_SATURATION_PERCENT},
        }),
    )
    .await;
    assert_eq!(
        grade.is_error,
        Some(false),
        "{:?}",
        grade.structured_content
    );
    let grade = grade.structured_content.as_ref().unwrap().clone();
    assert_eq!(grade["applied"], false);
    assert_eq!(
        grade["target_effect_id"].as_u64().unwrap(),
        effect_id,
        "the grade must land on the matted node, not on a second one"
    );

    cc7_prepare_commit_and_compare(
        &client,
        &core,
        revision + 1,
        grade["operations"].clone(),
        &expected,
    )
    .await;

    // CC7 §5.1(5): the manifest publishes the same qualifier integers.
    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    let node = &context["clips"][0]["color_nodes"][0];
    assert_eq!(node["kind"], "primary_correction");
    assert_eq!(node["matte"]["enabled"], true);
    assert_eq!(node["matte"]["qualifier"]["enabled"], true);
    assert_eq!(node["matte"]["window_count"], 0);
    assert_eq!(
        node["parameters"]["saturation_percent"],
        CC7_SECONDARY_SATURATION_PERCENT
    );

    // CC7 §4(d)(1): the qualifier covers exactly its patch.
    let inspect = invoke_capability(
        &client,
        "inspect_grade_matte",
        json!({
            "expected_revision": revision + 2,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "effect_id": effect_id,
            "timecode": 0,
        }),
    )
    .await;
    let inspect_body = inspect.structured_content.as_ref().unwrap();
    if inspect.is_error == Some(true) {
        assert!(
            gpu_may_skip,
            "inspect_grade_matte refused: {inspect_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to \
             accept an unavailable matte proof on a machine with no usable adapter."
        );
        assert_eq!(inspect_body["code"], "matte_proof_unavailable");
        assert_eq!(inspect_body["applied"], false);
        assert_eq!(inspect_body["details"]["observed"]["has_matte"], true);
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a matte proof; \
             scenario (d)'s containment was not measured."
        );
    } else {
        let statistics = &inspect_body["statistics"];
        eprintln!(
            "CC7 (d) measured: covered={} full={} partial={} covered_basis_points={}",
            statistics["covered_pixel_count"],
            statistics["full_pixel_count"],
            statistics["partial_pixel_count"],
            statistics["covered_basis_points"],
        );
        assert_eq!(
            statistics["covered_pixel_count"], CC7_PRODUCT_PATCH_PIXEL_COUNT,
            "{statistics}"
        );
        assert_eq!(
            statistics["full_pixel_count"], CC7_PRODUCT_PATCH_PIXEL_COUNT,
            "{statistics}"
        );
        assert_eq!(statistics["partial_pixel_count"], 0, "{statistics}");
    }

    // CC7 §4(d)(3): the skin band's hue is untouched by the product grade.
    let skin_after = invoke_capability(
        &client,
        "get_color_qc",
        json!({
            "timecode": 0,
            "checks": ["skin"],
            "roi": cc7_qc_roi(CC7_SKIN_BAND_ROI),
        }),
    )
    .await;
    let skin_after_body = skin_after.structured_content.as_ref().unwrap();
    if skin_after.is_error == Some(true) {
        assert!(
            gpu_may_skip,
            "get_color_qc refused: {skin_after_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to \
             accept an unavailable working proof on a machine with no usable adapter."
        );
        assert_eq!(skin_after_body["code"], "working_proof_unavailable");
        assert_eq!(skin_after_body["applied"], false, "{skin_after_body}");
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a working \
             proof; scenario (d)'s post-grade skin band was not measured."
        );
    } else {
        let before = &skin_before_body["report"]["skin"];
        let after = &skin_after_body["report"]["skin"];
        assert!(
            before["mean_hue_centidegrees"].is_i64(),
            "mean_hue_centidegrees must be Some on both sides: {before}"
        );
        assert!(
            after["mean_hue_centidegrees"].is_i64(),
            "mean_hue_centidegrees must be Some on both sides: {after}"
        );
        assert_eq!(
            before["mean_hue_centidegrees"], after["mean_hue_centidegrees"],
            "the product qualifier must not move the skin hue"
        );
        assert_eq!(
            before["in_band_basis_points"], CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS,
            "{before}"
        );
        assert_eq!(
            after["in_band_basis_points"], CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS,
            "{after}"
        );
        eprintln!(
            "CC7 (d) skin: hue={} in_band={} considered={} excluded_achromatic={}",
            after["mean_hue_centidegrees"],
            after["in_band_basis_points"],
            after["considered_pixel_count"],
            after["excluded_achromatic_pixel_count"],
        );
    }

    assert_eq!(cc7_revision(&client).await, revision + 2);
    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC7 §4(e)(1)'s failing direction, under the contract's own name, taking the
/// contract's **pre-authorized fallback**.
///
/// §4(e)(1) names the typed refusal `bypass_not_lossless` as the failing
/// direction, and says in the same breath: "**If no construction is
/// reachable**, the failing direction is instead the hash-equality check on
/// the `after` call — `after_rgba8_pixels_sha256 != before_rgba8_pixels_sha256`
/// — and the contract records that the typed refusal is unreachable rather
/// than pretending the gate has a failing direction it does not have."
///
/// No construction is reachable on the real renderer. The one place the
/// refusal fires (`server.rs:23300-23336`) reaches it by injecting a fault —
/// a `NoopMedia` built with `bypass_leaks_pixel: Some(0x7f)`, a stub whose
/// bypass render deliberately differs from its absent render. `Compositor`
/// applies a bypassed node by not applying it, so on the production path the
/// bypass raster and the absent raster are the same bytes by construction and
/// there is no document, no keyframe and no look node that makes them differ.
/// CC7 therefore takes the fallback here, and asserts the refusal **absent**
/// on every proofed variant (the `code == null` check beside this call).
fn cc7_e_after_does_not_match_absent(body: &serde_json::Value) {
    assert_ne!(
        body["hashes"]["before_rgba8_pixels_sha256"], body["hashes"]["after_rgba8_pixels_sha256"],
        "the warm look must change the picture: {body}"
    );
}

/// CC7 §4(e)(4)'s failing direction, under the contract's own name.
///
/// The portability check is CC4's bit-identical relocation fixture, cited not
/// duplicated; CC7 adds exactly one agent-visible claim — after a Save-As
/// relocation, `list_look_assets` reports the (c) imported asset `verified`
/// with the same `sha256`. A **bare** relocation — the project path moves and
/// the store does not — must not report `verified`, or the claim above would
/// hold whatever the store did.
///
/// A named function asserted inside the (e) script, for §5.2's reason: the
/// claim is about one server whose project path has just moved, and it is not
/// separable from the session that moved it.
fn cc7_e_a_bare_relocation_reports_missing(listed: &serde_json::Value, lut_asset_id: u64) {
    let entry = listed["assets"]
        .as_array()
        .expect("the asset list")
        .iter()
        .find(|entry| entry["lut_asset_id"] == lut_asset_id)
        .expect("the imported asset is listed");
    assert_ne!(
        entry["availability"]["kind"], "verified",
        "a bare relocation cannot report verified: {listed}"
    );
}

/// CC7 §5.2 (e) — creative look.
///
/// `plan_creative_look` binds the built-in `warm` asset at its neutral mix →
/// prepare/commit → `render_color_proof` in all three variants → `get_color_qc`
/// on the `deep_shadow` patch → `list_look_assets` across a Save-As
/// relocation. `bypass_matches_absent` is `true` and `bypass_not_lossless` is
/// asserted **absent**.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc7_e_creative_look_bypass_matches_absent_and_reports_its_gamut() {
    let directory = std::env::temp_dir().join(format!("kinewright-cc7-e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let project = directory.join("look.kinewright");
    let relocated = directory.join("look-saved-as.kinewright");
    let cube = write_log_like_inverse_cube(&directory, CC7_LOG_CUBE_SIZE);
    let cube_sha = kinewright_media::sha256_bytes(&std::fs::read(&cube).unwrap());

    let generated = cc7_camera_source(Cc7Camera::A);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let warm = kinewright_media::BuiltinLook::Warm;
    let mut base = single_clip_document(asset);
    // A built-in look is `verified` from this binary's own bake, so the
    // scenario's own node needs no store (CC4 §2.6).
    base.lut_assets = vec![warm.to_lut_asset(CC7_LUT_ASSET_ID)];
    base.validate().expect("the CC7 (e) base document is valid");
    let expected =
        cc7_canonical_document(&base, &cc7_canonical_operations(Cc7Scenario::CreativeLook));

    let core = Core::spawn(base.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    server.set_project_path(Some(project.clone()));
    let approvals = cc7_approve_confirmations(server.confirmations(), "import_lut_asset");
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let revision = cc7_revision(&client).await;
    assert_eq!(revision, 0);

    cc7_assert_stale_revision_prose(
        &client,
        &core,
        "plan_creative_look",
        json!({
            "expected_revision": revision + 6,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "lut_asset_id": CC7_LUT_ASSET_ID.0,
        }),
        revision,
        revision + 6,
    )
    .await;

    let plan = invoke_capability(
        &client,
        "plan_creative_look",
        json!({
            "expected_revision": revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "lut_asset_id": CC7_LUT_ASSET_ID.0,
            "mix_basis_points": CC7_LOOK_MIX_BASIS_POINTS,
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false), "{:?}", plan.structured_content);
    let plan = plan.structured_content.as_ref().unwrap().clone();
    cc7_assert_evidence_only(&plan, "plan_creative_look");
    assert_eq!(plan["kind"], "creative_look");
    assert_eq!(plan["color_stage"], "look");
    assert_eq!(plan["lut_asset"]["sha256"], warm.pinned_sha256());
    let effect_id = plan["target_effect_id"].as_u64().unwrap();
    assert!(
        query_document(&core).tracks[0].clips[0].effects.is_empty(),
        "planning must not apply anything"
    );

    cc7_prepare_commit_and_compare(
        &client,
        &core,
        revision,
        plan["operations"].clone(),
        &expected,
    )
    .await;
    let after_commit = revision + 1;

    // CC7 §5.1(5), R2-MAJ-2: the binding and the mix, re-read from
    // `get_color_context`'s `color_nodes` manifest.
    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    let nodes = context["clips"][0]["color_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1, "{context}");
    let node = &nodes[0];
    assert_eq!(node["kind"], "creative_look", "{node}");
    assert_eq!(node["color_stage"], "look", "{node}");
    assert_eq!(node["lut_asset_id"], CC7_LUT_ASSET_ID.0, "{node}");
    assert_eq!(node["lut_sha256"], warm.pinned_sha256(), "{node}");
    // The neutral mix is resolved by the manifest and stored by neither the
    // planner nor the document (§2.5), so the manifest republishes `10 000`
    // while the node's parameter map does not carry it.
    assert_eq!(
        node["mix_basis_points"], CC7_LOOK_MIX_BASIS_POINTS,
        "{node}"
    );
    assert!(
        !query_document(&core).tracks[0].clips[0].effects[0]
            .parameters
            .contains_key("mix_basis_points"),
        "the neutral mix is resolved, never stored"
    );

    // CC7 §5.5: `look_comparison` without an `effect_id` is typed.
    let unbound = invoke_capability(
        &client,
        "render_color_proof",
        json!({
            "expected_revision": after_commit,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "timecode": 0,
            "look_comparison": "bypass",
        }),
    )
    .await;
    assert_eq!(unbound.is_error, Some(true));
    assert_eq!(
        unbound.structured_content.as_ref().unwrap()["code"],
        "look_comparison_requires_effect_id"
    );

    // CC7 §4(e)(1): before, after, bypass.
    let mut bypass_seen = false;
    for variant in ["before", "after", "bypass"] {
        let proof = invoke_capability(
            &client,
            "render_color_proof",
            json!({
                "expected_revision": after_commit,
                "clip_id": CC7_SINGLE_CLIP_ID.0,
                "timecode": 0,
                "effect_id": effect_id,
                "look_comparison": variant,
            }),
        )
        .await;
        let body = proof.structured_content.as_ref().unwrap();
        if proof.is_error == Some(true) {
            assert!(
                std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
                    .ok()
                    .as_deref()
                    == Some("1"),
                "render_color_proof refused: {body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to \
                 accept an unavailable proof on a machine with no usable adapter."
            );
            assert_eq!(body["code"], "color_proof_render_failed");
            assert_eq!(body["applied"], false);
            eprintln!(
                "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a colour \
                 proof; scenario (e)'s {variant} cell was not exercised."
            );
            continue;
        }
        // `bypass_not_lossless` is a refusal, never a `false` footnote
        // (R-M4): reaching this branch is the assertion that it did not fire.
        // R2 minor 13: `assert_ne!(body["code"], "bypass_not_lossless")` on a
        // success body compared `Value::Null` against a string and could never
        // fire, so the claim is made in the only non-vacuous form there is —
        // a successful proof publishes **no** refusal code at all.
        assert_eq!(
            body["code"],
            json!(null),
            "a successful proof publishes no refusal code: {body}"
        );
        cc7_assert_evidence_only(body, "render_color_proof");
        assert_eq!(body["look_comparison"]["variant"], variant, "{body}");
        assert_eq!(
            body["look_comparison"]["before_variant"], "absent",
            "{body}"
        );
        if variant == "bypass" {
            bypass_seen = true;
            assert_eq!(
                body["look_comparison"]["bypass_matches_absent"], true,
                "{body}"
            );
            assert_eq!(
                body["hashes"]["before_rgba8_pixels_sha256"],
                body["hashes"]["after_rgba8_pixels_sha256"],
                "a bypassed node is the byte-identical twin of an absent one: {body}"
            );
        } else {
            assert_eq!(
                body["look_comparison"]["bypass_matches_absent"],
                json!(null),
                "only the bypass cell publishes the claim: {body}"
            );
            if variant == "after" {
                cc7_e_after_does_not_match_absent(body);
            }
        }
    }

    // CC7 §4(e)(2): the gamut excursion, exactly where it is analytic.
    let qc = invoke_capability(
        &client,
        "get_color_qc",
        json!({
            "timecode": 0,
            "checks": ["gamut", "range"],
            "roi": cc7_qc_roi(CC7_DEEP_SHADOW_ROI),
        }),
    )
    .await;
    let qc_body = qc.structured_content.as_ref().unwrap();
    if qc.is_error == Some(true) {
        assert!(
            std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
                .ok()
                .as_deref()
                == Some("1"),
            "get_color_qc refused: {qc_body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to accept an \
             unavailable working proof on a machine with no usable adapter."
        );
        assert_eq!(qc_body["code"], "working_proof_unavailable");
        assert_eq!(qc_body["applied"], false, "{qc_body}");
        eprintln!(
            "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a working \
             proof; scenario (e)'s gamut count was not measured."
        );
    } else {
        assert!(
            bypass_seen,
            "the bypass cell must have rendered wherever the QC did"
        );
        let report = &qc_body["report"];
        assert_eq!(report["technical_pass"], true, "{report}");
        // R2 minor 12: "how many pixels the ROI resolves to" and "how many of
        // them are out of gamut" are two quantities and are read from two
        // constants, so an ROI that shrank and a look that stopped clipping
        // can no longer cancel. §11.2.1's resolved-pixel-rect claim is the
        // first; §4(e)(2)'s gamut count is the second.
        assert_eq!(
            report["region"]["region_pixel_count"],
            CC7_DEEP_SHADOW_RECT.pixels(),
            "the deep_shadow ROI must resolve to its own pixel rect: {report}"
        );
        assert_eq!(
            report["gamut"]["out_of_gamut_pixel_count"], CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
            "{report}"
        );
        let excursions = qc_body["exceptions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|exception| exception["code"] == "delivery_gamut_excursion")
            .collect::<Vec<_>>();
        assert_eq!(excursions.len(), 1, "{:#?}", qc_body["exceptions"]);
        assert_eq!(excursions[0]["severity"], "warning", "{:#?}", excursions[0]);
        eprintln!(
            "CC7 (e) measured on deep_shadow: out_of_gamut={} basis_points={} below_black={} \
             minimum_linear_millionths={}",
            report["gamut"]["out_of_gamut_pixel_count"],
            report["gamut"]["out_of_gamut_basis_points"],
            report["gamut"]["below_black_pixel_count"],
            report["gamut"]["minimum_linear_millionths"],
        );
    }

    // CC7 §4(e)(4): the one agent-visible portability check. Import an asset
    // into this project's store, Save As, and read the availability back.
    let imported = invoke_capability(
        &client,
        "import_lut_asset",
        json!({"expected_revision": after_commit, "path": cube.display().to_string()}),
    )
    .await;
    assert_eq!(
        imported.is_error,
        Some(false),
        "{:?}",
        imported.structured_content
    );
    let imported_id = imported.structured_content.as_ref().unwrap()["lut_asset"]["lut_asset_id"]
        .as_u64()
        .unwrap();
    let availability = |listed: &serde_json::Value, id: u64| -> serde_json::Value {
        listed["assets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["lut_asset_id"] == id)
            .expect("the imported asset is listed")
            .clone()
    };
    let listed = invoke_capability(&client, "list_look_assets", json!({})).await;
    let listed = listed.structured_content.as_ref().unwrap().clone();
    assert_eq!(
        availability(&listed, imported_id)["availability"]["kind"],
        "verified"
    );
    assert_eq!(availability(&listed, imported_id)["sha256"], cube_sha);

    // A *bare* relocation — the project path moves and the store does not —
    // must not report `verified`, so the check below is not vacuous.
    server.set_project_path(Some(relocated.clone()));
    let unrelocated = invoke_capability(&client, "list_look_assets", json!({})).await;
    let unrelocated = unrelocated.structured_content.as_ref().unwrap().clone();
    cc7_e_a_bare_relocation_reports_missing(&unrelocated, imported_id);

    // Save As copies the store beside the new project file, and the same
    // sha256 verifies again.
    let store_root = directory.join("look.kinewright-assets");
    let relocated_root = directory.join("look-saved-as.kinewright-assets");
    cc7_copy_directory(&store_root, &relocated_root);
    let saved_as = invoke_capability(&client, "list_look_assets", json!({})).await;
    let saved_as = saved_as.structured_content.as_ref().unwrap().clone();
    assert_eq!(saved_as["store_root_known"], true);
    assert_eq!(
        availability(&saved_as, imported_id)["availability"]["kind"],
        "verified",
        "{saved_as}"
    );
    assert_eq!(availability(&saved_as, imported_id)["sha256"], cube_sha);

    approvals.assert_approved_and_stop("import_lut_asset");
    client.cancel().await.unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

/// Copy a LUT store directory wholesale, which is what Save As does to the
/// project's `<stem>.kinewright-assets` root (CC4 §2.2).
fn cc7_copy_directory(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            cc7_copy_directory(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// One observation that failed CC7 §4(f)(2)'s accuracy gate.
///
/// Typed rather than a bare `bool` so the rejection names *which* sample and
/// *which* axis missed, and by how much: the failing-direction fixture below
/// asserts the whole record, so a gate that rejected the right sample for the
/// wrong reason would not satisfy it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cc7ObservationOffTolerance {
    local_frame: i64,
    axis: usize,
    error_basis_points: i64,
    tolerance_basis_points: i64,
}

/// CC7 §4(f)(2)'s observation-accuracy gate: every **surviving** raw
/// observation is within `CC7_TRACK_TOLERANCE_BASIS_POINTS` of §2.3.6's
/// analytic centre, on both axes.
///
/// `Ok(worst)` returns the worst of the two axes' errors, which is what the
/// script accumulates into its reported figure; `Err(_)` is the typed
/// rejection. Written as a function so that the gate the live script runs and
/// the gate `cc7_f_observation_gate_rejects_a_doubled_offset` proves can fail
/// are the **same** code — a failing direction asserted against a re-typed
/// copy of the comparison proves nothing about the comparison that ships.
fn cc7_observation_within_tolerance(
    local_frame: i64,
    sample_index: usize,
    observed_centre: [i64; 2],
) -> Result<i64, Cc7ObservationOffTolerance> {
    let analytic = CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS[sample_index];
    let mut worst = 0_i64;
    for axis in 0..2 {
        let error = (observed_centre[axis] - analytic[axis]).abs();
        if error > CC7_TRACK_TOLERANCE_BASIS_POINTS {
            return Err(Cc7ObservationOffTolerance {
                local_frame,
                axis,
                error_basis_points: error,
                tolerance_basis_points: CC7_TRACK_TOLERANCE_BASIS_POINTS,
            });
        }
        worst = worst.max(error);
    }
    Ok(worst)
}

/// CC7 §4(f)(2)'s failing direction, and §4.2's `track observation` row.
///
/// The gate `cc7_f_tracked_secondary_drops_only_the_occluded_samples` runs
/// over the live `observations[]` is fed the pinned observed centres with one
/// centre moved by `2 × CC7_TRACK_TOLERANCE_BASIS_POINTS`, and must reject
/// it, naming the sample, the axis and the error. Both directions are
/// asserted over **every** surviving sample, so neither the acceptance nor
/// the rejection can be an accident of which row was picked.
///
/// It needs no MCP session: the gate is a pure comparison against §2.3.6's
/// analytic table, and the live tracker's own numbers are the pinned
/// `CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS`, taken against the shipped
/// tracker by the script itself.
#[test]
fn cc7_f_observation_gate_rejects_a_doubled_offset() {
    let doubled = 2 * CC7_TRACK_TOLERANCE_BASIS_POINTS;
    for (index, frame) in CC7_TRACK_SURVIVING_SAMPLE_FRAMES.into_iter().enumerate() {
        let observed = CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS[index];

        // The passing direction: the shipped tracker's own observation.
        let worst = cc7_observation_within_tolerance(frame, index, observed)
            .unwrap_or_else(|rejection| panic!("frame {frame} must pass the gate: {rejection:?}"));
        assert!(worst <= CC7_TRACK_TOLERANCE_BASIS_POINTS);

        // The failing direction, one axis at a time. A doubled offset is
        // `2 × 200 = 400` bp from the analytic centre before the observation's
        // own error is counted, so the reported error is at least the doubled
        // offset less that error — and always over the tolerance.
        for axis in 0..2 {
            let mut offset = observed;
            offset[axis] += doubled;
            let rejection = cc7_observation_within_tolerance(frame, index, offset)
                .expect_err("a centre offset by twice the tolerance must be rejected");
            assert_eq!(rejection.local_frame, frame);
            assert_eq!(rejection.axis, axis);
            assert_eq!(
                rejection.tolerance_basis_points,
                CC7_TRACK_TOLERANCE_BASIS_POINTS
            );
            assert!(
                rejection.error_basis_points > CC7_TRACK_TOLERANCE_BASIS_POINTS,
                "frame {frame} axis {axis}: {rejection:?}"
            );
            // The offset is signed: moving the other way is rejected too, so
            // the gate is a two-sided bound and not a ceiling.
            let mut below = observed;
            below[axis] -= doubled;
            assert!(cc7_observation_within_tolerance(frame, index, below).is_err());
        }
    }
    // Non-vacuity: the tolerance is not zero, so "rejects a doubled offset" is
    // not "rejects everything".
    const {
        assert!(CC7_TRACK_TOLERANCE_BASIS_POINTS > 0);
    }
    assert_eq!(
        CC7_TRACK_SURVIVING_SAMPLE_FRAMES.len(),
        CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS.len() - 1,
        "the occluded sample is dropped before the accuracy gate sees it"
    );
}

/// CC7 §4(f)(1)'s failing direction, under the contract's own name.
///
/// The identical call at the tool's own
/// `DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS = 5 000` returns an
/// **empty** `low_confidence_samples`, so `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS`
/// is what dropped sample 47 and not the recipe: a floor the default would
/// have produced anyway is decoration.
///
/// §4.2's row states the claim "at 5 000 **and** at 7 000". The second floor
/// here is `CC7_TRACK_OCCLUDED_CONFIDENCE_ON_THE_RECIPE_REPORTED − 1`
/// (**7 348**) rather than a fresh `7_000` literal: it is derived from a
/// pinned constant instead of introducing an ungated number into a gate
/// (rule 11.0.1), and it is the **tightest** floor that still drops nothing,
/// so it implies the contract's round figure rather than merely matching it.
///
/// A named function asserted inside the (f) script: the call it makes is the
/// *identical* one the script has just made, against the same seeded window
/// on the same server, and "identical but for the floor" is a claim a second
/// session cannot make.
async fn cc7_f_the_default_floor_drops_no_sample<F>(
    client: &RunningService<RoleClient, ()>,
    track_arguments: &F,
) where
    F: Fn(i64, i64) -> serde_json::Value,
{
    let tightest_floor_that_drops_nothing =
        CC7_TRACK_OCCLUDED_CONFIDENCE_ON_THE_RECIPE_REPORTED - 1;
    for floor in [
        CC7_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS,
        tightest_floor_that_drops_nothing,
    ] {
        assert!(
            floor < CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
            "a floor at or above the CC7 one is not a failing direction"
        );
        let defaulted = invoke_capability(
            client,
            "track_matte_window",
            track_arguments(floor, CC7_TRACK_STEP_FRAMES),
        )
        .await;
        let defaulted_body = defaulted.structured_content.as_ref().unwrap();
        assert_eq!(defaulted.is_error, Some(false), "{defaulted_body}");
        assert!(
            defaulted_body["low_confidence_samples"]
                .as_array()
                .unwrap()
                .is_empty(),
            "a floor of {floor} drops nothing: {defaulted_body}"
        );
        assert_eq!(
            defaulted_body["observations"].as_array().unwrap().len(),
            cc7_tracking_sample_frames().len(),
            "every sample survives at {floor}: {defaulted_body}"
        );
    }
    assert_ne!(
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        CC7_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS
    );
    // The two floors bracket the occluded sample's confidence, which is the
    // whole reason the CC7 floor exists.
    const {
        assert!(
            CC7_TRACK_OCCLUDED_CONFIDENCE_ON_THE_RECIPE_REPORTED
                < CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS
        );
    }
}

/// CC7 §5.2 (f) — tracked secondary.
///
/// `plan_secondary_correction` seeds the window on frame 0's square →
/// `plan_primary_correction` writes the grade → `track_matte_window` over
/// `0..48` at `step_frames 5` drops **exactly** the occluded sample 47 →
/// prepare/commit → `inspect_grade_matte` at five sampled frames → the (f2)
/// two-sample range refuses `tracking_confidence_too_low`.
///
/// Every gate reads `observations[]`, never `curves` (A17): the smoothed
/// curve's final keyframe carries the tool's published `known_systematic_lag`.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn cc7_f_tracked_secondary_drops_only_the_occluded_samples() {
    let generated = cc7_tracked_source();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let base = single_clip_document(asset);
    let expected = cc7_canonical_document(
        &base,
        &cc7_canonical_operations(Cc7Scenario::TrackedSecondary),
    );
    let core = Core::spawn(base.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let revision = cc7_revision(&client).await;
    assert_eq!(revision, 0);

    // The seeded window: CC7 §2.3.6's `375 / 667` bp half extents on frame 0's
    // square. Its centre is the descriptor neutral `5 000 / 5 000`, which is
    // exactly frame 0's continuous centre, so it is resolved and not stored
    // (errata A-E4).
    let plan = invoke_capability(
        &client,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "node_kind": "primary_correction",
            "windows": [{
                "center_x": CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS[0],
                "center_y": CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS[1],
                "half_width": CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS,
                "half_height": CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS,
            }],
            "timecode": 0,
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false), "{:?}", plan.structured_content);
    let plan = plan.structured_content.as_ref().unwrap().clone();
    cc7_assert_evidence_only(&plan, "plan_secondary_correction");
    let effect_id = plan["target_effect_id"].as_u64().unwrap();
    // CC7 §5.1(1), R2 minor 9: planning applied nothing on the (f) leg either.
    assert_eq!(
        query_document(&core),
        base,
        "planning must apply nothing to the (f) document"
    );
    let prepared = prepare_plan(&client, revision, plan["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    assert_eq!(
        client
            .call_tool(commit_request(revision, &prepared))
            .await
            .unwrap()
            .is_error,
        Some(false)
    );
    assert_eq!(cc7_revision(&client).await, revision + 1);

    let grade = invoke_capability(
        &client,
        "plan_primary_correction",
        json!({
            "expected_revision": revision + 1,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "parameters": {"saturation_percent": CC7_SECONDARY_SATURATION_PERCENT},
        }),
    )
    .await;
    assert_eq!(
        grade.is_error,
        Some(false),
        "{:?}",
        grade.structured_content
    );
    let grade = grade.structured_content.as_ref().unwrap().clone();
    assert_eq!(grade["target_effect_id"].as_u64().unwrap(), effect_id);
    let prepared = prepare_plan(&client, revision + 1, grade["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    assert_eq!(
        client
            .call_tool(commit_request(revision + 1, &prepared))
            .await
            .unwrap()
            .is_error,
        Some(false)
    );
    let tracked_revision = revision + 2;
    assert_eq!(cc7_revision(&client).await, tracked_revision);
    // The document the tracker is handed, kept so §5.1(1) can be asserted
    // against it: `track_matte_window` is evidence-only and must not write.
    let graded = query_document(&core);

    // CC7 §5.5: a window index past the node's one active window is typed.
    let out_of_range = invoke_capability(
        &client,
        "track_matte_window",
        json!({
            "expected_revision": tracked_revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "effect_id": effect_id,
            "window_index": 4,
        }),
    )
    .await;
    assert_eq!(out_of_range.is_error, Some(true));
    assert_eq!(
        out_of_range.structured_content.as_ref().unwrap()["code"],
        "matte_window_index_out_of_range"
    );

    cc7_assert_stale_revision_prose(
        &client,
        &core,
        "track_matte_window",
        json!({
            "expected_revision": tracked_revision + 8,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "effect_id": effect_id,
            "window_index": 0,
        }),
        tracked_revision,
        tracked_revision + 8,
    )
    .await;

    let track_arguments = |floor: i64, step: i64| {
        json!({
            "expected_revision": tracked_revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "effect_id": effect_id,
            "window_index": 0,
            "start_local_frame": CC7_TRACK_RANGE_START_LOCAL_FRAME,
            "end_local_frame": CC7_TRACK_RANGE_END_LOCAL_FRAME,
            "step_frames": step,
            "search_radius_percent": CC7_TRACK_SEARCH_RADIUS_PERCENT,
            "max_width": CC7_TRACK_MAX_WIDTH,
            "minimum_confidence_basis_points": floor,
        })
    };

    // CC7 §4(f)(1): the floor drops exactly the occluded sample.
    let tracked = invoke_capability(
        &client,
        "track_matte_window",
        track_arguments(CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS, CC7_TRACK_STEP_FRAMES),
    )
    .await;
    let tracked_body = tracked.structured_content.as_ref().unwrap().clone();
    assert_eq!(tracked.is_error, Some(false), "{tracked_body}");
    assert_eq!(tracked_body["applied"], false, "{tracked_body}");
    assert_eq!(
        tracked_body["minimum_confidence_basis_points"],
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS
    );
    // CC7 §5.1(1), R2 minor 9: the tracker publishes a prepared plan and
    // writes nothing until it is committed.
    assert_eq!(
        query_document(&core),
        graded,
        "track_matte_window must apply nothing"
    );

    let low = tracked_body["low_confidence_samples"].as_array().unwrap();
    let low_frames = low
        .iter()
        .map(|sample| sample["local_frame"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        low_frames,
        CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES.to_vec(),
        "only the occluded sample may drop: {tracked_body}"
    );

    let observations = tracked_body["observations"].as_array().unwrap();
    let observed_frames = observations
        .iter()
        .map(|sample| sample["local_frame"].as_i64().unwrap())
        .collect::<Vec<_>>();
    let sample_frames = cc7_tracking_sample_frames();
    let surviving = sample_frames
        .iter()
        .copied()
        .filter(|frame| !CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES.contains(frame))
        .collect::<Vec<_>>();
    assert_eq!(
        observed_frames, surviving,
        "the tool's even-distribution rule gives CC7_TRACK_SAMPLE_FRAMES: {tracked_body}"
    );

    // R4-M1: the two pinned observation tables are indexed by position in
    // `sample_frames`, so their length is asserted before either is indexed.
    assert_eq!(
        sample_frames.len(),
        CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS.len()
    );
    assert_eq!(
        sample_frames.len(),
        CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS.len()
    );

    // CC7 §4(f)(2): every surviving observation is within CC5's tolerance of
    // the analytic centre — read from `observations[]`, in **layer** space.
    let mut worst = 0_i64;
    for sample in observations {
        let frame = sample["local_frame"].as_i64().unwrap();
        let index = sample_frames
            .iter()
            .position(|candidate| *candidate == frame)
            .expect("every observation is a contract sample frame");
        let observed = [
            sample["center_x_basis_points"].as_i64().unwrap(),
            sample["center_y_basis_points"].as_i64().unwrap(),
        ];
        // The gate itself is `cc7_observation_within_tolerance`, so that
        // §4(f)(2)'s failing direction —
        // `cc7_f_observation_gate_rejects_a_doubled_offset` — drives this
        // code path and not a second copy of it.
        worst = worst.max(
            cc7_observation_within_tolerance(frame, index, observed).unwrap_or_else(|rejection| {
                panic!("CC7 §4(f)(2): {rejection:?}: {sample}");
            }),
        );
        assert!(
            sample["confidence_basis_points"].as_i64().unwrap()
                >= CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS
        );
        // CC7 §5.1(4), R4-M1: the analytic gate above is a 200 bp tolerance and
        // cannot see a systematic drift smaller than that.
        // `CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS` and
        // `CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS` are compared against the
        // live `track_matte_window` **nowhere else in the workspace** — media's
        // containment fixture reads the table and is therefore a pure function
        // of it — so a tracker that moved every observation 150 bp would leave
        // both gates green. R-M8 permits these two tables as regression pins;
        // this is where they are taken, exactly, against the shipped tracker.
        assert_eq!(
            [
                sample["center_x_basis_points"].as_i64().unwrap(),
                sample["center_y_basis_points"].as_i64().unwrap(),
            ],
            CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS[index],
            "frame {frame} is not the pinned observed centre: {sample}"
        );
        assert_eq!(
            sample["confidence_basis_points"].as_i64().unwrap(),
            CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS[index],
            "frame {frame} is not the pinned observed confidence: {sample}"
        );
    }
    eprintln!(
        "CC7 (f) measured: worst raw observation error {worst} bp; occluded confidence {}",
        low[0]["confidence_basis_points"]
    );
    assert!(
        low[0]["confidence_basis_points"].as_i64().unwrap() < CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS
    );
    // R4-M1: the dropped sample is the eleventh row of both tables — the
    // frozen pre-occlusion position and the confidence that fails the floor.
    let occluded = sample_frames.len() - 1;
    assert_eq!(
        [
            low[0]["center_x_basis_points"].as_i64().unwrap(),
            low[0]["center_y_basis_points"].as_i64().unwrap(),
        ],
        CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS[occluded],
        "the occluded sample is not the pinned frozen centre: {}",
        low[0]
    );
    assert_eq!(
        low[0]["confidence_basis_points"].as_i64().unwrap(),
        CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS[occluded],
        "the occluded sample is not the pinned confidence: {}",
        low[0]
    );

    cc7_f_the_default_floor_drops_no_sample(&client, &track_arguments).await;

    // Commit the tracker's own prepared plan: `track_matte_window` publishes a
    // `prepared_edit_plan`, not a bare operation list.
    let plan_id = tracked_body["prepared_edit_plan"]["plan_id"].clone();
    let committed = client
        .call_tool(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({"plan_id": plan_id, "expected_revision": tracked_revision})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        committed.is_error,
        Some(false),
        "{:?}",
        committed.structured_content
    );
    assert_eq!(cc7_revision(&client).await, tracked_revision + 1);
    assert_eq!(
        query_document(&core),
        expected,
        "the committed document must equal cc7_canonical_operations(TrackedSecondary)"
    );

    // CC7 §4(f)(3), R4-M1: the committed curves, keyframe by keyframe, against
    // `cc7_track_keyframe_centres(axis)` — the smoother's own output over the
    // pinned observations. The whole-document equality above covers this as a
    // `Document` diff; taken here by name, a tracker regression is reported as
    // "curve X keyframe i" rather than as a two-thousand-line struct mismatch,
    // and the derivation table gets a second, explicit live comparison.
    let tracked_document = query_document(&core);
    let tracked_effects = &tracked_document.tracks[0].clips[0].effects;
    assert_eq!(tracked_effects.len(), 1, "{tracked_effects:?}");
    for (axis, name) in CC7_F_KEYFRAMED_PARAMETERS.into_iter().enumerate() {
        let curve = tracked_effects[0]
            .keyframes
            .get(name)
            .unwrap_or_else(|| panic!("the (f) commit must write a {name} curve"));
        let smoothed = cc7_track_keyframe_centres(axis);
        assert_eq!(
            curve.keyframes.len(),
            smoothed.len(),
            "{name} must carry one keyframe per surviving sample"
        );
        for (index, keyframe) in curve.keyframes.iter().enumerate() {
            assert_eq!(
                keyframe.at.0, CC7_TRACK_SURVIVING_SAMPLE_FRAMES[index],
                "{name} keyframe {index} is at the wrong frame"
            );
            assert_eq!(
                keyframe.value, smoothed[index],
                "{name} keyframe {index} is not the smoother's value for the pinned observations"
            );
        }
    }

    // CC7 §5.1(5), R2-MAJ-2: the tracked node, re-read from
    // `get_color_context`'s `color_nodes` manifest. The tracker writes two
    // **curves** and no static centre, so the manifest — which reports the
    // stored static values for this metadata-only surface
    // (`color_status.rs:3111-3114`) — must still publish the seeded window at
    // its neutral centre and the contract's own half extents. A manifest that
    // silently flattened the curve into the static centre, or a commit that
    // lost the window, fails here.
    let context = invoke_capability(&client, "get_color_context", json!({})).await;
    let context = context.structured_content.as_ref().unwrap();
    let manifest_nodes = context["clips"][0]["color_nodes"].as_array().unwrap();
    assert_eq!(manifest_nodes.len(), 1, "{context}");
    let node = &manifest_nodes[0];
    assert_eq!(node["kind"], "primary_correction", "{node}");
    assert_eq!(
        node["parameters"]["saturation_percent"], CC7_SECONDARY_SATURATION_PERCENT,
        "{node}"
    );
    assert_eq!(node["matte"]["enabled"], true, "{node}");
    assert_eq!(node["matte"]["window_count"], 1, "{node}");
    assert_eq!(node["matte"]["qualifier"]["enabled"], false, "{node}");
    let windows = node["matte"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1, "{node}");
    assert_eq!(
        windows[0]["half_width_basis_points"], CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS,
        "{node}"
    );
    assert_eq!(
        windows[0]["half_height_basis_points"], CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        "{node}"
    );
    assert_eq!(
        windows[0]["center_x_basis_points"], CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS[0],
        "{node}"
    );
    assert_eq!(
        windows[0]["center_y_basis_points"], CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS[1],
        "{node}"
    );

    // CC7 §5.2 (f): the matte is inspectable at the sampled frames it moved
    // through. R2 minor 10: §5.2's five frames are **indexed out of**
    // `CC7_TRACK_SAMPLE_FRAMES` rather than restated as literals — positions
    // 0, 2, 4 and 6, plus the last surviving sample at position 9.
    let gpu_may_skip = std::env::var("KINEWRIGHT_GPU_TESTS_MAY_SKIP")
        .ok()
        .as_deref()
        == Some("1");
    for frame in [0_usize, 2, 4, 6, 9].map(|index| sample_frames[index]) {
        let inspect = invoke_capability(
            &client,
            "inspect_grade_matte",
            json!({
                "expected_revision": tracked_revision + 1,
                "clip_id": CC7_SINGLE_CLIP_ID.0,
                "effect_id": effect_id,
                "timecode": frame,
            }),
        )
        .await;
        let body = inspect.structured_content.as_ref().unwrap();
        if inspect.is_error == Some(true) {
            assert!(
                gpu_may_skip,
                "inspect_grade_matte refused: {body}. Set KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 to \
                 accept an unavailable matte proof on a machine with no usable adapter."
            );
            assert_eq!(body["code"], "matte_proof_unavailable");
            assert_eq!(body["applied"], false);
            eprintln!(
                "SKIPPED: KINEWRIGHT_GPU_TESTS_MAY_SKIP=1 and this build cannot render a matte \
                 proof; scenario (f)'s coverage at frame {frame} was not measured."
            );
            continue;
        }
        let statistics = &body["statistics"];
        let total = statistics["total_pixel_count"].as_u64().unwrap();
        let covered = statistics["covered_pixel_count"].as_u64().unwrap();
        assert!(covered > 0, "frame {frame} covered nothing: {statistics}");
        assert!(
            covered < total,
            "frame {frame} covered the whole raster: {statistics}"
        );
    }

    // CC7 §4(f)(4): (f2), the two-sample total loss.
    let refused = invoke_capability(
        &client,
        "track_matte_window",
        json!({
            "expected_revision": tracked_revision + 1,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "effect_id": effect_id,
            "window_index": 0,
            "start_local_frame": CC7_TRACK_RANGE_START_LOCAL_FRAME,
            "end_local_frame": CC7_TRACK_RANGE_END_LOCAL_FRAME,
            "step_frames": CC7_TRACK_F2_STEP_FRAMES,
            "search_radius_percent": CC7_TRACK_SEARCH_RADIUS_PERCENT,
            "max_width": CC7_TRACK_MAX_WIDTH,
            "minimum_confidence_basis_points": CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        }),
    )
    .await;
    assert_eq!(refused.is_error, Some(true));
    let refused_body = refused.structured_content.as_ref().unwrap();
    assert_eq!(
        refused_body["code"], "tracking_confidence_too_low",
        "{refused_body}"
    );
    assert_eq!(refused_body["applied"], false, "{refused_body}");
    assert_eq!(refused_body["evidence_only"], true, "{refused_body}");
    let details = &refused_body["details"];
    assert_eq!(details["field"], "minimum_confidence_basis_points");
    assert_eq!(details["observed"]["surviving_samples"], 1, "{details}");
    assert_eq!(
        details["observed"]["total_samples"],
        CC7_TRACK_F2_SAMPLE_FRAMES.len(),
        "{details}"
    );
    assert_eq!(
        details["observed"]["minimum_confidence_basis_points"],
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS
    );
    assert_eq!(
        details["allowed"]["minimum_surviving_samples"], CC7_MATTE_TRACK_MINIMUM_SAMPLES,
        "{details}"
    );
    assert_eq!(
        details["observed"]["low_confidence_samples"][0]["local_frame"],
        CC7_TRACK_F2_SAMPLE_FRAMES[1]
    );
    assert!(
        details["recovery_action"]
            .as_str()
            .is_some_and(|action| action.contains("minimum_confidence_basis_points")),
        "{details}"
    );
    eprintln!(
        "CC7 (f2) measured: occluded confidence {}",
        details["observed"]["low_confidence_samples"][0]["confidence_basis_points"]
    );

    // CC7 §4(f)(4)'s second direction: the same call at the 5 000 default does
    // **not** refuse, so the floor is what produced the refusal.
    let permissive = invoke_capability(
        &client,
        "track_matte_window",
        json!({
            "expected_revision": tracked_revision + 1,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "effect_id": effect_id,
            "window_index": 0,
            "start_local_frame": CC7_TRACK_RANGE_START_LOCAL_FRAME,
            "end_local_frame": CC7_TRACK_RANGE_END_LOCAL_FRAME,
            "step_frames": CC7_TRACK_F2_STEP_FRAMES,
            "search_radius_percent": CC7_TRACK_SEARCH_RADIUS_PERCENT,
            "max_width": CC7_TRACK_MAX_WIDTH,
            "minimum_confidence_basis_points":
                CC7_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS,
        }),
    )
    .await;
    let permissive_body = permissive.structured_content.as_ref().unwrap();
    assert_eq!(permissive.is_error, Some(false), "{permissive_body}");
    assert_eq!(
        permissive_body["observations"].as_array().unwrap().len(),
        CC7_TRACK_F2_SAMPLE_FRAMES.len(),
        "at 5000 both (f2) samples survive: {permissive_body}"
    );

    // Nothing after the one tracked commit moved the timeline.
    assert_eq!(cc7_revision(&client).await, tracked_revision + 1);
    client.cancel().await.unwrap();
    server.shutdown();
}

/// CC7 §4(f)(4), §11.2.28's agent half: the **(f2) failing direction**.
///
/// The identical two-sample range at the tool's own
/// `DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS = 5 000` does **not**
/// refuse, so `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS` is what produced
/// `cc7_f_tracked_secondary_drops_only_the_occluded_samples`' refusal rather
/// than the recipe alone. Stated as its own test because a floor nobody can
/// show is load-bearing is decoration.
#[tokio::test(flavor = "multi_thread")]
async fn cc7_f2_the_default_floor_does_not_refuse() {
    let generated = cc7_tracked_source();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let revision = cc7_revision(&client).await;
    let plan = invoke_capability(
        &client,
        "plan_secondary_correction",
        json!({
            "expected_revision": revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "node_kind": "primary_correction",
            "windows": [{
                "center_x": CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS[0],
                "center_y": CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS[1],
                "half_width": CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS,
                "half_height": CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS,
            }],
            "timecode": 0,
        }),
    )
    .await;
    assert_eq!(plan.is_error, Some(false), "{:?}", plan.structured_content);
    let plan = plan.structured_content.as_ref().unwrap().clone();
    let effect_id = plan["target_effect_id"].as_u64().unwrap();
    let prepared = prepare_plan(&client, revision, plan["operations"].clone()).await;
    assert_eq!(prepared.is_error, Some(false));
    assert_eq!(
        client
            .call_tool(commit_request(revision, &prepared))
            .await
            .unwrap()
            .is_error,
        Some(false)
    );
    let tracked_revision = revision + 1;

    let f2 = |floor: i64| {
        json!({
            "expected_revision": tracked_revision,
            "clip_id": CC7_SINGLE_CLIP_ID.0,
            "effect_id": effect_id,
            "window_index": 0,
            "start_local_frame": CC7_TRACK_RANGE_START_LOCAL_FRAME,
            "end_local_frame": CC7_TRACK_RANGE_END_LOCAL_FRAME,
            "step_frames": CC7_TRACK_F2_STEP_FRAMES,
            "search_radius_percent": CC7_TRACK_SEARCH_RADIUS_PERCENT,
            "max_width": CC7_TRACK_MAX_WIDTH,
            "minimum_confidence_basis_points": floor,
        })
    };

    let refused = invoke_capability(
        &client,
        "track_matte_window",
        f2(CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS),
    )
    .await;
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(
        refused.structured_content.as_ref().unwrap()["code"],
        "tracking_confidence_too_low"
    );

    let permitted = invoke_capability(
        &client,
        "track_matte_window",
        f2(CC7_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS),
    )
    .await;
    let permitted_body = permitted.structured_content.as_ref().unwrap();
    assert_eq!(permitted.is_error, Some(false), "{permitted_body}");
    assert!(
        permitted_body["low_confidence_samples"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the 5000 default drops neither (f2) sample: {permitted_body}"
    );
    assert_eq!(
        permitted_body["observations"].as_array().unwrap().len(),
        CC7_TRACK_F2_SAMPLE_FRAMES.len()
    );

    // Neither call moved the timeline: both are evidence-only.
    assert_eq!(cc7_revision(&client).await, tracked_revision);
    client.cancel().await.unwrap();
    server.shutdown();
}
