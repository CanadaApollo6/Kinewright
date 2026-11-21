//! Calibration of each check against generated encodes: every defect family
//! is seeded on its own beside a clean control (L7).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "synthetic test signals of a few seconds"
)]
use std::fs::File;

use kinewright_core::output_review::{
    AbsenceProof, AvLandmark, KeptWord, OverlayExpectations, ReviewCutIdentity, ReviewExpectations,
    ReviewInventory, ReviewLane, ReviewRowResult, ReviewSegment, ReviewSegmentId, ReviewSnapshotId,
    ReviewSpan, SegmentSampling, TranscriptIdentity,
};
use kinewright_core::{
    AssetId, ClipId, DeliveryEncodeDepth, DeliveryProfile, Document, ExactMicros,
    ExportCancellation, ExportSettings, Rational, TimeCode, TrackId,
};

use super::*;
use crate::recognizer::{self, Identity, Recognizer, Session, Update};
use crate::test_support::TempDirectory;

const WIDTH: usize = 128;
const HEIGHT: usize = 72;
const FRAMES: u64 = 120;
const RATE: u32 = 48_000;
const DURATION_US: u64 = 4_000_000;

fn us(value: u64) -> ExactMicros {
    ExactMicros::integer(value)
}

fn span(start: u64, end: u64) -> ReviewSpan {
    us(start)..us(end)
}

fn fps() -> Rational {
    Rational::new(30, 1).unwrap()
}

/// Output frames of a 30 fps picture in microseconds.
fn frame_us(frame: u64) -> u64 {
    frame * 1_000_000 / 30
}

// ---------------------------------------------------------------- picture

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picture {
    Clean,
    Flash,
    Black,
    Freeze,
}

/// Luma of `frame`: a moving ramp, brighter from 2 s (the sync landmark),
/// a two-frame bright cut insert at frames 30–31, authored black at 90–95
/// and an authored hold from frame 100.
fn luma(frame: u64, picture: Picture) -> Vec<u8> {
    let flat = |value: u8| vec![value; WIDTH * HEIGHT];
    let ramp = |frame: u64| {
        let offset = if frame >= 60 { 60 } else { 0 };
        (0..HEIGHT)
            .flat_map(|y| {
                (0..WIDTH).map(move |x| {
                    let phase = (x + y + 3 * usize::try_from(frame).unwrap()) % 128;
                    u8::try_from(64 + phase + offset).unwrap()
                })
            })
            .collect::<Vec<u8>>()
    };
    match (picture, frame) {
        (_, 30 | 31) => flat(220),
        (_, 90..=95) | (Picture::Black, 80..=85) => flat(16),
        (_, 100..) => ramp(100),
        (Picture::Flash, 15) => flat(235),
        (Picture::Freeze, 74..=89) => ramp(74),
        _ => ramp(frame),
    }
}

fn yuv(picture: Picture) -> Vec<u8> {
    let chroma = vec![128_u8; WIDTH * HEIGHT / 2];
    (0..FRAMES)
        .flat_map(|frame| {
            let mut plane = luma(frame, picture);
            plane.extend_from_slice(&chroma);
            plane
        })
        .collect()
}

// ---------------------------------------------------------------- sound

/// Tone-burst "words" on the output clock: (text, Hz, start µs, end µs).
const WORDS: [(&str, f64, u64, u64); 5] = [
    ("alpha", 300.0, 300_000, 600_000),
    ("bravo", 500.0, 900_000, 1_200_000),
    ("charlie", 700.0, 2_000_000, 2_300_000),
    ("delta", 900.0, 2_600_000, 2_900_000),
    ("echo", 1_100.0, 3_300_000, 3_600_000),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sound {
    Clean,
    Click,
    Missing,
    SilenceOverSpeech,
    OverPeak,
    DropWord,
    ClippedOnset,
}

fn sample_at(us: u64) -> usize {
    usize::try_from(us * u64::from(RATE) / 1_000_000).unwrap()
}

fn burst(pcm: &mut [f32], hz: f64, start: u64, end: u64, amplitude: f32, ramp_us: u64) {
    let (first, last) = (sample_at(start), sample_at(end));
    let ramp = sample_at(ramp_us).max(1);
    for (index, sample) in pcm.iter_mut().enumerate().take(last).skip(first) {
        let t = (index - first) as f64 / f64::from(RATE);
        let edge = (index - first).min(last - 1 - index);
        let gain = if edge >= ramp {
            1.0
        } else {
            0.5 - 0.5 * (std::f64::consts::PI * edge as f64 / ramp as f64).cos()
        };
        *sample += amplitude * (gain * (2.0 * std::f64::consts::PI * hz * t).sin()) as f32;
    }
}

fn pcm(sound: Sound) -> Vec<f32> {
    let mut pcm = vec![0.0_f32; sample_at(DURATION_US)];
    for (text, hz, start, end) in WORDS {
        match (sound, text) {
            (Sound::DropWord, "delta") => {}
            // The cut at 3 s lands 80 ms into the word: an abrupt onset.
            (Sound::ClippedOnset, "echo") => burst(&mut pcm, hz, 3_000_000, 3_220_000, 1.0, 0),
            _ => burst(&mut pcm, hz, start, end, 1.0, 10_000),
        }
    }
    // Normalise the clean programme to −14 LUFS before seeding.
    let mut meter = crate::loudness::LoudnessMeter::new(RATE, 1).unwrap();
    meter.push(&pcm).unwrap();
    let measured = meter.finish().unwrap().integrated_lufs_hundredths.unwrap();
    let gain = 10_f32.powf((-1_400 - measured) as f32 / 2_000.0);
    for sample in &mut pcm {
        *sample *= gain;
    }
    match sound {
        Sound::Click => pcm[sample_at(450_000)] += 0.6,
        Sound::Missing => pcm[sample_at(2_100_000)..sample_at(2_180_000)].fill(0.0),
        Sound::SilenceOverSpeech => pcm[sample_at(2_620_000)..sample_at(2_880_000)].fill(0.0),
        Sound::OverPeak => burst(&mut pcm, 1_000.0, 3_700_000, 3_720_000, 0.99, 5_000),
        _ => {}
    }
    pcm
}

// ---------------------------------------------------------------- encode

#[derive(Clone, Copy)]
struct Encode {
    picture: Picture,
    sound: Sound,
    audio_offset_ms: u32,
    frames: u64,
}

const CLEAN: Encode = Encode {
    picture: Picture::Clean,
    sound: Sound::Clean,
    audio_offset_ms: 0,
    frames: FRAMES,
};

struct Encoded {
    _directory: TempDirectory,
    path: std::path::PathBuf,
    timing: crate::ReviewOutputTiming,
}

impl Encoded {
    fn file(&self) -> File {
        File::open(&self.path).unwrap()
    }
}

fn encode(label: &str, plan: Encode) -> Encoded {
    crate::initialize_ffmpeg().unwrap();
    let directory = TempDirectory::new(label);
    let video = directory.path("picture.yuv");
    let audio = directory.path("sound.wav");
    std::fs::write(&video, yuv(plan.picture)).unwrap();
    std::fs::write(
        &audio,
        crate::test_support::wav_f32(&pcm(plan.sound), RATE, 1),
    )
    .unwrap();
    let path = directory.path("review.mp4");
    let size = format!("{WIDTH}x{HEIGHT}");
    let offset = format!("{:.3}", f64::from(plan.audio_offset_ms) / 1_000.0);
    let frames = plan.frames.to_string();
    let video_arg = video.to_string_lossy().into_owned();
    let audio_arg = audio.to_string_lossy().into_owned();
    crate::test_support::run_ffmpeg(
        &[
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-s",
            &size,
            "-r",
            "30",
            "-i",
            &video_arg,
            "-itsoffset",
            &offset,
            "-i",
            &audio_arg,
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-frames:v",
            &frames,
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "12",
            "-pix_fmt",
            "yuv420p",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-color_range",
            "tv",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
            "-movflags",
            "+faststart",
        ],
        &path,
    );
    let timing =
        crate::inspect_review_output(File::open(&path).unwrap(), &ExportCancellation::default())
            .unwrap();
    Encoded {
        _directory: directory,
        path,
        timing,
    }
}

// ---------------------------------------------------------------- expectations

fn segment_id(clip: u64, lane: ReviewLane, start_frame: i64) -> ReviewSegmentId {
    ReviewSegmentId {
        clip: ClipId(clip),
        lane,
        project_start: TimeCode(start_frame),
    }
}

fn video_segment(clip: u64, frames: std::ops::Range<u64>, hold: bool) -> ReviewSegment {
    let start = i64::try_from(frames.start).unwrap();
    let end = i64::try_from(frames.end).unwrap();
    ReviewSegment {
        id: segment_id(clip, ReviewLane::Video, start),
        track: TrackId(1),
        asset: AssetId(clip),
        source: span(frame_us(frames.start), frame_us(frames.end)),
        project: TimeCode(start)..TimeCode(end),
        output: span(frame_us(frames.start), frame_us(frames.end)),
        sampling: if hold {
            SegmentSampling::VideoHold {
                frame: TimeCode(start),
                source_fps: fps(),
            }
        } else {
            SegmentSampling::Video {
                clip_start: TimeCode(start),
                source_in: TimeCode(start),
                source_end: TimeCode(end),
                source_fps: fps(),
                effective_fps: fps(),
            }
        },
    }
}

fn audio_segment(clip: u64, output: (u64, u64), source_start: u64) -> ReviewSegment {
    let start = i64::try_from(output.0 * 30 / 1_000_000).unwrap();
    let end = i64::try_from(output.1 * 30 / 1_000_000).unwrap();
    ReviewSegment {
        id: segment_id(clip, ReviewLane::Audio, start),
        track: TrackId(2),
        asset: AssetId(50),
        source: span(source_start, source_start + output.1 - output.0),
        project: TimeCode(start)..TimeCode(end),
        output: span(output.0, output.1),
        sampling: SegmentSampling::Audio,
    }
}

fn inventory<T>(items: Vec<T>, absent: AbsenceProof) -> ReviewInventory<T> {
    let mut items = items.into_iter();
    match items.next() {
        Some(first) => ReviewInventory::Present {
            first,
            rest: items.collect(),
        },
        None => ReviewInventory::NoneProven(absent),
    }
}

fn transcript() -> TranscriptIdentity {
    TranscriptIdentity {
        asset: AssetId(50),
        source_sha256: [5; 32],
        source_byte_length: 1,
        audio_stream: 0,
        revision_sha256: [6; 32],
    }
}

/// Expectations of the authored cut every fixture renders.
fn expectations(sound: Sound) -> ReviewExpectations {
    let audio = [
        audio_segment(11, (0, 1_500_000), 0),
        audio_segment(12, (1_500_000, 3_000_000), 2_000_000),
        // The clipped render starts this source 80 ms later.
        audio_segment(
            13,
            (3_000_000, DURATION_US),
            if sound == Sound::ClippedOnset {
                4_080_000
            } else {
                4_000_000
            },
        ),
    ];
    let mut segments = vec![
        video_segment(1, 0..30, false),
        video_segment(2, 30..32, false),
        video_segment(3, 32..90, false),
        video_segment(4, 96..100, false),
        video_segment(5, 100..120, true),
    ];
    segments.extend(audio.iter().cloned());
    let words: Vec<KeptWord> = WORDS
        .iter()
        .enumerate()
        .map(|(index, &(text, _, start, end))| {
            let segment = match start {
                ..1_500_000 => &audio[0],
                1_500_000..3_000_000 => &audio[1],
                _ => &audio[2],
            };
            let (start, end) = if sound == Sound::ClippedOnset && text == "echo" {
                (3_000_000, 3_220_000)
            } else {
                (start, end)
            };
            KeptWord {
                transcript: transcript(),
                word_index: u32::try_from(index).unwrap(),
                segment: segment.id,
                text: text.into(),
                output: span(start, end),
                clipped: false,
            }
        })
        .collect();
    let speech = words.iter().map(|word| word.output.clone()).collect();
    ReviewExpectations {
        identity: ReviewCutIdentity {
            snapshot: ReviewSnapshotId([1; 32]),
            input_manifest_sha256: [2; 32],
            export_settings_sha256: [3; 32],
        },
        project_fps: fps(),
        output_fps: fps(),
        raster: (128, 72),
        output_frames: TimeCode(120),
        transcripts: vec![transcript()],
        segments,
        kept_words: inventory(words, AbsenceProof::NoRecognizedSpeechInKeptRanges),
        speech_windows: inventory(speech, AbsenceProof::NoRecognizedSpeechInKeptRanges),
        uncovered_speech_segments: Vec::new(),
        intended_pauses: ReviewInventory::NoneProven(AbsenceProof::NoAuthorPauseDeclarations),
        av_landmarks: inventory(
            vec![AvLandmark {
                asset: AssetId(50),
                source_landmark: 0,
                audio_segment: audio[1].id,
                video_segment: segment_id(3, ReviewLane::Video, 32),
                audio_at: us(2_000_000),
                video_at: us(2_000_000),
                uncertainty: ExactMicros::ZERO,
            }],
            AbsenceProof::NoPairedAudioVideo,
        ),
        overlays: OverlayExpectations {
            annotated_off_output: Vec::new(),
            captions: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
            titles: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
            cards: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
        },
    }
}

fn document() -> Document {
    let mut document = Document {
        fps: fps(),
        resolution: (128, 72),
        ..Document::default()
    };
    kinewright_core::apply_batch(
        &mut document,
        &[
            kinewright_core::Operation::AddTrack {
                track: kinewright_core::Track {
                    name: None,
                    id: TrackId(1),
                    kind: kinewright_core::TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                },
            },
            kinewright_core::Operation::AddSolidClip {
                track: TrackId(1),
                timeline_start: TimeCode::ZERO,
                duration: TimeCode(120),
                color: kinewright_core::SolidColor::default(),
            },
        ],
    )
    .unwrap();
    document
}

fn settings(document: &Document) -> ExportSettings {
    DeliveryProfile::SourceMaster.export_settings(
        document,
        DeliveryEncodeDepth::Eight,
        ExportCancellation::default(),
    )
}

fn scan(encoded: &Encoded, expectations: &ReviewExpectations) -> TechnicalReviewReport {
    let document = document();
    let settings = settings(&document);
    scan_review_artifact(
        encoded.file(),
        &ReviewScanInputs {
            timing: &encoded.timing,
            expectations,
            document: &document,
            settings: &settings,
        },
        &TechnicalScanConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap()
}

fn kinds(row: &ReviewRowReport) -> Vec<&FindingKind> {
    row.findings.iter().map(|finding| &finding.kind).collect()
}

fn assert_pass(row: &ReviewRowReport) {
    assert_eq!(row.result, ReviewRowResult::Pass, "{:?}", row.findings);
}

fn assert_only(row: &ReviewRowReport, expected: impl Fn(&FindingKind) -> bool) {
    assert_eq!(row.result, ReviewRowResult::Failed, "{:?}", row.findings);
    assert!(row.has(&expected), "{:?}", row.findings);
    assert!(
        row.findings.iter().all(|finding| expected(&finding.kind)),
        "unexpected findings {:?}",
        kinds(row)
    );
}

// ---------------------------------------------------------------- scan tests

#[test]
fn ja1_s5a_clean_encode_passes_with_authored_cut_black_and_hold() {
    let encoded = encode("ja1-s5-scan-clean", CLEAN);
    let report = scan(&encoded, &expectations(Sound::Clean));
    assert_pass(&report.structure);
    assert_pass(&report.loudness);
    assert_pass(&report.defects);
    assert_eq!(report.decoded_frames, FRAMES);
    assert_eq!(
        report.configuration_sha256,
        TechnicalScanConfig::default().sha256()
    );
    let measured = report.measured.unwrap();
    assert!(
        (measured.integrated_lufs_hundredths.unwrap() + 1_400).abs() <= 50,
        "{measured:?}"
    );
}

#[test]
fn ja1_s5a_picture_defects_are_found_one_family_at_a_time() {
    for (picture, kind) in [
        (Picture::Flash, FindingKind::Flash),
        (Picture::Black, FindingKind::Black),
        (Picture::Freeze, FindingKind::Freeze),
    ] {
        let encoded = encode("ja1-s5-scan-picture", Encode { picture, ..CLEAN });
        let report = scan(&encoded, &expectations(Sound::Clean));
        assert_only(&report.defects, |found| *found == kind);
        assert_pass(&report.structure);
        assert_pass(&report.loudness);
    }
}

#[test]
fn ja1_s5a_sound_defects_are_found_and_intended_pauses_excuse_them() {
    for (sound, kind, at) in [
        (Sound::Click, FindingKind::Click, (440_000, 470_000)),
        (Sound::Missing, FindingKind::Dropout, (2_080_000, 2_200_000)),
        (
            Sound::SilenceOverSpeech,
            FindingKind::SilenceOverSpeech,
            (2_600_000, 2_900_000),
        ),
    ] {
        let encoded = encode("ja1-s5-scan-sound", Encode { sound, ..CLEAN });
        let mut authored = expectations(Sound::Clean);
        let report = scan(&encoded, &authored);
        // A seeded hole also cuts the tone abruptly; its edges may click.
        assert_eq!(report.defects.result, ReviewRowResult::Failed);
        assert!(
            report.defects.has(|found| *found == kind),
            "{:?}",
            report.defects
        );
        assert!(
            report
                .defects
                .has(|found| *found == kind || *found == FindingKind::Click),
            "{:?}",
            report.defects
        );
        assert_pass(&report.structure);
        // The same encode, with the span declared an intended pause.
        authored.intended_pauses = inventory(
            vec![span(at.0, at.1)],
            AbsenceProof::NoAuthorPauseDeclarations,
        );
        let excused = scan(&encoded, &authored);
        assert_pass(&excused.defects);
    }
}

#[test]
fn ja1_s5a_structure_and_loudness_seeds_fail_independently() {
    let late = encode(
        "ja1-s5-scan-late",
        Encode {
            audio_offset_ms: 300,
            ..CLEAN
        },
    );
    let report = scan(&late, &expectations(Sound::Clean));
    assert_eq!(report.structure.result, ReviewRowResult::Failed);
    assert!(
        report
            .structure
            .has(|kind| matches!(kind, FindingKind::AudioStartLate { .. }))
    );
    assert!(
        report
            .structure
            .has(|kind| matches!(kind, FindingKind::SyncOffset { .. }))
    );

    let short = encode(
        "ja1-s5-scan-short",
        Encode {
            frames: 110,
            ..CLEAN
        },
    );
    let report = scan(&short, &expectations(Sound::Clean));
    assert!(report.structure.has(|kind| *kind
        == FindingKind::FrameCount {
            expected: FRAMES,
            decoded: 110
        }));
    assert_pass(&report.loudness);

    let loud = encode(
        "ja1-s5-scan-peak",
        Encode {
            sound: Sound::OverPeak,
            ..CLEAN
        },
    );
    let report = scan(&loud, &expectations(Sound::Clean));
    assert_only(&report.loudness, |kind| {
        matches!(kind, FindingKind::TruePeak { .. })
    });
    assert_pass(&report.structure);
    assert_pass(&report.defects);
}

#[test]
fn ja1_s5a_undecidable_scan_inputs_block_instead_of_passing() {
    let encoded = encode("ja1-s5-scan-undecided", CLEAN);
    let mut authored = expectations(Sound::Clean);
    authored.av_landmarks = ReviewInventory::NotDetermined { known: Vec::new() };
    authored.speech_windows = ReviewInventory::NotDetermined { known: Vec::new() };
    let report = scan(&encoded, &authored);
    assert_eq!(report.structure.result, ReviewRowResult::Unknown);
    assert!(
        report
            .structure
            .has(|kind| *kind == FindingKind::LandmarksNotDetermined)
    );

    // Unreadable bytes: structure unknown, the other rows unrun.
    let directory = TempDirectory::new("ja1-s5-scan-garbage");
    let path = directory.path("review.mp4");
    std::fs::write(&path, b"not an mp4 at all").unwrap();
    let document = document();
    let settings = settings(&document);
    let report = scan_review_artifact(
        File::open(&path).unwrap(),
        &ReviewScanInputs {
            timing: &encoded.timing,
            expectations: &authored,
            document: &document,
            settings: &settings,
        },
        &TechnicalScanConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(report.structure.result, ReviewRowResult::Unknown);
    assert_eq!(report.loudness.result, ReviewRowResult::Unrun);
    assert_eq!(report.defects.result, ReviewRowResult::Unrun);
}

#[test]
fn ja1_s5a_silence_with_undetermined_speech_is_undecided() {
    let encoded = encode(
        "ja1-s5-scan-silence-undetermined",
        Encode {
            sound: Sound::SilenceOverSpeech,
            ..CLEAN
        },
    );
    let mut authored = expectations(Sound::Clean);
    authored.speech_windows = ReviewInventory::NotDetermined { known: Vec::new() };
    let report = scan(&encoded, &authored);
    assert!(
        report
            .defects
            .has(|kind| *kind == FindingKind::SilenceWithUndeterminedSpeech)
    );
    assert!(
        !report
            .defects
            .has(|kind| *kind == FindingKind::SilenceOverSpeech)
    );
}

// ---------------------------------------------------------------- speech

/// Test recogniser: a tone burst is a word, named by its frequency.
struct ToneWords {
    identity: Identity,
}

struct ToneSession {
    origin_us: u64,
    fed: u64,
    hop: Vec<f32>,
    /// Loud-hop run: start sample, hops, zero crossings, last sample sign.
    run: Option<(u64, u64, u64)>,
    previous: f32,
    ready: Vec<recognizer::Word>,
}

const HOP: usize = 160;

impl ToneSession {
    fn close(&mut self) {
        if let Some((start, hops, crossings)) = self.run.take()
            && hops >= 5
        {
            let seconds = (hops as f64 * HOP as f64) / 16_000.0;
            let hz = crossings as f64 / 2.0 / seconds;
            let text = WORDS
                .iter()
                .min_by(|a, b| (a.1 - hz).abs().total_cmp(&(b.1 - hz).abs()))
                .unwrap()
                .0;
            let time = |sample: u64| self.origin_us + sample * 1_000_000 / 16_000;
            self.ready.push(recognizer::Word {
                text: text.into(),
                start_us: time(start),
                end_us: time(start + hops * HOP as u64),
            });
        }
    }

    fn analyse(&mut self) {
        let rms = (self.hop.iter().map(|s| s * s).sum::<f32>() / HOP as f32).sqrt();
        let mut crossings = 0;
        for &sample in &self.hop {
            if (sample >= 0.0) != (self.previous >= 0.0) {
                crossings += 1;
            }
            self.previous = sample;
        }
        let start = self.fed - HOP as u64;
        if rms > 0.02 {
            let run = self.run.get_or_insert((start, 0, 0));
            run.1 += 1;
            run.2 += crossings;
        } else {
            self.close();
        }
        self.hop.clear();
    }
}

impl Recognizer for ToneWords {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn supports_streaming(&self) -> bool {
        true
    }
    fn session(
        &mut self,
        origin_us: u64,
        _: ExportCancellation,
    ) -> Result<Box<dyn Session + '_>, recognizer::Error> {
        Ok(Box::new(ToneSession {
            origin_us,
            fed: 0,
            hop: Vec::with_capacity(HOP),
            run: None,
            previous: 0.0,
            ready: Vec::new(),
        }))
    }
}

impl Session for ToneSession {
    fn feed(&mut self, samples: &[f32]) -> Result<(), recognizer::Error> {
        for &sample in samples {
            self.hop.push(sample);
            self.fed += 1;
            if self.hop.len() == HOP {
                self.analyse();
            }
        }
        Ok(())
    }
    fn step(&mut self) -> Result<Option<Update>, recognizer::Error> {
        if self.ready.is_empty() {
            return Ok(None);
        }
        Ok(Some(Update::Committed {
            words: std::mem::take(&mut self.ready),
            audio_processed_us: self.origin_us + self.fed * 1_000_000 / 16_000,
        }))
    }
    fn commit_at(&mut self, _: u64) -> Result<(), recognizer::Error> {
        Ok(())
    }
    fn finish(&mut self) -> Result<(), recognizer::Error> {
        self.close();
        Ok(())
    }
}

fn recognise(encoded: &Encoded) -> Vec<RecognizedReviewWord> {
    let mut recognizer = ToneWords {
        identity: Identity::english(),
    };
    recognize_review_audio_with(
        &mut recognizer,
        encoded.file(),
        &encoded.timing,
        RetainedSpeechConfig::default().placement_tolerance_samples,
        &ExportCancellation::default(),
    )
    .unwrap()
}

fn retained(encoded: &Encoded, expectations: &ReviewExpectations) -> RetainedSpeechReport {
    let words = recognise(encoded);
    check_retained_speech(
        encoded.file(),
        &encoded.timing,
        expectations,
        &Ok(words),
        &RetainedSpeechConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap()
}

#[test]
fn ja1_s5a_retained_speech_clean_encode_matches_every_word_at_clean_joins() {
    let encoded = encode("ja1-s5-speech-clean", CLEAN);
    let words = recognise(&encoded);
    let texts: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
    assert_eq!(texts, ["alpha", "bravo", "charlie", "delta", "echo"]);
    let report = retained(&encoded, &expectations(Sound::Clean));
    assert_pass(&report.row);
    assert_eq!((report.expected_words, report.matched_words), (5, 5));
    assert!(report.joins_checked >= 2, "{report:?}");
}

#[test]
fn ja1_s5a_retained_speech_finds_a_dropped_word() {
    let encoded = encode(
        "ja1-s5-speech-dropped",
        Encode {
            sound: Sound::DropWord,
            ..CLEAN
        },
    );
    let report = retained(&encoded, &expectations(Sound::Clean));
    assert_only(&report.row, |kind| {
        matches!(kind, FindingKind::WordMissing { word_index: 3, .. })
    });
}

#[test]
fn ja1_s5a_join_audit_alone_catches_a_clipped_onset() {
    let encoded = encode(
        "ja1-s5-speech-clipped",
        Encode {
            sound: Sound::ClippedOnset,
            ..CLEAN
        },
    );
    let authored = expectations(Sound::ClippedOnset);
    let words = recognise(&encoded);
    assert!(words.iter().any(|word| word.text == "echo"), "{words:?}");
    let report = retained(&encoded, &authored);
    assert_only(&report.row, |kind| {
        matches!(kind, FindingKind::JoinInSound { .. })
    });
    assert_eq!(report.matched_words, 5);
}

#[test]
fn ja1_s5a_retained_speech_that_cannot_decide_blocks() {
    let encoded = encode("ja1-s5-speech-undecided", CLEAN);
    let authored = expectations(Sound::Clean);
    let unavailable = check_retained_speech(
        encoded.file(),
        &encoded.timing,
        &authored,
        &Err("no model".into()),
        &RetainedSpeechConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    assert_eq!(unavailable.row.result, ReviewRowResult::Unknown);
    let mut partial = authored.clone();
    partial.kept_words = ReviewInventory::NotDetermined {
        known: authored.kept_words.known().cloned().collect(),
    };
    let report = retained(&encoded, &partial);
    assert_eq!(report.row.result, ReviewRowResult::Unknown);
    assert!(
        report
            .row
            .has(|kind| *kind == FindingKind::SpeechNotDetermined)
    );
    let mut uncovered = authored;
    uncovered.uncovered_speech_segments = vec![segment_id(11, ReviewLane::Audio, 0)];
    let report = retained(&encoded, &uncovered);
    assert_eq!(report.row.result, ReviewRowResult::Unknown);
}
