use std::{
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use openreel_core::{MediaEngine, TranscriptStatus};
use openreel_media::FfmpegMediaEngine;

const TTS_TEXT: &str = "Hello, um, this is an Open Reel transcript test.";

#[test]
fn windows_sapi_speech_is_transcribed_by_the_real_model() {
    if std::env::var("OPENREEL_TRANSCRIPT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set OPENREEL_TRANSCRIPT_TEST=1 to run SAPI and local Whisper");
        return;
    }

    let clip = SpeechClip::generate();
    let engine = test_engine();
    let asset = engine
        .probe(&clip.mp4)
        .expect("generated clip should probe");
    engine.request_transcription(asset.clone());
    let transcript = wait_for_transcript(&engine, asset.id);
    let output = transcript
        .words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    println!("TTS: {TTS_TEXT}");
    println!("WHISPER: {output}");
    let normalized = normalized_words(&output);
    for expected in ["hello", "um", "this", "open", "transcript", "test"] {
        assert!(
            normalized.iter().any(|word| word == expected),
            "expected `{expected}` in Whisper output: {output}"
        );
    }
    for word in &transcript.words {
        assert!(
            word.source_start < word.source_end,
            "empty timestamp: {word:?}"
        );
        assert!(
            word.source_end <= asset.duration,
            "timestamp exceeds asset duration: {word:?}"
        );
    }
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

fn wait_for_transcript(
    engine: &FfmpegMediaEngine,
    asset: openreel_core::AssetId,
) -> std::sync::Arc<openreel_core::AssetTranscript> {
    let deadline = Instant::now() + Duration::from_secs(1_200);
    let mut last_status = String::new();
    loop {
        let status = engine.transcript_status(asset);
        let summary = format!("{status:?}");
        if summary != last_status {
            println!("TRANSCRIPTION: {summary}");
            last_status = summary;
        }
        match status {
            TranscriptStatus::Ready(transcript) => return transcript,
            TranscriptStatus::NoSpeech => panic!("Whisper reported no speech for SAPI audio"),
            TranscriptStatus::Failed(error) => panic!("transcription failed: {error}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "transcription timed out");
        thread::sleep(Duration::from_millis(100));
    }
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
        let stem = format!("openreel-transcript-{}-{nonce}", std::process::id());
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
    let ffmpeg = ffmpeg_executable();
    let status = Command::new(ffmpeg)
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
