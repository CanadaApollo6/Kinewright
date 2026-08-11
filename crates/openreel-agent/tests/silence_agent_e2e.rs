use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use openreel_agent::{ClaudeCodeDriver, McpServer};
use openreel_core::{
    AgentDriver, AgentEvent, Analysis, AssetId, Command as CoreCommand, Core, Document, Event,
    Playback, Query, QueryResult, SessionConfig, SilenceStatus, TimeCode,
    map_source_range_to_project,
};
use openreel_media::test_support::{
    SpeechClip, joined_words, normalized_words, single_clip_document, test_engine,
    wait_for_transcript,
};

const TTS_SSML: &str = "<speak version='1.0' xml:lang='en-US'>Alpha.<break time='1400ms'/>Bravo.<break time='1400ms'/>Charlie.</speak>";

#[test]
// This gated E2E keeps generation, transcription, agent editing, and verification together.
#[allow(clippy::too_many_lines)]
fn claude_removes_long_silences_with_one_atomic_plan() {
    if std::env::var("OPENREEL_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_AGENT_TEST=1 to run SAPI, Whisper, and Claude");
        return;
    }

    let clip = SpeechClip::ssml("agent-silence", TTS_SSML, "OPENREEL_AGENT_TEST_TTS_WAV");
    let media = Arc::new(test_engine("OPENREEL_AGENT_TEST_DATA_DIR"));
    let asset = media.probe(&clip.mp4).expect("generated clip should probe");
    media.request_transcription(asset.clone());
    media.request_silence_detection(asset.clone());
    media.request_scene_detection(asset.clone());
    let transcript = wait_for_transcript(media.as_ref(), asset.id, false);
    let silences = wait_for_silences(media.as_ref(), asset.id);
    let transcript_text = joined_words(&transcript);
    let long_silences = silences
        .spans
        .iter()
        .filter(|span| span.source_end.0.saturating_sub(span.source_start.0) >= 20)
        .count();
    assert!(long_silences >= 2, "fixture silences: {:?}", silences.spans);

    let original = single_clip_document(asset);
    media.set_document(Arc::new(original.clone()));
    let core = Core::spawn(original.clone()).expect("core should start");
    let server = McpServer::start(core.clone(), media.clone(), media.clone())
        .expect("MCP server should start");
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
    let prompt = "remove the silent gaps from this take";

    println!("TTS SSML: {TTS_SSML}");
    println!("TRANSCRIPT: {transcript_text}");
    println!("SILENCES: {:?}", silences.spans);
    println!("USER: {prompt}");
    session.send_user_message(prompt.to_owned()).unwrap();

    let deadline = Instant::now() + Duration::from_mins(5);
    let mut tool_names = Vec::new();
    let mut approved = 0_u32;
    loop {
        for request in confirmations.pending_requests() {
            println!("CONFIRM: {} - {}", request.tool_name, request.description);
            assert_eq!(request.tool_name, "apply_edit_plan");
            assert!(confirmations.approve(request.id));
            approved = approved.saturating_add(1);
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
        if let AgentEvent::ToolCall { name, .. } = &event {
            tool_names.push(name.clone());
        }
        if let AgentEvent::Error(error) = &event {
            panic!("Claude Code driver error: {error}");
        }
        if event == AgentEvent::Done {
            break;
        }
    }

    // The agent's plan shape is nondeterministic: it may remove silence via
    // delete_clip (destructive -> exactly one summarized confirmation) or via
    // trim/add operations (non-destructive -> none). Either is correct; the
    // deterministic approve/reject broker behavior is covered by the MCP
    // integration tests. What must never happen is more than one prompt per
    // plan.
    assert!(
        approved <= 1,
        "a single edit plan must never ask for confirmation more than once (asked {approved} times)"
    );
    assert!(
        tool_names
            .iter()
            .any(|name| name == "get_silences" || name == "get_timeline_silences"),
        "agent did not use a silence inspector: {tool_names:?}"
    );
    assert!(
        tool_names.iter().any(|name| name == "apply_edit_plan"),
        "agent did not submit an edit plan: {tool_names:?}"
    );

    let edited = query_document(&core);
    let remaining_silences = media
        .timeline_silences(&edited, None, TimeCode(20))
        .expect("edited silence mapping should succeed");
    assert!(
        remaining_silences.is_empty(),
        "long silence remains after edit: {remaining_silences:?}"
    );
    for adjacent in edited.tracks[0].clips.windows(2) {
        let asset = edited
            .asset(adjacent[0].asset)
            .expect("edited clips must retain their assets");
        let duration =
            map_source_range_to_project(adjacent[0].source_range.clone(), asset.fps, edited.fps)
                .unwrap();
        let left_end = adjacent[0].timeline_start.checked_add(duration).unwrap();
        assert_eq!(
            left_end, adjacent[1].timeline_start,
            "the silence was deleted but left an empty project gap"
        );
    }
    let final_words = media.timeline_transcript(&edited, None).unwrap();
    let final_transcript = final_words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = normalized_words(&final_transcript);
    println!("FINAL TRANSCRIPT: {final_transcript}");
    println!("FINAL CLIPS: {:?}", edited.tracks[0].clips);
    // Retention is asserted RELATIVE to the pre-edit transcript: every word
    // Whisper heard before the edit must survive it. Hardcoding the intended
    // script would couple the test to ASR accuracy (Whisper has rendered this
    // fixture's "Alpha." as "Al").
    let original_words = normalized_words(&transcript_text);
    assert!(
        !original_words.is_empty(),
        "the fixture transcript must contain words"
    );
    for expected in &original_words {
        assert!(
            normalized.iter().any(|word| word == expected),
            "content word {expected:?} was lost: {final_transcript}"
        );
    }

    let Event::DocumentChanged { doc, .. } = core.request(CoreCommand::Undo).unwrap() else {
        panic!("undo should return the restored document");
    };
    assert_eq!(&*doc, &original, "one undo must restore the original take");
    println!("ASSERT: one undo restored the original document");

    session.interrupt();
    server.shutdown();
}

fn wait_for_silences(engine: &dyn Analysis, asset: AssetId) -> Arc<openreel_core::AssetSilences> {
    let deadline = Instant::now() + Duration::from_mins(1);
    loop {
        match engine.silence_status(asset) {
            SilenceStatus::Ready(silences) => return silences,
            SilenceStatus::NoAudio => panic!("SAPI fixture has no audio stream"),
            SilenceStatus::Failed(error) => panic!("silence analysis failed: {error}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "silence analysis timed out");
        thread::sleep(Duration::from_millis(50));
    }
}

fn query_document(core: &Core) -> Arc<Document> {
    let Event::QueryResult(QueryResult::Document(document)) = core
        .request(CoreCommand::Query(Query::Document))
        .expect("document query should succeed")
    else {
        panic!("expected document query result");
    };
    document
}
