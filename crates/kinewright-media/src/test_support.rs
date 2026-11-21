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

/// Durable fixture root with no parent-traversal components on either desktop OS.
///
/// # Panics
/// Panics if the crate is no longer inside this workspace's crates directory.
#[must_use]
pub fn workspace_target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("target")
}

#[test]
fn workspace_target_dir_is_absolute_without_parent_components() {
    let root = workspace_target_dir();
    assert!(root.is_absolute());
    assert!(
        !root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    );
}

impl TempDirectory {
    /// Create a uniquely named temporary directory for one test fixture.
    ///
    /// # Panics
    ///
    /// Panics if the operating system cannot create the directory.
    #[must_use]
    pub fn new(label: &str) -> Self {
        Self::new_in(&std::env::temp_dir(), label)
    }

    /// Create a fixture under a caller-selected filesystem (for durable storage tests).
    ///
    /// # Panics
    /// Panics if the fixture directory cannot be created.
    #[must_use]
    pub fn new_in(parent: &Path, label: &str) -> Self {
        let path = parent.join(unique_stem(label));
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

    /// Write a fixture straight to disk, for material a generator cannot
    /// express exactly (AU2 §3.9: a single-sample impulse).
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be written.
    #[must_use]
    pub fn from_bytes(label: &str, extension: &str, bytes: &[u8]) -> Self {
        let output = std::env::temp_dir().join(format!("{}.{}", unique_stem(label), extension));
        fs::write(&output, bytes)
            .unwrap_or_else(|error| panic!("could not write {}: {error}", output.display()));
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
pub fn test_engine(data_dir_env: &str) -> crate::SessionMedia {
    std::env::var_os(data_dir_env).map_or_else(
        || FfmpegMediaEngine::legacy_fixture().expect("media engine should start"),
        |path| {
            FfmpegMediaEngine::new_with_data_dir(PathBuf::from(path))
                .expect("media engine should start")
                .into_legacy_fixture()
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
            TranscriptStatus::NoSpeech => panic!("NeMo reported no speech for SAPI audio"),
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
        .map(kinewright_core::normalize_transcript_word)
        .filter(|word| !word.is_empty())
        .collect()
}

#[must_use]
pub fn single_clip_document(asset: MediaAsset) -> Document {
    let duration = asset.duration;
    let fps = asset.fps;
    let resolution = asset.resolution.unwrap_or((320, 180));
    Document {
        investigator: None,
        imported_from: Vec::new(),
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            name: None,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
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
                audio_gain_curve: None,
                blend_mode: kinewright_core::BlendMode::Normal,
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

/// Path to an isolated test-only `FFmpeg` CLI tool.
#[must_use]
pub fn ffmpeg_tool(tool: &str) -> PathBuf {
    let filename = ffmpeg_cli_filename(&format!("{tool}-testgen"));
    std::env::var_os("KINEWRIGHT_FFMPEG_TESTGEN_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../third_party/ffmpeg-testgen/bin")
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
        // Visual-scene fixtures refer to the bundled font from the workspace root.
        .current_dir(workspace_target_dir().parent().expect("workspace root"))
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
    // The wall clock alone is not unique: Windows reports `SystemTime` at a
    // coarse granularity, so two test threads creating a directory for the
    // same label in one tick got the same path, and the first to finish
    // removed the other's directory mid-export ("export directory does not
    // exist", the AU6 delivery fixtures on the Windows lane). A process-wide
    // counter makes every stem distinct regardless of the clock.
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "kinewright-{label}-{}-{nonce}-{sequence}",
        std::process::id()
    )
}

/// A mono tone of `frames` samples. **Mono**, and it returns a bare sample
/// vector — a stereo fixture sums mono vectors and interleaves itself.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn tone(frequency: f64, amplitude: f32, rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            let phase = 2.0 * std::f64::consts::PI * frequency * frame as f64 / f64::from(rate);
            #[allow(clippy::cast_possible_truncation)]
            let sample = (phase.sin() as f32) * amplitude;
            sample
        })
        .collect()
}

/// A deterministic xorshift64 buffer, never a negative zero.
///
/// Uniform on `[-a, a)` in exact binary steps and identical on every OS, so its
/// RMS is `a / sqrt(3)`. The first argument is a **sample** count, not a frame
/// count.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn pseudo_random_amplitude(frames: usize, amplitude: f32) -> Vec<f32> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state >> 40) as f32 / 8_388_608.0 - 1.0;
            unit * amplitude
        })
        .collect()
}

/// Root mean square over every sample of an interleaved buffer.
#[must_use]
pub fn rms(samples: &[f32]) -> f64 {
    let sum: f64 = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let count = samples.len().max(1) as f64;
    (sum / count).sqrt()
}

/// A 32-bit float WAV, written by hand so a single-sample impulse survives
/// exactly (AU2 §3.9(c)).
///
/// # Panics
///
/// Panics when the sample count does not fit a RIFF chunk length.
#[must_use]
pub fn wav_f32(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
    let data_length = u32::try_from(samples.len() * 4).expect("the fixture should fit");
    let mut bytes = Vec::with_capacity(44 + samples.len() * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&3_u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * u32::from(channels) * 4).to_le_bytes());
    bytes.extend_from_slice(&(channels * 4).to_le_bytes());
    bytes.extend_from_slice(&32_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// A generated talking head: a moving head whose mouth follows a voiced
/// speech envelope, the default look of the review-scan calibration source.
/// Every technical and retained-speech check can decide it.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "synthetic signals of a few seconds"
)]
pub mod talking_head {
    use std::path::Path;

    pub const WIDTH: usize = 128;
    pub const HEIGHT: usize = 72;
    pub const FPS: u32 = 30;
    pub const FRAMES: u64 = 180;
    const RATE: u32 = 48_000;
    /// Spoken words in source microseconds.
    pub const WORDS: [(&str, u64, u64); 8] = [
        ("so", 300_000, 600_000),
        ("today", 900_000, 1_300_000),
        ("we", 1_500_000, 1_750_000),
        ("talk", 2_200_000, 2_700_000),
        ("about", 3_300_000, 3_600_000),
        ("cuts", 3_900_000, 4_300_000),
        ("and", 4_500_000, 4_800_000),
        ("joins", 5_300_000, 5_700_000),
    ];

    fn noise(seed: u64) -> f32 {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
        x ^= x >> 29;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 32;
        (x % 20_001) as f32 / 10_000.0 - 1.0
    }

    fn envelope(t: f64) -> f64 {
        for (index, &(_, start, end)) in WORDS.iter().enumerate() {
            let (start, end) = (start as f64 / 1e6, end as f64 / 1e6);
            if t >= start && t < end {
                let length = end - start;
                let syllables = (length / 0.12).round().max(1.0);
                let syllable = length / syllables;
                let phase = ((t - start) % syllable) / syllable;
                let accent = 0.65 + 0.35 * ((index as f64) * 1.7 + (t - start) * 9.0).sin().abs();
                return (std::f64::consts::PI * phase).sin().powf(0.7) * accent;
            }
        }
        0.0
    }

    fn voice(n: usize) -> [f32; 2] {
        let t = n as f64 / f64::from(RATE);
        let f0 = 130.0 * (1.0 + 0.03 * (2.0 * std::f64::consts::PI * 5.0 * t).sin());
        let carrier: f64 = (1..=8)
            .map(|k| (2.0 * std::f64::consts::PI * f64::from(k) * f0 * t).sin() / f64::from(k))
            .sum();
        let mut value = 0.12 * envelope(t) * carrier;
        for &(_, start, _) in &WORDS {
            let first = (start * u64::from(RATE) / 1_000_000) as usize;
            if n >= first && n < first + 200 {
                value += 0.35 * f64::from(noise(n as u64 * 7 + 3));
            }
        }
        let room = |channel: u64| 0.0015 * noise(n as u64 * 2 + channel + 11);
        [value as f32 + room(0), value as f32 + room(1)]
    }

    fn heads() -> Vec<f64> {
        let mut position = 0.0_f64;
        let mut velocity = 0.0_f64;
        (0..FRAMES)
            .map(|frame| {
                velocity = 0.8 * velocity + 1.6 * f64::from(noise(frame * 13 + 5));
                position = (position + velocity).clamp(-28.0, 28.0);
                position
            })
            .collect()
    }

    fn luma(frame: u64, heads: &[f64]) -> Vec<u8> {
        let t = frame as f64 / f64::from(FPS);
        let mouth = (2.0 + 22.0 * envelope(t + 0.5 / f64::from(FPS))).round() as usize;
        let center = (64.0 + heads[frame as usize]).round() as i64;
        let mut plane = Vec::with_capacity(WIDTH * HEIGHT);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let mut value = 60.0 + ((x * 7 + y * 13) % 40) as f64;
                let dx = x as i64 - center;
                if (-22..22).contains(&dx) && (10..64).contains(&y) {
                    value = 150.0 + ((x * 3 + y * 5) % 24) as f64;
                    if (-14..14).contains(&dx) && y >= 44 && y < 44 + mouth.min(18) {
                        value = 36.0;
                    }
                }
                plane.push(value.round().clamp(16.0, 235.0) as u8);
            }
        }
        plane
    }

    /// Encode the talking head (H.264 picture, AAC stereo voice) to `path`.
    ///
    /// # Panics
    /// Panics when the scratch files cannot be written or the encoder fails.
    pub fn write(path: &Path) {
        let heads = heads();
        let mut picture = Vec::new();
        for frame in 0..FRAMES {
            picture.extend_from_slice(&luma(frame, &heads));
            picture.extend(std::iter::repeat_n(128_u8, WIDTH * HEIGHT / 2));
        }
        let samples = (FRAMES * u64::from(RATE) / u64::from(FPS)) as usize;
        let pcm: Vec<f32> = (0..samples).flat_map(voice).collect();
        let video = path.with_extension("yuv");
        let audio = path.with_extension("wav");
        std::fs::write(&video, picture).expect("raw picture");
        std::fs::write(&audio, super::wav_f32(&pcm, RATE, 2)).expect("raw audio");
        let size = format!("{WIDTH}x{HEIGHT}");
        let rate = FPS.to_string();
        let video_arg = video.to_string_lossy().into_owned();
        let audio_arg = audio.to_string_lossy().into_owned();
        super::run_ffmpeg(
            &[
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
                "-s",
                &size,
                "-r",
                &rate,
                "-i",
                &video_arg,
                "-i",
                &audio_arg,
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "10",
                "-pix_fmt",
                "yuv420p",
                "-x264-params",
                "colorprim=bt709:transfer=bt709:colormatrix=bt709",
                "-color_range",
                "tv",
                "-movflags",
                "+faststart",
            ],
            path,
        );
        let _ = std::fs::remove_file(video);
        let _ = std::fs::remove_file(audio);
    }
}

#[cfg(test)]
mod unique_stem_tests {
    use super::unique_stem;

    #[test]
    fn two_stems_for_one_label_never_collide_even_within_one_clock_tick() {
        let stems: std::collections::BTreeSet<String> =
            (0..64).map(|_| unique_stem("same-label")).collect();
        assert_eq!(stems.len(), 64, "every call yields a distinct stem");
    }
}
