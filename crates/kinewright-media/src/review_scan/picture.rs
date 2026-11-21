//! The picture pass: decode the recorded video stream of the encode frame by
//! frame and, where the plan says a frame is a plain mapping of one source
//! frame, decode that source frame from the pinned input and compare their
//! luma grids. Only per-frame statistics are kept.
use std::fs::File;
use std::ops::Range;

use ffmpeg_next as ffmpeg;
use kinewright_core::{
    AssetId, ColorDescription, ColorTransfer, ExportCancellation, MediaAsset, MediaError, TimeCode,
};

use super::grid::{self, RgbaScaler, Stats};
use super::plan::{FrameCursor, FrameUse, Plan};
use super::reader::{frame_step, open_input};
use super::{
    ExclusionReason, MAX_OPEN_SOURCES, MAX_TIMING_EVENTS, ReviewScanInputs, TechnicalScanConfig,
};
use crate::cache::FrameCache;
use crate::decode::{VideoDecoder, backend};
use crate::frame::WorkingFrame;

/// Width source pictures are decoded at; the grid is coarser still, each
/// cell the mean of a 4×4 block at 16:9 (R7).
const SOURCE_WIDTH: u32 = 128;
/// Source frames decoded per window.
const SOURCE_WINDOW_FRAMES: i64 = 32;
const SOURCE_CACHE_FRAMES: usize = 48;
/// The decoder holds a stream's last frame to every later window end; a
/// mapped frame more than this many frames past it lies past the source's
/// real end and is not compared (R4).
const SOURCE_HOLD_FRAMES: i64 = 4;
/// Distinct not-compared reasons kept; later ones share the last slot.
pub(super) const MAX_NOT_COMPARED_REASONS: usize = 16;
/// Largest relative aspect difference between a source and the raster that
/// is still a plain full-frame mapping.
const ASPECT_TOLERANCE: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameState {
    /// No video segment covers the frame.
    Empty,
    /// The overlay check owns the frame.
    Excluded(ExclusionReason),
    /// Compared with its mapped source frame.
    Compared,
    /// A plain mapping whose source could not be compared, by reason index.
    NotCompared(u16),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FrameRecord {
    pub state: FrameState,
    /// Video part index, for plain frames.
    pub part: u32,
    pub source_frame: i64,
    pub out: Stats,
    /// Change from the previous output frame over this frame's cells.
    pub out_diff: Option<f32>,
    pub src: Stats,
    /// Change from the previous compared source frame of the same part.
    pub src_diff: Option<f32>,
    /// Mean absolute output-minus-source difference.
    pub mad: f32,
    pub corr: Option<f32>,
}

impl FrameRecord {
    fn new(state: FrameState) -> Self {
        Self {
            state,
            part: u32::MAX,
            source_frame: 0,
            out: Stats::default(),
            out_diff: None,
            src: Stats::default(),
            src_diff: None,
            mad: 0.0,
            corr: None,
        }
    }
}

pub(super) struct PictureScan {
    pub frames: Vec<FrameRecord>,
    /// Stopped at the frame cap: the frames after it were not compared.
    pub truncated: bool,
    pub size: (u32, u32),
    pub color: Option<ColorDescription>,
    pub time_base: (i32, i32),
    pub decoder: String,
    pub first_pts: Option<i64>,
    /// Frames whose PTS is off `first + index × step`.
    pub off_grid: Vec<u64>,
    pub off_grid_count: u64,
    pub reasons: Vec<String>,
    /// Seeks the source decoders made; a straight run of one part
    /// continues its windows without one.
    pub source_seeks: u64,
    /// Video streams in the container.
    pub video_streams: u32,
}

impl PictureScan {
    fn intern(&mut self, reason: String) -> u16 {
        if let Some(index) = self.reasons.iter().position(|known| *known == reason) {
            return u16::try_from(index).unwrap_or(u16::MAX);
        }
        if self.reasons.len() + 1 >= MAX_NOT_COMPARED_REASONS {
            if self.reasons.len() + 1 == MAX_NOT_COMPARED_REASONS {
                self.reasons.push("further reasons not listed".into());
            }
            return u16::try_from(MAX_NOT_COMPARED_REASONS - 1).unwrap_or(u16::MAX);
        }
        self.reasons.push(reason);
        u16::try_from(self.reasons.len() - 1).unwrap_or(u16::MAX)
    }
}

pub(super) fn scan(
    artifact: &File,
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    config: &TechnicalScanConfig,
    cancellation: &ExportCancellation,
) -> Result<PictureScan, MediaError> {
    let mut input = open_input(artifact, cancellation)?;
    let video_streams = input
        .streams()
        .filter(|stream| {
            stream.parameters().medium() == ffmpeg::media::Type::Video
                && !stream
                    .disposition()
                    .contains(ffmpeg::format::stream::Disposition::ATTACHED_PIC)
        })
        .count();
    let index = inputs.timing.video.index;
    let stream = input
        .stream(index)
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Video)
        .ok_or_else(|| MediaError::Backend("recorded video stream is missing".into()))?;
    let time_base = (
        stream.time_base().numerator(),
        stream.time_base().denominator(),
    );
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .and_then(|context| context.decoder().video())
        .map_err(backend)?;
    let mut state = Picture {
        scan: PictureScan {
            frames: Vec::new(),
            truncated: false,
            size: (0, 0),
            color: None,
            time_base,
            decoder: decoder
                .codec()
                .map(|codec| codec.name().to_owned())
                .unwrap_or_default(),
            first_pts: None,
            off_grid: Vec::new(),
            off_grid_count: 0,
            reasons: Vec::new(),
            source_seeks: 0,
            video_streams: u32::try_from(video_streams).unwrap_or(u32::MAX),
        },
        step: frame_step(time_base, inputs.expectations.output_fps),
        plan,
        config,
        inputs,
        cursor: FrameCursor::new(plan),
        scaler: RgbaScaler::default(),
        sources: Sources::default(),
        previous_out: Vec::new(),
        previous_src: None,
    };
    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) if packet.stream() == index => {
                decoder.send_packet(&packet).map_err(backend)?;
                state.receive(&mut decoder, cancellation)?;
            }
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(backend(error)),
        }
        if state.scan.truncated {
            return Ok(state.scan);
        }
    }
    decoder.send_eof().map_err(backend)?;
    state.receive(&mut decoder, cancellation)?;
    Ok(state.scan)
}

struct Picture<'p, 'a> {
    scan: PictureScan,
    step: Option<i64>,
    plan: &'p Plan<'a>,
    config: &'p TechnicalScanConfig,
    inputs: &'p ReviewScanInputs<'a>,
    cursor: FrameCursor,
    scaler: RgbaScaler,
    sources: Sources,
    previous_out: Vec<f32>,
    /// The previous compared frame's part and source grid.
    previous_src: Option<(usize, Vec<f32>)>,
}

impl Picture<'_, '_> {
    fn receive(
        &mut self,
        decoder: &mut ffmpeg::decoder::Video,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        let mut frame = ffmpeg::frame::Video::empty();
        loop {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            match decoder.receive_frame(&mut frame) {
                Ok(()) => self.frame(decoder, &frame, cancellation)?,
                Err(
                    ffmpeg::Error::Eof
                    | ffmpeg::Error::Other {
                        errno: ffmpeg::error::EAGAIN,
                    },
                ) => return Ok(()),
                Err(error) => return Err(backend(error)),
            }
            if self.scan.truncated {
                return Ok(());
            }
        }
    }

    fn timing(
        &mut self,
        decoder: &ffmpeg::decoder::Video,
        frame: &ffmpeg::frame::Video,
        index: u64,
    ) {
        let scan = &mut self.scan;
        if scan.color.is_none() {
            scan.size = (frame.width(), frame.height());
            scan.color = Some(crate::decode::color_description_from_decoder(
                decoder,
                Some(frame.format()),
            ));
        }
        let pts = frame.pts().or_else(|| frame.timestamp());
        let first = *scan.first_pts.get_or_insert(pts.unwrap_or(0));
        let on_grid = match (pts, self.step) {
            (Some(pts), Some(step)) => {
                i64::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_mul(step))
                    .and_then(|offset| first.checked_add(offset))
                    == Some(pts)
            }
            _ => false,
        };
        if !on_grid {
            scan.off_grid_count += 1;
            if scan.off_grid.len() < MAX_TIMING_EVENTS {
                scan.off_grid.push(index);
            }
        }
    }

    fn frame(
        &mut self,
        decoder: &ffmpeg::decoder::Video,
        frame: &ffmpeg::frame::Video,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        let index = u64::try_from(self.scan.frames.len()).unwrap_or(u64::MAX);
        if index >= self.config.max_frames {
            self.scan.truncated = true;
            return Ok(());
        }
        self.timing(decoder, frame, index);
        let out = grid::from_frame(frame, &mut self.scaler)?;
        self.cursor.advance(index);
        let use_ = self.plan.frame_use(index, &self.cursor);
        let no_mask = vec![false; grid::CELLS];
        let mask = match &use_ {
            FrameUse::Plain { masked, .. } => masked.as_slice(),
            _ => no_mask.as_slice(),
        };
        let out_diff = (!self.previous_out.is_empty())
            .then(|| grid::difference(&out, &self.previous_out, mask));
        let mut record = match use_ {
            FrameUse::Empty => FrameRecord::new(FrameState::Empty),
            FrameUse::Excluded(reason) => FrameRecord::new(FrameState::Excluded(reason)),
            FrameUse::Plain {
                part,
                source_frame,
                ref masked,
            } => self.compare(part, source_frame, &out, masked, cancellation)?,
        };
        if !matches!(record.state, FrameState::Compared) {
            self.previous_src = None;
        }
        record.out = grid::stats(&out, mask);
        record.out_diff = out_diff;
        self.previous_out = out;
        self.scan.frames.push(record);
        Ok(())
    }

    fn compare(
        &mut self,
        part: usize,
        source_frame: i64,
        out: &[f32],
        masked: &[bool],
        cancellation: &ExportCancellation,
    ) -> Result<FrameRecord, MediaError> {
        let video = &self.plan.video[part];
        let not_compared = |scan: &mut PictureScan, reason: String| {
            let mut record = FrameRecord::new(FrameState::NotCompared(scan.intern(reason)));
            record.part = u32::try_from(part).unwrap_or(u32::MAX);
            record.source_frame = source_frame;
            record
        };
        let Some(asset) = video.asset else {
            return Ok(not_compared(
                &mut self.scan,
                "the segment's asset is not in the document".into(),
            ));
        };
        if Plan::source_fps(video) != Some(asset.fps) {
            return Ok(not_compared(
                &mut self.scan,
                "the segment counts source frames at another rate than the asset".into(),
            ));
        }
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let source = match self.sources.grid(
            self.inputs,
            asset,
            source_frame,
            self.plan.raster,
            &mut self.scan.source_seeks,
        ) {
            Ok(source) => source,
            Err(reason) => return Ok(not_compared(&mut self.scan, reason)),
        };
        let src_diff = match &self.previous_src {
            Some((previous_part, previous)) if *previous_part == part => {
                Some(grid::difference(&source, previous, masked))
            }
            _ => None,
        };
        let record = FrameRecord {
            state: FrameState::Compared,
            part: u32::try_from(part).unwrap_or(u32::MAX),
            source_frame,
            out: Stats::default(),
            out_diff: None,
            src: grid::stats(&source, masked),
            src_diff,
            mad: grid::difference(out, &source, masked),
            corr: grid::correlation(out, &source, masked, self.config.texture_min),
        };
        self.previous_src = Some((part, source));
        Ok(record)
    }
}

struct SourceVideo {
    asset: AssetId,
    decoder: VideoDecoder,
    cache: FrameCache<WorkingFrame>,
    window: Option<Range<i64>>,
    used: u64,
}

/// Pinned source decoders, at most `MAX_OPEN_SOURCES` open; a source that
/// failed keeps its reason.
#[derive(Default)]
struct Sources {
    open: Vec<SourceVideo>,
    failed: Vec<(AssetId, String)>,
    tick: u64,
}

fn aspect_matches(size: (u32, u32), raster: (u32, u32)) -> bool {
    let source = f64::from(size.0) / f64::from(size.1.max(1));
    let raster = f64::from(raster.0) / f64::from(raster.1.max(1));
    ((source - raster) / raster).abs() <= ASPECT_TOLERANCE
}

impl Sources {
    /// The luma grid of source frame `frame`, or why it cannot be compared.
    fn grid(
        &mut self,
        inputs: &ReviewScanInputs<'_>,
        asset: &MediaAsset,
        frame: i64,
        raster: (u32, u32),
        seeks: &mut u64,
    ) -> Result<Vec<f32>, String> {
        if let Some((_, reason)) = self.failed.iter().find(|(id, _)| *id == asset.id) {
            return Err(reason.clone());
        }
        self.tick += 1;
        let position = match self.open.iter().position(|open| open.asset == asset.id) {
            Some(position) => position,
            None => match Self::open_source(inputs, asset, raster) {
                Ok(opened) => {
                    if self.open.len() >= MAX_OPEN_SOURCES
                        && let Some(oldest) =
                            (0..self.open.len()).min_by_key(|&i| self.open[i].used)
                    {
                        self.open.swap_remove(oldest);
                    }
                    self.open.push(opened);
                    self.open.len() - 1
                }
                Err(reason) => {
                    self.failed.push((asset.id, reason.clone()));
                    return Err(reason);
                }
            },
        };
        let source = &mut self.open[position];
        source.used = self.tick;
        if source
            .window
            .as_ref()
            .is_none_or(|window| !window.contains(&frame))
        {
            // The decoder's window end is inclusive and its continuation
            // starts one past it, so the next window continues without a
            // seek.
            let last = frame.saturating_add(SOURCE_WINDOW_FRAMES - 1);
            let before = source.decoder.seek_count();
            let decoded = source.decoder.decode_window_sequential(
                TimeCode(frame),
                TimeCode(last),
                &mut source.cache,
            );
            *seeks += source.decoder.seek_count().saturating_sub(before);
            if let Err(error) = decoded {
                let reason = format!("source decode failed: {}", super::reason(&error));
                self.open.swap_remove(position);
                self.failed.push((asset.id, reason.clone()));
                return Err(reason);
            }
            source.window = Some(frame..last.saturating_add(1));
        }
        // R4: at the end of its stream the decoder holds the last frame to
        // the window end; a mapping further past it has no source frame.
        let past_end = source
            .decoder
            .held_at_eof()
            .is_some_and(|held| frame.saturating_sub(held) > SOURCE_HOLD_FRAMES);
        if past_end {
            return Err("the source has no frame at the mapped time".to_owned());
        }
        let texture = source
            .cache
            .frame_at_or_before(TimeCode(frame))
            .ok_or_else(|| "the source has no frame at the mapped time".to_owned())?;
        grid::from_working(texture.width, texture.height, &texture.pixels)
            .ok_or_else(|| "the source frame is empty".to_owned())
    }

    fn open_source(
        inputs: &ReviewScanInputs<'_>,
        asset: &MediaAsset,
        raster: (u32, u32),
    ) -> Result<SourceVideo, String> {
        let certified = inputs
            .sources
            .source(asset.id)
            .map_err(|error| format!("source is not pinned: {}", super::reason(&error)))?;
        // R7: the export's own managed decode (scene-linear working frames
        // from the asset's colour description), so range, depth and matrix
        // are handled as the export handles them.
        if inputs.settings.delivery_color.transfer != ColorTransfer::Bt709 {
            return Err("the delivery transfer is not modelled".into());
        }
        let decoder = VideoDecoder::open_certified(
            &asset.path,
            asset.fps,
            Some(SOURCE_WIDTH),
            &asset.color_description,
            crate::render::d65_assumption(&asset.color_description),
            &certified,
            None,
        )
        .map_err(|error| format!("source does not open: {}", super::reason(&error)))?;
        // R6: the true source dimensions, not the floored thumbnail's. A
        // source of another aspect is placed by a fit the check does not
        // model, so its frames stay not compared (Unknown).
        if !aspect_matches(decoder.source_display_size(), raster) {
            return Err(
                "the source's aspect differs from the raster; its placement is not modelled".into(),
            );
        }
        Ok(SourceVideo {
            asset: asset.id,
            decoder,
            cache: FrameCache::new(SOURCE_CACHE_FRAMES),
            window: None,
            used: 0,
        })
    }
}
