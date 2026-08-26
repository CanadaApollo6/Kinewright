use std::sync::Arc;

use eframe::egui;
use kinewright_core::{
    Clip, EffectUniform, MatteParams, MediaKind, ThreePointMode, TimeCode, TrackId, TrackKind,
};

use crate::{
    app::KinewrightApp,
    color_qc_ui::{AnalysisColorQcSource, ColorQcSource, WorkingProofCache, WorkingProofKey},
    icons::Icon,
    inspector_ui::{InspectorEdits, matte_gesture_coalesce_key, matte_window_drag_operations},
    matte_overlay_ui::{
        AnalysisMatteProofSource, LayerTransform, MatteDrag, MatteFrame, MatteTarget, MatteViewKey,
        MatteViewStatus, coverage_color_image, matte_hit_test, paint_matte_overlay,
    },
    media_workflow::{
        paint_source_status, should_display_source_texture, source_access_is_allowed,
        source_display_state, source_edit_controls_are_enabled,
    },
    theme::{self, color, radius, space, type_size},
};

/// What one painted viewer frame occupies on screen.
///
/// `image_rect` is the letterboxed rectangle the picture was drawn into, and
/// is `None` when no texture was available. The matte overlay draws and
/// hit-tests through exactly that rectangle (CC5 §6), so it has to leave the
/// painter rather than stay a local.
struct ViewerFrame {
    response: egui::Response,
    image_rect: Option<egui::Rect>,
}

/// The Program viewer's pointer contract (CC5 §6).
///
/// The viewer takes click-and-drag input **only** while a matte section is
/// expanded for the selected clip; otherwise it keeps today's hover-only
/// behaviour exactly. Pure, so the decision is provable without a window —
/// giving the viewer input is the first interactive change to the preview
/// (CC5 §12) and the narrowness of that change is the mitigation.
#[must_use]
pub(crate) fn viewer_sense(matte_expanded: bool) -> egui::Sense {
    if matte_expanded {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    }
}

/// Everything the overlay needs about the node whose matte section is open.
struct MatteOverlayContext {
    target: MatteTarget,
    /// The keyframe-evaluated matte at the playhead: the overlay draws what the
    /// renderer renders, not what the card's sliders show.
    matte: MatteParams,
    /// The output raster aspect `a = W / H` (CC5 §2.3), supplied by the
    /// document rather than sniffed from the texture.
    aspect: f64,
    /// The clip's own `transform`, evaluated at the same clip-local frame as
    /// the matte. The shader evaluates the matte at the *layer* quad's uv while
    /// `image_rect` is the *composited* output, so without this a reframed clip
    /// draws its outline, its handles and its drag results displaced by the
    /// whole transform (CC5 §5.2).
    transform: LayerTransform,
    key: MatteViewKey,
}

impl MatteOverlayContext {
    /// Where this node's matte lands inside a painted viewer rectangle.
    fn frame(&self, image_rect: egui::Rect) -> MatteFrame {
        MatteFrame::new(self.aspect, image_rect, self.transform)
    }
}

/// One layer's resolved `transform`, at one clip-local frame.
///
/// Read through the descriptor's `EffectUniform`, not by effect name, so a
/// future effect that drives the same uniform is picked up here exactly as the
/// compositor picks it up (`compositor.rs`'s `LayerParams` accumulation); and
/// keyframe-evaluated at `local_at`, so an animated reframe moves the overlay
/// with the picture instead of pinning it to the static value.
fn resolved_layer_transform(clip: &Clip, local_at: TimeCode) -> LayerTransform {
    let mut transform = LayerTransform::IDENTITY;
    for effect in &clip.effects {
        let Some(descriptor) = kinewright_core::effect_descriptor(&effect.name) else {
            continue;
        };
        for parameter in descriptor.parameters {
            let value = effect
                .integer_parameter_at(parameter.name, local_at)
                .unwrap_or(parameter.neutral);
            #[allow(clippy::cast_precision_loss)]
            let value = value as f64;
            match parameter.uniform {
                EffectUniform::Scale => transform.scale *= value / 100.0,
                EffectUniform::OffsetX => transform.offset_x += value / 50.0,
                EffectUniform::OffsetY => transform.offset_y += value / 50.0,
                _ => {}
            }
        }
    }
    transform
}

/// Which texture the Program viewer shows.
///
/// `blocked` is consulted **first**: a source whose verification blocks the
/// preview must not leak through the Matte view or the QC mask, both of which
/// render the same media through the same decoder. Only then does a coverage
/// render, when one is ready, stand in for the picture.
///
/// **Precedence, normative (CC6 §8.2): the Matte view wins.** Both are
/// whole-picture replacements, so with both on one of them has to be the one
/// on screen; the matte view is the older, narrower, node-scoped view and is
/// the one a drag is being aimed at, so it takes the frame and the QC mask
/// waits.
fn viewer_picture<'a, T>(
    blocked: bool,
    matte: Option<&'a T>,
    qc_mask: Option<&'a T>,
    texture: Option<&'a T>,
) -> Option<&'a T> {
    if blocked {
        return None;
    }
    matte.or(qc_mask).or(texture)
}

// ---------------------------------------------------------------------------
// CC6 §8.2: the QC clipping mask
// ---------------------------------------------------------------------------

/// What the Program viewer paints instead of the picture (CC6 §8.2).
///
/// A whole-picture replacement, exactly like the CC5 matte view: no shader
/// change and no new `header.w` encoding, because that word is fully consumed
/// by matte-debug with an early return before the legacy stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum QcMaskView {
    #[default]
    Off,
    Clipping,
}

/// Any linear channel below zero: out of the Rec.709 gamut, clamped to black.
pub(crate) const QC_MASK_UNDER_RANGE_COLOR: [u8; 4] = [32, 64, 255, 255];
/// Any encoded channel above one: clamped to white by the delivery encode.
pub(crate) const QC_MASK_OVER_RANGE_COLOR: [u8; 4] = [255, 32, 32, 255];

/// What the status line says while the transport is running (CC6 §8.2).
pub(crate) const QC_MASK_PAUSED_ONLY: &str =
    "Paused only — the mask renders one full-resolution working proof per frame; pause to see it.";

/// The legend, always visible while the mask is on.
pub(crate) const QC_MASK_LEGEND: &str = "blue = a negative linear channel — out of the Rec.709 gamut and clamped to black; \
red = an encoded value above 1.0 — clamped to white.";

/// Build the clipping mask from one scene-linear working proof.
///
/// The transfer is `kinewright_core::color_qc::encode_bt709_delivery` — the
/// single source CC6 §3.0 names. The app depends on both crates, so which copy
/// this uses is normative rather than stylistic: a second transcription here
/// would let the mask and the counted basis points disagree about which pixels
/// clip.
///
/// Precedence: non-finite first, then under-range, then over-range, then grey.
/// A pixel that is over on one channel and under on another is drawn **blue**,
/// because a negative channel is the unrecoverable one — it is outside the
/// gamut, not merely outside the range. A pixel with any non-finite channel is
/// drawn **black**, which is exactly what core does with it: `measure_color_qc`
/// counts it in `non_finite_pixel_count` and excludes the whole pixel from both
/// excursion reports.
///
/// The grey is `round(255 · clamp(e(Y_linear), 0, 1)) / 2` in integer
/// division, so the picture stays readable underneath the two flags without
/// ever being mistaken for one of them.
#[must_use]
pub(crate) fn qc_mask_image(
    image: &kinewright_core::LinearRgbaImage,
) -> kinewright_core::RgbaImage {
    let pixel_count = (image.width as usize).saturating_mul(image.height as usize);
    let mut pixels = Vec::with_capacity(pixel_count.saturating_mul(4));
    let source = image.pixels.as_chunks::<4>().0;
    for index in 0..pixel_count {
        // Exactly `width · height` output pixels whatever the input length, so
        // the texture upload — which asserts the two agree — can never panic on
        // a truncated readback. A missing sample is drawn as black rather than
        // as either flag: an absent pixel is not evidence of clipping.
        let Some(pixel) = source.get(index) else {
            pixels.extend_from_slice(&[0, 0, 0, 255]);
            continue;
        };
        let linear = [pixel[0], pixel[1], pixel[2]];
        let encoded = linear.map(kinewright_core::encode_bt709_delivery);
        if linear
            .iter()
            .chain(&encoded)
            .any(|value| !value.is_finite())
        {
            // A non-finite sample is drawn black, exactly as a missing one is,
            // and for the same reason: it is not evidence of clipping.
            //
            // This is core's classification, not a second opinion:
            // `RegionAccumulator::add` counts a pixel with any non-finite
            // channel in `non_finite_pixel_count` and returns **before** the
            // range and gamut accumulators see it, so such a pixel is neither
            // over nor under however extreme its other channels are. The test
            // is therefore made first and over the whole pixel — flagging the
            // finite channel of a discarded pixel would draw an excursion core
            // never counted.
            pixels.extend_from_slice(&[0, 0, 0, 255]);
        } else if encoded.iter().any(|value| *value < 0.0) {
            pixels.extend_from_slice(&QC_MASK_UNDER_RANGE_COLOR);
        } else if encoded.iter().any(|value| *value > 1.0) {
            pixels.extend_from_slice(&QC_MASK_OVER_RANGE_COLOR);
        } else {
            // CC1's linear luma coefficients in the proof's own `f32`,
            // encoded through the same transfer as the flags so the grey is
            // the delivery's own value rather than a second opinion.
            let luma = 0.2126_f32 * linear[0] + 0.7152_f32 * linear[1] + 0.0722_f32 * linear[2];
            let encoded_luma = kinewright_core::encode_bt709_delivery(luma);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let code = (255.0 * f64::from(encoded_luma).clamp(0.0, 1.0)).round() as u8;
            let grey = code / 2;
            pixels.extend_from_slice(&[grey, grey, grey, 255]);
        }
    }
    kinewright_core::RgbaImage {
        width: image.width,
        height: image.height,
        pixels,
    }
}

/// The identity of one QC mask render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QcMaskKey {
    pub(crate) session_id: u64,
    pub(crate) revision: u64,
    pub(crate) frame: TimeCode,
}

impl QcMaskKey {
    /// The shared render this mask is built from (CC6 §8.1/§8.2).
    const fn proof_key(self) -> WorkingProofKey {
        WorkingProofKey {
            session_id: self.session_id,
            revision: self.revision,
            frame: self.frame,
        }
    }
}

/// Every standing reason the mask may not render, in one value.
///
/// Gathered rather than passed around loose so the status line and the request
/// path cannot disagree about them: each one withholds the *render*, not
/// merely its picture.
// Four bools on purpose: they are four independent standing reasons, each
// with its own status, and every one of them has to be answerable on its own.
// A state machine would have to name every combination of four conditions that
// can hold at once, which is exactly what this value exists to avoid.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct QcMaskConditions {
    /// Source verification withholds the render itself (CC5 §4.1): the proof
    /// worker owns its own decoder, outside the visual cache's block path.
    pub(crate) blocked: bool,
    /// The Matte view is on and takes the frame (CC6 §8.2 precedence), so a
    /// proof rendered now would be discarded by `viewer_picture`.
    pub(crate) behind_matte_view: bool,
    /// The transport is running. One full-resolution working proof per frame
    /// identity cannot keep up with playback, and every one of them would be
    /// stale before it landed, so the mask is a paused-only view.
    pub(crate) playing: bool,
    /// A playhead drag is in progress. Paused, but not still: the frame
    /// identity moves with the pointer, so every proof a scrub asked for would
    /// be stale before it landed — the same reason playback withholds them,
    /// arrived at by the other route. `playing` is `false` throughout a scrub
    /// (the drag pauses the transport and resumes it on release), so without
    /// this the mask queued one full-resolution render per pointer sample.
    pub(crate) scrubbing: bool,
}

impl QcMaskConditions {
    /// Whether any standing reason withholds the render.
    const fn withholds_render(self) -> bool {
        self.blocked || self.behind_matte_view || self.playing || self.scrubbing
    }
}

/// What the QC mask can show, as a typed state rather than a bare option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QcMaskStatus {
    Off,
    /// The source-verification block withholds the render, not merely its
    /// picture (CC5 §4.1). Nothing is in flight and nothing will be.
    Blocked,
    /// The Matte view is on and takes the frame (CC6 §8.2 precedence).
    BehindMatteView,
    /// The transport is moving — playing, or under a playhead drag — so
    /// nothing was asked for (CC6 §8.2).
    PausedOnly,
    Pending,
    Ready,
    Unavailable(String),
}

struct QcMaskResponse {
    generation: u64,
    key: QcMaskKey,
    result: Result<kinewright_core::RgbaImage, String>,
}

struct QcMaskRequest {
    generation: u64,
    key: QcMaskKey,
    source: Arc<dyn ColorQcSource>,
    cache: Arc<WorkingProofCache>,
    document: Arc<kinewright_core::Document>,
}

struct QcMaskWorker {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

/// Makes every live mask worker resolve the view exactly once: with its mask,
/// or with an error if the thread unwinds before it delivers one.
///
/// Without this a panicking `working_proof` would send nothing, leave
/// `pending` set for that frame identity forever, and wedge the viewer on
/// "Rendering working proof…" with no request in flight and no retry.
struct QcMaskCompletion {
    generation: u64,
    key: QcMaskKey,
    response_tx: std::sync::mpsc::Sender<QcMaskResponse>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    delivered: bool,
}

impl QcMaskCompletion {
    fn is_retired(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Acquire)
    }

    fn deliver(&mut self, result: Result<kinewright_core::RgbaImage, String>) {
        if self.delivered {
            return;
        }
        self.delivered = true;
        if self.is_retired() {
            return;
        }
        let _ = self.response_tx.send(QcMaskResponse {
            generation: self.generation,
            key: self.key,
            result,
        });
    }
}

impl Drop for QcMaskCompletion {
    fn drop(&mut self) {
        self.deliver(Err(
            "the QC clipping mask worker stopped before it delivered a proof".to_owned(),
        ));
    }
}

/// The QC mask's view state and its single working-proof worker.
///
/// Deliberately the matte view's machinery, method for method:
/// `request_view_if_needed` / `texture_for` / `set_texture`, single flight,
/// generation-keyed responses, and a sticky typed refusal per frame identity
/// so a `NotImplemented` backend is asked once rather than every repaint.
pub(crate) struct QcMaskState {
    view: QcMaskView,
    mask: Option<(QcMaskKey, kinewright_core::RgbaImage)>,
    texture: Option<(QcMaskKey, egui::TextureHandle)>,
    error: Option<String>,
    last_key: Option<QcMaskKey>,
    pending: Option<(u64, QcMaskKey)>,
    active: Option<QcMaskWorker>,
    queued: Option<QcMaskRequest>,
    generation: u64,
    /// Set on playhead drag start and cleared on drag stop.
    ///
    /// Lives here rather than beside `playing` because this is the only thing
    /// that reads it, and both scrub paths — the transport slider and the
    /// timeline ruler — already run through the app that owns this state.
    scrubbing: bool,
    response_tx: std::sync::mpsc::Sender<QcMaskResponse>,
    response_rx: std::sync::mpsc::Receiver<QcMaskResponse>,
    #[cfg(test)]
    spawned_workers: u64,
}

impl std::fmt::Debug for QcMaskState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QcMaskState")
            .field("view", &self.view)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Default for QcMaskState {
    fn default() -> Self {
        let (response_tx, response_rx) = std::sync::mpsc::channel();
        Self {
            view: QcMaskView::Off,
            mask: None,
            texture: None,
            error: None,
            last_key: None,
            pending: None,
            active: None,
            queued: None,
            generation: 0,
            scrubbing: false,
            response_tx,
            response_rx,
            #[cfg(test)]
            spawned_workers: 0,
        }
    }
}

impl QcMaskState {
    #[must_use]
    pub(crate) const fn is_on(&self) -> bool {
        matches!(self.view, QcMaskView::Clipping)
    }

    /// Note that a playhead drag started or stopped (CC6 §8.2).
    ///
    /// Called from both scrub paths — the transport slider and the timeline
    /// ruler — on drag start and drag stop. A scrub pauses the transport, so
    /// `playing` is `false` for its whole duration; without this the mask read
    /// a dragging playhead as a still frame and asked for one full-resolution
    /// working proof per pointer sample.
    pub(crate) const fn set_scrubbing(&mut self, scrubbing: bool) {
        self.scrubbing = scrubbing;
    }

    /// Whether a playhead drag is in progress.
    #[must_use]
    pub(crate) const fn is_scrubbing(&self) -> bool {
        self.scrubbing
    }

    pub(crate) fn set_view(&mut self, view: QcMaskView) {
        if self.view == view {
            return;
        }
        self.view = view;
        if matches!(view, QcMaskView::Off) {
            self.invalidate();
        }
    }

    #[must_use]
    fn mask_for(&self, key: QcMaskKey) -> Option<&kinewright_core::RgbaImage> {
        self.mask
            .as_ref()
            .filter(|(stored, _)| *stored == key)
            .map(|(_, image)| image)
    }

    #[must_use]
    pub(crate) fn texture_for(&self, key: QcMaskKey) -> Option<&egui::TextureHandle> {
        self.texture
            .as_ref()
            .filter(|(stored, _)| *stored == key)
            .map(|(_, texture)| texture)
    }

    pub(crate) fn set_texture(&mut self, key: QcMaskKey, texture: egui::TextureHandle) {
        self.texture = Some((key, texture));
    }

    /// What the mask can show for `key`, given the two reasons no render was
    /// asked for.
    ///
    /// Every reason no render was asked for is typed and answered *before*
    /// `Pending`: a withheld request has nothing in flight, and reporting one
    /// as pending would show a spinner for a render that will never start.
    #[must_use]
    pub(crate) fn status(&self, key: QcMaskKey, conditions: QcMaskConditions) -> QcMaskStatus {
        if !self.is_on() {
            return QcMaskStatus::Off;
        }
        if conditions.blocked {
            return QcMaskStatus::Blocked;
        }
        if conditions.behind_matte_view {
            return QcMaskStatus::BehindMatteView;
        }
        if conditions.playing || conditions.scrubbing {
            // Before the error and the mask: while the transport is moving
            // nothing is in flight and the frame identity moves every tick, so
            // neither a stale refusal nor a stale mask describes what the
            // viewer shows. A scrub is the same situation reached by dragging.
            return QcMaskStatus::PausedOnly;
        }
        if let Some(message) = &self.error {
            return QcMaskStatus::Unavailable(message.clone());
        }
        if self.mask_for(key).is_some() {
            return QcMaskStatus::Ready;
        }
        // On, nothing refused, nothing rendered: a render is on its way. The
        // controls ask for one immediately after the toggle is applied and
        // before this is read, so the only way to be here is between a
        // measurement being invalidated and the next request landing — which
        // is a pending render, not a failure. Reporting it as `Unavailable`
        // put a red "QC mask unavailable" under the viewer for one frame every
        // time the mask was switched on.
        QcMaskStatus::Pending
    }

    /// Whether a render is in flight, so the frame loop keeps repainting until
    /// the result can land.
    #[must_use]
    pub(crate) const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn needs_view(&self, key: QcMaskKey) -> bool {
        if !self.is_on() {
            return false;
        }
        if self.pending.is_some_and(|(_, pending)| pending == key) {
            return false;
        }
        if self.mask_for(key).is_some() {
            return false;
        }
        !(self.error.is_some() && self.pending.is_none() && self.last_key == Some(key))
    }

    /// Ask for the working proof `key` needs, if it needs one and may have one.
    ///
    /// Every [`QcMaskConditions`] flag withholds the *render*, not merely its
    /// picture: CC5 §4.1's block reason verbatim for a blocked source, the
    /// discarded-result reason for the matte view, and the transport for
    /// playback — a full-resolution proof per frame cannot keep up with it,
    /// and each one would be stale before it landed.
    pub(crate) fn request_view_if_needed(
        &mut self,
        conditions: QcMaskConditions,
        source: Arc<dyn ColorQcSource>,
        cache: Arc<WorkingProofCache>,
        document: Arc<kinewright_core::Document>,
        key: QcMaskKey,
    ) -> bool {
        if conditions.withholds_render() || !self.needs_view(key) {
            return false;
        }
        self.request_view(source, cache, document, key);
        true
    }

    /// Ask for one working proof, latest request wins.
    ///
    /// The proof comes from the shared [`WorkingProofCache`], so a frame the
    /// Colour QC window has already measured costs the mask nothing and the
    /// two never stand two `FrameRenderer`s side by side for one raster.
    pub(crate) fn request_view(
        &mut self,
        source: Arc<dyn ColorQcSource>,
        cache: Arc<WorkingProofCache>,
        document: Arc<kinewright_core::Document>,
        key: QcMaskKey,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = Some((generation, key));
        self.last_key = Some(key);
        self.error = None;
        let request = QcMaskRequest {
            generation,
            key,
            source,
            cache,
            document,
        };
        if self
            .active
            .as_ref()
            .is_some_and(|worker| !worker.handle.is_finished())
        {
            if let Some(worker) = self.active.as_ref() {
                worker
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            self.queued = Some(request);
            return;
        }
        self.reap_finished_worker();
        self.spawn(request);
    }

    fn spawn(&mut self, request: QcMaskRequest) {
        use std::sync::atomic::AtomicBool;

        let QcMaskRequest {
            generation,
            key,
            source,
            cache,
            document,
        } = request;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let response_tx = self.response_tx.clone();
        let spawn_result = std::thread::Builder::new()
            .name("kinewright-qc-mask".to_owned())
            .spawn(move || {
                let mut completion = QcMaskCompletion {
                    generation,
                    key,
                    response_tx,
                    cancelled: worker_cancelled,
                    delivered: false,
                };
                if completion.is_retired() {
                    completion.delivered = true;
                    return;
                }
                completion.deliver(
                    cache
                        .proof(source.as_ref(), document, key.proof_key())
                        .map(|proof| qc_mask_image(&proof.image)),
                );
            });
        let Ok(handle) = spawn_result else {
            self.pending = None;
            self.error = Some("Could not start the QC clipping mask worker".to_owned());
            return;
        };
        #[cfg(test)]
        {
            self.spawned_workers += 1;
        }
        self.active = Some(QcMaskWorker { cancelled, handle });
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

    /// Drain mask responses, accepting only the live generation and key.
    pub(crate) fn poll(&mut self) {
        while let Ok(response) = self.response_rx.try_recv() {
            if self.pending != Some((response.generation, response.key)) {
                continue;
            }
            self.pending = None;
            match response.result {
                Ok(mask) => {
                    self.mask = Some((response.key, mask));
                    self.error = None;
                }
                Err(message) => {
                    self.mask = None;
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

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(worker) = self.active.as_ref() {
            worker
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.queued = None;
        self.pending = None;
        self.mask = None;
        self.texture = None;
        self.error = None;
        self.last_key = None;
    }
}

/// The overlay context for one expanded matte section, as a pure function of
/// the document and the playhead (CC5 §6).
///
/// `None` — and therefore no overlay and no pointer capture — whenever the
/// report names a clip that is not selected, a clip or effect that is gone, or
/// a playhead that is **not over the clip**: outside `[timeline_start,
/// timeline_start + duration)` the renderer is showing some other clip's
/// picture, so an overlay there would draw one node's windows on top of another
/// node's frame and a drag would edit a matte the user cannot see. A matte with
/// no window still yields a context: the qualifier, the mix, and the Matte view
/// are all editable without one.
fn matte_overlay_context_for(
    document: &kinewright_core::Document,
    selected_clip: Option<kinewright_core::ClipId>,
    position: TimeCode,
    target: MatteTarget,
    session_id: u64,
    revision: u64,
) -> Option<MatteOverlayContext> {
    if selected_clip != Some(target.clip) {
        return None;
    }
    let clip = document.clip(target.clip)?;
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.id == target.effect)?;
    // Effect keyframes are clip-local (CC3 §3), so the overlay evaluates at the
    // playhead's local frame and draws the geometry the renderer used.
    //
    // `TimeCode` is a signed frame count, so `checked_sub` only fails on
    // overflow — a playhead *before* the clip yields a negative local frame
    // rather than `None`, which is why the range is tested explicitly. The old
    // `unwrap_or(TimeCode::ZERO)` fallback therefore never fired at all: it
    // evaluated a negative frame instead.
    let local_at = position.checked_sub(clip.timeline_start)?;
    let duration = document.clip_duration(clip).ok()?;
    if local_at < TimeCode::ZERO || local_at >= duration {
        return None;
    }
    let matte = MatteParams::from_effect(&effect.evaluated_at(local_at));
    let (width, height) = document.resolution;
    Some(MatteOverlayContext {
        target,
        matte,
        aspect: f64::from(width.max(1)) / f64::from(height.max(1)),
        transform: resolved_layer_transform(clip, local_at),
        key: MatteViewKey {
            session_id,
            revision,
            frame: position,
            target,
        },
    })
}

impl KinewrightApp {
    /// The monitor dock deliberately keeps Source and Program side by side.
    /// Source is a frame-accurate still/scrub monitor backed by `VisualCache`;
    /// Program remains the live project playback output.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn preview(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let (width_px, height_px) = self.focused().document.resolution;
        let aspect = height_px.max(1) as f32 / width_px.max(1) as f32;
        let panel_width = ((available.x - space::TWO) / 2.0).max(120.0);
        let frame_height = (panel_width * aspect).clamp(112.0, (available.y * 0.46).max(112.0));
        ui.columns(2, |columns| {
            self.source_viewer(&mut columns[0], frame_height);
            self.program_viewer(&mut columns[1], frame_height);
        });
    }

    #[allow(clippy::cast_precision_loss)]
    fn source_viewer(&mut self, ui: &mut egui::Ui, frame_height: f32) {
        let available_width = ui.available_width();
        let Some(asset_id) = self.focused().selected_asset else {
            Self::paint_empty_viewer(
                ui,
                egui::vec2(available_width, frame_height),
                "SOURCE",
                "Select an asset to cue Source",
                color::TEXT_MUTED,
            );
            return;
        };
        let Some(asset) = self.focused().document.asset(asset_id).cloned() else {
            self.focused_mut().reconcile_source_state();
            Self::paint_empty_viewer(
                ui,
                egui::vec2(available_width, frame_height),
                "SOURCE",
                "Source asset is no longer in this project",
                color::STATUS_DANGER,
            );
            return;
        };
        let source_status = self.source_media_status_for_asset(&asset);
        if let Some(refresh_after) = source_status.refresh_after {
            ui.ctx().request_repaint_after(refresh_after);
        }
        let source_state = source_display_state(source_status.status.as_ref());
        let revalidation_pending = self.source_edit_revalidation_pending();
        let blocked = source_state.blocks_preview();
        let source_access_allowed = source_access_is_allowed(source_state, revalidation_pending);
        let source_position = self
            .focused()
            .source_position
            .0
            .clamp(0, asset.duration.0.saturating_sub(1).max(0));
        self.focused_mut().source_position = TimeCode(source_position);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("SOURCE").font(theme::semibold(type_size::CAPTION)));
            ui.colored_label(color::TEXT_MUTED, &asset.name);
            paint_source_status(ui, source_state);
        });
        let source_texture = if source_access_allowed
            && matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo)
        {
            let analysis = std::sync::Arc::clone(&self.analysis);
            self.visual_cache
                .thumbnail(analysis.as_ref(), &asset, TimeCode(source_position), 512)
        } else {
            None
        };
        let source_texture =
            source_texture.filter(|_| should_display_source_texture(source_state, true));
        Self::paint_viewer_frame(
            ui,
            egui::vec2(available_width, frame_height),
            "SOURCE",
            source_texture.as_ref(),
            if blocked {
                source_state.label()
            } else if revalidation_pending {
                "Verifying source before edit"
            } else if !source_access_allowed {
                "Source verification required"
            } else if asset.kind == MediaKind::Audio {
                "Audio-only source (no picture)"
            } else {
                "Waiting for source frame"
            },
            if blocked {
                color::STATUS_DANGER
            } else {
                color::TEXT_MUTED
            },
            egui::Sense::hover(),
        );
        self.source_controls(ui, &asset, source_state, revalidation_pending);
    }

    fn program_viewer(&mut self, ui: &mut egui::Ui, frame_height: f32) {
        let available_width = ui.available_width();
        let playhead_state = self.playhead_media_state();
        let blocked = playhead_state
            .as_ref()
            .is_some_and(|(state, _)| state.blocks_preview());
        self.matte_overlay.poll();
        self.qc_mask.poll();
        let overlay = self.matte_overlay_context();
        // `blocked` reaches the *request*, not just the picture: the coverage
        // worker decodes the same media through its own renderer, so asking for
        // one while the source is blocked would decode what the block exists to
        // withhold (CC5 §4.1).
        let matte_texture = overlay
            .as_ref()
            .and_then(|context| self.matte_view_texture(ui.ctx(), blocked, context));
        // CC6 §8.2: both views are whole-picture replacements, and the matte
        // view wins. Decided from the *toggle*, not from whether its texture
        // happens to be ready, so the mask never pays for a full-resolution
        // working proof whose result `viewer_picture` would then discard.
        let qc_mask_conditions = QcMaskConditions {
            blocked,
            behind_matte_view: self.matte_overlay.matte_view(),
            playing: self.playing,
            scrubbing: self.qc_mask.is_scrubbing(),
        };
        let qc_mask_texture = self.qc_mask_texture(ui.ctx(), qc_mask_conditions);
        let texture = self.texture.clone();
        let picture = viewer_picture(
            blocked,
            matte_texture.as_ref(),
            qc_mask_texture.as_ref(),
            texture.as_ref(),
        );
        let frame = Self::paint_viewer_frame(
            ui,
            egui::vec2(available_width, frame_height),
            "PROGRAM",
            picture,
            if let Some((state, _)) = playhead_state {
                state.label()
            } else {
                "No timeline frame"
            },
            if blocked {
                color::STATUS_DANGER
            } else {
                color::TEXT_MUTED
            },
            viewer_sense(overlay.is_some()),
        );
        if let Some(context) = overlay {
            if let Some(image_rect) = frame.image_rect {
                paint_matte_overlay(
                    &ui.painter_at(frame.response.rect),
                    context.frame(image_rect),
                    &context.matte,
                    &self.matte_overlay,
                );
                self.handle_matte_pointer(&frame.response, image_rect, &context);
            }
            self.matte_viewer_controls(ui, &context);
        }
        self.qc_mask_controls(ui, qc_mask_conditions);
    }

    /// The CC6 §8.2 QC mask toggle, its status, and its legend.
    fn qc_mask_controls(&mut self, ui: &mut egui::Ui, conditions: QcMaskConditions) {
        let key = self.qc_mask_key();
        let mut on = self.qc_mask.is_on();
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("QC MASK").font(theme::semibold(type_size::CAPTION)));
            if ui
                .checkbox(&mut on, "Clipping")
                .on_hover_text(
                    "Replace the picture with the delivery clipping mask measured at \
                     working_linear_post_composite (CC6 §8.2). Evidence only: nothing about \
                     the grade or the export changes.",
                )
                .changed()
            {
                self.qc_mask.set_view(if on {
                    QcMaskView::Clipping
                } else {
                    QcMaskView::Off
                });
            }
            // Asked for *after* the toggle has been applied, and before the
            // status is read: a request made earlier in the frame — before the
            // checkbox was ticked — leaves the frame the user just turned the
            // mask on with nothing in flight, which read as a red "QC mask
            // unavailable" for exactly one frame.
            self.request_qc_mask_view(ui.ctx(), conditions);
            let current = self.qc_mask.status(key, conditions);
            match &current {
                QcMaskStatus::Off => {}
                QcMaskStatus::Blocked => {
                    ui.colored_label(
                        color::STATUS_DANGER,
                        "source verification withholds this render",
                    );
                }
                QcMaskStatus::BehindMatteView => {
                    ui.colored_label(
                        color::STATUS_WARNING,
                        "the Matte view is on and takes the frame; turn it off to see the mask",
                    );
                }
                QcMaskStatus::PausedOnly => {
                    ui.colored_label(color::STATUS_WARNING, QC_MASK_PAUSED_ONLY);
                }
                QcMaskStatus::Pending => {
                    ui.colored_label(color::STATUS_WARNING, "Rendering working proof…");
                }
                QcMaskStatus::Ready => {
                    ui.colored_label(color::STATUS_SUCCESS, "Clipping mask");
                }
                QcMaskStatus::Unavailable(message) => {
                    ui.colored_label(
                        color::STATUS_DANGER,
                        format!("QC mask unavailable: {message}"),
                    );
                }
            }
        });
        if self.qc_mask.is_on() {
            // Always visible while the view is on: the two colours mean two
            // different unrecoverable things and neither is guessable.
            ui.add(
                egui::Label::new(egui::RichText::new(QC_MASK_LEGEND).color(color::TEXT_MUTED))
                    .wrap(),
            );
        }
    }

    /// The frame identity one QC mask render belongs to.
    fn qc_mask_key(&self) -> QcMaskKey {
        let session = self.focused();
        QcMaskKey {
            session_id: session.id,
            revision: session.revision.0,
            frame: session.position,
        }
    }

    /// Ask for the mask's working proof, subject to every standing reason not
    /// to (CC6 §8.2).
    ///
    /// Called from the controls rather than from the texture path, because the
    /// toggle is read there: on the frame the mask is switched on, this is the
    /// only call that can still put a render in flight before the status line
    /// is drawn.
    fn request_qc_mask_view(&mut self, ctx: &egui::Context, conditions: QcMaskConditions) {
        if !self.qc_mask.is_on() {
            return;
        }
        let key = self.qc_mask_key();
        let cache = Arc::clone(&self.working_proof_cache);
        let source = Arc::new(AnalysisColorQcSource::new(
            Arc::clone(&self.analysis),
            Arc::clone(&cache),
        ));
        let document = Arc::clone(&self.focused().document);
        self.qc_mask
            .request_view_if_needed(conditions, source, cache, document, key);
        if self.qc_mask.is_pending() {
            // Nothing else drives a repaint while a worker renders, and a
            // result that lands between frames would otherwise wait for
            // unrelated input.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// Fetch, cache, and upload the clipping mask behind the QC mask view.
    ///
    /// The same machinery as the matte view, deliberately: one worker at a
    /// time, `NEAREST` sampling because a mask code is evidence and a bilinear
    /// filter would invent flags no pixel carries.
    fn qc_mask_texture(
        &mut self,
        ctx: &egui::Context,
        conditions: QcMaskConditions,
    ) -> Option<egui::TextureHandle> {
        if !self.qc_mask.is_on() || conditions.withholds_render() {
            return None;
        }
        let key = self.qc_mask_key();
        if let Some(texture) = self.qc_mask.texture_for(key) {
            return Some(texture.clone());
        }
        let image = coverage_color_image(self.qc_mask.mask_for(key)?);
        let texture = ctx.load_texture("qc-clipping-mask", image, egui::TextureOptions::NEAREST);
        self.qc_mask.set_texture(key, texture.clone());
        Some(texture)
    }

    /// The node whose matte section the inspector reported as expanded, with
    /// everything the overlay needs to draw and edit it (CC5 §6).
    ///
    /// `None` — and therefore no overlay and no pointer capture — whenever the
    /// report names a clip that is not selected, or a clip or effect that is
    /// gone. A matte with no window still yields a context: the qualifier, the
    /// mix, and the Matte view are all editable without one.
    fn matte_overlay_context(&self) -> Option<MatteOverlayContext> {
        let target = self.matte_overlay.expanded()?;
        let session = self.focused();
        matte_overlay_context_for(
            &session.document,
            session.selected_clip,
            session.position,
            target,
            session.id,
            session.revision.0,
        )
    }

    /// Turn one frame of viewer pointer interaction into coalesced edits.
    ///
    /// One gesture is one undo entry: every frame of a drag goes out under the
    /// CC5 §6 coalesce key for what was grabbed, and a move — which writes two
    /// parameters — uses the multi-operation live push.
    fn handle_matte_pointer(
        &mut self,
        response: &egui::Response,
        image_rect: egui::Rect,
        context: &MatteOverlayContext,
    ) {
        let mut edits = InspectorEdits::default();
        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some((window, hit)) = self.matte_hit(pointer, image_rect, context)
        {
            edits.begin_gesture();
            self.matte_overlay.begin_drag(MatteDrag {
                target: context.target,
                window,
                hit,
                start: context.matte.windows[window],
                start_pointer: pointer,
            });
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some((window, _)) = self.matte_hit(pointer, image_rect, context)
        {
            self.matte_overlay
                .select_window(window, context.matte.window_count);
        }
        if let Some(drag) = self.matte_overlay.drag()
            && drag.target == context.target
            && (response.dragged() || response.drag_stopped())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let next =
                crate::matte_overlay_ui::drag_to_params(&drag, pointer, context.frame(image_rect));
            // A frame that asks for the values the document already holds is
            // not an edit: the press frame, and any frame the pointer has not
            // moved far enough to change a basis point, write nothing.
            if next != context.matte.windows[drag.window] {
                let operations = matte_window_drag_operations(
                    context.target.clip,
                    context.target.effect,
                    drag.window,
                    drag.hit,
                    &next,
                );
                edits.extend_live(
                    operations,
                    matte_gesture_coalesce_key(
                        drag.hit,
                        context.target.clip,
                        context.target.effect,
                        drag.window,
                    ),
                );
            }
        }
        if response.drag_stopped() {
            self.matte_overlay.end_drag();
        }
        self.submit_inspector_edits(edits);
    }

    /// The window a pointer grabbed, testing the selected window first so a
    /// window drawn under another stays reachable.
    ///
    /// Only the selected window offers handles and a rotation arm, because only
    /// the selected window draws them: an unselected window is select-then-edit
    /// (CC5 §6). The selection is read clamped to the count the document holds
    /// this frame, so it is exactly the one the overlay painted.
    fn matte_hit(
        &self,
        pointer: egui::Pos2,
        image_rect: egui::Rect,
        context: &MatteOverlayContext,
    ) -> Option<(usize, crate::matte_overlay_ui::MatteHit)> {
        matte_hit_test(
            pointer,
            &context.matte,
            context.frame(image_rect),
            self.matte_overlay
                .selected_window(context.matte.window_count),
        )
    }

    /// The Matte view toggle, the window selector, and the coverage status.
    fn matte_viewer_controls(&mut self, ui: &mut egui::Ui, context: &MatteOverlayContext) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("MATTE").font(theme::semibold(type_size::CAPTION)));
            let mut view = self.matte_overlay.matte_view();
            if ui
                .checkbox(&mut view, "Matte view")
                .on_hover_text(
                    "Show this node's coverage instead of the picture: white is fully \
                     affected, black is untouched (CC5 §4.1).",
                )
                .changed()
            {
                self.matte_overlay.set_matte_view(view);
            }
            let selected = self
                .matte_overlay
                .selected_window(context.matte.window_count);
            for index in 0..context.matte.window_count {
                if ui
                    .selectable_label(selected == Some(index), format!("W{index}"))
                    .clicked()
                {
                    self.matte_overlay
                        .select_window(index, context.matte.window_count);
                }
            }
            if context.matte.window_count == 0 {
                ui.colored_label(color::TEXT_MUTED, "No windows to draw");
            }
            match self.matte_overlay.view_status(context.key) {
                MatteViewStatus::Off => {}
                MatteViewStatus::Pending => {
                    ui.colored_label(color::STATUS_WARNING, "Rendering coverage…");
                }
                MatteViewStatus::Ready => {
                    ui.colored_label(color::STATUS_SUCCESS, "Coverage");
                }
                MatteViewStatus::Unavailable(message) => {
                    ui.colored_label(
                        color::STATUS_DANGER,
                        format!("Matte view unavailable: {message}"),
                    );
                }
            }
        });
        ui.colored_label(
            color::TEXT_MUTED,
            "Windows are stored in the layer's own frame and drawn through this clip's \
             resolved transform, so a reframed clip's outline sits on its reframed \
             picture and a drag writes layer coordinates (CC5 §3.3, §5.2).",
        );
    }

    /// Fetch, cache, and upload the coverage image behind the Matte view.
    ///
    /// The render happens on a worker thread through [`MatteProofSource`], with
    /// the scope panel's single-flight policy: at most one `FrameRenderer` and
    /// its cache budget exist at a time, and the newest request wins.
    ///
    /// [`MatteProofSource`]: crate::matte_overlay_ui::MatteProofSource
    fn matte_view_texture(
        &mut self,
        ctx: &egui::Context,
        blocked: bool,
        context: &MatteOverlayContext,
    ) -> Option<egui::TextureHandle> {
        if !self.matte_overlay.matte_view() {
            return None;
        }
        let key = context.key;
        // Two `Arc` clones a frame, and the state decides whether a worker is
        // wanted: the "is this frame blocked?" rule lives in one place rather
        // than being restated by every caller.
        let source = Arc::new(AnalysisMatteProofSource(Arc::clone(&self.analysis)));
        let document = Arc::clone(&self.focused().document);
        self.matte_overlay
            .request_view_if_needed(blocked, source, document, key);
        if let Some(texture) = self.matte_overlay.texture_for(key) {
            return Some(texture.clone());
        }
        let image = coverage_color_image(self.matte_overlay.coverage_for(key)?);
        // Point sampling: a coverage code is evidence, and a bilinear filter
        // would invent partial coverage that no pixel has.
        let texture = ctx.load_texture("matte-coverage", image, egui::TextureOptions::NEAREST);
        self.matte_overlay.set_texture(key, texture.clone());
        Some(texture)
    }

    fn paint_empty_viewer(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        label: &str,
        message: &str,
        message_color: egui::Color32,
    ) {
        Self::paint_viewer_frame(
            ui,
            size,
            label,
            None,
            message,
            message_color,
            egui::Sense::hover(),
        );
    }

    fn paint_viewer_frame(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        label: &str,
        texture: Option<&egui::TextureHandle>,
        message: &str,
        message_color: egui::Color32,
        sense: egui::Sense,
    ) -> ViewerFrame {
        let (rect, response) = ui.allocate_exact_size(size, sense);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, radius::SM, color::LETTERBOX);
        theme::paint_inset_well(&painter, rect, radius::px(radius::SM));
        theme::paint_caps(
            &painter,
            rect.left_top() + egui::vec2(space::TWO, space::TWO),
            egui::Align2::LEFT_TOP,
            label,
            color::TEXT_MUTED,
        );
        if let Some(texture) = texture {
            let source = texture.size_vec2();
            if source.x > 0.0 && source.y > 0.0 {
                let inset = rect.shrink2(egui::vec2(space::FOUR, space::FOUR));
                let scale = (inset.width() / source.x).min(inset.height() / source.y);
                let image_rect = egui::Rect::from_center_size(inset.center(), source * scale);
                painter.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                return ViewerFrame {
                    response,
                    image_rect: Some(image_rect),
                };
            }
        }
        let inset = rect.shrink2(egui::vec2(space::FOUR, space::FOUR));
        painter.rect_stroke(
            inset,
            radius::XS,
            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
            egui::StrokeKind::Inside,
        );
        Icon::Filmstrip
            .image(24.0)
            .tint(color::TEXT_MUTED)
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    inset.center() - egui::vec2(0.0, 10.0),
                    egui::vec2(24.0, 24.0),
                ),
            );
        painter.text(
            inset.center() + egui::vec2(0.0, 16.0),
            egui::Align2::CENTER_CENTER,
            message,
            egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
            message_color,
        );
        ViewerFrame {
            response,
            image_rect: None,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn source_controls(
        &mut self,
        ui: &mut egui::Ui,
        asset: &kinewright_core::MediaAsset,
        source_state: crate::media_workflow::SourceDisplayState,
        revalidation_pending: bool,
    ) {
        let duration = asset.duration.0.max(0);
        let max_frame = duration.saturating_sub(1);
        let session = self.focused();
        let mut source_position = session.source_position.0.clamp(0, max_frame);
        let mut source_in = session.source_in.0.clamp(0, max_frame);
        let mut source_out = session.source_out.0.clamp(0, duration);
        if duration > 0 {
            source_out = source_out.max(source_in.saturating_add(1)).min(duration);
        } else {
            source_in = 0;
            source_out = 0;
        }
        let mut video_target = session.source_video_target;
        let mut audio_target = session.source_audio_target;
        if revalidation_pending {
            ui.colored_label(
                color::STATUS_WARNING,
                "Verifying source before edit… source controls are temporarily locked",
            );
        }
        ui.add_enabled_ui(!revalidation_pending, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("−1").clicked() {
                    source_position = source_position.saturating_sub(1);
                }
                if ui.small_button("+1").clicked() {
                    source_position = source_position.saturating_add(1).min(max_frame);
                }
                ui.label(format!("Frame {source_position}/{max_frame}"));
            });
            ui.add(
                egui::Slider::new(&mut source_position, 0..=max_frame)
                    .text("Source")
                    .show_value(false),
            );
            ui.horizontal(|ui| {
                if ui.button("Mark In").clicked() && duration > 0 {
                    source_in = source_position.min(duration.saturating_sub(1));
                    source_out = source_out.max(source_in.saturating_add(1)).min(duration);
                }
                if ui.button("Mark Out").clicked() && duration > 0 {
                    source_out = source_position
                        .saturating_add(1)
                        .clamp(source_in.saturating_add(1), duration);
                }
                ui.label("In");
                ui.add(egui::DragValue::new(&mut source_in).range(0..=max_frame));
                ui.label("Out");
                ui.add(egui::DragValue::new(&mut source_out).range(1..=duration.max(1)));
            });
            if duration > 0 {
                source_in = source_in.clamp(0, max_frame);
                source_out = source_out.clamp(source_in.saturating_add(1), duration);
            }

            ui.separator();
            ui.label(
                egui::RichText::new("PATCH DESTINATIONS").font(theme::semibold(type_size::CAPTION)),
            );
            let video_enabled = asset.kind.supports(TrackKind::Video);
            let audio_enabled = asset.kind.supports(TrackKind::Audio);
            Self::route_selector(
                self,
                ui,
                "Video",
                TrackKind::Video,
                video_enabled,
                &mut video_target,
            );
            Self::route_selector(
                self,
                ui,
                "Audio",
                TrackKind::Audio,
                audio_enabled,
                &mut audio_target,
            );
            let route_valid = self.valid_route(asset.kind, video_target, audio_target);
            let can_edit = source_edit_controls_are_enabled(
                source_state,
                duration,
                source_in,
                source_out,
                route_valid,
                revalidation_pending,
            );

            // Persist the exact interactive context before dispatching. The
            // async completion compares it with the live session again.
            let session = self.focused_mut();
            session.source_position = TimeCode(source_position);
            session.source_in = TimeCode(source_in);
            session.source_out = TimeCode(source_out);
            session.source_video_target = video_target;
            session.source_audio_target = audio_target;

            ui.horizontal(|ui| {
                for (label, mode) in [
                    ("Insert", ThreePointMode::Insert),
                    ("Overwrite", ThreePointMode::Overwrite),
                ] {
                    if ui.add_enabled(can_edit, egui::Button::new(label)).clicked() {
                        self.dispatch_source_edit(
                            asset.id,
                            source_in,
                            source_out,
                            video_target,
                            audio_target,
                            mode,
                        );
                    }
                }
                if !route_valid {
                    ui.colored_label(
                        color::STATUS_WARNING,
                        "Choose at least one compatible route",
                    );
                }
            });
        });
        self.focused_mut().source_position = TimeCode(source_position);
        self.focused_mut().source_in = TimeCode(source_in);
        self.focused_mut().source_out = TimeCode(source_out);
        self.focused_mut().source_video_target = video_target;
        self.focused_mut().source_audio_target = audio_target;
    }

    fn route_selector(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        kind: TrackKind,
        enabled: bool,
        target: &mut Option<TrackId>,
    ) {
        let tracks = self
            .focused()
            .document
            .tracks
            .iter()
            .filter(|track| track.kind == kind)
            .map(|track| track.id)
            .collect::<Vec<_>>();
        if !enabled {
            *target = None;
        }
        let selected = (*target).filter(|id| tracks.contains(id));
        *target = selected;
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(("source-route", label))
                .selected_text(
                    selected.map_or_else(|| "Off".to_owned(), |id| format!("{kind:?} · {id}")),
                )
                .show_ui(ui, |ui| {
                    if ui.selectable_label(selected.is_none(), "Off").clicked() {
                        *target = None;
                        ui.close();
                    }
                    for track in &tracks {
                        if ui
                            .selectable_label(
                                Some(*track) == selected,
                                format!("{kind:?} · {track}"),
                            )
                            .clicked()
                        {
                            *target = Some(*track);
                            ui.close();
                        }
                    }
                });
        });
    }

    fn valid_route(
        &self,
        kind: MediaKind,
        video_target: Option<TrackId>,
        audio_target: Option<TrackId>,
    ) -> bool {
        patch_routes_valid(&self.focused().document, kind, video_target, audio_target)
    }
}

fn patch_routes_valid(
    document: &kinewright_core::Document,
    kind: MediaKind,
    video_target: Option<TrackId>,
    audio_target: Option<TrackId>,
) -> bool {
    let valid_or_off = |target: Option<TrackId>, track_kind: TrackKind| {
        target.is_none_or(|target| {
            kind.supports(track_kind)
                && document
                    .tracks
                    .iter()
                    .any(|track| track.id == target && track.kind == track_kind)
        })
    };
    (video_target.is_some() || audio_target.is_some())
        && valid_or_off(video_target, TrackKind::Video)
        && valid_or_off(audio_target, TrackKind::Audio)
}

impl KinewrightApp {
    fn dispatch_source_edit(
        &mut self,
        asset: kinewright_core::AssetId,
        source_in: i64,
        source_out: i64,
        video_target: Option<TrackId>,
        audio_target: Option<TrackId>,
        mode: ThreePointMode,
    ) {
        if self.source_edit_revalidation_pending() {
            self.record_error(
                "Source monitor",
                "Source verification is already in progress; wait for it to finish before editing",
            );
            return;
        }
        let Some(current_asset) = self.focused().document.asset(asset).cloned() else {
            self.record_error("Source monitor", "Selected source asset no longer exists");
            return;
        };
        if self.focused().selected_asset != Some(asset) {
            self.record_error(
                "Source monitor",
                "Selected source changed before the edit could be checked",
            );
            return;
        }
        if source_in < 0 || source_out <= source_in || source_out > current_asset.duration.0 {
            self.record_error("Source monitor", "Source In/Out marks are no longer valid");
            return;
        }
        if !self.valid_route(current_asset.kind, video_target, audio_target) {
            self.record_error(
                "Source monitor",
                "Source patch destination is stale or incompatible; choose a current track",
            );
            return;
        }
        let session = self.focused();
        let pending = crate::media_workflow::PendingSourceEdit {
            session_id: session.id,
            request_id: 0,
            asset_id: asset,
            path: current_asset.path.clone(),
            fingerprint: current_asset.source_fingerprint.clone(),
            expected_revision: session.revision,
            selected_asset: session.selected_asset,
            source_position: session.source_position,
            timeline_in: session.position,
            source_in: TimeCode(source_in),
            source_out: TimeCode(source_out),
            video_target,
            audio_target,
            mode,
        };
        let Some(request_id) = self.force_source_edit_media_revalidation(&current_asset) else {
            self.record_error(
                "Source monitor",
                "Could not start mandatory source verification; no edit was applied",
            );
            return;
        };
        self.pending_source_edit = Some(crate::media_workflow::PendingSourceEdit {
            request_id,
            ..pending
        });
        self.status = format!(
            "Verifying Source before {mode:?} at revision {}…",
            self.focused().revision
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{Document, Track};

    fn document() -> Document {
        Document {
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
                    clips: Vec::new(),
                },
            ],
            ..Document::default()
        }
    }

    #[test]
    fn patch_routes_require_one_route_and_reject_stale_or_wrong_kind_routes() {
        let document = document();
        assert!(!patch_routes_valid(
            &document,
            MediaKind::AudioVideo,
            None,
            None
        ));
        assert!(patch_routes_valid(
            &document,
            MediaKind::AudioVideo,
            Some(TrackId(1)),
            Some(TrackId(2))
        ));
        assert!(patch_routes_valid(
            &document,
            MediaKind::Video,
            Some(TrackId(1)),
            None
        ));
        assert!(!patch_routes_valid(
            &document,
            MediaKind::Video,
            Some(TrackId(1)),
            Some(TrackId(2))
        ));
        assert!(!patch_routes_valid(
            &document,
            MediaKind::AudioVideo,
            Some(TrackId(99)),
            Some(TrackId(2))
        ));
    }

    /// CC5 §6: the viewer takes pointer input only while a matte section is
    /// expanded. The decision is a pure function so it is provable without a
    /// window, and so the "otherwise unchanged" half is a test rather than a
    /// promise.
    #[test]
    fn the_viewer_takes_pointer_input_only_for_an_expanded_matte_section() {
        let idle = viewer_sense(false);
        assert!(!idle.senses_click(), "an idle viewer is not clickable");
        assert!(!idle.senses_drag(), "an idle viewer is not draggable");
        assert_eq!(idle, egui::Sense::hover(), "today's behaviour, exactly");

        let editing = viewer_sense(true);
        assert!(editing.senses_click());
        assert!(editing.senses_drag());
        assert_eq!(editing, egui::Sense::click_and_drag());
    }

    // -----------------------------------------------------------------------
    // CC5 §6 overlay context
    // -----------------------------------------------------------------------

    const CLIP: kinewright_core::ClipId = kinewright_core::ClipId(10);
    const EFFECT: kinewright_core::EffectId = kinewright_core::EffectId(1);

    /// A one-clip 1920 × 1080 document whose clip starts at `timeline_start`,
    /// runs 30 frames, and carries a `color_wheels` node plus whatever
    /// `transform` percentages are asked for.
    fn matte_document(timeline_start: TimeCode, transform: &[(&str, i64)]) -> Document {
        use std::collections::BTreeMap;

        use kinewright_core::{
            AssetId, ClipContent, Effect, EffectId, MediaAsset, ParamValue, Rational,
        };

        let mut effects = vec![Effect {
            id: EFFECT,
            name: "color_wheels".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        }];
        if !transform.is_empty() {
            effects.push(Effect {
                id: EffectId(2),
                name: "transform".to_owned(),
                parameters: transform
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
                    .collect(),
                keyframes: BTreeMap::new(),
            });
        }
        let mut document = document();
        document.resolution = (1920, 1080);
        document.media_pool = vec![MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from("picture.mov"),
            name: "Picture".to_owned(),
            duration: TimeCode(120),
            fps: Rational::new(30, 1).expect("valid fps"),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        }];
        document.tracks[0].clips = vec![kinewright_core::Clip {
            id: CLIP,
            asset: AssetId(1),
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start,
            effects,
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }];
        document.duration = TimeCode(timeline_start.0 + 30);
        document.validate().expect("the fixture is a legal project");
        document
    }

    fn context_at(document: &Document, position: TimeCode) -> Option<MatteOverlayContext> {
        matte_overlay_context_for(
            document,
            Some(CLIP),
            position,
            MatteTarget::new(CLIP, EFFECT),
            1,
            0,
        )
    }

    /// CC5 §6: the overlay belongs to the clip under the playhead. With the
    /// playhead off the clip the renderer is showing somebody else's picture,
    /// and `checked_sub`'s old `unwrap_or(ZERO)` fallback drew this node's
    /// windows over it — and let a drag edit a matte nobody could see.
    #[test]
    fn no_overlay_when_the_playhead_is_off_the_selected_clip() {
        // The clip occupies timeline frames 20..50.
        let document = matte_document(TimeCode(20), &[]);

        assert!(
            context_at(&document, TimeCode(19)).is_none(),
            "one frame before the clip is not on the clip"
        );
        assert!(
            context_at(&document, TimeCode(20)).is_some(),
            "the first frame of the clip is"
        );
        assert!(
            context_at(&document, TimeCode(49)).is_some(),
            "and so is the last"
        );
        assert!(
            context_at(&document, TimeCode(50)).is_none(),
            "the end is exclusive: frame 50 belongs to whatever follows"
        );
        assert!(
            context_at(&document, TimeCode(500)).is_none(),
            "and a playhead far past the clip is not a clip-local frame 0"
        );

        // A clip that is not the selected one never yields a context either.
        assert!(
            matte_overlay_context_for(
                &document,
                Some(kinewright_core::ClipId(99)),
                TimeCode(25),
                MatteTarget::new(CLIP, EFFECT),
                1,
                0,
            )
            .is_none()
        );
    }

    /// CC5 §5.2: the overlay resolves the clip's own `transform` at the same
    /// clip-local frame as the matte, in the compositor's units — `scale` a
    /// product of `scale_percent / 100`, the offsets sums of `percent / 50`.
    #[test]
    fn the_overlay_context_resolves_the_layer_transform() {
        let plain = matte_document(TimeCode::ZERO, &[]);
        assert_eq!(
            context_at(&plain, TimeCode(3))
                .expect("a context")
                .transform,
            LayerTransform::IDENTITY,
            "an unreframed clip is the identity, so CC4 projects are unchanged"
        );

        let reframed = matte_document(
            TimeCode(20),
            &[("scale_percent", 50), ("x_percent", 25), ("y_percent", -10)],
        );
        let context = context_at(&reframed, TimeCode(25)).expect("a context");
        assert!(
            (context.transform.scale - 0.5).abs() < 1e-12
                && (context.transform.offset_x - 0.5).abs() < 1e-12
                && (context.transform.offset_y + 0.2).abs() < 1e-12,
            "resolved transform: {:?}",
            context.transform
        );

        // And it reaches the geometry: the window centre is drawn where the
        // reframed picture puts it, not where the raster centre is.
        let image_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(64.0, 36.0));
        let centre = crate::matte_overlay_ui::window_centre_point(
            &context.matte.windows[0],
            context.frame(image_rect),
        );
        assert!(
            (centre.x - 48.0).abs() < 1e-3 && (centre.y - 14.4).abs() < 1e-3,
            "reframed centre: {centre:?}"
        );
    }

    /// CC5 §4.1 and the source-verification contract: a blocked source shows no
    /// picture at all. The Matte view renders the same media through the same
    /// decoder, so it must not become the way that picture reaches the screen.
    #[test]
    fn a_blocked_source_shows_no_picture_not_even_a_matte_view() {
        let matte = 1_u8;
        let mask = 3_u8;
        let texture = 2_u8;

        assert_eq!(
            viewer_picture(false, Some(&matte), None, Some(&texture)),
            Some(&matte),
            "an available coverage stands in for the picture"
        );
        assert_eq!(
            viewer_picture(false, None, None, Some(&texture)),
            Some(&texture),
            "and the picture is shown when there is no coverage"
        );
        assert_eq!(
            viewer_picture(true, Some(&matte), Some(&mask), Some(&texture)),
            None,
            "a blocked source is blocked, coverage or not"
        );
        assert_eq!(viewer_picture(true, None, None, Some(&texture)), None);
        assert_eq!(viewer_picture::<u8>(false, None, None, None), None);

        // CC6 §8.2 precedence, stated as a test rather than as a comment.
        assert_eq!(
            viewer_picture(false, Some(&matte), Some(&mask), Some(&texture)),
            Some(&matte),
            "with both whole-picture views on, the matte view wins"
        );
        assert_eq!(
            viewer_picture(false, None, Some(&mask), Some(&texture)),
            Some(&mask),
            "and the QC mask replaces the picture when the matte view is off"
        );
    }

    // -----------------------------------------------------------------------
    // CC6 §8.2 QC clipping mask
    // -----------------------------------------------------------------------

    /// The half-luma grey a hand-computed in-range pixel must take.
    ///
    /// Transcribed independently of `qc_mask_image`: the linear luma, the
    /// BT.709 delivery transfer written out from CC6 §3.2's definition, then
    /// the integer code and the integer halving.
    /// CC6 §3.2's transfer, transcribed in `f32` rather than called: the
    /// sign-preserving odd extension wraps the *whole* function, so a
    /// magnitude at or above the seam takes the power branch on either side of
    /// zero.
    fn expected_transfer(linear: f32) -> f32 {
        if linear < 0.0 {
            -expected_transfer(-linear)
        } else if linear < 0.018_f32 {
            4.5_f32 * linear
        } else {
            1.099_f32 * linear.powf(0.45_f32) - 0.099_f32
        }
    }

    fn expected_grey(linear: [f32; 3]) -> u8 {
        let luma = 0.2126_f32 * linear[0] + 0.7152_f32 * linear[1] + 0.0722_f32 * linear[2];
        let encoded = expected_transfer(luma);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let code = (255.0 * f64::from(encoded).clamp(0.0, 1.0)).round() as u8;
        code / 2
    }

    /// CC6 §8.2 and §11.2.21: every flagged pixel takes its flag colour, every
    /// unflagged pixel takes the hand-computed half-luma grey, and a pixel
    /// that is both over and under takes the under colour.
    #[test]
    fn cc6_qc_mask_marks_only_the_flagged_pixels() {
        // Six hand-built linear pixels, in row-major order:
        //   0: mid grey, in range
        //   1: over range on green only
        //   2: under range on blue only
        //   3: over on red AND under on blue — the precedence case
        //   4: black, exactly on the lower bound (e = 0, not an excursion)
        //   5: nominal white, exactly on the upper bound (e = 1, strict >)
        let source = [
            [0.18_f32, 0.18, 0.18],
            [0.5, 1.4, 0.5],
            [0.5, 0.5, -0.05],
            [1.4, 0.5, -0.05],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
        ];
        let mut pixels = Vec::new();
        for linear in source {
            pixels.extend_from_slice(&[linear[0], linear[1], linear[2], 1.0]);
        }
        let image = kinewright_core::LinearRgbaImage {
            width: 6,
            height: 1,
            pixels,
        };

        let mask = qc_mask_image(&image);
        assert_eq!((mask.width, mask.height), (6, 1));
        assert_eq!(mask.pixels.len(), 6 * 4, "one RGBA quad per source pixel");
        let pixel = |index: usize| -> [u8; 4] {
            let start = index * 4;
            [
                mask.pixels[start],
                mask.pixels[start + 1],
                mask.pixels[start + 2],
                mask.pixels[start + 3],
            ]
        };

        for (index, linear) in [(0_usize, source[0]), (4, source[4]), (5, source[5])] {
            let grey = expected_grey(linear);
            assert_eq!(
                pixel(index),
                [grey, grey, grey, 255],
                "pixel {index} is in range and must be the half-luma grey"
            );
            assert_ne!(pixel(index), QC_MASK_OVER_RANGE_COLOR);
            assert_ne!(pixel(index), QC_MASK_UNDER_RANGE_COLOR);
        }
        assert_eq!(
            pixel(1),
            QC_MASK_OVER_RANGE_COLOR,
            "an encoded channel above 1.0 is red"
        );
        assert_eq!(
            pixel(2),
            QC_MASK_UNDER_RANGE_COLOR,
            "a negative linear channel is blue"
        );
        assert_eq!(
            pixel(3),
            QC_MASK_UNDER_RANGE_COLOR,
            "under wins over over: a negative channel is the unrecoverable one"
        );
        // The two flags are distinguishable, which is the whole point.
        assert_ne!(QC_MASK_OVER_RANGE_COLOR, QC_MASK_UNDER_RANGE_COLOR);
    }

    /// A source that counts its renders, or panics instead of rendering.
    struct MaskSource {
        renders: Arc<std::sync::atomic::AtomicU64>,
        panics: bool,
    }

    impl MaskSource {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                renders: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                panics: false,
            })
        }

        fn panicking() -> Arc<Self> {
            Arc::new(Self {
                renders: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                panics: true,
            })
        }

        fn render_count(&self) -> u64 {
            self.renders.load(std::sync::atomic::Ordering::Acquire)
        }
    }

    impl ColorQcSource for MaskSource {
        fn working_proof(
            &self,
            _document: Arc<kinewright_core::Document>,
            _key: WorkingProofKey,
        ) -> Result<kinewright_core::WorkingProof, String> {
            assert!(!self.panics, "the renderer fell over");
            self.renders
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Ok(kinewright_core::WorkingProof {
                image: kinewright_core::LinearRgbaImage {
                    width: 1,
                    height: 1,
                    pixels: vec![0.5, 0.5, 0.5, 1.0],
                },
                metadata: kinewright_core::WorkingProofMetadata {
                    render: kinewright_core::MonitorProofMetadata::test_double(),
                    stage: kinewright_core::WORKING_PROOF_STAGE.to_owned(),
                    encoding: kinewright_core::WORKING_PROOF_ENCODING.to_owned(),
                    raster_aspect_millionths: 1_000_000,
                },
            })
        }

        fn measure_with_nodes(
            &self,
            _document: Arc<kinewright_core::Document>,
            _key: WorkingProofKey,
            _request: &kinewright_core::ColorQcRequest,
        ) -> Result<
            (
                kinewright_core::ColorQcReport,
                kinewright_core::WorkingProofMetadata,
            ),
            String,
        > {
            unreachable!("the mask never measures a report");
        }
    }

    fn mask_key() -> QcMaskKey {
        QcMaskKey {
            session_id: 1,
            revision: 7,
            frame: TimeCode(3),
        }
    }

    fn settle(state: &mut QcMaskState, key: QcMaskKey, conditions: QcMaskConditions) {
        for _ in 0..2_000 {
            state.poll();
            if !state.is_pending() {
                return;
            }
            let _ = state.status(key, conditions);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the QC mask worker never settled");
    }

    /// CC6 §8.2: switching the mask on says a render is coming, never that one
    /// is unavailable.
    ///
    /// The toggle is read in the controls, after the frame's texture pass has
    /// already run. A status computed from "nothing has been requested" put a
    /// red `QC mask unavailable` under the viewer for exactly one frame on
    /// every switch-on — the frame the operator is looking at when they click.
    #[test]
    fn switching_the_mask_on_never_reads_as_unavailable() {
        let conditions = QcMaskConditions::default();
        let mut state = QcMaskState::default();
        assert_eq!(state.status(mask_key(), conditions), QcMaskStatus::Off);

        state.set_view(QcMaskView::Clipping);
        assert_eq!(
            state.status(mask_key(), conditions),
            QcMaskStatus::Pending,
            "the frame the mask is switched on reports the render it is about to make"
        );

        let source = MaskSource::new();
        assert!(state.request_view_if_needed(
            conditions,
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            Arc::default(),
            Arc::new(kinewright_core::Document::default()),
            mask_key(),
        ));
        assert_eq!(state.status(mask_key(), conditions), QcMaskStatus::Pending);
        settle(&mut state, mask_key(), conditions);
        assert_eq!(state.status(mask_key(), conditions), QcMaskStatus::Ready);

        // And a real refusal still reads as one rather than as a spinner.
        for (conditions, expected) in [
            (
                QcMaskConditions {
                    blocked: true,
                    ..QcMaskConditions::default()
                },
                QcMaskStatus::Blocked,
            ),
            (
                QcMaskConditions {
                    behind_matte_view: true,
                    ..QcMaskConditions::default()
                },
                QcMaskStatus::BehindMatteView,
            ),
        ] {
            assert_eq!(state.status(mask_key(), conditions), expected);
        }
    }

    /// CC6 §8.2: the mask is a paused-only view.
    ///
    /// One full-resolution working proof per frame identity cannot keep up
    /// with the transport, and every one of them would describe a frame that
    /// has already gone by. Playback therefore withholds the *render*, and the
    /// status line says which button to press.
    #[test]
    fn playback_withholds_the_mask_render_and_says_so() {
        let playing = QcMaskConditions {
            playing: true,
            ..QcMaskConditions::default()
        };
        let paused = QcMaskConditions::default();
        let source = MaskSource::new();
        let mut state = QcMaskState::default();
        state.set_view(QcMaskView::Clipping);

        assert!(
            !state.request_view_if_needed(
                playing,
                Arc::clone(&source) as Arc<dyn ColorQcSource>,
                Arc::default(),
                Arc::new(kinewright_core::Document::default()),
                mask_key(),
            ),
            "nothing is asked for while the transport is running"
        );
        assert!(!state.is_pending(), "and nothing is in flight");
        assert_eq!(source.render_count(), 0, "and nothing was rendered");
        assert_eq!(
            state.status(mask_key(), playing),
            QcMaskStatus::PausedOnly,
            "the status says the mask is a paused-only view"
        );
        assert!(
            QC_MASK_PAUSED_ONLY.starts_with("Paused only"),
            "and it leads with the condition, not with the reason"
        );

        // Pausing asks for exactly one render.
        assert!(state.request_view_if_needed(
            paused,
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            Arc::default(),
            Arc::new(kinewright_core::Document::default()),
            mask_key(),
        ));
        settle(&mut state, mask_key(), paused);
        assert_eq!(state.status(mask_key(), paused), QcMaskStatus::Ready);
        assert_eq!(source.render_count(), 1);
    }

    /// CC6 §8.2: a scrub withholds the render for the same reason playback
    /// does, and `playing` cannot say so.
    ///
    /// Both scrub paths pause the transport on drag start and resume it on
    /// drag stop, so `playing` is `false` for the whole drag. The mask read
    /// that as a still frame and queued one full-resolution working proof per
    /// pointer sample — every one of them describing a frame the pointer had
    /// already left.
    #[test]
    fn scrubbing_withholds_the_mask_render_the_way_playback_does() {
        let source = MaskSource::new();
        let mut state = QcMaskState::default();
        state.set_view(QcMaskView::Clipping);
        assert!(!state.is_scrubbing(), "nothing is being dragged yet");

        // Drag start. The transport is paused throughout, so the only thing
        // that knows a scrub is running is the flag the drag sets.
        state.set_scrubbing(true);
        assert!(state.is_scrubbing());
        let scrubbing = QcMaskConditions {
            playing: false,
            scrubbing: state.is_scrubbing(),
            ..QcMaskConditions::default()
        };
        for frame in 0..4 {
            let key = QcMaskKey {
                frame: TimeCode(frame),
                ..mask_key()
            };
            assert!(
                !state.request_view_if_needed(
                    scrubbing,
                    Arc::clone(&source) as Arc<dyn ColorQcSource>,
                    Arc::default(),
                    Arc::new(kinewright_core::Document::default()),
                    key,
                ),
                "a dragging playhead asks for nothing, at any frame it passes"
            );
            assert_eq!(
                state.status(key, scrubbing),
                QcMaskStatus::PausedOnly,
                "and it reads as the paused-only view it is, not as a spinner"
            );
        }
        assert!(!state.is_pending(), "nothing is in flight");
        assert_eq!(source.render_count(), 0, "and nothing was rendered");

        // Drag stop. One render, for the frame the playhead actually landed on.
        state.set_scrubbing(false);
        let settled = QcMaskConditions {
            scrubbing: state.is_scrubbing(),
            ..QcMaskConditions::default()
        };
        assert!(state.request_view_if_needed(
            settled,
            Arc::clone(&source) as Arc<dyn ColorQcSource>,
            Arc::default(),
            Arc::new(kinewright_core::Document::default()),
            mask_key(),
        ));
        settle(&mut state, mask_key(), settled);
        assert_eq!(state.status(mask_key(), settled), QcMaskStatus::Ready);
        assert_eq!(source.render_count(), 1);
    }

    /// A mask worker that unwinds resolves the view with an error rather than
    /// wedging it on a render that will never land.
    #[test]
    fn a_panicking_mask_render_resolves_the_view_with_an_error() {
        let conditions = QcMaskConditions::default();
        let mut state = QcMaskState::default();
        state.set_view(QcMaskView::Clipping);
        state.request_view(
            MaskSource::panicking() as Arc<dyn ColorQcSource>,
            Arc::default(),
            Arc::new(kinewright_core::Document::default()),
            mask_key(),
        );
        settle(&mut state, mask_key(), conditions);

        assert!(!state.is_pending(), "nothing is in flight");
        let QcMaskStatus::Unavailable(message) = state.status(mask_key(), conditions) else {
            panic!("a panicking render is reported as unavailable");
        };
        assert!(
            message.contains("stopped before it delivered"),
            "the message says what happened: {message}"
        );
    }

    /// CC6 §8.2 and §3.1: a non-finite sample is not evidence of clipping, and
    /// the mask draws exactly the classification core counts.
    ///
    /// The mixed pixel is the one that matters: core discards the **whole**
    /// pixel the moment any channel is non-finite, so drawing its finite
    /// over-range channel red would show the operator an excursion that no
    /// report counts.
    #[test]
    fn a_non_finite_sample_is_black_and_core_counts_it_the_same_way() {
        // Six hand-built pixels, in row-major order:
        //   0: one NaN channel, the rest in range
        //   1: every channel NaN
        //   2: a NaN channel beside a genuinely over-range one
        //   3: an infinity beside a genuinely negative one
        //   4: finite and over range
        //   5: finite and negative
        let source = [
            [f32::NAN, 0.5, 0.5],
            [f32::NAN, f32::NAN, f32::NAN],
            [f32::NAN, 1.4, 0.5],
            [f32::INFINITY, -0.05, 0.5],
            [1.4, 0.5, 0.5],
            [0.5, -0.05, 0.5],
        ];
        let mut pixels = Vec::new();
        for linear in source {
            pixels.extend_from_slice(&[linear[0], linear[1], linear[2], 1.0]);
        }
        let image = kinewright_core::LinearRgbaImage {
            width: 6,
            height: 1,
            pixels,
        };

        let mask = qc_mask_image(&image);
        let pixel = |index: usize| &mask.pixels[index * 4..index * 4 + 4];
        for index in 0..4 {
            assert_eq!(
                pixel(index),
                [0, 0, 0, 255],
                "pixel {index} carries a non-finite channel and is not a clipping flag"
            );
        }
        assert_eq!(
            pixel(4),
            QC_MASK_OVER_RANGE_COLOR,
            "a finite over-range pixel still reads as one"
        );
        assert_eq!(
            pixel(5),
            QC_MASK_UNDER_RANGE_COLOR,
            "and so does a finite negative one"
        );

        // Core's own classification of the same raster.
        let report = kinewright_core::measure_color_qc(
            &kinewright_core::WorkingProof {
                image,
                metadata: kinewright_core::WorkingProofMetadata {
                    render: kinewright_core::MonitorProofMetadata::test_double(),
                    stage: kinewright_core::WORKING_PROOF_STAGE.to_owned(),
                    encoding: kinewright_core::WORKING_PROOF_ENCODING.to_owned(),
                    raster_aspect_millionths: 6_000_000,
                },
            },
            &kinewright_core::ColorQcRequest {
                checks: vec![
                    kinewright_core::ColorQcCheck::Range,
                    kinewright_core::ColorQcCheck::Gamut,
                ],
                ..kinewright_core::ColorQcRequest::default()
            },
        )
        .expect("the raster measures");
        assert_eq!(
            report.non_finite_pixel_count, 4,
            "core discards the four pixels the mask drew black"
        );
        assert_eq!(
            report.range.clamped_pixel_count, 2,
            "and counts exactly the two the mask flagged"
        );
        assert_eq!(
            report.gamut.out_of_gamut_pixel_count, 1,
            "only the finite negative channel is out of gamut"
        );
    }

    /// The mask is uploaded as a texture whose dimensions are asserted against
    /// its buffer, so a truncated readback must not be able to panic the
    /// viewer — and an absent pixel must not be drawn as a clipping flag.
    #[test]
    fn a_truncated_working_proof_still_produces_a_whole_mask() {
        let mask = qc_mask_image(&kinewright_core::LinearRgbaImage {
            width: 4,
            height: 2,
            // Two pixels of samples for an eight-pixel raster.
            pixels: vec![1.4, 0.5, 0.5, 1.0, 0.5, 0.5, -0.05, 1.0],
        });
        assert_eq!(
            mask.pixels.len(),
            4 * 2 * 4,
            "the mask always has one RGBA quad per raster pixel"
        );
        assert_eq!(&mask.pixels[0..4], QC_MASK_OVER_RANGE_COLOR);
        assert_eq!(&mask.pixels[4..8], QC_MASK_UNDER_RANGE_COLOR);
        for index in 2..8 {
            assert_eq!(
                &mask.pixels[index * 4..index * 4 + 4],
                [0, 0, 0, 255],
                "a missing sample is black, not a flag"
            );
        }
        // And it uploads: this is the call that asserts the two agree.
        let _ = coverage_color_image(&mask);
    }

    /// CC6 §8.2: the mask uses the core transfer, not a second transcription.
    #[test]
    fn the_qc_mask_grey_uses_the_core_delivery_transfer() {
        let mut pixels = Vec::new();
        for step in 0..64_u32 {
            let value = f64::from(step) / 63.0;
            #[allow(clippy::cast_possible_truncation)]
            let value = value as f32;
            pixels.extend_from_slice(&[value, value, value, 1.0]);
        }
        let mask = qc_mask_image(&kinewright_core::LinearRgbaImage {
            width: 64,
            height: 1,
            pixels,
        });
        for step in 0..64_usize {
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            let value = (step as f64 / 63.0) as f32;
            assert_eq!(
                mask.pixels[step * 4],
                expected_grey([value, value, value]),
                "step {step} disagrees with the hand-transcribed transfer"
            );
        }
    }
}
