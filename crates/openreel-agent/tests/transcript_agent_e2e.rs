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
    Document, Event, MediaEngine, Query, QueryResult, SessionConfig, TimeCode, Track, TrackId,
    TrackKind, TranscriptStatus,
};
use openreel_media::FfmpegMediaEngine;

const TTS_TEXT: &str = "Hello, um, this is an Open Reel transcript test.";

#[test]
fn one_agent_message_removes_the_transcribed_filler_word() {
    if std::env::var("OPENREEL_TRANSCRIPT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_TRANSCRIPT_TEST=1 to run SAPI, Whisper, and Claude");
        return;
    }

    let clip = SpeechClip::generate();
    let media = Arc::new(test_engine());
    let asset = media.probe(&clip.mp4).expect("generated clip should probe");
    media.request_transcription(asset.clone());
    let transcript = wait_for_transcript(&media, asset.id);
    let whisper_output = joined_asset_words(&transcript);
    let whisper_words = normalized_words(&whisper_output);
    assert!(
        whisper_words.iter().any(|word| word == "um"),
        "fixture must contain a recognized filler word: {whisper_output}"
    );

    let original = single_clip_document(asset);
    media.set_document(Arc::new(original.clone()));
    let core = Core::spawn(original).expect("core should start");
    let agent_media: Arc<dyn MediaEngine> = media.clone();
    let server = McpServer::start(core.clone(), agent_media).expect("MCP server should start");
    let confirmations = server.confirmations();
    let mut session = ClaudeCodeDriver
        .start_session(SessionConfig {
            working_directory: std::env::current_dir().ok(),
            model: None,
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

    let deadline = Instant::now() + Duration::from_secs(300);
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

fn test_engine() -> FfmpegMediaEngine {
    std::env::var_os("OPENREEL_TRANSCRIPT_TEST_DATA_DIR").map_or_else(
        || FfmpegMediaEngine::new().expect("media engine should start"),
        |path| {
            FfmpegMediaEngine::new_with_data_dir(PathBuf::from(path))
                .expect("media engine should start")
        },
    )
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

fn joined_asset_words(transcript: &AssetTranscript) -> String {
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

struct SpeechClip {
    owned_audio: Option<PathBuf>,
    mp4: PathBuf,
}

impl SpeechClip {
    fn generate() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let stem = format!("openreel-agent-transcript-{}-{nonce}", std::process::id());
        let wav = std::env::temp_dir().join(format!("{stem}.wav"));
        let mp4 = std::env::temp_dir().join(format!("{stem}.mp4"));
        let (audio, owned_audio) = std::env::var_os("OPENREEL_TRANSCRIPT_AUDIO").map_or_else(
            || {
                synthesize_speech(&wav);
                (wav.clone(), Some(wav))
            },
            |path| (PathBuf::from(path), None),
        );
        mux_speech(&audio, &mp4);
        Self { owned_audio, mp4 }
    }
}

impl Drop for SpeechClip {
    fn drop(&mut self) {
        if let Some(audio) = &self.owned_audio {
            let _ = std::fs::remove_file(audio);
        }
        let _ = std::fs::remove_file(&self.mp4);
    }
}

fn synthesize_speech(output: &Path) {
    let script = concat!(
        "$ErrorActionPreference = 'Stop'; ",
        "Add-Type -AssemblyName System.Speech; ",
        "$voice = New-Object System.Speech.Synthesis.SpeechSynthesizer; ",
        "$voice.SetOutputToWaveFile($env:OPENREEL_SAPI_OUTPUT); ",
        "$voice.Speak('Hello, um, this is an Open Reel transcript test.'); ",
        "$voice.Dispose()"
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("OPENREEL_SAPI_OUTPUT", output)
        .status()
        .expect("Windows PowerShell with System.Speech is required by this gated test");
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
