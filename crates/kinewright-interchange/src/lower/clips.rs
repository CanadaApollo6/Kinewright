use std::{collections::BTreeMap, ops::Range};

use kinewright_core::{
    AssetId, AutomationCurve, BlendMode, Clip, ClipContent, ClipId, FreezeFrame, Keyframe,
    KeyframeInterpolation, LossBound, LossCode, MAX_KEY_FRAME_OFFSET, MediaKind, Rational,
    TimeCode, TrackKind,
};

use crate::{
    Refusal, Time,
    ir::{self, Loss, Playback},
    media::AssetInfo,
    record::Report,
};

use super::grid::Grid;

pub(super) struct ClipInput<'a> {
    pub raw: &'a ir::Clip,
    pub asset: AssetInfo,
    pub kind: TrackKind,
    pub project: Rational,
    pub transform_speed: Time,
    pub source_from: Time,
    pub wanted_end: Time,
    pub start: i64,
    pub limit: i64,
}

pub(super) struct Candidate {
    pub clip: Clip,
    pub end: i64,
    pub asset: Option<AssetInfo>,
    pub head: bool,
}

pub(super) fn blank(id: ClipId, content: ClipContent, start: i64, span: Range<TimeCode>) -> Clip {
    Clip {
        id,
        asset: AssetId::default(),
        source_range: span,
        content,
        timeline_start: TimeCode(start),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        audio_gain_curve: None,
        speed_percent: 100,
        enabled: true,
        enabled_curve: None,
        blend_mode: BlendMode::default(),
    }
}

pub(super) fn decide(
    input: &ClipInput<'_>,
    id: ClipId,
    report: &mut Report,
    losses: &mut Vec<Loss>,
) -> Result<Option<Candidate>, Refusal> {
    if !input.asset.kind.supports(input.kind) {
        losses.push(Loss::dropped(LossCode::UnknownItem).named("media component"));
        return Ok(None);
    }
    let raw = input.raw;
    if let Playback::Freeze(source) = raw.time {
        return freeze(input, id, source, losses);
    }
    if input.asset.kind == MediaKind::Image {
        return freeze(input, id, raw.source_start, losses);
    }
    let original_speed = raw.time.average()?.checked_mul(input.transform_speed)?;
    let play_speed = original_speed.abs()?;
    let speed_percent = speed(input, original_speed, play_speed, report, losses)?;
    let effective = kinewright_core::speed_scaled_fps(input.asset.fps, speed_percent)
        .map_err(|_| Refusal::arithmetic())?;
    let delta =
        Time::from_frame(TimeCode(input.start), input.project)?.checked_sub(input.source_from)?;
    let source = raw
        .source_start
        .checked_sub(input.asset.origin)?
        .checked_add(delta.checked_mul(play_speed)?)?;
    let rounded = source.frame(input.asset.fps)?.0;
    let source_start = rounded.max(0);
    let grid = Grid::new(effective, input.project, TimeCode(source_start))?;
    let wanted_span = input
        .wanted_end
        .frame(input.project)?
        .0
        .saturating_sub(input.start);
    let available_span = if source_start < input.asset.duration.0 {
        grid.duration(input.asset.duration.0)
    } else {
        0
    };
    if input.asset.probed && (source_start != rounded || wanted_span > available_span) {
        let loss = if input.asset.origin_known {
            LossCode::SourceClamped
        } else {
            LossCode::SourceOriginAssumed
        };
        losses.push(Loss::approximation(loss).bounded(LossBound::Frames(
            wanted_span.saturating_sub(available_span).unsigned_abs(),
        )));
    }
    let Some(source_end) = grid.nearest_end(
        source_start,
        input.asset.duration.0,
        input.start,
        input.wanted_end,
        input.limit,
    )?
    else {
        losses.push(Loss::dropped(LossCode::ClipTooShort));
        return Ok(None);
    };
    let length = grid.duration(source_end.0);
    let end = input
        .start
        .checked_add(length)
        .ok_or_else(Refusal::arithmetic)?;
    let snapped = end.abs_diff(input.wanted_end.frame(input.project)?.0);
    if snapped > 0 {
        losses.push(Loss::approximation(LossCode::CutSnapped).bounded(LossBound::Frames(snapped)));
    }
    let mut clip = blank(
        id,
        ClipContent::Media,
        input.start,
        TimeCode(source_start)..source_end,
    );
    clip.asset = input.asset.id;
    clip.speed_percent = speed_percent;
    clip.enabled = raw.enabled;
    if input.kind == TrackKind::Audio {
        audio(input, length, &mut clip, losses)?;
    }
    Ok(Some(Candidate {
        clip,
        end,
        asset: Some(input.asset),
        head: false,
    }))
}

fn freeze(
    input: &ClipInput<'_>,
    id: ClipId,
    at: Time,
    losses: &mut Vec<Loss>,
) -> Result<Option<Candidate>, Refusal> {
    if input.kind == TrackKind::Audio {
        losses.push(Loss::dropped(LossCode::RetimedAudioMuted));
        return Ok(None);
    }
    let end = input.wanted_end.frame(input.project)?.0.min(input.limit);
    if end <= input.start {
        losses.push(Loss::dropped(LossCode::ClipTooShort));
        return Ok(None);
    }
    let held = at
        .checked_sub(input.asset.origin)?
        .frame(input.asset.fps)?
        .0
        .clamp(0, input.asset.duration.0 - 1);
    let content = ClipContent::Freeze(FreezeFrame {
        source_frame: TimeCode(held),
    });
    let mut clip = blank(
        id,
        content,
        input.start,
        TimeCode::ZERO..TimeCode(end - input.start),
    );
    clip.asset = input.asset.id;
    clip.enabled = input.raw.enabled;
    Ok(Some(Candidate {
        clip,
        end,
        asset: Some(input.asset),
        head: false,
    }))
}

fn speed(
    input: &ClipInput<'_>,
    original: Time,
    play_speed: Time,
    report: &mut Report,
    losses: &mut Vec<Loss>,
) -> Result<u32, Refusal> {
    if original < Time::ZERO {
        losses.push(Loss::dropped(LossCode::ReverseDropped));
    }
    if let Playback::Variable(points) = &input.raw.time {
        let mut bound = 0;
        if let Some(first) = points.first() {
            let average = input.raw.time.average()?;
            for point in points {
                let approximate = first
                    .value
                    .checked_add(point.at.checked_sub(first.at)?.checked_mul(average)?)?;
                bound = bound.max(
                    point
                        .value
                        .checked_sub(approximate)?
                        .abs()?
                        .ceil_frame(input.asset.fps)?
                        .0
                        .unsigned_abs(),
                );
            }
        }
        losses.push(
            Loss::approximation(LossCode::VariableSpeedAveraged).bounded(LossBound::Frames(bound)),
        );
    }
    let percentage_grid = Rational::new(100, 1).map_err(|_| Refusal::arithmetic())?;
    let rounded = play_speed.frame(percentage_grid)?.0;
    let mut chosen = u32::try_from(rounded.clamp(10, 1000)).map_err(|_| Refusal::arithmetic())?;
    if kinewright_core::speed_scaled_fps(input.asset.fps, chosen).is_err() {
        chosen = 100;
    }
    let error = play_speed
        .checked_sub(Time::new(i64::from(chosen), 100)?)?
        .abs()?;
    if error != Time::ZERO {
        let unit = Rational::new(1_000, 1).map_err(|_| Refusal::arithmetic())?;
        let bound = LossBound::SpeedTenthPercent(error.ceil_frame(unit)?.0.unsigned_abs());
        let code = if i64::from(chosen) == rounded {
            LossCode::SpeedRounded
        } else {
            LossCode::SpeedClamped
        };
        losses.push(Loss::approximation(code).bounded(bound));
        let drift = error
            .checked_mul(input.raw.span)?
            .ceil_frame(input.asset.fps)?
            .0
            .unsigned_abs();
        report.retimed_drift = Some(report.retimed_drift.unwrap_or_default().max(drift));
    }
    if input.kind == TrackKind::Audio && chosen != 100 {
        losses.push(Loss::dropped(LossCode::RetimedAudioMuted));
    }
    Ok(chosen)
}

fn audio(
    input: &ClipInput<'_>,
    length: i64,
    clip: &mut Clip,
    losses: &mut Vec<Loss>,
) -> Result<(), Refusal> {
    let raw = input.raw;
    clip.audio_gain_tenth_db =
        i32::try_from(clamp_gain(raw.gain_tenth_db, losses)).map_err(|_| Refusal::arithmetic())?;
    let mut frames = BTreeMap::new();
    let mut key_mapping = false;
    let max_key = (length - 1).min(MAX_KEY_FRAME_OFFSET);
    for key in &raw.gain_keys {
        let position = input
            .source_from
            .checked_add(key.at.checked_div(input.transform_speed)?)?
            .frame(input.project)?
            .0
            .saturating_sub(input.start);
        let mapped = position.clamp(0, max_key);
        key_mapping |= position != mapped;
        let value = clamp_gain(key.tenth_db, losses);
        key_mapping |= frames.insert(mapped, value).is_some();
    }
    if key_mapping || !frames.is_empty() {
        losses.push(Loss::approximation(LossCode::GainKeysMapped));
    }
    if !frames.is_empty() {
        clip.audio_gain_curve = Some(AutomationCurve {
            keyframes: frames
                .into_iter()
                .map(|(at, value)| linear(at, value))
                .collect(),
        });
    }
    let fade_in = raw
        .fade_in
        .checked_div(input.transform_speed)?
        .frame(input.project)?
        .0
        .max(0);
    let fade_out = raw
        .fade_out
        .checked_div(input.transform_speed)?
        .frame(input.project)?
        .0
        .max(0);
    clip.audio_fade_in_frames = TimeCode(fade_in.min(length));
    clip.audio_fade_out_frames = TimeCode(fade_out.min(length - clip.audio_fade_in_frames.0));
    if (clip.audio_fade_in_frames.0, clip.audio_fade_out_frames.0) != (fade_in, fade_out) {
        losses.push(Loss::approximation(LossCode::TransitionFadeClamped));
    }
    Ok(())
}

fn clamp_gain(value: i64, losses: &mut Vec<Loss>) -> i64 {
    let gain = value.clamp(-600, 120);
    if gain != value {
        losses.push(
            Loss::approximation(LossCode::GainClamped)
                .bounded(LossBound::TenthDb(gain.abs_diff(value))),
        );
    }
    gain
}

pub(super) const fn linear(at: i64, value: i64) -> Keyframe {
    Keyframe {
        at: TimeCode(at),
        value,
        interpolation: KeyframeInterpolation::Linear,
        tangent_in: 0,
        tangent_out: 0,
    }
}
