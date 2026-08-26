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
