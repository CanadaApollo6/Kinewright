use std::{
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use openreel_agent::McpServer;
use openreel_core::{
    AssetId, Clip, ClipId, Command, Core, Document, Event, MediaAsset, MediaEngine, MediaKind,
    Query, QueryResult, Rational, TimeCode, Track, TrackId, TrackKind,
};
use openreel_media::FfmpegMediaEngine;
use rmcp::{
    ServiceExt as _,
    model::CallToolRequestParams,
    transport::StreamableHttpClientTransport,
};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn mutator_tool_applies_through_the_real_core_actor() {
    let core = Core::spawn(Document::default()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let agent_media: Arc<dyn MediaEngine> = media;
    let server = McpServer::start(core.clone(), agent_media).unwrap();
    let client = ().serve(StreamableHttpClientTransport::from_uri(server.endpoint())).await.unwrap();

    let result = client
        .call_tool(
            CallToolRequestParams::new("add_track").with_arguments(
                json!({"track": {"id": 7, "kind": "Video", "clips": []}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();

    assert_eq!(result.is_error, Some(false));
    let outcome = &result.content[0].as_text().unwrap().text;
    assert!(outcome.contains("applied add_track"));
    assert!(outcome.contains("tracks 0->1"));
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query result");
    };
    assert_eq!(document.tracks[0].id, TrackId(7));

    let rejected = client
        .call_tool(
            CallToolRequestParams::new("add_track").with_arguments(
                json!({"track": {"id": 7, "kind": "Video", "clips": []}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(rejected.is_error, Some(true));
    assert_eq!(
        rejected.content[0].as_text().unwrap().text,
        "track 7 occurs more than once"
    );

    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_plans_cross_the_real_mcp_server_atomically_with_one_confirmation() {
    let original = edit_plan_document();
    let core = Core::spawn(original.clone()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let agent_media: Arc<dyn MediaEngine> = media;
    let server = McpServer::start(core.clone(), agent_media).unwrap();
    let confirmations = server.confirmations();
    let client =
        ().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))
            .await
            .unwrap();

    let applied = client
        .call_tool(plan_request(json!([
            {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}},
            {"MoveClip": {"clip": 1, "to_track": 2, "to": 0}}
        ])))
        .await
        .unwrap();
    assert_eq!(applied.is_error, Some(false));
    let applied_text = &applied.content[0].as_text().unwrap().text;
    assert!(applied_text.contains("op 1 add_track: applied"));
    assert!(applied_text.contains("op 2 move_clip: applied"));
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("one undo should restore the pre-plan document");
    };
    assert_eq!(&*doc, &original);

    let rejected = client
        .call_tool(plan_request(json!([
            {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}},
            {"AddTrack": {"track": {"id": 2, "kind": "Video", "clips": []}}}
        ])))
        .await
        .unwrap();
    assert_eq!(rejected.is_error, Some(true));
    let rejected_text = &rejected.content[0].as_text().unwrap().text;
    assert!(rejected_text.contains("op 1 add_track: rolled back"));
    assert!(rejected_text.contains("op 2 add_track: rejected"));
    assert_eq!(query_document(&core), original);

    let (approved, ()) = tokio::join!(
        client.call_tool(plan_request(json!([{"RemoveTrack": {"track": 1}}]))),
        resolve_plan_confirmation(confirmations.clone(), true),
    );
    let approved = approved.unwrap();
    assert_eq!(approved.is_error, Some(false));
    assert!(query_document(&core).tracks.is_empty());
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("undo should restore the approved destructive plan");
    };
    assert_eq!(&*doc, &original);

    let (refused, ()) = tokio::join!(
        client.call_tool(plan_request(json!([{"RemoveTrack": {"track": 1}}]))),
        resolve_plan_confirmation(confirmations, false),
    );
    let refused = refused.unwrap();
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(query_document(&core), original);

    client.cancel().await.unwrap();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn get_frame_at_returns_a_downscaled_png_for_generated_media() {
    let generated = TestClip::generate();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(&generated.0).unwrap();
    let document = fixture_document(asset);
    let core = Core::spawn(document).unwrap();
    let agent_media: Arc<dyn MediaEngine> = media;
    let server = McpServer::start(core, agent_media).unwrap();
    let client = ().serve(StreamableHttpClientTransport::from_uri(server.endpoint())).await.unwrap();

    let result = client
        .call_tool(
            CallToolRequestParams::new("get_frame_at").with_arguments(
                json!({"timecode": 30}).as_object().unwrap().clone(),
            ),
        )
        .await
        .unwrap();

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

    client.cancel().await.unwrap();
    server.shutdown();
}

fn fixture_document(asset: openreel_core::MediaAsset) -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode(0)..TimeCode(60),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
            }],
        }],
        media_pool: vec![asset],
        fps: Rational::new(30_000, 1_001).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(60),
    }
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
    };
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..TimeCode(60),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
            }],
        }],
        media_pool: vec![asset],
        fps: Rational::new(30, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(60),
    }
}

fn plan_request(operations: serde_json::Value) -> CallToolRequestParams {
    CallToolRequestParams::new("apply_edit_plan").with_arguments(
        json!({"operations": operations})
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

async fn resolve_plan_confirmation(broker: openreel_agent::ConfirmationBroker, approve: bool) {
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

struct TestClip(PathBuf);

impl TestClip {
    fn generate() -> Self {
        let ffmpeg = ffmpeg_executable();
        assert!(ffmpeg.is_file(), "provisioned ffmpeg.exe is missing");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = std::env::temp_dir().join(format!(
            "openreel-m3-{}-{nonce}.mp4",
            std::process::id()
        ));
        let status = ProcessCommand::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
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
            ])
            .arg(&output)
            .status()
            .expect("failed to run provisioned ffmpeg.exe");
        assert!(status.success(), "test media generation failed");
        Self(output)
    }
}

fn ffmpeg_executable() -> PathBuf {
    std::env::var_os("FFMPEG_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../third_party/ffmpeg/bin/ffmpeg.exe")
        },
        |directory| PathBuf::from(directory).join("bin/ffmpeg.exe"),
    )
}

impl Drop for TestClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
