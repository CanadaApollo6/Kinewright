//! CC5 §6 matte overlay: window geometry, hit-testing, drag maths, and the
//! matte-view coverage source.
//!
//! Everything geometric in this module is a pure function of a
//! [`MatteWindowParams`] and a [`MatteFrame`] — the output raster aspect, the
//! letterboxed `image_rect` the preview draws the picture into, and the layer's
//! own resolved transform. That is deliberate: the overlay is the first
//! interactive surface on the viewer (CC5 §12), so the numbers a drag writes are
//! provable without a window.
//!
//! The maths transcribes CC5 §2.3 rather than re-deriving it. With
//! `hw = half_width_basis_points / 10000`, `hh = half_height_basis_points /
//! 10000`, `a` the raster aspect and `θ` the window rotation:
//!
//! ```text
//! d = ((u.x - cx) * a, (u.y - cy))
//! q = ( d.x·cosθ + d.y·sinθ , -d.x·sinθ + d.y·cosθ )
//! n = ( q.x / (hw·a) , q.y / hh )
//! ```
//!
//! `u` there is the **layer's** uv, because that is where the shader evaluates
//! the matte: the node stack runs on the layer quad's own interpolated uv, and
//! the `transform` effect only moves the quad. `image_rect` is the *composited*
//! output, so every conversion in this module passes through
//! [`LayerTransform`] (CC5 §5.2). The overlay walks the chain backwards — a
//! boundary point `n` becomes `q`, then `d`, then layer `uv`, then composite
//! `uv`, then a pixel inside `image_rect` — so an outline drawn here and a
//! coverage rendered by the compositor describe the same shape on a reframed
//! clip as on an untransformed one.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use eframe::egui::{self, Pos2, Rect, pos2, vec2};
use kinewright_core::{
    Analysis, ClipId, Document, EffectId, MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
    MATTE_WINDOW_CENTER_MIN_BASIS_POINTS, MATTE_WINDOW_HALF_EXTENT_MAX_BASIS_POINTS,
    MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS, MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES,
    MatteParams, MatteProof, MatteWindowParams, RgbaImage, TimeCode,
};

use crate::theme::color;

/// Pointer distance, in screen pixels, at which a handle is grabbed (CC5 §6).
pub(crate) const MATTE_HANDLE_RADIUS_PX: f32 = 8.0;

/// How far outside the top edge midpoint the rotation handle sits (CC5 §6).
pub(crate) const MATTE_ROTATION_HANDLE_OFFSET_PX: f32 = 24.0;

/// Segments in an ellipse outline. A closed polyline is what the painter can
/// stroke; 96 segments keeps the chord error below a tenth of a pixel on a
/// full-frame ellipse in a 1080-tall viewer.
const ELLIPSE_SEGMENTS: usize = 96;

/// Basis points per unit, the storage scale of every window control.
const BASIS_POINTS: f64 = 10_000.0;

/// Centidegrees per degree, the storage scale of the rotation control.
const CENTIDEGREES: f64 = 100.0;

/// One matte-carrying colour node, as the overlay and the scopes name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MatteTarget {
    pub(crate) clip: ClipId,
    pub(crate) effect: EffectId,
}

impl MatteTarget {
    pub(crate) const fn new(clip: ClipId, effect: EffectId) -> Self {
        Self { clip, effect }
    }
}

// ---------------------------------------------------------------------------
// Coverage source
// ---------------------------------------------------------------------------

/// The single blocking operation the matte view performs.
///
/// Modelled on the private `ScopeProofSource` in `color_scopes_ui.rs` for the
/// same reason: the panel's request policy stays testable without standing up
/// an `Analysis` backend, and `matte_proof_for_document` may still be
/// `NotImplemented` while the media half of CC5 lands.
pub(crate) trait MatteProofSource: Send + Sync + 'static {
    /// Render one node's coverage for an exact frame of an immutable document.
    ///
    /// # Errors
    ///
    /// Returns the backend's message when no coverage can be produced — the
    /// node is inactive, carries no matte, or the backend does not implement
    /// matte proofs yet.
    fn matte_proof(
        &self,
        document: Arc<Document>,
        at: TimeCode,
        clip: ClipId,
        effect: EffectId,
    ) -> Result<MatteProof, String>;
}

/// The production source: the live analysis backend's matte proof.
pub(crate) struct AnalysisMatteProofSource(pub(crate) Arc<dyn Analysis>);

impl MatteProofSource for AnalysisMatteProofSource {
    fn matte_proof(
        &self,
        document: Arc<Document>,
        at: TimeCode,
        clip: ClipId,
        effect: EffectId,
    ) -> Result<MatteProof, String> {
        self.0
            .matte_proof_for_document(document, at, clip, effect)
            .map_err(|error| error.to_string())
    }
}

// ---------------------------------------------------------------------------
// Geometry (CC5 §2.3, walked backwards)
// ---------------------------------------------------------------------------

/// One layer's resolved geometric transform, exactly as the compositor
/// accumulates it into `LayerParams` (CC5 §5.2).
///
/// `scale` is the product of every `transform.scale_percent / 100` on the
/// layer; `offset_x` and `offset_y` are the sums of every `x_percent` and
/// `y_percent` divided by 50 — `compositor.rs`'s
/// `EffectUniform::Scale | OffsetX | OffsetY` accumulation, transcribed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayerTransform {
    pub(crate) scale: f64,
    pub(crate) offset_x: f64,
    pub(crate) offset_y: f64,
}

impl LayerTransform {
    /// An unreframed layer: the layer's uv and the composite's uv agree.
    pub(crate) const IDENTITY: Self = Self {
        scale: 1.0,
        offset_x: 0.0,
        offset_y: 0.0,
    };

    /// Layer uv → composited-frame uv (CC5 §5.2).
    ///
    /// `compositor.wgsl`'s vertex stage places the quad at NDC
    /// `p = q·scale + (offset_x, −offset_y)` while mapping `v = (1 − ndc.y)/2`,
    /// so the downward-positive `v` axis already absorbs the shader's negation
    /// and **both** axes carry the same sign here. A positive `y_percent` moves
    /// the picture down the viewer, which is what this says.
    fn layer_to_composite(self, layer: (f64, f64)) -> (f64, f64) {
        (
            (layer.0 - 0.5).mul_add(self.scale, self.offset_x / 2.0) + 0.5,
            (layer.1 - 0.5).mul_add(self.scale, self.offset_y / 2.0) + 0.5,
        )
    }

    /// Composited-frame uv → layer uv: the exact inverse of
    /// [`Self::layer_to_composite`].
    ///
    /// A non-positive scale has no inverse. The descriptor bounds
    /// `scale_percent` at 1, so the compositor cannot produce one; returning
    /// the composite unchanged keeps a hostile document out of `NaN`
    /// arithmetic instead of poisoning every hit test.
    fn composite_to_layer(self, composite: (f64, f64)) -> (f64, f64) {
        if self.scale <= 0.0 {
            return composite;
        }
        (
            (composite.0 - 0.5 - self.offset_x / 2.0) / self.scale + 0.5,
            (composite.1 - 0.5 - self.offset_y / 2.0) / self.scale + 0.5,
        )
    }
}

/// Where one layer's matte lands on screen.
///
/// Bundled rather than passed as three arguments because every geometric
/// function in this module needs all three, and an overlay that knew the
/// letterbox but not the layer transform is exactly the bug this type exists to
/// make unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MatteFrame {
    /// The output raster aspect `a = W / H` (CC5 §2.3).
    pub(crate) aspect: f64,
    /// The letterboxed rectangle the composited picture is drawn into.
    pub(crate) image_rect: Rect,
    /// The layer's own transform between its uv and the composite's uv.
    pub(crate) transform: LayerTransform,
}

impl MatteFrame {
    pub(crate) const fn new(aspect: f64, image_rect: Rect, transform: LayerTransform) -> Self {
        Self {
            aspect,
            image_rect,
            transform,
        }
    }
}

/// One window's transform between its normalized field and screen pixels.
#[derive(Debug, Clone, Copy)]
struct WindowGeometry {
    centre_x: f64,
    centre_y: f64,
    half_width: f64,
    half_height: f64,
    cos_theta: f64,
    sin_theta: f64,
    aspect: f64,
    transform: LayerTransform,
    origin: Pos2,
    width: f64,
    height: f64,
    ellipse: bool,
}

impl WindowGeometry {
    #[allow(clippy::cast_precision_loss)]
    fn new(window: &MatteWindowParams, frame: MatteFrame) -> Self {
        let theta = (window.rotation_cd as f64 / CENTIDEGREES).to_radians();
        Self {
            centre_x: window.center_x_bp as f64 / BASIS_POINTS,
            centre_y: window.center_y_bp as f64 / BASIS_POINTS,
            half_width: window.half_width_bp as f64 / BASIS_POINTS,
            half_height: window.half_height_bp as f64 / BASIS_POINTS,
            cos_theta: theta.cos(),
            sin_theta: theta.sin(),
            aspect: frame.aspect,
            transform: frame.transform,
            origin: frame.image_rect.min,
            width: f64::from(frame.image_rect.width()),
            height: f64::from(frame.image_rect.height()),
            ellipse: window.is_ellipse(),
        }
    }

    /// The pixel position of a point in the window's normalized field.
    ///
    /// `n = (0, 0)` is the centre and `|n| = 1` is the boundary, so this is the
    /// inverse of the §2.3 chain — followed by the layer → composite
    /// conversion, because `image_rect` holds the composited output and the
    /// window is stored in the layer's own uv.
    #[allow(clippy::cast_possible_truncation)]
    fn point(&self, n: (f64, f64)) -> Pos2 {
        let q = (n.0 * self.half_width * self.aspect, n.1 * self.half_height);
        let d = (
            q.0 * self.cos_theta - q.1 * self.sin_theta,
            q.0 * self.sin_theta + q.1 * self.cos_theta,
        );
        let layer = (self.centre_x + d.0 / self.aspect, self.centre_y + d.1);
        let u = self.transform.layer_to_composite(layer);
        pos2(
            self.origin.x + (u.0 * self.width) as f32,
            self.origin.y + (u.1 * self.height) as f32,
        )
    }

    /// The **layer** uv coordinate of a screen position, inverting the
    /// letterbox and then the layer transform.
    fn uv(&self, pointer: Pos2) -> (f64, f64) {
        let composite = (
            f64::from(pointer.x - self.origin.x) / self.width,
            f64::from(pointer.y - self.origin.y) / self.height,
        );
        self.transform.composite_to_layer(composite)
    }

    /// The aspect-corrected offset `d` of a screen position from the window
    /// centre, before the window's own rotation.
    fn field_delta(&self, pointer: Pos2) -> (f64, f64) {
        let u = self.uv(pointer);
        ((u.0 - self.centre_x) * self.aspect, u.1 - self.centre_y)
    }

    /// The rotated field coordinate `q` of a screen position.
    fn field_offset(&self, pointer: Pos2) -> (f64, f64) {
        let d = self.field_delta(pointer);
        (
            d.0 * self.cos_theta + d.1 * self.sin_theta,
            -d.0 * self.sin_theta + d.1 * self.cos_theta,
        )
    }

    /// The §2.3 distance field `D` at a screen position.
    ///
    /// A degenerate half-extent — unreachable through the edit path, since the
    /// descriptor minimum is `1` — reports infinity rather than `NaN`, which
    /// mirrors the shader's defensive `w = 0`.
    fn distance(&self, pointer: Pos2) -> f64 {
        if self.half_width <= 0.0 || self.half_height <= 0.0 {
            return f64::INFINITY;
        }
        let q = self.field_offset(pointer);
        let n = (
            q.0 / (self.half_width * self.aspect),
            q.1 / self.half_height,
        );
        if self.ellipse {
            n.0.hypot(n.1)
        } else {
            n.0.abs().max(n.1.abs())
        }
    }
}

/// The eight resize handles, in `n`-space, in the order [`MatteHandle::ALL`]
/// lists them.
const HANDLE_FIELD_POINTS: [(f64, f64); 8] = [
    (-1.0, -1.0),
    (0.0, -1.0),
    (1.0, -1.0),
    (1.0, 0.0),
    (1.0, 1.0),
    (0.0, 1.0),
    (-1.0, 1.0),
    (-1.0, 0.0),
];

/// One of the eight edge or corner handles of a window (CC5 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatteHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl MatteHandle {
    pub(crate) const ALL: [Self; 8] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
    ];

    /// Whether this handle resizes the window's half width.
    #[must_use]
    pub(crate) const fn drives_width(self) -> bool {
        !matches!(self, Self::Top | Self::Bottom)
    }

    /// Whether this handle resizes the window's half height.
    #[must_use]
    pub(crate) const fn drives_height(self) -> bool {
        !matches!(self, Self::Left | Self::Right)
    }
}

/// What the pointer grabbed (CC5 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatteHit {
    /// The centre handle, or anywhere inside the window.
    Move,
    /// One of the eight edge or corner handles.
    Resize(MatteHandle),
    /// The rotation handle above the top edge midpoint.
    Rotate,
}

/// The closed outline of one window at its boundary, `D = 1`.
///
/// A rect returns its four corners in `n`-space order
/// `(-1,-1) → (1,-1) → (1,1) → (-1,1)`; an ellipse returns
/// [`ELLIPSE_SEGMENTS`] points. Both are closed by the caller, so no point is
/// repeated.
#[must_use]
pub(crate) fn window_outline_points(window: &MatteWindowParams, frame: MatteFrame) -> Vec<Pos2> {
    outline_points_at(window, frame, 1.0)
}

/// The two feather outlines, at `D = 1 − f` and `D = 1 + f` (CC5 §2.3).
///
/// `None` when the window is unfeathered: `f = 0` takes the hard branch, where
/// the band and the boundary are the same line and a second outline would draw
/// a lie.
#[must_use]
pub(crate) fn feather_outline_points(
    window: &MatteWindowParams,
    frame: MatteFrame,
) -> Option<(Vec<Pos2>, Vec<Pos2>)> {
    if window.feather_bp <= 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let feather = window.feather_bp as f64 / BASIS_POINTS;
    Some((
        outline_points_at(window, frame, 1.0 - feather),
        outline_points_at(window, frame, 1.0 + feather),
    ))
}

/// The outline of the level set `D = distance`.
fn outline_points_at(window: &MatteWindowParams, frame: MatteFrame, distance: f64) -> Vec<Pos2> {
    let geometry = WindowGeometry::new(window, frame);
    if distance <= 0.0 {
        return vec![geometry.point((0.0, 0.0))];
    }
    if window.is_ellipse() {
        #[allow(clippy::cast_precision_loss)]
        return (0..ELLIPSE_SEGMENTS)
            .map(|step| {
                let phi = std::f64::consts::TAU * step as f64 / ELLIPSE_SEGMENTS as f64;
                geometry.point((distance * phi.cos(), distance * phi.sin()))
            })
            .collect();
    }
    let corners: [(f64, f64); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    corners
        .into_iter()
        .map(|(x, y)| geometry.point((x * distance, y * distance)))
        .collect()
}

/// The eight edge and corner handle positions, in [`MatteHandle::ALL`] order.
#[must_use]
pub(crate) fn window_handle_points(window: &MatteWindowParams, frame: MatteFrame) -> [Pos2; 8] {
    let geometry = WindowGeometry::new(window, frame);
    HANDLE_FIELD_POINTS.map(|n| geometry.point(n))
}

/// The window centre in screen pixels.
#[must_use]
pub(crate) fn window_centre_point(window: &MatteWindowParams, frame: MatteFrame) -> Pos2 {
    WindowGeometry::new(window, frame).point((0.0, 0.0))
}

/// The rotation handle: [`MATTE_ROTATION_HANDLE_OFFSET_PX`] outside the top
/// edge midpoint, along the window's own "up" (CC5 §6).
///
/// The direction is measured from the transform itself rather than rebuilt
/// from `θ`, so the handle can never disagree with the outline about which way
/// the window is facing.
#[must_use]
pub(crate) fn rotation_handle_point(window: &MatteWindowParams, frame: MatteFrame) -> Pos2 {
    let geometry = WindowGeometry::new(window, frame);
    let centre = geometry.point((0.0, 0.0));
    let top = geometry.point((0.0, -1.0));
    // The *unnormalized* separation is what can be degenerate, and testing it
    // is simply the clearer statement of the condition: "this window has no
    // measurable up direction". (`emath`'s `normalized` returns `self` for a
    // zero-length vector, so testing the normalized length would answer the
    // same question — it just states it less directly.)
    let up = top - centre;
    if up.length() <= f32::EPSILON {
        return top;
    }
    top + up.normalized() * MATTE_ROTATION_HANDLE_OFFSET_PX
}

/// What a pointer at `pointer` would grab on this window (CC5 §6).
///
/// Priority is rotation, then the eight resize handles, then move. The
/// contract lists move first, but the edge handles sit exactly on the boundary
/// of the "inside the window" region: testing move first would make every
/// handle unreachable, so the more specific target wins and the broad
/// inside-the-window region is the fallback.
///
/// `selected` is what the overlay drew: handles and the rotation arm exist only
/// on the selected window, so an unselected one offers `Move` and nothing else.
/// Select-then-edit — hit-testing an affordance that was never painted is how a
/// four-window matte resizes the wrong window under an invisible handle.
#[must_use]
pub(crate) fn hit_test(
    pointer: Pos2,
    window: &MatteWindowParams,
    frame: MatteFrame,
    selected: bool,
) -> Option<MatteHit> {
    let geometry = WindowGeometry::new(window, frame);
    if selected {
        if within_handle(pointer, rotation_handle_point(window, frame)) {
            return Some(MatteHit::Rotate);
        }
        for (handle, point) in MatteHandle::ALL
            .into_iter()
            .zip(window_handle_points(window, frame))
        {
            if within_handle(pointer, point) {
                return Some(MatteHit::Resize(handle));
            }
        }
    }
    if within_handle(pointer, geometry.point((0.0, 0.0))) || geometry.distance(pointer) <= 1.0 {
        return Some(MatteHit::Move);
    }
    None
}

fn within_handle(pointer: Pos2, handle: Pos2) -> bool {
    pointer.distance(handle) <= MATTE_HANDLE_RADIUS_PX
}

/// The window a pointer grabbed on a whole matte, and what it grabbed.
///
/// The selected window is tested first so a window drawn under another stays
/// reachable, and `selected` is the *clamped* selection — the one
/// [`paint_matte_overlay`] drew handles for. Passing the raw stored index would
/// let a selection that outlived its window silently degrade every hit to
/// `Move`, because the window the user sees handles on would be tested as
/// unselected.
#[must_use]
pub(crate) fn matte_hit_test(
    pointer: Pos2,
    matte: &MatteParams,
    frame: MatteFrame,
    selected: Option<usize>,
) -> Option<(usize, MatteHit)> {
    let order = selected.into_iter().chain(0..matte.window_count);
    for index in order {
        if index >= matte.window_count {
            continue;
        }
        if let Some(hit) = hit_test(
            pointer,
            &matte.windows[index],
            frame,
            Some(index) == selected,
        ) {
            return Some((index, hit));
        }
    }
    None
}

/// One live overlay gesture: what it grabbed, on which window, and the stored
/// integers it started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MatteDrag {
    pub(crate) target: MatteTarget,
    pub(crate) window: usize,
    pub(crate) hit: MatteHit,
    pub(crate) start: MatteWindowParams,
    pub(crate) start_pointer: Pos2,
}

/// Round half away from zero, the CC3 §7 / CC5 §6 rule.
///
/// `f64::round` is exactly that rule; it is named here so the contract is
/// visible at the call site and cannot silently become banker's rounding.
fn round_half_away_from_zero(value: f64) -> f64 {
    value.round()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn to_basis_points(value: f64, min: i64, max: i64) -> i64 {
    round_half_away_from_zero(value * BASIS_POINTS).clamp(min as f64, max as f64) as i64
}

/// The stored integers one frame of a drag asks window `drag.window` to become.
///
/// Pure: it reads the pointer, the gesture's start state, and the letterbox
/// transform, and nothing else. Every result is rounded half away from zero and
/// clamped to the CC5 §2.2 descriptor bounds, so an off-frame drag produces a
/// legal off-frame centre (`-10000..=20000`) rather than a rejected operation.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn drag_to_params(
    drag: &MatteDrag,
    pointer: Pos2,
    frame: MatteFrame,
) -> MatteWindowParams {
    let aspect = frame.aspect;
    let geometry = WindowGeometry::new(&drag.start, frame);
    let mut next = drag.start;
    match drag.hit {
        MatteHit::Move => {
            let from = geometry.uv(drag.start_pointer);
            let to = geometry.uv(pointer);
            next.center_x_bp = shift_basis_points(drag.start.center_x_bp, to.0 - from.0);
            next.center_y_bp = shift_basis_points(drag.start.center_y_bp, to.1 - from.1);
        }
        MatteHit::Resize(handle) => {
            let q = geometry.field_offset(pointer);
            if handle.drives_width() {
                next.half_width_bp = to_basis_points(
                    (q.0 / aspect).abs(),
                    MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS,
                    MATTE_WINDOW_HALF_EXTENT_MAX_BASIS_POINTS,
                );
            }
            if handle.drives_height() {
                next.half_height_bp = to_basis_points(
                    q.1.abs(),
                    MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS,
                    MATTE_WINDOW_HALF_EXTENT_MAX_BASIS_POINTS,
                );
            }
        }
        MatteHit::Rotate => {
            // Measured in the window's own aspect-corrected field `d`, not in
            // screen pixels: θ is defined in that field (CC5 §2.3), so a
            // viewer whose `image_rect` aspect differs from the document's —
            // or a scaled layer — would otherwise write a sheared angle.
            let d = geometry.field_delta(pointer);
            if d.0.hypot(d.1) > f64::EPSILON {
                // `θ = 0` points the window's own up at the top of the frame,
                // and θ grows clockwise as the viewer sees it (CC5 §2.2), so
                // the angle of the grab is `atan2(d.x, -d.y)`.
                let degrees = d.0.atan2(-d.1).to_degrees();
                #[allow(clippy::cast_possible_truncation)]
                let centidegrees = round_half_away_from_zero(degrees * CENTIDEGREES).clamp(
                    -MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES as f64,
                    MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES as f64,
                ) as i64;
                next.rotation_cd = centidegrees;
            }
        }
    }
    next
}

/// Move one centre by a uv delta, in stored basis points.
#[allow(clippy::cast_possible_truncation)]
fn shift_basis_points(start: i64, delta_uv: f64) -> i64 {
    let delta = round_half_away_from_zero(delta_uv * BASIS_POINTS) as i64;
    start.saturating_add(delta).clamp(
        MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
        MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
    )
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// Draw every active window of one matte through the letterbox transform.
///
/// The selected window carries its handles and its rotation arm; the others are
/// drawn dimmed, so a four-window matte stays readable. Feathered windows show
/// the band as two dashed outlines at `D = 1 ± f`.
pub(crate) fn paint_matte_overlay(
    painter: &egui::Painter,
    frame: MatteFrame,
    matte: &MatteParams,
    state: &MatteOverlayState,
) {
    // No extra clip: the caller's painter is already bounded by the viewer
    // frame, and clipping to `image_rect` would cut the rotation arm off
    // whenever a window sits against the top of the picture.
    let selected_window = state.selected_window(matte.window_count);
    for (index, window) in matte.active_windows().enumerate() {
        let selected = Some(index) == selected_window;
        let stroke = egui::Stroke::new(
            if selected { 1.6 } else { 1.0 },
            if selected {
                color::ACCENT
            } else {
                color::BORDER_STRONG
            },
        );
        let outline = window_outline_points(window, frame);
        painter.add(egui::Shape::closed_line(outline, stroke));
        if let Some((inner, outer)) = feather_outline_points(window, frame) {
            let band = egui::Stroke::new(1.0, color::STATUS_WARNING);
            for mut points in [inner, outer] {
                if let Some(first) = points.first().copied() {
                    points.push(first);
                }
                painter.extend(egui::Shape::dashed_line(&points, band, 4.0, 4.0));
            }
        }
        if !selected {
            continue;
        }
        let centre = window_centre_point(window, frame);
        painter.circle_stroke(centre, 3.0, stroke);
        for handle in window_handle_points(window, frame) {
            painter.rect_filled(
                Rect::from_center_size(handle, vec2(6.0, 6.0)),
                0.0,
                color::ACCENT,
            );
        }
        let rotation = rotation_handle_point(window, frame);
        let top = window_geometry_top(window, frame);
        painter.line_segment([top, rotation], stroke);
        painter.circle_stroke(rotation, 4.0, stroke);
    }
}

fn window_geometry_top(window: &MatteWindowParams, frame: MatteFrame) -> Pos2 {
    WindowGeometry::new(window, frame).point((0.0, -1.0))
}

// ---------------------------------------------------------------------------
// Overlay state and the matte-view worker
// ---------------------------------------------------------------------------

/// The identity of one matte-view coverage render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MatteViewKey {
    pub(crate) session_id: u64,
    pub(crate) revision: u64,
    pub(crate) frame: TimeCode,
    pub(crate) target: MatteTarget,
}

/// What the matte view can show, as a typed state rather than a bare option.
///
/// `Unavailable` is a first-class outcome: `Analysis::matte_proof_for_document`
/// defaults to `NotImplemented`, so until the media half of CC5 lands the
/// toggle must say so instead of showing an empty frame (CC5 §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MatteViewStatus {
    /// The toggle is off.
    Off,
    /// A coverage render is in flight.
    Pending,
    /// Coverage is ready for the requested frame.
    Ready,
    /// The backend refused, with its typed message.
    Unavailable(String),
}

struct MatteViewResponse {
    generation: u64,
    key: MatteViewKey,
    result: Result<MatteProof, String>,
}

struct MatteViewRequest {
    generation: u64,
    key: MatteViewKey,
    source: Arc<dyn MatteProofSource>,
    document: Arc<Document>,
}

struct MatteViewWorker {
    cancelled: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

/// Every piece of ephemeral matte-overlay state.
///
/// None of it is part of the document: the selected window, the matte-view
/// toggle, and the live drag are view state, and the only thing that ever
/// reaches the project is the batch of `SetEffectParam`s a gesture produces.
pub(crate) struct MatteOverlayState {
    /// The card that reported an expanded matte section. The inspector renders
    /// after the viewer, so the viewer reads the previous frame's report and
    /// clears it; a card that stops rendering therefore stops the overlay on
    /// the next frame, exactly like the A/B hold mirror (CC4 §7).
    expanded: Option<MatteTarget>,
    /// Whether a card reported during the frame now being built. A frame in
    /// which no card reported at all — the inspector collapsed, the clip
    /// deselected — expires the report at the end of the frame.
    reported: bool,
    /// The window the overlay draws handles on and a click selects.
    ///
    /// Private, and read only through [`Self::selected_window`]: the count it
    /// indexes lives in the document, which can drop a window under it.
    selected_window: usize,
    /// Whether the viewer shows coverage instead of the picture (CC5 §6).
    matte_view: bool,
    drag: Option<MatteDrag>,
    coverage: Option<(MatteViewKey, RgbaImage)>,
    texture: Option<(MatteViewKey, egui::TextureHandle)>,
    error: Option<String>,
    /// The most recent key a render was requested for, so a sticky refusal is
    /// attributable to the frame identity it refused.
    last_key: Option<MatteViewKey>,
    pending: Option<(u64, MatteViewKey)>,
    active: Option<MatteViewWorker>,
    queued: Option<MatteViewRequest>,
    generation: u64,
    response_tx: mpsc::Sender<MatteViewResponse>,
    response_rx: mpsc::Receiver<MatteViewResponse>,
    #[cfg(test)]
    spawned_workers: u64,
}

impl std::fmt::Debug for MatteOverlayState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MatteOverlayState")
            .field("expanded", &self.expanded)
            .field("selected_window", &self.selected_window)
            .field("matte_view", &self.matte_view)
            .field("drag", &self.drag)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Default for MatteOverlayState {
    fn default() -> Self {
        let (response_tx, response_rx) = mpsc::channel();
        Self {
            expanded: None,
            reported: false,
            selected_window: 0,
            matte_view: false,
            drag: None,
            coverage: None,
            texture: None,
            error: None,
            last_key: None,
            pending: None,
            active: None,
            queued: None,
            generation: 0,
            response_tx,
            response_rx,
            #[cfg(test)]
            spawned_workers: 0,
        }
    }
}

impl MatteOverlayState {
    /// Record that a card drew an expanded matte section this frame.
    ///
    /// Only an expansion is reported. A collapsed section says nothing and is
    /// retired by [`Self::expire_unreported`] at the end of the frame, so a
    /// panel that produces edits without drawing the card — the viewer's own
    /// overlay drags — cannot clear the report it is acting on.
    pub(crate) fn report_expanded(&mut self, target: MatteTarget) {
        self.reported = true;
        self.set_expanded(Some(target));
    }

    fn set_expanded(&mut self, target: Option<MatteTarget>) {
        if self.expanded.is_some() && self.expanded != target {
            // A different node's section opened: its window selection and any
            // coverage belong to the node that just closed.
            self.selected_window = 0;
            self.invalidate_view();
        }
        self.expanded = target;
        if target.is_none() {
            self.matte_view = false;
            self.drag = None;
        }
    }

    /// The node whose matte section is expanded, if any.
    #[must_use]
    pub(crate) const fn expanded(&self) -> Option<MatteTarget> {
        self.expanded
    }

    /// End the frame: a report that nobody restated expires.
    ///
    /// The inspector renders after the monitor dock, so the viewer and the
    /// scopes read the previous frame's report. Expiring it here — rather than
    /// clearing it before the panels run — is what makes a card that stops
    /// rendering return the viewer to hover-only input on the next frame,
    /// instead of leaving it capturing drags for a node nobody is editing
    /// (CC5 §6, §12).
    pub(crate) fn expire_unreported(&mut self) {
        if !self.reported {
            self.set_expanded(None);
        }
        self.reported = false;
    }

    #[must_use]
    pub(crate) const fn matte_view(&self) -> bool {
        self.matte_view
    }

    pub(crate) fn set_matte_view(&mut self, enabled: bool) {
        if self.matte_view == enabled {
            return;
        }
        self.matte_view = enabled;
        if !enabled {
            self.invalidate_view();
        }
    }

    /// The window handles are drawn on and edited, clamped to what exists.
    ///
    /// `window_count` is the document's, this frame. Removing the selected
    /// window leaves the stored index past the end — nothing else moves it —
    /// and an out-of-range selection would paint no handles at all and make
    /// every hit a `Move` until the user happened to click a window (CC5 §6).
    #[must_use]
    pub(crate) const fn selected_window(&self, window_count: usize) -> Option<usize> {
        if window_count == 0 {
            return None;
        }
        Some(if self.selected_window >= window_count {
            window_count - 1
        } else {
            self.selected_window
        })
    }

    /// Select a window, clamped to the `window_count` the caller can see.
    pub(crate) fn select_window(&mut self, window: usize, window_count: usize) {
        self.selected_window = window.min(window_count.saturating_sub(1));
    }

    #[must_use]
    pub(crate) const fn drag(&self) -> Option<MatteDrag> {
        self.drag
    }

    pub(crate) fn begin_drag(&mut self, drag: MatteDrag) {
        self.selected_window = drag.window;
        self.drag = Some(drag);
    }

    pub(crate) fn end_drag(&mut self) {
        self.drag = None;
    }

    /// The coverage raster for `key`, when one has been rendered.
    #[must_use]
    pub(crate) fn coverage_for(&self, key: MatteViewKey) -> Option<&RgbaImage> {
        self.coverage
            .as_ref()
            .filter(|(stored, _)| *stored == key)
            .map(|(_, image)| image)
    }

    /// The cached texture for `key`, when it was built from that coverage.
    #[must_use]
    pub(crate) fn texture_for(&self, key: MatteViewKey) -> Option<&egui::TextureHandle> {
        self.texture
            .as_ref()
            .filter(|(stored, _)| *stored == key)
            .map(|(_, texture)| texture)
    }

    pub(crate) fn set_texture(&mut self, key: MatteViewKey, texture: egui::TextureHandle) {
        self.texture = Some((key, texture));
    }

    /// What the matte view can show for `key`.
    #[must_use]
    pub(crate) fn view_status(&self, key: MatteViewKey) -> MatteViewStatus {
        if !self.matte_view {
            return MatteViewStatus::Off;
        }
        if let Some(message) = &self.error {
            return MatteViewStatus::Unavailable(message.clone());
        }
        if self.coverage_for(key).is_some() {
            return MatteViewStatus::Ready;
        }
        MatteViewStatus::Pending
    }

    /// Whether `key` still needs a coverage render.
    #[must_use]
    pub(crate) fn needs_view(&self, key: MatteViewKey) -> bool {
        if !self.matte_view {
            return false;
        }
        if self.pending.is_some_and(|(_, pending)| pending == key) {
            return false;
        }
        if self.coverage_for(key).is_some() {
            return false;
        }
        // A refusal is sticky for the key it refused, so a `NotImplemented`
        // backend is asked once per frame identity instead of every repaint.
        !self.error_matches(key)
    }

    fn error_matches(&self, key: MatteViewKey) -> bool {
        self.error.is_some() && self.pending.is_none() && self.last_key == Some(key)
    }

    /// Ask for the coverage render `key` needs, if it needs one and may have
    /// one. Returns whether a render was started.
    ///
    /// `blocked` is the Program viewer's source-verification block, and it
    /// withholds the *render*, not merely its picture. The proof worker owns
    /// its own `FrameRenderer` and decoder, outside the visual cache's block
    /// path, so a request made while the source is Offline, Changed, or
    /// Unreadable decodes exactly the media the block exists to keep off the
    /// screen — [`viewer_picture`] would then discard the result, having paid
    /// for it (CC5 §4.1).
    ///
    /// [`viewer_picture`]: crate::preview_ui
    pub(crate) fn request_view_if_needed(
        &mut self,
        blocked: bool,
        source: Arc<dyn MatteProofSource>,
        document: Arc<Document>,
        key: MatteViewKey,
    ) -> bool {
        if blocked || !self.needs_view(key) {
            return false;
        }
        self.request_view(source, document, key);
        true
    }

    /// Ask for one coverage render, latest request wins.
    ///
    /// Single-flight, exactly like the scope panel: a request that arrives
    /// while a render is running parks in `queued` rather than starting a
    /// second `FrameRenderer` beside it.
    pub(crate) fn request_view(
        &mut self,
        source: Arc<dyn MatteProofSource>,
        document: Arc<Document>,
        key: MatteViewKey,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = Some((generation, key));
        self.last_key = Some(key);
        self.error = None;
        let request = MatteViewRequest {
            generation,
            key,
            source,
            document,
        };
        if self
            .active
            .as_ref()
            .is_some_and(|worker| !worker.handle.is_finished())
        {
            if let Some(worker) = self.active.as_ref() {
                worker.cancelled.store(true, Ordering::Release);
            }
            self.queued = Some(request);
            return;
        }
        self.reap_finished_worker();
        self.spawn(request);
    }

    fn spawn(&mut self, request: MatteViewRequest) {
        let MatteViewRequest {
            generation,
            key,
            source,
            document,
        } = request;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let response_tx = self.response_tx.clone();
        let spawn_result = thread::Builder::new()
            .name("kinewright-matte-proof".to_owned())
            .spawn(move || {
                if worker_cancelled.load(Ordering::Acquire) {
                    return;
                }
                let result =
                    source.matte_proof(document, key.frame, key.target.clip, key.target.effect);
                if worker_cancelled.load(Ordering::Acquire) {
                    return;
                }
                // Unbounded channel: the send only fails once the panel that
                // owns the receiver is gone.
                let _ = response_tx.send(MatteViewResponse {
                    generation,
                    key,
                    result,
                });
            });
        let Ok(handle) = spawn_result else {
            self.pending = None;
            self.error = Some("Could not start the matte coverage worker".to_owned());
            return;
        };
        #[cfg(test)]
        {
            self.spawned_workers += 1;
        }
        self.active = Some(MatteViewWorker { cancelled, handle });
    }

    fn reap_finished_worker(&mut self) {
        if self
            .active
            .as_ref()
            .is_some_and(|worker| worker.handle.is_finished())
            && let Some(worker) = self.active.take()
        {
            let _ = worker.handle.join();
        }
    }

    /// Drain coverage responses, accepting only the live generation and key.
    pub(crate) fn poll(&mut self) {
        while let Ok(response) = self.response_rx.try_recv() {
            if self.pending != Some((response.generation, response.key)) {
                continue;
            }
            self.pending = None;
            match response.result {
                Ok(proof) => {
                    self.coverage = Some((response.key, proof.coverage));
                    self.error = None;
                }
                Err(message) => {
                    self.coverage = None;
                    self.texture = None;
                    self.error = Some(message);
                }
            }
        }
        self.reap_finished_worker();
        if self.active.is_none()
            && let Some(request) = self.queued.take()
        {
            self.spawn(request);
        }
    }

    fn invalidate_view(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(worker) = self.active.as_ref() {
            worker.cancelled.store(true, Ordering::Release);
        }
        self.queued = None;
        self.pending = None;
        self.coverage = None;
        self.texture = None;
        self.error = None;
        self.last_key = None;
    }
}

/// Turn one coverage raster into an egui image.
///
/// The proof is already `R = G = B = round(255 · m)` with an opaque alpha
/// (CC5 §4.1), so this is a copy with no transfer, no tone map, and no
/// resample: what the viewer shows is the coverage the renderer measured.
#[must_use]
pub(crate) fn coverage_color_image(coverage: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [coverage.width as usize, coverage.height as usize],
        &coverage.pixels,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};
    use kinewright_core::MatteProofMetadata;

    /// The CC5 §9.1 raster: 64 × 36, aspect 16/9.
    const RASTER_WIDTH: f32 = 64.0;
    const RASTER_HEIGHT: f32 = 36.0;
    const ASPECT: f64 = 16.0 / 9.0;

    fn image_rect() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(RASTER_WIDTH, RASTER_HEIGHT))
    }

    /// The CC5 §9.1 raster drawn by an unreframed layer: the layer's uv and the
    /// composite's uv agree, so every hand-derived pixel below is unchanged
    /// from the pre-transform overlay.
    fn frame() -> MatteFrame {
        MatteFrame::new(ASPECT, image_rect(), LayerTransform::IDENTITY)
    }

    /// The same raster drawn by a layer scaled to `scale_percent` and offset by
    /// `x_percent` / `y_percent`, in the compositor's own units.
    fn reframed(scale_percent: i64, x_percent: i64, y_percent: i64) -> MatteFrame {
        reframed_in(image_rect(), scale_percent, x_percent, y_percent)
    }

    /// The same, letterboxed into an arbitrary rectangle — a ten-times raster
    /// keeps the eight-pixel handle radius small against a halved window.
    fn reframed_in(
        image_rect: Rect,
        scale_percent: i64,
        x_percent: i64,
        y_percent: i64,
    ) -> MatteFrame {
        #[allow(clippy::cast_precision_loss)]
        let transform = LayerTransform {
            scale: scale_percent as f64 / 100.0,
            offset_x: x_percent as f64 / 50.0,
            offset_y: y_percent as f64 / 50.0,
        };
        MatteFrame::new(ASPECT, image_rect, transform)
    }

    /// A window whose half-extents are equal **in pixels**: `hw · a == hh`, the
    /// CC5 §9.2 pixel-square case (7.2 px each way on a 64 × 36 raster).
    fn pixel_square_window(rotation_cd: i64) -> MatteWindowParams {
        MatteWindowParams {
            half_width_bp: 1_125,
            half_height_bp: 2_000,
            rotation_cd,
            ..MatteWindowParams::NEUTRAL
        }
    }

    fn assert_close(observed: Pos2, expected: (f32, f32), what: &str) {
        assert!(
            (observed.x - expected.0).abs() < 1e-3 && (observed.y - expected.1).abs() < 1e-3,
            "{what}: observed {observed:?}, expected {expected:?}"
        );
    }

    /// A centred neutral rect covers the middle half of the frame, so on a
    /// 64 × 36 raster its corners are the pixel positions `|u − 0.5| = 0.25`
    /// maps to — hand-derived, not read back from the transform.
    #[test]
    fn a_centred_rect_outlines_the_expected_pixel_corners() {
        let points = window_outline_points(&MatteWindowParams::NEUTRAL, frame());
        assert_eq!(points.len(), 4, "a rect outline is four corners");
        for (point, expected) in
            points
                .iter()
                .zip([(16.0, 9.0), (48.0, 9.0), (48.0, 27.0), (16.0, 27.0)])
        {
            assert_close(*point, expected, "centred rect corner");
        }
    }

    /// The aspect correction exists to keep rotation rigid (CC5 §2.3). With
    /// `hw · a == hh` the window is a square in pixels, so at 45° its corners
    /// sit on the axes at a common radius and the corner set is invariant under
    /// `(dx, dy) → (dy, dx)`. Without the aspect factor the square shears and
    /// neither holds.
    #[test]
    fn a_forty_five_degree_pixel_square_stays_square() {
        let centre = pos2(RASTER_WIDTH / 2.0, RASTER_HEIGHT / 2.0);
        let points = window_outline_points(&pixel_square_window(4_500), frame());
        let offsets: Vec<(f32, f32)> = points
            .iter()
            .map(|point| (point.x - centre.x, point.y - centre.y))
            .collect();
        // 7.2 px half-extent each way, so the corner radius is 7.2 · √2.
        let expected_radius = 7.2 * std::f32::consts::SQRT_2;
        for offset in &offsets {
            let radius = offset.0.hypot(offset.1);
            assert!(
                (radius - expected_radius).abs() < 1e-2,
                "corner radius {radius} is not {expected_radius}"
            );
        }
        for offset in &offsets {
            let swapped = (offset.1, offset.0);
            assert!(
                offsets.iter().any(|candidate| {
                    (candidate.0 - swapped.0).abs() < 1e-2 && (candidate.1 - swapped.1).abs() < 1e-2
                }),
                "the rotated corner set must be symmetric under swapping the axes: {offsets:?}"
            );
        }
    }

    /// An unfeathered window has no band to draw; a feathered one straddles the
    /// edge at `D = 1 ± f` exactly (CC5 §2.3).
    #[test]
    fn feather_outlines_straddle_the_edge_or_are_absent() {
        assert!(
            feather_outline_points(&MatteWindowParams::NEUTRAL, frame()).is_none(),
            "f = 0 takes the hard branch: there is no band"
        );
        let feathered = MatteWindowParams {
            feather_bp: 4_000,
            ..MatteWindowParams::NEUTRAL
        };
        let (inner, outer) = feather_outline_points(&feathered, frame()).expect("a feathered band");
        // D = 0.6 and D = 1.4 of a half-extent of 0.25 uv: 0.15 and 0.35.
        assert_close(inner[0], (22.4, 12.6), "inner band corner");
        assert_close(outer[0], (9.6, 5.4), "outer band corner");
    }

    /// Hand-picked pointers, one per CC5 §6 zone.
    #[test]
    fn hit_testing_names_the_move_resize_and_rotate_zones() {
        let window = MatteWindowParams::NEUTRAL;
        let frame = frame();
        let cases = [
            (pos2(32.0, 18.0), Some(MatteHit::Move), "the centre handle"),
            (pos2(40.0, 22.0), Some(MatteHit::Move), "inside the window"),
            (
                pos2(48.0, 18.0),
                Some(MatteHit::Resize(MatteHandle::Right)),
                "the right edge handle",
            ),
            (
                pos2(16.0, 9.0),
                Some(MatteHit::Resize(MatteHandle::TopLeft)),
                "the top-left corner handle",
            ),
            (
                pos2(32.0, 27.0),
                Some(MatteHit::Resize(MatteHandle::Bottom)),
                "the bottom edge handle",
            ),
            (
                pos2(32.0, 9.0 - MATTE_ROTATION_HANDLE_OFFSET_PX),
                Some(MatteHit::Rotate),
                "the rotation handle",
            ),
            (pos2(2.0, 2.0), None, "outside every zone"),
        ];
        for (pointer, expected, what) in cases {
            assert_eq!(
                hit_test(pointer, &window, frame, true),
                expected,
                "{what} at {pointer:?}"
            );
        }
        // Just past the 8 px radius, the rotation handle is not grabbed and the
        // pointer is outside the window, so nothing is.
        assert_eq!(
            hit_test(
                pos2(
                    32.0,
                    9.0 - MATTE_ROTATION_HANDLE_OFFSET_PX - MATTE_HANDLE_RADIUS_PX - 0.5
                ),
                &window,
                frame,
                true
            ),
            None
        );
    }

    /// An ellipse's eight handles are its bounding box in the rotated frame,
    /// but its interior is the disc: a pointer inside that box, outside the
    /// disc, and clear of every handle grabs nothing.
    ///
    /// Measured on a 640 × 360 viewer so the eight-pixel handle radius is small
    /// against the window rather than covering the corner lunes.
    #[test]
    fn an_ellipse_moves_only_from_inside_the_disc() {
        let ellipse = MatteWindowParams {
            shape_token: 2,
            ..MatteWindowParams::NEUTRAL
        };
        let frame = MatteFrame::new(
            ASPECT,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 360.0)),
            LayerTransform::IDENTITY,
        );
        // n = (0.5, 0.5): well inside the disc.
        assert_eq!(
            hit_test(pos2(400.0, 225.0), &ellipse, frame, true),
            Some(MatteHit::Move)
        );
        assert_eq!(
            hit_test(pos2(320.0, 180.0), &ellipse, frame, true),
            Some(MatteHit::Move),
            "the centre handle"
        );
        // n = (0.8, 0.8): |n| = 1.13, so inside the bounding box and outside the
        // disc, and 36.7 px from the nearest handle.
        assert_eq!(hit_test(pos2(448.0, 252.0), &ellipse, frame, true), None);
        // The same pointer on a rect window is inside it.
        assert_eq!(
            hit_test(pos2(448.0, 252.0), &MatteWindowParams::NEUTRAL, frame, true),
            Some(MatteHit::Move)
        );
    }

    /// CC5 §6: handles and the rotation arm are painted for the selected window
    /// only, so they are grabbable for the selected window only. An unselected
    /// window is select-then-edit — otherwise a pointer over an invisible
    /// handle resizes a window nobody chose.
    #[test]
    fn an_unselected_window_offers_only_move() {
        let window = MatteWindowParams::NEUTRAL;
        let frame = frame();

        let edge = pos2(48.0, 18.0);
        assert_eq!(
            hit_test(edge, &window, frame, true),
            Some(MatteHit::Resize(MatteHandle::Right)),
            "the selected window's right edge handle resizes"
        );
        assert_eq!(
            hit_test(edge, &window, frame, false),
            Some(MatteHit::Move),
            "the same pixel on an unselected window is just its boundary"
        );

        let arm = pos2(32.0, 9.0 - MATTE_ROTATION_HANDLE_OFFSET_PX);
        assert_eq!(hit_test(arm, &window, frame, true), Some(MatteHit::Rotate));
        assert_eq!(
            hit_test(arm, &window, frame, false),
            None,
            "an unselected window has no rotation arm to grab, and the arm is \
             outside the window"
        );

        assert_eq!(
            hit_test(pos2(40.0, 22.0), &window, frame, false),
            Some(MatteHit::Move),
            "the interior still moves, which is what makes select-then-edit work"
        );
    }

    // -----------------------------------------------------------------------
    // The layer transform (CC5 §5.2)
    // -----------------------------------------------------------------------

    /// The shader evaluates the matte at the *layer* quad's uv while
    /// `image_rect` holds the *composited* output, so the overlay converts
    /// between them. Hand-derived: at `scale = 0.5`, `x_percent = +25`
    /// (`offset_x = 0.5`) and `y_percent = -10` (`offset_y = -0.2`) the neutral
    /// window's centre composites at `u = (0.75, 0.4)`, which on the 64 × 36
    /// raster is `(48, 14.4)`, and its half-extents shrink with the layer.
    #[test]
    fn a_reframed_layer_moves_the_outline_with_its_picture() {
        let frame = reframed(50, 25, -10);
        let points = window_outline_points(&MatteWindowParams::NEUTRAL, frame);
        for (point, expected) in
            points
                .iter()
                .zip([(40.0, 9.9), (56.0, 9.9), (56.0, 18.9), (40.0, 18.9)])
        {
            assert_close(*point, expected, "reframed rect corner");
        }
        assert_close(
            window_centre_point(&MatteWindowParams::NEUTRAL, frame),
            (48.0, 14.4),
            "the reframed centre",
        );
        // The same reframe on a ten-times raster, so the eight-pixel handle
        // radius cannot swallow the window: the pre-transform centre pixel
        // (320, 180) now belongs to no window at all, which is exactly the bug
        // this fixes, and the reframed centre (480, 144) is where it moved to.
        let big = reframed_in(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 360.0)),
            50,
            25,
            -10,
        );
        assert_eq!(
            hit_test(pos2(320.0, 180.0), &MatteWindowParams::NEUTRAL, big, true),
            None,
            "the pre-transform centre pixel belongs to no window now"
        );
        assert_eq!(
            hit_test(pos2(480.0, 144.0), &MatteWindowParams::NEUTRAL, big, true),
            Some(MatteHit::Move),
            "the reframed centre is where the window actually is"
        );
        assert_eq!(
            hit_test(pos2(560.0, 144.0), &MatteWindowParams::NEUTRAL, big, true),
            Some(MatteHit::Resize(MatteHandle::Right)),
            "and its right edge handle follows the reframe"
        );
    }

    /// A drag writes *layer* basis points: the pointer moves through the
    /// composite, so the delta divides by the layer scale on the way in. Eight
    /// composite pixels across a half-scale layer is a quarter of the layer's
    /// own width, not an eighth.
    #[test]
    fn a_drag_on_a_reframed_layer_writes_layer_coordinates() {
        let frame = reframed(50, 25, -10);
        let start = MatteWindowParams::NEUTRAL;
        let gesture = drag(MatteHit::Move, start, pos2(48.0, 14.4));
        let moved = drag_to_params(&gesture, pos2(56.0, 18.0), frame);
        assert_eq!(
            (moved.center_x_bp, moved.center_y_bp),
            (7_500, 7_000),
            "the composite delta (0.125, 0.1) is a layer delta of (0.25, 0.2)"
        );

        // And the written centre round-trips: redrawing the window there puts
        // its centre under the pointer that dragged it.
        assert_close(
            window_centre_point(&moved, frame),
            (56.0, 18.0),
            "the drag result draws back under the pointer",
        );

        // A resize is measured in the layer's own field too: dragging the right
        // edge handle to the composite pixel the drag above landed on asks for
        // a half width of 0.25 layer-uv, unchanged by the reframe.
        let resize = drag(
            MatteHit::Resize(MatteHandle::Right),
            start,
            pos2(56.0, 14.4),
        );
        assert_eq!(
            drag_to_params(&resize, pos2(52.0, 14.4), frame).half_width_bp,
            1_250,
            "four composite pixels is 1250 layer basis points at half scale"
        );
    }

    /// An identity transform is the pre-CC5 behaviour, exactly: the conversion
    /// must not perturb an unreframed clip.
    #[test]
    fn an_identity_transform_leaves_the_overlay_where_it_was() {
        assert_eq!(
            window_outline_points(&MatteWindowParams::NEUTRAL, reframed(100, 0, 0)),
            window_outline_points(&MatteWindowParams::NEUTRAL, frame()),
        );
    }

    /// The forward and inverse conversions agree, including the sign of
    /// `offset_y`: `compositor.wgsl` negates it in NDC and `v = (1 − ndc.y)/2`
    /// negates it back, so a positive `y_percent` moves the picture *down*.
    #[test]
    fn the_layer_conversion_round_trips_and_moves_a_positive_y_offset_down() {
        let down = LayerTransform {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 1.0,
        };
        let composited = down.layer_to_composite((0.5, 0.5));
        assert!(
            (composited.1 - 1.0).abs() < 1e-12,
            "y_percent = +50 puts the layer centre on the bottom edge: {composited:?}"
        );
        for transform in [
            LayerTransform::IDENTITY,
            down,
            LayerTransform {
                scale: 0.5,
                offset_x: 0.5,
                offset_y: -0.2,
            },
        ] {
            for layer in [(0.0, 0.0), (0.25, 0.75), (1.0, 1.0)] {
                let back = transform.composite_to_layer(transform.layer_to_composite(layer));
                assert!(
                    (back.0 - layer.0).abs() < 1e-12 && (back.1 - layer.1).abs() < 1e-12,
                    "{transform:?} did not round-trip {layer:?}: {back:?}"
                );
            }
        }
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert!((round_half_away_from_zero(2.5) - 3.0).abs() < f64::EPSILON);
        assert!((round_half_away_from_zero(-2.5) + 3.0).abs() < f64::EPSILON);
    }

    fn drag(hit: MatteHit, start: MatteWindowParams, start_pointer: Pos2) -> MatteDrag {
        MatteDrag {
            target: MatteTarget::new(ClipId(1), EffectId(2)),
            window: 0,
            hit,
            start,
            start_pointer,
        }
    }

    /// A move is a uv delta in basis points, clamped to the CC5 §2.2 centre
    /// bounds — which are deliberately wider than the frame so a tracked window
    /// may leave and re-enter.
    #[test]
    fn a_move_writes_basis_points_and_allows_off_frame_centres() {
        let start = MatteWindowParams::NEUTRAL;
        let gesture = drag(MatteHit::Move, start, pos2(32.0, 18.0));
        let moved = drag_to_params(&gesture, pos2(38.4, 21.6), frame());
        assert_eq!(
            (moved.center_x_bp, moved.center_y_bp),
            (6_000, 6_000),
            "a tenth of the frame in each axis is 1000 basis points"
        );

        let far = drag_to_params(&gesture, pos2(32.0 + 192.0, 18.0 + 108.0), frame());
        assert_eq!(
            (far.center_x_bp, far.center_y_bp),
            (
                MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
                MATTE_WINDOW_CENTER_MAX_BASIS_POINTS
            ),
            "an off-frame drag clamps at +20000, it is not rejected"
        );
        let back = drag_to_params(&gesture, pos2(32.0 - 192.0, 18.0 - 108.0), frame());
        assert_eq!(
            (back.center_x_bp, back.center_y_bp),
            (
                MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
                MATTE_WINDOW_CENTER_MIN_BASIS_POINTS
            ),
            "and at -10000 on the other side"
        );
        assert_eq!(
            (back.half_width_bp, back.half_height_bp, back.rotation_cd),
            (start.half_width_bp, start.half_height_bp, start.rotation_cd),
            "a move touches nothing but the centre"
        );
    }

    /// A resize follows the pointer in the window's own rotated frame and can
    /// never write a degenerate half-extent: the descriptor minimum is 1.
    #[test]
    fn a_resize_follows_the_pointer_and_never_writes_a_zero_extent() {
        let start = MatteWindowParams::NEUTRAL;
        let gesture = drag(
            MatteHit::Resize(MatteHandle::Right),
            start,
            pos2(48.0, 18.0),
        );
        let resized = drag_to_params(&gesture, pos2(38.4, 18.0), frame());
        assert_eq!(resized.half_width_bp, 1_000);
        assert_eq!(
            resized.half_height_bp, start.half_height_bp,
            "an edge handle drives one axis only"
        );

        let collapsed = drag_to_params(&gesture, pos2(32.0, 18.0), frame());
        assert_eq!(
            collapsed.half_width_bp, MATTE_WINDOW_HALF_EXTENT_MIN_BASIS_POINTS,
            "a collapsed drag clamps to the minimum half-extent"
        );

        let corner = drag(
            MatteHit::Resize(MatteHandle::BottomRight),
            start,
            pos2(48.0, 27.0),
        );
        let both = drag_to_params(&corner, pos2(38.4, 21.6), frame());
        assert_eq!((both.half_width_bp, both.half_height_bp), (1_000, 1_000));
    }

    /// Rotation is measured from the window's centre, clockwise as the viewer
    /// sees it, and clamps at ±180°.
    #[test]
    fn a_rotate_measures_clockwise_from_the_top() {
        let start = MatteWindowParams::NEUTRAL;
        let gesture = drag(MatteHit::Rotate, start, pos2(32.0, 9.0));
        for (pointer, expected, what) in [
            (pos2(32.0, 0.0), 0, "straight up is zero"),
            (pos2(48.0, 18.0), 9_000, "to the right is +90°"),
            (pos2(16.0, 18.0), -9_000, "to the left is -90°"),
            (
                pos2(32.0, 36.0),
                MATTE_WINDOW_ROTATION_LIMIT_CENTIDEGREES,
                "straight down is +180°, the limit",
            ),
        ] {
            let rotated = drag_to_params(&gesture, pointer, frame());
            assert_eq!(rotated.rotation_cd, expected, "{what}");
            assert_eq!(
                (rotated.center_x_bp, rotated.half_width_bp),
                (start.center_x_bp, start.half_width_bp),
                "a rotate touches nothing but the angle"
            );
        }
    }

    /// θ is defined in the window's aspect-corrected field (CC5 §2.3), not in
    /// screen pixels, so the grab angle is measured there. A viewer whose
    /// `image_rect` is square while the document is 16 : 9 — a letterbox the
    /// caller sized from something other than the raster — must still write the
    /// angle the shader will rotate by.
    #[test]
    fn a_rotate_is_measured_in_field_space_not_in_screen_pixels() {
        let square = MatteFrame::new(
            ASPECT,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 64.0)),
            LayerTransform::IDENTITY,
        );
        let start = MatteWindowParams::NEUTRAL;
        let gesture = drag(MatteHit::Rotate, start, pos2(32.0, 16.0));
        // d = ((u.x − 0.5)·a, u.y − 0.5). Up-and-right at exactly 45° in that
        // field needs d = (0.25, −0.25), so u = (0.5 + 0.25·9/16, 0.25) and the
        // pixel is (41, 16) — *not* the 45° screen diagonal, which the raw
        // pixel offset would have read as 2936 centidegrees.
        assert_eq!(
            drag_to_params(&gesture, pos2(41.0, 16.0), square).rotation_cd,
            4_500,
            "the field-space diagonal is 45°"
        );
        assert_eq!(
            drag_to_params(&gesture, pos2(64.0, 32.0), square).rotation_cd,
            9_000,
            "straight right is still +90° whatever the letterbox aspect"
        );
        assert_eq!(
            drag_to_params(&gesture, pos2(32.0, 0.0), square).rotation_cd,
            0,
            "and straight up is still zero"
        );
    }

    /// The rotation arm's degenerate guard has to test the *separation*, not
    /// the normalized direction: a unit vector has length 1 by construction, so
    /// a guard on it can only ever fire for an exactly-zero separation. Below
    /// `f32::EPSILON` of separation the direction is noise, and an arm placed
    /// 24 px along it is 24 px of lie.
    #[test]
    fn a_degenerate_window_grows_no_rotation_arm() {
        // A one-basis-point half height on a 1e-4 px tall picture: the top edge
        // and the centre are 1e-8 px apart, far below f32::EPSILON.
        let sliver = MatteFrame::new(
            ASPECT,
            Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 1e-4)),
            LayerTransform::IDENTITY,
        );
        let window = MatteWindowParams {
            half_height_bp: 1,
            ..MatteWindowParams::NEUTRAL
        };
        let top = window_geometry_top(&window, sliver);
        assert_eq!(
            rotation_handle_point(&window, sliver),
            top,
            "a separation under f32::EPSILON offers no direction to place an arm along"
        );

        // Exactly zero is the same answer, and is finite.
        let collapsed = MatteWindowParams {
            half_height_bp: 0,
            ..MatteWindowParams::NEUTRAL
        };
        let handle = rotation_handle_point(&collapsed, frame());
        assert!(handle.x.is_finite() && handle.y.is_finite());
        assert_eq!(handle, window_geometry_top(&collapsed, frame()));

        // An ordinary window is untouched: the arm sits exactly the contract's
        // offset outside the top edge midpoint.
        let ordinary = MatteWindowParams::NEUTRAL;
        assert_close(
            rotation_handle_point(&ordinary, frame()),
            (32.0, 9.0 - MATTE_ROTATION_HANDLE_OFFSET_PX),
            "the arm on a window that has a top edge",
        );
    }

    struct RefusingProofSource;

    impl MatteProofSource for RefusingProofSource {
        fn matte_proof(
            &self,
            _document: Arc<Document>,
            _at: TimeCode,
            _clip: ClipId,
            _effect: EffectId,
        ) -> Result<MatteProof, String> {
            Err("matte proofs are not implemented by this backend".to_owned())
        }
    }

    struct CoverageProofSource;

    impl MatteProofSource for CoverageProofSource {
        fn matte_proof(
            &self,
            _document: Arc<Document>,
            _at: TimeCode,
            clip: ClipId,
            effect: EffectId,
        ) -> Result<MatteProof, String> {
            Ok(MatteProof {
                coverage: RgbaImage {
                    width: 1,
                    height: 1,
                    pixels: vec![255, 255, 255, 255],
                },
                metadata: MatteProofMetadata {
                    render: kinewright_core::MonitorProofMetadata {
                        render_kind: kinewright_core::MonitorProofRenderKind::TestDouble,
                        backend: "test".to_owned(),
                        adapter: "test".to_owned(),
                        software_fallback: true,
                        gpu_claim: false,
                        full_resolution: true,
                    },
                    clip,
                    effect,
                    node_kind: "color_wheels".to_owned(),
                    coverage_encoding: kinewright_core::MATTE_COVERAGE_ENCODING.to_owned(),
                    coverage_scale: kinewright_core::MATTE_COVERAGE_SCALE,
                    raster_aspect_millionths: 1_777_778,
                    matte_enabled: true,
                    window_count: 1,
                    qualifier_enabled: false,
                },
            })
        }
    }

    fn view_key() -> MatteViewKey {
        MatteViewKey {
            session_id: 1,
            revision: 3,
            frame: TimeCode(7),
            target: MatteTarget::new(ClipId(4), EffectId(5)),
        }
    }

    fn settle(state: &mut MatteOverlayState, key: MatteViewKey) {
        for _ in 0..2_000 {
            state.poll();
            if !matches!(state.view_status(key), MatteViewStatus::Pending) {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the matte view never resolved");
    }

    /// A backend without matte proofs — which is every backend until the media
    /// half of CC5 lands — leaves the toggle in a typed unavailable state
    /// rather than showing an empty frame, and is not asked again for the same
    /// frame identity.
    #[test]
    fn a_refused_coverage_render_becomes_a_typed_unavailable_state() {
        let mut state = MatteOverlayState::default();
        let key = view_key();
        assert_eq!(state.view_status(key), MatteViewStatus::Off);
        assert!(!state.needs_view(key), "the toggle is off");

        state.set_matte_view(true);
        assert!(state.needs_view(key));
        state.request_view(
            Arc::new(RefusingProofSource),
            Arc::new(Document::default()),
            key,
        );
        settle(&mut state, key);
        assert_eq!(
            state.view_status(key),
            MatteViewStatus::Unavailable(
                "matte proofs are not implemented by this backend".to_owned()
            )
        );
        assert!(
            !state.needs_view(key),
            "a refusal is sticky for the frame it refused"
        );
        assert_eq!(state.spawned_workers, 1);
    }

    /// The coverage the worker returns is kept for exactly the frame identity
    /// it was rendered for, and a second identity re-renders.
    #[test]
    fn coverage_is_kept_per_frame_identity_and_is_single_flight() {
        let mut state = MatteOverlayState::default();
        let key = view_key();
        state.set_matte_view(true);
        state.request_view(
            Arc::new(CoverageProofSource),
            Arc::new(Document::default()),
            key,
        );
        settle(&mut state, key);
        assert_eq!(state.view_status(key), MatteViewStatus::Ready);
        assert!(state.coverage_for(key).is_some());
        assert!(!state.needs_view(key));

        let next = MatteViewKey {
            frame: TimeCode(8),
            ..key
        };
        assert!(
            state.needs_view(next),
            "a different frame is a different coverage"
        );
        assert!(state.coverage_for(next).is_none());

        // Turning the toggle off drops the evidence rather than showing a stale
        // coverage the next time it is turned on.
        state.set_matte_view(false);
        assert_eq!(state.view_status(key), MatteViewStatus::Off);
        assert!(state.coverage_for(key).is_none());
    }

    /// CC5 §4.1: a blocked source blocks the coverage *render*, not merely its
    /// picture. The proof worker owns its own `FrameRenderer` and decoder,
    /// outside the visual cache's block path, so a request made while the
    /// Program viewer is blocked would decode exactly the media the block
    /// exists to keep off the screen — and `viewer_picture` would then throw
    /// the result away.
    #[test]
    fn a_blocked_preview_asks_for_no_coverage_render() {
        let mut state = MatteOverlayState::default();
        let key = view_key();
        state.set_matte_view(true);
        assert!(
            state.needs_view(key),
            "the toggle is on and nothing is cached"
        );

        assert!(
            !state.request_view_if_needed(
                true,
                Arc::new(CoverageProofSource),
                Arc::new(Document::default()),
                key,
            ),
            "a blocked source starts no render"
        );
        assert_eq!(
            state.spawned_workers, 0,
            "and therefore spawns no proof worker"
        );
        assert_eq!(
            state.view_status(key),
            MatteViewStatus::Pending,
            "nothing was marked in flight either"
        );
        assert!(
            state.needs_view(key),
            "the frame still needs a render once the block lifts"
        );

        assert!(state.request_view_if_needed(
            false,
            Arc::new(CoverageProofSource),
            Arc::new(Document::default()),
            key,
        ));
        settle(&mut state, key);
        assert_eq!(state.spawned_workers, 1);
        assert_eq!(state.view_status(key), MatteViewStatus::Ready);

        assert!(
            !state.request_view_if_needed(
                false,
                Arc::new(CoverageProofSource),
                Arc::new(Document::default()),
                key,
            ),
            "a coverage already in hand is not re-rendered"
        );
        assert_eq!(state.spawned_workers, 1);
    }

    /// CC5 §6: a selection outlives the window it named — removing the selected
    /// last window leaves the stored index past the count, and nothing else
    /// moves it. Read unclamped, the overlay would paint no handles at all and
    /// every hit would degrade to `Move` until the user clicked a window.
    #[test]
    fn a_selection_past_the_window_count_falls_back_to_the_last_window() {
        let mut state = MatteOverlayState::default();
        state.select_window(3, 4);
        assert_eq!(state.selected_window(4), Some(3));
        assert_eq!(
            state.selected_window(3),
            Some(2),
            "W3 removed: the selection lands on the last window that exists"
        );
        assert_eq!(state.selected_window(0), None, "no window, no selection");

        // A selection is clamped when it is made, too, so a card that offers a
        // stale index cannot store one.
        state.select_window(9, 2);
        assert_eq!(state.selected_window(2), Some(1));
        state.select_window(0, 0);
        assert_eq!(state.selected_window(0), None);
    }

    /// And the clamp reaches the pointer: the window the overlay drew handles
    /// on is the window whose handles can be grabbed.
    #[test]
    fn the_hit_path_grabs_the_handles_the_overlay_actually_drew() {
        // Four small windows in a row along the bottom of the raster; only the
        // first three are active.
        let small = |center_x_bp| MatteWindowParams {
            center_x_bp,
            center_y_bp: 8_000,
            half_width_bp: 500,
            half_height_bp: 500,
            ..MatteWindowParams::NEUTRAL
        };
        let matte = MatteParams {
            enabled: 1,
            window_count: 3,
            windows: [small(1_000), small(3_000), small(5_000), small(7_000)],
            ..MatteParams::NEUTRAL
        };
        let frame = frame();

        let mut state = MatteOverlayState::default();
        state.select_window(3, 4);
        // W2's rotation arm: 24 px above its top edge midpoint, which is
        // (32, 27) on the 64 × 36 raster, and nowhere near another window.
        let arm = pos2(32.0, 27.0 - MATTE_ROTATION_HANDLE_OFFSET_PX);
        assert_eq!(
            rotation_handle_point(&matte.windows[2], frame),
            arm,
            "the arm is where the paint path puts it"
        );

        assert_eq!(
            matte_hit_test(
                arm,
                &matte,
                frame,
                state.selected_window(matte.window_count)
            ),
            Some((2, MatteHit::Rotate)),
            "with W3 gone the selection is W2, and W2's arm is grabbable"
        );
        assert_eq!(
            matte_hit_test(arm, &matte, frame, Some(3)),
            None,
            "read unclamped, the arm belongs to a window that is not drawn:              every affordance is lost"
        );

        // The interior of an unselected window is still select-then-edit.
        assert_eq!(
            matte_hit_test(
                window_centre_point(&matte.windows[0], frame),
                &matte,
                frame,
                state.selected_window(matte.window_count),
            ),
            Some((0, MatteHit::Move))
        );
    }

    /// The expansion report lives one frame: a card that stops rendering
    /// returns the viewer to hover-only input.
    #[test]
    fn an_unreported_expansion_expires_at_the_end_of_the_frame() {
        let mut state = MatteOverlayState::default();
        let target = MatteTarget::new(ClipId(4), EffectId(5));
        state.report_expanded(target);
        state.expire_unreported();
        assert_eq!(
            state.expanded(),
            Some(target),
            "the frame that reported keeps the overlay live"
        );
        state.expire_unreported();
        assert_eq!(
            state.expanded(),
            None,
            "a frame in which no card reported expires it"
        );
        state.report_expanded(target);
        state.set_matte_view(true);
        state.expire_unreported();
        state.expire_unreported();
        assert!(
            !state.matte_view(),
            "closing the section also closes the matte view"
        );
    }
}
