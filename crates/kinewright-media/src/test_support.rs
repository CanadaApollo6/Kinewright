use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use kinewright_core::{
    Analysis, AssetTranscript, Clip, ClipId, Document, MediaAsset, TimeCode, Track, TrackId,
    TrackKind, TranscriptStatus,
};

use crate::FfmpegMediaEngine;

pub struct TempDirectory(PathBuf);

impl TempDirectory {
    /// Create a uniquely named temporary directory for one test fixture.
    ///
    /// # Panics
    ///
    /// Panics if the operating system cannot create the directory.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(unique_stem(label));
        fs::create_dir_all(&path).expect("temporary test directory should be created");
        Self(path)
    }

    #[must_use]
    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct GeneratedMedia(PathBuf);

impl GeneratedMedia {
    #[must_use]
    pub fn ffmpeg(label: &str, arguments: &[&str], extension: &str) -> Self {
        let output = std::env::temp_dir().join(format!("{}.{}", unique_stem(label), extension));
        run_ffmpeg(arguments, &output);
        Self(output)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for GeneratedMedia {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub struct SpeechClip {
    owned_audio: Option<PathBuf>,
    pub mp4: PathBuf,
}

impl SpeechClip {
    #[must_use]
    pub fn plain(label: &str, text: &str, audio_override_env: &str) -> Self {
        Self::generate(label, text, false, audio_override_env, false)
    }

    #[must_use]
    pub fn ssml(label: &str, ssml: &str, audio_override_env: &str) -> Self {
        Self::generate(label, ssml, true, audio_override_env, true)
    }

    fn generate(
        label: &str,
        speech: &str,
        is_ssml: bool,
        audio_override_env: &str,
        copy_override: bool,
    ) -> Self {
        let stem = unique_stem(label);
        let wav = std::env::temp_dir().join(format!("{stem}.wav"));
        let mp4 = std::env::temp_dir().join(format!("{stem}.mp4"));
        let (audio, owned_audio) = std::env::var_os(audio_override_env).map_or_else(
            || {
                synthesize_speech(&wav, speech, is_ssml);
                (wav.clone(), Some(wav.clone()))
            },
            |fixture| {
                let fixture = PathBuf::from(fixture);
                if copy_override {
                    fs::copy(&fixture, &wav).unwrap_or_else(|error| {
                        panic!(
                            "could not copy {audio_override_env} {}: {error}",
                            fixture.display()
                        )
                    });
                    (wav.clone(), Some(wav.clone()))
                } else {
                    (fixture, None)
                }
            },
        );
        mux_speech(&audio, &mp4);
        Self { owned_audio, mp4 }
    }
}

impl Drop for SpeechClip {
    fn drop(&mut self) {
        if let Some(audio) = &self.owned_audio {
            let _ = fs::remove_file(audio);
        }
        let _ = fs::remove_file(&self.mp4);
    }
}

#[must_use]
/// Start a test engine using an optional cache-directory environment override.
///
/// # Panics
///
/// Panics if the media engine cannot initialize.
pub fn test_engine(data_dir_env: &str) -> FfmpegMediaEngine {
    std::env::var_os(data_dir_env).map_or_else(
        || FfmpegMediaEngine::new().expect("media engine should start"),
        |path| {
            FfmpegMediaEngine::new_with_data_dir(PathBuf::from(path))
                .expect("media engine should start")
        },
    )
}

#[must_use]
/// Wait for an asset transcript to finish and return it.
///
/// # Panics
///
/// Panics if transcription fails or does not finish within twenty minutes.
pub fn wait_for_transcript(
    engine: &dyn Analysis,
    asset: &MediaAsset,
    report_progress: bool,
) -> Arc<AssetTranscript> {
    let deadline = Instant::now() + Duration::from_mins(20);
    let mut last_status = String::new();
    loop {
        let status = engine.transcript_status(asset);
        if report_progress {
            let summary = format!("{status:?}");
            if summary != last_status {
                println!("TRANSCRIPTION: {summary}");
                last_status = summary;
            }
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

#[must_use]
pub fn joined_words(transcript: &AssetTranscript) -> String {
    transcript
        .words
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[must_use]
pub fn normalized_words(text: &str) -> Vec<String> {
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

#[must_use]
pub fn single_clip_document(asset: MediaAsset) -> Document {
    let duration = asset.duration;
    let fps = asset.fps;
    let resolution = asset.resolution.unwrap_or((320, 180));
    Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..duration,
                content: kinewright_core::ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps,
        resolution,
        duration,
    }
}

#[must_use]
fn ffmpeg_cli_filename(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_owned()
    }
}

/// Path to a provisioned `FFmpeg` CLI tool (`ffmpeg` or `ffprobe`).
#[must_use]
pub fn ffmpeg_tool(tool: &str) -> PathBuf {
    let filename = ffmpeg_cli_filename(tool);
    std::env::var_os("FFMPEG_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../third_party/ffmpeg/bin")
                .join(&filename)
        },
        |directory| PathBuf::from(directory).join("bin").join(&filename),
    )
}

#[must_use]
pub fn ffmpeg_executable() -> PathBuf {
    ffmpeg_tool("ffmpeg")
}

#[must_use]
pub fn ffprobe_executable() -> PathBuf {
    ffmpeg_tool("ffprobe")
}

/// Run the provisioned `FFmpeg` executable and require a successful exit.
///
/// # Panics
///
/// Panics if `FFmpeg` is missing, cannot start, or reports an error.
pub fn run_ffmpeg<S: AsRef<OsStr>>(arguments: &[S], output: &Path) {
    let ffmpeg = ffmpeg_executable();
    assert!(
        ffmpeg.is_file(),
        "provisioned ffmpeg CLI is missing at {}",
        ffmpeg.display()
    );
    let result = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(arguments)
        .arg(output)
        .output()
        .expect("failed to run provisioned ffmpeg CLI");
    assert!(
        result.status.success(),
        "media generation failed for {}: {}",
        output.display(),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn synthesize_speech(output: &Path, speech: &str, is_ssml: bool) {
    #[cfg(windows)]
    {
        let speak = if is_ssml {
            "$voice.SpeakSsml($env:KINEWRIGHT_SAPI_SPEECH); "
        } else {
            "$voice.Speak($env:KINEWRIGHT_SAPI_SPEECH); "
        };
        let script = format!(
            "$ErrorActionPreference = 'Stop'; Add-Type -AssemblyName System.Speech; \
             $voice = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             $voice.SetOutputToWaveFile($env:KINEWRIGHT_SAPI_OUTPUT); {speak}$voice.Dispose()"
        );
        let status = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .env("KINEWRIGHT_SAPI_OUTPUT", output)
            .env("KINEWRIGHT_SAPI_SPEECH", speech)
            .status()
            .expect("Windows PowerShell with System.Speech is required by this gated test");
        assert!(status.success(), "SAPI speech synthesis failed");
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("espeak-ng");
        command.arg("-w").arg(output);
        if is_ssml {
            command.arg("-m");
        }
        command.arg(speech);
        let status = command.status().expect(
            "espeak-ng is required for gated speech tests on this platform; install it or set the audio override env var",
        );
        assert!(status.success(), "espeak-ng speech synthesis failed");
    }
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
        .expect("failed to run provisioned ffmpeg CLI");
    assert!(status.success(), "speech/video mux failed");
}

fn unique_stem(label: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    format!("kinewright-{label}-{}-{nonce}", std::process::id())
}
