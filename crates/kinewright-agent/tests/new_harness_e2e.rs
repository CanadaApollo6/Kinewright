//! Live subscription tests for the second-wave harnesses (Muse, `OpenCode`,
//! Qwen, Kimi, Kiro, Devin, Copilot). Each test runs the same split/delete
//! scenario as the first-wave suite against the real installed CLI.
//!
//! Gate: `#[ignore]` plus `KINEWRIGHT_NEW_AGENT_TEST=1`. These make real,
//! billed model calls, so the default lane must not run them — and it must
//! not report them as passing either. They used to return early, which
//! `cargo test` renders as seven green ticks for seven no-ops, and the
//! `eprintln!` explaining why is swallowed by its output capture. `#[ignore]`
//! makes the default transcript say `7 ignored`, which is the truth. Run
//! them with:
//!
//! ```text
//! KINEWRIGHT_NEW_AGENT_TEST=1 cargo test -p kinewright-agent \
//!     --test new_harness_e2e -- --ignored
//! ```
//!
//! The environment gate stays on top of `--ignored`, so `cargo test --
//! --ignored` on a machine that has not opted in still skips rather than
//! spending someone's subscription. A driver whose CLI is not installed, or
//! whose harness reports missing authentication at session start, is skipped
//! with a message; anything else that fails is a real failure.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use kinewright_agent::{
    CopilotDriver, DevinDriver, KimiDriver, KiroDriver, McpServer, MuseDriver, OpenCodeDriver,
    QwenDriver,
};
use kinewright_core::{
    AgentDriver, AgentEvent, AssetId, Clip, ClipId, Command, Core, Document, Event, MediaAsset,
    MediaKind, Query, QueryResult, Rational, SessionConfig, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::FfmpegMediaEngine;

fn run_split_delete(driver: &dyn AgentDriver, label: &str, model: Option<String>) {
    if std::env::var("KINEWRIGHT_NEW_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set KINEWRIGHT_NEW_AGENT_TEST=1 to use the installed {label} CLI");
        return;
    }
    if driver.detect().is_none() {
        eprintln!("skipped: {label} is not installed on PATH");
        return;
    }

    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let core = Core::spawn(original.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let confirmations = server.confirmations();
    let mut session = match driver.start_session(SessionConfig {
        working_directory: std::env::current_dir().ok(),
        model,
        effort: None,
        service_tier: None,
        max_turns: Some(2),
        mcp_url: Some(server.endpoint().to_owned()),
        tool_names: None,
    }) {
        Ok(session) => session,
        Err(error) if is_missing_authentication(&error.to_string()) => {
            eprintln!("skipped: {label} is not authenticated ({error})");
            server.shutdown();
            return;
        }
        Err(error) => panic!("{label} session start failed: {error}"),
    };
    let events = session.events();

    let prompt = "split the first clip at frame 30 then delete the second clip";
    println!("USER: {prompt}");
    session.send_user_message(prompt.to_owned()).unwrap();

    let deadline = Instant::now() + Duration::from_mins(5);
    let mut approved = 0;
    loop {
        for request in confirmations.pending_requests() {
            println!("CONFIRM: {} — {}", request.tool_name, request.description);
            assert!(confirmations.approve(request.id));
            approved += 1;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "{label} turn timed out");
        let event = match events.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(event) => event,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                panic!("{label} event stream ended")
            }
        };
        println!("AGENT: {event:?}");
        if let AgentEvent::Error(error) = &event {
            panic!("{label} driver error: {error}");
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
    assert_eq!(&*query_document(&core), &original);
    println!("ASSERT: one undo restores the atomic edit plan");

    session.interrupt();
    server.shutdown();
}

fn is_missing_authentication(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("authentication required")
        || message.contains("not logged in")
        || message.contains("not authenticated")
        || message.contains("unauthenticated")
}

#[test]
#[ignore = "live subscription run: also needs KINEWRIGHT_NEW_AGENT_TEST=1"]
fn muse_splits_then_deletes_via_the_live_mcp_server() {
    run_split_delete(&MuseDriver, "Muse", None);
}

#[test]
#[ignore = "live subscription run: also needs KINEWRIGHT_NEW_AGENT_TEST=1"]
fn opencode_splits_then_deletes_via_the_live_mcp_server() {
    run_split_delete(&OpenCodeDriver, "OpenCode", None);
}

#[test]
#[ignore = "live subscription run: also needs KINEWRIGHT_NEW_AGENT_TEST=1"]
fn qwen_splits_then_deletes_via_the_live_mcp_server() {
    run_split_delete(&QwenDriver, "Qwen", None);
}

#[test]
#[ignore = "live subscription run: also needs KINEWRIGHT_NEW_AGENT_TEST=1"]
fn kimi_splits_then_deletes_via_the_live_mcp_server() {
    run_split_delete(&KimiDriver, "Kimi", None);
}

#[test]
#[ignore = "live subscription run: also needs KINEWRIGHT_NEW_AGENT_TEST=1"]
fn kiro_splits_then_deletes_via_the_live_mcp_server() {
    run_split_delete(&KiroDriver, "Kiro", None);
}

#[test]
#[ignore = "live subscription run: also needs KINEWRIGHT_NEW_AGENT_TEST=1"]
fn devin_splits_then_deletes_via_the_live_mcp_server() {
    run_split_delete(&DevinDriver, "Devin", None);
}

#[test]
#[ignore = "live subscription run: also needs KINEWRIGHT_NEW_AGENT_TEST=1"]
fn copilot_splits_then_deletes_via_the_live_mcp_server() {
    run_split_delete(&CopilotDriver, "Copilot", None);
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
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
        assumed_from: None,
    };
    Document {
        investigator: None,
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(90),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                },
                Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(2),
                    asset: asset.id,
                    source_range: TimeCode(90)..TimeCode(150),
                    content: kinewright_core::ClipContent::Media,
                    timeline_start: TimeCode(90),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                },
            ],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps: Rational::new(30, 1).unwrap(),
        resolution: (320, 180),
        duration: TimeCode(150),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
    }
}
