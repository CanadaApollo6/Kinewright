//! CC3 §7 colour trackballs.
//!
//! Three balls — Lift (shadows), Gamma (midtones), Gain (highlights) — each with
//! an adjacent master slider. The ball position `(u, v)` in the unit disc maps to
//! per-channel integer deltas with primary directions R at 90°, G at 210°, and
//! B at 330°:
//!
//! ```text
//! delta_c = round_half_away_from_zero(k * (u*cos(theta_c) + v*sin(theta_c)))
//! ```
//!
//! `k` is the control's maximum magnitude: `2000` for lift, `1000` for the
//! gamma/gain deviation from neutral. Results are clamped to the descriptor
//! bounds.
//!
//! The widget owns no document state. It reads the stored integers, reports the
//! integers a gesture asks for, and leaves every operation, undo, and keyframe
//! decision to `inspector_ui`. The mapping itself is a pure function so the
//! numbers are provable without a window.

use eframe::egui;
use kinewright_core::{ColorWheelChannel, ColorWheelControl, ColorWheelControlSet};

use crate::{
    inspector_ui::is_live_drag,
    theme::{color, space, type_size},
};

/// Side of one trackball, in points.
const BALL_DIAMETER: f32 = 104.0;
/// Fraction of the ball radius at which the channel ticks start.
const TICK_INNER: f32 = 0.74;
/// Radius of the position indicator, in points.
const INDICATOR_RADIUS: f32 = 4.0;

/// The three primary directions of CC3 §7, in degrees.
const CHANNEL_ANGLES_DEGREES: [f64; 3] = [90.0, 210.0, 330.0];

/// The channels the ball drives, in [`CHANNEL_ANGLES_DEGREES`] order.
///
/// The master control is driven by the adjacent slider, never by the ball.
pub(crate) const BALL_CHANNELS: [ColorWheelChannel; 3] = [
    ColorWheelChannel::Red,
    ColorWheelChannel::Green,
    ColorWheelChannel::Blue,
];

const CHANNEL_COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(0xE0, 0x5A, 0x5A),
    egui::Color32::from_rgb(0x5C, 0xC2, 0x76),
    egui::Color32::from_rgb(0x5A, 0x8C, 0xE0),
];

/// One trackball's document state.
///
/// `values` are the stored integers of the four `color_wheels` controls in this
/// family; `keyframed` flags them in [`ColorWheelChannel::ALL`] order.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ColorWheelState {
    pub(crate) control: ColorWheelControl,
    pub(crate) values: ColorWheelControlSet,
    pub(crate) keyframed: [bool; 4],
}

impl ColorWheelState {
    fn is_keyframed(&self, channel: ColorWheelChannel) -> bool {
        let index = match channel {
            ColorWheelChannel::Master => 0,
            ColorWheelChannel::Red => 1,
            ColorWheelChannel::Green => 2,
            ColorWheelChannel::Blue => 3,
        };
        self.keyframed[index]
    }
}

/// What one frame of trackball interaction asks the document to become.
#[derive(Debug, Clone, Default)]
pub(crate) struct ColorWheelResponse {
    /// The controls whose stored integer must change, with their new values.
    pub(crate) changes: Vec<(ColorWheelChannel, i64)>,
    /// Whether this frame belongs to a live drag, so it coalesces into one
    /// undo entry.
    pub(crate) live: bool,
    /// Whether a drag began this frame, so a fresh gesture identity is opened.
    pub(crate) gesture_started: bool,
    /// Whether the ball was double-clicked: reset this wheel's four controls.
    pub(crate) reset: bool,
}

/// Rounds halves away from zero, as CC3 §7 requires.
///
/// `f64::round` is exactly that rule — it is named here so the contract is
/// visible at the call site and cannot silently become banker's rounding.
fn round_half_away_from_zero(value: f64) -> f64 {
    value.round()
}

/// The maximum magnitude `k` of one control family (CC3 §7).
#[must_use]
pub(crate) const fn control_scale(control: ColorWheelControl) -> f64 {
    match control {
        ColorWheelControl::Lift => 2_000.0,
        ColorWheelControl::Gamma | ColorWheelControl::Gain => 1_000.0,
    }
}

/// Map a unit-disc position to the red, green, and blue integer deltas.
///
/// Pure, and the only place the CC3 §7 formula exists. `u` is right-positive and
/// `v` is up-positive; neither is clamped here, so a caller that hands in a
/// position outside the disc gets the honest projection.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn disc_to_channel_deltas(u: f64, v: f64, k: f64) -> [i64; 3] {
    let mut deltas = [0_i64; 3];
    for (delta, degrees) in deltas.iter_mut().zip(CHANNEL_ANGLES_DEGREES) {
        let theta = degrees.to_radians();
        let projection = u * theta.cos() + v * theta.sin();
        *delta = round_half_away_from_zero(k * projection) as i64;
    }
    deltas
}

/// The stored integers a disc position asks for, clamped to descriptor bounds.
#[must_use]
pub(crate) fn disc_channel_values(control: ColorWheelControl, u: f64, v: f64) -> [i64; 3] {
    let (min, max, neutral) = control.bounds();
    disc_to_channel_deltas(u, v, control_scale(control))
        .map(|delta| neutral.saturating_add(delta).clamp(min, max))
}

/// The disc position that best explains three channel deltas.
///
/// Three unit vectors 120° apart form a tight frame with constant `3/2`, so the
/// least-squares inverse of [`disc_to_channel_deltas`] is `2/3` of the summed
/// projection. It is used only to draw the indicator: two degrees of freedom
/// cannot represent every clamped triple exactly, and the stored integers, not
/// the dot, remain the truth.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn channel_deltas_to_disc(deltas: [i64; 3], k: f64) -> (f64, f64) {
    if k == 0.0 {
        return (0.0, 0.0);
    }
    let mut u = 0.0;
    let mut v = 0.0;
    for (delta, degrees) in deltas.into_iter().zip(CHANNEL_ANGLES_DEGREES) {
        let theta = degrees.to_radians();
        let scaled = delta as f64 / k;
        u += scaled * theta.cos();
        v += scaled * theta.sin();
    }
    clamp_to_disc(u * 2.0 / 3.0, v * 2.0 / 3.0)
}

/// Clamp a position to the closed unit disc, preserving its direction.
#[must_use]
pub(crate) fn clamp_to_disc(u: f64, v: f64) -> (f64, f64) {
    let length = u.hypot(v);
    if length <= 1.0 {
        return (u, v);
    }
    (u / length, v / length)
}

/// The human read-out of one stored control integer (CC3 §7).
///
/// Lift is basis points of the `grade709` range; gamma and gain are
/// thousandths of an exponent or slope.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn wheel_readout(control: ColorWheelControl, value: i64) -> String {
    match control {
        ColorWheelControl::Lift => format!("{:+.4}", value as f64 / 10_000.0),
        ColorWheelControl::Gamma | ColorWheelControl::Gain => {
            format!("{:.3}", value as f64 / 1_000.0)
        }
    }
}

/// The human label of one control family.
#[must_use]
pub(crate) const fn control_label(control: ColorWheelControl) -> &'static str {
    match control {
        ColorWheelControl::Lift => "Lift",
        ColorWheelControl::Gamma => "Gamma",
        ColorWheelControl::Gain => "Gain",
    }
}

/// The stable token of one control family, used in coalesce keys.
#[must_use]
pub(crate) const fn control_token(control: ColorWheelControl) -> &'static str {
    match control {
        ColorWheelControl::Lift => "lift",
        ColorWheelControl::Gamma => "gamma",
        ColorWheelControl::Gain => "gain",
    }
}

/// Draw one trackball with its master slider and numeric read-outs.
pub(crate) fn color_wheel(ui: &mut egui::Ui, state: &ColorWheelState) -> ColorWheelResponse {
    let mut response = ColorWheelResponse::default();
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(control_label(state.control))
                .size(type_size::CAPTION)
                .color(color::TEXT_SECONDARY),
        );
        let (rect, ball) = ui.allocate_exact_size(
            egui::vec2(BALL_DIAMETER, BALL_DIAMETER),
            egui::Sense::click_and_drag(),
        );
        paint_ball(ui, rect, state);
        let ball = ball.on_hover_text(
            "Drag to grade the red, green, and blue controls; double-click to reset this wheel.",
        );
        if ball.double_clicked() {
            response.reset = true;
        } else {
            if ball.drag_started() {
                response.gesture_started = true;
            }
            if is_live_drag(&ball) {
                if let Some(pointer) = ball.interact_pointer_pos() {
                    let (u, v) = pointer_to_disc(rect, pointer);
                    let values = disc_channel_values(state.control, u, v);
                    for (channel, value) in BALL_CHANNELS.into_iter().zip(values) {
                        if value != state.values.channel(channel) {
                            response.changes.push((channel, value));
                        }
                    }
                }
                response.live = true;
            }
        }
        let (min, max, _) = state.control.bounds();
        let mut master = state.values.master;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("M").size(type_size::MICRO));
            ui.spacing_mut().slider_width = BALL_DIAMETER - 22.0;
            let slider = ui.add(
                egui::Slider::new(&mut master, min..=max)
                    .integer()
                    .show_value(false),
            );
            if slider.drag_started() {
                response.gesture_started = true;
            }
            if slider.changed() {
                response.changes.push((ColorWheelChannel::Master, master));
                if is_live_drag(&slider) {
                    response.live = true;
                }
            }
        });
        for channel in ColorWheelChannel::ALL {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(channel_initial(channel))
                        .size(type_size::MICRO)
                        .color(channel_color(channel)),
                );
                ui.monospace(
                    egui::RichText::new(wheel_readout(
                        state.control,
                        state.values.channel(channel),
                    ))
                    .size(type_size::MICRO),
                );
                if state.is_keyframed(channel) {
                    ui.label(
                        egui::RichText::new("KEY")
                            .size(type_size::MICRO)
                            .color(color::STATUS_WARNING),
                    );
                }
            });
        }
    });
    ui.add_space(space::TWO);
    response
}

const fn channel_initial(channel: ColorWheelChannel) -> &'static str {
    match channel {
        ColorWheelChannel::Master => "M",
        ColorWheelChannel::Red => "R",
        ColorWheelChannel::Green => "G",
        ColorWheelChannel::Blue => "B",
    }
}

const fn channel_color(channel: ColorWheelChannel) -> egui::Color32 {
    match channel {
        ColorWheelChannel::Master => color::TEXT_SECONDARY,
        ColorWheelChannel::Red => CHANNEL_COLORS[0],
        ColorWheelChannel::Green => CHANNEL_COLORS[1],
        ColorWheelChannel::Blue => CHANNEL_COLORS[2],
    }
}

/// Screen position to unit-disc coordinates, `v` up-positive and clamped to the
/// disc so a drag that leaves the ball saturates instead of exploding.
fn pointer_to_disc(rect: egui::Rect, pointer: egui::Pos2) -> (f64, f64) {
    let radius = rect.width().min(rect.height()) / 2.0;
    if radius <= 0.0 {
        return (0.0, 0.0);
    }
    let center = rect.center();
    let u = f64::from((pointer.x - center.x) / radius);
    let v = f64::from((center.y - pointer.y) / radius);
    clamp_to_disc(u, v)
}

#[allow(clippy::cast_possible_truncation)]
fn paint_ball(ui: &egui::Ui, rect: egui::Rect, state: &ColorWheelState) {
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let radius = rect.width().min(rect.height()) / 2.0 - 1.0;
    painter.circle_filled(center, radius, color::SURFACE);
    painter.circle_stroke(center, radius, egui::Stroke::new(1.0, color::BORDER_STRONG));
    painter.circle_stroke(
        center,
        radius * 0.5,
        egui::Stroke::new(1.0, color::BORDER_SUBTLE),
    );
    for (index, degrees) in CHANNEL_ANGLES_DEGREES.into_iter().enumerate() {
        let theta = degrees.to_radians();
        let direction = egui::vec2(theta.cos() as f32, -theta.sin() as f32);
        painter.line_segment(
            [
                center + direction * radius * TICK_INNER,
                center + direction * radius,
            ],
            egui::Stroke::new(2.0, CHANNEL_COLORS[index]),
        );
    }

    let (_, _, neutral) = state.control.bounds();
    let deltas = BALL_CHANNELS.map(|channel| state.values.channel(channel) - neutral);
    let (u, v) = channel_deltas_to_disc(deltas, control_scale(state.control));
    let indicator = egui::pos2(
        center.x + (u as f32) * radius,
        center.y - (v as f32) * radius,
    );
    painter.line_segment(
        [center, indicator],
        egui::Stroke::new(1.0, color::BORDER_STRONG),
    );
    painter.circle_filled(indicator, INDICATOR_RADIUS, color::ACCENT);
    painter.circle_stroke(
        indicator,
        INDICATOR_RADIUS,
        egui::Stroke::new(1.0, color::LETTERBOX),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CC3 §7: R sits at 90°, so straight up is pure red at full magnitude and
    /// exactly half of the negative lobe on green and blue.
    ///
    /// cos(90°) = 0 and sin(90°) = 1, so `delta_R = round(2000 * 1) = +2000`.
    /// cos(210°) = -√3/2 and sin(210°) = -1/2, so
    /// `delta_G = round(2000 * (0*-0.8660254 + 1*-0.5)) = -1000`.
    /// cos(330°) = +√3/2 and sin(330°) = -1/2, so `delta_B = -1000` likewise.
    #[test]
    fn straight_up_is_pure_red_for_lift() {
        assert_eq!(
            disc_to_channel_deltas(0.0, 1.0, 2_000.0),
            [2_000, -1_000, -1_000]
        );
        assert_eq!(
            disc_to_channel_deltas(0.0, -1.0, 2_000.0),
            [-2_000, 1_000, 1_000]
        );
    }

    /// Straight right is the green/blue axis: `cos(210°) = -0.8660254...` and
    /// `cos(330°) = +0.8660254...`, so `k = 1000` gives ∓866 and red stays 0.
    #[test]
    fn straight_right_splits_green_and_blue_by_root_three_over_two() {
        assert_eq!(disc_to_channel_deltas(1.0, 0.0, 1_000.0), [0, -866, 866]);
        assert_eq!(disc_to_channel_deltas(-1.0, 0.0, 1_000.0), [0, 866, -866]);
    }

    /// The origin is the neutral position for every control family.
    #[test]
    fn the_centre_of_the_disc_is_neutral() {
        for control in ColorWheelControl::ALL {
            let (_, _, neutral) = control.bounds();
            assert_eq!(
                disc_to_channel_deltas(0.0, 0.0, control_scale(control)),
                [0; 3]
            );
            assert_eq!(disc_channel_values(control, 0.0, 0.0), [neutral; 3]);
        }
    }

    /// CC3 §7 names `round_half_away_from_zero`, not banker's rounding: an
    /// exact half must land on ±1, never on 0.
    #[test]
    fn exact_halves_round_away_from_zero() {
        assert_eq!(disc_to_channel_deltas(0.0, 0.000_25, 2_000.0)[0], 1);
        assert_eq!(disc_to_channel_deltas(0.0, -0.000_25, 2_000.0)[0], -1);
        assert!((round_half_away_from_zero(2.5) - 3.0).abs() < f64::EPSILON);
        assert!((round_half_away_from_zero(-2.5) + 3.0).abs() < f64::EPSILON);
    }

    /// Deltas are added to the neutral and clamped to the descriptor bounds.
    /// Gamma's minimum is 100, so a full downward pull on red saturates there
    /// rather than reaching the raw `1000 - 1000 = 0`.
    #[test]
    fn values_are_clamped_to_descriptor_bounds() {
        let gamma = disc_channel_values(ColorWheelControl::Gamma, 0.0, -1.0);
        assert_eq!(gamma, [100, 1_500, 1_500]);
        let gain = disc_channel_values(ColorWheelControl::Gain, 0.0, -1.0);
        assert_eq!(gain, [0, 1_500, 1_500]);
        let lift = disc_channel_values(ColorWheelControl::Lift, 0.0, 1.0);
        assert_eq!(lift, [2_000, -1_000, -1_000]);
    }

    /// The indicator inverse is a least-squares inverse of the forward map, so
    /// a position inside the disc survives a round trip to within rounding.
    #[test]
    fn the_indicator_inverse_round_trips_a_disc_position() {
        for (u, v) in [(0.0, 0.0), (0.5, 0.25), (-0.3, 0.7), (0.0, -1.0)] {
            let deltas = disc_to_channel_deltas(u, v, 2_000.0);
            let (back_u, back_v) = channel_deltas_to_disc(deltas, 2_000.0);
            assert!(
                (back_u - u).abs() < 1e-3 && (back_v - v).abs() < 1e-3,
                "({u}, {v}) round-tripped to ({back_u}, {back_v})"
            );
        }
    }

    /// A drag that leaves the ball saturates on the rim instead of producing a
    /// projection longer than the control's maximum magnitude.
    #[test]
    fn positions_outside_the_disc_saturate_on_the_rim() {
        let (u, v) = clamp_to_disc(3.0, 4.0);
        assert!((u.hypot(v) - 1.0).abs() < 1e-12);
        assert!((u - 0.6).abs() < 1e-12 && (v - 0.8).abs() < 1e-12);
        let deltas = disc_to_channel_deltas(u, v, 2_000.0);
        assert!(deltas.iter().all(|delta| delta.abs() <= 2_000));
    }

    /// CC3 §7: `+0.0500` for lift, `1.200` for gamma and gain.
    #[test]
    fn read_outs_match_the_stored_integer_exactly() {
        assert_eq!(wheel_readout(ColorWheelControl::Lift, 500), "+0.0500");
        assert_eq!(wheel_readout(ColorWheelControl::Lift, -2_000), "-0.2000");
        assert_eq!(wheel_readout(ColorWheelControl::Lift, 0), "+0.0000");
        assert_eq!(wheel_readout(ColorWheelControl::Gamma, 1_200), "1.200");
        assert_eq!(wheel_readout(ColorWheelControl::Gain, 1_200), "1.200");
        assert_eq!(wheel_readout(ColorWheelControl::Gain, 0), "0.000");
    }

    #[test]
    fn control_tokens_are_stable_and_distinct() {
        let tokens: Vec<&str> = ColorWheelControl::ALL
            .into_iter()
            .map(control_token)
            .collect();
        assert_eq!(tokens, ["lift", "gamma", "gain"]);
    }
}
