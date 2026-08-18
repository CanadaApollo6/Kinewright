use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use kinewright_agent::{
    BranchApplyOutcome, ClaudeCodeDriver, CursorAcpDriver, McpServer, TimelineBranch,
};
use kinewright_core::{
    AgentDriver, AgentEvent, AssetId, Clip, ClipId, Command, Core, Document, Event, MediaAsset,
    MediaKind, Query, QueryResult, Rational, SessionConfig, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::FfmpegMediaEngine;

#[test]
fn claude_splits_then_deletes_via_the_live_mcp_server() {
    if std::env::var("KINEWRIGHT_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set KINEWRIGHT_AGENT_TEST=1 to use the installed Claude Code CLI");
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
            effort: None,
            service_tier: None,
            max_turns: Some(8),
            mcp_url: Some(server.endpoint().to_owned()),
            tool_names: None,
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
    assert_eq!(&*query_document(&core), &original);
    println!("ASSERT: one undo restores the atomic edit plan");

    session.interrupt();
    server.shutdown();
}

#[test]
fn codex_splits_then_deletes_via_the_live_mcp_server() {
    if std::env::var("KINEWRIGHT_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set KINEWRIGHT_AGENT_TEST=1 to use the installed Codex CLI");
        return;
    }

    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let core = Core::spawn(original.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let confirmations = server.confirmations();
    let mut session = kinewright_agent::CodexDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            effort: None,
            service_tier: None,
            max_turns: Some(2),
            mcp_url: Some(server.endpoint().to_owned()),
            tool_names: None,
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
    assert_eq!(&*query_document(&core), &original);
    println!("ASSERT: one undo restores the atomic edit plan");

    session.interrupt();
    server.shutdown();
}

#[test]
fn codex_edits_an_isolated_branch_then_one_merge_publishes_it() {
    if std::env::var("KINEWRIGHT_BRANCH_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set KINEWRIGHT_BRANCH_AGENT_TEST=1 to run the installed Codex branch smoke"
        );
        return;
    }

    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let live = Core::spawn(original.clone()).unwrap();
    let Event::QueryResult(QueryResult::Snapshot { revision, document }) =
        live.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("expected live snapshot");
    };
    let branch = TimelineBranch::new("Codex smoke", revision, document).unwrap();
    let server = McpServer::start_isolated(branch.core(), media.clone(), media).unwrap();
    let mut session = kinewright_agent::CodexDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            effort: None,
            service_tier: None,
            max_turns: Some(2),
            mcp_url: Some(server.endpoint().to_owned()),
            tool_names: None,
        })
        .expect("the gated test requires Codex CLI 0.147.0+ with a subscription login");
    let events = session.events();
    let prompt = "Inspect the timeline. Apply one atomic edit plan that splits clip 1 at project frame 30 and adds a marker at frame 30 labeled M31 proof. Then inspect the timeline and run the QA report.";
    println!("USER: {prompt}");
    session.send_user_message(prompt.to_owned()).unwrap();

    let deadline = Instant::now() + Duration::from_mins(3);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "Codex branch turn timed out");
        let event = events
            .recv_timeout(remaining.min(Duration::from_millis(100)))
            .unwrap_or_else(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => AgentEvent::Text(String::new()),
                crossbeam_channel::RecvTimeoutError::Disconnected => {
                    panic!("Codex branch event stream ended")
                }
            });
        if event == AgentEvent::Text(String::new()) {
            continue;
        }
        println!("AGENT: {event:?}");
        if let AgentEvent::Error(error) = &event {
            panic!("Codex driver error: {error}");
        }
        if event == AgentEvent::Done {
            break;
        }
    }

    assert_eq!(
        &*query_document(&live),
        &original,
        "branch leaked into live"
    );
    let comparison = branch.compare().unwrap();
    assert_eq!(comparison.operations.len(), 2);
    assert_eq!(comparison.document.markers.len(), 1);
    assert_eq!(comparison.document.tracks[0].clips.len(), 3);
    let outcome = branch.merge_into(&live).unwrap();
    assert!(matches!(
        outcome,
        BranchApplyOutcome::Applied {
            revision: kinewright_core::TimelineRevision(1),
            operation_count: 2,
            ..
        }
    ));
    assert_eq!(&*query_document(&live), &*comparison.document);
    live.request(Command::Undo).unwrap();
    assert_eq!(&*query_document(&live), &original);
    println!("ASSERT: live stayed unchanged until one merge; one undo restored the base");

    session.interrupt();
    server.shutdown();
}

#[test]
fn codex_uses_m32_source_and_catalog_primitives_on_an_isolated_branch() {
    if std::env::var("KINEWRIGHT_M32_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set KINEWRIGHT_M32_AGENT_TEST=1 to run the Codex M32 smoke");
        return;
    }

    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let live = Core::spawn(original.clone()).unwrap();
    let Event::QueryResult(QueryResult::Snapshot { revision, document }) =
        live.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("expected live snapshot");
    };
    let branch = TimelineBranch::new("Codex M32 smoke", revision, document).unwrap();
    let server = McpServer::start_isolated(branch.core(), media.clone(), media).unwrap();
    let mut session = kinewright_agent::CodexDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            effort: None,
            service_tier: None,
            max_turns: Some(2),
            mcp_url: Some(server.endpoint().to_owned()),
            tool_names: None,
        })
        .expect("the gated test requires Codex CLI 0.147.0+ with a subscription login");
    let events = session.events();
    let prompt = "Use search_media to find the headless fixture and get_source_info to inspect source frames 30 through 120. Then apply exactly one atomic edit plan with three operations: slip clip 1 to new_source_in 30; create bin 1 named Selects containing asset 1; create string-out 1 named Opening selects with one select from asset 1, source 30 through 60, labeled Opening. Inspect the final timeline.";
    println!("USER: {prompt}");
    session.send_user_message(prompt.to_owned()).unwrap();

    let deadline = Instant::now() + Duration::from_mins(3);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "Codex M32 turn timed out");
        let event = events
            .recv_timeout(remaining.min(Duration::from_millis(100)))
            .unwrap_or_else(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => AgentEvent::Text(String::new()),
                crossbeam_channel::RecvTimeoutError::Disconnected => {
                    panic!("Codex M32 event stream ended")
                }
            });
        if event == AgentEvent::Text(String::new()) {
            continue;
        }
        println!("AGENT: {event:?}");
        if let AgentEvent::Error(error) = &event {
            panic!("Codex driver error: {error}");
        }
        if event == AgentEvent::Done {
            break;
        }
    }

    assert_eq!(
        &*query_document(&live),
        &original,
        "branch leaked into live"
    );
    let comparison = branch.compare().unwrap();
    assert_eq!(comparison.operations.len(), 3);
    assert_eq!(
        comparison.document.clip(ClipId(1)).unwrap().source_range,
        TimeCode(30)..TimeCode(120)
    );
    assert_eq!(comparison.document.catalog.bins.len(), 1);
    assert_eq!(comparison.document.catalog.string_outs.len(), 1);
    let outcome = branch.merge_into(&live).unwrap();
    assert!(matches!(
        outcome,
        BranchApplyOutcome::Applied {
            operation_count: 3,
            ..
        }
    ));
    live.request(Command::Undo).unwrap();
    assert_eq!(&*query_document(&live), &original);
    println!("ASSERT: one model plan used M32 primitives; branch merge and one undo are exact");

    session.interrupt();
    server.shutdown();
}

#[test]
fn codex_builds_m33_visual_automation_and_an_audio_bus_on_an_isolated_branch() {
    if std::env::var("KINEWRIGHT_M33_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set KINEWRIGHT_M33_AGENT_TEST=1 to run the Codex M33 smoke");
        return;
    }

    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let live = Core::spawn(original.clone()).unwrap();
    let Event::QueryResult(QueryResult::Snapshot { revision, document }) =
        live.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("expected live snapshot");
    };
    let branch = TimelineBranch::new("Codex M33 smoke", revision, document).unwrap();
    let server = McpServer::start_isolated(branch.core(), media.clone(), media).unwrap();
    let mut session = kinewright_agent::CodexDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            effort: None,
            service_tier: None,
            max_turns: Some(2),
            mcp_url: Some(server.endpoint().to_owned()),
            tool_names: None,
        })
        .expect("the gated test requires Codex CLI 0.147.0+ with a subscription login");
    let events = session.events();
    let prompt = "Inspect the timeline. Apply exactly one atomic edit plan with exactly three operations. First add color_grade effect 1 to clip 1 with exposure_milli_stops -1000, temperature_percent 0, and tint_percent 0. Second set effect 1 exposure_milli_stops keyframes at clip-local frame 0 value -1000 linear, frame 45 value 1000 ease_in_out, and frame 89 value 0 linear. Third upsert audio bus 1 named Dialogue routing track 1, with no sidechain tracks and three effects: audio_gain effect 2 at gain_tenth_db -30 with gain keyframes at project frames 0 value -60 linear, 75 value 0 ease_in_out, and 149 value -30 linear; audio_eq effect 3 with low 20, mid -30, and high 10 tenths dB; audio_compressor effect 4 with threshold -180 tenths dB, ratio 400 hundredths, attack 10 ms, release 250 ms, and makeup 20 tenths dB. Then inspect the final timeline.";
    println!("USER: {prompt}");
    session.send_user_message(prompt.to_owned()).unwrap();

    let deadline = Instant::now() + Duration::from_mins(3);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "Codex M33 turn timed out");
        let event = events
            .recv_timeout(remaining.min(Duration::from_millis(100)))
            .unwrap_or_else(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => AgentEvent::Text(String::new()),
                crossbeam_channel::RecvTimeoutError::Disconnected => {
                    panic!("Codex M33 event stream ended")
                }
            });
        if event == AgentEvent::Text(String::new()) {
            continue;
        }
        println!("AGENT: {event:?}");
        if let AgentEvent::Error(error) = &event {
            panic!("Codex driver error: {error}");
        }
        if event == AgentEvent::Done {
            break;
        }
    }

    assert_eq!(
        &*query_document(&live),
        &original,
        "branch leaked into live"
    );
    let comparison = branch.compare().unwrap();
    assert_eq!(comparison.operations.len(), 3);
    let clip_effect = &comparison.document.clip(ClipId(1)).unwrap().effects[0];
    assert_eq!(clip_effect.name, "color_grade");
    assert_eq!(
        clip_effect
            .keyframes
            .get("exposure_milli_stops")
            .unwrap()
            .value_at(TimeCode(45)),
        Some(1_000)
    );
    let bus = &comparison.document.audio_mix.buses[0];
    assert_eq!(bus.name, "Dialogue");
    assert_eq!(bus.tracks, vec![TrackId(1)]);
    assert_eq!(bus.effects.len(), 3);
    assert_eq!(
        bus.effects[0]
            .keyframes
            .get("gain_tenth_db")
            .unwrap()
            .value_at(TimeCode(75)),
        Some(0)
    );
    let outcome = branch.merge_into(&live).unwrap();
    assert!(matches!(
        outcome,
        BranchApplyOutcome::Applied {
            operation_count: 3,
            ..
        }
    ));
    live.request(Command::Undo).unwrap();
    assert_eq!(&*query_document(&live), &original);
    println!("ASSERT: Codex built M33 visual and mix graphs; merge and one undo are exact");

    session.interrupt();
    server.shutdown();
}

#[test]
fn cursor_splits_then_deletes_via_the_live_mcp_server() {
    if std::env::var("KINEWRIGHT_CURSOR_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set KINEWRIGHT_CURSOR_AGENT_TEST=1 to use the installed Cursor Agent CLI"
        );
        return;
    }

    eprintln!("CURSOR TEST: building fixture and MCP server");
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let original = fixture_document();
    let core = Core::spawn(original.clone()).unwrap();
    let server = McpServer::start(core.clone(), media.clone(), media).unwrap();
    let confirmations = server.confirmations();
    eprintln!("CURSOR TEST: starting ACP session");
    let mut session = CursorAcpDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            effort: None,
            service_tier: None,
            max_turns: Some(2),
            mcp_url: Some(server.endpoint().to_owned()),
            tool_names: None,
        })
        .expect("the gated test requires an installed, authenticated Cursor Agent CLI");
    eprintln!("CURSOR TEST: ACP session ready");
    let events = session.events();

    let prompt = "split the first clip at frame 30 then delete the second clip";
    println!("USER: {prompt}");
    eprintln!("CURSOR TEST: sending prompt");
    session.send_user_message(prompt.to_owned()).unwrap();
    eprintln!("CURSOR TEST: prompt accepted; polling events and confirmations");

    let deadline = Instant::now() + Duration::from_mins(3);
    let mut approved = 0;
    loop {
        for request in confirmations.pending_requests() {
            println!("CONFIRM: {} — {}", request.tool_name, request.description);
            assert!(confirmations.approve(request.id));
            approved += 1;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "Cursor turn timed out");
        let event = match events.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(event) => event,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                panic!("Cursor event stream ended")
            }
        };
        println!("AGENT: {event:?}");
        if let AgentEvent::Error(error) = &event {
            panic!("Cursor driver error: {error}");
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
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                Clip {
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
                },
                Clip {
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
