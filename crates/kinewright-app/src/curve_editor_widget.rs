//! CC3 §7 curve editor.
//!
//! A square widget with the identity diagonal always drawn, the selected
//! curve, and its control points. Click on empty space adds a point (rejected
//! at 16); drag moves a point, clamped to `-2000..=12000` on both axes and to
//! at least one basis point of separation from its neighbours in `x`;
//! right-click or Delete removes a point (rejected at 2); double-click resets
//! the curve. The widget writes only integers.
//!
//! Every edit rule is a pure function over the point list so the clamps are
//! provable without a window. The widget never talks to the document: it
//! reports the point list a gesture asks for and `inspector_ui` turns that into
//! `{curve}_point_count` plus the active points' coordinates.

use eframe::egui;
use kinewright_core::{
    COLOR_CURVE_MAX_POINTS, COLOR_CURVE_MIN_POINTS, COLOR_CURVE_WHITE_BASIS_POINTS,
    ColorCurveChannel,
};

use crate::{
    inspector_ui::is_live_drag,
    theme::{self, color, radius, type_size},
};

/// Inclusive minimum of a curve coordinate, in basis points (CC3 §2.3).
pub(crate) const COORDINATE_MIN: i32 = -2_000;
/// Inclusive maximum of a curve coordinate, in basis points (CC3 §2.3).
pub(crate) const COORDINATE_MAX: i32 = 12_000;
/// The width of the editor's visible coordinate domain, in basis points.
const COORDINATE_SPAN: i32 = COORDINATE_MAX - COORDINATE_MIN;
/// The smallest legal separation between two neighbouring `x` coordinates.
///
/// `x` must be *strictly* increasing over the active prefix, and the stored
/// unit is an integer, so one basis point is the whole rule.
pub(crate) const MINIMUM_X_SEPARATION: i32 = 1;

const EDITOR_MIN_SIDE: f32 = 160.0;
const EDITOR_MAX_SIDE: f32 = 240.0;
/// How near the pointer must be, in points, to grab or delete a control point.
const HIT_RADIUS: f32 = 9.0;
const POINT_RADIUS: f32 = 3.5;
/// How long a rejected edit stays on screen, in seconds.
const REJECTION_SECONDS: f64 = 3.0;
/// Samples used to draw the curve. Display only; it never feeds a render.
const CURVE_SAMPLES: usize = 96;

/// Why the widget refused an edit before it could reach Core.
///
/// CC3 §6 makes Core the enforcement point; the editor's job is to never
/// produce an operation Core would have to reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CurveEditRejection {
    /// A curve already holds the maximum of 16 points.
    TooManyPoints,
    /// A curve already holds the minimum of 2 points.
    TooFewPoints,
    /// Neighbouring points leave no integer `x` for a new point.
    NoRoomBetweenNeighbours,
}

impl CurveEditRejection {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::TooManyPoints => "A curve holds at most 16 points.",
            Self::TooFewPoints => "A curve holds at least 2 points.",
            Self::NoRoomBetweenNeighbours => {
                "Neighbouring points leave no room for another point here."
            }
        }
    }
}

/// What one frame of curve interaction asks the document to become.
#[derive(Debug, Clone, Default)]
pub(crate) struct CurveEditorResponse {
    /// The new point list, when this frame changed it.
    pub(crate) points: Option<Vec<(i32, i32)>>,
    /// Whether this frame belongs to a live drag, so it coalesces into one
    /// undo entry.
    pub(crate) live: bool,
    /// Whether a drag began this frame, so a fresh gesture identity is opened.
    pub(crate) gesture_started: bool,
    /// Whether the widget was double-clicked: reset this curve.
    pub(crate) reset: bool,
}

/// Per-widget interaction state. Never serialized, never part of the document.
#[derive(Debug, Clone, Default)]
struct CurveEditorMemory {
    dragging: Option<usize>,
    selected: Option<usize>,
    rejection: Option<CurveEditRejection>,
    rejection_until: f64,
}

/// Clamp one coordinate into the CC3 §2.3 inclusive bounds.
#[must_use]
pub(crate) fn clamp_coordinate(value: i32) -> i32 {
    value.clamp(COORDINATE_MIN, COORDINATE_MAX)
}

/// The `x` a dragged point may take, kept strictly between its neighbours.
///
/// The first and last points are bounded only by the coordinate range, so a
/// curve can still be widened to the whole domain.
#[must_use]
pub(crate) fn constrain_dragged_x(points: &[(i32, i32)], index: usize, x: i32) -> i32 {
    let low = index
        .checked_sub(1)
        .and_then(|previous| points.get(previous))
        .map_or(COORDINATE_MIN, |point| {
            point.0.saturating_add(MINIMUM_X_SEPARATION)
        });
    let high = points.get(index + 1).map_or(COORDINATE_MAX, |point| {
        point.0.saturating_sub(MINIMUM_X_SEPARATION)
    });
    if low > high {
        // Unreachable on a valid curve: two neighbours are always at least two
        // basis points apart when a point sits between them.
        return low;
    }
    clamp_coordinate(x).clamp(low, high)
}

/// Move one point, clamping both axes and keeping `x` strictly increasing.
#[must_use]
pub(crate) fn move_point(
    points: &[(i32, i32)],
    index: usize,
    target: (i32, i32),
) -> Vec<(i32, i32)> {
    let mut moved = points.to_vec();
    if let Some(point) = moved.get_mut(index) {
        *point = (
            constrain_dragged_x(points, index, target.0),
            clamp_coordinate(target.1),
        );
    }
    moved
}

/// Insert one point, keeping the list sorted and strictly increasing in `x`.
///
/// The requested `x` is nudged into the gap between its neighbours rather than
/// rejected, so a click one basis point from an existing point still lands.
pub(crate) fn insert_point(
    points: &[(i32, i32)],
    point: (i32, i32),
) -> Result<(usize, Vec<(i32, i32)>), CurveEditRejection> {
    if points.len() >= COLOR_CURVE_MAX_POINTS {
        return Err(CurveEditRejection::TooManyPoints);
    }
    let x = clamp_coordinate(point.0);
    let index = points.partition_point(|existing| existing.0 < x);
    let low = index
        .checked_sub(1)
        .and_then(|previous| points.get(previous))
        .map_or(COORDINATE_MIN, |previous| {
            previous.0.saturating_add(MINIMUM_X_SEPARATION)
        });
    let high = points.get(index).map_or(COORDINATE_MAX, |next| {
        next.0.saturating_sub(MINIMUM_X_SEPARATION)
    });
    if low > high {
        return Err(CurveEditRejection::NoRoomBetweenNeighbours);
    }
    let mut inserted = points.to_vec();
    inserted.insert(index, (x.clamp(low, high), clamp_coordinate(point.1)));
    Ok((index, inserted))
}

/// Remove one point, refusing to drop below the minimum of two.
pub(crate) fn remove_point(
    points: &[(i32, i32)],
    index: usize,
) -> Result<Vec<(i32, i32)>, CurveEditRejection> {
    if points.len() <= COLOR_CURVE_MIN_POINTS {
        return Err(CurveEditRejection::TooFewPoints);
    }
    if index >= points.len() {
        return Ok(points.to_vec());
    }
    let mut remaining = points.to_vec();
    remaining.remove(index);
    Ok(remaining)
}

/// Map one curve coordinate pair to a pixel inside the widget.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn basis_to_pixel(rect: egui::Rect, point: (i32, i32)) -> egui::Pos2 {
    let span = COORDINATE_SPAN as f32;
    let x = (point.0 - COORDINATE_MIN) as f32 / span;
    let y = (point.1 - COORDINATE_MIN) as f32 / span;
    egui::pos2(
        rect.left() + x * rect.width(),
        rect.bottom() - y * rect.height(),
    )
}

/// Map a pixel back to a curve coordinate pair, clamped to the legal range.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn pixel_to_basis(rect: egui::Rect, position: egui::Pos2) -> (i32, i32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return (COORDINATE_MIN, COORDINATE_MIN);
    }
    let span = COORDINATE_SPAN as f32;
    let x = COORDINATE_MIN as f32 + (position.x - rect.left()) / rect.width() * span;
    let y = COORDINATE_MIN as f32 + (rect.bottom() - position.y) / rect.height() * span;
    (
        clamp_coordinate(x.round() as i32),
        clamp_coordinate(y.round() as i32),
    )
}

/// Solve the CC3 §2.3 Fritsch–Carlson tangents for display sampling.
///
/// This is a *display* implementation. The normative CPU reference lives in
/// `kinewright-media`, and the renderer's host solve lives in the compositor;
/// neither calls this, and this calls neither. A drawn curve that disagreed
/// with the render would be a bug in one of the three, which is exactly what
/// the CC3 §10 parity fixtures exist to catch.
#[allow(clippy::manual_midpoint)]
fn solve_tangents(points: &[(f64, f64)]) -> Vec<f64> {
    let count = points.len();
    if count < 2 {
        return vec![0.0; count];
    }
    let mut delta = Vec::with_capacity(count - 1);
    for window in points.windows(2) {
        let run = window[1].0 - window[0].0;
        delta.push(if run == 0.0 {
            0.0
        } else {
            (window[1].1 - window[0].1) / run
        });
    }
    let mut tangents = vec![0.0; count];
    tangents[0] = delta[0];
    tangents[count - 1] = delta[count - 2];
    for index in 1..count - 1 {
        // Transcribed literally from CC3 §2.3 step 2 rather than routed through
        // `f64::midpoint`, so the written contract is readable at the call site.
        tangents[index] = (delta[index - 1] + delta[index]) / 2.0;
    }
    for index in 0..count - 1 {
        if delta[index] == 0.0 {
            tangents[index] = 0.0;
            tangents[index + 1] = 0.0;
            continue;
        }
        let a = tangents[index] / delta[index];
        let b = tangents[index + 1] / delta[index];
        if a < 0.0 {
            tangents[index] = 0.0;
        }
        if b < 0.0 {
            tangents[index + 1] = 0.0;
        }
        if a >= 0.0 && b >= 0.0 && a * a + b * b > 9.0 {
            let tau = 3.0 / (a * a + b * b).sqrt();
            tangents[index] = tau * a * delta[index];
            tangents[index + 1] = tau * b * delta[index];
        }
    }
    tangents
}

fn evaluate(points: &[(f64, f64)], tangents: &[f64], x: f64) -> f64 {
    let last = points.len() - 1;
    if x < points[0].0 {
        return points[0].1 + tangents[0] * (x - points[0].0);
    }
    if x >= points[last].0 {
        return points[last].1 + tangents[last] * (x - points[last].0);
    }
    let mut segment = 0;
    for index in 0..last {
        if x >= points[index].0 && x < points[index + 1].0 {
            segment = index;
        }
    }
    let (x0, y0) = points[segment];
    let (x1, y1) = points[segment + 1];
    let h = x1 - x0;
    let t = (x - x0) / h;
    let t2 = t * t;
    let t3 = t2 * t;
    (2.0 * t3 - 3.0 * t2 + 1.0) * y0
        + (t3 - 2.0 * t2 + t) * h * tangents[segment]
        + (-2.0 * t3 + 3.0 * t2) * y1
        + (t3 - t2) * h * tangents[segment + 1]
}

/// Sample the curve across the visible domain, in basis points.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn sample_curve(points: &[(i32, i32)], samples: usize) -> Vec<(f64, f64)> {
    if points.len() < COLOR_CURVE_MIN_POINTS || samples < 2 {
        return Vec::new();
    }
    let float: Vec<(f64, f64)> = points
        .iter()
        .map(|point| (f64::from(point.0), f64::from(point.1)))
        .collect();
    let tangents = solve_tangents(&float);
    let span = f64::from(COORDINATE_SPAN);
    (0..samples)
        .map(|index| {
            let x = f64::from(COORDINATE_MIN) + span * index as f64 / (samples - 1) as f64;
            (x, evaluate(&float, &tangents, x))
        })
        .collect()
}

fn nearest_point(rect: egui::Rect, points: &[(i32, i32)], position: egui::Pos2) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| (index, basis_to_pixel(rect, *point).distance(position)))
        .filter(|(_, distance)| *distance <= HIT_RADIUS)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

/// Draw and drive one curve.
///
/// `points` is the curve's resolved active point list and `id` must be unique
/// per clip, effect, and curve so two editors never share drag state.
pub(crate) fn curve_editor(
    ui: &mut egui::Ui,
    points: &[(i32, i32)],
    curve: ColorCurveChannel,
    id: egui::Id,
) -> CurveEditorResponse {
    let mut response = CurveEditorResponse::default();
    let side = ui.available_width().clamp(EDITOR_MIN_SIDE, EDITOR_MAX_SIDE);
    let (rect, area) =
        ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click_and_drag());
    let mut memory = ui
        .data(|data| data.get_temp::<CurveEditorMemory>(id))
        .unwrap_or_default();
    let now = ui.input(|input| input.time);

    if area.double_clicked() {
        response.reset = true;
        memory.dragging = None;
        memory.selected = None;
    } else {
        if area.drag_started() {
            memory.dragging = area
                .interact_pointer_pos()
                .and_then(|position| nearest_point(rect, points, position));
            if memory.dragging.is_some() {
                memory.selected = memory.dragging;
                response.gesture_started = true;
            }
        }
        if is_live_drag(&area)
            && let Some(index) = memory.dragging
            && let Some(position) = area.interact_pointer_pos()
        {
            let moved = move_point(points, index, pixel_to_basis(rect, position));
            if moved.as_slice() != points {
                response.points = Some(moved);
            }
            // `is_live_drag` keeps the release frame inside the gesture: egui
            // reports it with `dragged() == false`, and dropping it here would
            // file the final point position as a second undo entry.
            response.live = true;
        }
        if area.drag_stopped() {
            memory.dragging = None;
        }
        if area.clicked()
            && let Some(position) = area.interact_pointer_pos()
        {
            match nearest_point(rect, points, position) {
                Some(index) => memory.selected = Some(index),
                None => match insert_point(points, pixel_to_basis(rect, position)) {
                    Ok((index, inserted)) => {
                        memory.selected = Some(index);
                        response.points = Some(inserted);
                    }
                    Err(rejection) => {
                        memory.rejection = Some(rejection);
                        memory.rejection_until = now + REJECTION_SECONDS;
                    }
                },
            }
        }
        if area.secondary_clicked()
            && let Some(index) = area
                .interact_pointer_pos()
                .and_then(|position| nearest_point(rect, points, position))
        {
            apply_removal(points, index, &mut memory, &mut response, now);
        }
        if area.hovered()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
            })
            && let Some(index) = memory.selected.filter(|index| *index < points.len())
        {
            apply_removal(points, index, &mut memory, &mut response, now);
        }
    }

    if memory.rejection.is_some() && now >= memory.rejection_until {
        memory.rejection = None;
    }
    paint(ui, rect, points, curve, memory.selected, memory.rejection);
    ui.data_mut(|data| data.insert_temp(id, memory));
    response
}

fn apply_removal(
    points: &[(i32, i32)],
    index: usize,
    memory: &mut CurveEditorMemory,
    response: &mut CurveEditorResponse,
    now: f64,
) {
    match remove_point(points, index) {
        Ok(remaining) => {
            memory.selected = None;
            memory.dragging = None;
            response.points = Some(remaining);
        }
        Err(rejection) => {
            memory.rejection = Some(rejection);
            memory.rejection_until = now + REJECTION_SECONDS;
        }
    }
}

fn paint(
    ui: &egui::Ui,
    rect: egui::Rect,
    points: &[(i32, i32)],
    curve: ColorCurveChannel,
    selected: Option<usize>,
    rejection: Option<CurveEditRejection>,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, radius::SM, color::LETTERBOX);
    theme::paint_inset_well(&painter, rect, radius::px(radius::SM));

    // The display-range box: (0, 0) to (10000, 10000) inside the -2000..=12000
    // domain, so a point placed below black or above white reads as such.
    let white = COLOR_CURVE_WHITE_BASIS_POINTS;
    let unit = egui::Rect::from_two_pos(
        basis_to_pixel(rect, (0, 0)),
        basis_to_pixel(rect, (white, white)),
    );
    painter.rect_stroke(
        unit,
        radius::NONE,
        egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        egui::StrokeKind::Inside,
    );
    // The identity diagonal is always drawn (CC3 §7).
    painter.line_segment(
        [
            basis_to_pixel(rect, (COORDINATE_MIN, COORDINATE_MIN)),
            basis_to_pixel(rect, (COORDINATE_MAX, COORDINATE_MAX)),
        ],
        egui::Stroke::new(1.0, color::BORDER_STRONG),
    );

    let stroke = egui::Stroke::new(1.6, curve_color(curve));
    let samples: Vec<egui::Pos2> = sample_curve(points, CURVE_SAMPLES)
        .into_iter()
        .map(|(x, y)| basis_to_pixel_f64(rect, x, y))
        .collect();
    if samples.len() >= 2 {
        painter.add(egui::Shape::line(samples, stroke));
    }
    for (index, point) in points.iter().enumerate() {
        let centre = basis_to_pixel(rect, *point);
        painter.circle_filled(centre, POINT_RADIUS, curve_color(curve));
        if selected == Some(index) {
            painter.circle_stroke(
                centre,
                POINT_RADIUS + 2.0,
                egui::Stroke::new(1.0, color::ACCENT),
            );
        }
    }
    if let Some(rejection) = rejection {
        painter.text(
            rect.left_bottom() + egui::vec2(4.0, -4.0),
            egui::Align2::LEFT_BOTTOM,
            rejection.message(),
            egui::FontId::proportional(type_size::MICRO),
            color::STATUS_WARNING,
        );
    }
}

#[allow(clippy::cast_possible_truncation)]
fn basis_to_pixel_f64(rect: egui::Rect, x: f64, y: f64) -> egui::Pos2 {
    let span = f64::from(COORDINATE_SPAN);
    let normalized_x = (x - f64::from(COORDINATE_MIN)) / span;
    let normalized_y = (y - f64::from(COORDINATE_MIN)) / span;
    egui::pos2(
        rect.left() + (normalized_x as f32) * rect.width(),
        rect.bottom() - (normalized_y as f32) * rect.height(),
    )
}

pub(crate) const fn curve_color(curve: ColorCurveChannel) -> egui::Color32 {
    match curve {
        ColorCurveChannel::Master => color::TEXT_PRIMARY,
        ColorCurveChannel::Red => egui::Color32::from_rgb(0xE0, 0x5A, 0x5A),
        ColorCurveChannel::Green => egui::Color32::from_rgb(0x5C, 0xC2, 0x76),
        ColorCurveChannel::Blue => egui::Color32::from_rgb(0x5A, 0x8C, 0xE0),
    }
}

/// The human label of one curve.
#[must_use]
pub(crate) const fn curve_label(curve: ColorCurveChannel) -> &'static str {
    match curve {
        ColorCurveChannel::Master => "Master",
        ColorCurveChannel::Red => "R",
        ColorCurveChannel::Green => "G",
        ColorCurveChannel::Blue => "B",
    }
}

#[cfg(test)]
mod tests {
    use kinewright_core::{COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN};

    use super::*;

    /// The structural identity curve: `(0, 0)` and `(10000, 10000)`.
    fn identity_points() -> Vec<(i32, i32)> {
        vec![
            (0, 0),
            (
                COLOR_CURVE_WHITE_BASIS_POINTS,
                COLOR_CURVE_WHITE_BASIS_POINTS,
            ),
        ]
    }

    /// The widget's local integer bounds must be the core contract's bounds.
    #[test]
    fn coordinate_bounds_match_the_core_contract() {
        assert_eq!(i64::from(COORDINATE_MIN), COLOR_CURVE_COORDINATE_MIN);
        assert_eq!(i64::from(COORDINATE_MAX), COLOR_CURVE_COORDINATE_MAX);
        assert_eq!(COLOR_CURVE_MIN_POINTS, 2);
        assert_eq!(COLOR_CURVE_MAX_POINTS, 16);
    }

    #[test]
    fn a_new_point_lands_in_x_order() {
        let points = identity_points();
        let (index, inserted) = insert_point(&points, (5_000, 6_000)).expect("room for a point");
        assert_eq!(index, 1);
        assert_eq!(inserted, [(0, 0), (5_000, 6_000), (10_000, 10_000)]);
        assert!(inserted.windows(2).all(|pair| pair[0].0 < pair[1].0));

        let (front, prepended) = insert_point(&points, (-2_000, -2_000)).expect("room below black");
        assert_eq!(front, 0);
        assert_eq!(prepended[0], (-2_000, -2_000));
    }

    /// Coordinates are clamped to `-2000..=12000` on both axes.
    #[test]
    fn inserted_coordinates_are_clamped_to_the_legal_range() {
        let (_, inserted) = insert_point(&identity_points(), (99_000, -99_000)).expect("clamped");
        assert!(inserted.contains(&(12_000, -2_000)));
        assert_eq!(clamp_coordinate(12_001), 12_000);
        assert_eq!(clamp_coordinate(-2_001), -2_000);
    }

    /// A point may not collide with a neighbour: one basis point is the whole
    /// separation rule, and the insert nudges rather than producing an `x` Core
    /// would reject.
    #[test]
    fn an_inserted_point_keeps_one_basis_point_of_separation() {
        let points = vec![(0, 0), (5_000, 5_000), (10_000, 10_000)];
        let (_, inserted) = insert_point(&points, (5_000, 9_000)).expect("nudged aside");
        assert!(inserted.windows(2).all(|pair| pair[1].0 - pair[0].0 >= 1));
        assert_eq!(inserted.len(), 4);

        // Two neighbours one basis point apart leave no integer between them.
        let packed = vec![(0, 0), (5_000, 5_000), (5_001, 5_100), (10_000, 10_000)];
        assert_eq!(
            insert_point(&packed, (5_001, 9_000)),
            Err(CurveEditRejection::NoRoomBetweenNeighbours)
        );
    }

    /// Sixteen points is the maximum (CC3 §2.3).
    #[test]
    fn a_seventeenth_point_is_rejected() {
        let full: Vec<(i32, i32)> = (0..16).map(|index| (index * 500, index * 500)).collect();
        assert_eq!(full.len(), COLOR_CURVE_MAX_POINTS);
        assert_eq!(
            insert_point(&full, (8_100, 4_000)),
            Err(CurveEditRejection::TooManyPoints)
        );
    }

    /// Two points is the minimum (CC3 §2.3).
    #[test]
    fn removing_below_two_points_is_rejected() {
        assert_eq!(
            remove_point(&identity_points(), 0),
            Err(CurveEditRejection::TooFewPoints)
        );
        let three = vec![(0, 0), (5_000, 6_000), (10_000, 10_000)];
        assert_eq!(remove_point(&three, 1), Ok(vec![(0, 0), (10_000, 10_000)]));
    }

    /// A dragged point stays strictly between its neighbours in `x` and inside
    /// the coordinate bounds on both axes.
    #[test]
    fn a_dragged_point_is_constrained_between_its_neighbours() {
        let points = vec![(0, 0), (5_000, 5_000), (10_000, 10_000)];
        assert_eq!(constrain_dragged_x(&points, 1, 99_000), 9_999);
        assert_eq!(constrain_dragged_x(&points, 1, -99_000), 1);
        assert_eq!(constrain_dragged_x(&points, 1, 10_000), 9_999);
        assert_eq!(constrain_dragged_x(&points, 1, 0), 1);
        // The ends are bounded only by the coordinate range.
        assert_eq!(constrain_dragged_x(&points, 0, -99_000), -2_000);
        assert_eq!(constrain_dragged_x(&points, 2, 99_000), 12_000);

        let moved = move_point(&points, 1, (99_000, 99_000));
        assert_eq!(moved[1], (9_999, 12_000));
        assert!(moved.windows(2).all(|pair| pair[0].0 < pair[1].0));

        let dragged_end = move_point(&points, 0, (-50_000, -50_000));
        assert_eq!(dragged_end[0], (-2_000, -2_000));
    }

    /// Dragging one point never disturbs another.
    #[test]
    fn moving_one_point_leaves_the_others_untouched() {
        let points = vec![(0, 0), (5_000, 5_000), (10_000, 10_000)];
        let moved = move_point(&points, 1, (6_000, 2_000));
        assert_eq!(moved[0], points[0]);
        assert_eq!(moved[2], points[2]);
        assert_eq!(moved[1], (6_000, 2_000));
    }

    /// The identity curve samples exactly on the diagonal, including the
    /// extrapolated tails outside `0..=10000`.
    #[test]
    fn the_identity_curve_samples_on_the_diagonal() {
        let samples = sample_curve(&identity_points(), 33);
        assert_eq!(samples.len(), 33);
        for (x, y) in samples {
            assert!((y - x).abs() < 1e-6, "identity sampled ({x}, {y})");
        }
    }

    /// A monotone point set yields a monotone curve — the CC3 §2.3 guarantee
    /// the Fritsch–Carlson limiting exists to provide.
    #[test]
    fn a_monotone_point_set_samples_monotonically() {
        let points = vec![
            (0, 0),
            (2_000, 400),
            (5_000, 5_000),
            (7_000, 9_400),
            (10_000, 10_000),
        ];
        let samples = sample_curve(&points, 200);
        for pair in samples.windows(2) {
            assert!(
                pair[1].1 >= pair[0].1 - 1e-9,
                "descending pair {:?} -> {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// Pixel mapping is the inverse of coordinate mapping to within one basis
    /// point of rounding, and the corners land on the corners.
    #[test]
    fn pixel_and_basis_mapping_round_trip() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(280.0, 280.0));
        assert_eq!(
            basis_to_pixel(rect, (COORDINATE_MIN, COORDINATE_MIN)),
            rect.left_bottom()
        );
        assert_eq!(
            basis_to_pixel(rect, (COORDINATE_MAX, COORDINATE_MAX)),
            rect.right_top()
        );
        for point in [(0, 0), (5_000, 6_000), (10_000, 10_000), (-2_000, 12_000)] {
            let back = pixel_to_basis(rect, basis_to_pixel(rect, point));
            assert!(
                (back.0 - point.0).abs() <= 2 && (back.1 - point.1).abs() <= 2,
                "{point:?} round-tripped to {back:?}"
            );
        }
        // Pixels outside the widget clamp instead of leaving the legal range.
        let outside = pixel_to_basis(rect, egui::pos2(-500.0, 900.0));
        assert_eq!(outside, (COORDINATE_MIN, COORDINATE_MIN));
    }

    #[test]
    fn curve_labels_cover_every_channel() {
        let labels: Vec<&str> = ColorCurveChannel::ALL
            .into_iter()
            .map(curve_label)
            .collect();
        assert_eq!(labels, ["Master", "R", "G", "B"]);
    }
}
