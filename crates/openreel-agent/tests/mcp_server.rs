use std::{
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use openreel_agent::McpServer;
use openreel_core::{
    Clip, ClipId, Command, Core, Document, Event, MediaEngine, Query, QueryResult, Rational,
    TimeCode, Track, TrackId, TrackKind,
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

struct TestClip(PathBuf);

impl TestClip {
    fn generate() -> Self {
        let ffmpeg = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/ffmpeg/bin/ffmpeg.exe");
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

impl Drop for TestClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
