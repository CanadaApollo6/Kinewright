use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use openreel_agent::{ClaudeCodeDriver, McpServer};
use openreel_core::{
    AgentDriver, AgentEvent, Analysis, Command as CoreCommand, Core, Document, Event, Playback,
    Query, QueryResult, SessionConfig,
};
use openreel_media::test_support::{
    SpeechClip, joined_words, normalized_words, single_clip_document, test_engine,
    wait_for_transcript,
};

const TTS_TEXT: &str = "Hello, um, this is an Open Reel transcript test.";

#[test]
fn one_agent_message_removes_the_transcribed_filler_word() {
    if std::env::var("OPENREEL_TRANSCRIPT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_TRANSCRIPT_TEST=1 to run SAPI, Whisper, and Claude");
        return;
    }

    let clip = SpeechClip::plain("agent-transcript", TTS_TEXT, "OPENREEL_TRANSCRIPT_AUDIO");
    let media = Arc::new(test_engine("OPENREEL_TRANSCRIPT_TEST_DATA_DIR"));
    let asset = media.probe(&clip.mp4).expect("generated clip should probe");
    media.request_transcription(asset.clone());
    let transcript = wait_for_transcript(media.as_ref(), asset.id, false);
    let whisper_output = joined_words(&transcript);
    let whisper_words = normalized_words(&whisper_output);
    assert!(
        whisper_words.iter().any(|word| word == "um"),
        "fixture must contain a recognized filler word: {whisper_output}"
    );

    let original = single_clip_document(asset);
    media.set_document(Arc::new(original.clone()));
    let core = Core::spawn(original).expect("core should start");
    let server = McpServer::start(core.clone(), media.clone(), media.clone())
        .expect("MCP server should start");
    let confirmations = server.confirmations();
    let mut session = ClaudeCodeDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
            effort: None,
            max_turns: Some(10),
            mcp_url: Some(server.endpoint().to_owned()),
        })
        .expect("the gated test requires an installed, authenticated Claude Code CLI");
    let events = session.events();
    let prompt = "remove the filler words";

    println!("TTS: {TTS_TEXT}");
    println!("WHISPER: {whisper_output}");
    println!("USER: {prompt}");
    session
        .send_user_message(prompt.to_owned())
        .expect("agent message should send");

    let deadline = Instant::now() + Duration::from_mins(5);
    let mut approved = 0_u32;
    loop {
        for request in confirmations.pending_requests() {
            println!("CONFIRM: {} - {}", request.tool_name, request.description);
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
        if event == AgentEvent::Done {
            break;
        }
    }

    let edited = query_document(&core);
    let final_words = media
        .timeline_transcript(&edited, None)
        .expect("edited timeline transcript should map");
    let final_output = final_words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let normalized_final = normalized_words(&final_output);
    println!("FINAL TIMELINE: {final_output}");
    println!("FINAL CLIPS: {:?}", edited.tracks[0].clips);

    println!("APPROVED DESTRUCTIVE EDITS: {approved}");
    assert!(
        !normalized_final.iter().any(|word| word == "um"),
        "filler word remains on the edited timeline: {final_output}"
    );
    for expected in ["hello", "this", "open", "transcript", "test"] {
        assert!(
            normalized_final.iter().any(|word| word == expected),
            "content word `{expected}` was lost: {final_output}"
        );
    }

    session.interrupt();
    server.shutdown();
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
