use std::sync::Arc;

use eframe::egui;
use kinewright_core::{
    Analysis, AutomationCurve, Clip, ClipContent, ClipId, Document, ENVELOPE_DISPLAY_MIN_TENTH_DB,
    FrameRounding, Keyframe, MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId, MediaAsset, MediaKind,
    Operation, Rational, SceneStatus, SilenceStatus, TRACK_MIX_GAIN_MAX, TRACK_MIX_GAIN_MIN,
    TimeCode, Title, TrackId, TrackKind, Transition, WaveformData, envelope_coalesce_key,
    map_frames_with_rounding, map_source_range_to_project,
};
use kinewright_media::{RoomToneStore, timeline_source_at};

use crate::{
    app::KinewrightApp,
    icons::{self, Icon},
    inspector_ui::{InspectorEdits, is_live_drag},
    mixer_ui::{MixToggle, paint_mix_toggle, track_caption_and_icon, track_mix_toggle_operation},
    theme::{self, color, radius, size, space, type_size},
    visual_cache::VisualCache,
};

/// Width of the timeline's track-label column.
///
/// AU1 §5.2 grew it from 76 to 96 to seat the mute and solo toggles between
/// the caption column and the sync-lock button.
const TRACK_LABEL_WIDTH: f32 = 96.0;
const EDGE_HANDLE_WIDTH: f32 = 6.0;
const SNAP_TOLERANCE: f32 = 8.0;
const FILMSTRIP_TILE_WIDTH: f32 = 96.0;
const THUMBNAIL_WIDTH: u32 = 128;
const INTERNAL_MARKER_LABEL_PREFIX: &str = "__kinewright_reframe_subject_v1:";

/// AU4 §5.1 rule 97: the clip width under which no envelope band is painted
/// and no envelope hit-testing runs. The same 24 the clip-width floor uses
/// (`clip_width = (duration * pixels_per_frame).max(24.0)`), so the coarse
/// gesture is never offered where it cannot land.
const ENVELOPE_MINIMUM_CLIP_WIDTH: f32 = 24.0;
/// AU4 §5.1 rule 99: how near the polyline or a key the pointer has to be
/// before the band takes an interact. `curve_editor_widget::HIT_RADIUS`
/// reused, not re-invented.
const ENVELOPE_HIT_RADIUS: f32 = 9.0;
/// AU4 §5.1 rule 102: `curve_editor_widget::POINT_RADIUS`, reused.
const ENVELOPE_POINT_RADIUS: f32 = 3.5;
/// AU4 §5.1 rule 102: `curve_editor_widget::CURVE_STROKE`, reused.
const ENVELOPE_STROKE: f32 = 1.6;
/// AU4 §5.1 rule 95: the top of the band's value axis, in tenth-dB.
///
/// The bottom is [`ENVELOPE_DISPLAY_MIN_TENTH_DB`] (−40 dB); together they are
/// a 520 tenth-dB span with unity at `(120 − 0) / 520` = 23.1 % from the top.
const ENVELOPE_DISPLAY_MAX_TENTH_DB: i32 = TRACK_MIX_GAIN_MAX;
/// AU4 §5.1 rule 95: the span of the band's value axis, in tenth-dB.
const ENVELOPE_DISPLAY_SPAN_TENTH_DB: i32 =
    ENVELOPE_DISPLAY_MAX_TENTH_DB - ENVELOPE_DISPLAY_MIN_TENTH_DB;

/// Tracker provenance markers are document sidecars, not editorial markers.
/// Keep them out of the timeline's visible, selectable, and snapping surfaces.
pub(crate) fn is_internal_marker(marker: &Marker) -> bool {
    marker.label.starts_with(INTERNAL_MARKER_LABEL_PREFIX)
}

#[derive(Clone, Copy)]
struct ClipBounds {
    id: ClipId,
    start: i64,
    end: i64,
}

#[derive(Clone, Copy)]
enum TrimEdge {
    Left,
    Right,
}

/// The envelope key one pointer is dragging, remembered across frames.
///
/// Frame-local like `CurveEditorMemory`: it lives in `ui.data` temp storage
/// and is never serialised, because it is a pointer state and not a document
/// or session fact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct EnvelopeDrag {
    clip: ClipId,
    index: usize,
}

/// AU4 §5.1 rule 95a: the envelope's paint, hit and interact rect.
///
/// Pure; no `Ui`, no session state. `band` is built exactly as the waveform's
/// is — `rect.top() + 18.0` for a pure-audio asset, `rect.bottom() −
/// rect.height() * 0.42` otherwise — and the return is
/// `band.shrink2(vec2(space::HALF, space::ONE))`, the same rect
/// `paint_waveform` draws into, so the ride and the waveform agree pixel for
/// pixel.
///
/// One helper because the interact is allocated some 750 lines before the
/// painting path builds its own `band`: without it the two sites would
/// disagree by `space::HALF = 2` px in x and `space::ONE = 4` px in y and the
/// drawn key would not be the grabbed key. A `MediaKind::Video` asset — and
/// every Title and Freeze clip, which reach this with no asset at all — has no
/// band, no paint and no interact.
fn envelope_band_rect(clip_rect: egui::Rect, kind: MediaKind) -> Option<egui::Rect> {
    envelope_band_unshrunk(clip_rect, kind)
        .map(|band| band.shrink2(egui::vec2(space::HALF, space::ONE)))
}

/// The same band before the shrink — what `paint_clip`'s waveform scrim fills.
///
/// The single expression [`envelope_band_rect`] is derived from, so the scrim,
/// the waveform and the ride cannot drift apart when the `18.0` or the `0.42`
/// changes.
fn envelope_band_unshrunk(clip_rect: egui::Rect, kind: MediaKind) -> Option<egui::Rect> {
    if !matches!(kind, MediaKind::Audio | MediaKind::AudioVideo) {
        return None;
    }
    let band_top = if matches!(kind, MediaKind::Audio) {
        clip_rect.top() + 18.0
    } else {
        clip_rect.bottom() - clip_rect.height() * 0.42
    };
    Some(egui::Rect::from_min_max(
        egui::pos2(clip_rect.left(), band_top),
        clip_rect.max,
    ))
}

/// AU4 §5.1 rule 98: the rect the envelope's `ui.interact` takes.
///
/// Rule 95a's band, intersected with `body_rect` horizontally. egui 0.35.0
/// resolves overlapping interactive rects by *last registered wins*
/// (`hit_test.rs:429-433`, the cargo-registry copy Cargo.lock:1116-1118 pins:
/// the comment sits at :429 and the code at :430, "In case of a tie, take the
/// last one = the one on top"), so this interact has to be allocated **after**
/// `body`, `left` and `right` or it loses every tie — and without the x
/// restriction it would steal both `EDGE_HANDLE_WIDTH` trim handles, which
/// live outside `body_rect`. Do not "fix" the order.
fn envelope_interact_rect(band: egui::Rect, body_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(band.left().max(body_rect.left()), band.top()),
        egui::pos2(band.right().min(body_rect.right()), band.bottom()),
    )
}

/// Half away from zero, the house rounding for a pointer coordinate.
fn envelope_round_half_away_from_zero(value: f64) -> f64 {
    if value < 0.0 {
        -(-value + 0.5).floor()
    } else {
        (value + 0.5).floor()
    }
}

/// AU4 §5.1 rule 96: pointer x to a clip-local frame, through the same rect
/// ratio the waveform uses. Pure; no window, no session state.
///
/// Not through `pixels_per_frame`: `clip_width` has a 24 px floor, so at any
/// zoom where `duration × pixels_per_frame < 24` the painted rect is wider
/// than the clip's true span and adjacent clips overlap in x. `paint_waveform`
/// survives that by mapping columns through a clip-local ratio of
/// `rect.width()`; the envelope does the same.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn envelope_x_to_local_frame(rect: egui::Rect, duration: TimeCode, x: f32) -> TimeCode {
    let ratio = f64::from(x - rect.left()) / f64::from(rect.width().max(1.0));
    let frame = envelope_round_half_away_from_zero(ratio * duration.0 as f64) as i64;
    TimeCode(frame.clamp(0, duration.0.saturating_sub(1).max(0)))
}

/// AU4 §5.1 rule 96: a clip-local frame back to x, through the same ratio.
#[allow(clippy::cast_precision_loss)]
fn envelope_local_frame_to_x(rect: egui::Rect, duration: TimeCode, at: TimeCode) -> f32 {
    let span = duration.0.max(1) as f32;
    rect.left() + rect.width() * (at.0 as f32 / span)
}

/// AU4 §5.1 rule 95: a tenth-dB value to y, linear over −400 … +120.
///
/// A value outside the range clamps to the edge rather than being hidden.
#[allow(clippy::cast_precision_loss)]
fn envelope_value_to_y(rect: egui::Rect, value: i32) -> f32 {
    let clamped = value.clamp(ENVELOPE_DISPLAY_MIN_TENTH_DB, ENVELOPE_DISPLAY_MAX_TENTH_DB);
    let fraction =
        (ENVELOPE_DISPLAY_MAX_TENTH_DB - clamped) as f32 / ENVELOPE_DISPLAY_SPAN_TENTH_DB as f32;
    rect.top() + rect.height() * fraction
}

/// AU4 §5.1 rule 95: y back to a tenth-dB value, linear over −400 … +120.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn envelope_y_to_value(rect: egui::Rect, y: f32) -> i32 {
    let fraction = (f64::from(y - rect.top()) / f64::from(rect.height().max(1.0))).clamp(0.0, 1.0);
    let value = f64::from(ENVELOPE_DISPLAY_MAX_TENTH_DB)
        - fraction * f64::from(ENVELOPE_DISPLAY_SPAN_TENTH_DB);
    (envelope_round_half_away_from_zero(value) as i32)
        .clamp(ENVELOPE_DISPLAY_MIN_TENTH_DB, ENVELOPE_DISPLAY_MAX_TENTH_DB)
}

/// AU4 §5.1 rules 95 and 101: the value one band grab writes.
///
/// Rule 95 fixes the *display* axis at −400 … +120 while rule 101 clamps a
/// dragged key to −600 … +120, so a key the inspector parked at −500 paints on
/// the band's floor and [`envelope_y_to_value`] would read −400 back out of
/// it — a purely horizontal drag would silently raise the key by 10 dB. A key
/// already below the display floor therefore keeps its stored value for as
/// long as the pointer stays on the floor; the first frame the pointer leaves
/// the floor, the drag means it.
fn envelope_grab_value(band: egui::Rect, pointer_y: f32, stored: i64) -> i32 {
    let pointed = envelope_y_to_value(band, pointer_y);
    if stored < i64::from(ENVELOPE_DISPLAY_MIN_TENTH_DB) && pointed == ENVELOPE_DISPLAY_MIN_TENTH_DB
    {
        let held = stored.clamp(i64::from(TRACK_MIX_GAIN_MIN), i64::from(TRACK_MIX_GAIN_MAX));
        return i32::try_from(held).unwrap_or(ENVELOPE_DISPLAY_MIN_TENTH_DB);
    }
    pointed
}

/// Every key of one clip envelope as a pixel position inside the band.
///
/// Index `i` is keyframe `i`, which is what makes [`envelope_hit`]'s return
/// addressable by the gesture rules.
#[allow(clippy::cast_possible_truncation)]
fn envelope_points(rect: egui::Rect, duration: TimeCode, keys: &[Keyframe]) -> Vec<egui::Pos2> {
    keys.iter()
        .map(|key| {
            egui::pos2(
                envelope_local_frame_to_x(rect, duration, key.at),
                envelope_value_to_y(rect, key.value as i32),
            )
        })
        .collect()
}

/// The drawn line through those keys, with a `Hold` segment drawn as a step.
///
/// A `Hold` segment is flat until the next key's first sample (AU4 §3.1 rule
/// 3), so drawing it as a ramp would make the band lie about what is heard.
/// The line runs the whole width of the band because `value_at` clamps outside
/// the keyed interval: a one-key curve really is a constant over the clip, and
/// drawing it as a single dot would hide the thing there is to grab.
fn envelope_polyline(
    rect: egui::Rect,
    points: &[egui::Pos2],
    keys: &[Keyframe],
) -> Vec<egui::Pos2> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    let mut line = Vec::with_capacity(points.len() * 2 + 2);
    line.push(egui::pos2(rect.left(), first.y));
    for (index, point) in points.iter().enumerate() {
        if index > 0
            && keys.get(index - 1).is_some_and(|key| {
                key.interpolation == kinewright_core::KeyframeInterpolation::Hold
            })
        {
            line.push(egui::pos2(point.x, points[index - 1].y));
        }
        line.push(*point);
    }
    if let Some(last) = points.last() {
        line.push(egui::pos2(rect.right(), last.y));
    }
    line
}

/// AU4 §0 E53: the line a clip with no curve shows — flat, at the clip's
/// parked `audio_gain_tenth_db`.
///
/// Without it rule 101's "a click on the line inserts a key" is unreachable
/// for the *first* key and nothing on an untouched audio clip says it has a
/// band at all. Two points, so [`envelope_near_curve`]'s segment probe answers
/// it exactly as it answers a real curve.
fn envelope_parked_polyline(band: egui::Rect, parked: i32) -> Vec<egui::Pos2> {
    let y = envelope_value_to_y(band, parked);
    vec![egui::pos2(band.left(), y), egui::pos2(band.right(), y)]
}

/// AU4 §5.1 rule 102: the rubber band over one clip's gain envelope.
///
/// Points at [`ENVELOPE_POINT_RADIUS`], the line at [`ENVELOPE_STROKE`], in
/// `ACCENT` when the clip is selected and `TEXT_PRIMARY_64` otherwise — accent
/// stays reserved for selection and direct manipulation (DESIGN.md).
///
/// AU4 §0 E53: a clip with no curve paints one flat line at `parked`, its
/// `audio_gain_tenth_db`, in `TEXT_MUTED` and with no key dots — the band is
/// an offer, not an envelope, until the first click lands a key on it.
#[allow(clippy::cast_possible_truncation)]
fn paint_clip_envelope(
    painter: &egui::Painter,
    band: egui::Rect,
    duration: TimeCode,
    keys: &[Keyframe],
    parked: i32,
    selected: bool,
) {
    if keys.is_empty() {
        painter.add(egui::Shape::line(
            envelope_parked_polyline(band, parked),
            egui::Stroke::new(ENVELOPE_STROKE, color::TEXT_MUTED),
        ));
        return;
    }
    let tint = if selected {
        color::ACCENT
    } else {
        color::TEXT_PRIMARY_64
    };
    let points = envelope_points(band, duration, keys);
    let line = envelope_polyline(band, &points, keys);
    if line.len() >= 2 {
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(ENVELOPE_STROKE, tint),
        ));
    }
    for (point, key) in points.iter().zip(keys) {
        painter.circle_filled(*point, ENVELOPE_POINT_RADIUS, tint);
        let value = key.value as i32;
        if !(ENVELOPE_DISPLAY_MIN_TENTH_DB..=ENVELOPE_DISPLAY_MAX_TENTH_DB).contains(&value) {
            // Rule 95: a value outside the display range clamps to the edge
            // and paints a 2 px marker there rather than being hidden.
            painter.line_segment(
                [
                    egui::pos2(point.x - ENVELOPE_POINT_RADIUS, point.y),
                    egui::pos2(point.x + ENVELOPE_POINT_RADIUS, point.y),
                ],
                egui::Stroke::new(2.0, tint),
            );
        }
    }
}

/// AU4 §5.1 rule 99: the key under the pointer, if one is within
/// [`ENVELOPE_HIT_RADIUS`].
///
/// Mirrors `curve_editor_widget::nearest_point` and the matte overlay's
/// `matte_hit_test`: O(n) over the key list, nearest wins, no spatial index.
/// A pointer further than the radius outside the band cannot hit anything, so
/// the rect is the first rejection.
fn envelope_hit(points: &[egui::Pos2], rect: egui::Rect, pointer: egui::Pos2) -> Option<usize> {
    if !rect.expand(ENVELOPE_HIT_RADIUS).contains(pointer) {
        return None;
    }
    points
        .iter()
        .enumerate()
        .map(|(index, point)| (index, point.distance(pointer)))
        .filter(|(_, distance)| *distance <= ENVELOPE_HIT_RADIUS)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

/// The distance from `pointer` to the segment `start..end`.
fn distance_to_segment(start: egui::Pos2, end: egui::Pos2, pointer: egui::Pos2) -> f32 {
    let span = end - start;
    let length_squared = span.length_sq();
    if length_squared <= f32::EPSILON {
        return start.distance(pointer);
    }
    let t = ((pointer - start).dot(span) / length_squared).clamp(0.0, 1.0);
    (start + span * t).distance(pointer)
}

/// AU4 §5.1 rule 99: whether the pointer is within [`ENVELOPE_HIT_RADIUS`] of
/// the drawn line or one of its keys.
///
/// This is the allocation predicate: body drags and the two 6 px trim handles
/// are untouched everywhere except within 9 px of the line.
fn envelope_near_curve(polyline: &[egui::Pos2], rect: egui::Rect, pointer: egui::Pos2) -> bool {
    if !rect.expand(ENVELOPE_HIT_RADIUS).contains(pointer) {
        return false;
    }
    match polyline {
        [] => false,
        [only] => only.distance(pointer) <= ENVELOPE_HIT_RADIUS,
        _ => polyline
            .windows(2)
            .any(|pair| distance_to_segment(pair[0], pair[1], pointer) <= ENVELOPE_HIT_RADIUS),
    }
}

/// AU4 §5.1 rule 101: insert one key at a snapped clip-local frame, taking the
/// curve's current value there. Pure; returns the whole key list.
///
/// A frame that already carries a key is left alone — the pointer would have
/// grabbed that key rather than the line.
fn envelope_insert_key(curve: &AutomationCurve, at: TimeCode) -> Vec<Keyframe> {
    let mut keys = curve.keyframes.clone();
    let index = keys.partition_point(|key| key.at.0 < at.0);
    if keys.get(index).is_some_and(|key| key.at == at) {
        return keys;
    }
    let value = curve.value_at(at).unwrap_or(0);
    // The segment the click landed on decides the new key's shape, so
    // inserting on a `Hold` run does not silently turn it into a ramp.
    let interpolation = index
        .checked_sub(1)
        .and_then(|previous| keys.get(previous))
        .map_or(kinewright_core::KeyframeInterpolation::Linear, |key| {
            key.interpolation
        });
    keys.insert(
        index,
        Keyframe {
            at,
            value,
            interpolation,
        },
    );
    keys
}

/// AU4 §5.1 rule 101: move one key, x kept strictly between its neighbours and
/// inside `0..=duration − 1`, y clamped to `-600..=120`. Pure; returns the
/// whole key list.
fn envelope_move_key(
    keys: &[Keyframe],
    index: usize,
    at: TimeCode,
    value: i32,
    duration: TimeCode,
) -> Vec<Keyframe> {
    let last = duration.0.saturating_sub(1).max(0);
    let low = index
        .checked_sub(1)
        .and_then(|previous| keys.get(previous))
        .map_or(0, |key| key.at.0.saturating_add(1));
    let high = keys
        .get(index + 1)
        .map_or(last, |key| key.at.0.saturating_sub(1));
    let mut moved = keys.to_vec();
    let Some(key) = moved.get_mut(index) else {
        return moved;
    };
    let low = low.clamp(0, last);
    let high = high.clamp(0, last);
    // Unreachable on a valid curve: two neighbours of a key are always at
    // least two frames apart.
    key.at = TimeCode(if low > high {
        low
    } else {
        at.0.clamp(low, high)
    });
    key.value = i64::from(value.clamp(TRACK_MIX_GAIN_MIN, TRACK_MIX_GAIN_MAX));
    moved
}

/// AU4 §5.1 rule 101: remove one key. `None` when there is nothing to remove —
/// **the last key cannot be removed**, because clearing the envelope is the
/// inspector's `Clear`, which sends `curve: None`.
fn envelope_remove_key(keys: &[Keyframe], index: usize) -> Option<Vec<Keyframe>> {
    if keys.len() <= 1 || index >= keys.len() {
        return None;
    }
    let mut remaining = keys.to_vec();
    remaining.remove(index);
    Some(remaining)
}

/// The operation one envelope gesture writes.
fn envelope_operation(clip: ClipId, keys: Vec<Keyframe>) -> Operation {
    Operation::SetClipGainEnvelope {
        clip,
        curve: Some(AutomationCurve { keyframes: keys }),
    }
}

/// AU4 §5.1 rules 97 and 100: whether this clip is offered a rubber band at
/// all. Pure; no window.
///
/// The toggle first, because it hides the overlay and with it all
/// hit-testing; then the 24 px floor, so the coarse gesture is never offered
/// where it cannot land.
fn envelope_is_offered(duration: TimeCode, pixels_per_frame: f32, show_envelopes: bool) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let width = duration.0 as f32 * pixels_per_frame;
    show_envelopes && width >= ENVELOPE_MINIMUM_CLIP_WIDTH
}

/// Where the in-flight envelope drag is remembered between frames.
const ENVELOPE_DRAG_MEMORY_ID: &str = "timeline-envelope-drag";

/// AU4 §5.1 rules 96 and 103: pointer x to a snapped clip-local frame.
///
/// The snapped **project** frame comes from the existing `nearest_snap` path,
/// so envelope keys snap to the same guides everything else does and Alt
/// bypasses them through the same frame-level flag; it is then converted to a
/// clip-local frame by subtracting `timeline_start`, never by re-deriving x
/// from `pixels_per_frame`.
#[allow(clippy::too_many_arguments)]
fn envelope_snapped_local_frame(
    band: egui::Rect,
    clip_start: TimeCode,
    duration: TimeCode,
    pointer_x: f32,
    candidates: &[i64],
    ruler_interval: i64,
    pixels_per_frame: f32,
    snapping_disabled: bool,
) -> (TimeCode, Option<i64>) {
    let local = envelope_x_to_local_frame(band, duration, pointer_x);
    let raw = clip_start.0.saturating_add(local.0);
    let (snapped, guide) = if snapping_disabled {
        (raw, None)
    } else {
        nearest_snap(raw, candidates, ruler_interval, pixels_per_frame)
    };
    let at = snapped
        .saturating_sub(clip_start.0)
        .clamp(0, duration.0.saturating_sub(1).max(0));
    (TimeCode(at), guide)
}

/// AU4 §5.1 rule 101 (AU4 §0 E49): the timeline's report that the pointer was
/// over one envelope key, read one frame later.
///
/// `keyboard_shortcuts` runs at the top of the frame, before `panel_layout`
/// draws the timeline, so a Delete can only be arbitrated against what the
/// *previous* frame saw. This is the matte overlay's
/// `InspectorEdits::matte_expanded` → `MatteOverlayState::report_expanded`
/// pattern (inspector_ui.rs:759-761): the input policy is the last frame's
/// report, and it costs no deferral because the pointer has not moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnvelopeHover {
    pub(crate) clip: ClipId,
    pub(crate) index: usize,
}

/// AU4 §5.1 rule 101 (AU4 §0 E49): what Delete/Backspace does this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnvelopeDelete {
    /// No envelope key was hovered: Delete deletes the selected clip, exactly
    /// as it did before. Also the answer to a stale report, whose clip or
    /// curve has since gone.
    Clip,
    /// The hovered key goes and the clip is left alone.
    ///
    /// The remaining key list rather than the built `Operation`, which is
    /// 552 bytes and would make every `Clip` answer carry them.
    RemoveKey { clip: ClipId, keys: Vec<Keyframe> },
    /// The hovered key is the envelope's last one, which rule 101 refuses to
    /// remove: clearing an envelope is the inspector's `Clear`.
    RefuseLastKey,
}

/// AU4 §5.1 rule 101 (AU4 §0 E49): arbitrate one Delete/Backspace between the
/// hovered envelope key and the selected clip. Pure; no window, no session.
pub(crate) fn envelope_delete_action(
    document: &Document,
    hover: Option<EnvelopeHover>,
) -> EnvelopeDelete {
    let Some(hover) = hover else {
        return EnvelopeDelete::Clip;
    };
    let Some(curve) = document
        .clip(hover.clip)
        .and_then(|clip| clip.audio_gain_curve.as_ref())
    else {
        return EnvelopeDelete::Clip;
    };
    envelope_remove_key(&curve.keyframes, hover.index).map_or(
        EnvelopeDelete::RefuseLastKey,
        |keys| EnvelopeDelete::RemoveKey {
            clip: hover.clip,
            keys,
        },
    )
}

/// AU4 §5.1 rule 101: what the app says when Delete lands on an envelope's
/// last key.
pub(crate) const ENVELOPE_LAST_KEY_NOTE: &str =
    "An envelope's last key stays: use Clear in the Inspector to remove the envelope.";

impl KinewrightApp {
    /// AU4 §5.1 rule 101 (AU4 §0 E49): Delete/Backspace over an envelope key.
    ///
    /// Returns `false` when nothing was hovered, which is the caller's cue to
    /// fall through to `delete_selected`. The report is passed in rather than
    /// read from the session because the caller takes it: it is good for one
    /// frame only.
    pub(crate) fn remove_hovered_envelope_key(&mut self, hover: Option<EnvelopeHover>) -> bool {
        let document = Arc::clone(&self.focused().document);
        match envelope_delete_action(&document, hover) {
            EnvelopeDelete::Clip => false,
            EnvelopeDelete::RemoveKey { clip, keys } => {
                self.send_operation(envelope_operation(clip, keys));
                true
            }
            EnvelopeDelete::RefuseLastKey => {
                self.record_error("Operations", ENVELOPE_LAST_KEY_NOTE);
                true
            }
        }
    }

    pub(crate) fn add_title_at_playhead(&mut self) {
        let document = Arc::clone(&self.focused().document);
        let at = self.focused().position;
        let duration = TimeCode(i64::from(nominal_fps(document.fps)).saturating_mul(3));
        let end = TimeCode(at.0.saturating_add(duration.0));
        let available_track = self
            .focused()
            .document
            .tracks
            .iter()
            .rev()
            .filter(|track| track.kind == TrackKind::Video)
            .find(|track| {
                track.clips.iter().all(|clip| {
                    let clip_end = self.focused().document.clip_duration(clip).map_or(
                        clip.timeline_start,
                        |clip_duration| {
                            TimeCode(clip.timeline_start.0.saturating_add(clip_duration.0))
                        },
                    );
                    clip_end <= at || clip.timeline_start >= end
                })
            })
            .map(|track| track.id);
        let add_title = |track| Operation::AddTitle {
            track,
            at,
            duration,
            title: Title::default(),
        };
        if let Some(track) = available_track {
            self.send_operation(add_title(track));
            return;
        }
        let Some(next_track) = self
            .focused()
            .document
            .tracks
            .iter()
            .map(|track| track.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .map(TrackId)
        else {
            self.record_error("Operations", "Track id space is exhausted");
            return;
        };
        self.send_operations(vec![
            Operation::AddTrack {
                track: kinewright_core::Track {
                    id: next_track,
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            },
            add_title(next_track),
        ]);
    }

    pub(crate) fn freeze_frame_at_playhead(&mut self) {
        let position = self.focused().position;
        match freeze_frame_operations(&self.focused().document, position) {
            Ok(operations) => self.send_operations(operations),
            Err(error) => self.record_error("Operations", error),
        }
    }

    pub(crate) fn split_at_playhead(&mut self) {
        let position = self.focused().position;
        let clip = self.focused().selected_clip.or_else(|| {
            timeline_source_at(&self.focused().document, position)
                .ok()
                .flatten()
                .map(|source| source.clip)
        });
        let Some(clip) = clip else {
            self.record_error(
                "Operations",
                "No clip is selected or active at the playhead",
            );
            return;
        };
        self.send_operation(Operation::SplitClip { clip, at: position });
    }

    pub(crate) fn delete_selected(&mut self) {
        if self.focused().transcript_selection.is_some() {
            self.cut_selected_transcript_words();
            return;
        }
        let selected_marker = self.focused().selected_marker;
        if selected_marker.is_some_and(|id| {
            self.focused()
                .document
                .marker(id)
                .is_some_and(is_internal_marker)
        }) {
            self.focused_mut().selected_marker = None;
        }
        if let Some(marker) = self.focused().selected_marker {
            self.send_operation(Operation::RemoveMarker { marker });
            return;
        }
        self.delete_selected_clips(self.ripple_mode);
    }

    pub(crate) fn ripple_delete_selected(&mut self) {
        self.delete_selected_clips(true);
    }

    fn delete_selected_clips(&mut self, ripple: bool) {
        let Some(clip) = self.focused().selected_clip else {
            self.record_error("Operations", "Select a clip to delete");
            return;
        };
        self.send_operations(linked_delete_operations(
            &self.focused().document,
            clip,
            ripple,
        ));
    }

    pub(crate) fn add_marker_at_playhead(&mut self) {
        let Some(id) = next_marker_id(&self.focused().document) else {
            self.record_error("Operations", "Marker id space is exhausted");
            return;
        };
        let marker = Marker {
            id,
            position: self.focused().position,
            label: format!("Marker {id}"),
            color_token: u8::try_from(id.0.saturating_sub(1) % u64::from(MARKER_COLOR_TOKEN_COUNT))
                .expect("marker color token is bounded by a u8 constant"),
        };
        self.focused_mut().selected_marker = Some(id);
        self.focused_mut().selected_clip = None;
        self.send_operation(Operation::AddMarker { marker });
    }

    pub(crate) fn apply_linked_trim(
        &mut self,
        clip: ClipId,
        new_source: std::ops::Range<TimeCode>,
    ) {
        let Some(original) = self.focused().document.clip(clip) else {
            self.record_error("Operations", format!("Clip {clip} no longer exists"));
            return;
        };
        let edge = if new_source.start == original.source_range.start {
            TrimEdge::Right
        } else {
            TrimEdge::Left
        };
        match linked_trim_operations(&self.focused().document, clip, new_source, edge) {
            Ok(operations) => self.send_operations(operations),
            Err(error) => self.record_error("Operations", error),
        }
    }

    // Timeline interaction intentionally maps exact frames to egui's f32 pixel coordinate space.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation
    )]
    pub(crate) fn timeline(&mut self, ui: &mut egui::Ui) {
        let project_index = self.focused_project;
        let old_zoom_target = self.focused().timeline_zoom_target;
        let mut zoom_target = old_zoom_target;
        let mut scroll_target = self.focused().timeline_scroll_target;
        let project_duration = self.focused().document.duration;
        let mut show_envelopes = self.focused().show_envelopes;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), size::TIMELINE_TOOLBAR_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if icons::button(ui, Icon::Split, "Split at playhead (S)").clicked() {
                    self.split_at_playhead();
                }
                if icons::button(ui, Icon::Delete, "Delete selected clip").clicked() {
                    self.delete_selected();
                }
                if ui
                    .button("T  Title")
                    .on_hover_text("Add a three-second title")
                    .clicked()
                {
                    self.add_title_at_playhead();
                }
                if ui
                    .button("Freeze")
                    .on_hover_text("Freeze the current frame for two seconds")
                    .clicked()
                {
                    self.freeze_frame_at_playhead();
                }
                let ripple = ui
                    .add(
                        egui::Button::new("Ripple")
                            .selected(self.ripple_mode)
                            .min_size(egui::vec2(54.0, 22.0)),
                    )
                    .on_hover_text(
                        "Ripple mode: deletes close space on the edited and sync-locked tracks",
                    );
                if ripple.clicked() {
                    self.ripple_mode = !self.ripple_mode;
                }
                if self.ripple_mode {
                    ui.label(theme::caps_label("RIPPLE", color::ACCENT));
                }
                // AU4 §5.1 rule 100: session state, not document state. Off
                // hides the overlay and with it all envelope hit-testing.
                let envelopes = ui
                    .add(
                        egui::Button::new("Envelopes")
                            .selected(show_envelopes)
                            .min_size(egui::vec2(72.0, 22.0)),
                    )
                    .on_hover_text(
                        "Show the gain envelope on audio clips. The band is the coarse gesture; \
                         the inspector's keyframe list is the exact one.",
                    );
                if envelopes.clicked() {
                    show_envelopes = !show_envelopes;
                }
                // AU5 §6.4 rule 129: beside `Envelopes`, and grey until there
                // is both a selected clip and a gap on its track to fill.
                let room_tone_block = self.room_tone_block();
                let room_tone = ui
                    .add_enabled(
                        room_tone_block.is_none(),
                        egui::Button::new(ROOM_TONE_BUTTON).min_size(egui::vec2(72.0, 22.0)),
                    )
                    .on_disabled_hover_text(room_tone_block.unwrap_or_default());
                let room_tone = if room_tone_block.is_none() {
                    room_tone.on_hover_text(
                        "Fill the gap nearest the playhead on the selected clip's track with \
                         room tone, captured from that track's longest silence.",
                    )
                } else {
                    room_tone
                };
                if room_tone.clicked() {
                    self.fill_room_tone_at_selection();
                }
                ui.separator();
                if icons::button(ui, Icon::Undo, "Undo (Ctrl+Z)").clicked() {
                    self.undo();
                }
                if icons::button(ui, Icon::Redo, "Redo (Ctrl+Shift+Z)").clicked() {
                    self.redo();
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("−").min_size(egui::vec2(22.0, 22.0)))
                    .on_hover_text("Zoom out")
                    .clicked()
                {
                    zoom_target = (zoom_target / 1.25).max(0.25);
                }
                ui.add(
                    egui::Slider::new(&mut zoom_target, 0.25..=20.0)
                        .logarithmic(true)
                        .show_value(false),
                )
                .on_hover_text(format!("Timeline zoom · {zoom_target:.2} px per frame"));
                if ui
                    .add(egui::Button::new("+").min_size(egui::vec2(22.0, 22.0)))
                    .on_hover_text("Zoom in")
                    .clicked()
                {
                    zoom_target = (zoom_target * 1.25).min(20.0);
                }
                if ui
                    .button("Fit")
                    .on_hover_text("Fit the whole project in view")
                    .clicked()
                {
                    let frames = project_duration.0.max(1) as f32;
                    let width = (ui.available_width() - 120.0).max(240.0);
                    zoom_target = (width / frames).clamp(0.25, 20.0);
                    scroll_target = 0.0;
                }
            },
        );

        let mut playhead_position = self.focused().position;
        if (old_zoom_target - zoom_target).abs() > f32::EPSILON {
            let playhead_before = playhead_position.0 as f32 * old_zoom_target;
            let playhead_after = playhead_position.0 as f32 * zoom_target;
            scroll_target = (scroll_target + playhead_after - playhead_before).max(0.0);
        }
        let pixels_per_frame = ui.ctx().animate_value_with_time(
            egui::Id::new("timeline-zoom-animation"),
            zoom_target,
            theme::motion::NAVIGATION,
        );
        let animated_scroll = ui.ctx().animate_value_with_time(
            egui::Id::new("timeline-scroll-animation"),
            scroll_target,
            theme::motion::NAVIGATION,
        );
        self.projects[project_index].timeline_zoom_target = zoom_target;
        self.projects[project_index].timeline_scroll_target = scroll_target;
        self.projects[project_index].pixels_per_frame = pixels_per_frame;
        self.projects[project_index].show_envelopes = show_envelopes;

        let document = Arc::clone(&self.focused().document);
        let mut selected_clip = self.focused().selected_clip;
        let mut selected_marker = self.focused().selected_marker;
        let mut selected_asset = self.focused().selected_asset;
        let mut title_text_focus = self.focused().title_text_focus;
        if document.tracks.is_empty() {
            ui.colored_label(color::TEXT_MUTED, "No tracks in this project");
            return;
        }
        let track_count = document.tracks.len().max(1) as f32;
        // Spare vertical space belongs to the editing surface: lanes stretch
        // (within bounds) so filmstrips and waveforms get taller, instead of
        // slack pooling at the bottom of the window.
        // Compact filmstrip lanes (M24): the timeline verifies and orients;
        // it no longer earns workbench height by default. Dragging the dock
        // taller grows lanes up to the classic height.
        let track_height = ((ui.available_height() - size::RULER_HEIGHT - size::CONTROL_HEIGHT)
            / track_count)
            .clamp(44.0, size::TRACK_HEIGHT);
        let total_height = size::RULER_HEIGHT + track_height * track_count;
        let marker_end = document
            .markers
            .iter()
            .filter(|marker| !is_internal_marker(marker))
            .map(|marker| marker.position.0.saturating_add(1))
            .max()
            .unwrap_or(0);
        let content_frames = document
            .duration
            .0
            .max(marker_end)
            .max(playhead_position.0.saturating_add(1))
            .max(i64::from(nominal_fps(document.fps)).saturating_mul(10));
        let viewport_width = (ui.available_width() - TRACK_LABEL_WIDTH - space::TWO).max(100.0);
        let content_width =
            ((content_frames as f32) * pixels_per_frame + space::SIX).max(viewport_width);
        let (major_tick, minor_tick) = tick_density(pixels_per_frame, document.fps);
        let clip_bounds = collect_clip_bounds(&document);
        let mut pending_operations = None;
        // AU4 §5.2 rule 104: the timeline's first coalescing path, used by the
        // envelope and nothing else. Clip move, trim, marker and playhead
        // drags keep their `drag_stopped`-only `pending_operations` batch
        // (rule 105); the two paths coexist.
        let mut envelope_edits = InspectorEdits::default();
        let mut envelope_drag = ui
            .data_mut(|data| data.get_temp::<EnvelopeDrag>(egui::Id::new(ENVELOPE_DRAG_MEMORY_ID)));
        // AU4 §5.1 rule 101 (AU4 §0 E49): the key the pointer is over this
        // frame, reported to the session so the *next* frame's
        // `keyboard_shortcuts` can answer Delete with it.
        let mut envelope_hover: Option<EnvelopeHover> = None;
        let mut seek = None;
        let mut scrub_started = false;
        let mut scrub_stopped = false;
        let mut snap_guide = None;
        let snapping_disabled = ui.input(|input| input.modifiers.alt);
        // Read once for the whole frame rather than once per audio clip: the
        // envelope's allocation predicate (rule 99) asks for it on every clip
        // that carries a curve, and the answer is the same screen position for
        // all of them.
        let envelope_pointer = ui.input(|input| input.pointer.hover_pos());

        ui.horizontal_top(|ui| {
            if let Some(operation) = paint_track_labels(ui, &document, total_height, track_height) {
                pending_operations = Some(vec![operation]);
            }
            let output = egui::ScrollArea::horizontal()
                .id_salt("timeline-scroll")
                .auto_shrink([false, false])
                .max_height(total_height + size::CONTROL_HEIGHT)
                .horizontal_scroll_offset(animated_scroll)
                .show(ui, |ui| {
                    let (rect, canvas_response) = ui.allocate_exact_size(
                        egui::vec2(content_width, total_height),
                        egui::Sense::click(),
                    );
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, radius::NONE, color::LETTERBOX);
                    paint_ruler(
                        &painter,
                        rect,
                        ui.clip_rect(),
                        pixels_per_frame,
                        document.fps,
                        content_frames,
                        major_tick,
                        minor_tick,
                    );

                    let mut clip_pointer_interaction = false;
                    let mut marker_pointer_interaction = false;
                    for (track_index, track) in document.tracks.iter().enumerate() {
                        let lane_top =
                            rect.top() + size::RULER_HEIGHT + track_index as f32 * track_height;
                        let lane = egui::Rect::from_min_size(
                            egui::pos2(rect.left(), lane_top),
                            egui::vec2(rect.width(), track_height),
                        );
                        painter.rect_filled(lane, radius::NONE, color::LETTERBOX);
                        painter.line_segment(
                            [lane.left_bottom(), lane.right_bottom()],
                            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
                        );

                        for clip in &track.clips {
                            let Ok(duration) = document.clip_duration(clip) else {
                                continue;
                            };
                            let asset = match &clip.content {
                                ClipContent::Media | ClipContent::Freeze(_) => {
                                    document.asset(clip.asset)
                                }
                                ClipContent::Title(_) => None,
                            };
                            if !matches!(&clip.content, ClipContent::Title(_)) && asset.is_none() {
                                continue;
                            }
                            let (source_fps, maximum_source_end) = match &clip.content {
                                ClipContent::Media => {
                                    asset.map_or((document.fps, TimeCode(i64::MAX)), |asset| {
                                        (
                                            kinewright_core::clip_effective_fps(asset.fps, clip)
                                                .unwrap_or(asset.fps),
                                            asset.duration,
                                        )
                                    })
                                }
                                ClipContent::Title(_) | ClipContent::Freeze(_) => {
                                    (document.fps, TimeCode(i64::MAX))
                                }
                            };
                            let x = rect.left() + clip.timeline_start.0 as f32 * pixels_per_frame;
                            let clip_width = (duration.0 as f32 * pixels_per_frame).max(24.0);
                            let clip_rect = egui::Rect::from_min_size(
                                egui::pos2(x, lane.top() + space::ONE),
                                egui::vec2(clip_width, lane.height() - space::TWO),
                            );
                            if !clip_rect.intersects(ui.clip_rect()) {
                                continue;
                            }
                            let body_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.top()),
                                egui::pos2(
                                    clip_rect.right() - EDGE_HANDLE_WIDTH,
                                    clip_rect.bottom(),
                                ),
                            );
                            let body = ui
                                .interact(
                                    body_rect,
                                    ui.make_persistent_id(("clip-body", clip.id.0)),
                                    egui::Sense::click_and_drag(),
                                )
                                .on_hover_text("Select or drag clip");
                            let left = ui
                                .interact(
                                    egui::Rect::from_min_max(
                                        clip_rect.min,
                                        egui::pos2(
                                            clip_rect.left() + EDGE_HANDLE_WIDTH,
                                            clip_rect.bottom(),
                                        ),
                                    ),
                                    ui.make_persistent_id(("clip-left", clip.id.0)),
                                    egui::Sense::drag(),
                                )
                                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                            let right = ui
                                .interact(
                                    egui::Rect::from_min_max(
                                        egui::pos2(
                                            clip_rect.right() - EDGE_HANDLE_WIDTH,
                                            clip_rect.top(),
                                        ),
                                        clip_rect.max,
                                    ),
                                    ui.make_persistent_id(("clip-right", clip.id.0)),
                                    egui::Sense::drag(),
                                )
                                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);

                            // AU4 §5.1 rule 98: the envelope's interact is
                            // allocated AFTER `body`, `left` and `right`,
                            // because egui 0.35.0 resolves overlapping
                            // interactive rects by last-registered-wins
                            // (`hit_test.rs:429-433`) — an interact allocated
                            // before the body would lose every tie. Rule 99:
                            // and only when a drag is in flight or the pointer
                            // is within 9 px of the line, so body drags and
                            // the two 6 px trim handles are untouched
                            // everywhere else.
                            let envelope_kind = match &clip.content {
                                ClipContent::Media => asset.map(|asset| asset.kind),
                                ClipContent::Title(_) | ClipContent::Freeze(_) => None,
                            };
                            // Rule 97: no band and no hit-testing under 24 px
                            // of clip width, so the coarse gesture is never
                            // offered where it cannot land.
                            let envelope_shown =
                                envelope_is_offered(duration, pixels_per_frame, show_envelopes);
                            let envelope_curve =
                                clip.audio_gain_curve.as_ref().filter(|_| envelope_shown);
                            if let Some(band) = envelope_kind
                                .filter(|_| envelope_shown)
                                .and_then(|kind| envelope_band_rect(clip_rect, kind))
                            {
                                let keys: &[Keyframe] =
                                    envelope_curve.map_or(&[], |curve| &curve.keyframes);
                                let points = envelope_points(band, duration, keys);
                                // AU4 §0 E53: a clip with no curve still shows
                                // a flat line at its parked
                                // `audio_gain_tenth_db`, so rule 101's "a
                                // click on the line inserts a key" reaches the
                                // *first* key too and the band is
                                // discoverable on an untouched audio clip.
                                let drawn = if keys.is_empty() {
                                    envelope_parked_polyline(band, clip.audio_gain_tenth_db)
                                } else {
                                    envelope_polyline(band, &points, keys)
                                };
                                let interact_rect = envelope_interact_rect(band, body_rect);
                                let dragging =
                                    envelope_drag.is_some_and(|drag| drag.clip == clip.id);
                                // The allocation predicate tests the rect the
                                // interact actually takes, not the whole band:
                                // a pointer over a 6 px trim handle within
                                // 9 px of the line would otherwise allocate an
                                // interact whose rect cannot contain it.
                                let near = envelope_pointer.is_some_and(|pointer| {
                                    envelope_near_curve(&drawn, interact_rect, pointer)
                                });
                                if dragging || near {
                                    // AU4 §0 E53: the parked line's offer is
                                    // click-only, so a clip drag that starts
                                    // on it still drags the clip — egui
                                    // hit-tests click and drag separately, and
                                    // a click-only widget over `body` yields
                                    // click: envelope, drag: body.
                                    let sense = if keys.is_empty() {
                                        egui::Sense::click()
                                    } else {
                                        egui::Sense::click_and_drag()
                                    };
                                    let envelope = ui.interact(
                                        interact_rect,
                                        ui.make_persistent_id(("clip-envelope", clip.id.0)),
                                        sense,
                                    );
                                    clip_pointer_interaction |=
                                        envelope.hovered() || envelope.dragged();
                                    // AU4 §5.1 rule 101 (AU4 §0 E49): the
                                    // pointer's key is reported to the session
                                    // at the end of the frame, because
                                    // `keyboard_shortcuts` runs before the
                                    // timeline paints and has to answer Delete
                                    // from the report, one frame old — the
                                    // matte overlay's `matte_expanded` →
                                    // `report_expanded` pattern.
                                    if envelope.hovered()
                                        && let Some(pointer) = envelope_pointer
                                        && let Some(index) = envelope_hit(&points, band, pointer)
                                    {
                                        envelope_hover = Some(EnvelopeHover {
                                            clip: clip.id,
                                            index,
                                        });
                                    }
                                    // The snap table is O(clips + markers) and
                                    // allocates, so it is built only on the
                                    // frames a gesture actually consumes it —
                                    // the same shape the clip body's own
                                    // `interacting` guard uses below.
                                    let interacting = envelope.drag_started()
                                        || is_live_drag(&envelope)
                                        || envelope.clicked();
                                    let candidates = if interacting {
                                        snap_candidates(
                                            &clip_bounds,
                                            &document.markers,
                                            clip.id,
                                            playhead_position.0,
                                        )
                                    } else {
                                        Vec::new()
                                    };
                                    if envelope.drag_started()
                                        && let Some(pointer) = envelope.interact_pointer_pos()
                                    {
                                        envelope_drag =
                                            envelope_hit(&points, band, pointer).map(|index| {
                                                EnvelopeDrag {
                                                    clip: clip.id,
                                                    index,
                                                }
                                            });
                                        if envelope_drag.is_some() {
                                            envelope_edits.begin_gesture();
                                        }
                                    }
                                    if let Some(drag) =
                                        envelope_drag.filter(|drag| drag.clip == clip.id)
                                        && let Some(stored) = keys.get(drag.index)
                                        && is_live_drag(&envelope)
                                        && let Some(pointer) = envelope.interact_pointer_pos()
                                    {
                                        let (at, guide) = envelope_snapped_local_frame(
                                            band,
                                            clip.timeline_start,
                                            duration,
                                            pointer.x,
                                            &candidates,
                                            minor_tick,
                                            pixels_per_frame,
                                            snapping_disabled,
                                        );
                                        if envelope.dragged() {
                                            snap_guide = guide.or(snap_guide);
                                        }
                                        let moved = envelope_move_key(
                                            keys,
                                            drag.index,
                                            at,
                                            envelope_grab_value(band, pointer.y, stored.value),
                                            duration,
                                        );
                                        // A frame that asks for the values the
                                        // document already holds is not an
                                        // edit (AU4 §5.2 rule 104).
                                        if moved != keys {
                                            envelope_edits.extend_live(
                                                vec![envelope_operation(clip.id, moved)],
                                                envelope_coalesce_key(clip.id),
                                            );
                                        }
                                    }
                                    if envelope.drag_stopped() {
                                        envelope_drag = None;
                                    }
                                    if envelope.clicked()
                                        && let Some(pointer) = envelope.interact_pointer_pos()
                                        && envelope_hit(&points, band, pointer).is_none()
                                    {
                                        let (at, _) = envelope_snapped_local_frame(
                                            band,
                                            clip.timeline_start,
                                            duration,
                                            pointer.x,
                                            &candidates,
                                            minor_tick,
                                            pixels_per_frame,
                                            snapping_disabled,
                                        );
                                        let inserted = match envelope_curve {
                                            Some(curve) => envelope_insert_key(curve, at),
                                            // AU4 §0 E53: the first key lands
                                            // at the value the flat line was
                                            // drawn at, through the same
                                            // `upsert_keyframe` the
                                            // inspector's `+ Key at playhead`
                                            // writes.
                                            None => {
                                                crate::inspector_ui::upsert_keyframe(
                                                    None,
                                                    at,
                                                    i64::from(clip.audio_gain_tenth_db),
                                                )
                                                .keyframes
                                            }
                                        };
                                        if inserted != keys {
                                            envelope_edits
                                                .push(envelope_operation(clip.id, inserted));
                                        }
                                    }
                                    // Removal is the secondary click or the
                                    // Delete/Backspace the session report
                                    // above arms: `KeyAction::Delete` runs
                                    // before the timeline paints, so the
                                    // keyboard half is answered in `keys.rs`
                                    // from that report (AU4 §0 E49).
                                    if envelope.secondary_clicked()
                                        && let Some(pointer) = envelope.interact_pointer_pos()
                                        && let Some(index) = envelope_hit(&points, band, pointer)
                                        && let Some(remaining) = envelope_remove_key(keys, index)
                                    {
                                        envelope_edits.push(envelope_operation(clip.id, remaining));
                                    }
                                }
                            }

                            clip_pointer_interaction |= body.hovered()
                                || body.dragged()
                                || left.hovered()
                                || left.dragged()
                                || right.hovered()
                                || right.dragged();
                            if body.clicked() {
                                selected_clip = Some(clip.id);
                                selected_marker = None;
                                selected_asset = asset.map(|asset| asset.id);
                            }
                            if body.double_clicked()
                                && matches!(&clip.content, ClipContent::Title(_))
                            {
                                title_text_focus = Some(clip.id);
                            }

                            let interacting = body.dragged()
                                || body.drag_stopped()
                                || left.dragged()
                                || left.drag_stopped()
                                || right.dragged()
                                || right.drag_stopped();
                            let candidates = if interacting {
                                snap_candidates(
                                    &clip_bounds,
                                    &document.markers,
                                    clip.id,
                                    playhead_position.0,
                                )
                            } else {
                                Vec::new()
                            };
                            let project_delta =
                                (body.drag_delta().x / pixels_per_frame).round() as i64;
                            let minimum_start = linked_minimum_primary_start(&document, clip.id);
                            let raw_start = clip
                                .timeline_start
                                .0
                                .saturating_add(project_delta)
                                .max(minimum_start);
                            let (snapped_start, body_guide) = if snapping_disabled {
                                (raw_start, None)
                            } else {
                                snap_move(
                                    raw_start,
                                    duration.0,
                                    &candidates,
                                    minor_tick,
                                    pixels_per_frame,
                                )
                            };
                            if body.dragged() {
                                snap_guide = body_guide.or(snap_guide);
                            }
                            if body.drag_stopped() && snapped_start != clip.timeline_start.0 {
                                pending_operations = Some(linked_move_operations(
                                    &document,
                                    clip.id,
                                    track.id,
                                    TimeCode(snapped_start),
                                ));
                            }

                            if left.drag_stopped() {
                                let raw_edge = clip
                                    .timeline_start
                                    .0
                                    .saturating_add(
                                        (left.drag_delta().x / pixels_per_frame).round() as i64,
                                    )
                                    .max(0);
                                let (edge, _) = if snapping_disabled {
                                    (raw_edge, None)
                                } else {
                                    nearest_snap(
                                        raw_edge,
                                        &candidates,
                                        minor_tick,
                                        pixels_per_frame,
                                    )
                                };
                                let source_delta = project_delta_to_source(
                                    edge.saturating_sub(clip.timeline_start.0),
                                    document.fps,
                                    source_fps,
                                );
                                let new_start = TimeCode(
                                    clip.source_range
                                        .start
                                        .0
                                        .saturating_add(source_delta)
                                        .clamp(0, clip.source_range.end.0.saturating_sub(1)),
                                );
                                if new_start != clip.source_range.start {
                                    pending_operations = linked_trim_operations(
                                        &document,
                                        clip.id,
                                        new_start..clip.source_range.end,
                                        TrimEdge::Left,
                                    )
                                    .ok();
                                }
                            }
                            if right.drag_stopped() {
                                let clip_end = clip.timeline_start.0.saturating_add(duration.0);
                                let raw_edge = clip_end.saturating_add(
                                    (right.drag_delta().x / pixels_per_frame).round() as i64,
                                );
                                let (edge, _) = if snapping_disabled {
                                    (raw_edge, None)
                                } else {
                                    nearest_snap(
                                        raw_edge,
                                        &candidates,
                                        minor_tick,
                                        pixels_per_frame,
                                    )
                                };
                                let source_delta = project_delta_to_source(
                                    edge.saturating_sub(clip_end),
                                    document.fps,
                                    source_fps,
                                );
                                let new_end = TimeCode(
                                    clip.source_range.end.0.saturating_add(source_delta).clamp(
                                        clip.source_range.start.0.saturating_add(1),
                                        maximum_source_end.0,
                                    ),
                                );
                                if new_end != clip.source_range.end {
                                    pending_operations = linked_trim_operations(
                                        &document,
                                        clip.id,
                                        clip.source_range.start..new_end,
                                        TrimEdge::Right,
                                    )
                                    .ok();
                                }
                            }

                            let draw_delta = if body.dragged() || body.drag_stopped() {
                                egui::vec2(
                                    (snapped_start - clip.timeline_start.0) as f32
                                        * pixels_per_frame,
                                    0.0,
                                )
                            } else {
                                egui::Vec2::ZERO
                            };
                            let draw_rect = clip_rect.translate(draw_delta);
                            let selected = selected_clip == Some(clip.id);
                            let dragging = body.dragged() || left.dragged() || right.dragged();
                            match (&clip.content, asset) {
                                (ClipContent::Media, Some(asset)) => paint_clip(
                                    &painter,
                                    ui.clip_rect(),
                                    &mut self.visual_cache,
                                    self.analysis.as_ref(),
                                    asset,
                                    clip.source_range.clone(),
                                    draw_rect,
                                    body.hovered() || left.hovered() || right.hovered(),
                                    selected,
                                    dragging,
                                    clip.transition_in
                                        .as_ref()
                                        .map(|transition| transition.duration),
                                    pixels_per_frame,
                                    duration,
                                    document.fps,
                                ),
                                (ClipContent::Title(title), _) => paint_title_clip(
                                    &painter,
                                    title,
                                    draw_rect,
                                    body.hovered() || left.hovered() || right.hovered(),
                                    selected,
                                    dragging,
                                    clip.transition_in
                                        .as_ref()
                                        .map(|transition| transition.duration),
                                    pixels_per_frame,
                                ),
                                (ClipContent::Freeze(freeze), Some(asset)) => paint_freeze_clip(
                                    &painter,
                                    ui.clip_rect(),
                                    &mut self.visual_cache,
                                    self.analysis.as_ref(),
                                    asset,
                                    freeze.source_frame,
                                    draw_rect,
                                    body.hovered() || left.hovered() || right.hovered(),
                                    selected,
                                    dragging,
                                    clip.transition_in
                                        .as_ref()
                                        .map(|transition| transition.duration),
                                    pixels_per_frame,
                                ),
                                (ClipContent::Media | ClipContent::Freeze(_), None) => {}
                            }
                            if clip.content.is_media() && clip.speed_percent != 100 {
                                paint_speed_badge(&painter, draw_rect, clip.speed_percent);
                            }
                            // AU4 §5.1 rule 102: the rubber band paints last,
                            // over the waveform it shares a rect with.
                            if let Some(band) = envelope_kind
                                .filter(|_| envelope_shown)
                                .and_then(|kind| envelope_band_rect(draw_rect, kind))
                            {
                                paint_clip_envelope(
                                    &painter,
                                    band,
                                    duration,
                                    envelope_curve.map_or(&[], |curve| &curve.keyframes),
                                    clip.audio_gain_tenth_db,
                                    selected,
                                );
                            }
                        }
                    }

                    for marker in document
                        .markers
                        .iter()
                        .filter(|marker| !is_internal_marker(marker))
                    {
                        let x = rect.left() + marker.position.0 as f32 * pixels_per_frame;
                        let marker_rect = egui::Rect::from_center_size(
                            egui::pos2(x + 3.0, rect.top() + size::RULER_HEIGHT / 2.0),
                            egui::vec2(14.0, size::RULER_HEIGHT),
                        );
                        let response = ui
                            .interact(
                                marker_rect,
                                ui.make_persistent_id(("timeline-marker", marker.id.0)),
                                egui::Sense::click_and_drag(),
                            )
                            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                            .on_hover_text(&marker.label);
                        marker_pointer_interaction |=
                            response.hovered() || response.dragged() || response.drag_stopped();
                        if response.clicked() || response.drag_started() {
                            selected_marker = Some(marker.id);
                            selected_clip = None;
                        }
                        if response.secondary_clicked() {
                            selected_marker = Some(marker.id);
                            selected_clip = None;
                            pending_operations =
                                Some(vec![Operation::RemoveMarker { marker: marker.id }]);
                        }
                        let raw = marker.position.0.saturating_add(
                            (response.drag_delta().x / pixels_per_frame).round() as i64,
                        );
                        let candidates = marker_snap_candidates(
                            &clip_bounds,
                            &document.markers,
                            marker.id,
                            playhead_position.0,
                        );
                        let (snapped, guide) = if snapping_disabled {
                            (raw.max(0), None)
                        } else {
                            nearest_snap(raw.max(0), &candidates, minor_tick, pixels_per_frame)
                        };
                        if response.dragged() {
                            snap_guide = guide.or(snap_guide);
                        }
                        if response.drag_stopped() && snapped != marker.position.0 {
                            pending_operations = Some(vec![Operation::MoveMarker {
                                marker: marker.id,
                                to: TimeCode(snapped),
                            }]);
                        }
                        let draw_position = if response.dragged() || response.drag_stopped() {
                            snapped
                        } else {
                            marker.position.0
                        };
                        paint_project_marker(
                            &painter,
                            rect,
                            draw_position,
                            marker.color_token,
                            selected_marker == Some(marker.id),
                            response.dragged(),
                            pixels_per_frame,
                        );
                    }

                    let playhead_x = rect.left() + playhead_position.0 as f32 * pixels_per_frame;
                    painter.line_segment(
                        [
                            egui::pos2(playhead_x, rect.top()),
                            egui::pos2(playhead_x, rect.bottom()),
                        ],
                        egui::Stroke::new(2.0, color::ACCENT),
                    );
                    let handle_points = vec![
                        egui::pos2(playhead_x - 5.0, rect.top()),
                        egui::pos2(playhead_x + 5.0, rect.top()),
                        egui::pos2(playhead_x + 5.0, rect.top() + 5.0),
                        egui::pos2(playhead_x, rect.top() + 9.0),
                        egui::pos2(playhead_x - 5.0, rect.top() + 5.0),
                    ];
                    painter.add(egui::Shape::convex_polygon(
                        handle_points,
                        color::ACCENT,
                        egui::Stroke::NONE,
                    ));
                    let playhead_response = ui
                        .interact(
                            egui::Rect::from_center_size(
                                egui::pos2(playhead_x, rect.top() + size::RULER_HEIGHT / 2.0),
                                egui::vec2(14.0, size::RULER_HEIGHT),
                            ),
                            ui.make_persistent_id("timeline-playhead"),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                        .on_hover_text("Drag playhead");
                    if playhead_response.drag_started() {
                        scrub_started = true;
                    }
                    if (playhead_response.dragged() || playhead_response.drag_stopped())
                        && let Some(pointer) = playhead_response.interact_pointer_pos()
                    {
                        let raw = ((pointer.x - rect.left()) / pixels_per_frame).round() as i64;
                        let candidates = clip_bounds
                            .iter()
                            .flat_map(|bounds| [bounds.start, bounds.end])
                            .chain(
                                document
                                    .markers
                                    .iter()
                                    .filter(|marker| !is_internal_marker(marker))
                                    .map(|marker| marker.position.0),
                            )
                            .collect::<Vec<_>>();
                        let (snapped, guide) = if snapping_disabled {
                            (raw.max(0), None)
                        } else {
                            nearest_snap(raw.max(0), &candidates, minor_tick, pixels_per_frame)
                        };
                        seek = Some(TimeCode(snapped));
                        snap_guide = guide.or(snap_guide);
                    }
                    if playhead_response.drag_stopped() {
                        scrub_stopped = true;
                    }
                    if canvas_response.clicked()
                        && !clip_pointer_interaction
                        && !marker_pointer_interaction
                        && !playhead_response.hovered()
                        && let Some(pointer) = canvas_response.interact_pointer_pos()
                    {
                        let frame = ((pointer.x - rect.left()) / pixels_per_frame).round() as i64;
                        seek = Some(TimeCode(frame.max(0)));
                        scrub_stopped = true;
                    }

                    if let Some(guide) = snap_guide {
                        let x = rect.left() + guide as f32 * pixels_per_frame;
                        painter.line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(1.0, color::ACCENT),
                        );
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(x, rect.top() + size::RULER_HEIGHT - 7.0),
                                egui::pos2(x + 4.0, rect.top() + size::RULER_HEIGHT - 3.0),
                                egui::pos2(x, rect.top() + size::RULER_HEIGHT + 1.0),
                                egui::pos2(x - 4.0, rect.top() + size::RULER_HEIGHT - 3.0),
                            ],
                            color::ACCENT,
                            egui::Stroke::NONE,
                        ));
                    }
                });
            if (output.state.offset.x - animated_scroll).abs() > 0.5 {
                scroll_target = output.state.offset.x;
            }
        });

        ui.data_mut(|data| {
            let id = egui::Id::new(ENVELOPE_DRAG_MEMORY_ID);
            match envelope_drag {
                Some(drag) => {
                    data.insert_temp(id, drag);
                }
                None => data.remove::<EnvelopeDrag>(id),
            }
        });
        if let Some(operations) = pending_operations {
            self.send_operations(operations);
        }
        // AU4 §5.2 rule 104: one `submit_inspector_edits` at the end of the
        // function, exactly as the CC5 matte overlay does it.
        self.submit_inspector_edits(envelope_edits);
        if scrub_started {
            self.resume_after_scrub = self.playing;
            // CC6 §8.2: the drag pauses the transport, so `playing` stops
            // describing a moving playhead the instant the scrub begins. The
            // QC mask is told directly.
            self.qc_mask.set_scrubbing(true);
            if self.playing {
                self.playback.pause();
            }
        }
        if let Some(position) = seek {
            let maximum = self.focused().document.duration.0.saturating_sub(1).max(0);
            playhead_position = TimeCode(position.0.clamp(0, maximum));
            self.playback.request_frame(playhead_position);
            if scrub_stopped {
                self.playback.seek(playhead_position);
            }
        }
        if scrub_stopped {
            self.qc_mask.set_scrubbing(false);
            if self.resume_after_scrub {
                self.playback.play(playhead_position);
            }
            self.resume_after_scrub = false;
        }
        let session = &mut self.projects[project_index];
        let previous_selected_asset = session.selected_asset;
        session.position = playhead_position;
        session.selected_clip = selected_clip;
        session.selected_marker = selected_marker;
        session.selected_asset = selected_asset;
        if previous_selected_asset != selected_asset {
            if let Some(asset_id) = selected_asset {
                // The local selection is assigned above so the timeline can
                // report it immediately; clear it for cue_source_asset to
                // establish fresh marks, cursor, and visible route defaults.
                session.selected_asset = None;
                session.cue_source_asset(asset_id);
            } else {
                session.reconcile_source_state();
            }
        }
        session.title_text_focus = title_text_focus;
        session.timeline_scroll_target = scroll_target;
        session.envelope_hover = envelope_hover;
    }
}

// Track indices are small and intentionally projected into egui's f32 coordinate space.
#[allow(clippy::cast_precision_loss)]
fn paint_track_labels(
    ui: &mut egui::Ui,
    document: &kinewright_core::Document,
    total_height: f32,
    track_height: f32,
) -> Option<Operation> {
    let mut pending_operation = None;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(TRACK_LABEL_WIDTH, total_height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, radius::NONE, color::PANEL);
    theme::paint_caps(
        &painter,
        egui::pos2(
            rect.left() + space::TWO,
            rect.top() + size::RULER_HEIGHT / 2.0,
        ),
        egui::Align2::LEFT_CENTER,
        "TRACKS",
        color::TEXT_MUTED,
    );
    for (index, track) in document.tracks.iter().enumerate() {
        let top = rect.top() + size::RULER_HEIGHT + index as f32 * track_height;
        let lane = egui::Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(rect.width(), track_height),
        );
        painter.rect_filled(lane, radius::NONE, color::PANEL);
        painter.line_segment(
            [lane.left_bottom(), lane.right_bottom()],
            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        );
        // The mixer names the same track the same way; one function owns the
        // spelling so the two surfaces cannot drift (AU1 §5.1).
        let (label, icon) = track_caption_and_icon(track.kind, index);
        painter.text(
            egui::pos2(lane.left() + space::TWO, lane.center().y - 8.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
            color::TEXT_PRIMARY,
        );
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(lane.left() + space::TWO, lane.center().y + 2.0),
            egui::vec2(size::ICON_SM, size::ICON_SM),
        );
        icon.image(size::ICON_SM)
            .tint(color::TEXT_MUTED)
            .paint_at(ui, icon_rect);

        // One operation per frame leaves this column: the header sends its
        // edits as a single non-coalesced batch, so a second click in the same
        // frame would open a second undo entry for a gesture nobody made.
        if let Some(operation) = paint_mix_toggles(ui, &painter, lane, document, track) {
            pending_operation = Some(operation);
        }
        if let Some(operation) = paint_sync_lock_toggle(ui, &painter, lane, track) {
            pending_operation = Some(operation);
        }
    }
    pending_operation
}

/// The track header's mute and solo toggles (AU1 §5.2).
///
/// The column sits between the caption and the sync-lock button; the two 14 px
/// squares are stacked and centred on the lane. Each returns the whole mix
/// state with one flag flipped, so a click is one idempotent operation and one
/// undo entry.
fn paint_mix_toggles(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    lane: egui::Rect,
    document: &kinewright_core::Document,
    track: &kinewright_core::Track,
) -> Option<Operation> {
    const TOGGLE: f32 = size::ICON_SM;
    let mix = document.track_mix(track.id);
    let column_x = mix_toggle_column_x(lane);
    let top = lane.center().y - TOGGLE - space::ONE / 2.0;
    let mut pending = None;
    for (offset, toggle, id) in [
        (0.0, MixToggle::Mute, "track-mute"),
        (TOGGLE + space::ONE, MixToggle::Solo, "track-solo"),
    ] {
        let rect = egui::Rect::from_min_size(
            egui::pos2(column_x, top + offset),
            egui::vec2(TOGGLE, TOGGLE),
        );
        let response = ui
            .interact(
                rect,
                ui.make_persistent_id((id, track.id.0)),
                egui::Sense::click(),
            )
            .on_hover_text(toggle.tooltip());
        paint_mix_toggle(painter, rect, toggle, toggle.is_on(&mix));
        if response.clicked() {
            pending = Some(track_mix_toggle_operation(&mix, toggle));
        }
    }
    pending
}

/// The left edge of the track header's mute/solo column (AU1 §5.2).
///
/// The column is inset from the lane's right edge by the sync-lock button and
/// the rhythm on either side of it, so the caption keeps the rest of the
/// label strip. The test asserts the resulting numbers, so changing this
/// expression fails it.
fn mix_toggle_column_x(lane: egui::Rect) -> f32 {
    lane.right() - space::TWO - size::ICON_BUTTON - space::TWO - size::ICON_SM
}

fn paint_sync_lock_toggle(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    lane: egui::Rect,
    track: &kinewright_core::Track,
) -> Option<Operation> {
    let toggle_rect = egui::Rect::from_center_size(
        egui::pos2(
            lane.right() - space::TWO - size::ICON_BUTTON / 2.0,
            lane.center().y,
        ),
        egui::vec2(size::ICON_BUTTON, size::TIMELINE_TOOLBAR_HEIGHT),
    );
    let tooltip = if track.sync_lock {
        "Sync lock on: ripple edits on other tracks shift this track to preserve sync"
    } else {
        "Sync lock off: this track runs free and stays put during ripple edits on other tracks"
    };
    let response = ui
        .interact(
            toggle_rect,
            ui.make_persistent_id(("track-sync-lock", track.id.0)),
            egui::Sense::click(),
        )
        .on_hover_text(tooltip);
    let lock_icon = if track.sync_lock {
        Icon::Lock
    } else {
        Icon::Unlock
    };
    let lock_color = if track.sync_lock {
        color::TEXT_MUTED
    } else {
        color::STATUS_WARNING
    };
    if response.hovered() || !track.sync_lock {
        painter.rect_filled(toggle_rect, radius::SM, color::SURFACE_RAISED);
    }
    let lock_offset = if track.sync_lock {
        0.0
    } else {
        space::ONE_HALF
    };
    let lock_rect = egui::Rect::from_center_size(
        egui::pos2(toggle_rect.center().x, toggle_rect.center().y + lock_offset),
        egui::vec2(size::ICON_SM, size::ICON_SM),
    );
    lock_icon
        .image(size::ICON_SM)
        .tint(lock_color)
        .paint_at(ui, lock_rect);
    if !track.sync_lock {
        theme::paint_caps(
            painter,
            egui::pos2(toggle_rect.center().x, toggle_rect.top() + space::ONE_HALF),
            egui::Align2::CENTER_CENTER,
            "FREE",
            color::STATUS_WARNING,
        );
    }
    response.clicked().then_some(Operation::SetTrackSyncLock {
        track: track.id,
        locked: !track.sync_lock,
    })
}

// Ruler bounds intentionally convert between exact frames and f32 viewport pixels.
#[allow(
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]
fn paint_ruler(
    painter: &egui::Painter,
    rect: egui::Rect,
    clip_rect: egui::Rect,
    pixels_per_frame: f32,
    fps: Rational,
    content_frames: i64,
    major_tick: i64,
    minor_tick: i64,
) {
    let ruler = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), size::RULER_HEIGHT));
    painter.rect_filled(ruler, radius::NONE, color::PANEL);
    painter.line_segment(
        [ruler.left_bottom(), ruler.right_bottom()],
        egui::Stroke::new(1.0, color::BORDER_STRONG),
    );
    let visible_start =
        (((clip_rect.left() - rect.left()) / pixels_per_frame).floor() as i64).max(0);
    let visible_end =
        (((clip_rect.right() - rect.left()) / pixels_per_frame).ceil() as i64).min(content_frames);
    let mut frame = visible_start - visible_start.rem_euclid(minor_tick);
    while frame <= visible_end {
        let x = rect.left() + frame as f32 * pixels_per_frame;
        let major = frame.rem_euclid(major_tick) == 0;
        painter.line_segment(
            [
                egui::pos2(x, ruler.bottom()),
                egui::pos2(x, ruler.bottom() - if major { 9.0 } else { 4.0 }),
            ],
            egui::Stroke::new(
                1.0,
                if major {
                    color::TEXT_SECONDARY
                } else {
                    color::BORDER_STRONG
                },
            ),
        );
        if major {
            painter.text(
                egui::pos2(x + space::ONE, ruler.top() + space::ONE),
                egui::Align2::LEFT_TOP,
                format_timecode(TimeCode(frame), fps),
                theme::ruler_font(),
                color::TEXT_SECONDARY,
            );
        }
        frame = frame.saturating_add(minor_tick);
    }
}

// Marker frame positions intentionally project into the ruler's f32 pixel space.
#[allow(clippy::cast_precision_loss)]
fn paint_project_marker(
    painter: &egui::Painter,
    timeline_rect: egui::Rect,
    position: i64,
    color_token: u8,
    selected: bool,
    dragging: bool,
    pixels_per_frame: f32,
) {
    let x = timeline_rect.left() + position as f32 * pixels_per_frame;
    // Markers exist to draw the eye to moments; their tokens are the chromatic
    // status palette, never the greyscale text ramp (which camouflages against
    // the ruler).
    let token_color = match color_token {
        1 => color::STATUS_SUCCESS,
        2 => color::STATUS_WARNING,
        3 => color::STATUS_DANGER,
        _ => color::ACCENT,
    };
    let marker_color = token_color;
    let top = timeline_rect.top() + space::ONE;
    painter.line_segment(
        [
            egui::pos2(x, top),
            egui::pos2(x, timeline_rect.top() + size::RULER_HEIGHT - space::HALF),
        ],
        egui::Stroke::new(if selected || dragging { 2.0 } else { 1.5 }, marker_color),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x, top),
            egui::pos2(x + 10.0, top + 4.0),
            egui::pos2(x, top + 8.0),
        ],
        if dragging {
            color::SURFACE_ACTIVE
        } else {
            marker_color
        },
        egui::Stroke::new(if selected { 1.0 } else { 0.0 }, color::ACCENT_DIM_BORDER),
    ));
}

// One clip paint pass keeps its layered drawing order explicit and reviewable.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn paint_clip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &dyn Analysis,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
    transition_duration: Option<TimeCode>,
    pixels_per_frame: f32,
    project_duration: TimeCode,
    project_fps: Rational,
) {
    painter.rect_filled(rect, radius::SM, color::SURFACE);
    if rect.intersects(clip_bounds)
        && matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo)
    {
        paint_filmstrip(
            painter,
            clip_bounds,
            visual_cache,
            media,
            asset,
            source_range.clone(),
            rect,
        );
    }
    // AU4 §5.1 rule 95a: the band comes from `envelope_band_rect`, the one
    // pure helper the envelope's paint, hit and interact all take, so the ride
    // and the waveform cannot drift apart by the 2 px in x and 4 px in y the
    // shrink costs. The scrim fills the same band before that shrink, taken
    // from the same expression rather than rebuilt here.
    if rect.intersects(clip_bounds)
        && let Some(band) = envelope_band_unshrunk(rect, asset.kind)
        && let Some(waveform_rect) = envelope_band_rect(rect, asset.kind)
    {
        // A strong scrim keeps waveforms legible over saturated footage.
        painter.rect_filled(band, radius::XS, color::MEDIA_SCRIM_78);
        if let Some(waveform) = visual_cache.waveform(media, asset) {
            paint_waveform(
                painter,
                clip_bounds,
                waveform.as_ref(),
                asset,
                source_range.clone(),
                waveform_rect,
                selected,
            );
        }
    }
    paint_transition_affordance(painter, rect, transition_duration, pixels_per_frame);

    let label_strip = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), (rect.top() + 19.0).min(rect.bottom())),
    );
    painter.rect_filled(label_strip, radius::SM, color::MEDIA_SCRIM_78);
    painter.text(
        egui::pos2(rect.left() + space::TWO, rect.top() + space::ONE),
        egui::Align2::LEFT_TOP,
        &asset.name,
        egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
        color::TEXT_PRIMARY,
    );
    if rect.width() >= 140.0 {
        // Project duration, not source length: they differ on speeded clips.
        painter.text(
            egui::pos2(rect.right() - space::TWO, rect.top() + space::ONE),
            egui::Align2::RIGHT_TOP,
            format_timecode(project_duration, project_fps),
            theme::code_font(),
            color::TEXT_SECONDARY,
        );
    }
    if selected {
        painter.rect_filled(rect, radius::SM, color::ACCENT_WASH);
    }
    if dragging {
        painter.rect_filled(rect, radius::SM, color::SURFACE_ACTIVE);
    }
    paint_derived_markers(
        painter,
        clip_bounds,
        media,
        asset,
        source_range.clone(),
        rect,
    );
    paint_clip_chrome(painter, rect, hovered, selected, dragging);
    if hovered || selected || dragging {
        painter.rect_filled(
            egui::Rect::from_min_max(
                rect.min,
                egui::pos2(rect.left() + EDGE_HANDLE_WIDTH, rect.bottom()),
            ),
            radius::XS,
            color::ACCENT_DIM_BORDER,
        );
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.right() - EDGE_HANDLE_WIDTH, rect.top()),
                rect.max,
            ),
            radius::XS,
            color::ACCENT_DIM_BORDER,
        );
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn paint_freeze_clip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &dyn Analysis,
    asset: &MediaAsset,
    source_frame: TimeCode,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
    transition_duration: Option<TimeCode>,
    pixels_per_frame: f32,
) {
    painter.rect_filled(rect, radius::SM, color::SURFACE);
    let visible = rect.intersect(clip_bounds);
    if visible.is_positive()
        && let Some(texture) = visual_cache.thumbnail(media, asset, source_frame, THUMBNAIL_WIDTH)
    {
        let first = ((visible.left() - rect.left()) / FILMSTRIP_TILE_WIDTH)
            .floor()
            .max(0.0) as usize;
        let last = ((visible.right() - rect.left()) / FILMSTRIP_TILE_WIDTH)
            .ceil()
            .max(1.0) as usize;
        for tile in first..last {
            let left = rect.left() + tile as f32 * FILMSTRIP_TILE_WIDTH;
            let tile_rect = egui::Rect::from_min_max(
                egui::pos2(left, rect.top()),
                egui::pos2(
                    (left + FILMSTRIP_TILE_WIDTH).min(rect.right()),
                    rect.bottom(),
                ),
            );
            painter.image(
                texture.id(),
                tile_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                color::MEDIA_TINT_78,
            );
        }
    }
    paint_transition_affordance(painter, rect, transition_duration, pixels_per_frame);
    let label_strip = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), (rect.top() + 19.0).min(rect.bottom())),
    );
    painter.rect_filled(label_strip, radius::SM, color::MEDIA_SCRIM_78);
    painter.text(
        egui::pos2(rect.left() + space::TWO, rect.top() + space::ONE),
        egui::Align2::LEFT_TOP,
        &asset.name,
        egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
        color::TEXT_PRIMARY,
    );
    theme::paint_caps(
        painter,
        egui::pos2(rect.right() - space::TWO, rect.top() + space::ONE),
        egui::Align2::RIGHT_TOP,
        "HOLD",
        color::TEXT_SECONDARY,
    );
    if selected {
        painter.rect_filled(rect, radius::SM, color::ACCENT_WASH);
    }
    if dragging {
        painter.rect_filled(rect, radius::SM, color::SURFACE_ACTIVE);
    }
    paint_clip_chrome(painter, rect, hovered, selected, dragging);
}

#[allow(clippy::too_many_arguments)]
fn paint_title_clip(
    painter: &egui::Painter,
    title: &Title,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
    transition_duration: Option<TimeCode>,
    pixels_per_frame: f32,
) {
    // Unselected titles are a raised surface with a wash badge; selection
    // adds ACCENT_WASH + dim border and never outranks the playhead.
    let fill = if dragging || hovered {
        color::SURFACE_ACTIVE
    } else {
        color::SURFACE_RAISED
    };
    painter.rect_filled(rect, radius::SM, fill);
    paint_transition_affordance(painter, rect, transition_duration, pixels_per_frame);
    painter.rect_filled(
        egui::Rect::from_min_max(
            rect.min,
            egui::pos2((rect.left() + 28.0).min(rect.right()), rect.bottom()),
        ),
        radius::SM,
        color::ACCENT_WASH,
    );
    painter.text(
        egui::pos2(rect.left() + space::TWO, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "T",
        egui::FontId::new(type_size::HEADING, egui::FontFamily::Proportional),
        color::TEXT_PRIMARY,
    );
    let label = title.text.lines().next().unwrap_or("Title");
    painter.text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
        color::TEXT_PRIMARY,
    );
    if selected {
        painter.rect_filled(rect, radius::SM, color::ACCENT_WASH);
    }
    paint_clip_chrome(painter, rect, hovered, selected, dragging);
}

fn paint_clip_chrome(
    painter: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
) {
    let stroke = if dragging {
        egui::Stroke::new(2.0, color::ACCENT_DIM_BORDER)
    } else if selected {
        egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)
    } else if hovered {
        egui::Stroke::new(1.0, color::BORDER_SUBTLE)
    } else {
        egui::Stroke::NONE
    };
    if stroke.width > 0.0 {
        painter.rect_stroke(rect, radius::SM, stroke, egui::StrokeKind::Inside);
    }
}

// Transition frame widths are intentionally projected into f32 timeline pixels.
#[allow(clippy::cast_precision_loss)]
fn paint_transition_affordance(
    painter: &egui::Painter,
    rect: egui::Rect,
    duration: Option<TimeCode>,
    pixels_per_frame: f32,
) {
    let Some(duration) = duration.filter(|duration| duration.0 > 0) else {
        return;
    };
    let left = rect.left() + EDGE_HANDLE_WIDTH;
    let width = duration.0 as f32 * pixels_per_frame;
    let right = (left + width).min(rect.right() - EDGE_HANDLE_WIDTH);
    if right <= left || rect.height() <= space::ONE * 2.0 {
        return;
    }
    let top = rect.top() + space::ONE;
    let bottom = rect.bottom() - space::ONE;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(left, top),
            egui::pos2(left, bottom),
            egui::pos2(right, bottom),
        ],
        color::MEDIA_SCRIM_78,
        egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER),
    ));
}

// Derived source-frame positions are intentionally projected into f32 clip pixels.
#[allow(clippy::cast_precision_loss)]
fn paint_derived_markers(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    media: &dyn Analysis,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
) {
    let visible = rect.intersect(clip_bounds);
    if !visible.is_positive() {
        return;
    }
    let source_span = source_range
        .end
        .0
        .saturating_sub(source_range.start.0)
        .max(1);
    let source_x = |frame: TimeCode| {
        rect.left()
            + frame.0.saturating_sub(source_range.start.0) as f32 / source_span as f32
                * rect.width()
    };

    if let SilenceStatus::Ready(silences) = media.silence_status(asset) {
        for span in &silences.spans {
            let start = span.source_start.max(source_range.start);
            let end = span.source_end.min(source_range.end);
            if end.0.saturating_sub(start.0) < 6 {
                continue;
            }
            let underline = egui::Rect::from_min_max(
                egui::pos2(source_x(start).max(visible.left()), rect.bottom() - 3.0),
                egui::pos2(source_x(end).min(visible.right()), rect.bottom() - 1.0),
            );
            if underline.is_positive() {
                painter.rect_filled(underline, radius::NONE, color::TEXT_MUTED);
            }
        }
    }

    if let SceneStatus::Ready(scenes) = media.scene_status(asset) {
        for change in &scenes.changes {
            if change.confidence_basis_points < 1_000
                || change.source_frame < source_range.start
                || change.source_frame >= source_range.end
            {
                continue;
            }
            let x = source_x(change.source_frame);
            if x < visible.left() || x > visible.right() {
                continue;
            }
            painter.line_segment(
                [
                    egui::pos2(x, rect.top() + 19.0),
                    egui::pos2(x, rect.bottom() - 3.0),
                ],
                egui::Stroke::new(1.0, color::TEXT_SECONDARY),
            );
        }
    }
}

// Filmstrip sampling intentionally converts between bounded pixel columns and source frames.
#[allow(
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn paint_filmstrip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &dyn Analysis,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
) {
    let visible = rect.intersect(clip_bounds);
    if !visible.is_positive() {
        return;
    }
    let tile_count = (rect.width() / FILMSTRIP_TILE_WIDTH).ceil().max(1.0) as usize;
    let first = (((visible.left() - rect.left()) / FILMSTRIP_TILE_WIDTH).floor() as usize)
        .min(tile_count.saturating_sub(1));
    let last =
        (((visible.right() - rect.left()) / FILMSTRIP_TILE_WIDTH).ceil() as usize).min(tile_count);
    let source_span = source_range.end.0.saturating_sub(source_range.start.0);
    for tile in first..last.max(first + 1) {
        let left = rect.left() + tile as f32 * FILMSTRIP_TILE_WIDTH;
        let tile_rect = egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(
                (left + FILMSTRIP_TILE_WIDTH).min(rect.right()),
                rect.bottom(),
            ),
        );
        let ratio = (tile as f64 + 0.5) / tile_count as f64;
        let source_at = TimeCode(
            source_range
                .start
                .0
                .saturating_add((source_span as f64 * ratio).round() as i64)
                .min(source_range.end.0.saturating_sub(1)),
        );
        if let Some(texture) = visual_cache.thumbnail(media, asset, source_at, THUMBNAIL_WIDTH) {
            painter.image(
                texture.id(),
                tile_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                color::MEDIA_TINT_78,
            );
        }
    }
}

// Waveform rasterization intentionally converts bounded pixels and peak indices across domains.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn paint_waveform(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    waveform: &WaveformData,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
    selected: bool,
) {
    if waveform.peaks.is_empty() || asset.duration.0 <= 0 {
        return;
    }
    let visible = rect.intersect(clip_bounds);
    if !visible.is_positive() {
        return;
    }
    let columns = visible.width().ceil().max(1.0) as usize;
    let center = rect.center().y;
    let amplitude = rect.height() / 2.0;
    let source_span = source_range
        .end
        .0
        .saturating_sub(source_range.start.0)
        .max(1);
    let peak_count = waveform.peaks.len();
    for column in 0..columns {
        let x = visible.left() + column as f32;
        let clip_ratio = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        let source_frame = source_range.start.0 as f64 + source_span as f64 * f64::from(clip_ratio);
        let peak_index =
            ((source_frame / asset.duration.0 as f64) * peak_count as f64).floor() as usize;
        let peak = waveform.peaks[peak_index.min(peak_count.saturating_sub(1))];
        let minimum = f32::from(peak.minimum) / f32::from(i16::MAX);
        let maximum = f32::from(peak.maximum) / f32::from(i16::MAX);
        painter.line_segment(
            [
                egui::pos2(x, center - maximum * amplitude),
                egui::pos2(x, center - minimum * amplitude),
            ],
            egui::Stroke::new(
                1.0,
                if selected {
                    color::TEXT_PRIMARY
                } else {
                    color::TEXT_PRIMARY_64
                },
            ),
        );
    }
}

pub(crate) fn linked_members(document: &Document, primary: ClipId) -> Vec<(TrackId, Clip)> {
    let Some(primary_clip) = document.clip(primary) else {
        return Vec::new();
    };
    let Some(link) = primary_clip.link else {
        return document
            .tracks
            .iter()
            .find_map(|track| {
                track
                    .clips
                    .iter()
                    .find(|clip| clip.id == primary)
                    .cloned()
                    .map(|clip| vec![(track.id, clip)])
            })
            .unwrap_or_default();
    };
    document
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .clips
                .iter()
                .filter(move |clip| clip.link == Some(link))
                .cloned()
                .map(move |clip| (track.id, clip))
        })
        .collect()
}

fn linked_minimum_primary_start(document: &Document, primary: ClipId) -> i64 {
    let Some(primary_start) = document.clip(primary).map(|clip| clip.timeline_start.0) else {
        return 0;
    };
    let minimum_member = linked_members(document, primary)
        .iter()
        .map(|(_, clip)| clip.timeline_start.0)
        .min()
        .unwrap_or(primary_start);
    primary_start.saturating_sub(minimum_member)
}

fn linked_move_operations(
    document: &Document,
    primary: ClipId,
    primary_track: TrackId,
    to: TimeCode,
) -> Vec<Operation> {
    let Some(primary_start) = document.clip(primary).map(|clip| clip.timeline_start.0) else {
        return Vec::new();
    };
    let delta = to.0.saturating_sub(primary_start);
    let mut members = linked_members(document, primary);
    members.sort_by(|(left_track, left), (right_track, right)| {
        let track_order = left_track.cmp(right_track);
        if track_order != std::cmp::Ordering::Equal {
            return track_order;
        }
        if delta > 0 {
            right.timeline_start.cmp(&left.timeline_start)
        } else {
            left.timeline_start.cmp(&right.timeline_start)
        }
    });
    members
        .into_iter()
        .filter_map(|(track, clip)| {
            let target = TimeCode(clip.timeline_start.0.saturating_add(delta));
            (target != clip.timeline_start || (clip.id == primary && track != primary_track))
                .then_some(Operation::MoveClip {
                    clip: clip.id,
                    to_track: if clip.id == primary {
                        primary_track
                    } else {
                        track
                    },
                    to: target,
                })
        })
        .collect()
}

fn linked_trim_operations(
    document: &Document,
    primary: ClipId,
    new_source: std::ops::Range<TimeCode>,
    edge: TrimEdge,
) -> Result<Vec<Operation>, String> {
    let primary_clip = document
        .clip(primary)
        .ok_or_else(|| format!("Clip {primary} no longer exists"))?;
    let primary_fps = match &primary_clip.content {
        ClipContent::Media => {
            let asset = document
                .asset(primary_clip.asset)
                .ok_or_else(|| format!("Asset {} no longer exists", primary_clip.asset))?;
            kinewright_core::clip_effective_fps(asset.fps, primary_clip)
                .map_err(|error| error.to_string())?
        }
        ClipContent::Title(_) | ClipContent::Freeze(_) => document.fps,
    };
    let (old_boundary, new_boundary) = match edge {
        TrimEdge::Left => (primary_clip.source_range.start, new_source.start),
        TrimEdge::Right => (primary_clip.source_range.end, new_source.end),
    };
    let project_delta =
        source_boundary_project_delta(old_boundary, new_boundary, primary_fps, document.fps)?;
    let mut operations = Vec::new();
    for (_, clip) in linked_members(document, primary) {
        let (source_fps, maximum_end) = match &clip.content {
            ClipContent::Media => {
                let asset = document
                    .asset(clip.asset)
                    .ok_or_else(|| format!("Asset {} no longer exists", clip.asset))?;
                (
                    kinewright_core::clip_effective_fps(asset.fps, &clip)
                        .map_err(|error| error.to_string())?,
                    asset.duration.0,
                )
            }
            ClipContent::Title(_) | ClipContent::Freeze(_) => (document.fps, i64::MAX),
        };
        let linked_source = if clip.id == primary {
            new_source.clone()
        } else {
            let source_delta = project_delta_to_source(project_delta, document.fps, source_fps);
            match edge {
                TrimEdge::Left => {
                    let start = TimeCode(
                        clip.source_range
                            .start
                            .0
                            .saturating_add(source_delta)
                            .clamp(0, clip.source_range.end.0.saturating_sub(1)),
                    );
                    start..clip.source_range.end
                }
                TrimEdge::Right => {
                    let end = TimeCode(
                        clip.source_range
                            .end
                            .0
                            .saturating_add(source_delta)
                            .clamp(clip.source_range.start.0.saturating_add(1), maximum_end),
                    );
                    clip.source_range.start..end
                }
            }
        };
        if linked_source != clip.source_range {
            operations.push(Operation::TrimClip {
                clip: clip.id,
                new_source: linked_source,
            });
        }
    }
    Ok(operations)
}

fn freeze_frame_operations(document: &Document, at: TimeCode) -> Result<Vec<Operation>, String> {
    let source = timeline_source_at(document, at)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "A media clip is required under the playhead".to_owned())?;
    let clip = document
        .clip(source.clip)
        .ok_or_else(|| format!("Clip {} no longer exists", source.clip))?;
    let duration = TimeCode(i64::from(nominal_fps(document.fps)).saturating_mul(2));
    let mut operations = Vec::with_capacity(3);
    if at > clip.timeline_start && at < source.timeline_end {
        operations.push(Operation::SplitClip {
            clip: source.clip,
            at,
        });
    }
    operations.extend([
        Operation::RippleInsertGap {
            track: source.track,
            at,
            duration,
        },
        Operation::AddFreezeFrame {
            track: source.track,
            at,
            duration,
            asset: source.asset,
            source_frame: source.source_at,
        },
    ]);
    Ok(operations)
}

fn source_boundary_project_delta(
    old: TimeCode,
    new: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
) -> Result<i64, String> {
    match new.cmp(&old) {
        std::cmp::Ordering::Greater => {
            map_source_range_to_project(old..new, source_fps, project_fps)
                .map(|delta| delta.0)
                .map_err(|error| error.to_string())
        }
        std::cmp::Ordering::Less => map_source_range_to_project(new..old, source_fps, project_fps)
            .map(|delta| delta.0.saturating_neg())
            .map_err(|error| error.to_string()),
        std::cmp::Ordering::Equal => Ok(0),
    }
}

fn linked_delete_operations(document: &Document, primary: ClipId, ripple: bool) -> Vec<Operation> {
    let mut members = linked_members(document, primary);
    if members.is_empty() {
        return Vec::new();
    }
    members.sort_by(|(left_track, left), (right_track, right)| {
        left_track
            .cmp(right_track)
            .then_with(|| right.timeline_start.cmp(&left.timeline_start))
    });
    if !ripple {
        return members
            .into_iter()
            .map(|(_, clip)| Operation::DeleteClip { clip: clip.id })
            .collect();
    }

    let mut operations = members
        .into_iter()
        .filter(|(_, clip)| clip.id != primary)
        .map(|(_, clip)| Operation::DeleteClip { clip: clip.id })
        .collect::<Vec<_>>();
    operations.push(Operation::RippleDeleteClip { clip: primary });
    operations
}

// Speed percentages are small integers; the f64 division is display-only.
#[allow(clippy::cast_precision_loss)]
fn paint_speed_badge(painter: &egui::Painter, rect: egui::Rect, speed_percent: u32) {
    if rect.width() < 64.0 {
        return;
    }
    // Below the label strip: the strip's right side belongs to the duration
    // timecode, and the badge must never collide with it.
    painter.text(
        egui::pos2(rect.right() - space::ONE, rect.top() + 21.0),
        egui::Align2::RIGHT_TOP,
        format!("{:.2}x", f64::from(speed_percent) / 100.0),
        egui::FontId::new(type_size::MICRO, egui::FontFamily::Monospace),
        color::TEXT_PRIMARY,
    );
}

/// Build the operations for a speed change. When the clip grows and a later
/// clip on its track would collide, a ripple gap opens first so the speed
/// change lands in cleared space; sync-locked tracks and markers shift with
/// the gap, preserving relative layout. Shrinking leaves a gap by design.
pub(super) fn clip_speed_operations(
    document: &Document,
    clip_id: ClipId,
    speed_percent: u32,
) -> Result<Vec<Operation>, String> {
    let (track, clip) = document
        .tracks
        .iter()
        .find_map(|track| {
            track
                .clips
                .iter()
                .find(|clip| clip.id == clip_id)
                .map(|clip| (track, clip))
        })
        .ok_or_else(|| format!("Clip {clip_id} no longer exists"))?;
    if !clip.content.is_media() {
        return Err("Only media clips have a playback speed".to_owned());
    }
    let current_duration = document
        .clip_duration(clip)
        .map_err(|error| error.to_string())?;
    let mut resped = clip.clone();
    resped.speed_percent = speed_percent;
    let new_duration = document
        .clip_duration(&resped)
        .map_err(|error| error.to_string())?;
    let current_end = clip
        .timeline_start
        .checked_add(current_duration)
        .ok_or_else(|| "Clip end overflowed".to_owned())?;
    let new_end = clip
        .timeline_start
        .checked_add(new_duration)
        .ok_or_else(|| "Clip end overflowed".to_owned())?;

    let mut operations = Vec::new();
    if new_end > current_end {
        let collides = track.clips.iter().any(|other| {
            other.id != clip_id
                && other.timeline_start >= current_end
                && other.timeline_start < new_end
        });
        if collides {
            let delta = new_end
                .checked_sub(current_end)
                .ok_or_else(|| "Speed delta overflowed".to_owned())?;
            operations.push(Operation::RippleInsertGap {
                track: track.id,
                at: current_end,
                duration: delta,
            });
        }
    }
    // Linked members share source geometry, so they take the same speed in
    // the same batch - otherwise the pair desynchronizes structurally. The
    // gap decision above covers them: sync-locked tracks shift together.
    for (_, member) in linked_members(document, clip_id) {
        if member.content.is_media() {
            operations.push(Operation::SetClipSpeed {
                clip: member.id,
                speed_percent,
            });
        }
    }
    Ok(operations)
}

pub(super) fn linked_transition_operations(
    document: &Document,
    primary: ClipId,
    transition: Option<&Transition>,
) -> Vec<Operation> {
    let mut operations = Vec::new();
    for (_, clip) in linked_members(document, primary) {
        if clip.transition_in.is_some() {
            operations.push(Operation::RemoveTransition { clip: clip.id });
        }
        if let Some(transition) = transition {
            operations.push(Operation::AddTransition {
                clip: clip.id,
                transition: transition.clone(),
            });
        }
    }
    operations
}

fn next_marker_id(document: &Document) -> Option<MarkerId> {
    document
        .markers
        .iter()
        .map(|marker| marker.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(MarkerId)
}

fn collect_clip_bounds(document: &kinewright_core::Document) -> Vec<ClipBounds> {
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter_map(|clip| {
            let duration = document.clip_duration(clip).ok()?;
            Some(ClipBounds {
                id: clip.id,
                start: clip.timeline_start.0,
                end: clip.timeline_start.0.saturating_add(duration.0),
            })
        })
        .collect()
}

fn snap_candidates(
    bounds: &[ClipBounds],
    markers: &[Marker],
    exclude: ClipId,
    playhead: i64,
) -> Vec<i64> {
    let mut candidates = vec![playhead];
    candidates.extend(
        markers
            .iter()
            .filter(|marker| !is_internal_marker(marker))
            .map(|marker| marker.position.0),
    );
    candidates.extend(
        bounds
            .iter()
            .filter(|bounds| bounds.id != exclude)
            .flat_map(|bounds| [bounds.start, bounds.end]),
    );
    candidates
}

fn marker_snap_candidates(
    bounds: &[ClipBounds],
    markers: &[Marker],
    exclude: MarkerId,
    playhead: i64,
) -> Vec<i64> {
    let mut candidates = vec![playhead];
    candidates.extend(bounds.iter().flat_map(|bounds| [bounds.start, bounds.end]));
    candidates.extend(
        markers
            .iter()
            .filter(|marker| marker.id != exclude && !is_internal_marker(marker))
            .map(|marker| marker.position.0),
    );
    candidates
}

fn snap_move(
    raw_start: i64,
    duration: i64,
    candidates: &[i64],
    ruler_interval: i64,
    pixels_per_frame: f32,
) -> (i64, Option<i64>) {
    let (start_snap, start_guide) =
        nearest_snap(raw_start, candidates, ruler_interval, pixels_per_frame);
    let raw_end = raw_start.saturating_add(duration);
    let (end_snap, end_guide) = nearest_snap(raw_end, candidates, ruler_interval, pixels_per_frame);
    let start_distance = start_snap.saturating_sub(raw_start).saturating_abs();
    let end_distance = end_snap.saturating_sub(raw_end).saturating_abs();
    if end_guide.is_some() && (start_guide.is_none() || end_distance < start_distance) {
        (end_snap.saturating_sub(duration).max(0), end_guide)
    } else {
        (start_snap.max(0), start_guide)
    }
}

// Snapping rounds between f32 pixel tolerance, f64 ratios, and exact integer frames.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn nearest_snap(
    raw: i64,
    candidates: &[i64],
    ruler_interval: i64,
    pixels_per_frame: f32,
) -> (i64, Option<i64>) {
    let tolerance = (SNAP_TOLERANCE / pixels_per_frame.max(0.01)).ceil() as i64;
    let ruler = ((raw as f64 / ruler_interval.max(1) as f64).round() as i64)
        .saturating_mul(ruler_interval.max(1));
    let mut best = (ruler, ruler.saturating_sub(raw).saturating_abs());
    for candidate in candidates {
        let distance = candidate.saturating_sub(raw).saturating_abs();
        if distance < best.1 {
            best = (*candidate, distance);
        }
    }
    if best.1 <= tolerance {
        (best.0, Some(best.0))
    } else {
        (raw, None)
    }
}

// Tick selection compares exact frame intervals in egui's f32 pixel coordinate space.
#[allow(clippy::cast_precision_loss)]
fn tick_density(pixels_per_frame: f32, fps: Rational) -> (i64, i64) {
    let nominal = i64::from(nominal_fps(fps));
    let candidates = [
        1_i64,
        2,
        5,
        10,
        nominal / 2,
        nominal,
        nominal * 2,
        nominal * 5,
        nominal * 10,
        nominal * 30,
        nominal * 60,
        nominal * 300,
        nominal * 600,
    ];
    let major = candidates
        .into_iter()
        .filter(|candidate| *candidate > 0)
        .find(|candidate| *candidate as f32 * pixels_per_frame >= 72.0)
        .unwrap_or(nominal.saturating_mul(600).max(1));
    let minor = [major / 10, major / 5, major / 2, major]
        .into_iter()
        .filter(|candidate| *candidate > 0)
        .find(|candidate| *candidate as f32 * pixels_per_frame >= 8.0)
        .unwrap_or(major)
        .max(1);
    (major.max(1), minor)
}

fn nominal_fps(fps: Rational) -> u32 {
    fps.numerator().saturating_add(fps.denominator() / 2) / fps.denominator().max(1)
}

pub(crate) fn format_timecode(frame: TimeCode, fps: Rational) -> String {
    let nominal = i64::from(nominal_fps(fps).max(1));
    let frame = frame.0.max(0);
    let frames = frame % nominal;
    let seconds_total = frame / nominal;
    let seconds = seconds_total % 60;
    let minutes_total = seconds_total / 60;
    let minutes = minutes_total % 60;
    let hours = minutes_total / 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}:{frames:02}")
}

fn project_delta_to_source(project_delta: i64, project_fps: Rational, source_fps: Rational) -> i64 {
    let sign = project_delta.signum();
    let magnitude = TimeCode(project_delta.saturating_abs());
    map_frames_with_rounding(magnitude, project_fps, source_fps, FrameRounding::Nearest)
        .map_or(0, |frames| frames.0.saturating_mul(sign))
}

// ---------------------------------------------------------------------------
// AU5 §6.4: the `Room tone` button
// ---------------------------------------------------------------------------

/// AU5 §6.4 rule 129: the toolbar button that fills a gap with room tone.
pub(crate) const ROOM_TONE_BUTTON: &str = "Room tone";
/// Where a refused fill lands in the error log (AU5 §0 R134).
///
/// Not `LOOK_ERROR_CATEGORY`: the button is a timeline gesture, and a
/// `Room tone` refusal filed under "Look" tells the editor to go and look at
/// a surface that had nothing to do with it.
pub(crate) const ROOM_TONE_ERROR_CATEGORY: &str = "Timeline";
/// AU5 §6.4 rule 129: why the button is grey with nothing selected.
pub(crate) const ROOM_TONE_NEEDS_CLIP: &str = "Select a clip on the track whose gap to fill";
/// AU5 §6.4 rule 129: why the button is grey on a track with no hole in it.
pub(crate) const ROOM_TONE_NEEDS_GAP: &str = "That track has no gap to fill";
/// AU5 §5.3 rule 95: the most clips one fill may lay.
///
/// The agent's `plan_room_tone_fill` holds the same ceiling; a gap that needs
/// more than 64 tiles of one capture is a hole, not a seam.
pub(crate) const ROOM_TONE_MAX_TILES: usize = 64;
/// What the button says while its first capture is decoding.
pub(crate) const ROOM_TONE_CAPTURING: &str = "Capturing room tone\u{2026}";

/// AU5 §6.4 rule 129: the gap this click fills.
///
/// The nearest gap on the selected clip's own track, measured from the
/// playhead: distance zero while the playhead is inside a gap, and the
/// distance to the nearer edge otherwise. Leading and interior gaps only,
/// which is what [`Document::track_gaps`] answers; a track that ends before
/// the project does has no hole in it.
pub(crate) fn room_tone_target(
    document: &Document,
    clip: ClipId,
    playhead: TimeCode,
) -> Option<(TrackId, std::ops::Range<TimeCode>)> {
    let track = document
        .tracks
        .iter()
        .find(|track| track.clips.iter().any(|candidate| candidate.id == clip))?;
    let gaps = document.track_gaps(track.id)?;
    let nearest = gaps.into_iter().min_by_key(|gap| {
        if playhead < gap.start {
            gap.start.0.saturating_sub(playhead.0)
        } else if playhead >= gap.end {
            playhead.0.saturating_sub(gap.end.0)
        } else {
            0
        }
    })?;
    Some((track.id, nearest))
}

/// The room-tone asset already in this project's pool, if there is one.
///
/// The store is content-addressed under `<stem>.kinewright-assets/room-tone`,
/// so "is this room tone?" is a question about where the file lives rather
/// than about a document flag: AU5 §5.1 rule 89 keeps room tone an **ordinary**
/// `MediaAsset` and adds no new document type to ask.
pub(crate) fn existing_room_tone_asset<'a>(
    document: &'a Document,
    room_tone_dir: &std::path::Path,
) -> Option<&'a MediaAsset> {
    document
        .media_pool
        .iter()
        .find(|asset| asset.path.parent() == Some(room_tone_dir))
}

/// Why one gap is refused, in the words `plan_room_tone_fill` uses (AU5 §0
/// R137).
///
/// All three of core's mapping refusals are matched **by name**, in the
/// agent's own words, so the button and `plan_room_tone_fill` answer one
/// sentence each. They are three different failures and the editor is told
/// which: an exact-in-frames range usually *does* exist at 30 → 25, so
/// "no exact source range" over a coverage failure would have them conclude
/// the rate pair is unusable, and `SourceTooShortToCover` is the one whose
/// answer really is *record more room tone*. A `_ =>` wildcard over the three
/// would put the first sentence on all of them, which is the misdirection
/// AU5 §0 R137 exists to end.
fn room_tone_skip_reason(
    length: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
    error: &kinewright_core::TimeMappingError,
) -> String {
    let rates = format!("{} / {}", rate_text(source_fps), rate_text(project_fps));
    match error {
        kinewright_core::TimeMappingError::InexactDuration { .. } => format!(
            "gap of {} project frames has no exact source range at {rates}",
            length.0
        ),
        kinewright_core::TimeMappingError::NoCoveringSourceRange {
            source_duration, ..
        } => format!(
            "gap of {} project frames has no exact source range at {rates} that also carries its \
             sample frames within the {source_duration}-source-frame room-tone asset",
            length.0
        ),
        kinewright_core::TimeMappingError::SourceTooShortToCover {
            minimum_source_frames,
            source_duration,
            ..
        } => format!(
            "gap of {} project frames needs at least {minimum_source_frames} source frames of room \
             tone at {rates}, and this asset carries only {source_duration}",
            length.0
        ),
        other => format!(
            "gap of {} project frames has no usable source range at {rates}: {other}",
            length.0
        ),
    }
}

/// AU5 §5.3: the `AddClip`s that fill one gap from one room-tone asset.
///
/// Butt-joined, no fade, tiling forward from `gap.start`, capped at
/// [`ROOM_TONE_MAX_TILES`]. Every tile is written at the default
/// `speed_percent` — `AddClip` carries no speed field — which is what makes
/// each tile's mapped project span exactly the span it was asked for
/// (AU5 §0 R44).
///
/// **Each tile's source range comes from
/// core's `covering_source_range_for_project_duration`, not from
/// `map_project_duration_to_source(TimeCode::ZERO, …)`** (AU5 §0 R135). The
/// latter answers a range that is exact in project *frames*, which is not the
/// same as one that carries enough *samples*: the mixer maps source samples to
/// project samples one for one, so at 30 → 25 a phase-0 seven-frame fill
/// (`0..8`) is 640 sample frames short and the join dips to silence. The core
/// helper sweeps the source phase and returns the first range that both maps
/// exactly and supplies the gap's sample frames, which at 25 fps is `1..10`
/// rather than `0..8`. Tiles may therefore start at a **non-zero** source
/// frame and differ from one another by a frame in length; what is fixed is
/// the project span each maps to.
///
/// **Core is asked once per pass, with what is left of the gap** (AU5 §0
/// R141). Its answer is the largest coverable span **at most** that long, so a
/// remainder it cannot cover is simply split again rather than refusing the
/// whole gap: coverability is not monotone in `want`, and "the remainder is
/// uncoverable while `remainder − 1` and `1` both are" is common rather than
/// exotic. Wherever a single hoisted tile would have worked, this emits the
/// identical tiles — with `remaining ≥ whole` core returns the same `whole`
/// every pass — so R137's "chosen once, not per tile" survives in effect while
/// the pathological remainders stop being fatal. It is also exactly
/// `plan_room_tone_fill`'s loop, which is what makes R140's "one rule" true.
///
/// # Errors
///
/// Returns the gap's own reason when no source range covers it at these two
/// rates, or when the gap needs more than the tile ceiling.
pub(crate) fn room_tone_fill_operations(
    document: &Document,
    track: TrackId,
    gap: &std::ops::Range<TimeCode>,
    asset: &MediaAsset,
) -> Result<Vec<Operation>, String> {
    let project_fps = document.fps;
    let length = TimeCode(gap.end.0.saturating_sub(gap.start.0));
    let mut operations = Vec::new();
    let mut cursor = gap.start;
    while cursor < gap.end {
        if operations.len() >= ROOM_TONE_MAX_TILES {
            return Err(format!(
                "gap of {} project frames needs more than {ROOM_TONE_MAX_TILES} tiles of this \
                 room tone",
                length.0
            ));
        }
        let remaining = TimeCode(gap.end.0 - cursor.0);
        let placed_so_far = operations.len();
        let (want, source) = kinewright_core::longest_coverable_project_tile(
            asset.duration,
            asset.fps,
            project_fps,
            kinewright_media::ROOM_TONE_SAMPLE_RATE,
            remaining,
        )
        .map_err(|error| {
            // The headline names the GAP, which is what an editor can see,
            // while core's refusal is about whatever is left of it. When those
            // differ — the tiler covered part of the gap and then ran out of
            // representable spans — the residue is named too, so a reader is
            // not told "gap of 1 project frames" about a seven-frame hole.
            // Word for word `plan_room_tone_fill`'s annotation.
            let reason = room_tone_skip_reason(length, asset.fps, project_fps, &error);
            if remaining == length {
                reason
            } else {
                format!(
                    "{reason}; {placed_so_far} tile(s) covered all but its last {} project \
                     frame(s)",
                    remaining.0
                )
            }
        })?;
        operations.push(Operation::AddClip {
            track,
            asset: asset.id,
            at: cursor,
            source,
        });
        cursor = TimeCode(cursor.0 + want.0);
    }
    if cursor != gap.end {
        return Err(format!(
            "the fill would end at {} rather than at the gap's own {}",
            cursor.0, gap.end.0
        ));
    }
    Ok(operations)
}

/// One frame rate as `30/1`, for a per-gap refusal an editor can act on.
fn rate_text(fps: kinewright_core::Rational) -> String {
    format!("{}/{}", fps.numerator(), fps.denominator())
}

impl KinewrightApp {
    /// AU5 §6.4 rule 129: why the `Room tone` button is grey, if it is.
    pub(crate) fn room_tone_block(&self) -> Option<&'static str> {
        let session = self.focused();
        let Some(clip) = session.selected_clip else {
            return Some(ROOM_TONE_NEEDS_CLIP);
        };
        if room_tone_target(&session.document, clip, session.position).is_none() {
            return Some(ROOM_TONE_NEEDS_GAP);
        }
        None
    }

    /// AU5 §6.4 rule 129: fill the gap nearest the playhead on the selected
    /// clip's track.
    ///
    /// One `DoBatch` and therefore one undo entry, either way: with a
    /// room-tone asset already in the pool the batch is the `AddClip`s alone
    /// and goes out on this frame; without one, the capture runs on a worker
    /// — DESIGN.md's performance contract keeps decode off the UI thread —
    /// and the same single batch, `AddAsset` first, goes out when it lands.
    pub(crate) fn fill_room_tone_at_selection(&mut self) {
        let mut edits = InspectorEdits::default();
        // A refused fill is a timeline refusal, not a look one (AU5 §0 R134).
        edits.set_error_category(ROOM_TONE_ERROR_CATEGORY);
        let capture = self.plan_room_tone_fill(&mut edits);
        // One submit, at the end, exactly as `timeline()`'s own is: every
        // refusal above reached `edits.errors` rather than the log directly.
        self.submit_inspector_edits(edits);
        if let Some(job) = capture {
            self.spawn_room_tone_capture(job);
        }
    }

    /// Fill now if the pool already holds room tone; otherwise name the
    /// capture that has to run first.
    ///
    /// Every refusal is an `InspectorEdits` error and never a live write:
    /// a fill is a discrete action, so the timeline's one coalescing path
    /// stays the envelope's (AU4 §5.1 rule 105).
    fn plan_room_tone_fill(&self, edits: &mut InspectorEdits) -> Option<RoomToneCaptureJob> {
        let session = self.focused();
        let Some(clip) = session.selected_clip else {
            edits.push_error(ROOM_TONE_NEEDS_CLIP);
            return None;
        };
        let Some((track, gap)) = room_tone_target(&session.document, clip, session.position) else {
            edits.push_error(ROOM_TONE_NEEDS_GAP);
            return None;
        };
        let store = match self.room_tone_store() {
            Ok(store) => store,
            Err(message) => {
                edits.push_error(message);
                return None;
            }
        };
        let directory = store.room_tone_dir();
        if let Some(asset) = existing_room_tone_asset(&session.document, &directory) {
            match room_tone_fill_operations(&session.document, track, &gap, asset) {
                Ok(operations) => edits.extend_operations(operations),
                Err(reason) => edits.push_error(reason),
            }
            return None;
        }
        // Nothing captured yet: take the track's own longest silence, which
        // is the only range in the project known to be room tone rather than
        // material.
        let Some((asset, source)) = self.longest_silence_on_track(track) else {
            edits.push_error(ROOM_TONE_CAPTURE_NEEDS_SILENCE);
            return None;
        };
        Some(RoomToneCaptureJob {
            track,
            gap,
            asset,
            source,
            store,
        })
    }

    /// The store this project's room tone lives in.
    fn room_tone_store(&self) -> Result<RoomToneStore, String> {
        let session = self.focused();
        let Some(path) = session.project_path.as_ref() else {
            return Err(ROOM_TONE_NEEDS_SAVED_PROJECT.to_owned());
        };
        RoomToneStore::for_project(path)
            .map_err(|error| format!("Could not open the room-tone store: {error}"))
    }

    /// The longest silence on one track, in its own asset's source frames.
    ///
    /// Filtered by the caller on `TimelineSilenceSpan.track`, exactly as the
    /// `Learn` gesture filters it (AU5 §0 R46): the routine has no track
    /// argument.
    fn longest_silence_on_track(
        &self,
        track: TrackId,
    ) -> Option<(MediaAsset, std::ops::Range<TimeCode>)> {
        let document = &self.focused().document;
        let spans = self
            .analysis
            .timeline_silences(document, None, TimeCode(1))
            .ok()?;
        let span = spans
            .into_iter()
            .filter(|span| span.track == track)
            .max_by_key(|span| span.source_end.0 - span.source_start.0)?;
        let asset = document
            .media_pool
            .iter()
            .find(|asset| asset.id == span.asset)?
            .clone();
        Some((asset, span.source_start..span.source_end))
    }

    /// Capture on a worker, then land one batch (AU5 §6.4 rule 129).
    fn spawn_room_tone_capture(&mut self, job: RoomToneCaptureJob) {
        let RoomToneCaptureJob {
            track,
            gap,
            asset,
            source,
            store,
        } = job;
        let session = self.focused();
        let (session_id, revision) = (session.id, session.revision);
        let analysis = Arc::clone(&self.analysis);
        let result_tx = self.room_tone_tx.clone();
        // The capture is capped at the store's own writer cap rather than at
        // the whole silence: 60 s of room tone tiles any gap the ceiling
        // allows, and a longer decode buys nothing.
        let capped = capped_capture_range(&source, asset.fps);
        let spawned = std::thread::Builder::new()
            .name("kinewright-room-tone".to_owned())
            .spawn(move || {
                let cancellation = kinewright_core::ExportCancellation::default();
                let result = store
                    .capture_room_tone(&asset, capped, &cancellation)
                    .map_err(|error| format!("Could not capture room tone: {error}"))
                    .and_then(|capture| {
                        analysis
                            .probe(&capture.path)
                            .map_err(|error| {
                                format!("Could not probe the captured room tone: {error}")
                            })
                            .map(|probed| MediaAsset {
                                // `probe_path` names a store file by its own
                                // digest; the capture names it after what it
                                // was taken from (AU5 §5.8 rule 115, R46).
                                name: format!("Room tone \u{2014} {}", asset.name),
                                ..probed
                            })
                    });
                let _ = result_tx.send(RoomToneCaptureResponse {
                    session_id,
                    revision,
                    track,
                    gap,
                    result,
                });
            })
            .is_ok();
        if spawned {
            self.room_tone_pending = self.room_tone_pending.saturating_add(1);
            ROOM_TONE_CAPTURING.clone_into(&mut self.status);
        } else {
            self.record_error("Media", "Could not start the room-tone capture worker");
        }
    }

    /// Drain the capture worker; called from `poll_background`.
    pub(crate) fn poll_room_tone(&mut self, ctx: &egui::Context) {
        while let Ok(response) = self.room_tone_rx.try_recv() {
            self.room_tone_pending = self.room_tone_pending.saturating_sub(1);
            let mut edits = InspectorEdits::default();
            edits.set_error_category(ROOM_TONE_ERROR_CATEGORY);
            match self.room_tone_batch(&response) {
                Ok(operations) => edits.extend_operations(operations),
                Err(message) => edits.push_error(message),
            }
            self.submit_inspector_edits(edits);
        }
        if self.room_tone_pending > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The one batch a completed capture lands: `[AddAsset, AddClip…]`.
    fn room_tone_batch(
        &self,
        response: &RoomToneCaptureResponse,
    ) -> Result<Vec<Operation>, String> {
        let asset = response.result.as_ref().map_err(Clone::clone)?;
        let session = self.focused();
        if session.id != response.session_id || session.revision != response.revision {
            // Landing it against a document that has moved would fill a gap
            // that is no longer there. The retry is not free: this refusal
            // lands no `AddAsset`, so `existing_room_tone_asset` still finds
            // nothing and the next press decodes again — the store's content
            // addressing de-duplicates the *write*, never the decode
            // (AU5 §0 R129).
            return Err(ROOM_TONE_DOCUMENT_MOVED.to_owned());
        }
        let mut operations = vec![Operation::AddAsset {
            asset: asset.clone(),
        }];
        operations.extend(room_tone_fill_operations(
            &session.document,
            response.track,
            &response.gap,
            asset,
        )?);
        Ok(operations)
    }
}

/// AU5 §6.4: one capture the button decided has to run before it can fill.
///
/// A named struct rather than a five-tuple, so the fill's decision and the
/// worker that carries it out cannot drift on argument order.
pub(crate) struct RoomToneCaptureJob {
    track: TrackId,
    gap: std::ops::Range<TimeCode>,
    /// The asset the room tone is taken **from**, raw and pre-chain (AU5
    /// §5.1 rule 91).
    asset: MediaAsset,
    /// The silence to capture, in that asset's own source frames.
    source: std::ops::Range<TimeCode>,
    store: RoomToneStore,
}

/// AU5 §6.4: what one finished capture hands back to the app.
pub(crate) struct RoomToneCaptureResponse {
    pub(crate) session_id: u64,
    /// The revision the fill was derived against; a moved document is refused
    /// rather than filled at the wrong frames.
    pub(crate) revision: kinewright_core::TimelineRevision,
    pub(crate) track: TrackId,
    pub(crate) gap: std::ops::Range<TimeCode>,
    pub(crate) result: Result<MediaAsset, String>,
}

/// AU5 §6.4: why a capture cannot start.
pub(crate) const ROOM_TONE_CAPTURE_NEEDS_SILENCE: &str =
    "No silence on that track to capture room tone from yet";
/// AU5 §5.1: the store is rooted at the project file, so there has to be one.
pub(crate) const ROOM_TONE_NEEDS_SAVED_PROJECT: &str =
    "Save the project before capturing room tone";
/// AU5 §6.4: a capture that finished against a document that has since moved.
pub(crate) const ROOM_TONE_DOCUMENT_MOVED: &str =
    "The timeline changed while the room tone was capturing; press Room tone again";

/// Trim one silence to the store's writer cap (AU5 §5.1 rule 90).
///
/// The cap guards the **writer**: `write_capture` refuses a buffer over
/// `ROOM_TONE_MAX_CAPTURE_MILLISECONDS`, so a two-minute silence is trimmed
/// here rather than decoded and then refused.
pub(crate) fn capped_capture_range(
    source: &std::ops::Range<TimeCode>,
    fps: Rational,
) -> std::ops::Range<TimeCode> {
    let end = source
        .end
        .0
        .min(source.start.0.saturating_add(longest_capture_frames(fps)));
    source.start..TimeCode(end)
}

/// The longest capture the store will write, in this asset's source frames.
pub(crate) fn longest_capture_frames(fps: Rational) -> i64 {
    i64::try_from(
        kinewright_media::ROOM_TONE_MAX_CAPTURE_MILLISECONDS
            .saturating_mul(u64::from(fps.numerator()))
            / (1_000 * u64::from(fps.denominator()).max(1)),
    )
    .unwrap_or(i64::MAX)
    .max(1)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{
        AssetId, LinkId, MediaAsset, Track, covering_source_range_for_project_duration,
        map_project_duration_to_source,
    };

    use super::*;

    /// AU1 §7 item 24: the header paints its mix toggles, and a frame of
    /// painting writes nothing.
    #[test]
    fn track_labels_paint_the_mix_toggles_and_write_nothing() {
        let ctx = egui::Context::default();
        // The header asks for the app's own font families, so the theme has to
        // be installed before a label can be laid out.
        crate::theme::install(&ctx);
        let document = linked_fixture();
        let mut operation = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            operation = paint_track_labels(ui, &document, 200.0, size::TRACK_HEIGHT);
        });
        assert!(
            operation.is_none(),
            "painting the track headers writes no operation"
        );
        let painted = crate::theme::painted_text(&output);
        for expected in ["TRACKS", "V1", "A2", "M", "S"] {
            assert!(
                painted.iter().any(|text| text == expected),
                "the track header paints {expected}; it painted {painted:?}"
            );
        }
    }

    /// AU1 §5.2: the mix column sits inside the widened label strip, clear of
    /// both the caption column and the sync-lock button.
    #[test]
    fn the_mix_toggle_column_fits_between_the_caption_and_the_sync_lock() {
        assert!((TRACK_LABEL_WIDTH - 96.0).abs() < f32::EPSILON);
        // Fixed numbers, not the formula again: the point of the test is that
        // the code the header runs still lands on §5.2's x = 40..54.
        let lane = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(TRACK_LABEL_WIDTH, size::TRACK_HEIGHT),
        );
        let column_left = mix_toggle_column_x(lane);
        assert!(
            (column_left - 40.0).abs() < f32::EPSILON,
            "the mix column starts at x = 40; it starts at {column_left}"
        );
        assert!(
            (column_left + size::ICON_SM - 54.0).abs() < f32::EPSILON,
            "the mix column ends at x = 54; it ends at {}",
            column_left + size::ICON_SM
        );
        assert!(
            column_left + size::ICON_SM <= 62.0,
            "the toggles must not overlap the sync-lock button, which starts at x = 62"
        );
        assert!(
            column_left >= space::TWO + size::ICON_SM,
            "the caption and kind icon keep their column"
        );
    }

    fn linked_fixture() -> Document {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("linked.mp4"),
            name: "linked.mp4".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let clip = |id, track_start| Clip {
            id: ClipId(id),
            asset: asset.id,
            source_range: TimeCode(0)..TimeCode(30),
            content: kinewright_core::ClipContent::Media,
            timeline_start: TimeCode(track_start),
            effects: Vec::new(),
            transition_in: None,
            link: Some(LinkId(7)),
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
            audio_gain_curve: None,
        };
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![clip(1, 0)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![clip(2, 0)],
                },
            ],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps,
            resolution: (1_920, 1_080),
            lut_assets: Vec::new(),
            duration: TimeCode(30),
        }
    }

    #[test]
    // This test checks the same intentional frame-to-pixel projection as tick_density.
    #[allow(clippy::cast_precision_loss)]
    fn ruler_density_keeps_labels_and_minor_ticks_readable() {
        let fps = Rational::new(30, 1).unwrap();
        for zoom in [0.25, 1.0, 6.0, 20.0] {
            let (major, minor) = tick_density(zoom, fps);
            assert!(major as f32 * zoom >= 72.0);
            assert!(minor as f32 * zoom >= 8.0);
            assert!(major >= minor);
        }
    }

    #[test]
    fn timecode_uses_hour_minute_second_frame_fields() {
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(format_timecode(TimeCode(0), fps), "00:00:00:00");
        assert_eq!(format_timecode(TimeCode(108_029), fps), "01:00:00:29");
    }

    #[test]
    fn freeze_at_playhead_builds_one_split_gap_add_batch_and_one_undo_reverts_it() {
        use kinewright_core::{Command, Core, Event};

        let mut document = linked_fixture();
        document.tracks.truncate(1);
        let operations = freeze_frame_operations(&document, TimeCode(10)).unwrap();
        assert_eq!(
            operations,
            vec![
                Operation::SplitClip {
                    clip: ClipId(1),
                    at: TimeCode(10),
                },
                Operation::RippleInsertGap {
                    track: TrackId(1),
                    at: TimeCode(10),
                    duration: TimeCode(60),
                },
                Operation::AddFreezeFrame {
                    track: TrackId(1),
                    at: TimeCode(10),
                    duration: TimeCode(60),
                    asset: AssetId(1),
                    source_frame: TimeCode(10),
                },
            ]
        );

        let core = Core::spawn(document.clone()).unwrap();
        core.request(Command::DoBatch(operations)).unwrap();
        let undone = match core.request(Command::Undo).unwrap() {
            Event::DocumentChanged { doc, .. } => doc,
            other => panic!("unexpected event: {other:?}"),
        };
        assert_eq!(undone.as_ref(), &document);
    }

    #[test]
    fn freeze_at_clip_start_skips_split_and_requires_media_under_playhead() {
        let document = linked_fixture();
        assert_eq!(
            freeze_frame_operations(&document, TimeCode::ZERO).unwrap(),
            vec![
                Operation::RippleInsertGap {
                    track: TrackId(1),
                    at: TimeCode::ZERO,
                    duration: TimeCode(60),
                },
                Operation::AddFreezeFrame {
                    track: TrackId(1),
                    at: TimeCode::ZERO,
                    duration: TimeCode(60),
                    asset: AssetId(1),
                    source_frame: TimeCode::ZERO,
                },
            ]
        );
        assert_eq!(
            freeze_frame_operations(&document, TimeCode(30)).unwrap_err(),
            "A media clip is required under the playhead"
        );
    }

    #[test]
    fn snapping_prefers_closest_clip_edge_and_respects_screen_tolerance() {
        let candidates = [100, 240];
        assert_eq!(nearest_snap(97, &candidates, 30, 2.0), (100, Some(100)));
        assert_eq!(nearest_snap(80, &candidates, 30, 2.0), (80, None));
        assert_eq!(snap_move(191, 50, &candidates, 30, 2.0), (190, Some(240)));
    }

    #[test]
    fn snapping_candidates_include_markers_and_other_tracks() {
        let bounds = [
            ClipBounds {
                id: ClipId(1),
                start: 10,
                end: 40,
            },
            ClipBounds {
                id: ClipId(2),
                start: 60,
                end: 90,
            },
        ];
        let markers = [
            Marker {
                id: MarkerId(3),
                position: TimeCode(50),
                label: "Review".to_owned(),
                color_token: 0,
            },
            Marker {
                id: MarkerId(4),
                position: TimeCode(75),
                label: "__kinewright_reframe_subject_v1:sidecar".to_owned(),
                color_token: 0,
            },
        ];
        assert_eq!(
            snap_candidates(&bounds, &markers, ClipId(1), 25),
            vec![25, 50, 60, 90]
        );
    }

    #[test]
    fn reframe_provenance_markers_are_not_editorial_or_snap_targets() {
        let editorial = Marker {
            id: MarkerId(3),
            position: TimeCode(50),
            label: "Review".to_owned(),
            color_token: 0,
        };
        let internal = Marker {
            id: MarkerId(4),
            position: TimeCode(75),
            label: "__kinewright_reframe_subject_v1:sidecar".to_owned(),
            color_token: 0,
        };
        assert!(!is_internal_marker(&editorial));
        assert!(is_internal_marker(&internal));

        let markers = [editorial, internal];
        let bounds = [ClipBounds {
            id: ClipId(1),
            start: 10,
            end: 40,
        }];
        assert_eq!(
            marker_snap_candidates(&bounds, &markers, MarkerId(3), 25),
            vec![25, 10, 40]
        );
    }

    #[test]
    fn linked_move_trim_and_delete_expand_to_atomic_batch_members() {
        let document = linked_fixture();
        assert_eq!(
            linked_move_operations(&document, ClipId(1), TrackId(1), TimeCode(10)),
            vec![
                Operation::MoveClip {
                    clip: ClipId(1),
                    to_track: TrackId(1),
                    to: TimeCode(10),
                },
                Operation::MoveClip {
                    clip: ClipId(2),
                    to_track: TrackId(2),
                    to: TimeCode(10),
                },
            ]
        );
        assert_eq!(
            linked_trim_operations(
                &document,
                ClipId(1),
                TimeCode(5)..TimeCode(30),
                TrimEdge::Left,
            )
            .unwrap(),
            vec![
                Operation::TrimClip {
                    clip: ClipId(1),
                    new_source: TimeCode(5)..TimeCode(30),
                },
                Operation::TrimClip {
                    clip: ClipId(2),
                    new_source: TimeCode(5)..TimeCode(30),
                },
            ]
        );
        assert_eq!(
            linked_delete_operations(&document, ClipId(1), true),
            vec![
                Operation::DeleteClip { clip: ClipId(2) },
                Operation::RippleDeleteClip { clip: ClipId(1) },
            ]
        );
    }

    #[test]
    fn linked_transition_operations_replace_each_member_in_one_batch() {
        let mut document = linked_fixture();
        document.tracks[0].clips[0].transition_in = Some(Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(6),
        });
        let replacement = Transition {
            name: "fade_from_white".to_owned(),
            duration: TimeCode(12),
        };

        assert_eq!(
            linked_transition_operations(&document, ClipId(1), Some(&replacement)),
            vec![
                Operation::RemoveTransition { clip: ClipId(1) },
                Operation::AddTransition {
                    clip: ClipId(1),
                    transition: replacement.clone(),
                },
                Operation::AddTransition {
                    clip: ClipId(2),
                    transition: replacement,
                },
            ]
        );
        assert_eq!(
            linked_transition_operations(&document, ClipId(1), None),
            vec![Operation::RemoveTransition { clip: ClipId(1) }]
        );
    }

    #[test]
    fn clip_speed_operations_open_a_gap_only_when_growth_collides() {
        let fps = Rational::new(30, 1).unwrap();
        let mut document = Document {
            fps,
            color_context: kinewright_core::ColorContext::default(),
            ..Document::default()
        };
        document.media_pool.push(MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from("speed.mp4"),
            name: "speed.mp4".to_owned(),
            duration: TimeCode(300),
            fps,
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        });
        document.tracks.push(Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                Clip {
                    id: ClipId(1),
                    asset: AssetId(1),
                    source_range: TimeCode(0)..TimeCode(30),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                },
                Clip {
                    id: ClipId(2),
                    asset: AssetId(1),
                    source_range: TimeCode(60)..TimeCode(90),
                    content: ClipContent::Media,
                    timeline_start: TimeCode(30),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                },
            ],
        });
        document.duration = TimeCode(60);

        // Slowing clip 1 to half speed doubles it into clip 2: gap first.
        assert_eq!(
            clip_speed_operations(&document, ClipId(1), 50).unwrap(),
            vec![
                Operation::RippleInsertGap {
                    track: TrackId(1),
                    at: TimeCode(30),
                    duration: TimeCode(30),
                },
                Operation::SetClipSpeed {
                    clip: ClipId(1),
                    speed_percent: 50,
                },
            ]
        );
        // Speeding up shrinks and needs no gap.
        assert_eq!(
            clip_speed_operations(&document, ClipId(1), 200).unwrap(),
            vec![Operation::SetClipSpeed {
                clip: ClipId(1),
                speed_percent: 200,
            }]
        );
        // The trailing clip grows into open space: no gap needed either.
        assert_eq!(
            clip_speed_operations(&document, ClipId(2), 50).unwrap(),
            vec![Operation::SetClipSpeed {
                clip: ClipId(2),
                speed_percent: 50,
            }]
        );
    }

    // ---- AU4 Part B §5.1-§5.2: the timeline rubber band ----

    /// A band of the exact geometry §5.1's cases produce, at a clip width that
    /// makes the mapping rect 240 px wide.
    fn envelope_test_band(track_height: f32, kind: MediaKind) -> egui::Rect {
        let lane = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, track_height));
        let clip_rect = egui::Rect::from_min_size(
            egui::pos2(lane.left(), lane.top() + space::ONE),
            // `space::HALF` a side comes off in the shrink, so 244 px of clip
            // is 240 px of band.
            egui::vec2(244.0, lane.height() - space::TWO),
        );
        envelope_band_rect(clip_rect, kind).expect("an audio clip has a band")
    }

    fn envelope_curve(keys: &[(i64, i64)]) -> AutomationCurve {
        AutomationCurve {
            keyframes: keys
                .iter()
                .map(|(at, value)| Keyframe {
                    at: TimeCode(*at),
                    value: *value,
                    interpolation: kinewright_core::KeyframeInterpolation::Linear,
                })
                .collect(),
        }
    }

    /// Every circle and every stroked path one paint pass emitted.
    ///
    /// Shared by the envelope's paint tests: `Shape::Vec` nests, so the walk is
    /// recursive and worth writing once.
    fn collect_envelope_shapes(
        shape: &egui::epaint::Shape,
        circles: &mut Vec<(f32, egui::Color32)>,
        lines: &mut Vec<(Vec<egui::Pos2>, f32, egui::Color32)>,
    ) {
        match shape {
            egui::epaint::Shape::Circle(circle) => circles.push((circle.radius, circle.fill)),
            egui::epaint::Shape::Path(path) => {
                if let egui::epaint::ColorMode::Solid(color) = path.stroke.color {
                    lines.push((path.points.clone(), path.stroke.width, color));
                }
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_envelope_shapes(shape, circles, lines);
                }
            }
            _ => {}
        }
    }

    /// AU4 §7 B1: the four pure mappings, with no window.
    ///
    /// The three band heights are exactly 38.00, 18.88 and 10.00 px, so
    /// 520 / 18.88 = 27.54 and an equality assert on 27.5 would fail; the
    /// resolutions are asserted within ±0.1 and printed.
    #[test]
    fn envelope_mappings_round_trip_and_place_unity_at_23_percent() {
        let band = envelope_test_band(size::TRACK_HEIGHT, MediaKind::Audio);
        assert!(
            (band.width() - 240.0).abs() < f32::EPSILON,
            "the mapping rect is 240 px wide; it is {}",
            band.width()
        );
        let duration = TimeCode(300);
        for frame in 0..duration.0 {
            let at = TimeCode(frame);
            let x = envelope_local_frame_to_x(band, duration, at);
            assert_eq!(
                envelope_x_to_local_frame(band, duration, x),
                at,
                "frame {frame} of a 300-frame clip round-trips through a 240 px rect"
            );
        }

        let unity = envelope_value_to_y(band, 0);
        let fraction = (unity - band.top()) / band.height();
        assert!(
            (fraction - 0.231).abs() <= 0.001,
            "unity sits at (120 - 0) / 520 = 23.1 % from the top; it sits at {fraction}"
        );
        assert_eq!(
            envelope_y_to_value(band, unity),
            0,
            "and maps back to unity"
        );
        assert!(
            (envelope_value_to_y(band, ENVELOPE_DISPLAY_MIN_TENTH_DB - 1) - band.bottom()).abs()
                < f32::EPSILON,
            "-401 clamps to the bottom edge"
        );
        assert!(
            (envelope_value_to_y(band, ENVELOPE_DISPLAY_MAX_TENTH_DB + 1) - band.top()).abs()
                < f32::EPSILON,
            "121 clamps to the top edge"
        );

        for (label, height, expected) in [
            ("a pure-audio clip on a 72 px track", 38.00_f32, 13.7_f32),
            ("an audio+video clip on a 72 px track", 18.88, 27.5),
            ("a pure-audio clip on the 44 px minimum track", 10.00, 52.0),
        ] {
            let case = match (height, label.contains("audio+video")) {
                (_, true) => envelope_test_band(size::TRACK_HEIGHT, MediaKind::AudioVideo),
                (h, false) if (h - 38.00).abs() < 0.01 => {
                    envelope_test_band(size::TRACK_HEIGHT, MediaKind::Audio)
                }
                _ => envelope_test_band(44.0, MediaKind::Audio),
            };
            assert!(
                (case.height() - height).abs() <= 0.01,
                "{label} gives a {height} px band; it gave {}",
                case.height()
            );
            #[allow(clippy::cast_precision_loss)]
            let resolution = ENVELOPE_DISPLAY_SPAN_TENTH_DB as f32 / case.height();
            println!(
                "AU4_ENVELOPE_RESOLUTION case={label} band_px={} tenth_db_per_px={resolution}",
                case.height()
            );
            assert!(
                (resolution - expected).abs() <= 0.1,
                "{label} resolves at {expected} tenth-dB per logical pixel; it resolved at {resolution}"
            );
        }

        assert!(
            envelope_band_rect(
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(240.0, 64.0)),
                MediaKind::Video
            )
            .is_none(),
            "a Video asset has no band, and a Title or Freeze clip reaches this with no kind at all"
        );
    }

    /// AU4 §7 B2: the hit radius, the allocation floor, and the trim handles.
    #[test]
    fn envelope_hit_stays_inside_nine_pixels_and_off_both_trim_handles() {
        let band = envelope_test_band(size::TRACK_HEIGHT, MediaKind::Audio);
        let key = band.center();
        assert_eq!(
            envelope_hit(&[key], band, key + egui::vec2(9.0, 0.0)),
            Some(0),
            "a key is found within 9.0"
        );
        assert_eq!(
            envelope_hit(&[key], band, key + egui::vec2(9.1, 0.0)),
            None,
            "and not at 9.1"
        );
        assert!(envelope_near_curve(
            &[key, key + egui::vec2(80.0, 0.0)],
            band,
            key + egui::vec2(40.0, 8.0)
        ));
        assert!(!envelope_near_curve(
            &[key, key + egui::vec2(80.0, 0.0)],
            band,
            key + egui::vec2(40.0, 10.0)
        ));

        // Rule 97: no hit-testing at all under 24 px of clip width.
        assert!(
            envelope_is_offered(TimeCode(30), 0.8, true),
            "24 px of clip is offered"
        );
        assert!(
            !envelope_is_offered(TimeCode(30), 0.79, true),
            "23.7 px of clip is not"
        );

        let clip_rect = egui::Rect::from_min_size(egui::pos2(100.0, 20.0), egui::vec2(244.0, 64.0));
        let body_rect = egui::Rect::from_min_max(
            egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.top()),
            egui::pos2(clip_rect.right() - EDGE_HANDLE_WIDTH, clip_rect.bottom()),
        );
        let band = envelope_band_rect(clip_rect, MediaKind::Audio).unwrap();
        let interact = envelope_interact_rect(band, body_rect);
        assert!(
            interact.left() >= body_rect.left() && interact.right() <= body_rect.right(),
            "the envelope interact never overlaps either 6 px trim handle in x: {interact:?}"
        );
        // A pointer 3 px from the line, inside the left handle's x band,
        // still reaches the handle.
        let probe = egui::pos2(clip_rect.left() + 2.0, band.center().y + 3.0);
        let left_handle = egui::Rect::from_min_max(
            clip_rect.min,
            egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.bottom()),
        );
        assert!(
            left_handle.contains(probe),
            "the probe is on the left handle"
        );
        assert!(
            !interact.contains(probe),
            "and the envelope interact does not cover it"
        );
    }

    /// AU4 §7 B3: the three gesture rules, pure, over the whole key list.
    #[test]
    fn envelope_gesture_rules_are_pure_over_the_key_list() {
        let curve = envelope_curve(&[(0, 0), (60, -200), (120, 0)]);
        let duration = TimeCode(180);

        // Insert takes the curve's current value at the snapped frame.
        let inserted = envelope_insert_key(&curve, TimeCode(30));
        assert_eq!(inserted.len(), 4);
        assert_eq!(inserted[1].at, TimeCode(30));
        assert_eq!(inserted[1].value, curve.value_at(TimeCode(30)).unwrap());
        assert_eq!(
            envelope_insert_key(&curve, TimeCode(60)),
            curve.keyframes,
            "a frame that already carries a key is left alone"
        );

        // A dragged key is constrained between its neighbours ± 1 frame and
        // clamped in both axes.
        let far = envelope_move_key(&curve.keyframes, 1, TimeCode(500), 999, duration);
        assert_eq!(
            far[1].at,
            TimeCode(119),
            "x stops one frame short of its neighbour"
        );
        assert_eq!(
            far[1].value,
            i64::from(TRACK_MIX_GAIN_MAX),
            "y clamps to +120"
        );
        let low = envelope_move_key(&curve.keyframes, 1, TimeCode(-9), -9_999, duration);
        assert_eq!(
            low[1].at,
            TimeCode(1),
            "and one frame past the one before it"
        );
        assert_eq!(
            low[1].value,
            i64::from(TRACK_MIX_GAIN_MIN),
            "y clamps to -600"
        );
        let last = envelope_move_key(&curve.keyframes, 2, TimeCode(9_999), 0, duration);
        assert_eq!(
            last[2].at,
            TimeCode(179),
            "the last key stops at duration - 1"
        );

        // Remove drops one key; the last one cannot be removed.
        let removed = envelope_remove_key(&curve.keyframes, 1).expect("three keys leave two");
        assert_eq!(removed.len(), 2);
        assert_eq!(removed[1].at, TimeCode(120));
        let only = envelope_curve(&[(0, 0)]);
        assert!(
            envelope_remove_key(&only.keyframes, 0).is_none(),
            "the last key cannot be removed: clearing is the inspector's `Clear`"
        );
    }

    /// AU4 §7 B3 and rules 96/103: a snapped insert lands on a project frame
    /// converted by subtracting `timeline_start`, and Alt bypasses snapping
    /// through the existing frame-level flag.
    #[test]
    fn envelope_snapping_runs_on_project_frames_and_alt_bypasses_it() {
        let band = envelope_test_band(size::TRACK_HEIGHT, MediaKind::Audio);
        let duration = TimeCode(300);
        let clip_start = TimeCode(40);
        // A guide at project frame 100 is clip-local frame 60.
        let candidates = [100_i64];
        let x = envelope_local_frame_to_x(band, duration, TimeCode(58));
        let (snapped, guide) = envelope_snapped_local_frame(
            band,
            clip_start,
            duration,
            x,
            &candidates,
            30,
            1.0,
            false,
        );
        assert_eq!(snapped, TimeCode(60), "the guide at project frame 100 wins");
        assert_eq!(
            guide,
            Some(100),
            "and paints through the existing snap path"
        );
        let (raw, guide) =
            envelope_snapped_local_frame(band, clip_start, duration, x, &candidates, 30, 1.0, true);
        assert_eq!(raw, TimeCode(58), "Alt bypasses snapping on envelope keys");
        assert_eq!(guide, None, "and paints no guide");
    }

    /// AU4 §7 B4: one rubber-band drag is one undo entry, on the
    /// `a_coalesced_bus_fader_drag_is_one_undo_entry` template
    /// (`au2_core.rs:1387`).
    #[test]
    fn a_coalesced_envelope_drag_is_one_undo_entry() {
        use kinewright_core::{Command, Core, Event};

        let mut document = linked_fixture();
        document.tracks[1].clips[0].audio_gain_curve = Some(envelope_curve(&[(0, 0), (20, 0)]));
        let clip = document.tracks[1].clips[0].id;
        let key = kinewright_core::envelope_coalesce_key(clip);
        assert_eq!(key, format!("envelope:{clip}"));
        let core = Core::spawn(document.clone()).unwrap();

        let duration = TimeCode(30);
        let stored = document.tracks[1].clips[0]
            .audio_gain_curve
            .clone()
            .unwrap();
        let mut last = None;
        for step in 1..=10_i64 {
            let moved = envelope_move_key(
                &stored.keyframes,
                1,
                TimeCode(20),
                i32::try_from(-step * 10).unwrap(),
                duration,
            );
            let Event::DocumentChanged { doc, .. } = core
                .request(Command::DoBatchCoalesced {
                    operations: vec![envelope_operation(clip, moved)],
                    coalesce_key: format!("{key}#1"),
                })
                .unwrap()
            else {
                panic!("a coalesced envelope batch should be accepted");
            };
            last = Some(doc);
        }
        let last = last.unwrap();
        assert_eq!(
            last.tracks[1].clips[0]
                .audio_gain_curve
                .as_ref()
                .unwrap()
                .keyframes[1]
                .value,
            -100
        );
        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("the gesture should be undoable");
        };
        assert_eq!(
            doc.as_ref(),
            &document,
            "one undo restores the pre-gesture document: ten drag frames are one entry"
        );

        // A frame whose recomputed curve equals the document writes nothing.
        let settled = envelope_move_key(&stored.keyframes, 1, TimeCode(20), 0, duration);
        assert_eq!(
            settled, stored.keyframes,
            "a frame that asks for the values the document already holds is not an edit"
        );

        // And a discrete edit in the same frame drops the key.
        let mut edits = crate::inspector_ui::InspectorEdits::default();
        edits.extend_live(
            vec![envelope_operation(clip, stored.keyframes.clone())],
            key.clone(),
        );
        assert_eq!(edits.coalesce_key(), Some(key.as_str()));
        edits.push(envelope_operation(clip, stored.keyframes.clone()));
        assert_eq!(
            edits.coalesce_key(),
            None,
            "a frame that also carries a discrete edit drops the key"
        );
    }

    /// AU4 §7 B4 (rule 105): the regression risk of grafting `InspectorEdits`
    /// into `timeline()` is that a clip move, a trim, a marker drag or a
    /// playhead drag starts filing per-frame batches. `timeline()` therefore
    /// owns exactly one `InspectorEdits`, submits it once, and never reaches
    /// the live path under any key but the envelope's.
    #[test]
    fn the_timeline_has_exactly_one_coalescing_path_and_the_envelope_owns_it() {
        const FILE: &str = include_str!("timeline_ui.rs");
        // The test module quotes every one of these names, so the pin reads
        // the production half of the file only.
        let source = FILE
            .split_once("\n#[cfg(test)]")
            .expect("timeline_ui.rs has a test module")
            .0;
        // AU5 §6.4 rule 129 adds two more, both **discrete**: the `Room
        // tone` button's own, submitted once at the end of
        // `fill_room_tone_at_selection`, and the one `poll_room_tone` lands a
        // finished capture through. The claim the pin defends is unchanged
        // and is the `extend_live` / `push_live` scan below: no path in this
        // file may file a per-frame coalesced batch but the envelope's.
        assert_eq!(
            source.matches("InspectorEdits::default()").count(),
            3,
            "the timeline gains ONE `InspectorEdits` for the envelope and AU5's two \
             discrete room-tone paths, and nothing else"
        );
        assert_eq!(
            source.matches("submit_inspector_edits(").count(),
            3,
            "and each submits exactly once"
        );
        for live in ["extend_live(", "push_live("] {
            for (index, _) in source.match_indices(live) {
                let window = &source[index..(index + 240).min(source.len())];
                assert!(
                    window.contains("envelope_coalesce_key"),
                    "every live write in the timeline is the envelope's: {live}"
                );
            }
        }
        assert!(
            source.matches("pending_operations = ").count() >= 5,
            "clip move, both trims, the marker drag and the ripple path keep their \
             `drag_stopped`-only batch"
        );
    }

    /// AU4 §7 B5: a painted frame of the band writes no operation and paints
    /// the polyline and its points at the contract's radii, in `ACCENT` when
    /// the clip is selected and `TEXT_PRIMARY_64` otherwise.
    #[test]
    fn a_painted_envelope_writes_nothing_and_draws_its_points_and_line() {
        let curve = envelope_curve(&[(0, 0), (15, -200), (29, 0)]);
        for (selected, expected) in [(false, color::TEXT_PRIMARY_64), (true, color::ACCENT)] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let band = envelope_test_band(size::TRACK_HEIGHT, MediaKind::Audio);
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                paint_clip_envelope(
                    ui.painter(),
                    band,
                    TimeCode(30),
                    &curve.keyframes,
                    0,
                    selected,
                );
            });
            let mut circles = Vec::new();
            let mut lines = Vec::new();
            for clipped in &output.shapes {
                collect_envelope_shapes(&clipped.shape, &mut circles, &mut lines);
            }
            assert_eq!(circles.len(), 3, "one point per key");
            for (radius, fill) in &circles {
                assert!((radius - ENVELOPE_POINT_RADIUS).abs() < f32::EPSILON);
                assert_eq!(*fill, expected);
            }
            assert!(
                lines.iter().any(
                    |(_, width, tint)| (width - ENVELOPE_STROKE).abs() < f32::EPSILON
                        && *tint == expected
                ),
                "the polyline is 1.6 px in {expected:?}; it painted {lines:?}"
            );
        }
        assert!(
            (ENVELOPE_HIT_RADIUS - crate::curve_editor_widget::HIT_RADIUS).abs() < f32::EPSILON
                && (ENVELOPE_POINT_RADIUS - crate::curve_editor_widget::POINT_RADIUS).abs()
                    < f32::EPSILON
                && (ENVELOPE_STROKE - crate::curve_editor_widget::CURVE_STROKE).abs()
                    < f32::EPSILON,
            "the three literals are the curve editor's own, reused rather than re-invented"
        );
    }

    /// AU4 §7 B6: the `Envelopes` toggle is session state, defaults on, and
    /// when off nothing paints and no interact is allocated.
    #[test]
    fn the_envelopes_toggle_is_session_state_that_defaults_on() {
        assert!(
            envelope_is_offered(TimeCode(30), 6.0, true),
            "the overlay is offered on an ordinary clip"
        );
        assert!(
            !envelope_is_offered(TimeCode(30), 6.0, false),
            "and hidden, with all its hit-testing, when the toggle is off"
        );
        let session = include_str!("project.rs");
        assert!(
            session.contains("show_envelopes: bool"),
            "the toggle lives beside `pixels_per_frame` on the session, not on the document"
        );
        assert!(session.contains("show_envelopes: true"), "and defaults on");
    }

    /// AU4 §7 B14: the timeline's first `include_str!` docs pin.
    #[test]
    fn the_design_note_states_the_timeline_envelope_rules() {
        const DESIGN: &str = include_str!("../../../docs/DESIGN.md");
        let timeline = DESIGN
            .split_once("### Timeline")
            .expect("DESIGN.md has a Timeline section")
            .1
            .split_once("\n### ")
            .expect("the Timeline section ends at the next heading")
            .0;
        // The file is hard-wrapped, so each phrase is one that fits a line.
        for expected in [
            "gain envelope",
            "`band.shrink2(vec2(2, 4))`, the same rect the waveform uses",
            "9 point hit radius",
            "3.5 point handles on a 1.6 point line",
            "`Envelopes` toolbar toggle",
            "Alt bypasses snapping on envelope keys as it does everywhere",
            "−40 to +12 dB",
            "Delete or Backspace over a key removes the key, not the clip",
            "a click on that line lands the first key there, holding that gain",
            "the band is the coarse gesture and the inspector's keyframe list is the exact one",
            // AU5 §6.5 rule 133: the `Room tone` button.
            "A `Room tone` toolbar button sits beside `Envelopes`",
            "grey until a clip is",
            "selected and that clip's track has a gap in it",
            "fills the gap nearest the",
            "playhead on that track",
            "butt joins and",
            "captures from that track's longest silence the first time it is",
            "Leading and interior gaps only",
            "The result is an ordinary clip",
            "trims, splits and",
            "deletes like any other, and deleting it brings the gap warning back",
            "one undo entry",
        ] {
            assert!(
                timeline.contains(expected),
                "DESIGN.md's Timeline section must state: {expected}"
            );
        }
    }

    /// AU4 §5.1 rule 101 (AU4 §0 E49): Delete arbitrates between the hovered
    /// envelope key and the selected clip.
    ///
    /// `keyboard_shortcuts` runs before the timeline paints, so the whole
    /// question is decided by [`envelope_delete_action`] against the report the
    /// timeline left last frame. Both directions are proven here: a hovered key
    /// yields a `SetClipGainEnvelope` that keeps the clip, and no hover yields
    /// `Clip`, which is the old `delete_selected` path untouched.
    #[test]
    fn delete_over_a_hovered_envelope_key_removes_the_key_and_not_the_clip() {
        let mut document = linked_fixture();
        document.tracks[1].clips[0].audio_gain_curve =
            Some(envelope_curve(&[(0, 0), (15, -200), (29, 0)]));
        let clip = document.tracks[1].clips[0].id;

        // No band hovered: Delete still deletes the selected clip.
        assert_eq!(
            envelope_delete_action(&document, None),
            EnvelopeDelete::Clip,
            "with nothing hovered Delete is the clip delete it has always been"
        );

        let hovered = envelope_delete_action(&document, Some(EnvelopeHover { clip, index: 1 }));
        let EnvelopeDelete::RemoveKey { clip: target, keys } = hovered else {
            panic!("a hovered key is removed, not the clip: {hovered:?}");
        };
        assert_eq!(target, clip, "the removal names the hovered clip");
        // The one operation `remove_hovered_envelope_key` sends for it.
        let operation = envelope_operation(target, keys);
        assert!(
            matches!(
                &operation,
                Operation::SetClipGainEnvelope { clip: target, curve: Some(_) } if *target == clip
            ),
            "the removal is one `SetClipGainEnvelope` on the hovered clip: {operation:?}"
        );
        let clips_before = document
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>();
        let mut applied = document.clone();
        kinewright_core::apply_batch(&mut applied, std::slice::from_ref(&operation))
            .expect("removing one key is a valid operation");
        assert_eq!(
            applied
                .tracks
                .iter()
                .map(|track| track.clips.len())
                .sum::<usize>(),
            clips_before,
            "no clip is deleted"
        );
        let remaining = applied
            .clip(clip)
            .and_then(|clip| clip.audio_gain_curve.as_ref())
            .expect("the envelope survives");
        assert_eq!(
            remaining
                .keyframes
                .iter()
                .map(|key| key.at.0)
                .collect::<Vec<_>>(),
            vec![0, 29],
            "and exactly the hovered key is gone"
        );

        // Rule 101's last-key refusal: clearing an envelope is the inspector's
        // `Clear`, so Delete on the only key deletes neither key nor clip.
        let mut single = document.clone();
        single.tracks[1].clips[0].audio_gain_curve = Some(envelope_curve(&[(0, 0)]));
        assert_eq!(
            envelope_delete_action(&single, Some(EnvelopeHover { clip, index: 0 })),
            EnvelopeDelete::RefuseLastKey,
            "the last key stays"
        );

        // A stale report — the curve has since been cleared — falls back to the
        // clip delete rather than doing nothing.
        let mut cleared = document.clone();
        cleared.tracks[1].clips[0].audio_gain_curve = None;
        assert_eq!(
            envelope_delete_action(&cleared, Some(EnvelopeHover { clip, index: 1 })),
            EnvelopeDelete::Clip,
            "a report whose curve is gone is not a veto"
        );

        // The arbitration has to happen before any `delete_selected` call, and
        // `keyboard_shortcuts` is the only place it can: it runs before the
        // timeline paints, so this is the pin that the branch stays first.
        let keys = include_str!("keys.rs");
        let envelope = keys
            .find("remove_hovered_envelope_key")
            .expect("`keyboard_shortcuts` consults the envelope report");
        let delete = keys
            .find("self.delete_selected()")
            .expect("`keyboard_shortcuts` still has its clip-delete path");
        assert!(
            envelope < delete,
            "the hovered-key branch is arbitrated before any `delete_selected`"
        );
        assert!(
            keys.contains("egui::Key::Backspace") && keys.contains("egui::Key::Delete"),
            "both keys reach it"
        );
        assert!(
            keys.contains("envelope_hover.take()"),
            "and the report is taken, so a frame in which the timeline does not draw \
             cannot leave a stale hover behind to swallow a later Delete"
        );
        assert!(
            keys.find("egui_wants_keyboard_input") < keys.find("remove_hovered_envelope_key"),
            "a Delete typed into a text field is still swallowed by the guard"
        );
        // AU4 §0 E53: the parked (curve-free) band is click-only, so a clip
        // drag that starts on it falls through to `body`.
        let timeline = include_str!("timeline_ui.rs");
        let sense = timeline
            .find("let sense = if keys.is_empty() {")
            .expect("the envelope interact picks its sense from the key list");
        assert!(
            timeline[sense..sense + 200].contains("egui::Sense::click()"),
            "the curve-free band senses clicks only"
        );
    }

    /// AU4 §0 E53: a clip with no curve still shows a band — one flat, muted
    /// line at its parked `audio_gain_tenth_db` — and a click on it lands the
    /// first key there.
    #[test]
    fn a_curve_free_clip_shows_a_flat_parked_line_that_a_click_seeds() {
        const PARKED: i32 = -60;
        let band = envelope_test_band(size::TRACK_HEIGHT, MediaKind::Audio);
        let parked_y = envelope_value_to_y(band, PARKED);

        // It paints: one line, no key dots, at the parked value, in the muted
        // tint that says "not an envelope yet".
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            paint_clip_envelope(ui.painter(), band, TimeCode(30), &[], PARKED, false);
        });
        let mut circles = Vec::new();
        let mut lines = Vec::new();
        for clipped in &output.shapes {
            collect_envelope_shapes(&clipped.shape, &mut circles, &mut lines);
        }
        assert!(circles.is_empty(), "a curve-free clip draws no key dots");
        assert_eq!(lines.len(), 1, "and exactly one line: {lines:?}");
        let (points, width, tint) = &lines[0];
        assert!(
            (width - ENVELOPE_STROKE).abs() < f32::EPSILON && *tint == color::TEXT_MUTED,
            "the parked line is {ENVELOPE_STROKE} px of TEXT_MUTED: {width} {tint:?}"
        );
        assert_eq!(points.len(), 2, "it is flat: {points:?}");
        assert!(
            (points[0].x - band.left()).abs() < f32::EPSILON
                && (points[1].x - band.right()).abs() < f32::EPSILON
                && (points[0].y - parked_y).abs() < f32::EPSILON
                && (points[1].y - parked_y).abs() < f32::EPSILON,
            "it spans the band at the parked value: {points:?} against {parked_y}"
        );

        // It hit-tests on the same 9 px rule the real curve does.
        let drawn = envelope_parked_polyline(band, PARKED);
        let on_line = egui::pos2(band.center().x, parked_y + 8.0);
        let off_line = egui::pos2(band.center().x, parked_y - 10.0);
        assert!(
            envelope_near_curve(&drawn, band, on_line),
            "8 px from the parked line is on it"
        );
        assert!(
            !envelope_near_curve(&drawn, band, off_line),
            "10 px from it is not"
        );
        assert!(
            envelope_hit(&[], band, on_line).is_none(),
            "there is no key to grab, so the click is an insertion"
        );

        // And the click inserts the first key at the snapped frame, carrying
        // the parked value, through the inspector's own `upsert_keyframe`.
        let at = envelope_x_to_local_frame(band, TimeCode(30), on_line.x);
        let seeded = crate::inspector_ui::upsert_keyframe(None, at, i64::from(PARKED)).keyframes;
        assert_eq!(seeded.len(), 1, "one key: {seeded:?}");
        assert_eq!(
            (seeded[0].at, seeded[0].value),
            (at, i64::from(PARKED)),
            "at the snapped frame, holding the parked gain"
        );

        // The 24 px floor still gates the whole offer, curve or no curve.
        assert!(
            !envelope_is_offered(TimeCode(30), 0.5, true),
            "a 15 px clip is offered no band to click"
        );
    }

    /// AU4 §5.1 rules 95 and 101: a band grab clamps to −600 … +120 but does
    /// not raise a key parked below the −400 display floor unless the pointer
    /// actually leaves the floor.
    #[test]
    fn a_band_grab_clamps_to_the_gain_range_without_raising_a_parked_key() {
        let band = envelope_test_band(size::TRACK_HEIGHT, MediaKind::Audio);

        // The floor of the band reads back as the display floor …
        assert_eq!(
            envelope_y_to_value(band, band.bottom()),
            ENVELOPE_DISPLAY_MIN_TENTH_DB
        );
        // … so a key stored at −500 would be raised to −400 by a purely
        // horizontal grab. It is not.
        assert_eq!(
            envelope_grab_value(band, band.bottom(), -500),
            -500,
            "a key below the display floor keeps its value while the pointer stays on the floor"
        );
        // The moment the pointer leaves the floor, the drag means it.
        let lifted = envelope_grab_value(band, band.center().y, -500);
        assert_eq!(
            lifted,
            envelope_y_to_value(band, band.center().y),
            "a pointer off the floor writes what it points at"
        );
        assert!(lifted > ENVELOPE_DISPLAY_MIN_TENTH_DB);
        // An ordinary key is unaffected: the floor is still the floor.
        assert_eq!(
            envelope_grab_value(band, band.bottom(), 0),
            ENVELOPE_DISPLAY_MIN_TENTH_DB,
            "a key inside the display range is clamped to the floor as before"
        );
        // And rule 101's own −600 … +120 clamp is untouched.
        let keys = envelope_curve(&[(0, 0), (29, 0)]).keyframes;
        let moved = envelope_move_key(&keys, 0, TimeCode(0), -900, TimeCode(30));
        assert_eq!(
            moved[0].value,
            i64::from(TRACK_MIX_GAIN_MIN),
            "the grab is still clamped to −600"
        );
    }

    /// AU4 §5.1 rules 98 and 99: the allocation predicate tests the rect the
    /// interact actually takes, not the whole band.
    ///
    /// The band reaches `space::HALF` into the clip's edge while the interact
    /// stops at `EDGE_HANDLE_WIDTH`, so a pointer sitting *outside* the clip,
    /// at the height of the line's left end, used to be within 9 px of the
    /// polyline and allocate an interact whose rect could never contain it.
    #[test]
    fn the_allocation_predicate_uses_the_interact_rect_not_the_band() {
        let lane =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, size::TRACK_HEIGHT));
        let clip_rect = egui::Rect::from_min_size(
            egui::pos2(lane.left() + 40.0, lane.top() + space::ONE),
            egui::vec2(244.0, lane.height() - space::TWO),
        );
        let band =
            envelope_band_rect(clip_rect, MediaKind::Audio).expect("an audio clip has a band");
        let body_rect = egui::Rect::from_min_max(
            egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.top()),
            egui::pos2(clip_rect.right() - EDGE_HANDLE_WIDTH, clip_rect.bottom()),
        );
        let interact = envelope_interact_rect(band, body_rect);
        assert!(
            interact.left() > band.left() && interact.right() < band.right(),
            "the interact is inside the band by the two trim handles"
        );

        let curve = envelope_curve(&[(0, 0), (29, 0)]);
        let points = envelope_points(band, TimeCode(30), &curve.keyframes);
        let drawn = envelope_polyline(band, &points, &curve.keyframes);
        let outside = egui::pos2(clip_rect.left() - 5.0, points[0].y);
        assert!(
            envelope_near_curve(&drawn, band, outside),
            "against the band, a pointer 5 px outside the clip is `near` the line"
        );
        assert!(
            !interact.contains(outside),
            "but the interact's rect does not contain it"
        );
        assert!(
            !envelope_near_curve(&drawn, interact, outside),
            "so tested against the interact rect it allocates nothing"
        );
        // And a pointer genuinely on the line still allocates.
        assert!(
            envelope_near_curve(&drawn, interact, egui::pos2(band.center().x, points[0].y)),
            "the ordinary hover is untouched"
        );
    }

    // -----------------------------------------------------------------------
    // AU5 §6.4: the `Room tone` button
    // -----------------------------------------------------------------------

    /// One audio track with a leading gap and an interior gap, plus a
    /// room-tone asset in the pool.
    ///
    /// Clips at 40..70 and 100..130 on a 30 fps project, so the gaps are
    /// `0..40` and `70..100` and the tail after 130 is **not** a gap.
    fn gapped_fixture(project_fps: Rational) -> Document {
        let asset_fps = Rational::new(30, 1).unwrap();
        let source = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("interview.wav"),
            name: "interview.wav".to_owned(),
            duration: TimeCode(600),
            fps: asset_fps,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let tone = MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("/p/show.kinewright-assets/room-tone/abc.wav"),
            name: "Room tone \u{2014} interview.wav".to_owned(),
            duration: TimeCode(60),
            fps: asset_fps,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let clip = |id: u64, at: i64, source_start: i64, source_end: i64| Clip {
            id: ClipId(id),
            asset: AssetId(1),
            source_range: TimeCode(source_start)..TimeCode(source_end),
            content: kinewright_core::ClipContent::Media,
            timeline_start: TimeCode(at),
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
            audio_gain_curve: None,
        };
        // The clips are written in the source domain so their project
        // durations are 30 frames at 30 fps and scale with the project rate.
        let second =
            map_project_duration_to_source(TimeCode::ZERO, TimeCode(30), asset_fps, project_fps)
                .unwrap_or(TimeCode(30));
        let span = map_source_range_to_project(TimeCode::ZERO..second, asset_fps, project_fps)
            .unwrap_or(TimeCode(30));
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![
                        clip(1, 40, 0, second.0),
                        clip(2, 70 + span.0, second.0, second.0 * 2),
                    ],
                },
            ],
            media_pool: vec![source, tone],
            markers: Vec::new(),
            fps: project_fps,
            resolution: (1_920, 1_080),
            lut_assets: Vec::new(),
            duration: TimeCode(400),
        }
    }

    /// AU5 §7 B15 (§6.4 rule 129): the button fills the gap **nearest the
    /// playhead on the selected clip's track**, and is disabled otherwise.
    #[test]
    fn au5_the_room_tone_button_targets_the_gap_nearest_the_playhead() {
        let fps = Rational::new(30, 1).unwrap();
        let document = gapped_fixture(fps);
        // Leading gap `0..40`, interior gap `70..100`; the tail after 130 is
        // not a gap and never appears.
        assert_eq!(
            document.track_gaps(TrackId(2)),
            Some(vec![TimeCode(0)..TimeCode(40), TimeCode(70)..TimeCode(100),])
        );
        for (playhead, expected) in [
            (TimeCode(0), TimeCode(0)..TimeCode(40)),
            (TimeCode(45), TimeCode(0)..TimeCode(40)),
            (TimeCode(80), TimeCode(70)..TimeCode(100)),
            (TimeCode(300), TimeCode(70)..TimeCode(100)),
        ] {
            let (track, gap) = room_tone_target(&document, ClipId(1), playhead)
                .expect("the clip's track has gaps");
            assert_eq!(track, TrackId(2));
            assert_eq!(gap, expected, "with the playhead at {}", playhead.0);
        }

        // A clip on a track with no gap, and a clip that is not in the
        // document at all, both disable the button.
        assert!(room_tone_target(&document, ClipId(99), TimeCode::ZERO).is_none());
        let mut gapless = document.clone();
        gapless.tracks[1].clips[0].timeline_start = TimeCode::ZERO;
        gapless.tracks[1].clips[1].timeline_start = TimeCode(30);
        assert_eq!(
            gapless.track_gaps(TrackId(2)),
            Some(Vec::new()),
            "butt-joined clips leave no gap"
        );
        assert!(room_tone_target(&gapless, ClipId(1), TimeCode::ZERO).is_none());
    }

    /// AU5 §7 B15 and §5.3: the fill is butt-joined, exact, and capped.
    #[test]
    fn au5_the_room_tone_fill_tiles_the_gap_exactly() {
        let fps = Rational::new(30, 1).unwrap();
        let document = gapped_fixture(fps);
        let tone = document
            .asset(AssetId(2))
            .expect("the pool holds room tone");
        let gap = TimeCode(0)..TimeCode(40);
        let operations = room_tone_fill_operations(&document, TrackId(2), &gap, tone)
            .expect("a 40-frame gap tiles from a 60-frame capture");
        assert_eq!(operations.len(), 1, "one tile covers it: {operations:?}");
        let Operation::AddClip {
            track,
            asset,
            at,
            source,
        } = &operations[0]
        else {
            panic!("a fill is `AddClip`s and nothing else: {operations:?}");
        };
        assert_eq!(*track, TrackId(2));
        assert_eq!(*asset, AssetId(2));
        assert_eq!(*at, TimeCode::ZERO);
        assert_eq!(*source, TimeCode::ZERO..TimeCode(40));

        // A gap longer than the capture tiles forward, butt-joined, with the
        // last tile short — and the tiles cover the gap exactly.
        let long = TimeCode(0)..TimeCode(150);
        let operations = room_tone_fill_operations(&document, TrackId(2), &long, tone)
            .expect("a 150-frame gap tiles from a 60-frame capture");
        assert_eq!(operations.len(), 3);
        let mut cursor = long.start;
        for operation in &operations {
            let Operation::AddClip { at, source, .. } = operation else {
                panic!("a fill is `AddClip`s: {operation:?}");
            };
            assert_eq!(*at, cursor, "butt-joined, never overlapping");
            let span = map_source_range_to_project(source.clone(), tone.fps, document.fps)
                .expect("the tile maps");
            cursor = TimeCode(cursor.0 + span.0);
        }
        assert_eq!(cursor, long.end, "the tiles cover the gap exactly");
        assert_eq!(
            operations.last().map(|operation| match operation {
                Operation::AddClip { source, .. } => source.end,
                _ => TimeCode::ZERO,
            }),
            Some(TimeCode(30)),
            "the last tile is the short one"
        );

        // The ceiling is a refusal with a reason, never a partial fill.
        let huge =
            TimeCode(0)..TimeCode(60 * i64::try_from(ROOM_TONE_MAX_TILES).unwrap_or(i64::MAX) + 60);
        let refusal = room_tone_fill_operations(&document, TrackId(2), &huge, tone)
            .expect_err("a gap past the tile ceiling is refused");
        assert!(
            refusal.contains(&ROOM_TONE_MAX_TILES.to_string()),
            "the refusal names the ceiling: {refusal}"
        );
    }

    /// AU5 §0 R12 and §5.3 rule 98: a gap with no exact source range is
    /// refused **by name** rather than filled one frame short.
    #[test]
    fn au5_a_gap_with_no_exact_source_range_is_refused_with_its_reason() {
        // A 60 fps project reading a 30 fps room-tone asset can only express
        // even-frame spans, so an odd gap has no exact source range at all.
        let document = gapped_fixture(Rational::new(60, 1).unwrap());
        let tone = document
            .asset(AssetId(2))
            .expect("the pool holds room tone");
        let odd = TimeCode(0)..TimeCode(7);
        let refusal = room_tone_fill_operations(&document, TrackId(2), &odd, tone)
            .expect_err("7 project frames is not representable at 30/60");
        assert!(
            refusal.contains("has no exact source range at 30/1 / 60/1"),
            "the refusal names both rates: {refusal}"
        );
        // An even gap at the same two rates fills exactly.
        let even = TimeCode(0)..TimeCode(8);
        let operations = room_tone_fill_operations(&document, TrackId(2), &even, tone)
            .expect("8 project frames is four source frames");
        assert_eq!(operations.len(), 1);
        let Operation::AddClip { source, .. } = &operations[0] else {
            panic!("a fill is `AddClip`s");
        };
        assert_eq!(*source, TimeCode::ZERO..TimeCode(4));
    }

    /// AU5 §5.1 rule 90: a silence longer than the writer cap is trimmed
    /// before it is decoded, not decoded and then refused.
    #[test]
    fn au5_a_capture_is_capped_before_it_is_decoded() {
        let fps = Rational::new(30, 1).unwrap();
        // 60 s at 30 fps is 1 800 asset frames.
        let long = TimeCode(100)..TimeCode(9_000);
        assert_eq!(
            capped_capture_range(&long, fps),
            TimeCode(100)..TimeCode(1_900)
        );
        let short = TimeCode(10)..TimeCode(40);
        assert_eq!(capped_capture_range(&short, fps), short);
    }

    /// AU5 §5.1 rule 89: "is this room tone?" is a question about the store,
    /// not about a document flag.
    #[test]
    fn au5_the_room_tone_asset_is_recognised_by_its_store_directory() {
        let document = gapped_fixture(Rational::new(30, 1).unwrap());
        let directory = PathBuf::from("/p/show.kinewright-assets/room-tone");
        assert_eq!(
            existing_room_tone_asset(&document, &directory).map(|asset| asset.id),
            Some(AssetId(2))
        );
        let elsewhere = PathBuf::from("/other/show.kinewright-assets/room-tone");
        assert!(
            existing_room_tone_asset(&document, &elsewhere).is_none(),
            "a capture from a different project's store is not this project's room tone"
        );
    }

    /// AU5 §0 R135 (media R101 reversed): a 25 fps fill takes the **covering**
    /// source range, not the phase-0 exact one.
    ///
    /// `map_project_duration_to_source(TimeCode::ZERO, 7, 30, 25)` answers
    /// `0..8`, which maps to exactly seven project frames and is still 640
    /// sample frames short of what the mixer will ask the decoder for, so the
    /// join dips to silence. `covering_source_range_for_project_duration`
    /// sweeps the phase and answers `1..10`.
    #[test]
    fn au5_a_twenty_five_fps_fill_covers_the_gap_in_samples_not_only_in_frames() {
        let project_fps = Rational::new(25, 1).unwrap();
        let asset_fps = Rational::new(30, 1).unwrap();
        let document = seven_frame_gap_fixture(project_fps, asset_fps);
        let tone = document
            .asset(AssetId(2))
            .expect("the pool holds room tone");
        let gap = TimeCode(0)..TimeCode(7);
        assert_eq!(
            document.track_gaps(TrackId(2)),
            Some(vec![gap.clone()]),
            "the fixture's only gap is the leading seven frames"
        );

        // The exact-in-frames answer, which is what R135 reverses.
        assert_eq!(
            map_project_duration_to_source(TimeCode::ZERO, TimeCode(7), asset_fps, project_fps),
            Ok(TimeCode(8)),
            "phase 0 is exact in frames — and short in samples"
        );
        let covering = covering_source_range_for_project_duration(
            TimeCode(7),
            asset_fps,
            project_fps,
            tone.duration,
            kinewright_media::ROOM_TONE_SAMPLE_RATE,
        )
        .expect("a 60-frame capture covers seven project frames at 25 fps");
        assert_eq!(
            covering,
            TimeCode(1)..TimeCode(10),
            "the covering range is the phase-1 one"
        );

        let operations = room_tone_fill_operations(&document, TrackId(2), &gap, tone)
            .expect("the seven-frame gap fills");
        assert_eq!(operations.len(), 1);
        let Operation::AddClip { at, source, .. } = &operations[0] else {
            panic!("a fill is `AddClip`s: {operations:?}");
        };
        assert_eq!(*at, TimeCode::ZERO);
        assert_eq!(
            *source, covering,
            "the button lays exactly the range the core helper answers"
        );

        // And the fill really closes the gap: applied, the track has none left.
        let mut filled = document.clone();
        for operation in &operations {
            operation
                .apply(&mut filled)
                .expect("the fill is a document the core accepts");
        }
        assert_eq!(
            filled.track_gaps(TrackId(2)),
            Some(Vec::new()),
            "no residual `track_gap` remains after the fill"
        );
    }

    /// One 25 fps audio track whose only gap is the leading seven frames.
    ///
    /// The dialogue clip starts at project frame 7 and runs 12 source frames
    /// of a 30 fps asset, which is ten project frames at 25.
    fn seven_frame_gap_fixture(project_fps: Rational, asset_fps: Rational) -> Document {
        let source = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("interview.wav"),
            name: "interview.wav".to_owned(),
            duration: TimeCode(600),
            fps: asset_fps,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        };
        let tone = MediaAsset {
            id: AssetId(2),
            path: PathBuf::from("/p/show.kinewright-assets/room-tone/abc.wav"),
            name: "Room tone \u{2014} interview.wav".to_owned(),
            duration: TimeCode(60),
            fps: asset_fps,
            ..source.clone()
        };
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![Clip {
                        id: ClipId(1),
                        asset: AssetId(1),
                        source_range: TimeCode(0)..TimeCode(12),
                        content: kinewright_core::ClipContent::Media,
                        timeline_start: TimeCode(7),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                        audio_gain_curve: None,
                    }],
                },
            ],
            media_pool: vec![source, tone],
            markers: Vec::new(),
            fps: project_fps,
            resolution: (1_920, 1_080),
            lut_assets: Vec::new(),
            // The project is exactly its content: 7 frames of gap plus the
            // dialogue clip's ten, so `apply` accepts the fill.
            duration: TimeCode(17),
        }
    }

    /// AU5 §0 R137: the repeating tile is the longest one the asset can
    /// **cover**, not its whole mapped length.
    ///
    /// A 61-frame capture maps to 51 project frames at 25 fps, and a 51-frame
    /// tile is impossible: the demand is `⌈51 × 48 000 / 25⌉ = 97 920` sample
    /// frames and the whole asset supplies `⌊61 × 48 000 / 30⌋ = 97 600`. That
    /// is not a rare shape — every capture length with `D mod 6 ∈ {1, 2, 3}`
    /// has it, about half of them — and asking for it refused the **whole**
    /// fill of any gap at least as long as the tone. One project frame shorter
    /// tiles the same gap exactly.
    #[test]
    fn au5_a_full_tile_that_cannot_cover_steps_down_instead_of_refusing_the_fill() {
        let project_fps = Rational::new(25, 1).unwrap();
        let asset_fps = Rational::new(30, 1).unwrap();
        let mut document = seven_frame_gap_fixture(project_fps, asset_fps);
        // A capture in the failing residue class, and a gap twice its length.
        document
            .media_pool
            .iter_mut()
            .find(|asset| asset.id == AssetId(2))
            .expect("the pool holds room tone")
            .duration = TimeCode(61);
        let tone = document
            .asset(AssetId(2))
            .expect("the pool holds room tone");

        // The premise: the whole asset's own mapped length has no covering
        // range, so a tiler that asks for it refuses everything.
        let whole =
            map_source_range_to_project(TimeCode::ZERO..tone.duration, asset_fps, project_fps)
                .expect("the asset maps");
        assert_eq!(whole, TimeCode(51));
        assert!(
            covering_source_range_for_project_duration(
                whole,
                asset_fps,
                project_fps,
                tone.duration,
                kinewright_media::ROOM_TONE_SAMPLE_RATE,
            )
            .is_err(),
            "the premise: a 51-frame tile cannot be covered by a 61-frame asset at 30 → 25"
        );
        let tile = tile_for(&document, tone, TimeCode(100))
            .expect("one frame shorter is coverable, and core's search finds it");
        assert_eq!(tile, TimeCode(50), "the search lands on the frame below");

        let gap = TimeCode(0)..TimeCode(100);
        let operations = room_tone_fill_operations(&document, TrackId(2), &gap, tone)
            .expect("a 100-frame gap tiles from a 61-frame capture rather than refusing");
        assert_eq!(operations.len(), 2, "50 + 50: {operations:?}");
        let mut cursor = gap.start;
        for operation in &operations {
            let Operation::AddClip { at, source, .. } = operation else {
                panic!("a fill is `AddClip`s: {operation:?}");
            };
            assert_eq!(*at, cursor, "butt-joined, never overlapping");
            let span = map_source_range_to_project(source.clone(), tone.fps, project_fps)
                .expect("the tile maps");
            cursor = TimeCode(cursor.0 + span.0);
        }
        assert_eq!(cursor, gap.end, "the tiles cover the gap exactly");

        // And it is a real fill: laid into the document, the gap is gone.
        let mut filled = document.clone();
        filled.tracks[1].clips[0].timeline_start = TimeCode(100);
        filled.duration = TimeCode(110);
        assert_eq!(
            filled.track_gaps(TrackId(2)),
            Some(vec![gap.clone()]),
            "the fixture's only gap is the leading hundred frames"
        );
        for operation in
            &room_tone_fill_operations(&filled, TrackId(2), &gap, tone).expect("the fill holds")
        {
            operation
                .apply(&mut filled)
                .expect("the fill is a document the core accepts");
        }
        assert_eq!(
            filled.track_gaps(TrackId(2)),
            Some(Vec::new()),
            "no residual `track_gap` remains after a multi-tile fill"
        );
    }

    /// Core's tile search at the app's own sample rate, for a whole gap.
    fn tile_for(
        document: &Document,
        asset: &MediaAsset,
        gap: TimeCode,
    ) -> Result<TimeCode, String> {
        kinewright_core::longest_coverable_project_tile(
            asset.duration,
            asset.fps,
            document.fps,
            kinewright_media::ROOM_TONE_SAMPLE_RATE,
            gap,
        )
        .map(|(tile, _)| tile)
        .map_err(|error| error.to_string())
    }

    /// AU5 §0 R139: each of core's three mapping refusals gets its own
    /// sentence, and none of them gets another's.
    ///
    /// `SourceTooShortToCover` is the one whose answer really is *record more
    /// room tone*; under a `_ =>` wildcard it printed
    /// `InexactDuration`'s "no exact source range", which sends an editor to
    /// look at the rate pair instead of at the microphone — the exact
    /// misdirection R137 exists to end.
    #[test]
    fn au5_each_mapping_refusal_gets_its_own_sentence() {
        use kinewright_core::TimeMappingError;
        let source_fps = Rational::new(30, 1).unwrap();
        let project_fps = Rational::new(25, 1).unwrap();
        let reason = |error: &TimeMappingError| {
            room_tone_skip_reason(TimeCode(7), source_fps, project_fps, error)
        };

        let inexact = reason(&TimeMappingError::InexactDuration {
            source_start: 0,
            project_duration: 7,
        });
        let no_covering = reason(&TimeMappingError::NoCoveringSourceRange {
            project_duration: 7,
            source_duration: 61,
        });
        let too_short = reason(&TimeMappingError::SourceTooShortToCover {
            project_duration: 7,
            minimum_source_frames: 62,
            source_duration: 61,
        });
        for sentence in [&inexact, &no_covering, &too_short] {
            assert!(
                sentence.contains("gap of 7 project frames"),
                "every refusal names the span that was asked for: {sentence}"
            );
            assert!(
                sentence.contains("30/1 / 25/1"),
                "and both rates: {sentence}"
            );
        }
        assert_ne!(inexact, no_covering);
        assert_ne!(no_covering, too_short);
        assert_ne!(inexact, too_short);

        assert!(inexact.ends_with("no exact source range at 30/1 / 25/1"));
        assert!(
            no_covering.contains("also carries its sample frames within the 61-source-frame"),
            "{no_covering}"
        );
        assert!(
            too_short.contains("needs at least 62 source frames of room tone")
                && too_short.contains("carries only 61"),
            "the one refusal whose answer is `record more room tone` says so: {too_short}"
        );
        assert!(
            !too_short.contains("no exact source range"),
            "and never wears `InexactDuration`'s sentence: {too_short}"
        );

        // No `_ =>` over the three named variants: the wildcard exists, but it
        // catches only what core adds later, and it says so rather than
        // borrowing one of these sentences.
        let other = reason(&TimeMappingError::NegativeFrames(TimeCode(-1)));
        assert!(
            other.contains("has no usable source range at 30/1 / 25/1"),
            "{other}"
        );
        for sentence in [&inexact, &no_covering, &too_short] {
            assert_ne!(*sentence, other);
        }
    }

    /// AU5 §0 R141: an uncoverable remainder is **split again**, not refused.
    ///
    /// Coverability is not monotone in `want`, so a remainder core cannot
    /// cover says nothing about `remainder − 1` or about 1. Asking core once
    /// for the whole gap and then insisting the leftover be coverable at
    /// exactly its own length refused gaps `plan_room_tone_fill` fills: 300
    /// combinations at 30 → 59.94, 226 at 30 → 50, 240 at 30 → 48, all with a
    /// store-produced 30 fps tone, because `RoomToneStore::asset_fps()` is
    /// `Rational::default()` and those three are workspace project rates.
    ///
    /// This is the reviewer's worked case: a twenty-second tone and a
    /// 1 001-frame gap at 30 → 59.94. Asking for the residue of 2 refuses;
    /// asking core for "the largest span at most 2" answers 1, twice.
    #[test]
    fn au5_an_uncoverable_remainder_is_split_again_rather_than_refusing_the_gap() {
        let project_fps = Rational::new(60_000, 1_001).unwrap();
        let asset_fps = Rational::new(30, 1).unwrap();
        let mut document = seven_frame_gap_fixture(project_fps, asset_fps);
        document.tracks[1].clips[0].timeline_start = TimeCode(1_001);
        // Twelve 30 fps source frames are twenty-four project frames at 59.94.
        document.duration = TimeCode(1_025);
        document
            .media_pool
            .iter_mut()
            .find(|asset| asset.id == AssetId(2))
            .expect("the pool holds room tone")
            .duration = TimeCode(600);
        let tone = document
            .asset(AssetId(2))
            .expect("the pool holds room tone");
        let gap = TimeCode(0)..TimeCode(1_001);
        assert_eq!(document.track_gaps(TrackId(2)), Some(vec![gap.clone()]));

        // The premise: the whole gap's tile leaves a residue of 2, and 2 is
        // not coverable at this rate pair while 1 is.
        assert_eq!(tile_for(&document, tone, gap.end), Ok(TimeCode(999)));
        assert!(
            covering_source_range_for_project_duration(
                TimeCode(2),
                asset_fps,
                project_fps,
                tone.duration,
                kinewright_media::ROOM_TONE_SAMPLE_RATE,
            )
            .is_err(),
            "the residue of 2 has no covering range"
        );
        assert_eq!(tile_for(&document, tone, TimeCode(2)), Ok(TimeCode(1)));

        let operations = room_tone_fill_operations(&document, TrackId(2), &gap, tone)
            .expect("the residue is split again rather than refusing the gap");
        assert_eq!(operations.len(), 3, "999 + 1 + 1: {operations:?}");
        let mut cursor = gap.start;
        for operation in &operations {
            let Operation::AddClip { at, source, .. } = operation else {
                panic!("a fill is `AddClip`s: {operation:?}");
            };
            assert_eq!(*at, cursor, "butt-joined, never overlapping");
            let span = map_source_range_to_project(source.clone(), tone.fps, project_fps)
                .expect("the tile maps");
            cursor = TimeCode(cursor.0 + span.0);
        }
        assert_eq!(cursor, gap.end, "the three tiles sum to the gap exactly");

        let mut filled = document.clone();
        for operation in &operations {
            operation
                .apply(&mut filled)
                .expect("the fill is a document the core accepts");
        }
        assert_eq!(
            filled.track_gaps(TrackId(2)),
            Some(Vec::new()),
            "no residual `track_gap` remains at 30 \u{2192} 59.94"
        );
    }

    /// AU5 §0 R141: a partly-tiled gap names **both** the gap and the residue.
    ///
    /// The headline names the gap, which is the span an editor can see and
    /// asked about; core's refusal is about whatever is left of it. Printing
    /// only the residue said "gap of 1 project frames" about a 241-frame hole,
    /// and printing only the gap hid which span actually had no range. The
    /// annotation is `plan_room_tone_fill`'s, word for word.
    #[test]
    fn au5_a_refused_tile_names_both_the_gap_and_the_residue() {
        // 30 → 60 can only express even spans. A two-second tone tiles 120
        // project frames at a time, so a 241-frame gap lays two full tiles and
        // then has a **one**-frame residue that no source range can express.
        let project_fps = Rational::new(60, 1).unwrap();
        let asset_fps = Rational::new(30, 1).unwrap();
        let mut document = seven_frame_gap_fixture(project_fps, asset_fps);
        document.tracks[1].clips[0].timeline_start = TimeCode(241);
        document.duration = TimeCode(265);
        let tone = document
            .asset(AssetId(2))
            .expect("the pool holds room tone");
        let gap = TimeCode(0)..TimeCode(241);
        assert_eq!(document.track_gaps(TrackId(2)), Some(vec![gap.clone()]));
        assert_eq!(
            tile_for(&document, tone, gap.end),
            Ok(TimeCode(120)),
            "the repeating tile is the whole two-second capture"
        );
        let refusal = room_tone_fill_operations(&document, TrackId(2), &gap, tone)
            .expect_err("an odd residue is not representable at 30 \u{2192} 60");
        assert!(
            refusal.contains("gap of 241 project frames"),
            "the headline names the hole the editor can see: {refusal}"
        );
        assert!(
            refusal.contains("2 tile(s) covered all but its last 1 project frame(s)"),
            "and the annotation names how far it got and what is left: {refusal}"
        );

        // A gap that fails on its very first pass carries no annotation: there
        // is no residue distinct from the gap to name.
        let mut odd = document.clone();
        odd.tracks[1].clips[0].timeline_start = TimeCode(1);
        odd.duration = TimeCode(25);
        let tone = odd.asset(AssetId(2)).expect("the pool holds room tone");
        let first = TimeCode(0)..TimeCode(1);
        let refusal = room_tone_fill_operations(&odd, TrackId(2), &first, tone)
            .expect_err("one project frame is not representable at 30 \u{2192} 60");
        assert!(refusal.contains("gap of 1 project frames"), "{refusal}");
        assert!(
            !refusal.contains("tile(s) covered"),
            "nothing was covered, so nothing claims to have been: {refusal}"
        );
    }

    /// AU5 §0 R140: an NTSC capture under the floor is refused as a
    /// **coverage** failure naming the asset's own length, not as a rate one.
    ///
    /// The refusal has to send the editor to the microphone, not to the frame
    /// rate. Which sentence it is is core's call, not the app's: below the
    /// ~501-source-frame floor at 30 → 29.97 the search's last refusal is
    /// `NoCoveringSourceRange`, whose sentence ends by naming the
    /// **60-source-frame room-tone asset** as the thing the samples had to fit
    /// inside. The app no longer computes a floor of its own to append (R138's
    /// first text did, and it was a figure the app remembered rather than one
    /// the search produced).
    #[test]
    fn au5_a_short_ntsc_capture_is_refused_with_the_length_it_needs() {
        let project_fps = Rational::new(30_000, 1_001).unwrap();
        let asset_fps = Rational::new(30, 1).unwrap();
        let document = seven_frame_gap_fixture(project_fps, asset_fps);
        let tone = document
            .asset(AssetId(2))
            .expect("the pool holds room tone");
        assert_eq!(tone.duration, TimeCode(60), "two seconds of room tone");
        let gap = TimeCode(0)..TimeCode(7);
        let refusal = room_tone_fill_operations(&document, TrackId(2), &gap, tone)
            .expect_err("a two-second capture covers nothing at 29.97");
        assert!(
            refusal.contains("within the 60-source-frame room-tone asset"),
            "the refusal names the asset's own length as the binding limit: {refusal}"
        );
        assert!(
            refusal.contains("also carries its sample frames"),
            "and says coverage, not representability, is what failed: {refusal}"
        );
        // One frame over the floor and the same gap fills, which is what makes
        // the sentence above actionable rather than fatalistic.
        let mut longer = document.clone();
        longer
            .media_pool
            .iter_mut()
            .find(|asset| asset.id == AssetId(2))
            .expect("the pool holds room tone")
            .duration = TimeCode(501);
        let tone = longer.asset(AssetId(2)).expect("the pool holds room tone");
        assert!(
            room_tone_fill_operations(&longer, TrackId(2), &gap, tone).is_ok(),
            "501 source frames is where covering first becomes possible at this rate pair"
        );
    }

    /// AU5 §0 R138 / R140: an NTSC project fills, and the tile starts deep in
    /// the capture.
    ///
    /// 30000/1001 against a 30/1 capture is the rate pair every North American
    /// project is in, and it needs both of core's AU5 searches. A 29.97
    /// project frame demands 1 601.6 sample frames and a 30 fps source frame
    /// supplies 1 600, so **no phase-0 range can ever cover**: the covering
    /// range for a 40-frame gap is `460..501`, forty-one source frames from
    /// the middle of the capture, which only a phase sweep as deep as the
    /// ratio's period can find.
    ///
    /// Two capture lengths, and the second is the point of R140. Twenty
    /// seconds is comfortably over the ~501-source-frame floor below which
    /// nothing at this rate pair covers anything. **Forty seconds is the
    /// length the app's own four-frame step-down used to refuse** — its
    /// coverable tile is 1 000 project frames — the span of the 1 001-frame
    /// map period — 199 frames
    /// below what 1 200 source frames map to — and it is an entirely ordinary
    /// capture, so that bound skipped every gap on the track for a large part
    /// of all NTSC recordings.
    #[test]
    fn au5_a_drop_frame_project_still_proposes_a_fill() {
        let project_fps = Rational::new(30_000, 1_001).unwrap();
        let asset_fps = Rational::new(30, 1).unwrap();
        for (label, capture) in [("twenty seconds", 600_i64), ("forty seconds", 1_200)] {
            let mut document = seven_frame_gap_fixture(project_fps, asset_fps);
            document.tracks[1].clips[0].timeline_start = TimeCode(40);
            document.duration = TimeCode(52);
            document
                .media_pool
                .iter_mut()
                .find(|asset| asset.id == AssetId(2))
                .expect("the pool holds room tone")
                .duration = TimeCode(capture);
            let tone = document
                .asset(AssetId(2))
                .expect("the pool holds room tone");
            let gap = TimeCode(0)..TimeCode(40);
            assert_eq!(document.track_gaps(TrackId(2)), Some(vec![gap.clone()]));
            let operations = room_tone_fill_operations(&document, TrackId(2), &gap, tone)
                .unwrap_or_else(|error| {
                    panic!("a {label} 30 fps capture fills an NTSC gap: {error}")
                });
            assert_eq!(operations.len(), 1, "{label}: {operations:?}");
            let Operation::AddClip { at, source, .. } = &operations[0] else {
                panic!("a fill is `AddClip`s: {operations:?}");
            };
            assert_eq!(*at, TimeCode::ZERO);
            assert_eq!(
                *source,
                TimeCode(460)..TimeCode(501),
                "{label}: forty-one source frames from the middle of the capture, which only \
                 a phase sweep deeper than three can find"
            );
            let mut filled = document.clone();
            for operation in &operations {
                operation
                    .apply(&mut filled)
                    .expect("the fill is a document the core accepts");
            }
            assert_eq!(
                filled.track_gaps(TrackId(2)),
                Some(Vec::new()),
                "{label}: no residual `track_gap` remains at 30000/1001"
            );
        }
    }

    /// AU5 §0 R134: a room-tone refusal is filed under `Timeline`, and the
    /// inspector's own default is still `Look`.
    #[test]
    fn au5_a_room_tone_refusal_is_not_filed_under_look() {
        assert_eq!(
            InspectorEdits::default().error_category(),
            crate::inspector_ui::LOOK_ERROR_CATEGORY,
            "every CC4-CC6 look card keeps the category it has always had"
        );
        let mut edits = InspectorEdits::default();
        edits.set_error_category(ROOM_TONE_ERROR_CATEGORY);
        assert_eq!(edits.error_category(), "Timeline");
        assert_ne!(
            ROOM_TONE_ERROR_CATEGORY,
            crate::inspector_ui::LOOK_ERROR_CATEGORY,
            "the button is a timeline gesture; a `Room tone` refusal under `Look` \
             sends the editor to a surface that had nothing to do with it"
        );
        // Both submitting paths set it: the production sources are read here
        // because neither path can be driven without a `KinewrightApp`.
        let source = include_str!("timeline_ui.rs")
            .split_once("\n#[cfg(test)]")
            .expect("timeline_ui.rs has a test module")
            .0;
        assert_eq!(
            source
                .matches("set_error_category(ROOM_TONE_ERROR_CATEGORY)")
                .count(),
            2,
            "`fill_room_tone_at_selection` and `poll_room_tone` both set it"
        );
        assert_eq!(
            source.matches("InspectorEdits::default()").count(),
            3,
            "and those are two of the file's three `InspectorEdits`; the third is \
             the envelope's, which keeps the `Look` default"
        );
    }
}
