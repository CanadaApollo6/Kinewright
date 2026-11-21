//! The sound pass: decode the recorded audio stream of the encode into
//! per-channel 10 ms levels, and decode every comparable kept audio segment
//! from its pinned source at the output rate, shaped by the clip's own gain,
//! into the same hops. Only per-hop levels are kept.
use std::fs::File;

use kinewright_core::{
    AudioLoudness, Document, ExactMicros, ExportCancellation, MediaError, TrackId,
};

use super::plan::{Plan, samples_at};
use super::reader::{AudioReader, Placement, open_input};
use super::{MAX_SOURCE_HOP_ENTRIES, MAX_TIMING_EVENTS, ReviewScanInputs, TechnicalScanConfig};
use crate::audio::{AudioDecoder, ClipAudioShaping};

/// Digital zero in centi-dB.
pub(super) const ZERO_CDB: i16 = i16::MIN;

/// RMS in centi-dB full scale; digital zero is `ZERO_CDB`.
pub(super) fn centi_db(rms: f64) -> i16 {
    if rms <= 0.0 {
        return ZERO_CDB;
    }
    #[allow(clippy::cast_possible_truncation, reason = "clamped to the i16 range")]
    let value = (2_000.0 * rms.log10()).round().clamp(-32_000.0, 32_000.0) as i16;
    value
}

/// Linear amplitude of a centi-dB level.
pub(super) fn linear(cdb: i16) -> f64 {
    if cdb == ZERO_CDB {
        0.0
    } else {
        10_f64.powf(f64::from(cdb) / 2_000.0)
    }
}

/// One hop's level per channel: RMS and the peak absolute second
/// difference, both in centi-dB. Mono fills channel 0 only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HopLevel {
    pub rms: [i16; 2],
    pub second: [i16; 2],
}

impl HopLevel {
    pub(super) const SILENT: Self = Self {
        rms: [ZERO_CDB; 2],
        second: [ZERO_CDB; 2],
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimingEvent {
    Gap { at: i64, samples: u64 },
    Overlap { at: i64, samples: u64 },
}

/// The encode's audio.
pub(super) struct OutputSound {
    pub rate: u32,
    pub channels: u16,
    pub frame_size: u32,
    pub time_base: (i32, i32),
    pub decoder: String,
    /// Output sample position of the first decoded sample.
    pub first: Option<i64>,
    /// Output sample position after the last decoded sample.
    pub end: i64,
    pub decoded_samples: u64,
    /// Samples decoded before output sample zero.
    pub early_samples: u64,
    pub events: Vec<TimingEvent>,
    pub event_count: u64,
    /// A decoded frame after the first had no PTS.
    pub missing_pts: bool,
    /// One hop per 10 ms from output sample zero.
    pub hops: Vec<HopLevel>,
    /// Stopped at the hop cap: later audio was not decoded.
    pub truncated: bool,
    pub loudness: Result<AudioLoudness, String>,
}

/// One kept part's shaped source level over one output hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceHop {
    pub hop: u32,
    pub part: u32,
    pub level: HopLevel,
}

/// The mapped sources' levels on the output hop clock.
#[derive(Debug, Default)]
pub(super) struct SourceSound {
    /// Sorted by part, then hop.
    pub hops: Vec<SourceHop>,
    /// Parts whose source could not be decoded, with why.
    pub failed: Vec<(usize, String)>,
    /// The entry cap stopped the source decode.
    pub truncated: bool,
    /// Static mix gain in dB (track, buses, master) per part.
    pub static_db: Vec<[f64; 2]>,
}

pub(super) struct SoundScan {
    /// Audio streams in the container.
    pub streams: u32,
    pub output: Option<OutputSound>,
    pub sources: SourceSound,
}

/// Integer hop boundaries: hop `i` starts at `ceil(i × rate / 100)`.
fn hop_start(hop: u64, rate: u32) -> i64 {
    let step = u128::from(super::SCAN_HOP_MICROS) * u128::from(rate);
    i64::try_from((u128::from(hop) * step).div_ceil(1_000_000)).unwrap_or(i64::MAX)
}

/// Accumulates interleaved samples into per-channel hops from position 0.
struct HopMeter {
    rate: u32,
    channels: usize,
    hops: Vec<HopLevel>,
    max_hops: u64,
    index: u64,
    next_boundary: i64,
    squares: [f64; 2],
    peak: [f64; 2],
    count: u64,
    history: [[f32; 2]; 2],
    seen: u8,
    truncated: bool,
}

impl HopMeter {
    fn new(rate: u32, channels: usize, max_hops: u64) -> Self {
        Self {
            rate,
            channels: channels.min(2),
            hops: Vec::new(),
            max_hops,
            index: 0,
            next_boundary: hop_start(1, rate),
            squares: [0.0; 2],
            peak: [0.0; 2],
            count: 0,
            history: [[0.0; 2]; 2],
            seen: 0,
            truncated: false,
        }
    }

    fn close(&mut self) {
        let mut level = HopLevel::SILENT;
        if self.count > 0 {
            // A partly covered hop carries its power over the whole hop, so
            // levels of parts that share a hop add up to the output's.
            let length = hop_start(self.index + 1, self.rate) - hop_start(self.index, self.rate);
            #[allow(
                clippy::cast_precision_loss,
                reason = "a hop is at most a few thousand samples"
            )]
            let count = self.count.max(length.unsigned_abs()) as f64;
            for channel in 0..self.channels {
                level.rms[channel] = centi_db((self.squares[channel] / count).sqrt());
                level.second[channel] = centi_db(self.peak[channel]);
            }
        }
        self.hops.push(level);
        self.squares = [0.0; 2];
        self.peak = [0.0; 2];
        self.count = 0;
        self.index += 1;
        self.next_boundary = hop_start(self.index + 1, self.rate);
        if self.index >= self.max_hops {
            self.truncated = true;
        }
    }

    /// Advance to `position`, closing every hop before it. Bounded by the
    /// hop cap however far a PTS jumps (F22).
    fn advance_to(&mut self, position: i64) {
        while !self.truncated && position >= self.next_boundary {
            self.close();
        }
    }

    /// Digital silence over `count` samples from `position`.
    fn silence(&mut self, position: i64, count: u64) {
        let end = position.saturating_add(i64::try_from(count).unwrap_or(i64::MAX));
        self.advance_to(end);
        self.history = [[0.0; 2]; 2];
    }

    fn samples(&mut self, start: i64, interleaved: &[f32], channels: usize) {
        for (offset, frame) in interleaved.chunks_exact(channels.max(1)).enumerate() {
            let position = start.saturating_add(i64::try_from(offset).unwrap_or(i64::MAX));
            if position < 0 {
                continue;
            }
            self.advance_to(position);
            if self.truncated {
                return;
            }
            for channel in 0..self.channels {
                let value = frame[channel];
                self.squares[channel] += f64::from(value) * f64::from(value);
                if self.seen >= 2 {
                    let [older, old] = self.history;
                    let second = f64::from(value) - 2.0 * f64::from(old[channel])
                        + f64::from(older[channel]);
                    self.peak[channel] = self.peak[channel].max(second.abs());
                }
            }
            let mut current = [0.0; 2];
            current[..self.channels].copy_from_slice(&frame[..self.channels]);
            self.history = [self.history[1], current];
            self.seen = self.seen.saturating_add(1).min(2);
            self.count += 1;
        }
    }

    fn finish(mut self) -> (Vec<HopLevel>, bool) {
        if self.count > 0 && !self.truncated {
            self.close();
        }
        (self.hops, self.truncated)
    }
}

pub(super) fn scan(
    artifact: &File,
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    config: &TechnicalScanConfig,
    cancellation: &ExportCancellation,
) -> Result<SoundScan, MediaError> {
    let Some(stream) = &inputs.timing.audio else {
        let input = open_input(artifact, cancellation)?;
        return Ok(SoundScan {
            streams: super::reader::audio_streams(&input),
            output: None,
            sources: SourceSound::default(),
        });
    };
    let mut reader = AudioReader::open(artifact, inputs.timing, stream.index, None, cancellation)?;
    let output = scan_output(&mut reader, config, cancellation)?;
    let sources = scan_sources(inputs, plan, &output, config, cancellation)?;
    Ok(SoundScan {
        streams: reader.streams,
        output: Some(output),
        sources,
    })
}

fn scan_output(
    reader: &mut AudioReader,
    config: &TechnicalScanConfig,
    cancellation: &ExportCancellation,
) -> Result<OutputSound, MediaError> {
    let channels = usize::from(reader.channels);
    let mut hops = HopMeter::new(reader.rate, channels, config.max_hops);
    let mut placement = Placement {
        cursor: None,
        tolerance: config.audio_pts_tolerance_samples,
    };
    let mut meter = crate::loudness::LoudnessMeter::new(reader.rate, reader.channels);
    let mut out = OutputSound {
        rate: reader.rate,
        channels: reader.channels,
        frame_size: reader.frame_size,
        time_base: reader.time_base(),
        decoder: reader.decoder_name.clone(),
        first: None,
        end: 0,
        decoded_samples: 0,
        early_samples: 0,
        events: Vec::new(),
        event_count: 0,
        missing_pts: false,
        hops: Vec::new(),
        truncated: false,
        loudness: Err("not measured".into()),
    };
    let mut samples = Vec::new();
    while let Some(chunk) = reader.next(&mut samples, cancellation)? {
        let placed = placement.place(&chunk);
        out.missing_pts |= placed.missing_pts;
        let frames = u64::try_from(chunk.frames).unwrap_or(u64::MAX);
        let event = if placed.gap > 0 {
            hops.silence(
                placed.start - i64::try_from(placed.gap).unwrap_or(0),
                placed.gap,
            );
            Some(TimingEvent::Gap {
                at: placed.start,
                samples: placed.gap,
            })
        } else {
            (placed.overlap > 0).then_some(TimingEvent::Overlap {
                at: placed.start,
                samples: placed.overlap,
            })
        };
        if let Some(event) = event {
            out.event_count += 1;
            if out.events.len() < MAX_TIMING_EVENTS {
                out.events.push(event);
            }
        }
        out.first.get_or_insert(placed.start);
        let end = placed
            .start
            .saturating_add(i64::try_from(frames).unwrap_or(i64::MAX));
        out.end = out.end.max(end);
        out.decoded_samples = out.decoded_samples.saturating_add(frames);
        let early = placed
            .start
            .saturating_neg()
            .clamp(0, i64::try_from(frames).unwrap_or(0));
        out.early_samples = out.early_samples.saturating_add(early.unsigned_abs());
        if let Ok(meter) = meter.as_mut()
            && let Err(error) = meter.push(&samples)
        {
            out.loudness = Err(super::reason(&error));
        }
        hops.samples(placed.start, &samples, channels);
        if hops.truncated {
            break;
        }
    }
    let (levels, truncated) = hops.finish();
    out.hops = levels;
    out.truncated = truncated;
    if out.loudness.is_err() && !truncated {
        out.loudness = match meter {
            Ok(meter) => meter.finish().map_err(|error| super::reason(&error)),
            Err(error) => Err(super::reason(&error)),
        };
    }
    Ok(out)
}

/// Static gain in dB the mix applies to `track` per output channel: its
/// fader, its buses' faders, the master fader and the track's pan under the
/// document's pan law (R11). Automation is not static (see
/// `plan::uncomparable`). A channel the pan sends nothing to is at the dB
/// floor.
fn static_db(document: &Document, track: TrackId, channels: u16) -> [f64; 2] {
    let mix = &document.audio_mix;
    let track_gain = mix
        .tracks
        .iter()
        .find(|entry| entry.track == track)
        .map_or(0, |entry| entry.gain_tenth_db);
    let buses: i32 = mix
        .buses
        .iter()
        .filter(|bus| bus.tracks.contains(&track))
        .map(|bus| bus.gain_tenth_db)
        .sum();
    let fader = f64::from(track_gain + buses + mix.master.gain_tenth_db) / 10.0;
    if channels < 2 {
        return [fader; 2];
    }
    let pan = mix
        .tracks
        .iter()
        .find(|entry| entry.track == track)
        .map_or(0, |entry| entry.pan_percent);
    crate::audio::pan_channel_ratios(mix.pan_law, pan).map(|ratio| {
        if ratio > 0.0 {
            fader + 20.0 * f64::from(ratio).log10()
        } else {
            -100.0
        }
    })
}

fn scan_sources(
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    output: &OutputSound,
    config: &TechnicalScanConfig,
    cancellation: &ExportCancellation,
) -> Result<SourceSound, MediaError> {
    let mut sources = SourceSound {
        static_db: plan
            .audio
            .iter()
            .map(|part| static_db(plan.document(), part.segment.track, output.channels))
            .collect(),
        ..SourceSound::default()
    };
    let max_hop = u64::try_from(output.hops.len())
        .unwrap_or(0)
        .min(config.max_hops);
    for (index, part) in plan.audio.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        if part.muted || part.uncomparable.is_some() {
            continue;
        }
        let (Some(asset), Some(clip)) = (part.asset, part.clip) else {
            sources.failed.push((
                index,
                "the segment's clip or asset is not in the document".into(),
            ));
            continue;
        };
        match decode_part(
            inputs,
            plan,
            index,
            asset,
            clip,
            output,
            max_hop,
            &mut sources,
            cancellation,
        ) {
            Ok(()) => {}
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error) => sources.failed.push((index, super::reason(&error))),
        }
        if sources.truncated {
            break;
        }
    }
    Ok(sources)
}

#[allow(clippy::too_many_arguments, reason = "one part's decode context")]
fn decode_part(
    inputs: &ReviewScanInputs<'_>,
    plan: &Plan<'_>,
    index: usize,
    asset: &kinewright_core::MediaAsset,
    clip: &kinewright_core::Clip,
    output: &OutputSound,
    max_hop: u64,
    sources: &mut SourceSound,
    cancellation: &ExportCancellation,
) -> Result<(), MediaError> {
    let segment = plan.audio[index].segment;
    let rate = output.rate;
    let certified = inputs.sources.source(asset.id)?;
    let source_start = samples_at(segment.source.start, rate);
    let source_end = samples_at(segment.source.end, rate);
    let output_start = samples_at(segment.output.start, rate);
    let output_end = samples_at(segment.output.end, rate);
    let project_start = ExactMicros::frames(segment.project.start, plan.project_fps)
        .map(|at| samples_at(at, rate))
        .map_err(|error| MediaError::Backend(super::reason(&error)))?;
    let shaping = ClipAudioShaping::new(clip, plan.clip_duration(clip), rate, plan.project_fps);
    let mut decoder = AudioDecoder::open_with_source(
        &asset.path,
        rate,
        output.channels,
        source_start,
        source_end,
        Some(&certified),
    )?;
    let channels = usize::from(output.channels);
    let mut hops = HopMeter::new(rate, channels, max_hop);
    // Skip to the part's first output sample without keeping the hops
    // before it.
    hops.index = first_hop(output_start, rate);
    hops.next_boundary = hop_start(hops.index + 1, rate);
    let first = hops.index;
    let mut written = 0_u64;
    let wanted = output_end.saturating_sub(output_start);
    while let Some(mut chunk) = decoder.next_chunk()? {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let frames = u64::try_from(chunk.len() / channels.max(1)).unwrap_or(0);
        let keep = frames.min(wanted.saturating_sub(written));
        chunk.truncate(usize::try_from(keep).unwrap_or(0) * channels);
        for (offset, frame) in chunk.chunks_exact_mut(channels.max(1)).enumerate() {
            let at = output_start + written + u64::try_from(offset).unwrap_or(0);
            let project = (at - output_start).saturating_add(project_start);
            let gain = shaping.gain_at(project);
            for value in frame.iter_mut() {
                *value *= gain;
            }
        }
        hops.samples(
            i64::try_from(output_start + written).unwrap_or(i64::MAX),
            &chunk,
            channels,
        );
        written += keep;
        if written >= wanted || hops.truncated {
            break;
        }
    }
    let (levels, truncated) = hops.finish();
    let hop_samples = u64::from(rate) / 100 + 1;
    if !truncated && written.saturating_add(hop_samples) < wanted {
        return Err(MediaError::Backend(
            "the source ends before its segment".into(),
        ));
    }
    let part = u32::try_from(index).unwrap_or(u32::MAX);
    for (offset, level) in levels.into_iter().enumerate() {
        let hop = first + u64::try_from(offset).unwrap_or(u64::MAX);
        if hop >= max_hop {
            break;
        }
        if u64::try_from(sources.hops.len()).unwrap_or(u64::MAX) >= MAX_SOURCE_HOP_ENTRIES {
            sources.truncated = true;
            break;
        }
        sources.hops.push(SourceHop {
            hop: u32::try_from(hop).unwrap_or(u32::MAX),
            part,
            level,
        });
    }
    Ok(())
}

/// The hop holding output sample `sample`.
pub(super) fn first_hop(sample: u64, rate: u32) -> u64 {
    let step = u128::from(super::SCAN_HOP_MICROS) * u128::from(rate);
    // Largest hop whose start is at or before `sample`.
    let estimate = u128::from(sample) * 1_000_000 / step.max(1);
    let mut hop = u64::try_from(estimate).unwrap_or(u64::MAX);
    while hop > 0 && hop_start(hop, rate) > i64::try_from(sample).unwrap_or(i64::MAX) {
        hop -= 1;
    }
    while hop_start(hop + 1, rate) <= i64::try_from(sample).unwrap_or(i64::MAX) {
        hop += 1;
    }
    hop
}

#[cfg(test)]
mod tests {
    use super::HopMeter;

    /// F22: a PTS jump of any size fills at most the hop cap.
    #[test]
    fn ja1_s5a_zero_fill_is_bounded_by_the_hop_cap() {
        let mut hops = HopMeter::new(48_000, 2, 100);
        hops.silence(0, u64::MAX / 4);
        hops.samples(i64::MAX / 4, &[0.5, 0.5], 2);
        let (levels, truncated) = hops.finish();
        assert!(truncated);
        assert_eq!(levels.len(), 100);
    }
}
