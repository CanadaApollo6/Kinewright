use std::{
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openreel_agent::{ClaudeCodeDriver, McpServer};
use openreel_core::{
    AgentDriver, AgentEvent, Clip, ClipId, Command, Core, Document, Event, MediaAsset, MediaEngine,
    Query, QueryResult, Rational, SessionConfig, TimeCode, Track, TrackId, TrackKind,
};
use openreel_media::FfmpegMediaEngine;

#[test]
fn claude_splits_then_deletes_via_the_live_mcp_server() {
    if std::env::var("OPENREEL_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_AGENT_TEST=1 to use the installed Claude Code CLI");
        return;
    }

    let generated = TestClip::generate();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let asset = media.probe(&generated.0).unwrap();
    let original = fixture_document(asset);
    let core = Core::spawn(original.clone()).unwrap();
    let agent_media: Arc<dyn MediaEngine> = media;
    let server = McpServer::start(core.clone(), agent_media).unwrap();
    let mut session = ClaudeCodeDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            max_turns: Some(8),
            mcp_url: Some(server.endpoint().to_owned()),
        })
        .expect("the gated test requires an installed, authenticated Claude Code CLI");
    let events = session.events();

    let prompt = "split the first clip at frame 30 then delete the second clip";
    println!("USER: {prompt}");
    session.send_user_message(prompt.to_owned()).unwrap();

    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "Claude Code turn timed out");
        let event = events
            .recv_timeout(remaining.min(Duration::from_secs(10)))
            .expect("Claude Code event stream ended or stalled");
        println!("AGENT: {event:?}");
        if event == AgentEvent::Done {
            break;
        }
    }

    let edited = query_document(&core);
    let clips = &edited.tracks[0].clips;
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].id, ClipId(1));
    assert_eq!(clips[0].source_range, TimeCode(0)..TimeCode(30));
    assert_eq!(clips[1].id, ClipId(3));
    assert_eq!(clips[1].source_range, TimeCode(30)..TimeCode(90));
    assert!(edited.clip(ClipId(2)).is_none());
    println!("ASSERT: clips are [1:0..30, 3:30..90]; clip 2 is deleted");

    let _ = core.request(Command::Undo).unwrap();
    let _ = core.request(Command::Undo).unwrap();
    assert_eq!(&*query_document(&core), &original);
    println!("ASSERT: two undo commands restore the original two-clip document");

    session.interrupt();
    server.shutdown();
}

fn query_document(core: &Core) -> Arc<Document> {
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected document query result");
    };
    document
}

fn fixture_document(asset: MediaAsset) -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![
                Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(90),
                    timeline_start: TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(90)..TimeCode(150),
                    timeline_start: TimeCode(90),
                    effects: Vec::new(),
                    transition_in: None,
                },
            ],
        }],
        media_pool: vec![asset],
        fps: Rational::new(30, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(150),
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
            "openreel-m3-agent-{}-{nonce}.mp4",
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
                "testsrc2=size=320x180:rate=30",
                "-frames:v",
                "180",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "60",
                "-an",
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
