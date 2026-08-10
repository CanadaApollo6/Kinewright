use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openreel_agent::{ClaudeCodeDriver, McpServer};
use openreel_core::{
    AgentDriver, AgentEvent, AssetId, AssetTranscript, Clip, ClipId, Command as CoreCommand, Core,
    Document, Event, MediaEngine, Query, QueryResult, SessionConfig, SilenceStatus, TimeCode,
    Track, TrackId, TrackKind, TranscriptStatus, map_source_range_to_project,
};
use openreel_media::FfmpegMediaEngine;

const TTS_SSML: &str = "<speak version='1.0' xml:lang='en-US'>Alpha.<break time='1400ms'/>Bravo.<break time='1400ms'/>Charlie.</speak>";

#[test]
fn claude_removes_long_silences_with_one_atomic_plan() {
    if std::env::var("OPENREEL_AGENT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_AGENT_TEST=1 to run SAPI, Whisper, and Claude");
        return;
    }

    let clip = SpeechClip::generate();
    let media = Arc::new(test_engine());
    let asset = media.probe(&clip.mp4).expect("generated clip should probe");
    media.request_transcription(asset.clone());
    media.request_silence_detection(asset.clone());
    media.request_scene_detection(asset.clone());
    let transcript = wait_for_transcript(&media, asset.id);
    let silences = wait_for_silences(&media, asset.id);
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
    let agent_media: Arc<dyn MediaEngine> = media.clone();
    let server = McpServer::start(core.clone(), agent_media).expect("MCP server should start");
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

    let deadline = Instant::now() + Duration::from_secs(300);
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

fn test_engine() -> FfmpegMediaEngine {
    std::env::var_os("OPENREEL_AGENT_TEST_DATA_DIR").map_or_else(
        || FfmpegMediaEngine::new().expect("media engine should start"),
        |path| {
            FfmpegMediaEngine::new_with_data_dir(PathBuf::from(path))
                .expect("media engine should start")
        },
    )
}

fn wait_for_transcript(engine: &FfmpegMediaEngine, asset: AssetId) -> Arc<AssetTranscript> {
    let deadline = Instant::now() + Duration::from_secs(1_200);
    loop {
        match engine.transcript_status(asset) {
            TranscriptStatus::Ready(transcript) => return transcript,
            TranscriptStatus::NoSpeech => panic!("Whisper reported no speech for SAPI audio"),
            TranscriptStatus::Failed(error) => panic!("transcription failed: {error}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "transcription timed out");
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_silences(
    engine: &FfmpegMediaEngine,
    asset: AssetId,
) -> Arc<openreel_core::AssetSilences> {
    let deadline = Instant::now() + Duration::from_secs(60);
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

fn joined_words(transcript: &AssetTranscript) -> String {
    transcript
        .words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphabetic())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn single_clip_document(asset: openreel_core::MediaAsset) -> Document {
    let duration = asset.duration;
    let fps = asset.fps;
    let resolution = asset.resolution.unwrap_or((320, 180));
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..duration,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
            }],
        }],
        media_pool: vec![asset],
        fps,
        resolution,
        duration,
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

struct SpeechClip {
    wav: PathBuf,
    mp4: PathBuf,
}

impl SpeechClip {
    fn generate() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let stem = format!("openreel-agent-silence-{}-{nonce}", std::process::id());
        let wav = std::env::temp_dir().join(format!("{stem}.wav"));
        let mp4 = std::env::temp_dir().join(format!("{stem}.mp4"));
        if let Some(fixture) = std::env::var_os("OPENREEL_AGENT_TEST_TTS_WAV") {
            std::fs::copy(&fixture, &wav).unwrap_or_else(|error| {
                panic!(
                    "could not copy OPENREEL_AGENT_TEST_TTS_WAV {}: {error}",
                    PathBuf::from(fixture).display()
                )
            });
        } else {
            synthesize_speech(&wav);
        }
        mux_speech(&wav, &mp4);
        Self { wav, mp4 }
    }
}

impl Drop for SpeechClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.wav);
        let _ = std::fs::remove_file(&self.mp4);
    }
}

fn synthesize_speech(output: &Path) {
    let script = concat!(
        "$ErrorActionPreference = 'Stop'; ",
        "Add-Type -AssemblyName System.Speech; ",
        "$voice = New-Object System.Speech.Synthesis.SpeechSynthesizer; ",
        "$voice.SetOutputToWaveFile($env:OPENREEL_SAPI_OUTPUT); ",
        "$voice.SpeakSsml($env:OPENREEL_SAPI_SSML); ",
        "$voice.Dispose()"
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("OPENREEL_SAPI_OUTPUT", output)
        .env("OPENREEL_SAPI_SSML", TTS_SSML)
        .status()
        .expect("Windows PowerShell with System.Speech is required");
    assert!(status.success(), "SAPI speech synthesis failed");
}

fn mux_speech(wav: &Path, output: &Path) {
    let status = Command::new(ffmpeg_executable())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=navy:size=320x180:rate=30",
            "-i",
        ])
        .arg(wav)
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(output)
        .status()
        .expect("failed to run provisioned ffmpeg.exe");
    assert!(status.success(), "speech/video mux failed");
}

fn ffmpeg_executable() -> PathBuf {
    std::env::var_os("FFMPEG_DIR")
        .map(PathBuf::from)
        .expect("FFMPEG_DIR must point at the provisioned FFmpeg directory")
        .join("bin")
        .join("ffmpeg.exe")
}
