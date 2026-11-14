//! Source-clock acoustic evidence, independent of recognition and its model.
use std::path::Path;

use ffmpeg_next as ffmpeg;
use kinewright_core::dialogue::{SoundProfile, SourceRange, measure::ProfileBuilder};
use kinewright_core::{ExportCancellation, MediaError, MediaSourceFingerprint};
use serde::{Deserialize, Serialize};

use crate::derived_cache::JsonCache;

// Bump for any change to source sampling, hop arithmetic, detector or threshold.
const VERSION: u32 = 1;
const EARSHOT_VERSION: &str = "1.2.2";
const DETECTOR_RATE: u32 = 16_000;
const DETECTOR_SAMPLES: usize = 256;
const VOICE_THRESHOLD: f32 = 0.5;
const _: () = assert!(crate::recognizer::SAMPLE_RATE == DETECTOR_RATE);
fn measurement_contract() -> String {
    format!(
        "source-all-channels-h{}-earshot-{EARSHOT_VERSION}-r{DETECTOR_RATE}-n{DETECTOR_SAMPLES}-gt{:08x}",
        kinewright_core::dialogue::DEFAULT_HOP_MICROS,
        VOICE_THRESHOLD.to_bits()
    )
}

/// The stream selected by source decoding and, consequently, both renderers.
/// # Errors
/// Refuses inaccessible media and media without an audio stream.
pub fn audio_stream(path: &Path) -> Result<u32, MediaError> {
    crate::initialize_ffmpeg()?;
    let input = crate::decode::media_input(path)?;
    u32::try_from(crate::audio::selected_audio_stream(&input, path)?).map_err(error)
}

/// A detector failure never means measured absence of speech.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SoundMeasurement {
    Measured { profile: SoundProfile },
    NoVoiceInformation { reason: String },
}

/// Identity of one stream's acoustic evidence. Paths and recogniser settings
/// deliberately do not participate.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawIdentity")]
pub struct ProfileIdentity {
    sha256: String,
    bytes: u64,
    stream: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIdentity {
    sha256: String,
    bytes: u64,
    stream: u32,
}
impl TryFrom<RawIdentity> for ProfileIdentity {
    type Error = &'static str;
    fn try_from(raw: RawIdentity) -> Result<Self, Self::Error> {
        if raw.sha256.len() != 64
            || !raw
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid content hash");
        }
        Ok(Self {
            sha256: raw.sha256,
            bytes: raw.bytes,
            stream: raw.stream,
        })
    }
}
impl ProfileIdentity {
    /// External locators without a completed content hash have no evidence key.
    #[must_use]
    pub fn new(fingerprint: &MediaSourceFingerprint, stream: u32) -> Option<Self> {
        let sha256 = fingerprint.content_sha256.as_deref()?;
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        Some(Self {
            sha256: sha256.into(),
            bytes: fingerprint.byte_len?,
            stream,
        })
    }
    fn config_key(&self) -> String {
        format!("-{}-{}-{}", self.bytes, self.stream, measurement_contract())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    identity: ProfileIdentity,
    contract: String,
    measurement: SoundMeasurement,
}
/// Persistent source measurements beneath the application's private data root.
/// Reads validate both the key and Core's profile arithmetic.
pub struct SoundProfileCache {
    cache: JsonCache,
}
impl SoundProfileCache {
    /// # Errors
    /// Refuses an invalid application data root or an inaccessible cache.
    pub fn new(data_root: &Path) -> Result<Self, MediaError> {
        Ok(Self {
            cache: JsonCache::new(
                crate::derived::cache_directory(data_root)?
                    .child("sound-profiles")?
                    .child(&format!("v{VERSION}"))?,
                VERSION,
                "sound profile",
            ),
        })
    }
    /// # Errors
    /// Returns filesystem errors. Missing, corrupt or obsolete entries are misses.
    pub fn load(&self, identity: &ProfileIdentity) -> Result<Option<SoundMeasurement>, MediaError> {
        self.cache.load(
            &self
                .cache
                .path_for(&identity.sha256, &identity.config_key()),
            |entry: Stored| {
                (entry.identity == *identity && entry.contract == measurement_contract())
                    .then_some(entry.measurement)
            },
        )
    }
    pub(crate) fn save(
        &self,
        identity: ProfileIdentity,
        measurement: SoundMeasurement,
        cancel: &ExportCancellation,
    ) -> Result<(), MediaError> {
        check_cancel(cancel)?;
        let path = self
            .cache
            .path_for(&identity.sha256, &identity.config_key());
        self.cache.save(
            &path,
            &Stored {
                identity,
                contract: measurement_contract(),
                measurement,
            },
        )
    }
}
pub(crate) struct ProfileTarget {
    pub cache: SoundProfileCache,
    pub identity: ProfileIdentity,
}
impl ProfileTarget {
    pub fn save(
        self,
        measurement: SoundMeasurement,
        cancel: &ExportCancellation,
    ) -> Result<(), MediaError> {
        self.cache.save(self.identity, measurement, cancel)
    }
}
fn error(error: impl std::fmt::Display) -> MediaError {
    MediaError::Backend(format!("sound measurement failed: {error}"))
}
fn check_cancel(cancel: &ExportCancellation) -> Result<(), MediaError> {
    if cancel.is_cancelled() {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

/// Owns only hop output, detector intervals and one format-conversion frame.
/// The 256-sample detector carry never grows with recording length.
pub(crate) struct Measurement {
    levels: Option<ProfileBuilder>,
    converter: Option<ffmpeg::software::resampling::Context>,
    rate: u32,
    channels: u16,
    frames: u64,
    detector: Option<earshot::Detector>,
    unavailable: Option<String>,
    carry: [f32; DETECTOR_SAMPLES],
    used: usize,
    mono_samples: u64,
    intervals: Vec<SourceRange>,
}
impl Measurement {
    pub fn new() -> Self {
        Self {
            levels: None,
            converter: None,
            rate: 0,
            channels: 0,
            frames: 0,
            detector: Some(earshot::Detector::default()),
            unavailable: None,
            carry: [0.0; DETECTOR_SAMPLES],
            used: 0,
            mono_samples: 0,
            intervals: Vec::new(),
        }
    }
    pub fn source(&mut self, decoded: &ffmpeg::frame::Audio) -> Result<(), MediaError> {
        if self.levels.is_none() {
            self.rate = decoded.rate();
            self.channels = decoded.channels();
            self.levels = Some(ProfileBuilder::new(self.rate, self.channels).map_err(error)?);
            self.converter = Some(
                ffmpeg::software::resampling::Context::get(
                    decoded.format(),
                    decoded.channel_layout(),
                    self.rate,
                    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar),
                    decoded.channel_layout(),
                    self.rate,
                )
                .map_err(error)?,
            );
        }
        if decoded.rate() != self.rate || decoded.channels() != self.channels {
            return Err(error("source sample rate or channel count changed"));
        }
        let mut converted = ffmpeg::frame::Audio::empty();
        self.converter
            .as_mut()
            .expect("source converter")
            .run(decoded, &mut converted)
            .map_err(error)?;
        let mut interleaved = Vec::with_capacity(converted.samples() * usize::from(self.channels));
        for frame in 0..converted.samples() {
            for channel in 0..usize::from(self.channels) {
                interleaved.push(converted.plane::<f32>(channel)[frame]);
            }
        }
        self.levels
            .as_mut()
            .expect("source levels")
            .push(&interleaved)
            .map_err(error)?;
        self.frames = self
            .frames
            .checked_add(u64::try_from(converted.samples()).map_err(error)?)
            .ok_or_else(|| error("source clock overflow"))?;
        Ok(())
    }
    pub fn mono(&mut self, samples: &[f32]) {
        if self.detector.is_none() {
            return;
        }
        for &sample in samples {
            if !sample.is_finite() {
                self.detector = None;
                self.unavailable = Some("detector input was not finite".into());
                return;
            }
            self.carry[self.used] = sample;
            self.used += 1;
            self.mono_samples += 1;
            if self.used == DETECTOR_SAMPLES {
                self.detect();
                if self.detector.is_none() {
                    return;
                }
            }
        }
    }
    fn detect(&mut self) {
        let Some(detector) = self.detector.as_mut() else {
            return;
        };
        // Panics use the transcript job's existing boundary. A failed score
        // becomes explicit missing voice information, never measured quiet.
        let score = detector.predict_f32(&self.carry);
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            self.detector = None;
            self.unavailable = Some("Earshot detector failed".into());
            self.used = 0;
            return;
        }
        if score > VOICE_THRESHOLD {
            let start_micros = (self.mono_samples
                - u64::try_from(self.used).expect("bounded detector frame"))
                * 1_000_000
                / u64::from(DETECTOR_RATE);
            let end_micros = (self.mono_samples * 1_000_000).div_ceil(u64::from(DETECTOR_RATE));
            if let Some(last) = self
                .intervals
                .last_mut()
                .filter(|last| last.end_micros == start_micros)
            {
                last.end_micros = end_micros;
            } else {
                self.intervals.push(SourceRange {
                    start_micros,
                    end_micros,
                });
            }
        }
        self.used = 0;
    }
    pub fn finish(mut self) -> Result<SoundMeasurement, MediaError> {
        if self.used > 0 {
            // Padding belongs only to the fixed detector input. The returned
            // voice interval is bounded by the actual sample count.
            self.carry[self.used..].fill(0.0);
            self.detect();
        }
        if let Some(reason) = self.unavailable {
            return Ok(SoundMeasurement::NoVoiceInformation { reason });
        }
        if self.detector.is_none() {
            return Ok(SoundMeasurement::NoVoiceInformation {
                reason: "Earshot is unavailable".into(),
            });
        }
        let duration = self
            .frames
            .checked_mul(1_000_000)
            .ok_or_else(|| error("duration overflow"))?
            .div_ceil(u64::from(self.rate.max(1)));
        for interval in &mut self.intervals {
            interval.end_micros = interval.end_micros.min(duration);
        }
        self.intervals
            .retain(|interval| interval.start_micros < interval.end_micros);
        let profile = self
            .levels
            .ok_or_else(|| error("empty audio stream"))?
            .finish(Some(&self.intervals))
            .map_err(error)?;
        Ok(SoundMeasurement::Measured { profile })
    }
}

/// Decode all source samples without retaining PCM, using the same mono path as
/// recognition. Used when a transcript exists but its independent profile does not.
pub(crate) fn measure_file(
    path: &Path,
    cancel: &ExportCancellation,
) -> Result<SoundMeasurement, MediaError> {
    let mut decoder = crate::audio::AudioDecoder::open(path, DETECTOR_RATE, 1, 0, u64::MAX)?;
    let mut measurement = Measurement::new();
    loop {
        check_cancel(cancel)?;
        if decoder.next_measured_chunk(&mut measurement)?.is_none() {
            break;
        }
    }
    check_cancel(cancel)?;
    measurement.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{GeneratedMedia, TempDirectory, wav_f32};
    fn profile(measurement: SoundMeasurement) -> SoundProfile {
        match measurement {
            SoundMeasurement::Measured { profile } => profile,
            other @ SoundMeasurement::NoVoiceInformation { .. } => {
                panic!("expected measured voice information: {other:?}")
            }
        }
    }
    #[test]
    fn dq_media_levels_use_source_rate_and_every_channel() {
        crate::initialize_ffmpeg().unwrap();
        // A non-16k stereo stream with an out-of-phase pair would disappear
        // in the recogniser's mono signal. Its native levels must not disappear.
        let samples: Vec<f32> = (0..882).flat_map(|_| [0.25, -0.25]).collect();
        let source = GeneratedMedia::from_bytes(
            "dq-media-source-levels",
            "wav",
            &wav_f32(&samples, 44_100, 2),
        );
        let measured =
            profile(measure_file(source.path(), &ExportCancellation::default()).unwrap());
        assert_eq!(measured.duration_micros(), 20_000);
        assert_eq!(measured.len(), 2);
        for hop in measured.hops() {
            assert!(
                (hop.peak_dbfs_hundredths + 1204).abs() <= 1,
                "native channel peak"
            );
            assert!(
                (hop.rms_dbfs_hundredths + 1204).abs() <= 1,
                "native channel RMS"
            );
        }
    }
    #[test]
    fn dq_media_measurement_keeps_recognizer_pcm_unchanged() {
        crate::initialize_ffmpeg().unwrap();
        let samples: Vec<f32> = (0..4801)
            .map(|i| if i % 17 < 8 { 0.125 } else { -0.125 })
            .collect();
        let source =
            GeneratedMedia::from_bytes("dq-media-tee", "wav", &wav_f32(&samples, 48_000, 1));
        let mut plain =
            crate::audio::AudioDecoder::open(source.path(), 16_000, 1, 0, u64::MAX).unwrap();
        let mut tee =
            crate::audio::AudioDecoder::open(source.path(), 16_000, 1, 0, u64::MAX).unwrap();
        let mut measurement = Measurement::new();
        loop {
            let expected = plain.next_chunk().unwrap();
            let actual = tee.next_measured_chunk(&mut measurement).unwrap();
            assert_eq!(
                actual, expected,
                "measurement cannot change recognizer input"
            );
            if actual.is_none() {
                break;
            }
        }
        let measured = profile(measurement.finish().unwrap());
        assert_eq!(
            measured.duration_micros(),
            100_021,
            "last native sample retained"
        );
        assert_eq!(measured.len(), 11, "partial final hop retained");
    }
    #[test]
    fn dq_media_pinned_detector_finds_voice_and_quiet_in_committed_speech() {
        crate::initialize_ffmpeg().unwrap();
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ami-es2002a-90-102.wav");
        let measured = profile(measure_file(&path, &ExportCancellation::default()).unwrap());
        assert_eq!(measured.duration_micros(), 12_000_000);
        assert!(
            measured.hops().any(|hop| hop.voice),
            "speech must reach the detector"
        );
        assert!(
            measured.hops().any(|hop| !hop.voice),
            "quiet must remain distinguishable"
        );
    }
    #[test]
    fn dq_media_voice_unavailable_is_not_measured_no_voice() {
        let mut unavailable = Measurement::new();
        unavailable.detector = None;
        assert!(matches!(
            unavailable.finish().unwrap(),
            SoundMeasurement::NoVoiceInformation { .. }
        ));
        let mut failed = Measurement::new();
        failed.mono(&[f32::NAN]);
        assert!(matches!(
            failed.finish().unwrap(),
            SoundMeasurement::NoVoiceInformation { .. }
        ));
        crate::initialize_ffmpeg().unwrap();
        let source = GeneratedMedia::from_bytes(
            "dq-media-no-voice",
            "wav",
            &wav_f32(&vec![0.0; 481], 16_000, 1),
        );
        let silence = profile(measure_file(source.path(), &ExportCancellation::default()).unwrap());
        assert!(silence.hops().all(|hop| !hop.voice));
        assert_eq!(silence.duration_micros(), 30_063);
    }
    #[test]
    fn dq_media_detector_dependency_matches_its_cache_version() {
        // Upgrading the dependency without changing the evidence identity is
        // an invalid state even when its Rust interface remains compatible.
        let pin = format!("earshot = \"={EARSHOT_VERSION}\"");
        assert!(
            include_str!("../Cargo.toml")
                .lines()
                .any(|line| line.trim() == pin)
        );
    }
    #[test]
    fn dq_media_cache_is_content_stream_and_contract_scoped_without_a_model() {
        let root = TempDirectory::new("dq-media-profile-cache");
        let cache = SoundProfileCache::new(root.root()).unwrap();
        let fingerprint = MediaSourceFingerprint {
            content_sha256: Some("a".repeat(64)),
            byte_len: Some(123),
        };
        let key = ProfileIdentity::new(&fingerprint, 1).unwrap();
        assert!(cache.load(&key).unwrap().is_none());
        let profile =
            SoundProfile::from_raw(10_000, 10_000, vec![-600], vec![-900], vec![true]).unwrap();
        cache
            .save(
                key,
                SoundMeasurement::Measured { profile },
                &ExportCancellation::default(),
            )
            .unwrap();
        let same = ProfileIdentity::new(&fingerprint, 1).unwrap();
        assert!(matches!(
            cache.load(&same).unwrap(),
            Some(SoundMeasurement::Measured { .. })
        ));
        assert!(
            cache
                .load(&ProfileIdentity::new(&fingerprint, 2).unwrap())
                .unwrap()
                .is_none()
        );
        let other = MediaSourceFingerprint {
            content_sha256: Some("b".repeat(64)),
            byte_len: Some(123),
        };
        assert!(
            cache
                .load(&ProfileIdentity::new(&other, 1).unwrap())
                .unwrap()
                .is_none()
        );
        let other = MediaSourceFingerprint {
            content_sha256: Some("a".repeat(64)),
            byte_len: Some(124),
        };
        assert!(
            cache
                .load(&ProfileIdentity::new(&other, 1).unwrap())
                .unwrap()
                .is_none()
        );
        let path = cache.cache.path_for(&same.sha256, &same.config_key());
        let raw = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&measurement_contract(), "obsolete-detector");
        std::fs::write(&path, raw).unwrap();
        assert!(cache.load(&same).unwrap().is_none());
        assert!(ProfileIdentity::new(&MediaSourceFingerprint::unknown(), 1).is_none());
    }
    #[test]
    fn dq_media_cancelled_measurement_publishes_no_cache_entry() {
        let root = TempDirectory::new("dq-media-profile-cancel");
        let cache = SoundProfileCache::new(root.root()).unwrap();
        let fingerprint = MediaSourceFingerprint {
            content_sha256: Some("c".repeat(64)),
            byte_len: Some(123),
        };
        let cancel = ExportCancellation::default();
        cancel.cancel();
        assert_eq!(
            cache.save(
                ProfileIdentity::new(&fingerprint, 0).unwrap(),
                SoundMeasurement::NoVoiceInformation {
                    reason: "test".into()
                },
                &cancel
            ),
            Err(MediaError::Cancelled)
        );
        assert!(
            cache
                .load(&ProfileIdentity::new(&fingerprint, 0).unwrap())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    #[ignore = "on-demand: explicit KINEWRIGHT_MEASUREMENT_AUDIO and KINEWRIGHT_MEASUREMENT_OUTPUT paths"]
    fn dq_media_measurement_file_benchmark() {
        crate::initialize_ffmpeg().unwrap();
        let input = std::env::var_os("KINEWRIGHT_MEASUREMENT_AUDIO").expect("explicit input");
        let output = std::env::var_os("KINEWRIGHT_MEASUREMENT_OUTPUT").expect("explicit output");
        let start = std::time::Instant::now();
        let measurement = measure_file(Path::new(&input), &ExportCancellation::default()).unwrap();
        let elapsed = start.elapsed();
        let bytes = serde_json::to_vec(&measurement).unwrap();
        std::fs::write(output, &bytes).unwrap();
        println!(
            "DQ_MEDIA_MEASUREMENT elapsed={elapsed:?} serialized_bytes={}",
            bytes.len()
        );
    }
}

#[cfg(test)]
mod pass_comparison {
    use super::*;
    #[test]
    #[ignore = "on-demand: explicit KINEWRIGHT_MEASUREMENT_AUDIO and KINEWRIGHT_MEASUREMENT_OUTPUT paths"]
    fn dq_media_decode_pass_comparison() {
        crate::initialize_ffmpeg().unwrap();
        let input = std::env::var_os("KINEWRIGHT_MEASUREMENT_AUDIO").expect("explicit input");
        let output = std::env::var_os("KINEWRIGHT_MEASUREMENT_OUTPUT").expect("explicit output");
        let mut times = [Vec::new(), Vec::new()];
        for trial in 0..5 {
            let mut baseline = None;
            for passes in 1..=2 {
                let start = std::time::Instant::now();
                let measurement = if passes == 1 {
                    measure_file(Path::new(&input), &ExportCancellation::default()).unwrap()
                } else {
                    let mut measurement = Measurement::new();
                    let mut levels =
                        crate::audio::AudioDecoder::open(Path::new(&input), 16_000, 1, 0, u64::MAX)
                            .unwrap();
                    while levels
                        .next_chunk_with(&mut |frame| measurement.source(frame))
                        .unwrap()
                        .is_some()
                    {}
                    let mut mono =
                        crate::audio::AudioDecoder::open(Path::new(&input), 16_000, 1, 0, u64::MAX)
                            .unwrap();
                    while let Some(chunk) = mono.next_chunk().unwrap() {
                        measurement.mono(&chunk);
                    }
                    measurement.finish().unwrap()
                };
                let elapsed = start.elapsed().as_secs_f64();
                times[passes - 1].push(elapsed);
                if let Some(expected) = &baseline {
                    assert_eq!(
                        &measurement, expected,
                        "one and two passes must produce the same evidence"
                    );
                } else {
                    baseline = Some(measurement);
                }
                println!("DQ_MEDIA_PASSES trial={trial} passes={passes} seconds={elapsed}");
            }
        }
        std::fs::write(output, serde_json::to_vec(&times).unwrap()).unwrap();
    }
}

#[cfg(test)]
mod recognition_benchmark {
    use super::*;
    #[derive(Serialize)]
    struct Completed {
        measurement: SoundMeasurement,
        recognition: crate::recognizer::Recognition,
    }

    #[test]
    #[ignore = "on-demand: pinned EN model plus explicit KINEWRIGHT_MEASUREMENT_AUDIO and KINEWRIGHT_MEASUREMENT_OUTPUT paths"]
    fn dq_media_recognition_and_measurement_benchmark() {
        use crate::recognizer::{BuiltinRecognizer, ENGLISH_MODEL, queue};
        crate::initialize_ffmpeg().unwrap();
        let input = std::env::var_os("KINEWRIGHT_MEASUREMENT_AUDIO").expect("explicit input");
        let output = std::env::var_os("KINEWRIGHT_MEASUREMENT_OUTPUT").expect("explicit output");
        let root =
            std::env::var_os("KINEWRIGHT_ASR_MODEL_DIR").expect("explicit pinned model directory");
        let receipts = crate::test_support::TempDirectory::new("dq-media-real-receipts");
        let cancel = ExportCancellation::default();
        let start = std::time::Instant::now();
        let mut model = BuiltinRecognizer::load(
            &Path::new(&root).join(ENGLISH_MODEL.files[0].name),
            receipts.root(),
            &cancel,
        )
        .unwrap();
        let mut decoder =
            crate::audio::AudioDecoder::open(Path::new(&input), 16_000, 1, 0, u64::MAX).unwrap();
        let mut measurement = Measurement::new();
        let recognition = queue::drive(
            &mut model,
            &cancel,
            |window| {
                if let Some(chunk) = decoder.next_measured_chunk(&mut measurement)? {
                    window.extend(chunk);
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
            |_| {},
            true,
        )
        .unwrap();
        let measurement = measurement.finish().unwrap();
        let elapsed = start.elapsed();
        println!(
            "DQ_MEDIA_RECOGNITION_MEASUREMENT elapsed={elapsed:?} words={}",
            recognition.words.len()
        );
        std::fs::write(
            output,
            serde_json::to_vec(&Completed {
                measurement,
                recognition,
            })
            .unwrap(),
        )
        .unwrap();
    }
}
