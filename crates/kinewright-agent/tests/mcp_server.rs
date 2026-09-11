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
                audio_gain_curve: None,
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
        CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED, CC7_C2_OVER_RANGE_PIXELS_REPORTED,
        CC7_CANDIDATE_CLIP_ID, CC7_CHART_BAND_ROI, CC7_DEEP_SHADOW_RECT, CC7_DEEP_SHADOW_ROI,
        CC7_F_KEYFRAMED_PARAMETERS, CC7_LOG_CUBE_SIZE, CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
        CC7_LOG_P99_MAX_CODE16, CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        CC7_LOOK_MIX_BASIS_POINTS, CC7_LUT_ASSET_ID, CC7_MATCH_PROPOSAL_B, CC7_MATCH_PROPOSAL_C1,
        CC7_MATCH_PROPOSAL_C2, CC7_PRODUCT_PATCH_PIXEL_COUNT, CC7_PRODUCT_RED_ROI,
        CC7_REFERENCE_CLIP_ID, CC7_SECONDARY_SATURATION_PERCENT, CC7_SINGLE_CLIP_ID,
        CC7_SKIN_BAND_ROI, CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS,
        CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS, CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES,
        CC7_TRACK_F2_SAMPLE_FRAMES, CC7_TRACK_F2_STEP_FRAMES, CC7_TRACK_MAX_WIDTH,
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS, CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS,
        CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS, CC7_TRACK_RANGE_END_LOCAL_FRAME,
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

/// CC7 §5.4: CC7 added no tool, so the served surface stays byte-for-byte
/// what CC6 published. AU1 §6.2 adds two internal tools — the generated
/// `set_track_mix` mutator and the `get_audio_levels` inspector — so the
/// registry counts move while the served seven do not. AU2 §4.2 Part A added
/// no tool at all: the registry stayed at 126 with 76 inspectors and only
/// descriptions grew, by the +8,165 B the registry figure records.
///
/// AU2 §6.4 Part B adds three: the generated `set_audio_master` and
/// `set_pan_law` mutators and the `get_audio_spectrum` inspector, so 52
/// generated operations + 77 inspectors = 129. The served seven still do not
/// move a byte, because none of them embeds the `Operation` schema and none
/// of the three is served.
///
/// AU3 §4.2 Part A (A16) adds one: the `get_audio_qc` inspector, registered
/// directly after `get_audio_spectrum`, so 52 + 78 = 130. Served: unchanged.
///
/// AU3 §6.4 Part B (B13) adds none — it grows `queue_export`'s arguments by one
/// boolean and rewrites three descriptions — so the counts hold at 52 + 78 and
/// the served quad is byte-identical for the eighth consecutive measurement.
///
/// AU4 §4.3 Part A (A19) adds two generated mutators, `set_clip_gain_envelope`
/// and `set_track_automation`, so 54 + 78 = 132. `INSPECTOR_TOOL_NAMES` does
/// not move in Part A, and neither does the served quad: the two new tools are
/// registry-only and the seven served tools embed no `Operation` schema, so
/// this is the ninth consecutive byte-identical measurement.
///
/// AU4 §6.3 Part B (B13) adds the two planners `plan_audio_ducking` and
/// `plan_clip_fades` to `INSPECTOR_TOOL_NAMES`, so 54 + 80 = 134 with the
/// generated count unchanged. The served quad does not move for the tenth
/// consecutive measurement: a planner is registry-only, reached through
/// `invoke_capability`, whose argument schema is generic.
///
/// AU5 §4.3 Part A (A18) adds one hand-written capability, the
/// `get_audio_repair` inspector, registered directly after `get_audio_qc`, so
/// 54 + 81 = 135 with the generated count unchanged: AU5 adds no `Operation`
/// variant, so `UNGENERATED_OPERATION_VARIANTS` and the 54 generated mutators
/// hold. The three new effect descriptors reach no input schema at all — the
/// `Operation` schema embeds `Effect.parameters` as an untyped map — so their
/// whole registry cost is `effect_documentation()`'s rows on the five spliced
/// effect tools, hatched down to one pattern sentence by §4.2 rule 78. The
/// served quad does not move for the eleventh consecutive measurement.
///
/// AU5 §5.9 Part B (B12) adds three hand-written capabilities — the two
/// planners `plan_dialogue_repair` and `plan_room_tone_fill`, and the
/// `capture_room_tone` Action — so 54 + 84 = 138, again with the generated
/// count unchanged, because Part B adds no `Operation` variant either: the
/// capture submits an ordinary `AddAsset`. It adds no effect descriptor
/// either, so `effect_documentation()` does not move a byte and R84's pattern
/// sentence naming `plan_dialogue_repair` — shipped in Part A, three commits
/// before the planner existed — is deliberately left exactly as it was. The
/// served quad does not move for the twelfth consecutive measurement.
///
/// AU6 §5.4 Part A adds **no** capability at all — it is an evaluation slice —
/// so the registry holds at 54 + 84 = 138 and the generated count cannot move:
/// AU6 adds no `Operation` variant, no tool and no effect descriptor. The one
/// product change Part A ships is an app-side `MixerChain` arm that emits the
/// **existing** `SetTrackAutomation`, which reaches no schema, and which does
/// not touch core's `AudioChain`. The served quad does not move for the
/// thirteenth consecutive measurement.
///
/// AU6 §7 Part B adds none either: the seventh eval suite, its assertion
/// variants and its audio evidence block all live in
/// `crates/kinewright-agent/src/eval.rs` and `src/bin/kinewright-eval.rs`,
/// neither of which is a capability. The served quad does not move for the
/// fourteenth consecutive measurement.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
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
    assert_eq!(
        tools.len(),
        7,
        "no part of AU1, AU2, AU3, AU4, AU5, or AU6 adds a served tool"
    );
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        kinewright_agent::compact_tool_names()
    );

    // The internal registry: 138 tools, of which `INSPECTOR_TOOL_NAMES` is 84.
    let registry = kinewright_agent::capability_tool_names().unwrap();
    let operations = kinewright_agent::operation_tools().unwrap();
    assert_eq!(
        registry.len(),
        138,
        "AU1 adds set_track_mix and get_audio_levels; AU2 Part A adds no tool; \
         AU2 Part B adds set_audio_master, set_pan_law and get_audio_spectrum; \
         AU3 Part A adds get_audio_qc; AU3 Part B adds none; \
         AU4 Part A adds set_clip_gain_envelope and set_track_automation; \
         AU4 Part B adds plan_audio_ducking and plan_clip_fades; \
         AU5 Part A adds get_audio_repair; \
         AU5 Part B adds plan_dialogue_repair, capture_room_tone and plan_room_tone_fill; \
         AU6 §5.4 Part A adds no capability at all"
    );
    assert_eq!(
        operations.len(),
        54,
        "AU2 Part B generates two more mutators; neither part of AU3 generates one; \
         AU4 Part A generates two more; AU4 Part B generates none; \
         AU5 Part A generates none, because it adds no Operation variant; \
         AU5 Part B generates none either, because capture_room_tone submits an ordinary AddAsset; \
         AU6 adds no Operation variant"
    );
    for name in [
        "set_track_mix",
        "get_audio_levels",
        "set_audio_master",
        "set_pan_law",
        "get_audio_spectrum",
        "get_audio_qc",
        "set_clip_gain_envelope",
        "set_track_automation",
        "plan_audio_ducking",
        "plan_clip_fades",
        "get_audio_repair",
        "plan_dialogue_repair",
        "capture_room_tone",
        "plan_room_tone_fill",
    ] {
        assert!(registry.iter().any(|entry| entry == name), "missing {name}");
    }
    assert_eq!(
        registry.len() - operations.len(),
        84,
        "AU1 adds get_audio_levels; AU2 Part B adds get_audio_spectrum; \
         AU3 Part A adds get_audio_qc; AU3 Part B adds no inspector; \
         AU4 Part A adds no inspector; AU4 Part B adds the two planners; \
         AU5 Part A adds get_audio_repair; AU5 Part B adds all three of its capabilities; \
         AU6 §5.4 Part A and Part B add none"
    );
    let spectrum = registry
        .iter()
        .position(|entry| entry == "get_audio_spectrum")
        .unwrap();
    assert_eq!(
        registry.get(spectrum + 1).map(String::as_str),
        Some("get_audio_qc"),
        "AU3 §4.2: get_audio_qc is registered directly after get_audio_spectrum"
    );
    // AU5 §4.3 rule 81: the repair inspector joins the audio evidence family
    // at its end, so the three measurement surfaces stay adjacent and a fourth
    // one appended anywhere else would fail here.
    let qc = registry
        .iter()
        .position(|entry| entry == "get_audio_qc")
        .unwrap();
    assert_eq!(
        registry.get(qc + 1).map(String::as_str),
        Some("get_audio_repair"),
        "AU5 §4.3: get_audio_repair is registered directly after get_audio_qc"
    );
    // AU4 §4.3: each new mutator is generated directly after the scalar tool
    // whose owner it automates, because `operation_tools` follows `Operation`
    // declaration order and the two variants were declared there.
    for (scalar, curve) in [
        ("set_track_mix", "set_track_automation"),
        ("set_clip_audio", "set_clip_gain_envelope"),
    ] {
        let index = registry.iter().position(|entry| entry == scalar).unwrap();
        assert_eq!(
            registry.get(index + 1).map(String::as_str),
            Some(curve),
            "AU4 §4.3: {curve} is registered directly after {scalar}"
        );
    }
    // AU4 §6.3 rule 134: the two Part B planners keep the audio family's
    // alphabetical order around `plan_audio_normalization` — the ordering
    // assert is extended, never relaxed.
    let normalization = registry
        .iter()
        .position(|entry| entry == "plan_audio_normalization")
        .unwrap();
    assert_eq!(
        registry.get(normalization - 1).map(String::as_str),
        Some("plan_audio_ducking"),
        "AU4 §6.3: plan_audio_ducking is registered directly before plan_audio_normalization"
    );
    assert_eq!(
        registry.get(normalization + 1).map(String::as_str),
        Some("plan_clip_fades"),
        "AU4 §6.3: plan_clip_fades is registered directly after plan_audio_normalization"
    );
    // AU5 §5.9 rule 117: the ordering assert is EXTENDED, never relaxed. The
    // three Part B capabilities follow the audio family in one run, with
    // `capture_room_tone` directly before the planner that consumes what it
    // writes, so appending a fourth anywhere else fails here.
    for (before, after) in [
        ("plan_clip_fades", "plan_dialogue_repair"),
        ("plan_dialogue_repair", "capture_room_tone"),
        ("capture_room_tone", "plan_room_tone_fill"),
    ] {
        let index = registry.iter().position(|entry| entry == before).unwrap();
        assert_eq!(
            registry.get(index + 1).map(String::as_str),
            Some(after),
            "AU5 §5.9: {after} is registered directly after {before}"
        );
    }

    // The served byte counts CC6 recorded, asserted byte-identically: no AU1,
    // AU2, AU3, or AU4 tool is served, and the seven served tools do not embed
    // the `Operation` schema, so neither the generated mutators nor the new
    // audio descriptor rows, prose, and QC schema reach them. AU3 Part B
    // (§6.4/B13) moves the registry by 783 B — `QueueExportArgs`'
    // `normalize_loudness` boolean and three rewritten descriptions — and none
    // of it is served: `queue_export`, `get_export_jobs` and
    // `plan_audio_normalization` are all registry-only tools. AU4 Part A
    // (§4.3/A19) moves it by 93,394 B — two generated mutators and 820 B of
    // shared `$defs` field growth on every tool that embeds `Operation` — and
    // none of that is served either, for the same structural reason.
    let metrics = server.tool_surface_metrics();
    assert_eq!(metrics.tool_count, 7);
    assert_eq!(metrics.serialized_bytes, 5_660, "{metrics:?}");
    assert_eq!(metrics.input_schema_bytes, 3_510, "{metrics:?}");
    assert_eq!(metrics.description_bytes, 998, "{metrics:?}");

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU1 §7 item 21: the generated `set_track_mix` mutator survives the whole
/// agent round trip — plan preparation, commit, and the compact state
/// rendering — and a neutral set removes both the entry and the suffix.
#[tokio::test(flavor = "multi_thread")]
async fn au1_set_track_mix_round_trips_through_edit_plans_and_state() {
    let core = Core::spawn(edit_plan_document()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let prepared = prepare_plan(
        &client,
        0,
        json!([{
            "op": "set_track_mix",
            "track": 1,
            "gain_tenth_db": -60,
            "pan_percent": 25,
            "mute": false,
            "solo": true
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let document = query_document(&core);
    assert_eq!(document.audio_mix.tracks.len(), 1);
    assert_eq!(document.track_mix(TrackId(1)).gain_tenth_db, -60);
    assert_eq!(document.track_mix(TrackId(1)).pan_percent, 25);
    assert!(document.track_mix(TrackId(1)).solo);

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = &state.content[0].as_text().unwrap().text;
    assert!(
        text.contains(
            "track 1 video sync_lock=true clips=1 mix=gain:-60,pan:25,mute:false,solo:true"
        ),
        "{text}"
    );

    // A neutral set removes the entry, so the suffix disappears again.
    let prepared = prepare_plan(
        &client,
        1,
        json!([{
            "op": "set_track_mix",
            "track": 1,
            "gain_tenth_db": 0,
            "pan_percent": 0,
            "mute": false,
            "solo": false
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");
    assert!(query_document(&core).audio_mix.tracks.is_empty());

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = &state.content[0].as_text().unwrap().text;
    assert!(
        text.contains("track 1 video sync_lock=true clips=1\n"),
        "{text}"
    );
    assert!(!text.contains("mix=gain:"), "{text}");

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU1 §7 item 21: `get_audio_levels` measured end to end on generated 440 Hz
/// sine media.
///
/// This test deliberately exercises `Analysis::mix_levels` on the REAL
/// `FfmpegMediaEngine` — no stub, no double — so it is the agent-side proof
/// that the measurement reaches the media engine's track stage. The engine
/// implements the facet in `engine.rs` by delegating to
/// `export::measure_mix_levels`; a `NotImplemented` failure on the first
/// assertion means that impl was lost, not that the measurement is a stub.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au1_get_audio_levels_measures_the_real_mix() {
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
    let generated = GeneratedMedia::ffmpeg("au1-levels", &arguments, "mp4");
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();

    // Two tracks carrying the same sine so the report has a track to mute and
    // a track to attenuate independently.
    let mut document = single_clip_document(asset.clone());
    document.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Audio,
        sync_lock: true,
        clips: vec![Clip {
            id: ClipId(2),
            asset: asset.id,
            source_range: TimeCode::ZERO..asset.duration,
            content: kinewright_core::ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
            audio_gain_curve: None,
        }],
    });
    let duration = document.duration.0;
    assert!(
        duration > 2,
        "the fixture needs a measurable span: {duration}"
    );
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let baseline = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(
        baseline.is_error,
        Some(false),
        "get_audio_levels must measure the real mix: {baseline:?}"
    );
    let report = &baseline
        .structured_content
        .as_ref()
        .expect("get_audio_levels must publish the machine-readable report")["report"];
    assert_eq!(report["tracks"].as_array().unwrap().len(), 2);
    assert_eq!(report["any_solo"], false);
    let before = report["tracks"][0]["levels"]["integrated_lufs_hundredths"]
        .as_i64()
        .expect("the unmuted sine track must measure a programme loudness");
    assert!(
        report["tracks"][1]["levels"]["integrated_lufs_hundredths"].is_i64(),
        "{report}"
    );

    // -60 tenth-dB is -6 dB, so the track stem must fall by 600 hundredths.
    let prepared = prepare_plan(
        &client,
        0,
        json!([{
            "op": "set_track_mix",
            "track": 1,
            "gain_tenth_db": -60,
            "pan_percent": 0,
            "mute": false,
            "solo": false
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let attenuated = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(attenuated.is_error, Some(false), "{attenuated:?}");
    let report = &attenuated.structured_content.as_ref().unwrap()["report"];
    let after = report["tracks"][0]["levels"]["integrated_lufs_hundredths"]
        .as_i64()
        .expect("an attenuated sine is still not silent");
    assert!(
        (after - before + 600).abs() <= 5,
        "a -60 tenth-dB track gain must move the stem by -600 LUFS hundredths, \
         measured {before} -> {after}"
    );

    // A muted track reads null: its post-stage stem is exact silence.
    let prepared = prepare_plan(
        &client,
        1,
        json!([{
            "op": "set_track_mix",
            "track": 2,
            "gain_tenth_db": 0,
            "pan_percent": 0,
            "mute": true,
            "solo": false
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let muted = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(muted.is_error, Some(false), "{muted:?}");
    let report = &muted.structured_content.as_ref().unwrap()["report"];
    assert_eq!(report["tracks"][1]["mix"]["mute"], true);
    assert_eq!(report["tracks"][1]["audible"], false);
    assert!(
        report["tracks"][1]["levels"]["integrated_lufs_hundredths"].is_null(),
        "a muted track must read none: {report}"
    );
    let text = &muted.content[0].as_text().unwrap().text;
    assert!(
        text.contains("mute=true") && text.contains("lufs=none"),
        "{text}"
    );

    // An inverted range is refused rather than clamped.
    let inverted = invoke_capability(
        &client,
        "get_audio_levels",
        json!({"start_frame": 30, "end_frame": 10}),
    )
    .await;
    assert_eq!(inverted.is_error, Some(true), "{inverted:?}");

    // AU1 §6.2: one bound given fills the other. `start_frame` alone runs to
    // the timeline duration; `end_frame` alone starts at frame 0. The echoed
    // `report.range` is the proof.
    let from_start = invoke_capability(
        &client,
        "get_audio_levels",
        json!({"start_frame": duration / 2}),
    )
    .await;
    assert_eq!(from_start.is_error, Some(false), "{from_start:?}");
    let range = &from_start.structured_content.as_ref().unwrap()["report"]["range"];
    assert_eq!(range["start"], json!(duration / 2), "{range}");
    assert_eq!(range["end"], json!(duration), "{range}");

    let to_end = invoke_capability(
        &client,
        "get_audio_levels",
        json!({"end_frame": duration / 2}),
    )
    .await;
    assert_eq!(to_end.is_error, Some(false), "{to_end:?}");
    let range = &to_end.structured_content.as_ref().unwrap()["report"]["range"];
    assert_eq!(range["start"], json!(0), "{range}");
    assert_eq!(range["end"], json!(duration / 2), "{range}");

    // AU1 §6.1: a range past the end is clamped by the media side, not
    // rejected — unlike the sibling transcript/silence range helper.
    let beyond = invoke_capability(
        &client,
        "get_audio_levels",
        json!({"start_frame": 0, "end_frame": duration + 100_000}),
    )
    .await;
    assert_eq!(beyond.is_error, Some(false), "{beyond:?}");
    let report = &beyond.structured_content.as_ref().unwrap()["report"];
    assert_eq!(report["range"]["start"], json!(0), "{report}");
    assert_eq!(report["range"]["end"], json!(duration), "{report}");
    let text = &beyond.content[0].as_text().unwrap().text;
    assert!(
        text.contains(&format!("mix_levels range=0..{duration} ")),
        "{text}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU2 §7 item B17: the two Part B mutators survive the whole agent round
/// trip — plan preparation, commit, and the compact state rendering — beside
/// a bus carrying the new fader, and a neutral set removes every line again.
///
/// The AU1 sibling above is
/// `au1_set_track_mix_round_trips_through_edit_plans_and_state`; this test
/// mirrors its shape.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au2_set_audio_master_and_pan_law_round_trip_through_edit_plans_and_state() {
    let core = Core::spawn(edit_plan_document()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // One plan, all three Part B edits: a bus with a non-zero fader, a master
    // chain, and the constant-power law.
    let prepared = prepare_plan(
        &client,
        0,
        json!([
            {
                "op": "upsert_audio_bus",
                "bus": {
                    "id": 1,
                    "name": "Dialogue",
                    "tracks": [1],
                    "gain_tenth_db": -35
                }
            },
            {
                "op": "set_audio_master",
                "master": {
                    "gain_tenth_db": 15,
                    "effects": [{
                        "id": 9,
                        "name": "audio_true_peak_limiter",
                        "parameters": {"ceiling_tenth_db": -10}
                    }]
                }
            },
            {"op": "set_pan_law", "law": "constant_power"}
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let document = query_document(&core);
    assert_eq!(document.audio_mix.master.gain_tenth_db, 15);
    assert_eq!(document.audio_mix.master.effects.len(), 1);
    assert!(!document.audio_mix.master.is_neutral());
    assert_eq!(
        document.audio_mix.pan_law,
        kinewright_core::PanLaw::ConstantPower
    );
    assert_eq!(
        document
            .audio_mix
            .bus(kinewright_core::AudioBusId(1))
            .expect("the bus must be stored")
            .gain_tenth_db,
        -35
    );

    // AU2 §6.3: the pan law first, then the buses, then the master.
    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = &state.content[0].as_text().unwrap().text;
    assert!(
        text.contains(
            "audio_pan_law=constant_power\naudio_buses:\n  audio_bus 1 \"Dialogue\" tracks=1 gain=-35 sidechain=none effects=none\naudio_master gain=15 effects=[9:audio_true_peak_limiter(ceiling_tenth_db=-10)]"
        ),
        "{text}"
    );

    // A neutral set removes the master and returns the law to balance, so
    // both lines disappear again.
    let prepared = prepare_plan(
        &client,
        1,
        json!([
            {"op": "set_audio_master", "master": {"gain_tenth_db": 0, "effects": []}},
            {"op": "set_pan_law", "law": "balance"},
            {
                "op": "upsert_audio_bus",
                "bus": {"id": 1, "name": "Dialogue", "tracks": [1], "gain_tenth_db": 0}
            }
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let document = query_document(&core);
    assert!(document.audio_mix.master.is_neutral());
    assert!(document.audio_mix.pan_law.is_balance());

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = &state.content[0].as_text().unwrap().text;
    assert!(
        text.contains("  audio_bus 1 \"Dialogue\" tracks=1 sidechain=none effects=none"),
        "{text}"
    );
    assert!(!text.contains("audio_pan_law"), "{text}");
    assert!(!text.contains("audio_master"), "{text}");
    assert!(!text.contains("gain="), "{text}");

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU4 §4.1: one rendered `[at:value:Interp,...]` list turned back into the
/// JSON curve a mutator takes, so a test can prove the agent can read a curve
/// out of `get_timeline_state` and write it straight back (rule 34).
fn curve_from_rendered(rendered: &str) -> serde_json::Value {
    let keyframes = rendered
        .split(',')
        .map(|keyframe| {
            let mut fields = keyframe.split(':');
            let at: i64 = fields.next().unwrap().parse().unwrap();
            let value: i64 = fields.next().unwrap().parse().unwrap();
            let interpolation = fields.next().unwrap();
            assert!(fields.next().is_none(), "{keyframe} has a fourth field");
            json!({
                "at": at,
                "value": value,
                "interpolation": pascal_to_snake(interpolation)
            })
        })
        .collect::<Vec<_>>();
    json!({"keyframes": keyframes})
}

/// `EaseInOut` -> `ease_in_out`: the rendering prints the `Debug` name and the
/// wire takes serde's `snake_case` one.
fn pascal_to_snake(value: &str) -> String {
    let mut snake = String::new();
    for (index, character) in value.char_indices() {
        if character.is_uppercase() && index != 0 {
            snake.push('_');
        }
        snake.extend(character.to_lowercase());
    }
    snake
}

/// The `[...]` payload of one rendered curve field, by its key.
fn rendered_curve(text: &str, key: &str) -> String {
    let start = text
        .find(key)
        .unwrap_or_else(|| panic!("{key} must be rendered: {text}"))
        + key.len();
    let end = start
        + text[start..]
            .find(']')
            .expect("every rendered curve closes its bracket");
    text[start..end].to_owned()
}

/// AU4 §7 item A17: the two new curve mutators survive the whole agent round
/// trip — plan preparation, commit, the compact state rendering, and the
/// clear — on the AU1/AU2 template
/// (`au1_set_track_mix_round_trips_through_edit_plans_and_state`,
/// `au2_set_audio_master_and_pan_law_round_trip_through_edit_plans_and_state`).
///
/// The two refusals at the end are the wire half of AU4 §0 E1 and of rule 39:
/// an omitted `curve` is an error rather than a silent clear, and an unknown
/// `parameter` gets the closed vocabulary back.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au4_set_clip_gain_envelope_and_track_automation_round_trip_through_edit_plans_and_state() {
    let core = Core::spawn(edit_plan_document()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // One plan, all three curve writes: the clip envelope in clip-local
    // frames and both track rides in project frames.
    let prepared = prepare_plan(
        &client,
        0,
        json!([
            {
                "op": "set_clip_gain_envelope",
                "clip": 1,
                "curve": {"keyframes": [
                    {"at": 0, "value": 0, "interpolation": "linear"},
                    {"at": 30, "value": -60, "interpolation": "hold"},
                    {"at": 59, "value": -120, "interpolation": "ease_in_out"}
                ]}
            },
            {
                "op": "set_track_automation",
                "track": 1,
                "parameter": "gain_tenth_db",
                "curve": {"keyframes": [
                    {"at": 0, "value": 0, "interpolation": "linear"},
                    {"at": 45, "value": -45, "interpolation": "hold"}
                ]}
            },
            {
                "op": "set_track_automation",
                "track": 1,
                "parameter": "pan_percent",
                "curve": {"keyframes": [
                    {"at": 0, "value": -100, "interpolation": "ease_in"},
                    {"at": 59, "value": 100, "interpolation": "linear"}
                ]}
            }
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let document = query_document(&core);
    let clip = &document.tracks[0].clips[0];
    assert_eq!(
        clip.audio_gain_curve
            .as_ref()
            .expect("the envelope must be stored")
            .keyframes
            .len(),
        3
    );
    // AU4 §2.5 rule 32a: a `SetTrackAutomation` on an un-mixed track pushes a
    // new entry, and the five scalars stay neutral.
    assert_eq!(document.audio_mix.tracks.len(), 1);
    let mix = document.track_mix(TrackId(1));
    assert_eq!(mix.gain_tenth_db, 0);
    assert_eq!(mix.pan_percent, 0);
    assert!(mix.gain_curve.is_some() && mix.pan_curve.is_some());
    // AU4 §2.1 rule 11: a curve-bearing entry is not neutral, which is the
    // only reason the `mix=` suffix below exists at all.
    assert!(!mix.is_neutral());

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = &state.content[0].as_text().unwrap().text;
    assert!(
        text.contains(
            "track 1 video sync_lock=true clips=1 mix=gain:0,pan:0,mute:false,solo:false,gain_curve:[0:0:Linear,45:-45:Hold],pan_curve:[0:-100:EaseIn,59:100:Linear]\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            " audio=gain:0,fade_in:0f,fade_out:0f,envelope:[0:0:Linear,30:-60:Hold,59:-120:EaseInOut]"
        ),
        "{text}"
    );

    // Clearing every curve removes both suffixes, and rule 32a removes the
    // whole entry, so the document is the one that never carried a curve.
    let prepared = prepare_plan(
        &client,
        1,
        json!([
            {"op": "set_clip_gain_envelope", "clip": 1, "curve": null},
            {"op": "set_track_automation", "track": 1, "parameter": "gain_tenth_db", "curve": null},
            {"op": "set_track_automation", "track": 1, "parameter": "pan_percent", "curve": null}
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let cleared = query_document(&core);
    assert!(cleared.tracks[0].clips[0].audio_gain_curve.is_none());
    assert!(cleared.audio_mix.tracks.is_empty());
    assert_eq!(
        serde_json::to_string(&cleared).unwrap(),
        serde_json::to_string(&edit_plan_document()).unwrap(),
        "clearing the last curve must leave the document byte-identical to one that never had one"
    );

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = &state.content[0].as_text().unwrap().text;
    assert!(
        text.contains("track 1 video sync_lock=true clips=1\n"),
        "{text}"
    );
    for absent in [
        "mix=gain:",
        "envelope:",
        "gain_curve",
        "pan_curve",
        " audio=",
    ] {
        assert!(!text.contains(absent), "{absent} in {text}");
    }

    // AU4 §0 E1: an omitted `curve` is an error, never a silent clear.
    let omitted = prepare_plan(
        &client,
        2,
        json!([{"op": "set_clip_gain_envelope", "clip": 1}]),
    )
    .await;
    assert_eq!(omitted.is_error, Some(true), "{omitted:?}");
    let text = &omitted.content[0].as_text().unwrap().text;
    assert!(text.contains("missing field `curve`"), "{text}");
    let omitted = prepare_plan(
        &client,
        2,
        json!([{"op": "set_track_automation", "track": 1, "parameter": "gain_tenth_db"}]),
    )
    .await;
    assert_eq!(omitted.is_error, Some(true), "{omitted:?}");
    let text = &omitted.content[0].as_text().unwrap().text;
    assert!(text.contains("missing field `curve`"), "{text}");

    // AU4 §2.6 rule 39: the vocabulary comes back with the refusal, and it is
    // raised before the curve is even validated.
    let unknown = prepare_plan(
        &client,
        2,
        json!([{
            "op": "set_track_automation",
            "track": 1,
            "parameter": "loudness",
            "curve": {"keyframes": [{"at": 0, "value": 0}]}
        }]),
    )
    .await;
    assert_eq!(unknown.is_error, Some(true), "{unknown:?}");
    let text = &unknown.content[0].as_text().unwrap().text;
    assert!(
        text.contains("unknown track automation parameter")
            && text.contains("gain_tenth_db")
            && text.contains("pan_percent"),
        "{text}"
    );
    // Nothing was applied by any refusal.
    assert_eq!(
        serde_json::to_string(&query_document(&core)).unwrap(),
        serde_json::to_string(&edit_plan_document()).unwrap()
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU4 §7 item A17, rule 34: whole-owner-set semantics on `upsert_audio_bus`
/// and `set_audio_master`, made recoverable by the `render_*` spelling.
///
/// A curve-bearing bus round-trips: the fader ride read out of
/// `get_timeline_state` is written straight back and the document does not
/// move a byte. Then the same call with `gain_curve` **omitted** clears it,
/// exactly as an omitted `effects` clears the chain — which is the reason the
/// rendering has to carry the curve in the first place.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au4_audio_bus_and_master_fader_curves_round_trip_and_clear_when_omitted() {
    let core = Core::spawn(edit_plan_document()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let prepared = prepare_plan(
        &client,
        0,
        json!([
            {
                "op": "upsert_audio_bus",
                "bus": {
                    "id": 1,
                    "name": "Dialogue",
                    "tracks": [1],
                    "gain_tenth_db": -35,
                    "gain_curve": {"keyframes": [
                        {"at": 0, "value": -35, "interpolation": "linear"},
                        {"at": 30, "value": 0, "interpolation": "ease_out"}
                    ]}
                }
            },
            {
                "op": "set_audio_master",
                "master": {
                    "gain_tenth_db": 15,
                    "gain_curve": {"keyframes": [{"at": 0, "value": 15, "interpolation": "hold"}]}
                }
            }
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = state.content[0].as_text().unwrap().text.clone();
    assert!(
        text.contains(
            "  audio_bus 1 \"Dialogue\" tracks=1 gain=-35 gain_curve=[0:-35:Linear,30:0:EaseOut] sidechain=none effects=none\naudio_master gain=15 gain_curve=[0:15:Hold] effects=none"
        ),
        "{text}"
    );

    // The round trip: read the rendered ride back, write it straight into
    // `upsert_audio_bus`, and the document does not move.
    let before = query_document(&core);
    let bus_curve = curve_from_rendered(&rendered_curve(&text, "gain_curve=["));
    let prepared = prepare_plan(
        &client,
        1,
        json!([{
            "op": "upsert_audio_bus",
            "bus": {
                "id": 1,
                "name": "Dialogue",
                "tracks": [1],
                "gain_tenth_db": -35,
                "gain_curve": bus_curve
            }
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");
    assert_eq!(
        serde_json::to_string(&query_document(&core)).unwrap(),
        serde_json::to_string(&before).unwrap(),
        "writing back the rendered fader curve must be the identity"
    );

    // Rule 34: an omitted `gain_curve` clears it, exactly as an omitted
    // `effects` clears the chain.
    let prepared = prepare_plan(
        &client,
        2,
        json!([
            {
                "op": "upsert_audio_bus",
                "bus": {"id": 1, "name": "Dialogue", "tracks": [1], "gain_tenth_db": -35}
            },
            {"op": "set_audio_master", "master": {"gain_tenth_db": 15}}
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(2, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let document = query_document(&core);
    assert!(
        document
            .audio_mix
            .bus(kinewright_core::AudioBusId(1))
            .expect("the bus survives the clear")
            .gain_curve
            .is_none(),
        "an omitted gain_curve must clear the bus fader ride"
    );
    assert!(
        document.audio_mix.master.gain_curve.is_none(),
        "an omitted gain_curve must clear the master fader ride"
    );

    let state = client
        .call_tool(CallToolRequestParams::new("get_timeline_state"))
        .await
        .unwrap();
    let text = &state.content[0].as_text().unwrap().text;
    assert!(
        text.contains(
            "  audio_bus 1 \"Dialogue\" tracks=1 gain=-35 sidechain=none effects=none\naudio_master gain=15 effects=none"
        ),
        "{text}"
    );
    assert!(!text.contains("gain_curve"), "{text}");

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU4 §7 item A18: `get_audio_levels`' golden pair.
///
/// `measure_mix_levels` puts `document.track_mix(track.id)` straight into
/// `TrackLevels.mix`, so the two new `Option` fields reach the agent's
/// structured content automatically. They are default-omitted, so a curve-free
/// report carries neither key — which is why the AU1 golden
/// (`au1_get_audio_levels_measures_the_real_mix`) is byte-unchanged — and a
/// curve-bearing one carries both, with every leaf still an integer, boolean,
/// string or null.
///
/// The measurement runs on the real `FfmpegMediaEngine`, but this test asserts
/// the report's *shape*, not the audible effect of the ride: how automation
/// changes the measured stem is AU4 §3's media evidence (A10-A12), not the
/// agent's.
#[tokio::test(flavor = "multi_thread")]
async fn au4_get_audio_levels_reports_both_track_automation_curves() {
    let generated = au3_sine_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let document = single_clip_document(asset);
    let duration = document.duration.0;
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // Without a curve: neither key appears anywhere in the report.
    let baseline = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(baseline.is_error, Some(false), "{baseline:?}");
    let report = baseline
        .structured_content
        .as_ref()
        .expect("get_audio_levels must publish the machine-readable report")["report"]
        .clone();
    let serialized = serde_json::to_string(&report).unwrap();
    for absent in ["gain_curve", "pan_curve"] {
        assert!(
            !serialized.contains(absent),
            "an absent curve must be omitted: {serialized}"
        );
    }
    assert_integer_leaves("report", &report);
    let baseline_mix = report["tracks"][0]["mix"].clone();

    let prepared = prepare_plan(
        &client,
        0,
        json!([
            {
                "op": "set_track_automation",
                "track": 1,
                "parameter": "gain_tenth_db",
                "curve": {"keyframes": [
                    {"at": 0, "value": 0, "interpolation": "linear"},
                    {"at": duration - 1, "value": -60, "interpolation": "linear"}
                ]}
            },
            {
                "op": "set_track_automation",
                "track": 1,
                "parameter": "pan_percent",
                "curve": {"keyframes": [{"at": 0, "value": -50, "interpolation": "hold"}]}
            }
        ]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let automated = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(automated.is_error, Some(false), "{automated:?}");
    let report = automated.structured_content.as_ref().unwrap()["report"].clone();
    let mix = &report["tracks"][0]["mix"];
    assert_eq!(
        mix["gain_curve"]["keyframes"],
        json!([
            {"at": 0, "value": 0, "interpolation": "linear"},
            {"at": duration - 1, "value": -60, "interpolation": "linear"}
        ]),
        "{report}"
    );
    assert_eq!(
        mix["pan_curve"]["keyframes"],
        json!([{"at": 0, "value": -50, "interpolation": "hold"}]),
        "{report}"
    );
    // AU3 A14's walk, applied to the automated report: an envelope adds no
    // float to the wire.
    assert_integer_leaves("report", &report);

    // The golden half: stripping the two new keys gives back the pre-AU4
    // `mix` object byte for byte, so the only shape change AU4 makes to this
    // report is the two default-omitted fields. Compared on the `mix` object
    // rather than the whole report because the *measurement* is media's to
    // change once automation is evaluated (AU4 §3), while `mix` is document
    // state and must not move at all.
    let mut stripped = mix.clone();
    let stripped = stripped.as_object_mut().unwrap();
    stripped.remove("gain_curve");
    stripped.remove("pan_curve");
    assert_eq!(
        serde_json::to_string(&serde_json::Value::Object(stripped.clone())).unwrap(),
        serde_json::to_string(&baseline_mix).unwrap(),
        "the two Option fields are the only change AU4 makes to TrackLevels.mix"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// One band's measured level, in hundredths of a dBFS, from a structured
/// `get_audio_spectrum` report. Panics on a silent band, which none of the
/// 1 kHz fixture's measured points has.
fn spectrum_band_level(report: &serde_json::Value, center_hertz_tenths: i64) -> i64 {
    report["bands"]
        .as_array()
        .expect("every report carries its bands")
        .iter()
        .find(|band| band["center_hertz_tenths"] == json!(center_hertz_tenths))
        .unwrap_or_else(|| panic!("band {center_hertz_tenths} must be reported: {report}"))
        ["level_dbfs_hundredths"]
        .as_i64()
        .unwrap_or_else(|| panic!("band {center_hertz_tenths} measured silent: {report}"))
}

/// AU2 §7 item B17: `get_audio_spectrum` measured end to end on generated
/// 1 kHz sine media.
///
/// Like its AU1 sibling `au1_get_audio_levels_measures_the_real_mix`, this
/// test deliberately exercises `Analysis::mix_spectrum` on the REAL
/// `FfmpegMediaEngine` — no stub, no double — so it is the agent-side proof
/// that the measurement reaches the media engine's mix path. The engine
/// implements the facet in `engine.rs` by delegating to
/// `export::measure_mix_spectrum`; a `NotImplemented` failure on the first
/// assertion means that impl was lost, not that the measurement is a stub.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au2_get_audio_spectrum_measures_the_real_mix() {
    let mut arguments = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30000/1001",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=1000:sample_rate=48000",
        "-frames:v",
        "60",
        "-t",
        "2.002",
    ];
    arguments.extend(MANAGED_BT709_ENCODE_ARGUMENTS);
    arguments.extend(["-c:a", "aac", "-shortest"]);
    let generated = GeneratedMedia::ffmpeg("au2-spectrum", &arguments, "mp4");
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let document = single_clip_document(asset);
    let duration = document.duration.0;
    assert!(
        duration > 30,
        "the fixture must exceed the 512 ms Welch minimum: {duration}"
    );
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // AU2 §6.2: omitting both bounds measures the whole timeline at master.
    let master = invoke_capability(&client, "get_audio_spectrum", json!({})).await;
    assert_eq!(
        master.is_error,
        Some(false),
        "get_audio_spectrum must measure the real mix: {master:?}"
    );
    let structured = master
        .structured_content
        .as_ref()
        .expect("get_audio_spectrum must publish the machine-readable report");
    assert_eq!(structured["timeline_revision"], json!(0));
    let report = &structured["report"];
    assert_eq!(report["point"], json!("master"), "{report}");
    assert_eq!(report["range"]["start"], json!(0), "{report}");
    assert_eq!(report["range"]["end"], json!(duration), "{report}");
    assert_eq!(report["sample_rate"], json!(48_000), "{report}");
    assert!(
        report["segments"].as_u64().unwrap() >= 2,
        "Welch needs at least two segments: {report}"
    );

    let bands = report["bands"].as_array().unwrap();
    assert_eq!(bands.len(), 31, "31 ISO third-octave bands: {report}");
    assert_eq!(bands[0]["center_hertz_tenths"], json!(200));
    assert_eq!(bands[30]["center_hertz_tenths"], json!(200_000));
    let peak = bands
        .iter()
        .max_by_key(|band| band["level_dbfs_hundredths"].as_i64().unwrap_or(i64::MIN))
        .unwrap();
    assert_eq!(
        peak["center_hertz_tenths"],
        json!(10_000),
        "a 1 kHz sine must peak in the 1000 Hz band: {report}"
    );

    // AU2 §6.2: the text format, including the five flagged bands and the
    // one-decimal centre.
    let text = &master.content[0].as_text().unwrap().text;
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 32, "one header and 31 band lines: {text}");
    assert!(
        lines[0].starts_with(&format!(
            "mix_spectrum range=0..{duration} point=master sample_frames="
        )) && lines[0].ends_with("levels in dBFS hundredths"),
        "{text}"
    );
    for (index, centre) in ["20", "25", "31.5", "40", "50"].into_iter().enumerate() {
        assert!(
            lines[index + 1].starts_with(&format!("band {centre} "))
                && lines[index + 1].ends_with(" window_limited"),
            "{text}"
        );
    }
    assert!(lines[6].starts_with("band 63 "), "{text}");
    assert!(!lines[6].ends_with(" window_limited"), "{text}");
    assert!(lines[31].starts_with("band 20000 "), "{text}");
    assert!(!text.ends_with('\n'), "{text}");

    // A track target and a bus target, both through the real mix path.
    let track = invoke_capability(&client, "get_audio_spectrum", json!({"track": 1})).await;
    assert_eq!(track.is_error, Some(false), "{track:?}");
    let report = &track.structured_content.as_ref().unwrap()["report"];
    assert_eq!(report["point"], json!({"track": 1}), "{report}");
    let peak = report["bands"]
        .as_array()
        .unwrap()
        .iter()
        .max_by_key(|band| band["level_dbfs_hundredths"].as_i64().unwrap_or(i64::MIN))
        .unwrap()
        .clone();
    assert_eq!(peak["center_hertz_tenths"], json!(10_000), "{report}");
    let track_1k = spectrum_band_level(report, 10_000);
    assert!(
        track.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("point=track 1"),
        "{track:?}"
    );

    let prepared = prepare_plan(
        &client,
        0,
        json!([{
            "op": "upsert_audio_bus",
            "bus": {"id": 1, "name": "Dialogue", "tracks": [1], "gain_tenth_db": -60}
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");

    let bus = invoke_capability(&client, "get_audio_spectrum", json!({"bus": 1})).await;
    assert_eq!(bus.is_error, Some(false), "{bus:?}");
    let report = &bus.structured_content.as_ref().unwrap()["report"];
    assert_eq!(report["point"], json!({"bus": 1}), "{report}");
    let peak = report["bands"]
        .as_array()
        .unwrap()
        .iter()
        .max_by_key(|band| band["level_dbfs_hundredths"].as_i64().unwrap_or(i64::MIN))
        .unwrap()
        .clone();
    assert_eq!(peak["center_hertz_tenths"], json!(10_000), "{report}");
    // AU2 §5.6: the bus fader sits inside the bus stem, so a -60 tenth-dB bus
    // gain must move every band of that stem by -600 hundredths against the
    // post-track-stage stem feeding it. This is the AU1 gain pin
    // (`au1_get_audio_levels_measures_the_real_mix`) applied to the spectrum:
    // a bus stem tapped pre-fader, or the wrong stem entirely, would still
    // peak at 1 kHz and pass the assertion above.
    let bus_1k = spectrum_band_level(report, 10_000);
    assert!(
        (bus_1k - track_1k + 600).abs() <= 5,
        "a -60 tenth-dB bus gain must move the 1 kHz band by -600 hundredths, \
         measured track {track_1k} -> bus {bus_1k}"
    );
    assert!(
        bus.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("point=bus 1"),
        "{bus:?}"
    );

    // AU2 §5.9/A23: a range shorter than two Welch segments is refused with
    // the typed `MixSpectrumRangeTooShort`, never a degenerate spectrum.
    let short = invoke_capability(
        &client,
        "get_audio_spectrum",
        json!({"start_frame": 0, "end_frame": 10}),
    )
    .await;
    assert_eq!(short.is_error, Some(true), "{short:?}");
    let text = &short.content[0].as_text().unwrap().text;
    assert!(
        text.contains("could not measure the mix spectrum")
            && text.contains("mix spectrum needs at least 24576 sample frames"),
        "{text}"
    );

    // An inverted range is refused rather than clamped, exactly as
    // `get_audio_levels` refuses one.
    let inverted = invoke_capability(
        &client,
        "get_audio_spectrum",
        json!({"start_frame": 30, "end_frame": 10}),
    )
    .await;
    assert_eq!(inverted.is_error, Some(true), "{inverted:?}");
    assert!(
        inverted.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("get_audio_spectrum needs start_frame < end_frame; got 30..10"),
        "{inverted:?}"
    );

    // Two targets at once is a caller error, not a silent preference.
    let both =
        invoke_capability(&client, "get_audio_spectrum", json!({"track": 1, "bus": 1})).await;
    assert_eq!(both.is_error, Some(true), "{both:?}");
    assert_eq!(
        both.content[0].as_text().unwrap().text,
        "get_audio_spectrum takes at most one of track and bus"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU3: one managed clip carrying a 440 Hz sine at lavfi's default amplitude
/// (1/8 full scale, about -18 dBFS), 60 frames at exactly 30 fps so one
/// project frame is exactly 1 600 sample frames at 48 kHz and the whole
/// 2 s programme is 96 000. The AU1/AU2 fixture pattern with an integer rate.
fn au3_sine_media() -> GeneratedMedia {
    let mut arguments = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30",
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
    GeneratedMedia::ffmpeg("au3-audio-qc", &arguments, "mp4")
}

/// AU3: `au3_sine_media`'s 440 Hz sine at -20 dBFS with a hot 0.5 ms 2 kHz
/// tick at 0.9 full scale every 100 ms on top, so the delivery limiter has
/// something to do. Same container, rate, and 60-frame length as its sibling,
/// which keeps the QC pins on `au3_sine_media` untouched.
///
/// Making the sine *louder* would not do it. Loudness normalization is
/// level-independent: the planner asks for `target - measured_lufs` dB of
/// gain, so the peak arriving at the limiter is
/// `source_peak - source_lufs + target` — the source's peak-to-loudness ratio
/// plus the target, whatever the file's absolute level. A steady sine's PLR is
/// about 5.7 dB, so at -1600 its normalized peak sits roughly 13 dB under the
/// -300 processing ceiling however hot the file is, and `volume=12dB` only
/// drives the plan to a *negative* gain, leaving the limiter idler still.
/// Crest is what engages it, and the tick is what supplies crest: the file
/// measures about -0.4 dBTP against -21.4 LUFS, a PLR near 21 dB (about 18 dB
/// once the mono clip is panned into the stereo mix), which puts the
/// normalized peak over the ceiling.
///
/// The split of duties is deliberate. The steady tone carries the loudness, so
/// the four-iteration convergence loop still lands inside the tolerance (it
/// stops at -1660, 40 hundredths inside the requested +/-100); the
/// tick carries the peak, and at 0.5 ms it is far shorter than the emitted
/// compressor's 5 ms attack, so the compressor cannot swallow it and the
/// true-peak limiter is what holds the ceiling. A denser or louder tick makes
/// the tick itself carry the programme loudness, and then limiting it costs
/// more loudness than the loop can win back: at 5 ms bursts every 100 ms with
/// no tone the plan lands at -1687, and at 2 ms bursts it fails outright with
/// "normalization could not satisfy the delivery contract".
fn au3_hot_sine_media() -> GeneratedMedia {
    let mut arguments = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30",
        "-f",
        "lavfi",
        "-i",
        "aevalsrc=0.10*sin(2*PI*440*t)+0.9*sin(2*PI*2000*t)*lt(mod(t\\,0.1)\\,0.0005):s=48000",
        "-frames:v",
        "60",
        "-t",
        "2.002",
    ];
    arguments.extend(MANAGED_BT709_ENCODE_ARGUMENTS);
    arguments.extend(["-c:a", "aac", "-shortest"]);
    GeneratedMedia::ffmpeg("au3-audio-hot", &arguments, "mp4")
}

/// CC6's walk over the QC report, applied to the whole `get_audio_qc`
/// envelope: every leaf is an integer, a bool, a string, or null (AU3 A14).
fn assert_integer_leaves(path: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::Number(number) => assert!(
            number.is_i64() || number.is_u64(),
            "{path} = {number} is not an integer"
        ),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_integer_leaves(&format!("{path}[{index}]"), item);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                assert_integer_leaves(&format!("{path}.{key}"), item);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => {}
    }
}

/// AU3 §7 items A14 and A15: `get_audio_qc` is integer-reported and
/// evidence-only over the live endpoint.
///
/// The closed argument schema, the stale-revision envelope, the inverted-range
/// refusal, and `get_delivery_profiles`' published targets need no decoder
/// and come first; the sub-block refusal text, the four text lines, the
/// all-integer envelope, and the profile-bound report are measured on the
/// real 440 Hz fixture. Nothing on this path moves the revision.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au3_get_audio_qc_is_evidence_only_and_revision_gated() {
    let generated = au3_sine_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let document = single_clip_document(asset);
    let duration = document.duration.0;
    assert_eq!(duration, 60, "the fixture is exactly 60 frames at 30 fps");
    let core = Core::spawn(document).unwrap();
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

    // The published schema: an inspector with exactly the four AU3 §4.1
    // arguments and nothing else accepted.
    let opened = client
        .call_tool(
            CallToolRequestParams::new("get_capability")
                .with_arguments(json!({"name": "get_audio_qc"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(opened.is_error, Some(false));
    let opened = opened.structured_content.as_ref().unwrap();
    assert_eq!(opened["capability"]["kind"], "inspector");
    let properties = opened["input_schema"]["properties"].as_object().unwrap();
    for present in ["expected_revision", "start_frame", "end_frame", "profile"] {
        assert!(properties.contains_key(present), "missing {present}");
    }
    assert_eq!(properties.len(), 4, "{opened}");
    assert_eq!(opened["input_schema"]["additionalProperties"], json!(false));
    // `get_capability` publishes the description's first sentence as the
    // summary; the full description's 1 KB budget is pinned in-crate.
    let summary = opened["capability"]["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("Measure evidence-only audio QC of the master mix")
            && summary.ends_with("every value an integer in hundredths."),
        "{summary}"
    );

    // A stale revision is the uniform envelope, refused before any decode.
    let stale = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"expected_revision": revision + 7}),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));
    let stale_body = stale.structured_content.as_ref().unwrap();
    assert_eq!(stale_body["code"], "stale_revision");
    assert_eq!(stale_body["applied"], false);
    assert_eq!(stale_body["evidence_only"], true);
    assert_eq!(stale_body["details"]["expected_revision"], revision + 7);
    assert_eq!(stale_body["details"]["actual_revision"], revision);

    // An inverted range is refused rather than clamped.
    let inverted = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"start_frame": 30, "end_frame": 10}),
    )
    .await;
    assert_eq!(inverted.is_error, Some(true));
    assert_eq!(
        inverted.content[0].as_text().unwrap().text,
        "get_audio_qc needs start_frame < end_frame; got 30..10"
    );

    // `deny_unknown_fields`: a resolution knob of any spelling is a malformed
    // request, surfaced as a protocol error rather than silently ignored.
    let unknown = client
        .call_tool(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({"name": "get_audio_qc", "arguments": {"resolution": "proxy"}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;
    assert!(unknown.is_err(), "{unknown:?}");

    // F12: every delivery profile publishes its loudness target, and the
    // description says so.
    let profiles = invoke_capability(&client, "get_delivery_profiles", json!({})).await;
    assert_eq!(profiles.is_error, Some(false), "{profiles:?}");
    let profiles = profiles.structured_content.as_ref().unwrap()["profiles"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(profiles.len(), 4);
    for profile in &profiles {
        let expected = match profile["id"].as_str().unwrap() {
            "source_master" => -2_300,
            "youtube_1080p" | "vertical_short" | "square_social" => -1_400,
            other => panic!("unexpected profile {other}"),
        };
        let target = &profile["loudness_target"];
        assert_eq!(target["integrated_lufs_hundredths"], expected, "{profile}");
        assert_eq!(target["tolerance_lu_hundredths"], 100, "{profile}");
        assert_eq!(
            target["true_peak_ceiling_dbtp_hundredths"], -100,
            "{profile}"
        );
    }
    let opened = client
        .call_tool(
            CallToolRequestParams::new("get_capability").with_arguments(
                json!({"name": "get_delivery_profiles"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    // The description is one sentence, so the published summary is all of it.
    let summary = opened.structured_content.as_ref().unwrap()["capability"]["summary"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        summary.ends_with("bitrates, and the loudness target normalization and QC read."),
        "{summary}"
    );

    // Q3: a range shorter than one 400 ms gating block is refused, typed,
    // with the frame counts. One frame at 30 fps is 1 600 sample frames.
    let short = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"start_frame": 0, "end_frame": 1}),
    )
    .await;
    assert_eq!(short.is_error, Some(true), "{short:?}");
    assert_eq!(
        short.content[0].as_text().unwrap().text,
        "get_audio_qc needs at least one 400 ms gating block (19200 sample frames); got 1600"
    );

    // The measurement itself. `expected_revision` is deliberately absent:
    // this is an inspector, not a planner.
    let report = invoke_capability(&client, "get_audio_qc", json!({})).await;
    assert_eq!(report.is_error, Some(false), "{report:?}");
    let body = report.structured_content.as_ref().unwrap();
    assert_eq!(body["evidence_only"], true);
    assert_eq!(body["applied"], false);
    assert_eq!(body["timeline_revision"], revision);
    assert!(
        body.get("stage").is_none(),
        "a mix measurement has no stage"
    );
    assert!(body.get("full_resolution").is_none());
    let qc = &body["report"];
    assert_eq!(qc["evidence_only"], true);
    assert_eq!(qc["range"], json!({"start": 0, "end": duration}));
    assert_eq!(qc["provenance"]["engine"], "kinewright_audio_qc_v1");
    assert_eq!(qc["provenance"]["measurement_rate"], 48_000);
    assert!(qc["target"].is_null(), "no profile, no target: {qc}");
    assert_eq!(body["exceptions"], qc["exceptions"]);
    // A14: every leaf of the report and of the envelope is an integer, a
    // bool, a string, or null.
    assert_integer_leaves("envelope", body);
    // A15: the four text lines of §4.1, none of them quoted JSON.
    let text = report.content[0].as_text().unwrap().text.clone();
    let lines = text.lines().collect::<Vec<_>>();
    assert!(lines.len() >= 3, "{text}");
    assert!(
        lines[0].starts_with(&format!(
            "audio_qc range=0..{duration} profile=none target=none±none ceiling=none technical_pass="
        )),
        "{text}"
    );
    assert!(lines[1].starts_with("master lufs="), "{text}");
    assert!(
        lines[1].contains(" true_peak=") && lines[1].contains(" frames=96000"),
        "{text}"
    );
    assert!(lines[2].starts_with("balance="), "{text}");
    assert!(lines[2].contains(" clipping L samples="), "{text}");
    for line in &lines[3..] {
        assert!(line.starts_with("exception "), "{text}");
    }
    assert!(!text.contains('"'), "{text}");
    let assumptions = body["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|assumption| assumption.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(assumptions.len(), 4, "{assumptions:#?}");
    assert!(assumptions[0].starts_with("Measured at 48 kHz stereo"));
    assert!(
        assumptions[3].ends_with("technical_pass is not export_ready."),
        "{assumptions:#?}"
    );

    // A profile binds the report to that profile's published target and adds
    // exactly one assumption. The wire spelling is serde's `youtube1080p`;
    // the text echoes `get_delivery_profiles`' id.
    let judged =
        invoke_capability(&client, "get_audio_qc", json!({"profile": "youtube1080p"})).await;
    assert_eq!(judged.is_error, Some(false), "{judged:?}");
    let body = judged.structured_content.as_ref().unwrap();
    assert_eq!(
        body["report"]["target"],
        json!({
            "integrated_lufs_hundredths": -1_400,
            "tolerance_lu_hundredths": 100,
            "true_peak_ceiling_dbtp_hundredths": -100
        })
    );
    assert_integer_leaves("envelope", body);
    let assumptions = body["assumptions"].as_array().unwrap();
    assert_eq!(assumptions.len(), 5, "{assumptions:#?}");
    assert!(
        assumptions[3]
            .as_str()
            .unwrap()
            .contains("get_delivery_profiles")
    );
    let text = judged.content[0].as_text().unwrap().text.clone();
    assert!(
        text.starts_with(&format!(
            "audio_qc range=0..{duration} profile=youtube_1080p target=-1400±100 ceiling=-100 technical_pass="
        )),
        "{text}"
    );

    // Whatever was measured, nothing moved.
    assert_eq!(
        query_document(&core),
        before,
        "get_audio_qc must never mutate the timeline"
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

/// AU3 §7 items A13–A15 (agent half): `get_audio_qc` measures the real mix.
///
/// On the 440 Hz fixture the master reads a programme loudness with its true
/// peak at or above its sample peak, no short-term maximum and no range on a
/// 2 s programme, a centred balance, no clipping, and no exception; a −6 dB
/// track gain moves loudness and both peaks by −600 hundredths; a streaming
/// profile raises exactly the out-of-tolerance warning; exactly one gating
/// block measures while one frame is refused; and a muted track is digital
/// silence — `integrated: null` with the lone `audio_silent` warning and
/// `technical_pass` still true. `get_audio_levels` carries the same four new
/// fields and the same sub-block refusal.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au3_get_audio_qc_measures_the_real_mix() {
    let generated = au3_sine_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let document = single_clip_document(asset);
    let duration = document.duration.0;
    assert_eq!(duration, 60, "the fixture is exactly 60 frames at 30 fps");
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let baseline = invoke_capability(&client, "get_audio_qc", json!({})).await;
    assert_eq!(
        baseline.is_error,
        Some(false),
        "get_audio_qc must measure the real mix: {baseline:?}"
    );
    let qc = baseline.structured_content.as_ref().unwrap()["report"].clone();
    let master = &qc["master"];
    assert_eq!(master["sample_rate"], 48_000, "{master}");
    assert_eq!(master["channels"], 2, "{master}");
    assert_eq!(master["sample_frames"], 96_000, "{master}");
    let before = master["integrated_lufs_hundredths"]
        .as_i64()
        .expect("a -18 dBFS sine measures a programme loudness");
    let sample_peak = master["sample_peak_dbfs_hundredths"].as_i64().unwrap();
    let true_peak = master["true_peak_dbtp_hundredths"]
        .as_i64()
        .expect("a non-silent programme has a true peak");
    assert!(
        true_peak >= sample_peak,
        "true peak {true_peak} reads at or above the sample peak {sample_peak}"
    );
    let momentary = master["momentary_max_lufs_hundredths"]
        .as_i64()
        .expect("a 2 s programme has complete 400 ms windows");
    assert!(
        momentary + 1 >= before,
        "the loudest window {momentary} is at or above the gated mean {before}"
    );
    // §2.1 / §6.9: the 2.002 s fixture reports no short-term maximum and no
    // loudness range, both of which need complete 3 s windows.
    assert!(
        master["short_term_max_lufs_hundredths"].is_null(),
        "{master}"
    );
    assert!(master["loudness_range_lu_hundredths"].is_null(), "{master}");
    // A mono sine panned centre lands equally on both channels.
    let balance = qc["channel_balance_lu_hundredths"].as_i64().unwrap();
    assert!(balance.abs() <= 5, "balance {balance}");
    for side in ["left", "right"] {
        assert_eq!(qc["clipping"][side]["clipped_runs"], 0, "{qc}");
        assert_eq!(qc["clipping"][side]["over_full_scale_samples"], 0, "{qc}");
    }
    assert!(qc["leading_silence_frames"].as_i64().unwrap() <= 3, "{qc}");
    assert!(qc["trailing_silence_frames"].as_i64().unwrap() <= 3, "{qc}");
    assert_eq!(qc["exceptions"], json!([]), "{qc}");
    assert_eq!(qc["technical_pass"], true);

    // A streaming profile: a -18 dBFS sine is far under -14 LUFS, so exactly
    // the out-of-tolerance warning is raised, the peak is under the ceiling,
    // and technical_pass stays true because no Error was raised.
    let judged =
        invoke_capability(&client, "get_audio_qc", json!({"profile": "youtube1080p"})).await;
    assert_eq!(judged.is_error, Some(false), "{judged:?}");
    let qc = judged.structured_content.as_ref().unwrap()["report"].clone();
    assert_eq!(qc["target"]["integrated_lufs_hundredths"], -1_400);
    let exceptions = qc["exceptions"].as_array().unwrap();
    assert_eq!(exceptions.len(), 1, "{exceptions:?}");
    assert_eq!(exceptions[0]["code"], "audio_loudness_out_of_tolerance");
    assert_eq!(exceptions[0]["severity"], "warning");
    assert_eq!(exceptions[0]["field"], "integrated_lufs_hundredths");
    assert_eq!(exceptions[0]["observed"], before.to_string());
    assert_eq!(exceptions[0]["allowed"], "-1500..=-1300");
    assert_eq!(qc["technical_pass"], true);
    let text = judged.content[0].as_text().unwrap().text.clone();
    assert!(
        text.contains(&format!("exception Warning audio_loudness_out_of_tolerance integrated_lufs_hundredths observed={before} allowed=-1500..=-1300")),
        "{text}"
    );
    let mastered =
        invoke_capability(&client, "get_audio_qc", json!({"profile": "source_master"})).await;
    assert_eq!(mastered.is_error, Some(false), "{mastered:?}");
    assert_eq!(
        mastered.structured_content.as_ref().unwrap()["report"]["target"]["integrated_lufs_hundredths"],
        -2_300
    );

    // Exactly one gating block (12 frames at 30 fps = 19 200 sample frames)
    // measures; one frame is refused before anything is decoded.
    let one_block = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"start_frame": 0, "end_frame": 12}),
    )
    .await;
    assert_eq!(one_block.is_error, Some(false), "{one_block:?}");
    let qc = one_block.structured_content.as_ref().unwrap()["report"].clone();
    assert_eq!(qc["master"]["sample_frames"], 19_200, "{qc}");
    assert!(
        qc["master"]["integrated_lufs_hundredths"].is_i64(),
        "one complete block is gated in: {qc}"
    );
    // An `end_frame` past the timeline is clamped, not refused: the report's
    // range and the first text line both show the clamped bounds.
    let over_long = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"start_frame": 0, "end_frame": 600}),
    )
    .await;
    assert_eq!(over_long.is_error, Some(false), "{over_long:?}");
    let over_long_text = over_long.content[0].as_text().unwrap().text.clone();
    assert!(
        over_long_text.starts_with("audio_qc range=0..60 "),
        "{over_long_text}"
    );
    assert_eq!(
        over_long.structured_content.as_ref().unwrap()["report"]["master"]["sample_frames"],
        96_000,
        "{over_long_text}"
    );
    let one_frame = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"start_frame": 0, "end_frame": 1}),
    )
    .await;
    assert_eq!(one_frame.is_error, Some(true), "{one_frame:?}");
    assert_eq!(
        one_frame.content[0].as_text().unwrap().text,
        "get_audio_qc needs at least one 400 ms gating block (19200 sample frames); got 1600"
    );
    let levels_short = invoke_capability(
        &client,
        "get_audio_levels",
        json!({"start_frame": 0, "end_frame": 1}),
    )
    .await;
    assert_eq!(levels_short.is_error, Some(true), "{levels_short:?}");
    assert_eq!(
        levels_short.content[0].as_text().unwrap().text,
        "get_audio_levels needs at least one 400 ms gating block (19200 sample frames); got 1600"
    );

    // -60 tenth-dB is -6 dB, so loudness and both peaks fall by 600.
    let prepared = prepare_plan(
        &client,
        0,
        json!([{
            "op": "set_track_mix",
            "track": 1,
            "gain_tenth_db": -60,
            "pan_percent": 0,
            "mute": false,
            "solo": false
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(0, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");
    let attenuated = invoke_capability(&client, "get_audio_qc", json!({})).await;
    assert_eq!(attenuated.is_error, Some(false), "{attenuated:?}");
    let body = attenuated.structured_content.as_ref().unwrap();
    assert_eq!(body["timeline_revision"], 1);
    let master = &body["report"]["master"];
    let after = master["integrated_lufs_hundredths"].as_i64().unwrap();
    assert!(
        (after - before + 600).abs() <= 5,
        "a -60 tenth-dB track gain must move the master by -600 LUFS hundredths, \
         measured {before} -> {after}"
    );
    let after_true_peak = master["true_peak_dbtp_hundredths"].as_i64().unwrap();
    assert!(
        (after_true_peak - true_peak + 600).abs() <= 5,
        "true peak {true_peak} -> {after_true_peak}"
    );
    let after_sample_peak = master["sample_peak_dbfs_hundredths"].as_i64().unwrap();
    assert!(
        (after_sample_peak - sample_peak + 600).abs() <= 5,
        "sample peak {sample_peak} -> {after_sample_peak}"
    );

    // AU3 §2.1 gloss on `get_audio_levels`: the master line spells the four
    // new fields, `none` for the two a 2 s programme cannot measure.
    let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(levels.is_error, Some(false), "{levels:?}");
    let levels_text = levels.content[0].as_text().unwrap().text.clone();
    let master_line = levels_text
        .lines()
        .find(|line| line.starts_with("master "))
        .unwrap();
    assert!(
        master_line.contains(&format!(
            " momentary_max={}",
            master["momentary_max_lufs_hundredths"].as_i64().unwrap()
        )) && master_line.contains(" short_term_max=none lra=none true_peak=")
            && master_line.ends_with(&format!("true_peak={after_true_peak}")),
        "{master_line}"
    );

    // A muted track is digital silence: measured, not refused, with
    // `integrated: null`, the lone `audio_silent` warning, and no Error.
    let prepared = prepare_plan(
        &client,
        1,
        json!([{
            "op": "set_track_mix",
            "track": 1,
            "gain_tenth_db": 0,
            "pan_percent": 0,
            "mute": true,
            "solo": false
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");
    let muted =
        invoke_capability(&client, "get_audio_qc", json!({"profile": "youtube1080p"})).await;
    assert_eq!(muted.is_error, Some(false), "{muted:?}");
    let qc = muted.structured_content.as_ref().unwrap()["report"].clone();
    assert!(qc["master"]["integrated_lufs_hundredths"].is_null(), "{qc}");
    assert!(
        qc["master"]["sample_peak_dbfs_hundredths"].is_null(),
        "{qc}"
    );
    assert!(qc["master"]["true_peak_dbtp_hundredths"].is_null(), "{qc}");
    assert!(qc["channel_balance_lu_hundredths"].is_null(), "{qc}");
    let exceptions = qc["exceptions"].as_array().unwrap();
    assert_eq!(
        exceptions.len(),
        1,
        "audio_silent suppresses the target checks: {exceptions:?}"
    );
    assert_eq!(exceptions[0]["code"], "audio_silent");
    assert_eq!(exceptions[0]["severity"], "warning");
    assert_eq!(qc["technical_pass"], true);
    let text = muted.content[0].as_text().unwrap().text.clone();
    assert!(
        text.contains("master lufs=none momentary_max=none short_term_max=none lra=none true_peak=none peak=none frames=96000"),
        "{text}"
    );
    assert!(
        text.contains("exception Warning audio_silent integrated_lufs_hundredths observed=none allowed=> -7000"),
        "{text}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU5 §7 A15: `get_audio_repair` is integer-reported, evidence-only and
/// revision-gated over the live endpoint.
///
/// The closed argument schema, the published first sentence, the stale
/// envelope, the inverted range and the two-point refusal need no decoder and
/// come first; the measurement itself runs on the real 440 Hz fixture, whose
/// steady tone is the honest worst case for a percentile floor — a signal with
/// no silence in it, where the 10th percentile is not a noise floor at all and
/// the SNR reads near zero. That is exactly the bias rule 21 makes the first
/// sentence carry, so the test asserts the direction rather than a number.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au5_get_audio_repair_is_evidence_only_and_revision_gated() {
    let generated = au3_sine_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let document = single_clip_document(asset);
    let duration = document.duration.0;
    assert_eq!(duration, 60, "the fixture is exactly 60 frames at 30 fps");
    let core = Core::spawn(document).unwrap();
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

    // The published schema: an inspector with exactly the five AU5 §4.1
    // arguments and nothing else accepted.
    let opened = client
        .call_tool(
            CallToolRequestParams::new("get_capability").with_arguments(
                json!({"name": "get_audio_repair"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(opened.is_error, Some(false));
    let opened = opened.structured_content.as_ref().unwrap();
    assert_eq!(opened["capability"]["kind"], "inspector");
    let properties = opened["input_schema"]["properties"].as_object().unwrap();
    for present in [
        "expected_revision",
        "start_frame",
        "end_frame",
        "track",
        "bus",
    ] {
        assert!(properties.contains_key(present), "missing {present}");
    }
    assert_eq!(properties.len(), 5, "{opened}");
    assert_eq!(opened["input_schema"]["additionalProperties"], json!(false));
    // Rule 76: `get_capability` publishes only the first sentence, so the
    // percentile and the DIRECTION of its bias have to be inside it.
    let summary = opened["capability"]["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("Measures a percentile noise floor, percentile SNR"),
        "{summary}"
    );
    assert!(
        summary.contains("10th-percentile short window, not a detected silence"),
        "{summary}"
    );
    assert!(
        summary.contains("higher floor and a lower SNR"),
        "the bias direction is what a planner needs: {summary}"
    );

    // A stale revision is the uniform envelope, refused before any decode.
    let stale = invoke_capability(
        &client,
        "get_audio_repair",
        json!({"expected_revision": revision + 7}),
    )
    .await;
    assert_eq!(stale.is_error, Some(true));
    let stale_body = stale.structured_content.as_ref().unwrap();
    assert_eq!(stale_body["code"], "stale_revision");
    assert_eq!(stale_body["applied"], false);
    assert_eq!(stale_body["evidence_only"], true);
    assert_eq!(stale_body["details"]["expected_revision"], revision + 7);
    assert_eq!(stale_body["details"]["actual_revision"], revision);

    // The range rule is `get_audio_levels`': an inverted range is refused by
    // name rather than clamped.
    let inverted = invoke_capability(
        &client,
        "get_audio_repair",
        json!({"start_frame": 30, "end_frame": 10}),
    )
    .await;
    assert_eq!(inverted.is_error, Some(true));
    assert_eq!(
        inverted.content[0].as_text().unwrap().text,
        "get_audio_repair needs start_frame < end_frame; got 30..10"
    );

    // The point rule is `get_audio_spectrum`': at most one of track and bus.
    let both = invoke_capability(&client, "get_audio_repair", json!({"track": 1, "bus": 1})).await;
    assert_eq!(both.is_error, Some(true));
    assert_eq!(
        both.content[0].as_text().unwrap().text,
        "get_audio_repair takes at most one of track and bus"
    );

    // `deny_unknown_fields`: a misspelled bound is a malformed request,
    // surfaced as a protocol error rather than silently defaulted.
    let unknown = client
        .call_tool(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({"name": "get_audio_repair", "arguments": {"start_fram": 0}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;
    assert!(unknown.is_err(), "{unknown:?}");

    // The measurement itself. `expected_revision` is deliberately absent:
    // this is an inspector, not a planner.
    let report = invoke_capability(&client, "get_audio_repair", json!({})).await;
    assert_eq!(report.is_error, Some(false), "{report:?}");
    let body = report.structured_content.as_ref().unwrap();
    // A15: exactly the two keys, and every leaf an integer, bool, string or
    // null.
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["report", "timeline_revision"],
        "{body}"
    );
    assert_eq!(body["timeline_revision"], revision);
    assert_integer_leaves("envelope", body);
    let repair = &body["report"];
    assert_eq!(repair["evidence_only"], true);
    assert_eq!(repair["range"], json!({"start": 0, "end": duration}));
    assert_eq!(repair["point"], json!("master"));
    assert_eq!(repair["sample_rate"], 48_000);
    assert_eq!(repair["sample_frames"], 96_000);
    assert_eq!(repair["window_milliseconds"], 10);
    assert_eq!(repair["provenance"]["engine"], "kinewright_audio_repair_v1");
    assert!(
        repair.get("export_ready").is_none(),
        "an evidence report never carries an export gate: {repair}"
    );
    // Two seconds of steady tone is 200 whole 10 ms windows, all but the odd
    // edge window energetic, so the percentiles are reported and their
    // difference is small: the 10th and 90th percentile of a constant signal
    // are nearly the same window level.
    let windows = repair["windows"].as_i64().unwrap();
    assert!(
        (190..=200).contains(&windows),
        "a 2 s programme holds 200 whole 10 ms windows: {repair}"
    );
    let floor = repair["noise_floor_dbfs_hundredths"].as_i64().unwrap();
    let signal = repair["signal_dbfs_hundredths"].as_i64().unwrap();
    let snr = repair["snr_db_hundredths"].as_i64().unwrap();
    assert_eq!(snr, signal - floor, "the SNR is the difference: {repair}");
    assert!(
        snr < 300,
        "a signal with no silence in it reads a near-zero SNR, which is the \
         percentile bias rule 21 publishes: {repair}"
    );
    // The 440 Hz sine carries no mains hum: every 50 and 60 Hz harmonic sits
    // at or under its own sixth-octave shoulders, so both summed excesses are
    // clamped to zero and neither raises `mains_hum_present`.
    assert_eq!(repair["hum_50_excess_db_hundredths"], 0, "{repair}");
    assert_eq!(repair["hum_60_excess_db_hundredths"], 0, "{repair}");
    // The click density is derived from the count, not measured twice.
    let clicks = repair["click_count"].as_i64().unwrap();
    assert_eq!(
        repair["click_density_per_minute"].as_i64().unwrap(),
        clicks * 60 * 48_000 / 96_000,
        "{repair}"
    );
    for harmonics in [
        "hum_50_harmonic_excess_db_hundredths",
        "hum_60_harmonic_excess_db_hundredths",
    ] {
        assert_eq!(
            repair[harmonics].as_array().unwrap().len(),
            4,
            "four harmonics or none: {repair}"
        );
    }

    // Rule 77: the rendered text names every figure with its unit and repeats
    // the percentile clause in prose, so a reader of the text alone still
    // learns what the floor is.
    let text = report.content[0].as_text().unwrap().text.clone();
    assert!(
        text.starts_with(&format!(
            "audio_repair range=0..60 point=master sample_rate=48000 sample_frames=96000 window=10ms windows={windows}"
        )),
        "{text}"
    );
    assert!(
        text.contains(
            "10th-percentile 10 ms window and the signal the 90th, not a detected silence"
        ),
        "{text}"
    );
    assert!(text.contains("in dBFS hundredths"), "{text}");
    assert!(text.contains("evidence_only=true"), "{text}");
    assert!(!text.contains('"'), "{text}");

    // One track's stem is a legal point, and so is a bus.
    let on_track = invoke_capability(&client, "get_audio_repair", json!({"track": 1})).await;
    assert_eq!(on_track.is_error, Some(false), "{on_track:?}");
    assert_eq!(
        on_track.structured_content.as_ref().unwrap()["report"]["point"],
        json!({"track": 1}),
        "{on_track:?}"
    );

    // Nothing on this path moved the document or the revision.
    assert_eq!(
        serde_json::to_string(&query_document(&core)).unwrap(),
        serde_json::to_string(&before).unwrap(),
        "an inspector never edits"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU5 §7 A16: `effect_documentation`'s profile hatch, over the live endpoint.
///
/// The five spliced tools carry the pattern sentence exactly once and no
/// `profile_band` row at all, and the sentence's bounds come from the Core
/// descriptor rather than a literal, so §2.1's table and the published prose
/// cannot drift. The saving the hatch buys is asserted against the enumeration
/// it replaces, built here from the same descriptor.
#[test]
fn au5_effect_documentation_hatches_the_noise_profile() {
    let tools = kinewright_agent::operation_tools().unwrap();
    let denoise = kinewright_core::effect_descriptor("audio_denoise")
        .expect("AU5 Part A registers the denoiser");
    let bands = denoise
        .parameters
        .iter()
        .filter(|parameter| kinewright_core::is_noise_profile_parameter(parameter.name))
        .collect::<Vec<_>>();
    assert_eq!(bands.len(), 31, "AU5 §2.1: 31 profile rows");
    // The row spelling `effect_documentation` would otherwise have emitted,
    // built from the same descriptor the hatch reads.
    let enumerated = bands
        .iter()
        .map(|parameter| {
            format!(
                "{}={}..={}, neutral {}",
                parameter.name, parameter.min, parameter.max, parameter.neutral
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    for name in [
        "add_effect",
        "insert_effect",
        "set_effect_param",
        "set_effect_keyframes",
        "clear_effect_keyframes",
    ] {
        let description = tools
            .iter()
            .find(|definition| definition.tool.name == name)
            .expect("every spliced effect tool is generated")
            .tool
            .description
            .as_deref()
            .expect("every generated tool carries a description")
            .to_owned();
        assert!(
            description.contains("audio_denoise("),
            "{name} must document the new descriptor"
        );
        assert!(
            !description.contains("profile_band0") && !description.contains("profile_band1"),
            "{name} must never enumerate a profile row"
        );
        assert_eq!(
            description.matches("profile_band{01..31}_tenth_db").count(),
            1,
            "{name} carries the pattern sentence exactly once"
        );
        assert!(
            description.contains(
                "profile_band{01..31}_tenth_db=-1200..=0, neutral -1200, one per ISO third-octave centre 20 Hz..20 kHz low to high; write all 31 or none; learn them with plan_dialogue_repair"
            ),
            "{name}: {description}"
        );
        assert!(
            !description.contains(&enumerated),
            "{name} must not carry the enumeration"
        );
        // AU5 §0 R81/R82: the whole AU5 growth on this description is the
        // three new descriptor sections, and it is 758 B — the per-tool figure
        // the registry ledger's description-byte split is built from. Pinning
        // it here makes that split falsifiable without re-measuring the whole
        // registry.
        let first = description
            .find("; audio_denoise(")
            .expect("the three AU5 descriptors are appended after audio_true_peak_limiter");
        let trailer = description
            .find(". cube_lut additionally requires")
            .expect("the effect documentation ends with the cube_lut trailer");
        assert_eq!(
            trailer - first,
            758,
            "{name}: AU5's three descriptor sections must measure 758 B"
        );
        // The hatch is worth more than four times its own length on every one
        // of the five tools, which is rule 78's argument measured rather than
        // asserted.
        let pattern = description
            .split_once("profile_band{01..31}")
            .map(|(_, rest)| {
                rest.split_once("plan_dialogue_repair")
                    .map_or(rest.len(), |(head, _)| {
                        head.len() + "plan_dialogue_repair".len()
                    })
            })
            .unwrap()
            + "profile_band{01..31}".len();
        assert!(
            pattern * 4 < enumerated.len(),
            "{name}: pattern {pattern} B against enumerated {} B",
            enumerated.len()
        );
    }
}

/// AU3 §6.3 / §7 B11: the normalization planner converges through the real
/// engine, and what it commits is a true-peak limiter.
///
/// The plan is built by measuring, applying, and re-measuring the candidate in
/// memory, so a planner that emitted the wrong node would still converge on
/// *loudness*; the true-peak assertion is what separates the two. The
/// committed bus is read back from the document — node names, ids, and the
/// limiter's four parameters — and then the mix is re-measured through
/// `get_audio_levels`, whose master line carries the decoded true peak AU3
/// Part A added.
///
/// F17b: the bus declares 5 ms of `CHAIN_LOOKAHEAD_MILLISECONDS`' 20 ms
/// budget, so committing it re-cues a running playback exactly once.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au3_plan_audio_normalization_converges_through_the_real_engine() {
    let generated = au3_sine_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let planned = invoke_capability(
        &client,
        "plan_audio_normalization",
        json!({
            "track_ids": [1],
            "target_lufs_hundredths": -1_600,
            "maximum_sample_peak_dbfs_hundredths": -100,
            "tolerance_hundredths": 100
        }),
    )
    .await;
    assert_eq!(
        planned.is_error,
        Some(false),
        "the planner must converge on the real 440 Hz fixture: {planned:?}"
    );
    let body = planned.structured_content.as_ref().unwrap();
    let revision = body["timeline_revision"].as_u64().unwrap();
    // AU3 §6.3: the headroom is core's, and the processing ceiling is the
    // requested ceiling minus it.
    assert_eq!(body["lossy_codec_peak_headroom_hundredths"], 200);
    assert_eq!(body["processing_ceiling_dbfs_hundredths"], -300);
    // The predicted measurement carries AU3's true peak through `AudioLoudness`.
    assert!(
        body["predicted"]["true_peak_dbtp_hundredths"].is_i64(),
        "{body}"
    );
    let predicted = body["predicted"]["integrated_lufs_hundredths"]
        .as_i64()
        .expect("a converged plan predicts a programme loudness");
    assert!(
        (predicted + 1_600).abs() <= 100,
        "the plan is only returned inside the tolerance: {predicted}"
    );

    let plan_id = body["prepared_edit_plan"]["plan_id"].clone();
    let committed = client
        .call_tool(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({"plan_id": plan_id, "expected_revision": revision})
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

    // What landed in the document: one delivery bus whose last node is the
    // inter-sample-aware limiter, never the legacy sample-peak clamp.
    let document = query_document(&core);
    let bus = document
        .audio_mix
        .buses
        .last()
        .expect("the plan commits one delivery bus");
    assert_eq!(bus.name, "Delivery normalization");
    assert_eq!(bus.tracks, vec![TrackId(1)]);
    let names = bus
        .effects
        .iter()
        .map(|effect| effect.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !names.contains(&"audio_limiter"),
        "the legacy clamp must not reach a committed document: {names:?}"
    );
    assert_eq!(names.last(), Some(&"audio_true_peak_limiter"), "{names:?}");
    let limiter = bus.effects.last().unwrap();
    for (parameter, value) in [
        ("ceiling_tenth_db", -30),
        ("lookahead_milliseconds", 5),
        ("release_milliseconds", 50),
        ("true_peak", 1),
    ] {
        assert_eq!(
            limiter.static_integer_parameter(parameter),
            Some(value),
            "{parameter}"
        );
    }
    assert_eq!(
        kinewright_core::chain_lookahead_milliseconds(&bus.effects),
        5,
        "F17b: one re-cue, 5 ms of the 20 ms budget"
    );

    // The committed mix really measures where the plan said it would, and its
    // decoded true peak is under the requested ceiling.
    let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(levels.is_error, Some(false), "{levels:?}");
    let master = &levels.structured_content.as_ref().unwrap()["report"]["master"];
    let integrated = master["integrated_lufs_hundredths"]
        .as_i64()
        .expect("the normalized master is not silent");
    assert!(
        (integrated + 1_600).abs() <= 100,
        "master {integrated} is outside -1600 +/- 100"
    );
    let true_peak = master["true_peak_dbtp_hundredths"]
        .as_i64()
        .expect("the normalized master has a true peak");
    assert!(
        true_peak <= -100,
        "the true-peak limiter must hold the requested ceiling: {true_peak}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU3 §6.3 / §7 B11, review nit: the same planner path on hot, high-crest
/// material, where the true-peak limiter is doing real work.
///
/// The sibling test above converges on the steady sine, whose normalized peak
/// sits about 13 dB under the processing ceiling — so its `true_peak <= -100`
/// assertion would hold even with the limiter bypassed. `au3_hot_sine_media`
/// has a peak-to-loudness ratio near 17 dB, so at the same -1600 / -100 / 100
/// contract the gain the planner needs pushes the programme peak *over* the
/// ceiling and the limiter has to pull it back. The proof is differential: the
/// committed bus is re-upserted with its last node removed and the mix is
/// measured again, so the number compared against is the same chain, the same
/// compressor, and the same gain, with only the limiter gone.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au3_plan_audio_normalization_engages_the_true_peak_limiter_on_hot_material() {
    let generated = au3_hot_sine_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // The fixture as the engine measures it, before any processing.
    let source = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(source.is_error, Some(false), "{source:?}");
    let source_master = &source.structured_content.as_ref().unwrap()["report"]["master"];
    let source_lufs = source_master["integrated_lufs_hundredths"]
        .as_i64()
        .expect("the hot fixture is not silent");
    let source_sample_peak = source_master["sample_peak_dbfs_hundredths"]
        .as_i64()
        .expect("the hot fixture has a sample peak");
    let source_true_peak = source_master["true_peak_dbtp_hundredths"]
        .as_i64()
        .expect("the hot fixture has a true peak");
    println!(
        "au3 hot fixture: integrated={source_lufs} sample_peak={source_sample_peak} true_peak={source_true_peak} peak_to_loudness={}",
        source_true_peak - source_lufs
    );
    assert!(
        source_true_peak - source_lufs > 1_300,
        "the fixture only engages the limiter if its peak-to-loudness ratio clears \
         ceiling - target = 1300 hundredths: {source_true_peak} over {source_lufs}"
    );

    let planned = invoke_capability(
        &client,
        "plan_audio_normalization",
        json!({
            "track_ids": [1],
            "target_lufs_hundredths": -1_600,
            "maximum_sample_peak_dbfs_hundredths": -100,
            "tolerance_hundredths": 100
        }),
    )
    .await;
    assert_eq!(
        planned.is_error,
        Some(false),
        "the planner must converge on the hot fixture too: {planned:?}"
    );
    let body = planned.structured_content.as_ref().unwrap();
    let revision = body["timeline_revision"].as_u64().unwrap();
    assert_eq!(body["processing_ceiling_dbfs_hundredths"], -300);
    let current_lufs = body["current"]["integrated_lufs_hundredths"]
        .as_i64()
        .unwrap();
    let current_peak = body["current"]["sample_peak_dbfs_hundredths"]
        .as_i64()
        .unwrap();
    let predicted_lufs = body["predicted"]["integrated_lufs_hundredths"]
        .as_i64()
        .expect("a converged plan predicts a programme loudness");
    let predicted_peak = body["predicted"]["true_peak_dbtp_hundredths"]
        .as_i64()
        .expect("a converged plan predicts a true peak");
    assert!(
        (predicted_lufs + 1_600).abs() <= 100,
        "the plan is only returned inside the tolerance: {predicted_lufs}"
    );
    println!(
        "au3 hot plan: current={current_lufs} predicted={predicted_lufs} predicted_true_peak={predicted_peak}"
    );

    let plan_id = body["prepared_edit_plan"]["plan_id"].clone();
    let committed = client
        .call_tool(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({"plan_id": plan_id, "expected_revision": revision})
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

    // The same chain contract the steady-sine test pins, on hot material: a
    // true-peak limiter last, never the legacy sample-peak clamp, and 5 ms of
    // the 20 ms re-cue budget.
    let document = query_document(&core);
    let bus = document
        .audio_mix
        .buses
        .last()
        .expect("the plan commits one delivery bus")
        .clone();
    let names = bus
        .effects
        .iter()
        .map(|effect| effect.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !names.contains(&"audio_limiter"),
        "the legacy clamp must not reach a committed document: {names:?}"
    );
    assert_eq!(names.last(), Some(&"audio_true_peak_limiter"), "{names:?}");
    assert_eq!(
        kinewright_core::chain_lookahead_milliseconds(&bus.effects),
        5,
        "F17b: one re-cue, 5 ms of the 20 ms budget"
    );
    // The compressor is engaged, not the 1:1 pass-through the steady sine
    // gets: this is the branch whose peak the limiter has to finish.
    let compressor = bus
        .effects
        .first()
        .expect("the positive-gain branch leads with the compressor");
    assert_eq!(compressor.name, "audio_compressor");
    assert_eq!(
        compressor.static_integer_parameter("ratio_hundredths"),
        Some(400),
        "the hot fixture must take the compression-required branch: {:?}",
        compressor.parameters
    );
    let planned_gain_tenth_db = bus
        .effects
        .iter()
        .filter_map(|effect| match effect.name.as_str() {
            "audio_compressor" => effect.static_integer_parameter("makeup_gain_tenth_db"),
            "audio_gain" => effect.static_integer_parameter("gain_tenth_db"),
            _ => None,
        })
        .sum::<i64>();
    println!(
        "au3 hot chain: {names:?} planned_gain_tenth_db={planned_gain_tenth_db} compressor_threshold_tenth_db={:?}",
        compressor.static_integer_parameter("threshold_tenth_db")
    );
    // The planner's own compression-required test, restated on the numbers it
    // published: the gain it committed carries the measured peak over the
    // processing ceiling, which is exactly why the compressor is engaged and
    // why the limiter below has a peak left to catch.
    assert!(
        current_peak + planned_gain_tenth_db * 10 > -300,
        "measured peak {current_peak} plus {planned_gain_tenth_db} tenth dB must clear the -300 processing ceiling"
    );

    // What the committed chain, limiter included, actually measures.
    let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(levels.is_error, Some(false), "{levels:?}");
    let master = &levels.structured_content.as_ref().unwrap()["report"]["master"];
    let limited_integrated = master["integrated_lufs_hundredths"]
        .as_i64()
        .expect("the normalized master is not silent");
    assert!(
        (limited_integrated + 1_600).abs() <= 100,
        "master {limited_integrated} is outside -1600 +/- 100"
    );
    let limited_true_peak = master["true_peak_dbtp_hundredths"]
        .as_i64()
        .expect("the normalized master has a true peak");
    assert!(
        limited_true_peak <= -100,
        "the true-peak limiter must hold the requested ceiling: {limited_true_peak}"
    );

    // The differential: the same bus with only the limiter removed. If the
    // limiter were a no-op, this would measure the same peak.
    let mut unlimited_bus = bus.clone();
    let removed = unlimited_bus
        .effects
        .pop()
        .expect("the limiter is the node under test");
    assert_eq!(removed.name, "audio_true_peak_limiter");
    let prepared = prepare_plan(
        &client,
        revision + 1,
        json!([{
            "op": "upsert_audio_bus",
            "bus": serde_json::to_value(&unlimited_bus).unwrap()
        }]),
    )
    .await;
    assert_eq!(prepared.is_error, Some(false), "{prepared:?}");
    let committed = client
        .call_tool(commit_request(revision + 1, &prepared))
        .await
        .unwrap();
    assert_eq!(committed.is_error, Some(false), "{committed:?}");
    assert_eq!(
        query_document(&core)
            .audio_mix
            .buses
            .last()
            .unwrap()
            .effects
            .last()
            .unwrap()
            .name,
        "audio_compressor",
        "the comparison chain must be the committed one minus its limiter"
    );

    let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(levels.is_error, Some(false), "{levels:?}");
    let master = &levels.structured_content.as_ref().unwrap()["report"]["master"];
    let unlimited_true_peak = master["true_peak_dbtp_hundredths"]
        .as_i64()
        .expect("the unlimited master has a true peak");
    let unlimited_integrated = master["integrated_lufs_hundredths"].as_i64().unwrap();
    println!(
        "au3 hot delivery: limited integrated={limited_integrated} true_peak={limited_true_peak}; unlimited integrated={unlimited_integrated} true_peak={unlimited_true_peak}; reduction={}",
        unlimited_true_peak - limited_true_peak
    );
    assert!(
        unlimited_true_peak - limited_true_peak >= 200,
        "the limiter must move the delivered peak: limited {limited_true_peak}, unlimited {unlimited_true_peak}"
    );
    assert!(
        unlimited_true_peak > -100,
        "without the limiter the same gain overshoots the requested ceiling: {unlimited_true_peak}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU3 §6.1/§6.2/§6.4 and §7 B10: `queue_export` normalizes and the job record
/// carries both the report and the decoded measurement of the written file.
///
/// This is the whole Part B agent path end to end through the real engine: the
/// request's `normalize_loudness` becomes `settings.loudness_normalization =
/// profile.loudness_target()`, the export reports what its normalization step
/// did, the finished file is decoded and measured against that same target,
/// and `get_export_jobs` renders §6.4's two lines. A `NotImplemented` failure
/// here means the media half of Part B is missing, not that this path is a
/// stub.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au3_queue_export_normalizes_and_verifies_audio() {
    let generated = au3_sine_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let directory = std::env::temp_dir().join(format!(
        "kinewright-au3-queue-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("normalized.mp4");

    let core = Core::spawn(single_clip_document(asset)).unwrap();
    let server =
        McpServer::start_with_exporter(core.clone(), media.clone(), media.clone(), media.clone())
            .unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // The target is never on the request: the request says yes, the profile
    // says what to. `get_delivery_profiles` publishes the same number.
    let profiles = invoke_capability(&client, "get_delivery_profiles", json!({})).await;
    let published = profiles.structured_content.as_ref().unwrap()["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["id"] == "source_master")
        .cloned()
        .expect("source_master is published");
    assert_eq!(
        published["loudness_target"]["integrated_lufs_hundredths"],
        kinewright_core::DeliveryProfile::SourceMaster
            .loudness_target()
            .integrated_lufs_hundredths
    );

    let queued = invoke_capability(
        &client,
        "queue_export",
        json!({
            "expected_revision": 0,
            "output_path": output,
            "profile": "source_master",
            "verify": false,
            "normalize_loudness": true
        }),
    )
    .await;
    assert_eq!(
        queued.is_error,
        Some(false),
        "{:?}",
        queued.structured_content
    );

    // Poll the queue rather than sleeping on a fixed budget: a real encode
    // plus a real decode is the slowest thing in this file.
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let job = loop {
        let jobs = invoke_capability(&client, "get_export_jobs", json!({})).await;
        assert_eq!(jobs.is_error, Some(false), "{jobs:?}");
        let record = jobs.structured_content.as_ref().unwrap()["jobs"][0].clone();
        let text = jobs.content[0].as_text().unwrap().text.clone();
        if matches!(
            record["state"].as_str(),
            Some("completed" | "failed" | "cancelled")
        ) {
            break (record, text);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the export never settled: {record}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let (record, text) = job;
    assert_eq!(record["state"], "completed", "{record}");
    assert_eq!(record["error"], serde_json::Value::Null);
    assert!(
        output.is_file(),
        "the deliverable is where it was asked for"
    );

    // AU3 §5.2: the export reported what its normalization step did.
    let report = record["audio_report"].clone();
    let target = kinewright_core::DeliveryProfile::SourceMaster.loudness_target();
    assert_eq!(
        report["target"]["integrated_lufs_hundredths"], target.integrated_lufs_hundredths,
        "{record}"
    );
    assert_eq!(
        report["skipped_reason"],
        serde_json::Value::Null,
        "{record}"
    );
    assert_eq!(report["on_target"], true, "{record}");
    let after = report["after"]["integrated_lufs_hundredths"]
        .as_i64()
        .unwrap();
    assert!(
        (after - i64::from(target.integrated_lufs_hundredths)).abs()
            <= i64::from(target.tolerance_lu_hundredths),
        "the pre-encode master is on target: {after}"
    );

    // AU3 §5.3: the written file was decoded and measured against that target,
    // independently of `verify: false`.
    assert_eq!(
        record["audio_verification_unavailable_reason"],
        serde_json::Value::Null,
        "{record}"
    );
    let verification = record["audio_verification"].clone();
    assert_eq!(
        verification["target"]["integrated_lufs_hundredths"],
        target.integrated_lufs_hundredths
    );
    assert_eq!(verification["sample_rate"], 48_000);
    assert_eq!(verification["channels"], 2);
    assert_eq!(verification["technical_pass"], true, "{verification}");
    let decoded = verification["measured"]["integrated_lufs_hundredths"]
        .as_i64()
        .expect("the written file is not silent");
    assert!(
        (decoded - i64::from(target.integrated_lufs_hundredths)).abs() <= 100,
        "the decoded delivery is on target: {decoded}"
    );
    let decoded_peak = verification["measured"]["true_peak_dbtp_hundredths"]
        .as_i64()
        .expect("the written file has a true peak");
    assert!(
        decoded_peak <= i64::from(target.true_peak_ceiling_dbtp_hundredths),
        "the delivered true peak is under the ceiling: {decoded_peak}"
    );
    // `verify: false` still governs the video comparison only.
    assert_eq!(record["verification"], serde_json::Value::Null);

    // AU3 §6.4: the two text lines, in order, beside CC6's unchanged count.
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "1 retained export job(s)", "{text}");
    assert_eq!(
        lines[1],
        format!(
            "job 1 audio lufs={decoded} true_peak={decoded_peak} lra={} target={} \
             ceiling={} technical_pass=true",
            verification["measured"]["loudness_range_lu_hundredths"]
                .as_i64()
                .map_or_else(|| "none".to_owned(), |value| value.to_string()),
            target.integrated_lufs_hundredths,
            target.true_peak_ceiling_dbtp_hundredths
        ),
        "{text}"
    );
    assert_eq!(
        lines[2],
        format!(
            "job 1 normalization gain={} passes={} reduction={} on_target=true skipped=none",
            report["applied_gain_hundredths"].as_i64().unwrap(),
            report["limiter_passes"].as_i64().unwrap(),
            report["peak_reduction_hundredths"].as_i64().unwrap()
        ),
        "{text}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
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

    // CC7 §4(a)(1) failing direction: the reference may not also be a
    // candidate, so "the reference was retained" cannot be satisfied by
    // matching it against itself.
    let self_match = invoke_capability(
        &client,
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
        // printed and never read. The magnitude itself has no constant in
        // `cc7_scenarios` (recorded as owed in the errata; §4(b)(3) states
        // 41 538 in prose and §2.1 forbids restating it here), but the
        // channel it lands on is asserted: the excursion's depth is on blue
        // alone, so a run that clipped red or green fails here rather than in
        // a printed line nobody reads.
        assert!(
            report["range"]["blue"]["maximum_over_excursion_millionths"]
                .as_i64()
                .unwrap_or_else(|| panic!("the range report publishes a depth: {report}"))
                > 0,
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
                assert_ne!(
                    body["hashes"]["before_rgba8_pixels_sha256"],
                    body["hashes"]["after_rgba8_pixels_sha256"],
                    "the warm look must change the picture: {body}"
                );
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
    assert_ne!(
        availability(&unrelocated, imported_id)["availability"]["kind"],
        "verified",
        "a bare relocation cannot report verified: {unrelocated}"
    );

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
        let analytic = CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS[index];
        for (axis, key) in ["center_x_basis_points", "center_y_basis_points"]
            .into_iter()
            .enumerate()
        {
            let error = (sample[key].as_i64().unwrap() - analytic[axis]).abs();
            worst = worst.max(error);
            assert!(
                error <= CC7_TRACK_TOLERANCE_BASIS_POINTS,
                "frame {frame} axis {axis} is {error} bp off the analytic centre: {sample}"
            );
        }
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

    // CC7 §4(f)(1) failing direction: the tool's own default drops nothing, so
    // `CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS` is load-bearing.
    let defaulted = invoke_capability(
        &client,
        "track_matte_window",
        track_arguments(
            CC7_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS,
            CC7_TRACK_STEP_FRAMES,
        ),
    )
    .await;
    let defaulted_body = defaulted.structured_content.as_ref().unwrap();
    assert_eq!(defaulted.is_error, Some(false), "{defaulted_body}");
    assert!(
        defaulted_body["low_confidence_samples"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the 5000 default drops nothing: {defaulted_body}"
    );
    assert_ne!(
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        CC7_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS
    );

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

/// AU4 §6.1 rule 130: 12 s of steady 440 Hz music at 30 fps, the bed the duck
/// rides on.
///
/// A steady tone is deliberate: the ducked and the un-ducked measurement
/// windows sit at different *times*, so anything but constant material would
/// fold a content difference into the delta rule 130 attributes to the duck.
fn au4_music_media() -> GeneratedMedia {
    let mut arguments = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000",
        "-frames:v",
        "360",
        "-t",
        "12.0",
    ];
    arguments.extend(MANAGED_BT709_ENCODE_ARGUMENTS);
    arguments.extend(["-c:a", "aac", "-shortest"]);
    GeneratedMedia::ffmpeg("au4-duck-music", &arguments, "mp4")
}

/// AU4 §6.1 rule 130: 12 s of digital silence carrying one gated burst from
/// 2.0 s to 4.0 s, so the merged dialogue span is `[60, 120)` in project
/// frames and the held floor `[60, 126)`.
///
/// The 6.0 s to 8.0 s stretch is silent, which is what gives the un-ducked
/// measurement window rule 128.2 needs — the gap from the release key at 138
/// to the end of the project is 222 frames, far past `release + 400 ms`.
fn au4_dialogue_media() -> GeneratedMedia {
    let mut arguments = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30",
        "-f",
        "lavfi",
        "-i",
        "aevalsrc=0.30*sin(2*PI*300*t)*between(t\\,2\\,3.999):s=48000",
        "-frames:v",
        "360",
        "-t",
        "12.0",
    ];
    arguments.extend(MANAGED_BT709_ENCODE_ARGUMENTS);
    arguments.extend(["-c:a", "aac", "-shortest"]);
    GeneratedMedia::ffmpeg("au4-duck-dialogue", &arguments, "mp4")
}

/// AU4 §6.1 rule 130: the accepted error between the requested depth and the
/// measured un-ducked/ducked delta, in hundredths of LU.
const PLAN_DUCKING_DEPTH_BUDGET_HUNDREDTHS: i64 = 50;

/// AU4 §7 B11, rule 130: `plan_audio_ducking` converges through the real
/// engine.
///
/// On AU3's planner template: invoke, assert the structured windows and the
/// relative keying, commit, assert the committed `TrackMix.gain_curve`, then
/// re-measure the music stem with `get_audio_levels` over a ducked window and
/// an un-ducked window and assert the two differ by the requested depth.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au4_plan_audio_ducking_converges_through_the_real_engine() {
    let music_media = au4_music_media();
    let dialogue_media = au4_dialogue_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let music = media.probe(music_media.path()).unwrap();
    let dialogue = media.probe(dialogue_media.path()).unwrap();
    assert_eq!(music.duration, TimeCode(360), "12 s at 30 fps");
    assert_eq!(dialogue.duration, TimeCode(360));

    let mut document = single_clip_document(music.clone());
    document.media_pool.push(dialogue.clone());
    document.tracks.push(Track {
        id: TrackId(2),
        kind: TrackKind::Audio,
        sync_lock: false,
        clips: vec![Clip {
            id: ClipId(2),
            asset: dialogue.id,
            source_range: TimeCode::ZERO..dialogue.duration,
            content: kinewright_core::ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
            audio_gain_curve: None,
        }],
    });
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // Silence analysis is asynchronous. The first call requests it and is
    // refused by name; the loop is the readiness refusal's other branch.
    let depth = -120_i64;
    let arguments = json!({
        "music_track": 1,
        "dialogue_tracks": [2],
        "depth_tenth_db": depth,
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let planned = loop {
        let result = invoke_capability(&client, "plan_audio_ducking", arguments.clone()).await;
        if result.is_error == Some(false) {
            break result;
        }
        let text = result.content[0].as_text().unwrap().text.clone();
        assert!(
            text.contains("silence analysis is not ready"),
            "the only expected refusal here is a pending analysis: {text}"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "silence analysis did not finish: {text}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let body = planned.structured_content.as_ref().unwrap();
    let revision = body["timeline_revision"].as_u64().unwrap();
    assert_eq!(
        body["windows"],
        json!([{"start_frame": 60, "end_frame": 126}]),
        "the 2.0-4.0 s burst plus the 200 ms hold is a 66-frame floor: {body}"
    );
    // Rule 124: the parked fader is unity here, so the floor is the depth
    // itself — and it is the *sum*, not the depth, that is committed.
    assert_eq!(body["parked_gain_tenth_db"], 0);
    assert_eq!(
        body["ducked_gain_tenth_db"].as_i64().unwrap(),
        body["parked_gain_tenth_db"].as_i64().unwrap() + depth
    );
    assert_eq!(body["keyframe_count"], 4);
    let plan_measured = body["measured"].clone();
    assert!(
        plan_measured["delta_hundredths"].is_i64(),
        "both flat windows clear the gating block by a wide margin: {body}"
    );
    assert_eq!(
        body["measurement_unavailable_reason"],
        serde_json::Value::Null
    );

    let plan_id = body["prepared_edit_plan"]["plan_id"].clone();
    let committed = client
        .call_tool(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({"plan_id": plan_id, "expected_revision": revision})
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

    // What landed: one gain curve on the music track, four keys, the attack
    // key 5 frames before the first spoken frame and the release key 12 after
    // the held floor ends.
    let document = query_document(&core);
    let curve = document
        .track_mix(TrackId(1))
        .gain_curve
        .expect("the plan commits a gain curve on the music track");
    assert_eq!(curve.keyframes.len(), 4, "{curve:?}");
    assert_eq!(curve.keyframes.first().unwrap().at, TimeCode(55));
    assert_eq!(curve.keyframes.last().unwrap().at, TimeCode(138));
    assert_eq!(curve.value_at(TimeCode(60)), Some(depth));
    assert_eq!(curve.value_at(TimeCode(125)), Some(depth));
    assert_eq!(curve.value_at(TimeCode(200)), Some(0));

    // The engine's own answer: the music stem measured inside the ducked
    // floor and inside the un-ducked stretch.
    let ducked = au4_music_track_loudness(&client, 66, 120).await;
    let unducked = au4_music_track_loudness(&client, 180, 240).await;
    let delta = ducked - unducked;
    println!(
        "AU4_DUCK depth={depth} unducked={unducked} ducked={ducked} delta={delta} budget={PLAN_DUCKING_DEPTH_BUDGET_HUNDREDTHS}"
    );
    assert!(
        (delta - depth * 10).abs() <= PLAN_DUCKING_DEPTH_BUDGET_HUNDREDTHS,
        "the committed ride must really move the music stem by {depth} tenth dB: \
         unducked={unducked} ducked={ducked} delta={delta}"
    );
    // The planner's own measurement agrees with the re-measurement, which is
    // what makes rule 129's `measured` worth publishing.
    assert!(
        (plan_measured["delta_hundredths"].as_i64().unwrap() - delta).abs()
            <= PLAN_DUCKING_DEPTH_BUDGET_HUNDREDTHS,
        "plan {plan_measured} against re-measured {delta}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// The music track's integrated loudness over one project window.
async fn au4_music_track_loudness(
    client: &RunningService<RoleClient, ()>,
    start_frame: i64,
    end_frame: i64,
) -> i64 {
    let levels = invoke_capability(
        client,
        "get_audio_levels",
        json!({"start_frame": start_frame, "end_frame": end_frame}),
    )
    .await;
    assert_eq!(levels.is_error, Some(false), "{levels:?}");
    let report = &levels.structured_content.as_ref().unwrap()["report"];
    // AU4 §6.1 rule 128.1: the agent restates media's private audio rate as
    // `MIX_MEASUREMENT_SAMPLE_RATE`, and this is where that restatement meets
    // the real decoder — `LOUDNESS_GATING_BLOCK_FRAMES / rate == 400 ms` is
    // only a fact about the gating block if the mix really runs at 48 kHz.
    assert_eq!(
        report["master"]["sample_rate"].as_u64(),
        Some(kinewright_agent::MIX_MEASUREMENT_SAMPLE_RATE),
        "the real mix path measures at the rate the agent's constant claims: {report}"
    );
    report["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["track"] == 1)
        .expect("the music track is reported")["levels"]["integrated_lufs_hundredths"]
        .as_i64()
        .expect("the music stem is not silent")
}

/// AU4 §7 B12: `plan_clip_fades` through the real engine and the live
/// endpoint.
///
/// The measurement is the real mix path, so the head and tail windows are
/// decoded, not scripted; the clip is a full-scale-ish steady tone, so both
/// windows read well above the -4000 hundredth default. The plan emits
/// `SetClipAudio` only, and the committed document carries the fades and
/// nothing else.
///
/// **AU5 §7 A17's regression.** AU4's two 400 ms `mix_levels` renders per clip
/// became one `mix_window_levels` pass per track (§3.8 rule 67), and this test
/// is the proof that the accessor agrees with the thing it replaced: the same
/// fixture proposes the same fades — the editor's 7-frame fade-in kept, a
/// 1-frame fade-out added — over a 20 ms RMS window instead of a 400 ms true
/// peak. Only the window figures and the name of the published evidence move.
#[tokio::test(flavor = "multi_thread")]
async fn au4_plan_clip_fades_measures_the_real_mix_and_commits_set_clip_audio() {
    let generated = au4_music_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let mut document = single_clip_document(asset);
    // An editor-set fade-in the planner must not overwrite.
    document.tracks[0].clips[0].audio_fade_in_frames = TimeCode(7);
    document.tracks[0].clips[0].audio_gain_tenth_db = -35;
    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // `deny_unknown_fields` closes the schema: a misspelled threshold is a
    // protocol-level refusal that names the three accepted fields, not a
    // silent fall back to the default.
    let misspelled = client
        .call_tool(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({
                    "name": "plan_clip_fades",
                    "arguments": {"threshold_dbfs_hundreths": -4_000}
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect_err("an unknown field is refused");
    assert!(
        misspelled
            .to_string()
            .contains("unknown field `threshold_dbfs_hundreths`"),
        "{misspelled}"
    );

    let planned = invoke_capability(&client, "plan_clip_fades", json!({"tracks": [1]})).await;
    assert_eq!(planned.is_error, Some(false), "{planned:?}");
    let body = planned.structured_content.as_ref().unwrap();
    let revision = body["timeline_revision"].as_u64().unwrap();
    // Rule 131 in the terms B12 asks for, on AU5's window.
    assert_eq!(body["window_milliseconds"], 20);
    assert_eq!(body["window_sample_frames"], 960);
    assert_eq!(body["window_project_frames"], 1);
    assert_eq!(body["fade_frames"], 1, "20 ms is 1 project frame at 30 fps");
    assert_eq!(body["skipped"], json!([]));
    let clips = body["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 1, "{body}");
    assert_eq!(clips[0]["fade_in_frames"], 7, "a non-zero fade is kept");
    assert_eq!(clips[0]["fade_out_frames"], 1);
    assert!(
        clips[0]["tail_dbfs_hundredths"].as_i64().unwrap() > -4_000,
        "the sine's tail window is hot: {body}"
    );

    let plan_id = body["prepared_edit_plan"]["plan_id"].clone();
    let committed = client
        .call_tool(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({"plan_id": plan_id, "expected_revision": revision})
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

    let clip = query_document(&core).tracks[0].clips[0].clone();
    assert_eq!(clip.audio_fade_in_frames, TimeCode(7));
    assert_eq!(clip.audio_fade_out_frames, TimeCode(1));
    assert_eq!(
        clip.audio_gain_tenth_db, -35,
        "SetClipAudio carries the existing gain through"
    );
    assert!(
        clip.audio_gain_curve.is_none(),
        "rule 133: the fade planner adds no curve"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

// ===========================================================================
// AU5 Part B — the agent's real-engine lanes (§7 B2, B6, B7, B10, B11).
//
// Every fixture here is exact `wav_f32` bytes through `GeneratedMedia::
// from_bytes`, never lavfi, for AU5 §3.11's reason: the provisioned FFmpeg's
// `sine` emits -18 dBFS. Every gated term prints its measurement beside its
// budget in the `AU3_NORMALIZE` / `AU4_DUCK` house style, and no tolerance is
// conditioned on the operating system.
// ===========================================================================

/// AU5 §3.11(e): the SNR gain `plan_dialogue_repair` must buy on fixture (a)
/// at its **default** `reduction_tenth_db` of 120, in hundredths of a dB.
///
/// Part A's own lane gates the same fixture at 600 hundredths with the
/// reduction set to 200; the planner's default asks for 12 dB rather than 20,
/// so the contract halves the budget and expects roughly 12 dB measured — a
/// margin of about 4x.
const PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS: i64 = 300;

/// AU5 §3.11(a): 3 s of 48 kHz stereo — `[0, 1 s)` noise only, `[1, 2.5 s)`
/// tone plus noise, `[2.5, 3 s)` noise only.
///
/// The noise is `pseudo_random_amplitude(144_000, 0.010)`, whose first
/// argument is a **sample** count, so this is 3 s of MONO samples at
/// `0.010/sqrt(3) = 5.774e-3` RMS = -44.77 dBFS; the 1 kHz tone is
/// `0.200/sqrt(2) = 0.14142` = -16.99 dBFS. The sum is interleaved to stereo
/// here, because the promoted helpers are mono and their signatures did not
/// change (AU5 §0 R45).
///
/// The two noise-only stretches are what make this fixture work end to end:
/// they are 27 dB under the tone, so the silence detector finds them and
/// rule 107's profile has something to learn from, and they are where the
/// 10th-percentile floor lives, so a real floor drop moves the measured SNR.
fn au5_repair_media() -> GeneratedMedia {
    let mut mono = kinewright_media::test_support::pseudo_random_amplitude(144_000, 0.010);
    for (index, sample) in kinewright_media::test_support::tone(1_000.0, 0.200, 48_000, 72_000)
        .into_iter()
        .enumerate()
    {
        mono[48_000 + index] += sample;
    }
    let stereo = mono
        .iter()
        .flat_map(|sample| [*sample, *sample])
        .collect::<Vec<_>>();
    GeneratedMedia::from_bytes(
        "au5-repair-noisy",
        "wav",
        &kinewright_media::test_support::wav_f32(&stereo, 48_000, 2),
    )
}

/// AU5 §7 B10's negative arm: **clean** material — 1 s of digital silence
/// followed by 2 s of a 440 Hz tone, 48 kHz stereo.
///
/// The digital silence is deliberate. It gives the silence detector a span to
/// find, so the planner reaches its measured loop rather than refusing at the
/// readiness gate; and a profile learned over it reads
/// `PROFILE_BAND_NEUTRAL_TENTH_DB` in all 31 bands, which AU5 §2.1 rule 5 / R4
/// define as unity gain. The denoiser is then an exact identity, so the
/// measured SNR gain is zero and the planner has to refuse.
fn au5_clean_media() -> GeneratedMedia {
    let mut mono = vec![0.0_f32; 48_000];
    mono.extend(kinewright_media::test_support::tone(
        440.0, 0.300, 48_000, 96_000,
    ));
    let stereo = mono
        .iter()
        .flat_map(|sample| [*sample, *sample])
        .collect::<Vec<_>>();
    GeneratedMedia::from_bytes(
        "au5-repair-clean",
        "wav",
        &kinewright_media::test_support::wav_f32(&stereo, 48_000, 2),
    )
}

/// One audio-only asset on one audio track, whole.
fn au5_audio_document(asset: MediaAsset) -> Document {
    let duration = asset.duration;
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..duration,
                content: kinewright_core::ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
            }],
        }],
        media_pool: vec![asset],
        duration,
        ..Document::default()
    }
}

/// AU5 §5.6 rule 107 / B9: invoke a capability, tolerating **exactly one**
/// class of refusal — the asynchronous silence analysis that has not finished
/// yet — on `au4_plan_audio_ducking_converges_through_the_real_engine`'s own
/// template (this file's AU4 lane).
async fn au5_invoke_when_silence_is_ready(
    client: &RunningService<RoleClient, ()>,
    name: &str,
    arguments: serde_json::Value,
) -> CallToolResult {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let result = invoke_capability(client, name, arguments.clone()).await;
        if result.is_error == Some(false) {
            return result;
        }
        let text = result.content[0].as_text().unwrap().text.clone();
        if !text.contains("silence analysis is not ready") {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "silence analysis did not finish: {text}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The master SNR `get_audio_repair` measures over the whole timeline, in
/// hundredths of a dB.
async fn au5_master_snr(client: &RunningService<RoleClient, ()>) -> i64 {
    let measured = invoke_capability(client, "get_audio_repair", json!({})).await;
    let body = measured.structured_content.as_ref().unwrap();
    assert_eq!(measured.is_error, Some(false), "{body}");
    body["report"]["snr_db_hundredths"]
        .as_i64()
        .unwrap_or_else(|| panic!("the fixture must measure a percentile SNR: {body}"))
}

/// AU5 §7 B10 — **the measured refusal, which is the deliverable.**
///
/// `plan_dialogue_repair` runs on §3.11(a)'s fixture through the real engine,
/// commits, and the SNR is re-measured through `get_audio_repair` — the public
/// inspector, not the planner's own number — against
/// `PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS` with its margin printed. The
/// negative arm then runs the same planner on clean material and asserts it
/// **refuses**, names both numbers, and prepares no plan.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au5_plan_dialogue_repair_moves_the_snr_through_the_real_engine() {
    let noisy = au5_repair_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(noisy.path()).unwrap();
    assert_eq!(asset.kind, MediaKind::Audio);
    assert_eq!(asset.duration, TimeCode(90), "3 s at the 30 fps audio grid");
    let core = Core::spawn(au5_audio_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media.clone()).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let before = au5_master_snr(&client).await;
    let planned =
        au5_invoke_when_silence_is_ready(&client, "plan_dialogue_repair", json!({"tracks": [1]}))
            .await;
    let body = planned.structured_content.as_ref().unwrap();
    assert_eq!(planned.is_error, Some(false), "{body}");
    assert_eq!(body["reused_existing_bus"], false, "{body}");
    assert_eq!(body["chain_lookahead_milliseconds"], 15, "{body}");
    assert_eq!(
        body["repair_prefix"],
        json!(["audio_denoise", "audio_hum_removal", "audio_declick"]),
        "{body}"
    );
    // Rule 107: the profile is learned over the LONGEST silence span, which on
    // this fixture is the leading second of noise-only material.
    let learn = &body["learn_range"];
    assert_eq!(learn["track"], 1, "{body}");
    assert!(
        learn["end_frame"].as_i64().unwrap() - learn["start_frame"].as_i64().unwrap() >= 15,
        "the learn range must hold a whole 22,528 sample-frame profile window: {body}"
    );

    let revision = body["timeline_revision"].as_u64().unwrap();
    let plan_id = body["prepared_edit_plan"]["plan_id"].clone();
    let committed = client
        .call_tool(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({"plan_id": plan_id, "expected_revision": revision})
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

    // What landed: one bus carrying the three repair nodes at its head.
    let document = query_document(&core);
    assert_eq!(
        document.audio_mix.buses.len(),
        1,
        "{:?}",
        document.audio_mix
    );
    let bus = &document.audio_mix.buses[0];
    assert_eq!(
        bus.effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>(),
        ["audio_denoise", "audio_hum_removal", "audio_declick"]
    );
    assert_eq!(
        kinewright_core::chain_lookahead_milliseconds(&bus.effects),
        15
    );

    // The engine's own answer, re-measured through the public inspector.
    let after = au5_master_snr(&client).await;
    let gain = after - before;
    let margin = f64::from(i32::try_from(gain).unwrap())
        / f64::from(i32::try_from(PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS).unwrap());
    println!(
        "AU5_PLAN_REPAIR_SNR before_hundredths={before} after_hundredths={after} \
         gain_hundredths={gain} budget={PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS} margin={margin:.2}"
    );
    assert!(
        gain >= PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS,
        "the committed repair must really move the measured SNR: before={before} after={after}"
    );
    assert!(
        margin >= 2.0,
        "AU5 §3.12: every gated term keeps a 2x margin; measured {margin:.2}x"
    );
    // The planner's own measurement agrees with the re-measurement, which is
    // what makes rule 106's `measured` worth publishing at all.
    assert!(
        (body["measured"]["snr_gain_db_hundredths"].as_i64().unwrap() - gain).abs()
            <= PLAN_REPAIR_SNR_GAIN_BUDGET_HUNDREDTHS,
        "plan {} against re-measured {gain}",
        body["measured"]
    );
    client.cancel().await.unwrap();
    server.shutdown();

    // ---- The negative arm: clean material refuses, naming both numbers. ----
    let clean = au5_clean_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(clean.path()).unwrap();
    let core = Core::spawn(au5_audio_document(asset)).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let refused =
        au5_invoke_when_silence_is_ready(&client, "plan_dialogue_repair", json!({"tracks": [1]}))
            .await;
    let text = refused.content[0].as_text().unwrap().text.clone();
    assert_eq!(refused.is_error, Some(true), "{text}");
    assert!(
        refused.structured_content.is_none(),
        "a refusal prepares no plan: {refused:?}"
    );
    assert!(
        text.contains("minimum_snr_gain_db_hundredths 100"),
        "the required number: {text}"
    );
    assert!(
        text.contains("signal-to-noise gain of"),
        "the measured number: {text}"
    );
    assert!(
        text.contains("HIGHER floor and therefore a LOWER gain"),
        "and which way the percentile floor biases it (rule 21): {text}"
    );
    // Nothing was prepared, so nothing can be committed: the timeline is
    // exactly where it started.
    assert_eq!(cc7_revision(&client).await, 0);
    assert!(query_document(&core).audio_mix.buses.is_empty());
    println!("AU5_PLAN_REPAIR_REFUSAL {text}");
    client.cancel().await.unwrap();
    server.shutdown();
}

/// Commit one prepared plan out of a planner's structured content.
async fn au5_commit_prepared(client: &RunningService<RoleClient, ()>, body: &serde_json::Value) {
    let revision = body["timeline_revision"].as_u64().unwrap();
    let plan_id = body["prepared_edit_plan"]["plan_id"].clone();
    let committed = client
        .call_tool(
            CallToolRequestParams::new("commit_edit_plan").with_arguments(
                json!({"plan_id": plan_id, "expected_revision": revision})
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
}

/// AU5 §5.7 rule 111 / B11's R35 arm: give the one existing bus a **non-zero
/// fader and a gain curve**, through the ordinary edit-plan path.
///
/// The neutral-fader fixture alone would not catch a reset, because
/// `gain_tenth_db` is `skip_serializing_if = "i32_is_zero"` and the reset
/// would be invisible in the golden too.
///
/// The values are deliberately **small** — 0.2 dB falling to 0.1 dB — for a
/// reason worth stating: in the normalize-first order the fader is armed
/// *after* the convergence loop has already run, so anything larger would show
/// up as a loudness error the planner never had a chance to absorb, and the
/// tolerance assertion below would be measuring this fixture's own arithmetic
/// instead of the planners'. What R35 needs is a value that is not zero, not a
/// value that is loud.
async fn au5_arm_the_bus_fader(client: &RunningService<RoleClient, ()>, core: &Core) {
    let document = query_document(core);
    let bus = document.audio_mix.buses.last().unwrap().clone();
    let revision = cc7_revision(client).await;
    let effects = serde_json::to_value(&bus.effects).unwrap();
    let prepared = prepare_plan(
        client,
        revision,
        json!([{
            "op": "upsert_audio_bus",
            "bus": {
                "id": bus.id.0,
                "name": bus.name,
                "tracks": [1],
                "gain_tenth_db": -2,
                "gain_curve": {
                    "keyframes": [
                        {"at": 0, "value": -2, "interpolation": "linear"},
                        {"at": 60, "value": -1, "interpolation": "linear"}
                    ]
                },
                "effects": effects
            }
        }]),
    )
    .await;
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
}

/// AU5 §5.7 / B11 — **S7, in either order.**
///
/// `plan_dialogue_repair` and `plan_audio_normalization` run on the same
/// document in both orders and land the same one bus: the repair prefix, then
/// the compressor/gain, then the true-peak limiter, declaring exactly
/// `CHAIN_LOOKAHEAD_MILLISECONDS` = 20 ms — 15 of repair plus 5 of limiter,
/// the whole budget and not a millisecond of slack. In each order the bus's
/// **non-zero fader and gain curve** are armed after the first planner commits
/// and asserted to survive the second (R35), and the committed loudness lands
/// inside AU3's own tolerance, so the four convergence iterations really did
/// converge on the *repaired* loudness rather than on the raw one.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au5_the_repair_and_normalization_planners_agree_in_either_order() {
    let target = -1_600_i64;
    let tolerance = 100_i64;
    let normalization = json!({
        "track_ids": [1],
        "target_lufs_hundredths": target,
        "maximum_sample_peak_dbfs_hundredths": -100,
        "tolerance_hundredths": tolerance
    });
    for repair_first in [true, false] {
        let generated = au5_repair_media();
        let media = Arc::new(FfmpegMediaEngine::new().unwrap());
        let asset = media.probe(generated.path()).unwrap();
        let core = Core::spawn(au5_audio_document(asset)).unwrap();
        let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
        let client =
            ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
                .await
                .unwrap();
        let label = if repair_first {
            "repair-then-normalize"
        } else {
            "normalize-then-repair"
        };

        let first = if repair_first {
            au5_invoke_when_silence_is_ready(
                &client,
                "plan_dialogue_repair",
                json!({"tracks": [1]}),
            )
            .await
        } else {
            invoke_capability(&client, "plan_audio_normalization", normalization.clone()).await
        };
        let body = first.structured_content.as_ref().unwrap().clone();
        assert_eq!(first.is_error, Some(false), "{label}: {body}");
        au5_commit_prepared(&client, &body).await;
        au5_arm_the_bus_fader(&client, &core).await;

        let second = if repair_first {
            invoke_capability(&client, "plan_audio_normalization", normalization.clone()).await
        } else {
            au5_invoke_when_silence_is_ready(
                &client,
                "plan_dialogue_repair",
                json!({"tracks": [1]}),
            )
            .await
        };
        let body = second.structured_content.as_ref().unwrap().clone();
        assert_eq!(
            second.is_error,
            Some(false),
            "{label}: the second planner must EXTEND the first planner's bus rather than refuse \
             the intersection: {body}"
        );
        au5_commit_prepared(&client, &body).await;

        // One bus, whatever the order, and exactly one.
        let document = query_document(&core);
        assert_eq!(
            document.audio_mix.buses.len(),
            1,
            "{label}: {:?}",
            document.audio_mix
        );
        let bus = &document.audio_mix.buses[0];
        let names = bus
            .effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            &names[..3],
            ["audio_denoise", "audio_hum_removal", "audio_declick"],
            "{label}: the repair prefix is at the HEAD in both orders: {names:?}"
        );
        assert_eq!(
            names.last(),
            Some(&"audio_true_peak_limiter"),
            "{label}: the chain still ends in the inter-sample-aware limiter: {names:?}"
        );
        assert!(
            names[3..names.len() - 1]
                .iter()
                .all(|name| matches!(*name, "audio_compressor" | "audio_gain")),
            "{label}: only the delivery processing sits between them: {names:?}"
        );
        assert!(
            !names.contains(&"audio_limiter"),
            "{label}: never the legacy sample-peak clamp"
        );
        assert_eq!(
            kinewright_core::chain_lookahead_milliseconds(&bus.effects),
            kinewright_core::CHAIN_LOOKAHEAD_MILLISECONDS,
            "{label}: 15 of repair plus 5 of limiter is exactly the budget: {names:?}"
        );
        // R35: every `AudioBus` field except `effects` rides across.
        assert_eq!(bus.gain_tenth_db, -2, "{label}: the fader is not reset");
        let curve = bus
            .gain_curve
            .as_ref()
            .unwrap_or_else(|| panic!("{label}: the gain curve is not dropped"));
        assert_eq!(curve.keyframes.len(), 2, "{label}: {curve:?}");
        assert_eq!(curve.keyframes[0].value, -2, "{label}");
        assert_eq!(curve.keyframes[1].value, -1, "{label}");
        assert_eq!(bus.tracks, vec![TrackId(1)], "{label}");
        // Effect ids are unique, in both orders: the delivery processing never
        // reuses a repair node's id.
        let mut ids = bus
            .effects
            .iter()
            .map(|effect| effect.id.0)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), bus.effects.len(), "{label}: {:?}", bus.effects);

        // The loudness the convergence landed on is the REPAIRED loudness,
        // measured through the public inspector on the committed document.
        let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
        let levels = levels.structured_content.as_ref().unwrap();
        let measured = levels["report"]["master"]["integrated_lufs_hundredths"]
            .as_i64()
            .unwrap_or_else(|| panic!("{label}: the committed mix must measure: {levels}"));
        println!(
            "AU5_REPAIR_NORMALIZE order={label} measured_lufs_hundredths={measured} \
             target={target} tolerance={tolerance} error={}",
            (measured - target).abs()
        );
        assert!(
            (measured - target).abs() <= tolerance,
            "{label}: the committed loudness must sit inside AU3's tolerance: {measured}"
        );
        client.cancel().await.unwrap();
        server.shutdown();
    }
}

/// AU5 §5.8 rule 115: approve the one confirmation `capture_room_tone` raises,
/// asserting the sentence the operator is actually shown.
async fn au5_approve_capture(broker: kinewright_agent::ConfirmationBroker, approve: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(request) = broker.pending_requests().into_iter().next() {
            assert_eq!(request.tool_name, "capture_room_tone");
            assert!(
                request.description.contains("room-tone store"),
                "the operator is told where the bytes go: {}",
                request.description
            );
            assert!(
                request.description.contains("source frames 0..30"),
                "and which range is captured: {}",
                request.description
            );
            if approve {
                assert!(broker.approve(request.id));
            } else {
                assert!(broker.reject(request.id, "not this take"));
            }
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "capture_room_tone must publish a confirmation before it writes a byte"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// AU5 §7 B2 and B7 — capture, fill, and both idempotences, through the real
/// store and the real decoder.
///
/// The timeline is §5.4's shape: clip A over source `0..30`, a **30-frame
/// interior gap**, clip B over source `0..30`. One second of the fixture's
/// leading noise is captured into the project's room-tone store, one
/// `plan_room_tone_fill` fills the gap with a single butt-joined tile, and
/// both halves are then re-run to prove they are no-ops: a second identical
/// capture returns the same `asset_id` and emits **no** operation, and a
/// second fill proposes nothing, because a filled gap is not a gap.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au5_capture_room_tone_and_fill_a_gap_through_the_real_store() {
    let generated = au5_repair_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    let clip = |id: u64, at: i64| Clip {
        id: ClipId(id),
        asset: asset.id,
        source_range: TimeCode::ZERO..TimeCode(30),
        content: kinewright_core::ClipContent::Media,
        timeline_start: TimeCode(at),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
    };
    let document = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip(1, 0), clip(2, 60)],
        }],
        media_pool: vec![asset.clone()],
        duration: TimeCode(90),
        ..Document::default()
    };
    let core = Core::spawn(document).unwrap();
    let project = kinewright_media::test_support::TempDirectory::new("au5-room-tone");
    let handle = Arc::new(std::sync::RwLock::new(Some(
        project.path("show.kinewright"),
    )));
    let server = McpServer::start_isolated_with_project_path(
        core.clone(),
        media.clone(),
        media,
        Arc::clone(&handle),
    )
    .unwrap();
    let confirmations = server.confirmations();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    // A refused confirmation writes nothing at all, which is the whole reason
    // the confirmation is raised before the decode rather than after it.
    let capture = json!({
        "expected_revision": 0,
        "asset_id": asset.id.0,
        "source_start_frame": 0,
        "source_end_frame": 30
    });
    let (refused, ()) = tokio::join!(
        invoke_capability(&client, "capture_room_tone", capture.clone()),
        au5_approve_capture(confirmations.clone(), false),
    );
    assert_eq!(refused.is_error, Some(true));
    let body = refused.structured_content.as_ref().unwrap();
    assert_eq!(body["code"], "capture_refused", "{body}");
    assert_eq!(body["details"]["store_file_written"], false, "{body}");
    assert_eq!(body["details"]["document_changed"], false, "{body}");
    assert!(
        !project.path("show.kinewright-assets").exists(),
        "a refused capture leaves no store directory behind"
    );

    // The approved capture: one 48 kHz stereo second, written under its own
    // digest, probed, and registered as ONE AddAsset carrying an overridden
    // name.
    let (captured, ()) = tokio::join!(
        invoke_capability(&client, "capture_room_tone", capture.clone()),
        au5_approve_capture(confirmations.clone(), true),
    );
    let body = captured.structured_content.as_ref().unwrap();
    assert_eq!(captured.is_error, Some(false), "{body}");
    assert_eq!(body["applied"], true, "{body}");
    assert_eq!(body["reused_existing_asset"], false, "{body}");
    let tone = &body["room_tone_asset"];
    assert_eq!(tone["milliseconds"], 1_000, "{tone}");
    assert_eq!(tone["sample_frames"], 48_000, "{tone}");
    assert_eq!(
        tone["frames"], 30,
        "1 s is a whole 30 fps asset frame count: {tone}"
    );
    assert_eq!(tone["fps"], "30/1", "an audio-only asset probes at 30 fps");
    // R46 / rule 115: `probe_path` names a store file after its own digest, so
    // the capture OVERRIDES that name rather than carrying it.
    let name = tone["name"].as_str().unwrap();
    assert!(name.starts_with("Room tone — "), "{tone}");
    assert!(
        !name.contains(tone["sha256"].as_str().unwrap()),
        "the registered name must not be the digest `probe_path` read off the store file: {tone}"
    );
    assert!(
        name.ends_with(&document_source_name(&core, asset.id)),
        "it names the SOURCE the tone was captured from: {tone}"
    );
    let store_path = PathBuf::from(tone["store_path"].as_str().unwrap());
    assert!(store_path.is_file(), "{store_path:?}");
    assert_eq!(
        store_path.parent().unwrap().file_name().unwrap(),
        "room-tone"
    );
    assert_eq!(
        store_path.file_stem().unwrap().to_str().unwrap(),
        tone["sha256"].as_str().unwrap(),
        "the store is content-addressed"
    );

    // Exactly one operation landed, and it was an AddAsset.
    let document = query_document(&core);
    assert_eq!(document.media_pool.len(), 2, "{:?}", document.media_pool);
    let room_tone = document.media_pool.last().unwrap();
    assert_eq!(room_tone.duration, TimeCode(30));
    assert_eq!(room_tone.name, name);
    let after_capture = cc7_revision(&client).await;
    assert_eq!(after_capture, 1, "one AddAsset, one revision");
    // AU5 §0 R112: a captured room tone earns NO background analysis. Queueing
    // a transcription on a second of noise means downloading the Whisper model
    // to read a file with no speech in it, and the abort that used to end this
    // test binary was that download still in flight at process exit.
    let jobs = invoke_capability(
        &client,
        "get_analysis_status",
        json!({"asset_id": room_tone.id.0}),
    )
    .await;
    let jobs = jobs.structured_content.as_ref().unwrap();
    // F11: assert the SHAPE, not the absence of two words. Silence detection on
    // one second of noise can finish between the capture and this line, and a
    // `"ready"` phase would sail through a substring search — so a regression
    // that re-enabled `request_asset_analysis` would be caught only by
    // whichever job happened to still be pending. Every phase is checked, and
    // the only two a never-requested asset may report are these.
    // Pass-2 finding 5: assert the list is non-empty first, so a shape change
    // that answered `[]` for an un-requested asset would turn this pin
    // green-and-vacuous rather than red.
    let jobs = jobs["jobs"].as_array().unwrap();
    assert!(
        !jobs.is_empty(),
        "the status surface must report every kind"
    );
    for job in jobs {
        assert!(
            matches!(job["phase"].as_str(), Some("not_requested" | "unavailable")),
            "the capture must queue no analysis at all: {job}"
        );
    }

    // Rule 116: a second identical capture is a NO-OP — the same asset id, no
    // operation, and the revision does not move. It still asks first, because
    // the caller cannot know it is a repeat.
    let (again, ()) = tokio::join!(
        invoke_capability(
            &client,
            "capture_room_tone",
            json!({
                "expected_revision": after_capture,
                "asset_id": asset.id.0,
                "source_start_frame": 0,
                "source_end_frame": 30
            })
        ),
        au5_approve_capture(confirmations.clone(), true),
    );
    let body = again.structured_content.as_ref().unwrap();
    assert_eq!(again.is_error, Some(false), "{body}");
    assert_eq!(body["reused_existing_asset"], true, "{body}");
    assert_eq!(body["applied"], false, "{body}");
    // F2 / rule 114: the reused branch publishes the SAME `room_tone_asset`
    // shape the applied branch does, so a caller reads one key either way —
    // `import_lut_asset`'s own rule, which is what "on its exact shape" means.
    assert_eq!(
        body["room_tone_asset"]["asset_id"], room_tone.id.0,
        "{body}"
    );
    assert_eq!(body["room_tone_asset"]["name"], room_tone.name, "{body}");
    assert_eq!(body["room_tone_asset"]["sha256"], tone["sha256"], "{body}");
    assert_eq!(
        body["room_tone_asset"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        tone.as_object().unwrap().keys().collect::<Vec<_>>(),
        "the two branches publish the same key set"
    );
    assert_eq!(cc7_revision(&client).await, after_capture);

    // §5.5: the fill. One 30-frame gap, one tile of the 30-frame sample, and
    // `asset_id` defaulted to the sole registered room-tone asset.
    let planned = invoke_capability(&client, "plan_room_tone_fill", json!({"track": 1})).await;
    let body = planned.structured_content.as_ref().unwrap();
    assert_eq!(planned.is_error, Some(false), "{body}");
    assert_eq!(body["asset_id"], room_tone.id.0, "{body}");
    assert_eq!(
        body["gaps"],
        json!([{"start": 30, "end": 60, "tiles": 1, "skipped_reason": null}]),
        "{body}"
    );
    au5_commit_prepared(&client, body).await;

    // What landed: one ordinary media clip of the room-tone asset, butt-joined
    // at the gap, at `speed_percent = 100` and with no fade of any kind.
    let document = query_document(&core);
    let filled = document.tracks[0]
        .clips
        .iter()
        .find(|clip| clip.asset == room_tone.id)
        .expect("the fill is an ordinary media clip");
    assert_eq!(filled.timeline_start, TimeCode(30));
    assert_eq!(filled.source_range, TimeCode::ZERO..TimeCode(30));
    assert_eq!(filled.speed_percent, 100, "rule 44");
    assert_eq!(filled.audio_fade_in_frames, TimeCode::ZERO, "rule 95");
    assert_eq!(filled.audio_fade_out_frames, TimeCode::ZERO);
    assert!(filled.audio_gain_curve.is_none());
    // Rule 97's own assertion, taken from the committed document.
    assert_eq!(document.clip_duration(filled).unwrap(), TimeCode(30));
    // The gap is gone, which is the only observable "the hole is closed"
    // signal the product has (rule 96).
    assert_eq!(
        document.track_gaps(TrackId(1)),
        Some(Vec::new()),
        "{:?}",
        document.tracks[0].clips
    );

    // Rule 104: idempotent by construction. A filled gap is not a gap.
    let repeat = invoke_capability(&client, "plan_room_tone_fill", json!({"track": 1})).await;
    let body = repeat.structured_content.as_ref().unwrap();
    assert_eq!(repeat.is_error, Some(false), "{body}");
    assert_eq!(body["gaps"], json!([]), "{body}");
    assert_eq!(
        body["prepared_edit_plan"],
        serde_json::Value::Null,
        "{body}"
    );
    assert!(
        repeat.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("no leading or interior gap to fill"),
        "{repeat:?}"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

/// The pooled name of one asset, for AU5 §5.8's `"Room tone — {source}"` pin.
fn document_source_name(core: &Core, asset: AssetId) -> String {
    query_document(core)
        .media_pool
        .iter()
        .find(|candidate| candidate.id == asset)
        .expect("the source asset is pooled")
        .name
        .clone()
}

/// AU5 §5.3 rule 97 / §0 R96 / B6 — **the 25 fps arm, committed.**
///
/// A 25 fps project reading the 30 fps audio-only asset over a **7-frame gap**.
/// `map_frames(8, 30, 25) = round(40/6) = 7`, so source `0..8` is exact in
/// project *frames* and 640 sample frames short of what the gap demands,
/// because the mixer maps source samples to project samples one for one and
/// then stops. The planner must therefore take its tile from core's
/// **covering** range, which starts at a non-zero source frame; this lane
/// asserts the committed fill is exactly that range — the same shared helper
/// media's seam lane builds its reference from, so the two cannot disagree —
/// that it measures exactly the gap, that Core accepted it without a
/// `ClipOverlap`, and that **no residual gap is left behind**, which is the
/// only observable "the hole is closed" signal the product has.
#[tokio::test(flavor = "multi_thread")]
async fn au5_plan_room_tone_fill_commits_a_covering_tile_at_25_fps() {
    let generated = au5_repair_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    assert_eq!(asset.fps, Rational::new(30, 1).unwrap());
    let clip = |id: u64, at: i64| Clip {
        id: ClipId(id),
        asset: asset.id,
        source_range: TimeCode::ZERO..TimeCode(30),
        content: kinewright_core::ClipContent::Media,
        timeline_start: TimeCode(at),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
    };
    let document = Document {
        fps: Rational::new(25, 1).unwrap(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip(1, 0), clip(2, 32)],
        }],
        media_pool: vec![asset.clone()],
        duration: TimeCode(57),
        ..Document::default()
    };
    assert_eq!(
        document.track_gaps(TrackId(1)),
        Some(vec![TimeCode(25)..TimeCode(32)]),
        "30 source frames map to 25 project frames at 25 fps, so the gap is 25..32"
    );
    let expected = kinewright_core::covering_source_range_for_project_duration(
        TimeCode(7),
        asset.fps,
        document.fps,
        asset.duration,
        48_000,
    )
    .expect("a 7-frame gap has a covering range at 30 -> 25");
    assert_ne!(
        expected.start,
        TimeCode::ZERO,
        "R96: source 0..8 is exact in frames and short in samples, so phase 0 is not the answer"
    );

    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let planned = invoke_capability(
        &client,
        "plan_room_tone_fill",
        json!({"track": 1, "asset_id": asset.id.0}),
    )
    .await;
    let body = planned.structured_content.as_ref().unwrap();
    assert_eq!(planned.is_error, Some(false), "{body}");
    assert_eq!(
        body["gaps"],
        json!([{"start": 25, "end": 32, "tiles": 1, "skipped_reason": null}]),
        "{body}"
    );
    au5_commit_prepared(&client, body).await;

    let document = query_document(&core);
    let filled = document.tracks[0]
        .clips
        .iter()
        .find(|clip| clip.timeline_start == TimeCode(25))
        .expect("the fill lands at the gap start");
    assert_eq!(
        filled.source_range, expected,
        "the committed tile is core's covering range, phase and all"
    );
    assert_eq!(document.clip_duration(filled).unwrap(), TimeCode(7));
    assert_eq!(
        document.track_gaps(TrackId(1)),
        Some(Vec::new()),
        "no residual gap: {:?}",
        document.tracks[0].clips
    );
    println!(
        "AU5_ROOM_TONE_FILL_25FPS gap=7 source={}..{} covered_frames={}",
        expected.start.0,
        expected.end.0,
        expected.end.0 - expected.start.0
    );
    client.cancel().await.unwrap();
    server.shutdown();
}

/// Twenty seconds of 48 kHz stereo room tone — the same noise §3.11(a) uses,
/// long enough that an NTSC covering phase (around source frame 500) exists
/// inside it.
fn au5_long_room_tone_media() -> GeneratedMedia {
    au5_room_tone_media_of(960_000)
}

/// Forty seconds — **1 200 source frames at 30 fps**, the length pass-2
/// finding 1 showed the old four-frame shortfall bound could not tile at all.
///
/// At 30 -> 29.97 the tile that works is the map period's `0..1001`, so a
/// 1 200-frame asset has to step down 199 project frames to reach it. Against a
/// four-frame allowance it found no tile and skipped **every** gap on the
/// track; core's `longest_coverable_project_tile` finds it, and the lane below
/// is what says so.
fn au5_ntsc_long_room_tone_media() -> GeneratedMedia {
    au5_room_tone_media_of(1_920_000)
}

fn au5_room_tone_media_of(mono_samples: usize) -> GeneratedMedia {
    let mono = kinewright_media::test_support::pseudo_random_amplitude(mono_samples, 0.010);
    let stereo = mono
        .iter()
        .flat_map(|sample| [*sample, *sample])
        .collect::<Vec<_>>();
    GeneratedMedia::from_bytes(
        &format!("au5-room-tone-{mono_samples}"),
        "wav",
        &kinewright_media::test_support::wav_f32(&stereo, 48_000, 2),
    )
}

/// AU5 §5.3 / §0 R96, R119 — **the NTSC lane.**
///
/// A 30000/1001 project reading the 30 fps audio-only asset. This is the rate
/// pair the core review found the planner silently failing on: covering needs a
/// source start hundreds of frames in (at `D = 1` the phases that work are
/// around 499..501), and the helper's phase window was too narrow to reach
/// them, so **every** gap on an NTSC timeline was skipped. The agent half made
/// that invisible by `.ok()`ing the helper's error into one per-gap sentence
/// about exact source ranges; the reason now carries the error's own kind
/// (R119), and this lane asserts the fill really is proposed, really is core's
/// own covering range, and really closes the gap.
#[tokio::test(flavor = "multi_thread")]
async fn au5_plan_room_tone_fill_commits_a_covering_tile_at_29_97_fps() {
    // Twenty seconds, not three: at 30 -> 29.97 the covering phases for a
    // ten-frame span sit around source frame 500, so a three-second sample is
    // provably too short and core answers `NoCoveringSourceRange` — a real
    // refusal with a real reason, and the wrong fixture for this lane.
    let generated = au5_long_room_tone_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    assert_eq!(
        asset.duration,
        TimeCode(600),
        "20 s at the 30 fps audio grid"
    );
    let project_fps = Rational::new(30_000, 1_001).unwrap();
    let clip = |id: u64, at: i64| Clip {
        id: ClipId(id),
        asset: asset.id,
        source_range: TimeCode::ZERO..TimeCode(30),
        content: kinewright_core::ClipContent::Media,
        timeline_start: TimeCode(at),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
    };
    let document = Document {
        fps: project_fps,
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip(1, 0), clip(2, 40)],
        }],
        media_pool: vec![asset.clone()],
        duration: TimeCode(70),
        ..Document::default()
    };
    assert_eq!(
        document.track_gaps(TrackId(1)),
        Some(vec![TimeCode(30)..TimeCode(40)]),
        "30 source frames map to 30 project frames at 29.97, so the gap is 30..40"
    );
    let expected = kinewright_core::covering_source_range_for_project_duration(
        TimeCode(10),
        asset.fps,
        project_fps,
        asset.duration,
        48_000,
    )
    .expect("core's widened phase window reaches an NTSC covering range");

    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let planned = invoke_capability(
        &client,
        "plan_room_tone_fill",
        json!({"track": 1, "asset_id": asset.id.0}),
    )
    .await;
    let body = planned.structured_content.as_ref().unwrap();
    assert_eq!(planned.is_error, Some(false), "{body}");
    assert_eq!(
        body["gaps"],
        json!([{"start": 30, "end": 40, "tiles": 1, "skipped_reason": null}]),
        "an NTSC project must get a fill, not a per-gap reason: {body}"
    );
    au5_commit_prepared(&client, body).await;

    let document = query_document(&core);
    let filled = document.tracks[0]
        .clips
        .iter()
        .find(|clip| clip.timeline_start == TimeCode(30))
        .expect("the fill lands at the gap start");
    assert_eq!(filled.source_range, expected);
    assert_eq!(document.clip_duration(filled).unwrap(), TimeCode(10));
    assert_eq!(
        document.track_gaps(TrackId(1)),
        Some(Vec::new()),
        "no residual gap: {:?}",
        document.tracks[0].clips
    );
    println!(
        "AU5_ROOM_TONE_FILL_29_97FPS gap=10 source={}..{}",
        expected.start.0, expected.end.0
    );
    client.cancel().await.unwrap();
    server.shutdown();
}

/// AU5 §0 R119 / pass-2 finding 1 — **the length the four-frame bound could not
/// tile.**
///
/// A 1 200-source-frame (40 s) room tone in a 29.97 project. Its own mapped
/// length is 1 199 project frames, and the only span it can cover is the map
/// period's 1 000 — so the tiler has to step down **199** frames to find it.
/// The agent's former `ROOM_TONE_MAX_TILE_SHORTFALL_FRAMES = 4` gave up at
/// step 4 and therefore skipped *every* gap on the track, which is the exact
/// user-visible failure R96 was written to close, relocated from core's phase
/// window into the agent's own constant. Core's
/// `longest_coverable_project_tile` owns the search now and the constant is
/// gone; this lane is what proves it, end to end and committed.
#[tokio::test(flavor = "multi_thread")]
async fn au5_plan_room_tone_fill_tiles_a_1200_frame_asset_at_29_97_fps() {
    let generated = au5_ntsc_long_room_tone_media();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(generated.path()).unwrap();
    assert_eq!(
        asset.duration,
        TimeCode(1_200),
        "40 s at the 30 fps audio grid"
    );
    let project_fps = Rational::new(30_000, 1_001).unwrap();
    // The step-down this length really needs, taken from core rather than
    // restated: a bound of 4 could never have reached it.
    let (tile, source) = kinewright_core::longest_coverable_project_tile(
        asset.duration,
        asset.fps,
        project_fps,
        48_000,
        TimeCode(1_199),
    )
    .expect("core's search finds the map period's tile");
    let mapped = kinewright_core::map_source_range_to_project(
        TimeCode::ZERO..asset.duration,
        asset.fps,
        project_fps,
    )
    .unwrap();
    assert!(
        mapped.0 - tile.0 > 4,
        "this lane is only meaningful if the step-down exceeds the deleted bound: \
         mapped={mapped:?} tile={tile:?}"
    );
    println!(
        "AU5_ROOM_TONE_FILL_NTSC_1200 mapped={} tile={} step={} source={}..{}",
        mapped.0,
        tile.0,
        mapped.0 - tile.0,
        source.start.0,
        source.end.0
    );

    let clip = |id: u64, at: i64| Clip {
        id: ClipId(id),
        asset: asset.id,
        source_range: TimeCode::ZERO..TimeCode(30),
        content: kinewright_core::ClipContent::Media,
        timeline_start: TimeCode(at),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
    };
    let document = Document {
        fps: project_fps,
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![clip(1, 0), clip(2, 40)],
        }],
        media_pool: vec![asset.clone()],
        duration: TimeCode(70),
        ..Document::default()
    };
    assert_eq!(
        document.track_gaps(TrackId(1)),
        Some(vec![TimeCode(30)..TimeCode(40)])
    );

    let core = Core::spawn(document).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let planned = invoke_capability(
        &client,
        "plan_room_tone_fill",
        json!({"track": 1, "asset_id": asset.id.0}),
    )
    .await;
    let body = planned.structured_content.as_ref().unwrap();
    assert_eq!(planned.is_error, Some(false), "{body}");
    assert_eq!(
        body["gaps"],
        json!([{"start": 30, "end": 40, "tiles": 1, "skipped_reason": null}]),
        "a 40 s NTSC capture must fill, not skip every gap on the track: {body}"
    );
    au5_commit_prepared(&client, body).await;
    let document = query_document(&core);
    assert_eq!(
        document.track_gaps(TrackId(1)),
        Some(Vec::new()),
        "no residual gap: {:?}",
        document.tracks[0].clips
    );
    client.cancel().await.unwrap();
    server.shutdown();
}

// ===========================================================================
// AU6 §5 — the six scripted agent end-to-end tests.
// ===========================================================================

use kinewright_core::au6_scenarios::{
    AU6_A_BED_TRACK, AU6_A_DUCK_ATTACK_MS, AU6_A_DUCK_DEPTH_TENTH_DB, AU6_A_DUCK_HOLD_MS,
    AU6_A_DUCK_RELEASE_MS, AU6_A_VOICE_A_TRACK, AU6_A_VOICE_B_TRACK, AU6_B_VOICE_A_TRACK,
    AU6_B_VOICE_B_TRACK, AU6_C_DIALOGUE_TRACK, AU6_C_LEARN_SOURCE_RANGE, AU6_C_RIGHT_CLIP_ID,
    AU6_SOURCE_FPS, Au6Scenario, au6_c_gap_operations, au6_canonical_operations, au6_spec,
};
use kinewright_media::au6_sources::{au6_scenario_sources, au6_stamp_on_project_grid};

const AU6_DUCKING_ALREADY_RIDDEN_REFUSAL: &str =
    "already carries a gain curve; clear it or pass replace: true";
const AU6_CLIP_FADES_NOTHING_TO_PROPOSE: &str = "nothing to propose";
const AU6_ROOM_TONE_NO_ASSET_REFUSAL: &str =
    "the project has no registered room-tone asset; capture one with capture_room_tone first";
const AU6_REPAIR_ALREADY_REPAIRED_REFUSAL: &str = "already carries an AU5 repair prefix";

struct Au6AgentScene {
    _media: Vec<GeneratedMedia>,
    document: Document,
}

fn au6_agent_scene(scenario: Au6Scenario) -> Au6AgentScene {
    let spec = au6_spec(scenario);
    let generated = au6_scenario_sources(scenario);
    let fps = Rational::new(AU6_SOURCE_FPS, 1).expect("25 fps");
    let engine = FfmpegMediaEngine::new().expect("the AU6 agent engine starts");
    let media_pool = generated
        .iter()
        .zip(spec.tracks)
        .map(|(generated, track)| {
            let probed = engine
                .probe(generated.path())
                .unwrap_or_else(|error| panic!("probe {}: {error}", generated.path().display()));
            let mut stamped =
                au6_stamp_on_project_grid(probed, fps, TimeCode(i64::from(spec.asset_frames)));
            stamped.id = kinewright_core::AssetId(track.track.0);
            stamped
        })
        .collect::<Vec<_>>();
    let tracks = spec
        .tracks
        .iter()
        .map(|track| Track {
            id: track.track,
            kind: track.kind,
            sync_lock: track.sync_lock,
            clips: spec
                .clips
                .iter()
                .filter(|clip| clip.track == track.track)
                .map(|clip| Clip {
                    id: clip.clip,
                    asset: clip.asset,
                    source_range: clip.range(),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: clip.start,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                })
                .collect(),
        })
        .collect();
    let mut document = Document {
        tracks,
        media_pool,
        fps,
        resolution: (
            kinewright_core::au6_scenarios::AU6_SOURCE_WIDTH,
            kinewright_core::au6_scenarios::AU6_SOURCE_HEIGHT,
        ),
        duration: TimeCode(i64::from(spec.frames)),
        ..Document::default()
    };
    if scenario == Au6Scenario::Multicam {
        document
            .catalog
            .sync_groups
            .push(kinewright_core::au6_scenarios::au6_d_sync_group(
                kinewright_core::au6_scenarios::AU6_D_ANGLE_ASSETS,
            ));
    }
    document
        .validate()
        .unwrap_or_else(|error| panic!("{scenario:?}: {error}"));
    Au6AgentScene {
        _media: generated,
        document,
    }
}

fn au6_ops_json(operations: &[Operation]) -> serde_json::Value {
    serde_json::to_value(operations).expect("AU6 operations serialize")
}

fn au6_mix_and_bus_ops(scenario: Au6Scenario) -> Vec<Operation> {
    au6_canonical_operations(scenario)
        .into_iter()
        .filter(|operation| {
            matches!(
                operation,
                Operation::SetTrackMix { .. } | Operation::UpsertAudioBus { .. }
            )
        })
        .collect()
}

async fn au6_commit_ops(
    client: &RunningService<RoleClient, ()>,
    revision: u64,
    operations: &[Operation],
) -> u64 {
    let prepared = prepare_plan(client, revision, au6_ops_json(operations)).await;
    assert_eq!(
        prepared.is_error,
        Some(false),
        "{:?} {}",
        prepared.structured_content,
        au6_error_text(&prepared)
    );
    let committed = client
        .call_tool(commit_request(revision, &prepared))
        .await
        .unwrap();
    assert_eq!(
        committed.is_error,
        Some(false),
        "{:?} {}",
        committed.structured_content,
        au6_error_text(&committed)
    );
    revision + 1
}

/// `DeleteClip` in a prepared batch raises `apply_edit_plan`'s destructive
/// confirmation (`plan_confirmation_description`). (c)'s gap commit and (d)'s
/// angle cuts both delete clips; the approval loop has to run beside the
/// commit, AU5/CC7's shape.
async fn au6_commit_ops_approved(
    client: &RunningService<RoleClient, ()>,
    confirmations: kinewright_agent::ConfirmationBroker,
    revision: u64,
    operations: &[Operation],
) -> u64 {
    let approvals = cc7_approve_confirmations(confirmations, "apply_edit_plan");
    let revision = au6_commit_ops(client, revision, operations).await;
    approvals.assert_approved_and_stop("apply_edit_plan");
    revision
}

fn au6_error_text(result: &CallToolResult) -> String {
    result.content[0].as_text().unwrap().text.clone()
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au6_a1_the_interview_ducks_the_bed_and_matches_the_voices() {
    let scene = au6_agent_scene(Au6Scenario::Interview);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let core = Core::spawn(scene.document.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(
        levels.is_error,
        Some(false),
        "{:?}",
        levels.structured_content
    );
    if let Some(body) = levels.structured_content.as_ref() {
        assert_ne!(body["applied"], true, "{body}");
    }

    let canonical = au6_canonical_operations(Au6Scenario::Interview);
    let _revision = au6_commit_ops(&client, 0, &au6_mix_and_bus_ops(Au6Scenario::Interview)).await;

    let planned = au5_invoke_when_silence_is_ready(
        &client,
        "plan_audio_ducking",
        json!({
            "music_track": AU6_A_BED_TRACK.0,
            "dialogue_tracks": [AU6_A_VOICE_A_TRACK.0, AU6_A_VOICE_B_TRACK.0],
            "depth_tenth_db": AU6_A_DUCK_DEPTH_TENTH_DB,
            "attack_milliseconds": AU6_A_DUCK_ATTACK_MS,
            "hold_milliseconds": AU6_A_DUCK_HOLD_MS,
            "release_milliseconds": AU6_A_DUCK_RELEASE_MS,
        }),
    )
    .await;
    let body = planned.structured_content.as_ref().unwrap();
    assert_eq!(planned.is_error, Some(false), "{body}");
    au5_commit_prepared(&client, body).await;

    let after = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(after.is_error, Some(false));
    let qc = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"expected_revision": cc7_revision(&client).await}),
    )
    .await;
    assert_eq!(qc.is_error, Some(false), "{:?}", qc.structured_content);
    let revision = cc7_revision(&client).await;
    cc7_assert_stale_revision(
        &client,
        "get_audio_qc",
        json!({"expected_revision": revision + 3}),
        revision,
        revision + 3,
    )
    .await;

    let refused = au5_invoke_when_silence_is_ready(
        &client,
        "plan_audio_ducking",
        json!({
            "music_track": AU6_A_BED_TRACK.0,
            "dialogue_tracks": [AU6_A_VOICE_A_TRACK.0, AU6_A_VOICE_B_TRACK.0],
        }),
    )
    .await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        au6_error_text(&refused).contains(AU6_DUCKING_ALREADY_RIDDEN_REFUSAL),
        "{}",
        au6_error_text(&refused)
    );

    let mut expected = scene.document.clone();
    apply_batch(&mut expected, &canonical).expect("canonical (a)");
    assert_eq!(query_document(&core), expected);
    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au6_a2_the_podcast_chain_matches_the_voices_and_tames_the_ride() {
    let scene = au6_agent_scene(Au6Scenario::Podcast);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let core = Core::spawn(scene.document.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let before = query_document(&core);
    let schema = client
        .call_tool(
            CallToolRequestParams::new("invoke_capability").with_arguments(
                json!({
                    "name": "plan_audio_normalization",
                    "arguments": {"profile": "source_master"}
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await;
    let schema_text = match &schema {
        Ok(result) => au6_error_text(result),
        Err(error) => error.to_string(),
    };
    assert!(
        schema_text.contains("track_ids") || schema_text.contains("missing field"),
        "{schema_text}"
    );
    if let Ok(result) = &schema {
        assert_eq!(result.is_error, Some(true));
    }
    assert_eq!(query_document(&core), before);

    let _mix_revision =
        au6_commit_ops(&client, 0, &au6_mix_and_bus_ops(Au6Scenario::Podcast)).await;
    let after_mix = query_document(&core);

    let ebu = invoke_capability(
        &client,
        "plan_audio_normalization",
        json!({
            "track_ids": [AU6_B_VOICE_A_TRACK.0, AU6_B_VOICE_B_TRACK.0],
            "target_lufs_hundredths": -2300
        }),
    )
    .await;
    assert_eq!(ebu.is_error, Some(false), "{:?}", ebu.structured_content);
    if let Some(body) = ebu.structured_content.as_ref() {
        assert_eq!(body["applied"], false);
    }
    let streaming = invoke_capability(
        &client,
        "plan_audio_normalization",
        json!({
            "track_ids": [AU6_B_VOICE_A_TRACK.0, AU6_B_VOICE_B_TRACK.0],
            "target_lufs_hundredths": -1400
        }),
    )
    .await;
    assert_eq!(streaming.is_error, Some(false));
    assert_eq!(query_document(&core), after_mix);

    let fades = au5_invoke_when_silence_is_ready(
        &client,
        "plan_clip_fades",
        json!({"tracks": [AU6_B_VOICE_A_TRACK.0, AU6_B_VOICE_B_TRACK.0]}),
    )
    .await;
    let fade_body = fades.structured_content.as_ref().unwrap();
    assert_eq!(fades.is_error, Some(false), "{fade_body}");
    au5_commit_prepared(&client, fade_body).await;
    let revision = cc7_revision(&client).await;
    let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(levels.is_error, Some(false));
    let qc = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"expected_revision": revision}),
    )
    .await;
    assert_eq!(qc.is_error, Some(false), "{:?}", qc.structured_content);
    cc7_assert_stale_revision(
        &client,
        "get_audio_qc",
        json!({"expected_revision": revision + 5}),
        revision,
        revision + 5,
    )
    .await;
    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn au6_a3_the_location_dialogue_is_repaired_and_its_gap_filled() {
    let scene = au6_agent_scene(Au6Scenario::LocationDialogue);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let core = Core::spawn(scene.document.clone()).unwrap();
    let project = kinewright_media::test_support::TempDirectory::new("au6-a3");
    let handle = Arc::new(std::sync::RwLock::new(Some(
        project.path("show.kinewright"),
    )));
    let server = McpServer::start_isolated_with_exporter_and_project_path(
        core.clone(),
        media.clone(),
        media.clone(),
        media.clone(),
        Arc::clone(&handle),
    )
    .unwrap();
    let confirmations = server.confirmations();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let no_asset = invoke_capability(
        &client,
        "plan_room_tone_fill",
        json!({"track": AU6_C_DIALOGUE_TRACK.0}),
    )
    .await;
    assert_eq!(no_asset.is_error, Some(true));
    assert!(
        au6_error_text(&no_asset).contains(AU6_ROOM_TONE_NO_ASSET_REFUSAL),
        "{}",
        au6_error_text(&no_asset)
    );

    let revision = au6_commit_ops_approved(
        &client,
        confirmations.clone(),
        0,
        &au6_c_gap_operations(AU6_C_RIGHT_CLIP_ID),
    )
    .await;

    let capture = json!({
        "expected_revision": revision,
        "asset_id": 1,
        "source_start_frame": AU6_C_LEARN_SOURCE_RANGE.start.0,
        "source_end_frame": AU6_C_LEARN_SOURCE_RANGE.end.0
    });
    let approvals = cc7_approve_confirmations(confirmations.clone(), "capture_room_tone");
    let captured = au5_invoke_when_silence_is_ready(&client, "capture_room_tone", capture).await;
    approvals.assert_approved_and_stop("capture_room_tone");
    assert_eq!(
        captured.is_error,
        Some(false),
        "{:?}",
        captured.structured_content
    );
    assert_eq!(cc7_revision(&client).await, revision + 1);

    let fill = au5_invoke_when_silence_is_ready(
        &client,
        "plan_room_tone_fill",
        json!({"track": AU6_C_DIALOGUE_TRACK.0}),
    )
    .await;
    let fill_body = fill.structured_content.as_ref().unwrap();
    assert_eq!(fill.is_error, Some(false), "{fill_body}");
    au5_commit_prepared(&client, fill_body).await;

    let before = invoke_capability(
        &client,
        "get_audio_repair",
        json!({"expected_revision": cc7_revision(&client).await}),
    )
    .await;
    assert_eq!(
        before.is_error,
        Some(false),
        "{:?}",
        before.structured_content
    );

    let planned = au5_invoke_when_silence_is_ready(
        &client,
        "plan_dialogue_repair",
        json!({"tracks": [AU6_C_DIALOGUE_TRACK.0]}),
    )
    .await;
    let repair_body = planned.structured_content.as_ref().unwrap();
    assert_eq!(planned.is_error, Some(false), "{repair_body}");
    au5_commit_prepared(&client, repair_body).await;

    let after = invoke_capability(
        &client,
        "get_audio_repair",
        json!({"expected_revision": cc7_revision(&client).await}),
    )
    .await;
    assert_eq!(
        after.is_error,
        Some(false),
        "{:?}",
        after.structured_content
    );

    let revision = cc7_revision(&client).await;
    cc7_assert_stale_revision(
        &client,
        "get_audio_repair",
        json!({"expected_revision": revision + 7}),
        revision,
        revision + 7,
    )
    .await;

    let already = au5_invoke_when_silence_is_ready(
        &client,
        "plan_dialogue_repair",
        json!({"tracks": [AU6_C_DIALOGUE_TRACK.0]}),
    )
    .await;
    assert_eq!(already.is_error, Some(true));
    assert!(
        au6_error_text(&already).contains(AU6_REPAIR_ALREADY_REPAIRED_REFUSAL),
        "{}",
        au6_error_text(&already)
    );

    let conflict = invoke_capability(
        &client,
        "capture_room_tone",
        json!({
            "expected_revision": 999,
            "asset_id": 1,
            "source_start_frame": AU6_C_LEARN_SOURCE_RANGE.start.0,
            "source_end_frame": AU6_C_LEARN_SOURCE_RANGE.end.0
        }),
    )
    .await;
    assert_eq!(conflict.is_error, Some(true));
    assert_eq!(
        conflict.structured_content.as_ref().unwrap()["code"],
        "revision_conflict"
    );

    let unknown = invoke_capability(
        &client,
        "capture_room_tone",
        json!({
            "expected_revision": cc7_revision(&client).await,
            "asset_id": 99,
            "source_start_frame": 0,
            "source_end_frame": 10
        }),
    )
    .await;
    assert_eq!(unknown.is_error, Some(true));
    assert_eq!(
        unknown.structured_content.as_ref().unwrap()["code"],
        "unknown_asset"
    );

    let inverted = invoke_capability(
        &client,
        "capture_room_tone",
        json!({
            "expected_revision": cc7_revision(&client).await,
            "asset_id": 1,
            "source_start_frame": 10,
            "source_end_frame": 10
        }),
    )
    .await;
    assert_eq!(inverted.is_error, Some(true));
    assert_eq!(
        inverted.structured_content.as_ref().unwrap()["code"],
        "invalid_source_range"
    );

    let too_long = invoke_capability(
        &client,
        "capture_room_tone",
        json!({
            "expected_revision": cc7_revision(&client).await,
            "asset_id": 1,
            "source_start_frame": 0,
            "source_end_frame": 2000
        }),
    )
    .await;
    assert_eq!(too_long.is_error, Some(true));
    assert_eq!(
        too_long.structured_content.as_ref().unwrap()["code"],
        "room_tone_capture_too_long"
    );

    client.cancel().await.unwrap();
    server.shutdown();

    let unsaved = Core::spawn(scene.document.clone()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(unsaved, media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let unsaved_capture = invoke_capability(
        &client,
        "capture_room_tone",
        json!({
            "expected_revision": 0,
            "asset_id": 1,
            "source_start_frame": AU6_C_LEARN_SOURCE_RANGE.start.0,
            "source_end_frame": AU6_C_LEARN_SOURCE_RANGE.end.0
        }),
    )
    .await;
    assert_eq!(unsaved_capture.is_error, Some(true));
    assert_eq!(
        unsaved_capture.structured_content.as_ref().unwrap()["code"],
        "project_not_saved"
    );
    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn au6_a4_the_multicam_cuts_leave_the_master_audio_untouched() {
    let scene = au6_agent_scene(Au6Scenario::Multicam);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let core = Core::spawn(scene.document.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let revision = au6_commit_ops_approved(
        &client,
        server.confirmations(),
        0,
        &au6_canonical_operations(Au6Scenario::Multicam),
    )
    .await;
    let levels = invoke_capability(&client, "get_audio_levels", json!({})).await;
    assert_eq!(levels.is_error, Some(false));
    let qc = invoke_capability(
        &client,
        "get_audio_qc",
        json!({"expected_revision": revision}),
    )
    .await;
    assert_eq!(qc.is_error, Some(false), "{:?}", qc.structured_content);
    cc7_assert_stale_revision(
        &client,
        "get_audio_qc",
        json!({"expected_revision": revision + 3}),
        revision,
        revision + 3,
    )
    .await;
    let mut expected = scene.document.clone();
    apply_batch(
        &mut expected,
        &au6_canonical_operations(Au6Scenario::Multicam),
    )
    .expect("canonical (d)");
    assert_eq!(query_document(&core), expected);
    client.cancel().await.unwrap();
    server.shutdown();
}

async fn au6_queue_and_poll(
    client: &RunningService<RoleClient, ()>,
    revision: u64,
    profile: &str,
    directory: &std::path::Path,
    await_complete: bool,
) -> serde_json::Value {
    let output = directory.join(format!("{profile}.mp4"));
    let queued = invoke_capability(
        client,
        "queue_export",
        json!({
            "expected_revision": revision,
            "output_path": output,
            "profile": profile,
            "normalize_loudness": true
        }),
    )
    .await;
    assert_eq!(
        queued.is_error,
        Some(false),
        "{:?}",
        queued.structured_content
    );
    let job = queued.structured_content.as_ref().unwrap()["job"].clone();
    assert_eq!(job["profile"], profile);
    if !await_complete {
        return job;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    loop {
        let jobs = invoke_capability(client, "get_export_jobs", json!({})).await;
        let record = jobs.structured_content.as_ref().unwrap()["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == job["id"])
            .cloned()
            .expect("the queued job is listed");
        if matches!(
            record["state"].as_str(),
            Some("completed" | "failed" | "cancelled")
        ) {
            return record;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the export never settled: {record}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn au6_a5a_the_delivery_lands_on_the_ebu_r128_target() {
    let scene = au6_agent_scene(Au6Scenario::Interview);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let core = Core::spawn(scene.document.clone()).unwrap();
    let server =
        McpServer::start_with_exporter(core.clone(), media.clone(), media.clone(), media.clone())
            .unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let revision = au6_commit_ops(
        &client,
        0,
        &au6_canonical_operations(Au6Scenario::Interview),
    )
    .await;
    let directory = std::env::temp_dir().join(format!(
        "kinewright-au6-a5a-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let record = au6_queue_and_poll(&client, revision, "source_master", &directory, true).await;
    assert_eq!(record["state"], "completed", "{record}");
    assert_eq!(record["audio_report"]["limiter_passes"], 1, "{record}");
    assert_eq!(
        record["audio_verification"]["technical_pass"], true,
        "{record}"
    );
    client.cancel().await.unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test(flavor = "multi_thread")]
async fn au6_a5b_the_streaming_target_is_reachable_by_the_agent() {
    let scene = au6_agent_scene(Au6Scenario::Interview);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let core = Core::spawn(scene.document.clone()).unwrap();
    let server =
        McpServer::start_with_exporter(core.clone(), media.clone(), media.clone(), media.clone())
            .unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let revision = au6_commit_ops(
        &client,
        0,
        &au6_canonical_operations(Au6Scenario::Interview),
    )
    .await;
    let profiles = invoke_capability(&client, "get_delivery_profiles", json!({})).await;
    let youtube = profiles.structured_content.as_ref().unwrap()["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["id"] == "youtube_1080p")
        .cloned()
        .expect("youtube_1080p is published");
    assert_eq!(
        youtube["loudness_target"]["integrated_lufs_hundredths"],
        -1400
    );
    assert_eq!(
        youtube["loudness_target"]["true_peak_ceiling_dbtp_hundredths"],
        -100
    );
    assert_eq!(
        youtube["resolution"],
        json!({"width": 1920, "height": 1080})
    );

    let directory = std::env::temp_dir().join(format!(
        "kinewright-au6-a5b-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let job = au6_queue_and_poll(&client, revision, "youtube1080p", &directory, false).await;
    assert_eq!(job["profile"], "youtube1080p");
    assert!(
        matches!(job["state"].as_str(), Some("queued" | "running")),
        "{job}"
    );
    let listed = invoke_capability(&client, "get_export_jobs", json!({})).await;
    assert!(
        listed.structured_content.as_ref().unwrap()["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == job["id"] && entry["profile"] == "youtube1080p")
    );
    let cancelled = invoke_capability(&client, "cancel_export", json!({"job_id": job["id"]})).await;
    assert_eq!(
        cancelled.is_error,
        Some(false),
        "{:?}",
        cancelled.structured_content
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let jobs = invoke_capability(&client, "get_export_jobs", json!({})).await;
        let record = jobs.structured_content.as_ref().unwrap()["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == job["id"])
            .cloned()
            .unwrap();
        if matches!(
            record["state"].as_str(),
            Some("cancelled" | "completed" | "failed")
        ) {
            assert_ne!(record["state"], "failed", "{record}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{record}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    client.cancel().await.unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test(flavor = "multi_thread")]
async fn au6_b_a_clip_whose_head_window_is_silent_gets_no_fade() {
    let scene = au6_agent_scene(Au6Scenario::Podcast);
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let core = Core::spawn(scene.document).unwrap();
    let server = McpServer::start(core, media.clone(), media).unwrap();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();
    let planned = au5_invoke_when_silence_is_ready(
        &client,
        "plan_clip_fades",
        json!({"tracks": [AU6_B_VOICE_A_TRACK.0]}),
    )
    .await;
    let text = au6_error_text(&planned);
    assert_eq!(planned.is_error, Some(false), "{text}");
    let body = planned.structured_content.as_ref().unwrap();
    assert!(
        text.contains(AU6_CLIP_FADES_NOTHING_TO_PROPOSE)
            || body["operations"].as_array().is_some_and(Vec::is_empty)
            || body["prepared_edit_plan"]["operations"]
                .as_array()
                .is_some_and(Vec::is_empty),
        "a silent head window must propose no fades: {body} {text}"
    );
    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn au6_the_four_planners_prose_is_pinned_by_exact_string() {
    assert!(AU6_DUCKING_ALREADY_RIDDEN_REFUSAL.contains("replace: true"));
    assert_eq!(AU6_CLIP_FADES_NOTHING_TO_PROPOSE, "nothing to propose");
    assert!(AU6_ROOM_TONE_NO_ASSET_REFUSAL.contains("capture_room_tone"));
    assert!(AU6_REPAIR_ALREADY_REPAIRED_REFUSAL.contains("AU5 repair prefix"));
}
