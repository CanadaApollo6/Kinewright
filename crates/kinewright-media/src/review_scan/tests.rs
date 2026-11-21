//! Calibration of each check against generated media (L10–L12): a pinned
//! source talking head, a cut of it, and encodes rendered from the same
//! generators. Every defect family is seeded on its own beside clean
//! controls that ordinary footage produces (static tripod, dark room, fade
//! through black, plosives, a music bed, non-English speech, 10-bit, stereo).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    reason = "synthetic test signals of a few seconds; a look is a set of switches"
)]
use std::collections::BTreeMap;
use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use kinewright_core::output_review::{
    AbsenceProof, KeptWord, OverlayExpectations, ReviewCutIdentity, ReviewExpectations,
    ReviewInventory, ReviewLane, ReviewRowResult, ReviewSegment, ReviewSegmentId, ReviewSnapshotId,
    ReviewSpan, SegmentSampling, TranscriptIdentity,
};
use kinewright_core::{
    AssetId, AudioMix, BlendMode, Clip, ClipContent, ClipId, DeliveryEncodeDepth, DeliveryProfile,
    Document, ExactMicros, ExportCancellation, ExportSettings, LoudnessTarget, MediaAsset,
    Rational, TimeCode, Track, TrackId, TrackKind, TrackMix,
};

use super::*;
use crate::owned_io::{CHUNK_BYTES, CertifiedSource, LifetimeGuard};
use crate::test_support::{TempDirectory, run_ffmpeg, wav_f32};

const WIDTH: usize = 128;
const HEIGHT: usize = 72;
const RATE: u32 = 48_000;
const SOURCE_FRAMES: u64 = 180;
const SPEECH: AssetId = AssetId(1);
const MUSIC: AssetId = AssetId(2);
const SPEECH_TRACK: TrackId = TrackId(1);
const MUSIC_TRACK: TrackId = TrackId(2);
const INSERT_TRACK: TrackId = TrackId(3);
const SOLID_TRACK: TrackId = TrackId(4);
/// Source words in microseconds; cuts at 2 s, 3 s and 5 s fall in gaps.
const WORDS: [(u64, u64); 8] = [
    (300_000, 600_000),
    (900_000, 1_300_000),
    (1_500_000, 1_750_000),
    (2_200_000, 2_700_000),
    (3_300_000, 3_600_000),
    (3_900_000, 4_300_000),
    (4_500_000, 4_800_000),
    (5_300_000, 5_700_000),
];
const ENGLISH: [&str; 8] = ["so", "today", "we", "talk", "about", "cuts", "and", "joins"];
const SPANISH: [&str; 8] = [
    "hoy", "vamos", "a", "hablar", "de", "cortes", "y", "uniones",
];

fn fps() -> Rational {
    Rational::new(30, 1).unwrap()
}

fn us(value: u64) -> ExactMicros {
    ExactMicros::integer(value)
}

fn frame_us(frame: u64) -> u64 {
    frame * 1_000_000 / 30
}

fn sample_of_us(value: u64) -> usize {
    usize::try_from(value * u64::from(RATE) / 1_000_000).unwrap()
}

fn sample_of_frame(frame: u64) -> usize {
    usize::try_from(frame * u64::from(RATE) / 30).unwrap()
}

/// Deterministic noise in −1..1.
fn noise(seed: u64) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x % 20_001) as f32 / 10_000.0 - 1.0
}

// ---------------------------------------------------------------- the source

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Look {
    /// The head stays put; only the mouth moves.
    tripod: bool,
    dark: bool,
    /// The footage fades to black over frames 40..60 and up over 90..110.
    fade: bool,
    ten_bit: bool,
    /// A higher voice and other words: nothing about the check is English.
    spanish: bool,
}

/// Speech envelope of the source at `t` seconds: syllables inside words.
fn envelope(t: f64) -> f64 {
    for (index, &(start, end)) in WORDS.iter().enumerate() {
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

/// One stereo source sample of the talking head at sample index `n`.
fn voice(n: usize, look: Look) -> [f32; 2] {
    let t = n as f64 / f64::from(RATE);
    let pitch = if look.spanish { 205.0 } else { 130.0 };
    let f0 = pitch * (1.0 + 0.03 * (2.0 * std::f64::consts::PI * 5.0 * t).sin());
    let carrier: f64 = (1..=8)
        .map(|k| (2.0 * std::f64::consts::PI * f64::from(k) * f0 * t).sin() / f64::from(k))
        .sum();
    let mut value = 0.12 * envelope(t) * carrier;
    // Plosive: a short broadband burst at every word onset.
    for &(start, _) in &WORDS {
        let first = sample_of_us(start);
        if n >= first && n < first + 200 {
            value += 0.35 * f64::from(noise(n as u64 * 7 + 3));
        }
    }
    let room = |channel: u64| 0.0015 * noise(n as u64 * 2 + channel + 11);
    [value as f32 + room(0), value as f32 + room(1)]
}

/// A sustained chord with a slow swell, for the music bed asset.
fn music(n: usize) -> [f32; 2] {
    let t = n as f64 / f64::from(RATE);
    let swell = 0.7 + 0.3 * (2.0 * std::f64::consts::PI * 0.5 * t).sin();
    let chord: f64 = [220.0, 277.18, 329.63]
        .iter()
        .map(|hz| (2.0 * std::f64::consts::PI * hz * t).sin())
        .sum();
    let value = (0.05 * swell * chord) as f32;
    [value, value]
}

/// Head positions: a smoothed deterministic walk, never periodic.
fn head_positions() -> Vec<f64> {
    let mut position = 0.0_f64;
    let mut velocity = 0.0_f64;
    (0..SOURCE_FRAMES)
        .map(|frame| {
            velocity = 0.8 * velocity + 1.6 * f64::from(noise(frame * 13 + 5));
            position = (position + velocity).clamp(-28.0, 28.0);
            position
        })
        .collect()
}

/// 8-bit luma of source frame `frame`.
fn source_luma(frame: u64, look: Look, heads: &[f64]) -> Vec<u8> {
    let t = frame as f64 / 30.0;
    let bob = if look.tripod {
        0.0
    } else {
        heads[frame as usize]
    };
    let mouth = (2.0 + 22.0 * envelope(t + 0.5 / 30.0)).round() as usize;
    let center = (64.0 + bob).round() as i64;
    let fade = if !look.fade {
        1.0
    } else if (40..60).contains(&frame) {
        1.0 - (frame - 40) as f64 / 20.0
    } else if (90..110).contains(&frame) {
        (frame - 90) as f64 / 20.0
    } else if (60..90).contains(&frame) {
        0.0
    } else {
        1.0
    };
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
            if look.dark {
                value = 16.0 + (value - 16.0) * 0.15;
            }
            value = 16.0 + (value - 16.0) * fade;
            plane.push(value.round().clamp(16.0, 235.0) as u8);
        }
    }
    plane
}

/// Raw planar frames for the encoder, 8 or 10 bit.
fn raw_frames(frames: impl Iterator<Item = Vec<u8>>, ten_bit: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for luma in frames {
        if ten_bit {
            for value in &luma {
                bytes.extend_from_slice(&(u16::from(*value) << 2).to_le_bytes());
            }
            for _ in 0..WIDTH * HEIGHT / 2 {
                bytes.extend_from_slice(&(128_u16 << 2).to_le_bytes());
            }
        } else {
            bytes.extend_from_slice(&luma);
            bytes.extend(std::iter::repeat_n(128_u8, WIDTH * HEIGHT / 2));
        }
    }
    bytes
}

#[derive(Clone, Copy)]
struct EncodeOptions {
    ten_bit: bool,
    audio: bool,
    /// Map the audio twice: a second stream the cut never asked for.
    extra_audio: bool,
    /// Container offset of the audio input, in milliseconds.
    audio_offset_ms: i32,
}

const PLAIN: EncodeOptions = EncodeOptions {
    ten_bit: false,
    audio: true,
    extra_audio: false,
    audio_offset_ms: 0,
};

fn encode(
    directory: &TempDirectory,
    name: &str,
    picture: &[u8],
    pcm: &[f32],
    options: EncodeOptions,
) -> PathBuf {
    let video = directory.path(&format!("{name}.yuv"));
    let audio = directory.path(&format!("{name}.wav"));
    std::fs::write(&video, picture).unwrap();
    std::fs::write(&audio, wav_f32(pcm, RATE, 2)).unwrap();
    let path = directory.path(&format!("{name}.mp4"));
    let size = format!("{WIDTH}x{HEIGHT}");
    let pixel = if options.ten_bit {
        "yuv420p10le"
    } else {
        "yuv420p"
    };
    let offset = format!("{:.3}", f64::from(options.audio_offset_ms) / 1_000.0);
    let video_arg = video.to_string_lossy().into_owned();
    let audio_arg = audio.to_string_lossy().into_owned();
    let mut arguments: Vec<&str> = vec![
        "-f", "rawvideo", "-pix_fmt", pixel, "-s", &size, "-r", "30", "-i", &video_arg,
    ];
    if options.audio {
        arguments.extend([
            "-itsoffset",
            &offset,
            "-i",
            &audio_arg,
            "-map",
            "0:v",
            "-map",
            "1:a",
        ]);
        if options.extra_audio {
            arguments.extend(["-map", "1:a"]);
        }
        arguments.extend(["-c:a", "aac", "-b:a", "192k"]);
    } else {
        arguments.extend(["-map", "0:v"]);
    }
    arguments.extend([
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "10",
        "-pix_fmt",
        pixel,
        "-x264-params",
        "colorprim=bt709:transfer=bt709:colormatrix=bt709",
        "-color_range",
        "tv",
        "-movflags",
        "+faststart",
    ]);
    run_ffmpeg(&arguments, &path);
    path
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut hash = kinewright_hash::Sha256::new();
    hash.update(bytes);
    hash.finalize()
}

/// A pinned owned input for a generated file, verified end to end.
fn certify(path: &Path) -> CertifiedSource {
    let bytes = std::fs::read(path).unwrap();
    let chunks: Arc<[[u8; 32]]> = bytes
        .chunks(CHUNK_BYTES)
        .map(digest)
        .collect::<Vec<_>>()
        .into();
    let table: Vec<u8> = chunks.iter().flatten().copied().collect();
    let guard: LifetimeGuard = Arc::new(());
    CertifiedSource::verify(
        File::open(path).unwrap(),
        digest(&bytes),
        bytes.len() as u64,
        digest(&table),
        chunks,
        guard,
        Arc::new(|_| Ok(Arc::new(()) as LifetimeGuard)),
    )
    .unwrap()
}

struct Pinned(BTreeMap<AssetId, CertifiedSource>);

impl ReviewSources for Pinned {
    fn source(&self, asset: AssetId) -> Result<CertifiedSource, MediaError> {
        self.0
            .get(&asset)
            .cloned()
            .ok_or_else(|| MediaError::Backend("no pinned input".into()))
    }
}

/// The pinned sources of one look: the talking head and a music bed.
struct Sources {
    directory: TempDirectory,
    look: Look,
    heads: Vec<f64>,
    speech: MediaAsset,
    music: MediaAsset,
    pinned: Pinned,
}

fn asset(path: &Path, id: AssetId, certified: &CertifiedSource) -> MediaAsset {
    certified
        .probe_asset(id, path, Arc::new(AtomicBool::new(false)))
        .unwrap()
}

impl Sources {
    fn new(label: &str, look: Look) -> Self {
        crate::initialize_ffmpeg().unwrap();
        let directory = TempDirectory::new(label);
        let heads = head_positions();
        let picture = raw_frames(
            (0..SOURCE_FRAMES).map(|f| source_luma(f, look, &heads)),
            look.ten_bit,
        );
        let samples = sample_of_frame(SOURCE_FRAMES);
        let pcm: Vec<f32> = (0..samples).flat_map(|n| voice(n, look)).collect();
        let options = EncodeOptions {
            ten_bit: look.ten_bit,
            ..PLAIN
        };
        let speech_path = encode(&directory, "speech-source", &picture, &pcm, options);
        let dark = raw_frames(
            (0..SOURCE_FRAMES).map(|_| vec![16_u8; WIDTH * HEIGHT]),
            false,
        );
        let bed: Vec<f32> = (0..samples).flat_map(music).collect();
        let music_path = encode(&directory, "music-source", &dark, &bed, PLAIN);
        let speech_source = certify(&speech_path);
        let music_source = certify(&music_path);
        let speech = asset(&speech_path, SPEECH, &speech_source);
        let music = asset(&music_path, MUSIC, &music_source);
        Self {
            directory,
            look,
            heads,
            speech,
            music,
            pinned: Pinned(BTreeMap::from([
                (SPEECH, speech_source),
                (MUSIC, music_source),
            ])),
        }
    }
}

// ---------------------------------------------------------------- the cut

/// One kept range of the talking head, in source frames, at a timeline frame.
#[derive(Clone)]
struct Part {
    source: Range<u64>,
    at: u64,
}

#[derive(Clone)]
struct Cut {
    parts: Vec<Part>,
    frames: u64,
    music: bool,
    /// A two-frame insert of the music asset's picture on a higher track.
    insert_at: Option<u64>,
    /// A solid-colour clip over these output frames, on the top track.
    solid: Option<Range<u64>>,
    mute_speech: bool,
}

fn standard_cut() -> Cut {
    Cut {
        parts: vec![
            Part {
                source: 0..60,
                at: 0,
            },
            Part {
                source: 90..150,
                at: 60,
            },
        ],
        frames: 120,
        music: false,
        insert_at: None,
        solid: None,
        mute_speech: false,
    }
}

impl Cut {
    fn part_at(&self, frame: u64) -> Option<&Part> {
        self.parts.iter().find(|part| {
            frame >= part.at && frame < part.at + (part.source.end - part.source.start)
        })
    }

    /// The source sample output sample `n` maps on the speech asset.
    fn source_sample(&self, n: usize) -> Option<usize> {
        self.parts.iter().find_map(|part| {
            let start = sample_of_frame(part.at);
            let end = sample_of_frame(part.at + part.source.end - part.source.start);
            (n >= start && n < end).then(|| sample_of_frame(part.source.start) + n - start)
        })
    }
}

fn segment_id(clip: u64, lane: ReviewLane, at: u64) -> ReviewSegmentId {
    ReviewSegmentId {
        clip: ClipId(clip),
        lane,
        project_start: TimeCode(i64::try_from(at).unwrap()),
    }
}

fn span(start: u64, end: u64) -> ReviewSpan {
    us(start)..us(end)
}

fn segments_of(
    clip: u64,
    track: TrackId,
    asset: AssetId,
    source: Range<u64>,
    at: u64,
    video: bool,
) -> Vec<ReviewSegment> {
    let length = source.end - source.start;
    let as_tc = |value: u64| TimeCode(i64::try_from(value).unwrap());
    let output = span(frame_us(at), frame_us(at + length));
    let source_span = span(frame_us(source.start), frame_us(source.end));
    let mut segments = Vec::new();
    if video {
        segments.push(ReviewSegment {
            id: segment_id(clip, ReviewLane::Video, at),
            track,
            asset,
            source: source_span.clone(),
            project: as_tc(at)..as_tc(at + length),
            output: output.clone(),
            sampling: SegmentSampling::Video {
                clip_start: as_tc(at),
                source_in: as_tc(source.start),
                source_end: as_tc(source.end),
                source_fps: fps(),
                effective_fps: fps(),
            },
        });
    }
    segments.push(ReviewSegment {
        id: segment_id(clip, ReviewLane::Audio, at),
        track,
        asset,
        source: source_span,
        project: as_tc(at)..as_tc(at + length),
        output,
        sampling: SegmentSampling::Audio,
    });
    segments
}

fn transcript() -> TranscriptIdentity {
    TranscriptIdentity {
        asset: SPEECH,
        source_sha256: [5; 32],
        source_byte_length: 1,
        audio_stream: 0,
        revision_sha256: [6; 32],
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

fn clip(id: u64, asset: AssetId, source: Range<u64>, at: u64) -> Clip {
    Clip {
        enabled: true,
        enabled_curve: None,
        id: ClipId(id),
        asset,
        source_range: TimeCode(i64::try_from(source.start).unwrap())
            ..TimeCode(i64::try_from(source.end).unwrap()),
        content: ClipContent::Media,
        timeline_start: TimeCode(i64::try_from(at).unwrap()),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
        blend_mode: BlendMode::Normal,
    }
}

fn mix(track: TrackId, gain_tenth_db: i32, mute: bool) -> TrackMix {
    TrackMix {
        track,
        gain_tenth_db,
        pan_percent: 0,
        gain_curve: None,
        pan_curve: None,
        mute,
        solo: false,
    }
}

/// Music bed level under the speech, in tenths of a dB.
const MUSIC_GAIN_TENTH_DB: i32 = -100;

/// The frozen document and expectations of a cut.
fn frozen(sources: &Sources, cut: &Cut) -> (Document, ReviewExpectations) {
    let mut segments = Vec::new();
    let mut speech_clips = Vec::new();
    for (index, part) in cut.parts.iter().enumerate() {
        let id = index as u64 + 1;
        speech_clips.push(clip(id, SPEECH, part.source.clone(), part.at));
        segments.extend(segments_of(
            id,
            SPEECH_TRACK,
            SPEECH,
            part.source.clone(),
            part.at,
            true,
        ));
    }
    let mut tracks = vec![Track {
        id: SPEECH_TRACK,
        kind: TrackKind::Video,
        name: None,
        sync_lock: true,
        clips: speech_clips,
    }];
    let mut mixes = Vec::new();
    if cut.mute_speech {
        mixes.push(mix(SPEECH_TRACK, 0, true));
    }
    if cut.music {
        tracks.push(Track {
            id: MUSIC_TRACK,
            kind: TrackKind::Audio,
            name: None,
            sync_lock: true,
            clips: vec![clip(10, MUSIC, 0..cut.frames, 0)],
        });
        mixes.push(mix(MUSIC_TRACK, MUSIC_GAIN_TENTH_DB, false));
        segments.extend(segments_of(10, MUSIC_TRACK, MUSIC, 0..cut.frames, 0, false));
    }
    if let Some(at) = cut.insert_at {
        tracks.push(Track {
            id: INSERT_TRACK,
            kind: TrackKind::Video,
            name: None,
            sync_lock: true,
            clips: vec![clip(20, MUSIC, 0..2, at)],
        });
        let mut insert = segments_of(20, INSERT_TRACK, MUSIC, 0..2, at, true);
        // The insert's sound is not part of the cut.
        insert.retain(|segment| segment.id.lane == ReviewLane::Video);
        segments.extend(insert);
        mixes.push(mix(INSERT_TRACK, 0, true));
    }
    if let Some(frames) = &cut.solid {
        let as_tc = |value: u64| TimeCode(i64::try_from(value).unwrap());
        // A generated clip: no media segment, a span of project frames.
        tracks.push(Track {
            id: SOLID_TRACK,
            kind: TrackKind::Video,
            name: None,
            sync_lock: true,
            clips: vec![Clip {
                content: ClipContent::Solid(kinewright_core::SolidColor::default()),
                source_range: TimeCode::ZERO..as_tc(frames.end - frames.start),
                ..clip(30, AssetId::default(), 0..1, frames.start)
            }],
        });
    }
    let document = Document {
        investigator: None,
        imported_from: Vec::new(),
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: AudioMix {
            tracks: mixes,
            ..AudioMix::default()
        },
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks,
        media_pool: vec![sources.speech.clone(), sources.music.clone()],
        markers: Vec::new(),
        fps: fps(),
        resolution: (128, 72),
        duration: TimeCode(i64::try_from(cut.frames).unwrap()),
    };
    let words = if sources.look.spanish {
        SPANISH
    } else {
        ENGLISH
    };
    let mut kept = Vec::new();
    for (index, part) in cut.parts.iter().enumerate() {
        let (source_start, source_end) = (frame_us(part.source.start), frame_us(part.source.end));
        let offset = frame_us(part.at);
        for (word_index, &(start, end)) in WORDS.iter().enumerate() {
            let (start_kept, end_kept) = (start.max(source_start), end.min(source_end));
            if start_kept >= end_kept {
                continue;
            }
            kept.push(KeptWord {
                transcript: transcript(),
                word_index: u32::try_from(word_index).unwrap(),
                segment: segment_id(index as u64 + 1, ReviewLane::Audio, part.at),
                text: words[word_index].into(),
                output: span(
                    start_kept - source_start + offset,
                    end_kept - source_start + offset,
                ),
                clipped: start_kept != start || end_kept != end,
            });
        }
    }
    let speech_windows = kept.iter().map(|word| word.output.clone()).collect();
    let expectations = ReviewExpectations {
        identity: ReviewCutIdentity {
            snapshot: ReviewSnapshotId([1; 32]),
            input_manifest_sha256: [2; 32],
            export_settings_sha256: [3; 32],
        },
        project_fps: fps(),
        output_fps: fps(),
        raster: (128, 72),
        output_frames: TimeCode(i64::try_from(cut.frames).unwrap()),
        transcripts: vec![transcript()],
        segments,
        kept_words: inventory(kept, AbsenceProof::NoRecognizedSpeechInKeptRanges),
        speech_windows: inventory(speech_windows, AbsenceProof::NoRecognizedSpeechInKeptRanges),
        uncovered_speech_segments: Vec::new(),
        intended_pauses: ReviewInventory::NoneProven(AbsenceProof::NoAuthorPauseDeclarations),
        av_landmarks: ReviewInventory::NoneProven(AbsenceProof::NoPairedAudioVideo),
        overlays: OverlayExpectations {
            annotated_off_output: Vec::new(),
            captions: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
            titles: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
            cards: ReviewInventory::NoneProven(AbsenceProof::NoDocumentElementsOnOutput),
        },
    };
    (document, expectations)
}

// ---------------------------------------------------------------- the encode

/// One defect seeded into the render, or none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Seed {
    Clean,
    Freeze,
    Black,
    Flash,
    FlashAtCut,
    WrongFrames,
    MissingWord,
    ClippedOnset,
    Click,
    ChannelDropout,
    MusicDropout,
    AudioLate,
    LoudBurst,
}

/// Render the cut from the source generators, then seed one defect.
fn render(sources: &Sources, cut: &Cut, seed: Seed) -> (Vec<u8>, Vec<f32>) {
    let look = sources.look;
    let frame = |output: u64| -> Vec<u8> {
        if cut
            .insert_at
            .is_some_and(|at| output >= at && output < at + 2)
            || cut
                .solid
                .as_ref()
                .is_some_and(|frames| frames.contains(&output))
        {
            return vec![16; WIDTH * HEIGHT];
        }
        let Some(part) = cut.part_at(output) else {
            return vec![16; WIDTH * HEIGHT];
        };
        let mut source = part.source.start + output - part.at;
        if seed == Seed::WrongFrames && part.at > 0 {
            source -= 30;
        }
        match (seed, output) {
            (Seed::Freeze, 20..=35) => source_luma(20, look, &sources.heads),
            (Seed::Black, 70..=76) => vec![16; WIDTH * HEIGHT],
            (Seed::Flash, 45) | (Seed::FlashAtCut, 60) => vec![235; WIDTH * HEIGHT],
            _ => source_luma(source, look, &sources.heads),
        }
    };
    let picture = raw_frames((0..cut.frames).map(frame), look.ten_bit);
    let samples = sample_of_frame(cut.frames);
    let music_gain = 10_f32.powf(MUSIC_GAIN_TENTH_DB as f32 / 200.0);
    let delay = if seed == Seed::AudioLate {
        sample_of_us(120_000)
    } else {
        0
    };
    let mut pcm = Vec::with_capacity(samples * 2);
    for n in 0..samples {
        let mut value = [0.0_f32; 2];
        if !cut.mute_speech
            && let Some(source) = n.checked_sub(delay).and_then(|n| cut.source_sample(n))
        {
            value = voice(source, look);
        }
        if cut.music {
            let bed = music(n);
            value[0] += music_gain * bed[0];
            value[1] += music_gain * bed[1];
        }
        pcm.extend(value);
    }
    let zero = |pcm: &mut [f32], from: u64, to: u64, channels: &[usize]| {
        for n in sample_of_us(from)..sample_of_us(to) {
            for &channel in channels {
                pcm[n * 2 + channel] = 0.0;
            }
        }
    };
    match seed {
        // "and" (source 4.5–4.8 s) plays at 3.5–3.8 s.
        Seed::MissingWord => zero(&mut pcm, 3_500_000, 3_800_000, &[0, 1]),
        // "cuts" (source 3.9–4.3 s) plays from 2.9 s; its first 60 ms go.
        Seed::ClippedOnset => zero(&mut pcm, 2_900_000, 2_960_000, &[0, 1]),
        Seed::ChannelDropout => zero(&mut pcm, 2_950_000, 3_250_000, &[1]),
        // Between "we" and the cut: only the music bed plays here.
        Seed::MusicDropout => zero(&mut pcm, 1_800_000, 1_980_000, &[0, 1]),
        Seed::Click => {
            let at = sample_of_us(2_100_000) * 2;
            for (offset, value) in [0.6_f32, -0.6, 0.6].into_iter().enumerate() {
                pcm[at + offset * 2] += value;
                pcm[at + offset * 2 + 1] += value;
            }
        }
        Seed::LoudBurst => {
            for n in sample_of_us(3_820_000)..sample_of_us(3_860_000) {
                let value =
                    0.95 * (2.0 * std::f32::consts::PI * 1_000.0 * n as f32 / RATE as f32).sin();
                pcm[n * 2] = value;
                pcm[n * 2 + 1] = value;
            }
        }
        _ => {}
    }
    (picture, pcm)
}

struct Encoded {
    path: PathBuf,
    timing: crate::ReviewOutputTiming,
}

fn encode_review(
    sources: &Sources,
    name: &str,
    cut: &Cut,
    seed: Seed,
    options: EncodeOptions,
) -> Encoded {
    let (picture, pcm) = render(sources, cut, seed);
    let options = EncodeOptions {
        ten_bit: sources.look.ten_bit,
        ..options
    };
    let path = encode(&sources.directory, name, &picture, &pcm, options);
    let timing =
        crate::inspect_review_output(File::open(&path).unwrap(), &ExportCancellation::default())
            .unwrap();
    Encoded { path, timing }
}

fn settings(document: &Document, ten_bit: bool) -> ExportSettings {
    let depth = if ten_bit {
        DeliveryEncodeDepth::Ten
    } else {
        DeliveryEncodeDepth::Eight
    };
    DeliveryProfile::SourceMaster.export_settings(document, depth, ExportCancellation::default())
}

struct Checked {
    report: ReviewCheckReport,
}

fn check_with(
    sources: &Sources,
    cut: &Cut,
    encoded: &Encoded,
    technical: &TechnicalScanConfig,
    adjust: impl FnOnce(&mut ExportSettings),
) -> Checked {
    let (document, expectations) = frozen(sources, cut);
    let mut settings = settings(&document, sources.look.ten_bit);
    adjust(&mut settings);
    let report = check_review_artifact(
        &File::open(&encoded.path).unwrap(),
        &ReviewScanInputs {
            timing: &encoded.timing,
            expectations: &expectations,
            document: &document,
            settings: &settings,
            sources: &sources.pinned,
        },
        technical,
        &RetainedSpeechConfig::default(),
        &ExportCancellation::default(),
    )
    .unwrap();
    Checked { report }
}

fn check(sources: &Sources, cut: &Cut, encoded: &Encoded) -> Checked {
    check_with(
        sources,
        cut,
        encoded,
        &TechnicalScanConfig::default(),
        |_| {},
    )
}

fn kinds(row: &ReviewRowReport) -> Vec<&FindingKind> {
    row.findings.iter().map(|finding| &finding.kind).collect()
}

fn assert_pass(label: &str, row: &ReviewRowReport) {
    assert_eq!(
        row.result,
        ReviewRowResult::Pass,
        "{label}: {:?} {:?}",
        row.row,
        row.findings
    );
    for coverage in &row.coverage {
        assert_eq!(
            coverage.checked + coverage.delegated,
            coverage.expected,
            "{label}: {:?} coverage {coverage:?}",
            row.row
        );
    }
}

fn assert_all_pass(label: &str, checked: &Checked) {
    for row in checked.report.rows() {
        assert_pass(label, row);
    }
}

/// The row failed, holds a finding `wanted` accepts, and every finding is
/// one `allowed` accepts (a seed's own consequences).
fn assert_failed(
    label: &str,
    row: &ReviewRowReport,
    wanted: impl Fn(&FindingKind) -> bool,
    allowed: impl Fn(&FindingKind) -> bool,
) {
    assert_eq!(
        row.result,
        ReviewRowResult::Failed,
        "{label}: {:?}",
        kinds(row)
    );
    assert!(row.has(&wanted), "{label}: {:?}", kinds(row));
    assert!(
        row.findings
            .iter()
            .all(|finding| wanted(&finding.kind) || allowed(&finding.kind)),
        "{label}: unexpected {:?}",
        kinds(row)
    );
}

fn none(_: &FindingKind) -> bool {
    false
}

// ---------------------------------------------------------------- controls

/// Ordinary footage of every listed kind passes all four rows, with full
/// coverage and a measured sync.
#[test]
fn ja1_s5a_controls_of_ordinary_footage_pass_every_row() {
    let controls = [
        ("moving", Look::default(), false),
        (
            "tripod",
            Look {
                tripod: true,
                ..Look::default()
            },
            false,
        ),
        (
            "dark",
            Look {
                dark: true,
                ..Look::default()
            },
            false,
        ),
        (
            "fade",
            Look {
                fade: true,
                ..Look::default()
            },
            false,
        ),
        (
            "spanish",
            Look {
                spanish: true,
                ..Look::default()
            },
            false,
        ),
        (
            "ten-bit",
            Look {
                ten_bit: true,
                ..Look::default()
            },
            false,
        ),
        ("music", Look::default(), true),
    ];
    for (label, look, with_music) in controls {
        let sources = Sources::new(&format!("ja1-s5-control-{label}"), look);
        let cut = Cut {
            music: with_music,
            ..standard_cut()
        };
        let encoded = encode_review(&sources, "review", &cut, Seed::Clean, PLAIN);
        let checked = check(&sources, &cut, &encoded);
        assert_all_pass(label, &checked);
        let report = &checked.report;
        assert_eq!(report.decoded_frames, 120, "{label}");
        assert!(report.measured.is_some(), "{label}");
        let sync = &report.sync;
        assert!(
            sync.audio_windows >= 2 && sync.video_windows >= 2,
            "{label}: {sync:?}"
        );
        let error = sync.audio_lag_us.unwrap() - sync.video_lag_us.unwrap();
        assert!(error.abs() <= 25_000, "{label}: {sync:?}");
        // Every kept word was decided.
        let words = report
            .speech
            .coverage
            .iter()
            .find(|coverage| coverage.unit == CoverageUnit::Words)
            .unwrap();
        assert_eq!(words.checked, 6, "{label}");
        let joins = report
            .speech
            .coverage
            .iter()
            .find(|coverage| coverage.unit == CoverageUnit::Joins)
            .unwrap();
        assert_eq!(joins.expected, 3, "{label}");
    }
}

// ---------------------------------------------------------------- seeds

fn is_picture_consequence(kind: &FindingKind) -> bool {
    matches!(kind, FindingKind::PictureMismatch | FindingKind::Flash)
}

/// Encode-level picture defects, each found against its mapped source.
#[test]
fn ja1_s5a_picture_defects_are_found_against_the_source() {
    let sources = Sources::new("ja1-s5-picture", Look::default());
    let cut = standard_cut();
    for (seed, wanted) in [
        (Seed::Freeze, FindingKind::Freeze),
        (Seed::Black, FindingKind::Black),
        (Seed::Flash, FindingKind::Flash),
        // F3: a flash on the first frame after a cut is still a flash.
        (Seed::FlashAtCut, FindingKind::Flash),
        (Seed::WrongFrames, FindingKind::PictureMismatch),
    ] {
        let label = format!("{seed:?}");
        let encoded = encode_review(&sources, &label, &cut, seed, PLAIN);
        let report = check(&sources, &cut, &encoded).report;
        // Wrong frames also hold still where the frames shown do while the
        // mapped source moves: that freeze is the same defect.
        assert_failed(
            &label,
            &report.defects,
            |kind| *kind == wanted,
            |kind| {
                is_picture_consequence(kind)
                    || (seed == Seed::WrongFrames && *kind == FindingKind::Freeze)
            },
        );
        assert_pass(&label, &report.speech);
        assert_pass(&label, &report.loudness);
    }
}

/// Edit-level defects come from the document, exactly.
#[test]
fn ja1_s5a_edit_defects_come_from_the_document() {
    let sources = Sources::new("ja1-s5-edit", Look::default());
    // A two-frame insert of another source between two runs of the head.
    let flash = Cut {
        insert_at: Some(30),
        ..standard_cut()
    };
    let encoded = encode_review(&sources, "insert", &flash, Seed::Clean, PLAIN);
    let report = check(&sources, &flash, &encoded).report;
    assert_failed(
        "insert",
        &report.defects,
        |kind| *kind == FindingKind::EditFlash { frames: 2 },
        none,
    );
    assert!(
        report
            .excluded
            .iter()
            .any(|span| span.reason == ExclusionReason::Composited
                && span.output_us == [frame_us(30), frame_us(32)]),
        "{:?}",
        report.excluded
    );
    // A hole of two frames in the timeline.
    let hole = Cut {
        parts: vec![
            Part {
                source: 0..60,
                at: 0,
            },
            Part {
                source: 92..150,
                at: 62,
            },
        ],
        ..standard_cut()
    };
    let encoded = encode_review(&sources, "hole", &hole, Seed::Clean, PLAIN);
    let report = check(&sources, &hole, &encoded).report;
    assert_failed(
        "hole",
        &report.defects,
        |kind| *kind == FindingKind::TimelineHole,
        none,
    );
    assert_eq!(
        report.defects.findings[0].output_us,
        [frame_us(60), frame_us(62)]
    );
    // The same gap filled by a solid clip is not a hole: the frozen document
    // draws those frames, and they are delegated as generated (no source).
    let filled = Cut {
        solid: Some(60..62),
        ..hole
    };
    let encoded = encode_review(&sources, "filled", &filled, Seed::Clean, PLAIN);
    let report = check(&sources, &filled, &encoded).report;
    assert_pass("filled", &report.defects);
    assert!(
        report
            .excluded
            .iter()
            .any(|span| span.reason == ExclusionReason::Generated
                && span.output_us == [frame_us(60), frame_us(62)]),
        "filled: {:?}",
        report.excluded
    );
    // A solid over the speech is not a plain mapping: those frames are
    // excluded as composited, not compared against the head.
    let covered = Cut {
        solid: Some(40..44),
        ..standard_cut()
    };
    let encoded = encode_review(&sources, "covered", &covered, Seed::Clean, PLAIN);
    let report = check(&sources, &covered, &encoded).report;
    assert_pass("covered", &report.defects);
    assert!(
        report
            .excluded
            .iter()
            .any(|span| span.reason == ExclusionReason::Composited
                && span.output_us == [frame_us(40), frame_us(44)]),
        "covered: {:?}",
        report.excluded
    );
    // The speech track muted in the document, rendered silent.
    let muted = Cut {
        mute_speech: true,
        ..standard_cut()
    };
    let encoded = encode_review(&sources, "muted", &muted, Seed::Clean, PLAIN);
    let report = check(&sources, &muted, &encoded).report;
    assert_failed(
        "muted",
        &report.defects,
        |kind| matches!(kind, FindingKind::SilenceOverSpeech { .. }),
        none,
    );
    assert_failed(
        "muted",
        &report.speech,
        |kind| matches!(kind, FindingKind::WordMuted { .. }),
        none,
    );
    assert_pass("muted", &report.structure);
}

/// Encode-level sound defects, per channel, against the mapped source.
#[test]
fn ja1_s5a_sound_defects_are_found_against_the_source() {
    let sources = Sources::new("ja1-s5-sound", Look::default());
    let cut = standard_cut();
    let dropout = |channel: u8| move |kind: &FindingKind| *kind == FindingKind::Dropout { channel };
    // F7: one channel lost, the other intact.
    let encoded = encode_review(&sources, "channel", &cut, Seed::ChannelDropout, PLAIN);
    let report = check(&sources, &cut, &encoded).report;
    // The seed cuts the channel hard, so its edges are clicks on that
    // channel too; the intact channel has no finding.
    assert_failed("channel", &report.defects, dropout(1), |kind| {
        matches!(kind, FindingKind::Click { channel: 1, .. })
    });
    assert_pass("channel", &report.speech);
    // A click the source does not have, between words (the plosives at
    // every onset are not clicks: the controls pass).
    let encoded = encode_review(&sources, "click", &cut, Seed::Click, PLAIN);
    let report = check(&sources, &cut, &encoded).report;
    assert_failed(
        "click",
        &report.defects,
        |kind| matches!(kind, FindingKind::Click { .. }),
        none,
    );
    // GLM F2: a dropout of non-speech audio, under the music bed.
    let bed = Cut {
        music: true,
        ..standard_cut()
    };
    let encoded = encode_review(&sources, "bed-dropout", &bed, Seed::MusicDropout, PLAIN);
    let report = check(&sources, &bed, &encoded).report;
    assert_failed(
        "bed-dropout",
        &report.defects,
        |kind| matches!(kind, FindingKind::Dropout { .. }),
        none,
    );
    assert_pass("bed-dropout", &report.speech);
}

/// L11: kept words compared acoustically with their source spans.
#[test]
fn ja1_s5a_retained_speech_is_compared_with_the_source_audio() {
    let sources = Sources::new("ja1-s5-speech", Look::default());
    let cut = standard_cut();
    let is_dropout = |kind: &FindingKind| matches!(kind, FindingKind::Dropout { .. });
    let encoded = encode_review(&sources, "missing", &cut, Seed::MissingWord, PLAIN);
    let report = check(&sources, &cut, &encoded).report;
    assert_failed(
        "missing",
        &report.speech,
        |kind| matches!(kind, FindingKind::WordMissing { word_index: 6, .. }),
        none,
    );
    assert_failed("missing", &report.defects, is_dropout, none);
    let encoded = encode_review(&sources, "onset", &cut, Seed::ClippedOnset, PLAIN);
    let report = check(&sources, &cut, &encoded).report;
    assert_failed(
        "onset",
        &report.speech,
        |kind| *kind == FindingKind::WordClippedStart { word_index: 5 },
        none,
    );
    // Under a music bed the same loss is still decided, not left ambiguous.
    let bed = Cut {
        music: true,
        ..standard_cut()
    };
    let encoded = encode_review(&sources, "bed-missing", &bed, Seed::MissingWord, PLAIN);
    let report = check(&sources, &bed, &encoded).report;
    assert_failed(
        "bed-missing",
        &report.speech,
        |kind| matches!(kind, FindingKind::WordMissing { word_index: 6, .. }),
        none,
    );
}

/// F5 / GLM F1: a cut inside sound at output 0 and at the output's end is
/// guarded like any other join.
#[test]
fn ja1_s5a_joins_at_output_start_and_end_are_guarded() {
    let sources = Sources::new("ja1-s5-joins", Look::default());
    // The first part starts inside "so" (source 0.3–0.6 s).
    let start = Cut {
        parts: vec![
            Part {
                source: 12..60,
                at: 0,
            },
            Part {
                source: 90..150,
                at: 48,
            },
        ],
        frames: 108,
        ..standard_cut()
    };
    // The last part ends inside "and" (source 4.5–4.8 s).
    let end = Cut {
        parts: vec![
            Part {
                source: 0..60,
                at: 0,
            },
            Part {
                source: 90..141,
                at: 60,
            },
        ],
        frames: 111,
        ..standard_cut()
    };
    for (label, cut, at) in [("start", start, 0), ("end", end, frame_us(111))] {
        let encoded = encode_review(&sources, label, &cut, Seed::Clean, PLAIN);
        let report = check(&sources, &cut, &encoded).report;
        assert_failed(
            label,
            &report.speech,
            |kind| matches!(kind, FindingKind::JoinInSound { .. }),
            none,
        );
        assert!(
            report
                .speech
                .findings
                .iter()
                .all(|finding| finding.output_us == [at, at]),
            "{label}: {:?}",
            report.speech.findings
        );
    }
}

/// Sync is measured against the source: audio 120 ms late fails.
#[test]
fn ja1_s5a_sync_offset_is_measured_against_the_source() {
    let sources = Sources::new("ja1-s5-sync", Look::default());
    let cut = standard_cut();
    let encoded = encode_review(&sources, "late", &cut, Seed::AudioLate, PLAIN);
    let report = check(&sources, &cut, &encoded).report;
    assert_failed(
        "late",
        &report.structure,
        |kind| matches!(kind, FindingKind::SyncOffset { .. }),
        none,
    );
    let lag = report.sync.audio_lag_us.unwrap() - report.sync.video_lag_us.unwrap();
    assert!((lag - 120_000).abs() <= 15_000, "{:?}", report.sync);
}

/// L10: loudness against the frozen export target, or peak only.
#[test]
fn ja1_s5a_loudness_is_judged_against_the_export_target() {
    let sources = Sources::new("ja1-s5-loudness", Look::default());
    let cut = standard_cut();
    let encoded = encode_review(&sources, "clean", &cut, Seed::Clean, PLAIN);
    let clean = check(&sources, &cut, &encoded).report;
    let measured = clean.measured.unwrap();
    let lufs = measured.integrated_lufs_hundredths.unwrap();
    let peak = measured.true_peak_dbtp_hundredths.unwrap();
    // Not normalised: integrated loudness is only recorded, far from −14.
    assert!(lufs < -1_800, "{measured:?}");
    assert_pass("unnormalised", &clean.loudness);
    let target = |integrated: i32, ceiling: i32| LoudnessTarget {
        integrated_lufs_hundredths: integrated,
        tolerance_lu_hundredths: 100,
        true_peak_ceiling_dbtp_hundredths: ceiling,
        loudness_range_max_lu_hundredths: None,
    };
    let technical = TechnicalScanConfig::default();
    let met = check_with(&sources, &cut, &encoded, &technical, |settings| {
        settings.loudness_normalization = Some(target(lufs, peak + 100));
    });
    assert_pass("met", &met.report.loudness);
    let missed = check_with(&sources, &cut, &encoded, &technical, |settings| {
        settings.loudness_normalization = Some(target(lufs + 300, peak + 100));
    });
    assert_failed(
        "missed",
        &missed.report.loudness,
        |kind| matches!(kind, FindingKind::IntegratedLoudness { .. }),
        none,
    );
    let burst = encode_review(&sources, "burst", &cut, Seed::LoudBurst, PLAIN);
    let over = check_with(&sources, &cut, &burst, &technical, |settings| {
        settings.loudness_normalization = Some(target(lufs, peak + 100));
    });
    assert!(
        over.report
            .loudness
            .has(|kind| matches!(kind, FindingKind::TruePeak { .. })),
        "{:?}",
        over.report.loudness
    );
}

// ---------------------------------------------------------------- L12

fn assert_not_pass(label: &str, row: &ReviewRowReport) {
    assert_ne!(row.result, ReviewRowResult::Pass, "{label}: {:?}", row.row);
}

/// F1 / GLM F11: a scan stopped at its cap leaves its rows undecided.
#[test]
fn ja1_s5a_truncated_scans_never_pass() {
    let sources = Sources::new("ja1-s5-truncated", Look::default());
    let cut = standard_cut();
    let encoded = encode_review(&sources, "review", &cut, Seed::Clean, PLAIN);
    let frames = TechnicalScanConfig {
        max_frames: 60,
        ..TechnicalScanConfig::default()
    };
    let report = check_with(&sources, &cut, &encoded, &frames, |_| {}).report;
    let truncated =
        |pass: ScanPass| move |kind: &FindingKind| *kind == FindingKind::ScanTruncated { pass };
    for row in [&report.structure, &report.defects] {
        assert_eq!(row.result, ReviewRowResult::Unknown, "{:?}", row.findings);
        assert!(row.has(truncated(ScanPass::Picture)), "{:?}", row.findings);
    }
    let hops = TechnicalScanConfig {
        max_hops: 200,
        ..TechnicalScanConfig::default()
    };
    let report = check_with(&sources, &cut, &encoded, &hops, |_| {}).report;
    for row in [&report.structure, &report.loudness, &report.defects] {
        assert_eq!(row.result, ReviewRowResult::Unknown, "{:?}", row.findings);
        assert!(row.has(truncated(ScanPass::Sound)), "{:?}", row.findings);
    }
    assert_not_pass("hops", &report.speech);
}

/// F2: an encode without its audio stream never passes a sound row.
#[test]
fn ja1_s5a_missing_audio_stream_never_passes() {
    let sources = Sources::new("ja1-s5-no-audio", Look::default());
    let cut = standard_cut();
    let silent = EncodeOptions {
        audio: false,
        ..PLAIN
    };
    let encoded = encode_review(&sources, "review", &cut, Seed::Clean, silent);
    assert!(encoded.timing.audio.is_none());
    let report = check(&sources, &cut, &encoded).report;
    assert!(
        report
            .structure
            .has(|kind| *kind == FindingKind::AudioStreamMissing)
    );
    assert_eq!(report.structure.result, ReviewRowResult::Failed);
    for row in [&report.loudness, &report.defects, &report.speech] {
        assert_not_pass("no-audio", row);
    }
    assert!(
        report
            .defects
            .has(|kind| matches!(kind, FindingKind::SoundNotCompared { .. }))
    );
}

/// GLM F10 and the muted-track case: audio the cut did not ask for fails.
#[test]
fn ja1_s5a_unexpected_audio_fails_structure() {
    let sources = Sources::new("ja1-s5-unexpected", Look::default());
    let cut = standard_cut();
    let extra = EncodeOptions {
        extra_audio: true,
        ..PLAIN
    };
    let encoded = encode_review(&sources, "extra", &cut, Seed::Clean, extra);
    let report = check(&sources, &cut, &encoded).report;
    assert_failed(
        "extra",
        &report.structure,
        |kind| *kind == FindingKind::ExtraAudioStream { streams: 2 },
        none,
    );
    // The document mutes the speech, the encode still carries it.
    let muted = Cut {
        mute_speech: true,
        ..standard_cut()
    };
    let encoded = encode_review(&sources, "audible", &cut, Seed::Clean, PLAIN);
    let report = check(&sources, &muted, &encoded).report;
    assert_failed(
        "audible",
        &report.structure,
        |kind| *kind == FindingKind::UnexpectedAudio,
        none,
    );
}

/// F7: audio decoded before the first picture is reported.
#[test]
fn ja1_s5a_early_audio_is_reported() {
    let sources = Sources::new("ja1-s5-early", Look::default());
    let cut = standard_cut();
    let early = EncodeOptions {
        audio_offset_ms: -100,
        ..PLAIN
    };
    let encoded = encode_review(&sources, "early", &cut, Seed::Clean, early);
    let report = check(&sources, &cut, &encoded).report;
    assert!(
        report
            .structure
            .has(|kind| matches!(kind, FindingKind::AudioEarly { .. })),
        "{:?} {:?}",
        report.structure.findings,
        encoded.timing
    );
    assert_ne!(report.structure.result, ReviewRowResult::Pass);
}

/// Neon F9: a timing record that no longer describes the encode (another
/// decoder build, another audio stream decoder) is never a pass.
#[test]
fn ja1_s5a_a_changed_timing_record_is_not_a_pass() {
    let sources = Sources::new("ja1-s5-timing", Look::default());
    let cut = standard_cut();
    let mut encoded = encode_review(&sources, "review", &cut, Seed::Clean, PLAIN);
    let changed = |kind: &FindingKind| *kind == FindingKind::TimingRecordChanged;
    encoded.timing.decoder_version ^= 1;
    let report = check(&sources, &cut, &encoded).report;
    assert!(
        report.structure.has(changed),
        "version: {:?}",
        report.structure.findings
    );
    assert_ne!(report.structure.result, ReviewRowResult::Pass, "version");
    encoded.timing.decoder_version ^= 1;
    encoded.timing.audio.as_mut().unwrap().decoder = "another".into();
    let report = check(&sources, &cut, &encoded).report;
    assert!(
        report.structure.has(changed),
        "decoder: {:?}",
        report.structure.findings
    );
    assert_ne!(report.structure.result, ReviewRowResult::Pass, "decoder");
}

// ---------------------------------------------------------------- units

/// F4: P010 keeps its 10-bit code in the high bits.
#[test]
fn ja1_s5a_p010_luma_reads_the_high_bits() {
    crate::initialize_ffmpeg().unwrap();
    let mut frame = ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::P010LE, 64, 36);
    frame.set_color_range(ffmpeg_next::util::color::Range::MPEG);
    let stride = frame.stride(0);
    let code: u16 = 512 << 6;
    for y in 0..36 {
        let row = &mut frame.data_mut(0)[y * stride..y * stride + 128];
        for pair in row.as_chunks_mut::<2>().0 {
            *pair = code.to_le_bytes();
        }
    }
    let mut scaler = grid::RgbaScaler::default();
    let cells = grid::from_frame(&frame, &mut scaler).unwrap();
    let expected = (512.0 - 64.0) / 876.0;
    assert!(
        cells.iter().all(|cell| (cell - expected).abs() < 0.002),
        "{:?}",
        &cells[..4]
    );
}

/// F24: samples the resampler holds at the end are flushed.
#[test]
fn ja1_s5a_resampled_output_audio_is_flushed() {
    let sources = Sources::new("ja1-s5-flush", Look::default());
    let cut = standard_cut();
    let encoded = encode_review(&sources, "review", &cut, Seed::Clean, PLAIN);
    let stream = encoded.timing.audio.as_ref().unwrap().index;
    let count = |rate: Option<u32>| {
        let file = File::open(&encoded.path).unwrap();
        let cancel = ExportCancellation::default();
        let mut reader =
            reader::AudioReader::open(&file, &encoded.timing, stream, rate, &cancel).unwrap();
        let mut samples = Vec::new();
        let mut total = 0_u64;
        while let Some(chunk) = reader.next(&mut samples, &cancel).unwrap() {
            total += chunk.frames as u64;
        }
        total
    };
    let native = count(None);
    let resampled = count(Some(44_100));
    let expected = native * 44_100 / 48_000;
    assert!(
        resampled.abs_diff(expected) <= 2,
        "{resampled} of {expected}"
    );
}
