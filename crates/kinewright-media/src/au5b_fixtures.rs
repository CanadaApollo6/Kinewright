//! AU5 §5.4: the room-tone seam lanes and their budget.
//!
//! Exit-gate clause 2 — *"fills are seamless at 1e-4 across the join"* — is
//! closed here. Every lane mixes a real document through `export::mix_audio`,
//! the same path an export takes, and compares it sample by sample against the
//! concatenation of each piece's **own** `decode_audio_range`.
//!
//! **Why this file is in `src/` and not in `tests/au5_fixtures.rs`.** AU5 §8
//! attributes the seam lanes to the integration fixture file, but an
//! integration test links only the crate's **public** surface and
//! `export::mix_audio` is `pub(crate)` (export.rs:1078) — as is
//! `test_support`, behind the `test-util` feature no default build enables.
//! Rather than widen the public API for a test, the lanes live beside the code
//! they measure, exactly as AU5 §0 R62 already moved four Part A lanes here
//! for the same reason. Recorded as AU5 §0 R100.
//!
//! Every lane **prints its measurement** and asserts against a single named
//! constant. There is no `cfg`-conditioned tolerance anywhere in this file,
//! and there must never be one.

use std::ops::Range;

use kinewright_core::{
    AssetId, AudioMix, Clip, ClipContent, ClipId, ColorContext, Document, ExportCancellation,
    ExportSettings, MediaAsset, MediaCatalog, Rational, TimeCode, TimeMappingError, Track, TrackId,
    TrackKind, covering_source_range_for_project_duration, map_frames,
    map_project_duration_to_source, map_source_range_to_project,
};

use crate::{
    audio::decode_audio_range,
    clock::frame_to_samples,
    decode::probe_path,
    export::mix_audio,
    room_tone_store::{ROOM_TONE_CHANNELS, ROOM_TONE_SAMPLE_RATE, RoomToneStore},
    test_support::{GeneratedMedia, TempDirectory, pseudo_random_amplitude, tone, wav_f32},
};

/// AU5 §5.4 rule 100, the roadmap's own number: the per-sample deviation a
/// fill may show across a join.
///
/// **Expected: exactly 0.0.** Each `AudioMixSource` opens its own
/// `AudioDecoder` at its own `source_sample_start` and the resampler is built
/// per decoder from the first frame; with a **48 kHz PCM** fixture meeting a
/// 48 kHz mix, swresample does format conversion only — no rate filter, no
/// state to prime — so the difference is exact rather than 1e-7. **That
/// exactness is a property of the fixture and does not generalise to a
/// rate-converted source**, and this sentence is part of the pin: a lane that
/// ever measures 1e-7 here has silently acquired a resampler.
const ROOM_TONE_SEAM_BUDGET: f64 = 1.0e-4;

/// AU5 §0 R101: the sub-frame shortfall a **non-covering** fill leaves, in
/// 48 kHz sample frames.
///
/// A fill's source range is a whole number of **asset** frames and its project
/// span a whole number of **project** frames, and `map_frames` maps absolute
/// boundaries and subtracts — so which exact ranges exist, and how many sample
/// frames they carry, depends on the **phase** of `source_start`. A phase whose
/// exact end falls short can miss by up to half a project frame: at 25 fps, 960
/// sample frames. One whole project frame is pinned here as the conservative
/// bound; the phase-0 lane measures 640. This budget applies **only** to the
/// lane that deliberately builds a non-covering fill;
/// `covering_source_range_for_project_duration` is what a real fill uses, and
/// its lane asserts an exact seam instead.
const ROOM_TONE_FILL_RESIDUAL_BUDGET_SAMPLE_FRAMES: u64 = 1_920;

/// CC6 rule 11.0.5: a budget no measurement approaches proves nothing.
const FIXTURE_MINIMUM_MARGIN: f64 = 2.0;

/// The window either side of a join the seam is measured over, in
/// milliseconds (AU5 §5.4 rule 100).
const SEAM_WINDOW_MILLISECONDS: u64 = 10;

/// AU5 §5.4 rule 99's room tone: `pseudo_random_amplitude(96_000 * 2, 0.010)`.
///
/// The helper's first argument is a **sample** count (AU5 §0 R45), so this is
/// 96 000 sample frames × 2 channels — 2 s of 48 kHz stereo, and exactly 60
/// frames of the 30 fps grid an audio-only asset probes on.
fn room_tone_samples() -> Vec<f32> {
    pseudo_random_amplitude(96_000 * 2, 0.010)
}

/// Three seconds of stand-in dialogue: a 220 Hz carrier over the same noise
/// floor, so a level error, a fade or a one-sample slip at a join is visible
/// in the difference rather than hidden in silence.
fn dialogue_samples() -> Vec<f32> {
    let frames = 144_000;
    let carrier = tone(220.0, 0.200, ROOM_TONE_SAMPLE_RATE, frames);
    let noise = pseudo_random_amplitude(frames, 0.010);
    let mut stereo = Vec::with_capacity(frames * 2);
    for (carrier, noise) in carrier.iter().zip(&noise) {
        let sample = carrier + noise;
        stereo.push(sample);
        stereo.push(sample);
    }
    stereo
}

/// One clip of the seam document.
struct Piece {
    asset: AssetId,
    source: Range<TimeCode>,
    start: TimeCode,
}

/// One same-track document: the dialogue either side, the fill between.
fn seam_document(fps: Rational, assets: &[MediaAsset], pieces: Vec<Piece>) -> Document {
    let clips: Vec<Clip> = pieces
        .into_iter()
        .enumerate()
        .map(|(index, piece)| Clip {
            id: ClipId(index as u64 + 1),
            asset: piece.asset,
            source_range: piece.source,
            content: ClipContent::Media,
            timeline_start: piece.start,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
            audio_gain_curve: None,
        })
        .collect();
    let duration = clips
        .iter()
        .map(|clip| {
            let asset = assets
                .iter()
                .find(|asset| asset.id == clip.asset)
                .expect("every clip names a pooled asset");
            let span =
                map_source_range_to_project(clip.source_range.clone(), asset.fps, fps).unwrap();
            TimeCode(clip.timeline_start.0 + span.0)
        })
        .max()
        .expect("the document carries clips");
    let document = Document {
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips,
        }],
        media_pool: assets.to_vec(),
        markers: Vec::new(),
        fps,
        resolution: (320, 180),
        duration,
    };
    document
        .validate()
        .expect("the seam document is a legal document");
    document
}

fn seam_settings(document: &Document) -> ExportSettings {
    ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 1,
        audio_bitrate: 1,
        loudness_normalization: None,
        cancellation: ExportCancellation::default(),
    }
}

/// One piece's **own** decode, at the same rate and channel count the mix runs
/// at — the reference half of AU5 §5.4 rule 100.
fn reference_decode(asset: &MediaAsset, source: Range<TimeCode>) -> Vec<f32> {
    decode_audio_range(
        &asset.path,
        asset.fps,
        source.start,
        source.end,
        ROOM_TONE_SAMPLE_RATE,
        ROOM_TONE_CHANNELS,
        &ExportCancellation::default(),
    )
    .expect("a fixture decodes")
}

/// The sample-frame index of one project frame.
fn sample_frame_at(frame: TimeCode, fps: Rational) -> u64 {
    frame_to_samples(frame, ROOM_TONE_SAMPLE_RATE, fps)
}

/// `SEAM_WINDOW_MILLISECONDS` either side of a join, in sample frames.
const fn seam_window() -> u64 {
    SEAM_WINDOW_MILLISECONDS * ROOM_TONE_SAMPLE_RATE as u64 / 1_000
}

/// The largest per-sample deviation over `[from, to)` sample frames.
fn max_deviation(mixed: &[f32], expected: &[f32], from: u64, to: u64) -> f64 {
    let channels = usize::from(ROOM_TONE_CHANNELS);
    let start = usize::try_from(from).unwrap() * channels;
    let end = usize::try_from(to).unwrap() * channels;
    assert!(
        end <= mixed.len() && end <= expected.len(),
        "the comparison window {start}..{end} must lie inside both buffers \
         (mixed {}, expected {})",
        mixed.len(),
        expected.len(),
    );
    mixed[start..end]
        .iter()
        .zip(&expected[start..end])
        .map(|(mixed, expected)| f64::from(*mixed) - f64::from(*expected))
        .fold(0.0_f64, |worst, difference| worst.max(difference.abs()))
}

/// AU3's idiom: a zero deviation has unbounded margin and says so.
fn render_margin(measured: f64, budget: f64) -> String {
    if measured <= 0.0 {
        "unbounded".to_owned()
    } else {
        format!("{:.2}x", budget / measured)
    }
}

fn assert_seam(lane: &str, join: &str, measured: f64) {
    println!(
        "AU5_ROOM_TONE_SEAM lane={lane} join={join} measured={measured:.3e} \
         budget={ROOM_TONE_SEAM_BUDGET:.1e} margin={}",
        render_margin(measured, ROOM_TONE_SEAM_BUDGET),
    );
    assert!(
        measured <= ROOM_TONE_SEAM_BUDGET,
        "the {lane} lane's {join} join deviated by {measured}, past \
         {ROOM_TONE_SEAM_BUDGET}",
    );
    assert!(
        measured <= 0.0,
        "a 48 kHz PCM fill meets a 48 kHz mix with no resampler in the way, so \
         the {lane} lane's {join} join must be exactly 0.0, not {measured}: a \
         non-zero reading here means a rate conversion has appeared",
    );
}

/// The room tone, dialogue and their probed assets, kept alive together.
struct SeamFixture {
    _store_root: TempDirectory,
    _dialogue_media: GeneratedMedia,
    room_tone: MediaAsset,
    dialogue: MediaAsset,
    room_tone_samples: Vec<f32>,
}

impl SeamFixture {
    fn new(label: &str) -> Self {
        crate::initialize_ffmpeg().expect("FFmpeg initializes");
        let store_root = TempDirectory::new(label);
        let store = RoomToneStore::for_project(&store_root.path("seam.kinewright"))
            .expect("the store root derives");
        let room_tone_samples = room_tone_samples();
        let capture = store
            .write_capture(&room_tone_samples)
            .expect("2 s of room tone is a legal capture");
        assert_eq!(capture.frames, TimeCode(60));

        let dialogue_media = GeneratedMedia::from_bytes(
            label,
            "wav",
            &wav_f32(
                &dialogue_samples(),
                ROOM_TONE_SAMPLE_RATE,
                ROOM_TONE_CHANNELS,
            ),
        );
        let room_tone = probe_path(&capture.path, AssetId(1)).expect("the capture probes");
        let dialogue = probe_path(dialogue_media.path(), AssetId(2)).expect("the dialogue probes");
        assert_eq!(room_tone.fps, Rational::default());
        assert_eq!(dialogue.fps, Rational::default());
        assert_eq!(room_tone.duration, TimeCode(60));
        assert_eq!(dialogue.duration, TimeCode(90));

        Self {
            _store_root: store_root,
            _dialogue_media: dialogue_media,
            room_tone,
            dialogue,
            room_tone_samples,
        }
    }

    fn assets(&self) -> Vec<MediaAsset> {
        vec![self.room_tone.clone(), self.dialogue.clone()]
    }
}

/// AU5 §7 B4 / §5.4 rule 100: clip A `0..30`, gap `30..60`, clip B `60..90`,
/// one tile of room tone in the gap, at 30 fps.
///
/// One assertion catches an off-by-one fill start, a stray fade, a level
/// error, a resample-phase error and a wrong source range — every plausible
/// way a fill stops being seamless.
#[test]
fn au5_a_single_tile_fill_is_seamless_across_its_join() {
    let fixture = SeamFixture::new("au5-seam-single");
    let assets = fixture.assets();
    let fps = Rational::new(30, 1).unwrap();
    let document = seam_document(
        fps,
        &assets,
        vec![
            Piece {
                asset: AssetId(2),
                source: TimeCode(0)..TimeCode(30),
                start: TimeCode(0),
            },
            Piece {
                asset: AssetId(1),
                source: TimeCode(0)..TimeCode(30),
                start: TimeCode(30),
            },
            Piece {
                asset: AssetId(2),
                source: TimeCode(60)..TimeCode(90),
                start: TimeCode(60),
            },
        ],
    );
    assert_eq!(document.duration, TimeCode(90));

    let fill = &document.tracks[0].clips[1];
    assert_eq!(
        document.clip_duration(fill).unwrap(),
        TimeCode(30),
        "AU5 §5.3: the fill's project duration is exactly the gap",
    );

    let mixed = mix_audio(&document, &seam_settings(&document)).expect("the seam document mixes");
    let mut expected = reference_decode(&fixture.dialogue, TimeCode(0)..TimeCode(30));
    let reference = reference_decode(&fixture.room_tone, TimeCode(0)..TimeCode(30));
    assert_eq!(reference, fixture.room_tone_samples[..reference.len()]);
    expected.extend_from_slice(&reference);
    expected.extend(reference_decode(
        &fixture.dialogue,
        TimeCode(60)..TimeCode(90),
    ));
    assert_eq!(mixed.len(), expected.len());

    let window = seam_window();
    let measured = max_deviation(
        &mixed,
        &expected,
        sample_frame_at(TimeCode(30), fps) - window,
        sample_frame_at(TimeCode(60), fps) + window,
    );
    assert_seam("single_tile", "whole_gap", measured);
}

/// AU5 §7 B4, CC6 rule 11.0.5: the single-tile pin is **falsifiable**.
///
/// Every seam lane prints `margin=unbounded`, so `FIXTURE_MINIMUM_MARGIN` never
/// applies to one and nothing else in the suite shows the pin can fail. This
/// lane slips the fill's source range by exactly one asset frame — `1..31`
/// instead of `0..30`, which is still a legal, non-overlapping, 30-project-frame
/// fill — and measures **199x the budget**. That is the number that makes the
/// zeros above mean something.
#[test]
fn au5_a_one_frame_slip_breaks_the_seam_pin() {
    let fixture = SeamFixture::new("au5-seam-slip");
    let assets = fixture.assets();
    let fps = Rational::new(30, 1).unwrap();
    let document = seam_document(
        fps,
        &assets,
        vec![
            Piece {
                asset: AssetId(2),
                source: TimeCode(0)..TimeCode(30),
                start: TimeCode(0),
            },
            Piece {
                asset: AssetId(1),
                source: TimeCode(1)..TimeCode(31),
                start: TimeCode(30),
            },
            Piece {
                asset: AssetId(2),
                source: TimeCode(60)..TimeCode(90),
                start: TimeCode(60),
            },
        ],
    );
    let fill = &document.tracks[0].clips[1];
    assert_eq!(
        document.clip_duration(fill).unwrap(),
        TimeCode(30),
        "the slipped fill is still exactly the gap, so nothing but the samples changed",
    );

    let mixed =
        mix_audio(&document, &seam_settings(&document)).expect("the slipped document mixes");
    let mut expected = reference_decode(&fixture.dialogue, TimeCode(0)..TimeCode(30));
    expected.extend(reference_decode(
        &fixture.room_tone,
        TimeCode(0)..TimeCode(30),
    ));
    expected.extend(reference_decode(
        &fixture.dialogue,
        TimeCode(60)..TimeCode(90),
    ));
    assert_eq!(mixed.len(), expected.len());

    let window = seam_window();
    let measured = max_deviation(
        &mixed,
        &expected,
        sample_frame_at(TimeCode(30), fps) - window,
        sample_frame_at(TimeCode(60), fps) + window,
    );
    println!(
        "AU5_ROOM_TONE_SEAM_FALSIFICATION lane=one_frame_slip measured={measured:.4e} \
         budget={ROOM_TONE_SEAM_BUDGET:.1e} over_budget={:.0}x",
        measured / ROOM_TONE_SEAM_BUDGET,
    );
    assert!(
        measured > ROOM_TONE_SEAM_BUDGET,
        "a one-frame source slip must break the seam pin, or the pin proves nothing: \
         {measured}",
    );
    assert!(
        measured >= 1.0e-2,
        "the slip is expected to read about 1.99e-2, two orders of magnitude past the \
         budget, not marginally over it: {measured}",
    );
}

/// AU5 §7 B5 / §5.4 rule 101(i): a 90-frame gap tiled from the 60-frame
/// sample, asserting **all three** joins — clip A → tile 1, tile 1 → tile 2,
/// tile 2 → clip B, of which one is internal to the fill.
#[test]
fn au5_a_tiled_fill_is_seamless_across_all_three_joins() {
    let fixture = SeamFixture::new("au5-seam-tiled");
    let assets = fixture.assets();
    let fps = Rational::new(30, 1).unwrap();
    let gap = TimeCode(30)..TimeCode(120);
    let tile_frames = fixture.room_tone.duration.0;
    let gap_frames = gap.end.0 - gap.start.0;
    let tiles = (gap_frames + tile_frames - 1) / tile_frames;
    assert_eq!(tiles, 2, "90 frames from a 60-frame sample is two tiles");

    let document = seam_document(
        fps,
        &assets,
        vec![
            Piece {
                asset: AssetId(2),
                source: TimeCode(0)..TimeCode(30),
                start: TimeCode(0),
            },
            Piece {
                asset: AssetId(1),
                source: TimeCode(0)..TimeCode(60),
                start: TimeCode(30),
            },
            Piece {
                asset: AssetId(1),
                source: TimeCode(0)..TimeCode(30),
                start: TimeCode(90),
            },
            Piece {
                asset: AssetId(2),
                source: TimeCode(60)..TimeCode(90),
                start: TimeCode(120),
            },
        ],
    );
    assert_eq!(document.duration, TimeCode(150));

    let mixed = mix_audio(&document, &seam_settings(&document)).expect("the tiled document mixes");
    let mut expected = reference_decode(&fixture.dialogue, TimeCode(0)..TimeCode(30));
    expected.extend(reference_decode(
        &fixture.room_tone,
        TimeCode(0)..TimeCode(60),
    ));
    expected.extend(reference_decode(
        &fixture.room_tone,
        TimeCode(0)..TimeCode(30),
    ));
    expected.extend(reference_decode(
        &fixture.dialogue,
        TimeCode(60)..TimeCode(90),
    ));
    assert_eq!(mixed.len(), expected.len());

    let window = seam_window();
    for (join, frame) in [
        ("clip_a_to_tile_1", TimeCode(30)),
        ("tile_1_to_tile_2", TimeCode(90)),
        ("tile_2_to_clip_b", TimeCode(120)),
    ] {
        let centre = sample_frame_at(frame, fps);
        let measured = max_deviation(&mixed, &expected, centre - window, centre + window);
        assert_seam("tiled", join, measured);
    }
    let measured = max_deviation(
        &mixed,
        &expected,
        sample_frame_at(gap.start, fps) - window,
        sample_frame_at(gap.end, fps) + window,
    );
    assert_seam("tiled", "whole_gap", measured);
}

/// AU5 §7 B6 / §5.4 rule 101(ii): a **25 fps** project reading the 30 fps
/// audio-only asset over a **7**-frame gap, filled seamlessly.
///
/// The fill comes from `covering_source_range_for_project_duration`, not from
/// `map_project_duration_to_source`: an exact range is exact in project
/// *frames*, which is not the same as carrying enough sample frames to fill
/// the gap, and **which exact ranges exist depends on the phase of
/// `source_start`** (§0 R96, R101). At 30 → 25 with `D = 7` the phase-0 answer
/// `0..8` supplies 12 800 sample frames against the gap's 13 440 and leaves
/// 640 of silence — that is the next lane — while phase 1 admits the run
/// `{9, 10}` and `1..10` supplies 14 400. A source range carrying **more**
/// sample frames than the gap is neither an overlap nor a validation failure:
/// the mixer bounds the destination by `project_sample_end` and `retire()`s
/// the source once the span is covered (audio.rs:4245-4262), so the excess is
/// cleanly truncated.
#[test]
fn au5_a_twenty_five_fps_fill_is_seamless_across_its_join() {
    let fixture = SeamFixture::new("au5-seam-25fps");
    let assets = fixture.assets();
    let fps = Rational::new(25, 1).unwrap();
    let gap = TimeCode(25)..TimeCode(32);
    let gap_frames = TimeCode(gap.end.0 - gap.start.0);
    assert_eq!(gap_frames, TimeCode(7));

    let source = covering_source_range_for_project_duration(
        gap_frames,
        fixture.room_tone.fps,
        fps,
        fixture.room_tone.duration,
        ROOM_TONE_SAMPLE_RATE,
    )
    .expect("a 7-frame gap at 25 fps has a covering 30 fps source range");
    assert_eq!(
        source,
        TimeCode(1)..TimeCode(10),
        "the smallest covering phase, §0 R96's own worked example",
    );

    let document = seam_document(
        fps,
        &assets,
        vec![
            Piece {
                asset: AssetId(2),
                source: TimeCode(0)..TimeCode(30),
                start: TimeCode(0),
            },
            Piece {
                asset: AssetId(1),
                source: source.clone(),
                start: gap.start,
            },
            Piece {
                asset: AssetId(2),
                source: TimeCode(60)..TimeCode(90),
                start: gap.end,
            },
        ],
    );
    assert_eq!(document.duration, TimeCode(57));

    let fill = &document.tracks[0].clips[1];
    assert_eq!(
        document.clip_duration(fill).unwrap(),
        gap_frames,
        "the fill fills the gap exactly, so no ClipOverlap and no residual gap",
    );
    assert_eq!(
        TimeCode(fill.timeline_start.0 + document.clip_duration(fill).unwrap().0),
        gap.end,
        "and it ends exactly where clip B begins",
    );

    let mixed = mix_audio(&document, &seam_settings(&document)).expect("the 25 fps document mixes");

    let channels = u64::from(ROOM_TONE_CHANNELS);
    let required = sample_frame_at(gap.end, fps) - sample_frame_at(gap.start, fps);
    let reference = reference_decode(&fixture.room_tone, source);
    let supplied = reference.len() as u64 / channels;
    println!(
        "AU5_ROOM_TONE_COVERAGE lane=twenty_five_fps required={required} supplied={supplied} \
         excess={}",
        supplied - required,
    );
    assert_eq!((required, supplied), (13_440, 14_400));
    assert!(
        supplied >= required,
        "a covering range must supply at least the gap's sample frames",
    );

    let mut expected = reference_decode(&fixture.dialogue, TimeCode(0)..TimeCode(30));
    expected.extend_from_slice(&reference[..usize::try_from(required * channels).unwrap()]);
    expected.extend(reference_decode(
        &fixture.dialogue,
        TimeCode(60)..TimeCode(90),
    ));
    assert_eq!(mixed.len(), expected.len());

    let window = seam_window();
    for (join, frame) in [("clip_a_to_fill", gap.start), ("fill_to_clip_b", gap.end)] {
        let centre = sample_frame_at(frame, fps);
        let measured = max_deviation(&mixed, &expected, centre - window, centre + window);
        assert_seam("twenty_five_fps", join, measured);
    }
    assert_seam(
        "twenty_five_fps",
        "whole_gap",
        max_deviation(
            &mixed,
            &expected,
            sample_frame_at(gap.start, fps) - window,
            sample_frame_at(gap.end, fps) + window,
        ),
    );
}

/// AU5 §0 R101: **the phase of the source range decides whether a fill covers
/// its gap**, and phase 0 at 30 → 25 does not.
///
/// This is the lane the previous one exists because of. `0..8` is a perfectly
/// legal fill — `clip_duration == 7`, no overlap, `validate()` passes — and it
/// still leaves 640 sample frames of silence before clip B, because the mixer
/// maps source samples to project samples one for one and the source runs out.
/// A caller that hard-codes `source_start = 0` and takes
/// `map_project_duration_to_source`'s smallest exact end ships exactly this,
/// which is why `covering_source_range_for_project_duration` exists and why the
/// seam lane above uses it.
#[test]
fn au5_a_zero_phase_fill_at_twenty_five_fps_falls_short_of_its_gap() {
    let fixture = SeamFixture::new("au5-seam-25fps-phase");
    let assets = fixture.assets();
    let fps = Rational::new(25, 1).unwrap();
    let gap = TimeCode(25)..TimeCode(32);
    let gap_frames = TimeCode(gap.end.0 - gap.start.0);

    let smallest =
        map_project_duration_to_source(TimeCode::ZERO, gap_frames, fixture.room_tone.fps, fps)
            .expect("phase 0 can express seven project frames");
    assert_eq!(smallest, TimeCode(8), "AU5 §5.4 rule 101(ii)");

    let document = seam_document(
        fps,
        &assets,
        vec![
            Piece {
                asset: AssetId(2),
                source: TimeCode(0)..TimeCode(30),
                start: TimeCode(0),
            },
            Piece {
                asset: AssetId(1),
                source: TimeCode(0)..smallest,
                start: gap.start,
            },
            Piece {
                asset: AssetId(2),
                source: TimeCode(60)..TimeCode(90),
                start: gap.end,
            },
        ],
    );
    let fill = &document.tracks[0].clips[1];
    assert_eq!(
        document.clip_duration(fill).unwrap(),
        gap_frames,
        "exact in project frames — which is exactly the trap",
    );

    let mixed =
        mix_audio(&document, &seam_settings(&document)).expect("the phase-0 document mixes");
    let channels = u64::from(ROOM_TONE_CHANNELS);
    let required = sample_frame_at(gap.end, fps) - sample_frame_at(gap.start, fps);
    let reference = reference_decode(&fixture.room_tone, TimeCode(0)..smallest);
    let supplied = reference.len() as u64 / channels;
    let residual = required - supplied;
    assert_eq!((required, supplied), (13_440, 12_800));

    let mut expected = reference_decode(&fixture.dialogue, TimeCode(0)..TimeCode(30));
    expected.extend_from_slice(&reference);
    expected.extend(std::iter::repeat_n(
        0.0_f32,
        usize::try_from(residual * channels).unwrap(),
    ));
    expected.extend(reference_decode(
        &fixture.dialogue,
        TimeCode(60)..TimeCode(90),
    ));
    assert_eq!(mixed.len(), expected.len());

    let window = seam_window();
    assert_seam(
        "twenty_five_fps_phase_zero",
        "whole_gap_against_model",
        max_deviation(
            &mixed,
            &expected,
            sample_frame_at(gap.start, fps) - window,
            sample_frame_at(gap.end, fps) + window,
        ),
    );

    #[allow(clippy::cast_precision_loss)]
    let residual_milliseconds = residual as f64 * 1_000.0 / f64::from(ROOM_TONE_SAMPLE_RATE);
    #[allow(clippy::cast_precision_loss)]
    let margin = ROOM_TONE_FILL_RESIDUAL_BUDGET_SAMPLE_FRAMES as f64 / residual as f64;
    println!(
        "AU5_ROOM_TONE_FILL_RESIDUAL lane=twenty_five_fps_phase_zero measured={residual} \
         measured_ms={residual_milliseconds:.3} \
         budget={ROOM_TONE_FILL_RESIDUAL_BUDGET_SAMPLE_FRAMES} margin={margin:.2}x",
    );
    assert_eq!(residual, 640, "8 x 1600 against 7 x 1920");
    assert!(
        margin >= FIXTURE_MINIMUM_MARGIN,
        "the residual {residual} left only {margin} against \
         {ROOM_TONE_FILL_RESIDUAL_BUDGET_SAMPLE_FRAMES}",
    );

    assert_eq!(
        covering_source_range_for_project_duration(
            gap_frames,
            fixture.room_tone.fps,
            fps,
            fixture.room_tone.duration,
            ROOM_TONE_SAMPLE_RATE,
        ),
        Ok(TimeCode(1)..TimeCode(10)),
    );
}

/// AU5 §7 B6 / §5.4 rule 101(iii): a **60 fps** project over a **7**-frame gap
/// has no exact source range, so the planner skips the gap with rule 97's
/// reason while the rest of the plan commits.
///
/// `map_frames(E, 30, 60) = round(2E)` is always even, so no source end maps to
/// an odd project duration — the window search is not merely unlucky, it is
/// searching a set that cannot contain the answer, which the exhaustive scan
/// below states rather than implies.
#[test]
fn au5_a_sixty_fps_gap_of_seven_frames_has_no_exact_source_range() {
    let source_fps = Rational::default();
    let project_fps = Rational::new(60, 1).unwrap();

    assert!(matches!(
        map_project_duration_to_source(TimeCode::ZERO, TimeCode(7), source_fps, project_fps),
        Err(TimeMappingError::InexactDuration {
            source_start: 0,
            project_duration: 7,
        }),
    ));
    assert!(matches!(
        covering_source_range_for_project_duration(
            TimeCode(7),
            source_fps,
            project_fps,
            TimeCode(60),
            ROOM_TONE_SAMPLE_RATE,
        ),
        Err(TimeMappingError::InexactDuration {
            project_duration: 7,
            ..
        }),
    ));
    for duration in 1..=120_i64 {
        let found = map_project_duration_to_source(
            TimeCode::ZERO,
            TimeCode(duration),
            source_fps,
            project_fps,
        )
        .ok();
        if duration % 2 == 0 {
            assert_eq!(
                found,
                Some(TimeCode(duration / 2)),
                "an even 60 fps duration is exactly half as many 30 fps frames",
            );
        } else {
            assert_eq!(found, None, "no source range maps to {duration} at 60 fps");
        }
    }
    assert!(
        (1..=240_i64).all(|candidate| {
            map_frames(TimeCode(candidate), source_fps, project_fps)
                .is_ok_and(|mapped| mapped.0 == candidate * 2 && mapped.0 % 2 == 0)
        }),
        "every 30 fps boundary maps to an even 60 fps frame, at every phase",
    );

    // The 25 fps neighbour, for contrast: the same search finds an answer.
    let twenty_five = Rational::new(25, 1).unwrap();
    assert_eq!(
        map_project_duration_to_source(TimeCode::ZERO, TimeCode(7), source_fps, twenty_five).ok(),
        Some(TimeCode(8)),
    );
    for duration in 1..=120_i64 {
        assert_eq!(
            map_project_duration_to_source(
                TimeCode::ZERO,
                TimeCode(duration),
                source_fps,
                source_fps
            )
            .ok(),
            Some(TimeCode(duration)),
        );
    }
}
