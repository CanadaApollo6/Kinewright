use std::collections::BTreeMap;

use kinewright_core::{
    AutomationCurve, ClipContent, ClipId, Effect, EffectId, LossBound, LossCode,
    MAX_KEY_FRAME_OFFSET, ParamValue, Rational, TimeCode, TrackKind, Transition,
};

use crate::{Refusal, ir::Loss};

use super::{
    clips::{Candidate, blank, linear},
    grid::Grid,
};

pub(super) struct TransitionInput<'a> {
    pub kind: TrackKind,
    pub name: &'a str,
    pub project: Rational,
    pub before: i64,
    pub after: i64,
    pub horizon: i64,
    pub head_id: ClipId,
}

pub(super) fn lower_same_track_transition(
    input: &TransitionInput<'_>,
    a: &mut Option<Candidate>,
    b: &mut Option<Candidate>,
    notes: &mut Vec<Loss>,
) -> Result<Option<Candidate>, Refusal> {
    let length = input.before.saturating_add(input.after);
    if length == 0 {
        notes.push(Loss::approximation(LossCode::TransitionHandlesShort));
        return Ok(None);
    }
    let descriptor = descriptor(input.name, notes);
    if input.kind == TrackKind::Audio {
        notes.push(
            Loss::approximation(LossCode::AudioCrossfadeLinear).bounded(LossBound::TenthDb(30)),
        );
    }
    match (a.as_mut(), b.as_mut()) {
        (Some(outgoing), Some(entering))
            if outgoing.end == entering.clip.timeline_start.0
                && outgoing.clip.content.is_media()
                && entering.clip.content.is_media() =>
        {
            notes.push(Loss::approximation(LossCode::TransitionLane));
            let head = split(input, outgoing, entering, descriptor, notes)?;
            if entering.end == entering.clip.timeline_start.0 {
                *b = None;
            }
            Ok(head)
        }
        (_, Some(entering)) => {
            fade_in(entering, input.kind, descriptor, length, notes);
            Ok(None)
        }
        (Some(outgoing), None) => {
            fade_out(outgoing, input.kind, length, notes);
            Ok(None)
        }
        (None, None) => {
            notes.push(
                Loss::approximation(LossCode::TransitionHandlesShort)
                    .bounded(LossBound::Frames(length.unsigned_abs())),
            );
            Ok(None)
        }
    }
}

fn split(
    input: &TransitionInput<'_>,
    outgoing: &mut Candidate,
    entering: &mut Candidate,
    descriptor: &str,
    notes: &mut Vec<Loss>,
) -> Result<Option<Candidate>, Refusal> {
    let (Some(a_asset), Some(b_asset)) = (outgoing.asset, entering.asset) else {
        return Ok(None);
    };
    let cut = outgoing.end;
    let a_effective = kinewright_core::speed_scaled_fps(a_asset.fps, outgoing.clip.speed_percent)
        .map_err(|_| Refusal::arithmetic())?;
    let b_effective = kinewright_core::speed_scaled_fps(b_asset.fps, entering.clip.speed_percent)
        .map_err(|_| Refusal::arithmetic())?;
    let a_grid = Grid::new(a_effective, input.project, outgoing.clip.source_range.start)?;
    let b_start = entering.clip.source_range.start.0;
    let b_end = entering.clip.source_range.end.0;
    let b_grid = Grid::new(b_effective, input.project, TimeCode(b_start))?;
    let wanted_after = input.after.min(entering.end - cut);
    let mut trim = b_grid.last_at_most(b_start, b_end, wanted_after);
    let mut after = b_grid.duration(trim);
    let mut a_end = outgoing.clip.source_range.end;
    if after > 0 {
        let expanded = cut.checked_add(after).ok_or_else(Refusal::arithmetic)?
            - outgoing.clip.timeline_start.0;
        if let Some(end) = a_grid
            .exact_end(outgoing.clip.source_range.start.0, expanded)
            .filter(|end| *end <= a_asset.duration)
        {
            a_end = end;
        } else {
            trim = b_start;
            after = 0;
        }
    }
    let allowed_before = input.before.min(cut.saturating_sub(input.horizon).max(0));
    let absolute = Grid::new(b_effective, input.project, TimeCode::ZERO)?;
    let target = absolute.duration(b_start).saturating_sub(allowed_before);
    let head_source = absolute.first_at_least(0, b_start, target);
    let before = absolute.duration(b_start) - absolute.duration(head_source);
    if (before, after) != (input.before, input.after) {
        let missing = input
            .before
            .saturating_sub(before)
            .saturating_add(input.after.saturating_sub(after));
        notes.push(
            Loss::approximation(LossCode::TransitionHandlesShort)
                .bounded(LossBound::Frames(missing.unsigned_abs())),
        );
    }
    if before + after == 0 {
        return Ok(None);
    }
    let full_len = before + after;
    let mut head = blank(
        input.head_id,
        ClipContent::Media,
        cut - before,
        TimeCode(head_source)..TimeCode(trim),
    );
    head.asset = entering.clip.asset;
    head.link = entering.clip.link;
    head.enabled = entering.clip.enabled;
    head.speed_percent = entering.clip.speed_percent;
    head.audio_gain_tenth_db = entering.clip.audio_gain_tenth_db;

    if input.kind == TrackKind::Audio {
        head.audio_fade_in_frames = TimeCode(full_len);
        outgoing.clip.audio_fade_out_frames = TimeCode(full_len);
    } else {
        head.transition_in = Some(Transition {
            name: descriptor.into(),
            duration: TimeCode(full_len),
        });
    }
    let new_cut = cut + after;
    outgoing.clip.source_range.end = a_end;
    outgoing.end = new_cut;
    clamp_fades(outgoing, notes);
    entering.clip.source_range.start = TimeCode(trim);
    entering.clip.timeline_start = TimeCode(new_cut);
    if let Some(mut gain) = entering.clip.audio_gain_curve.take() {
        gain.keyframes.retain(|key| key.at.0 >= after);
        for key in &mut gain.keyframes {
            key.at.0 -= after;
        }
        if !gain.keyframes.is_empty() {
            entering.clip.audio_gain_curve = Some(gain);
        }
    }
    clamp_fades(entering, notes);
    Ok(Some(Candidate {
        clip: head,
        end: new_cut,
        asset: Some(b_asset),
        head: true,
    }))
}

fn fade_in(
    entering: &mut Candidate,
    kind: TrackKind,
    descriptor: &str,
    requested: i64,
    notes: &mut Vec<Loss>,
) {
    let length = requested.min(entering.end - entering.clip.timeline_start.0);
    if length < requested {
        notes.push(Loss::approximation(LossCode::TransitionFadeClamped));
    }
    if length == 0 {
        return;
    }
    if kind == TrackKind::Audio && entering.clip.content.is_media() {
        entering.clip.audio_fade_in_frames = TimeCode(length);
        clamp_fades(entering, notes);
    } else if kind == TrackKind::Video {
        entering.clip.transition_in = Some(Transition {
            name: descriptor.into(),
            duration: TimeCode(length),
        });
    }
}

fn fade_out(outgoing: &mut Candidate, kind: TrackKind, requested: i64, notes: &mut Vec<Loss>) {
    let span = outgoing.end - outgoing.clip.timeline_start.0;
    let length = requested.min(span);
    if length < requested {
        notes.push(Loss::approximation(LossCode::TransitionFadeClamped));
    }
    if kind == TrackKind::Audio && outgoing.clip.content.is_media() {
        outgoing.clip.audio_fade_out_frames = TimeCode(length);
        clamp_fades(outgoing, notes);
    } else if kind == TrackKind::Video && length > 0 {
        let last = (span - 1).min(MAX_KEY_FRAME_OFFSET);
        let first = (span - length).min(last);
        let mut keyframes = Vec::with_capacity(2);
        if first < last {
            keyframes.push(linear(first, 100));
        }
        keyframes.push(linear(last, 0));
        outgoing.clip.effects.push(Effect {
            id: EffectId(outgoing.clip.id.0),
            name: "opacity".into(),
            parameters: BTreeMap::from([("percent".into(), ParamValue::Integer(100))]),
            keyframes: BTreeMap::from([("percent".into(), AutomationCurve { keyframes })]),
            enabled: true,
            enabled_curve: None,
        });
    }
}

fn clamp_fades(clip: &mut Candidate, notes: &mut Vec<Loss>) {
    let length = clip.end - clip.clip.timeline_start.0;
    let (before, after) = (
        clip.clip.audio_fade_in_frames,
        clip.clip.audio_fade_out_frames,
    );
    clip.clip.audio_fade_in_frames.0 = before.0.min(length);
    clip.clip.audio_fade_out_frames.0 = after.0.min(length - clip.clip.audio_fade_in_frames.0);
    if (before, after)
        != (
            clip.clip.audio_fade_in_frames,
            clip.clip.audio_fade_out_frames,
        )
    {
        notes.push(Loss::approximation(LossCode::TransitionFadeClamped));
    }
}

fn descriptor(name: &str, notes: &mut Vec<Loss>) -> &'static str {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("dissolve") || normalized.contains("crossfade") {
        return "crossfade";
    }
    notes.push(Loss::approximation(LossCode::TransitionTypeMapped).named(name));
    kinewright_core::TRANSITION_DESCRIPTORS
        .iter()
        .find(|descriptor| {
            descriptor
                .name
                .split_once('_')
                .is_some_and(|(family, direction)| {
                    matches!(family, "wipe" | "push" | "slide")
                        && normalized.contains(family)
                        && normalized.contains(direction)
                })
        })
        .map_or("crossfade", |descriptor| descriptor.name)
}
