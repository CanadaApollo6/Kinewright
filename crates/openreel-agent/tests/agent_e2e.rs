use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use openreel_agent::{ClaudeCodeDriver, McpServer};
use openreel_core::{
    AgentDriver, AgentEvent, AssetId, Clip, ClipId, Command, Core, Document, Event, MediaAsset,
    MediaKind, Query, QueryResult, Rational, SessionConfig, TimeCode, Track, TrackId, TrackKind,
};
use openreel_media::FfmpegMediaEngine;

#[test]
fn claude_splits_then_deletes_via_the_live_mcp_server() {
    if std::env::var("OPENREEL_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_AGENT_TEST=1 to use the installed Claude Code CLI");
        return;
    }

    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let core = Core::spawn(original.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let confirmations = server.confirmations();
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

    let deadline = Instant::now() + Duration::from_mins(3);
    let mut approved = 0;
    loop {
        for request in confirmations.pending_requests() {
            println!("CONFIRM: {} — {}", request.tool_name, request.description);
            assert!(confirmations.approve(request.id));
            approved += 1;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "Claude Code turn timed out");
        let event = match events.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(event) => event,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                panic!("Claude Code event stream ended")
            }
        };
        println!("AGENT: {event:?}");
        if event == AgentEvent::Done {
            break;
        }
    }
    assert_eq!(approved, 1, "the delete must require one approval");

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

#[test]
fn codex_splits_then_deletes_via_the_live_mcp_server() {
    if std::env::var("OPENREEL_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_AGENT_TEST=1 to use the installed Codex CLI");
        return;
    }

    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let core = Core::spawn(original.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let confirmations = server.confirmations();
    let mut session = openreel_agent::CodexDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            max_turns: Some(2),
            mcp_url: Some(server.endpoint().to_owned()),
        })
        .expect("the gated test requires Codex CLI 0.147.0+ with a subscription login");
    let events = session.events();

    let prompt = "split the first clip at frame 30 then delete the second clip";
    println!("USER: {prompt}");
    session.send_user_message(prompt.to_owned()).unwrap();

    let deadline = Instant::now() + Duration::from_mins(3);
    let mut approved = 0;
    loop {
        for request in confirmations.pending_requests() {
            println!("CONFIRM: {} — {}", request.tool_name, request.description);
            assert!(confirmations.approve(request.id));
            approved += 1;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "Codex turn timed out");
        let event = match events.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(event) => event,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                panic!("Codex event stream ended")
            }
        };
        println!("AGENT: {event:?}");
        if let AgentEvent::Error(error) = &event {
            panic!("Codex driver error: {error}");
        }
        if event == AgentEvent::Done {
            break;
        }
    }
    assert_eq!(approved, 1, "the delete must require one approval");

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

fn fixture_document() -> Document {
    let asset = MediaAsset {
        id: AssetId(1),
        path: PathBuf::from("headless-agent-fixture.mp4"),
        name: "headless fixture".to_owned(),
        duration: TimeCode(180),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((320, 180)),
    };
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![
                Clip {
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(90),
                    content: openreel_core::ClipContent::Media,
                    timeline_start: TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                },
                Clip {
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(90)..TimeCode(150),
                    content: openreel_core::ClipContent::Media,
                    timeline_start: TimeCode(90),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                },
            ],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps: Rational::new(30, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(150),
    }
}
