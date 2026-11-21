//! The cut's layout on the output clock, from the frozen expectations and
//! document only: which source each output frame and hop maps, which spans
//! the picture checks compare, and which they leave to the overlay check.
use std::ops::Range;

use kinewright_core::output_review::{
    KeptWord, ReviewExpectations, ReviewLane, ReviewSegment, ReviewSegmentId, ReviewSpan,
    SegmentSampling,
};
use kinewright_core::{
    BlendMode, Clip, ClipContent, Document, ExactMicros, MediaAsset, Rational, TimeCode,
    TitlePixelBounds, TrackId, TrackKind,
};

use super::{ExclusionReason, ReviewScanInputs};

/// Output clock on the expected frame grid.
#[derive(Debug, Clone, Copy)]
pub(super) struct Clock {
    fps: Rational,
}

impl Clock {
    pub(super) fn new(fps: Rational) -> Self {
        Self { fps }
    }

    /// Start of output frame `index`, in microseconds.
    pub(super) fn frame_us(self, index: u64) -> u64 {
        let value = u128::from(index) * 1_000_000 * u128::from(self.fps.denominator())
            / u128::from(self.fps.numerator().max(1));
        u64::try_from(value).unwrap_or(u64::MAX)
    }

    /// First frame presented at or after `at`.
    pub(super) fn first_frame_from(self, at: ExactMicros) -> u64 {
        let numerator = at.numerator() * u128::from(self.fps.numerator());
        let denominator =
            u128::from(at.denominator()) * 1_000_000 * u128::from(self.fps.denominator().max(1));
        u64::try_from(numerator.div_ceil(denominator.max(1))).unwrap_or(u64::MAX)
    }

    /// Frames presented inside `span`.
    pub(super) fn frames(self, span: &ReviewSpan) -> Range<u64> {
        self.first_frame_from(span.start)..self.first_frame_from(span.end)
    }

    pub(super) fn fps(self) -> Rational {
        self.fps
    }
}

/// Integer output samples of an exact instant at `rate`, rounded down.
pub(super) fn samples_at(at: ExactMicros, rate: u32) -> u64 {
    let value =
        at.numerator() * u128::from(rate) / (u128::from(at.denominator()) * 1_000_000).max(1);
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// One video segment as the picture checks see it.
pub(super) struct VideoPart<'a> {
    pub segment: &'a ReviewSegment,
    pub frames: Range<u64>,
    /// Position of the segment's track, bottom first.
    pub layer: usize,
    pub asset: Option<&'a MediaAsset>,
    /// Why no frame of this segment can be a plain mapping, if any.
    pub excluded: Option<ExclusionReason>,
    /// Project frames of the clip's incoming transition.
    pub transition: Option<Range<i64>>,
}

/// One audio segment as the sound checks see it.
pub(super) struct AudioPart<'a> {
    pub segment: &'a ReviewSegment,
    pub clip: Option<&'a Clip>,
    pub asset: Option<&'a MediaAsset>,
    /// Silent by the document (track mute, another track's solo, a disabled
    /// clip): the encode is expected silent here.
    pub muted: bool,
    /// Channels the track's pan sends nothing to.
    pub silent_channels: [bool; 2],
    /// A source-contiguous segment continues this one at its start / end.
    pub continued: [bool; 2],
    /// Why the encode's level here is not a fixed gain of the source, so the
    /// sound comparison does not run on this part.
    pub uncomparable: Option<&'static str>,
}

/// How the picture checks treat one output frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FrameUse {
    /// No video segment covers the frame.
    Empty,
    /// The overlay check owns the frame.
    Excluded(ExclusionReason),
    /// A plain mapping of one source frame, with cells overlays cover masked.
    Plain {
        part: usize,
        source_frame: i64,
        masked: Vec<bool>,
    },
}

/// Ranges active at a monotonically increasing frame.
pub(super) struct Sweep {
    order: Vec<usize>,
    next: usize,
    pub active: Vec<usize>,
}

impl Sweep {
    fn new(ranges: &[Range<u64>]) -> Self {
        let mut order: Vec<usize> = (0..ranges.len()).collect();
        order.sort_by_key(|&index| ranges[index].start);
        Self {
            order,
            next: 0,
            active: Vec::new(),
        }
    }

    /// Advance to `frame`; frames must not decrease.
    fn advance(&mut self, ranges: &[Range<u64>], frame: u64) {
        self.active.retain(|&index| ranges[index].end > frame);
        while let Some(&index) = self.order.get(self.next) {
            if ranges[index].start > frame {
                break;
            }
            self.next += 1;
            if ranges[index].end > frame {
                self.active.push(index);
            }
        }
    }
}

/// Monotonic per-frame view of the plan.
pub(super) struct FrameCursor {
    video_ranges: Vec<Range<u64>>,
    overlay_ranges: Vec<Range<u64>>,
    video: Sweep,
    overlays: Sweep,
}

impl FrameCursor {
    pub(super) fn new(plan: &Plan<'_>) -> Self {
        let video_ranges: Vec<_> = plan.video.iter().map(|part| part.frames.clone()).collect();
        let overlay_ranges: Vec<_> = plan.overlays.iter().map(|o| o.frames.clone()).collect();
        Self {
            video: Sweep::new(&video_ranges),
            overlays: Sweep::new(&overlay_ranges),
            video_ranges,
            overlay_ranges,
        }
    }

    pub(super) fn advance(&mut self, frame: u64) {
        self.video.advance(&self.video_ranges, frame);
        self.overlays.advance(&self.overlay_ranges, frame);
    }

    pub(super) fn video(&self) -> &[usize] {
        &self.video.active
    }

    pub(super) fn overlays(&self) -> &[usize] {
        &self.overlays.active
    }
}

pub(super) struct Overlay {
    pub frames: Range<u64>,
    pub bounds: Option<TitlePixelBounds>,
}

pub(super) struct Plan<'a> {
    pub clock: Clock,
    pub output_frames: u64,
    pub raster: (u32, u32),
    pub video: Vec<VideoPart<'a>>,
    pub audio: Vec<AudioPart<'a>>,
    pub overlays: Vec<Overlay>,
    pub project_fps: Rational,
    /// Every video-track clip by project-frame span and track layer, or
    /// `None` when a span could not be determined from the document.
    pictures: Option<Vec<Picture<'a>>>,
    /// Every title on output is in the overlay record, so its cells are
    /// masked rather than its frames excluded.
    titles_masked: bool,
    document: &'a Document,
}

fn find_clip(
    document: &Document,
    track: TrackId,
    clip: kinewright_core::ClipId,
) -> Option<(usize, &Clip)> {
    document
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.id == track)
        .find_map(|(layer, candidate)| {
            candidate
                .clips
                .iter()
                .find(|item| item.id == clip)
                .map(|item| (layer, item))
        })
}

/// One video-track clip on the project-frame grid.
struct Picture<'a> {
    frames: Range<i64>,
    clip: &'a Clip,
    layer: usize,
}

impl Picture<'_> {
    /// An adjustment layer changes what is below it but draws nothing.
    fn draws(&self) -> bool {
        !matches!(self.clip.content, ClipContent::Adjustment)
    }

    fn shown_at(&self, project: i64) -> bool {
        self.frames.contains(&project)
            && self
                .clip
                .is_enabled_at(TimeCode(project - self.frames.start))
    }
}

fn pictures(document: &Document) -> Option<Vec<Picture<'_>>> {
    let mut spans = Vec::new();
    for (layer, track) in document.tracks.iter().enumerate() {
        if track.kind != TrackKind::Video {
            continue;
        }
        for clip in &track.clips {
            let duration = document.clip_duration(clip).ok()?;
            let start = clip.timeline_start.0;
            spans.push(Picture {
                frames: start..start.checked_add(duration.0)?,
                clip,
                layer,
            });
        }
    }
    Some(spans)
}

fn asset(document: &Document, id: kinewright_core::AssetId) -> Option<&MediaAsset> {
    document.media_pool.iter().find(|asset| asset.id == id)
}

/// Why a kept audio segment's encode level is not a fixed gain of its source
/// times the clip's own gain shape. Constant gains anywhere in the mix are
/// measured, not modelled; anything that changes level over time or by
/// signal is not.
fn uncomparable(
    document: &Document,
    track: TrackId,
    clip: Option<&Clip>,
    mix: Option<&kinewright_core::TrackMix>,
) -> Option<&'static str> {
    let audio_effects = |effects: &[kinewright_core::Effect]| {
        effects
            .iter()
            .any(|effect| kinewright_core::is_audio_effect(&effect.name))
    };
    let clip = clip?;
    if clip.speed_percent != 100 {
        return Some("speed change");
    }
    if clip.enabled_curve.is_some() {
        return Some("enable automation");
    }
    if audio_effects(&clip.effects) {
        return Some("clip audio effects");
    }
    if mix.is_some_and(|mix| mix.gain_curve.is_some() || mix.pan_curve.is_some()) {
        return Some("track automation");
    }
    let mixer = &document.audio_mix;
    if mixer.buses.iter().any(|bus| {
        bus.tracks.contains(&track)
            && (bus.gain_curve.is_some()
                || audio_effects(&bus.effects)
                || !bus.ducking_sidechain_tracks.is_empty())
    }) {
        return Some("bus processing");
    }
    if mixer.master.gain_curve.is_some() || audio_effects(&mixer.master.effects) {
        return Some("master processing");
    }
    None
}

/// Pan law endpoints: a hard-panned track sends nothing to one channel.
fn silent_channels(pan_percent: i32) -> [bool; 2] {
    [pan_percent >= 100, pan_percent <= -100]
}

impl<'a> Plan<'a> {
    #[allow(
        clippy::too_many_lines,
        reason = "one pass over the frozen segments builds every part"
    )]
    pub(super) fn new(inputs: &ReviewScanInputs<'a>) -> Self {
        let expectations: &'a ReviewExpectations = inputs.expectations;
        let document: &'a Document = inputs.document;
        let clock = Clock::new(expectations.output_fps);
        let video = expectations
            .segments
            .iter()
            .filter(|segment| segment.id.lane == ReviewLane::Video)
            .map(|segment| {
                let found = find_clip(document, segment.track, segment.id.clip);
                let asset = asset(document, segment.asset);
                let excluded = match found {
                    None => Some(ExclusionReason::Generated),
                    Some((_, clip)) if !matches!(clip.content, ClipContent::Media) => {
                        Some(ExclusionReason::Generated)
                    }
                    Some((_, clip))
                        if !clip.effects.is_empty() || clip.blend_mode != BlendMode::Normal =>
                    {
                        Some(ExclusionReason::Effect)
                    }
                    Some(_) => None,
                };
                let transition = found.and_then(|(_, clip)| {
                    clip.transition_in.as_ref().map(|transition| {
                        clip.timeline_start.0
                            ..clip.timeline_start.0.saturating_add(transition.duration.0)
                    })
                });
                VideoPart {
                    segment,
                    frames: clock.frames(&segment.output),
                    layer: found.map_or(0, |(layer, _)| layer),
                    asset,
                    excluded,
                    transition,
                }
            })
            .collect();
        let soloed = document.audio_mix.tracks.iter().any(|mix| mix.solo);
        let audio_segments: Vec<&'a ReviewSegment> = expectations
            .segments
            .iter()
            .filter(|segment| segment.id.lane == ReviewLane::Audio)
            .collect();
        // A source-contiguous continuation shares track, asset, output instant
        // and source instant.
        let key = |segment: &ReviewSegment, output: ExactMicros, source: ExactMicros| {
            (
                segment.track,
                segment.asset,
                (output.numerator(), output.denominator()),
                (source.numerator(), source.denominator()),
            )
        };
        let ends: std::collections::BTreeSet<_> = audio_segments
            .iter()
            .map(|segment| key(segment, segment.output.end, segment.source.end))
            .collect();
        let starts: std::collections::BTreeSet<_> = audio_segments
            .iter()
            .map(|segment| key(segment, segment.output.start, segment.source.start))
            .collect();
        let audio = audio_segments
            .iter()
            .map(|&segment| {
                let found = find_clip(document, segment.track, segment.id.clip);
                let clip = found.map(|(_, clip)| clip);
                let mix = document
                    .audio_mix
                    .tracks
                    .iter()
                    .find(|mix| mix.track == segment.track);
                let audio_track = document
                    .tracks
                    .iter()
                    .find(|track| track.id == segment.track)
                    .is_some_and(|track| {
                        track.kind == TrackKind::Audio || track.kind == TrackKind::Video
                    });
                let muted = !audio_track
                    || mix.is_some_and(|mix| mix.mute)
                    || (soloed && !mix.is_some_and(|mix| mix.solo))
                    || clip.is_none_or(|clip| !clip.enabled);
                let uncomparable = uncomparable(document, segment.track, clip, mix);
                let continued = [
                    ends.contains(&key(segment, segment.output.start, segment.source.start)),
                    starts.contains(&key(segment, segment.output.end, segment.source.end)),
                ];
                AudioPart {
                    segment,
                    clip,
                    asset: asset(document, segment.asset),
                    muted,
                    silent_channels: mix.map_or([false; 2], |mix| {
                        if mix.pan_curve.is_some() {
                            [false; 2]
                        } else {
                            silent_channels(mix.pan_percent)
                        }
                    }),
                    continued,
                    uncomparable,
                }
            })
            .collect();
        let overlays = expectations
            .overlays
            .captions
            .known()
            .chain(expectations.overlays.titles.known())
            .chain(expectations.overlays.cards.known())
            .flat_map(|overlay| overlay.visible.iter())
            .map(|run| Overlay {
                frames: clock.frames(&run.output),
                bounds: run.bounds.as_ref().ok().copied(),
            })
            .collect();
        Self {
            clock,
            output_frames: u64::try_from(expectations.output_frames.0).unwrap_or(0),
            raster: expectations.raster,
            video,
            audio,
            overlays,
            project_fps: expectations.project_fps,
            pictures: pictures(document),
            titles_masked: expectations.overlays.is_resolved(),
            document,
        }
    }

    /// Whether a document clip draws a picture on output frame `frame`, or
    /// `None` when the document's spans could not be determined.
    pub(super) fn pictured(&self, frame: u64) -> Option<bool> {
        let project = self.project_frame(frame);
        Some(
            self.pictures
                .as_ref()?
                .iter()
                .any(|picture| picture.draws() && picture.shown_at(project)),
        )
    }

    /// Whether another clip on a higher layer (a picture or an adjustment)
    /// changes output frame `frame` of `part`; `None` when undetermined.
    /// Titles are not counted when the overlay record masks their cells.
    fn covered_above(&self, part: &VideoPart<'_>, frame: u64) -> Option<bool> {
        let project = self.project_frame(frame);
        Some(self.pictures.as_ref()?.iter().any(|picture| {
            picture.layer > part.layer
                && picture.clip.id != part.segment.id.clip
                && !(self.titles_masked && matches!(picture.clip.content, ClipContent::Title(_)))
                && picture.shown_at(project)
        }))
    }

    pub(super) fn document(&self) -> &'a Document {
        self.document
    }

    /// The project frame output frame `frame` samples.
    fn project_frame(&self, frame: u64) -> i64 {
        let output = self.clock.fps();
        let value = i128::from(frame)
            * i128::from(self.project_fps.numerator())
            * i128::from(output.denominator())
            / (i128::from(self.project_fps.denominator()) * i128::from(output.numerator())).max(1);
        i64::try_from(value).unwrap_or(i64::MAX)
    }

    /// The source frame a plain segment maps output frame `frame` to.
    pub(super) fn source_frame(&self, part: &VideoPart<'_>, frame: u64) -> Option<i64> {
        match part.segment.sampling {
            SegmentSampling::Video {
                clip_start,
                source_in,
                source_end,
                effective_fps,
                ..
            } => {
                let offset = i128::from(self.project_frame(frame) - clip_start.0);
                let local = offset
                    * i128::from(effective_fps.numerator())
                    * i128::from(self.project_fps.denominator())
                    / (i128::from(effective_fps.denominator())
                        * i128::from(self.project_fps.numerator()))
                    .max(1);
                let source = i128::from(source_in.0) + local;
                let last = i128::from(source_end.0) - 1;
                i64::try_from(source.min(last).max(i128::from(source_in.0))).ok()
            }
            SegmentSampling::VideoHold { frame, .. } => Some(frame.0),
            SegmentSampling::Audio => None,
        }
    }

    /// The source frame rate a video segment's frames are counted in.
    pub(super) fn source_fps(part: &VideoPart<'_>) -> Option<Rational> {
        match part.segment.sampling {
            SegmentSampling::Video { source_fps, .. }
            | SegmentSampling::VideoHold { source_fps, .. } => Some(source_fps),
            SegmentSampling::Audio => None,
        }
    }

    /// How the picture checks use output frame `frame`; `cursor` must have
    /// been advanced to it.
    pub(super) fn frame_use(&self, frame: u64, cursor: &FrameCursor) -> FrameUse {
        let (index, part) = match cursor.video() {
            // A generated clip (title, solid, held frame without a mapped
            // segment) has no source to compare against.
            [] if self.pictured(frame) != Some(false) => {
                return FrameUse::Excluded(ExclusionReason::Generated);
            }
            [] => return FrameUse::Empty,
            [index] => (*index, &self.video[*index]),
            _ => return FrameUse::Excluded(ExclusionReason::Composited),
        };
        if let Some(reason) = part.excluded {
            return FrameUse::Excluded(reason);
        }
        // Not a plain mapping: something on a higher layer changes the frame,
        // or the layers could not be determined.
        if self.covered_above(part, frame) != Some(false) {
            return FrameUse::Excluded(ExclusionReason::Composited);
        }
        let project = self.project_frame(frame);
        if part
            .transition
            .as_ref()
            .is_some_and(|window| window.contains(&project))
        {
            return FrameUse::Excluded(ExclusionReason::Transition);
        }
        let (columns, rows) = super::LUMA_GRID;
        let mut masked = vec![false; columns * rows];
        let (width, height) = (
            i64::from(self.raster.0.max(1)),
            i64::from(self.raster.1.max(1)),
        );
        let columns_i = i64::try_from(columns).unwrap_or(1);
        let rows_i = i64::try_from(rows).unwrap_or(1);
        for overlay in cursor.overlays().iter().map(|&index| &self.overlays[index]) {
            let Some(bounds) = overlay.bounds else {
                return FrameUse::Excluded(ExclusionReason::OverlayUnresolved);
            };
            let column = |x: i32| (i64::from(x) * columns_i / width).clamp(0, columns_i);
            let row = |y: i32| (i64::from(y) * rows_i / height).clamp(0, rows_i);
            // Every cell the bounds touch, including partly covered ones.
            let (left, right) = (column(bounds.left), column(bounds.right - 1) + 1);
            let (top, bottom) = (row(bounds.top), row(bounds.bottom - 1) + 1);
            for cell_row in top..bottom.min(rows_i) {
                for cell_column in left..right.min(columns_i) {
                    let cell = usize::try_from(cell_row * columns_i + cell_column).unwrap_or(0);
                    masked[cell] = true;
                }
            }
        }
        if masked.iter().all(|&cell| cell) {
            return FrameUse::Excluded(ExclusionReason::OverlayCovered);
        }
        match self.source_frame(part, frame) {
            Some(source_frame) => FrameUse::Plain {
                part: index,
                source_frame,
                masked,
            },
            None => FrameUse::Excluded(ExclusionReason::Generated),
        }
    }

    /// The topmost video segment at the cursor's frame, by track layer.
    pub(super) fn top_asset(&self, cursor: &FrameCursor) -> Option<kinewright_core::AssetId> {
        cursor
            .video()
            .iter()
            .map(|&index| &self.video[index])
            .max_by_key(|part| part.layer)
            .map(|part| part.segment.asset)
    }

    pub(super) fn audio_part(&self, id: &ReviewSegmentId) -> Option<usize> {
        self.audio.iter().position(|part| part.segment.id == *id)
    }

    /// The source span a kept word occupies, from its output span and the
    /// audio segment's 1:1 placement.
    pub(super) fn word_source(&self, word: &KeptWord) -> Option<(usize, Range<ExactMicros>)> {
        let index = self.audio_part(&word.segment)?;
        let segment = self.audio[index].segment;
        let map = |at: ExactMicros| {
            at.checked_sub(segment.output.start)
                .ok()?
                .checked_add(segment.source.start)
                .ok()
        };
        Some((index, map(word.output.start)?..map(word.output.end)?))
    }

    /// Exact duration of an asset in microseconds.
    pub(super) fn asset_duration(asset: &MediaAsset) -> Option<ExactMicros> {
        ExactMicros::frames(asset.duration, asset.fps).ok()
    }

    /// Project duration of a clip, for its gain shaping.
    pub(super) fn clip_duration(&self, clip: &Clip) -> TimeCode {
        self.document.clip_duration(clip).unwrap_or(TimeCode::ZERO)
    }
}
