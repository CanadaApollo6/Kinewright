//! Human-facing colour scopes and reference-shot inspection.
//!
//! The panel deliberately has no edit path.  A scope request renders an
//! immutable, full-resolution monitor proof on a worker thread and tags the
//! response with the project, revision, playhead, ROI, and a monotonic
//! generation.  A response that no longer describes the live editor context
//! is discarded before it can reach the paint path.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use eframe::egui;
use kinewright_core::{
    Analysis, Document, MonitorProof, NormalizedRoi, ScopeComparison, ScopeEvidence, ScopeRequest,
    ScopeResolution, ScopeStage, TimeCode, compare_scope_evidence, measure_scope,
};

use crate::{
    app::KinewrightApp,
    theme::{self, color, radius, type_size},
};

const ROI_MAX: u16 = 10_000;

/// One of the three CC2 monitor views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ScopeKind {
    #[default]
    Waveform,
    RgbParade,
    Vectorscope,
}

impl ScopeKind {
    const ALL: [Self; 3] = [Self::Waveform, Self::RgbParade, Self::Vectorscope];

    const fn label(self) -> &'static str {
        match self {
            Self::Waveform => "Waveform",
            Self::RgbParade => "RGB parade",
            Self::Vectorscope => "Vectorscope",
        }
    }
}

/// A geometric region of interest expressed as normalized basis points.
///
/// Basis points avoid a second floating-point coordinate convention in the
/// editor.  `(0, 0, 10_000, 10_000)` is the complete raster and the right and
/// bottom edges are exclusive for sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScopeRoi {
    pub(crate) left: u16,
    pub(crate) top: u16,
    pub(crate) right: u16,
    pub(crate) bottom: u16,
}

impl Default for ScopeRoi {
    fn default() -> Self {
        Self::full_frame()
    }
}

impl ScopeRoi {
    #[must_use]
    pub(crate) const fn full_frame() -> Self {
        Self {
            left: 0,
            top: 0,
            right: ROI_MAX,
            bottom: ROI_MAX,
        }
    }

    /// Normalize potentially reversed/out-of-range controls into a safe ROI.
    ///
    /// Out-of-range fields are clamped per field, never widened to the full
    /// frame, and the clamp is reported so the panel can say it happened
    /// instead of silently measuring a different region than the one typed.
    /// A zero-area or reversed rectangle is still refused.
    #[must_use]
    pub(crate) fn normalize(left: i32, top: i32, right: i32, bottom: i32) -> RoiNormalization {
        let bounds = 0..=i32::from(ROI_MAX);
        let clamped = ![left, top, right, bottom]
            .iter()
            .all(|value| bounds.contains(value));
        let (left, top) = (clamp_basis_points(left), clamp_basis_points(top));
        let (right, bottom) = (clamp_basis_points(right), clamp_basis_points(bottom));
        let roi = (right > left && bottom > top).then_some(Self {
            left,
            top,
            right,
            bottom,
        });
        RoiNormalization { roi, clamped }
    }

    #[must_use]
    fn to_core(self) -> NormalizedRoi {
        NormalizedRoi::new(
            u32::from(self.left),
            u32::from(self.top),
            u32::from(self.right.saturating_sub(self.left)),
            u32::from(self.bottom.saturating_sub(self.top)),
        )
    }
}

fn clamp_basis_points(value: i32) -> u16 {
    u16::try_from(value.clamp(0, i32::from(ROI_MAX))).unwrap_or(ROI_MAX)
}

/// The outcome of normalizing raw ROI controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RoiNormalization {
    /// `None` when the rectangle has no positive area after clamping.
    pub(crate) roi: Option<ScopeRoi>,
    /// True when at least one control was outside `0..=ROI_MAX`.
    pub(crate) clamped: bool,
}

/// A bounded response identity.  It is intentionally independent of the
/// `Document` pointer so tests can prove stale response rejection cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScopeRequestKey {
    pub(crate) session_id: u64,
    pub(crate) revision: u64,
    pub(crate) frame: TimeCode,
    pub(crate) roi: ScopeRoi,
}

/// One accepted full-resolution measurement and its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeMeasurement {
    pub(crate) key: ScopeRequestKey,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) full_resolution: bool,
    pub(crate) stage: ScopeStage,
    pub(crate) backend: String,
    pub(crate) adapter: String,
    pub(crate) software_fallback: bool,
    pub(crate) gpu_claim: bool,
    pub(crate) evidence: ScopeEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReferenceMeasurement {
    measurement: ScopeMeasurement,
}

#[derive(Debug)]
struct ScopeResponse {
    generation: u64,
    key: ScopeRequestKey,
    result: Result<ScopeMeasurement, String>,
}

#[derive(Debug)]
struct PendingScope {
    generation: u64,
    key: ScopeRequestKey,
}

/// The one worker thread the panel is allowed to have in flight.
///
/// Retiring flips the shared flag, which is checked before the render starts
/// and again before the result is delivered. It is **not** a way to interrupt a
/// render that is already running: a `FrameRenderer` holds a large frame cache
/// budget, so a retired worker keeps that memory until it finishes on its own.
/// That is why the panel is single-flight — `queued` holds the newest request
/// until this thread is finished rather than starting a second renderer beside
/// it.
///
/// `handle` is retained so `poll` can tell "still rendering" from "finished and
/// reapable" without ever blocking the UI thread on a join.
#[derive(Debug)]
struct ActiveWorker {
    cancelled: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl ActiveWorker {
    fn retire(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Whether this worker has been superseded. Its result will be dropped
    /// rather than delivered, but it still owns a renderer until it finishes.
    #[cfg(test)]
    fn is_retired(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

/// A request the panel accepted while a worker was still rendering.
///
/// Only the newest one is kept: an intermediate request from a seek the user
/// has already moved past describes a frame nobody is looking at any more, so
/// rendering it would only delay the one they are.
struct QueuedSample {
    generation: u64,
    key: ScopeRequestKey,
    source: Arc<dyn ScopeProofSource>,
    document: Arc<Document>,
    frame: TimeCode,
}

impl std::fmt::Debug for QueuedSample {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueuedSample")
            .field("generation", &self.generation)
            .field("key", &self.key)
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

/// The single blocking operation a scope worker performs.
///
/// Naming it as a trait keeps the panel's single-flight policy testable
/// without standing up a whole `Analysis` backend: a test can supply a source
/// that blocks on command and counts how many renders actually started.
trait ScopeProofSource: Send + Sync + 'static {
    fn monitor_proof(
        &self,
        document: Arc<Document>,
        frame: TimeCode,
    ) -> Result<MonitorProof, String>;
}

/// The production source: the live analysis backend's managed monitor proof.
struct AnalysisProofSource(Arc<dyn Analysis>);

impl ScopeProofSource for AnalysisProofSource {
    fn monitor_proof(
        &self,
        document: Arc<Document>,
        frame: TimeCode,
    ) -> Result<MonitorProof, String> {
        self.0
            .monitor_proof_for_document(document, frame)
            .map_err(|error| error.to_string())
    }
}

/// Makes every live worker resolve the panel exactly once: with its proof, or
/// with an error if the thread unwinds before it delivers one.
struct WorkerCompletion {
    generation: u64,
    key: ScopeRequestKey,
    response_tx: mpsc::Sender<ScopeResponse>,
    cancelled: Arc<AtomicBool>,
    delivered: bool,
}

impl WorkerCompletion {
    fn is_retired(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn deliver(&mut self, result: Result<ScopeMeasurement, String>) {
        if self.delivered {
            return;
        }
        self.delivered = true;
        if self.is_retired() {
            return;
        }
        // The channel is unbounded, so this only fails once the panel that
        // owns the receiver is gone and nothing could display the result.
        let _ = self.response_tx.send(ScopeResponse {
            generation: self.generation,
            key: self.key,
            result,
        });
    }
}

impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        // A panic or an early return must not strand the panel on
        // "Rendering full-resolution proof…" forever.
        self.deliver(Err(
            "the full-resolution scope worker stopped before it delivered a proof".to_owned(),
        ));
    }
}

/// All ephemeral panel state. Nothing here is part of the project document or
/// grade, and no method in this type emits a Core operation.
#[derive(Debug)]
pub(crate) struct ColorScopesState {
    pub(crate) kind: ScopeKind,
    pub(crate) roi: ScopeRoi,
    current_context: Option<ScopeRequestKey>,
    pub(crate) current: Option<ScopeMeasurement>,
    reference: Option<ReferenceMeasurement>,
    pending: Option<PendingScope>,
    active: Option<ActiveWorker>,
    /// The newest request accepted while `active` was still rendering. It
    /// starts as soon as that thread finishes, so at most one `FrameRenderer`
    /// and its cache budget exist at a time.
    queued: Option<QueuedSample>,
    generation: u64,
    response_tx: mpsc::Sender<ScopeResponse>,
    response_rx: mpsc::Receiver<ScopeResponse>,
    pub(crate) error: Option<String>,
    /// True when the last ROI the user typed had to be clamped into range.
    pub(crate) roi_clamped: bool,
    /// How many worker threads this panel has started. Single-flight means it
    /// advances once per render actually begun, not once per request.
    #[cfg(test)]
    spawned_workers: u64,
}

impl Default for ColorScopesState {
    fn default() -> Self {
        // Unbounded: a superseded worker is retired by its cancel flag and
        // filtered by generation, so a full queue must never be able to drop
        // the response the panel is actually waiting for.
        let (response_tx, response_rx) = mpsc::channel();
        Self {
            kind: ScopeKind::default(),
            roi: ScopeRoi::default(),
            current_context: None,
            current: None,
            reference: None,
            pending: None,
            active: None,
            queued: None,
            generation: 0,
            response_tx,
            response_rx,
            error: None,
            roi_clamped: false,
            #[cfg(test)]
            spawned_workers: 0,
        }
    }
}

impl ColorScopesState {
    #[must_use]
    pub(crate) const fn has_reference(&self) -> bool {
        self.reference.is_some()
    }

    #[must_use]
    pub(crate) const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn set_kind(&mut self, kind: ScopeKind) {
        self.kind = kind;
    }

    /// ROI changes invalidate both the current proof and any reference shot;
    /// retaining a measurement from a different geometric region would make a
    /// comparison look valid while measuring different pixels.
    #[must_use]
    pub(crate) fn set_roi(&mut self, roi: ScopeRoi) -> bool {
        if roi.right <= roi.left || roi.bottom <= roi.top {
            return false;
        }
        if self.roi == roi {
            return true;
        }
        self.roi = roi;
        self.invalidate(false);
        if let Some(context) = self.current_context {
            self.current_context = Some(ScopeRequestKey { roi, ..context });
        }
        true
    }

    /// Observe the editor context every frame. A project switch, core revision
    /// change, or playhead move invalidates pending/current evidence while
    /// leaving an explicitly captured reference untouched.
    pub(crate) fn observe_context(&mut self, session_id: u64, revision: u64, frame: TimeCode) {
        let context = ScopeRequestKey {
            session_id,
            revision,
            frame,
            roi: self.roi,
        };
        if let Some(previous) = self.current_context
            && (previous.session_id != context.session_id
                || previous.revision != context.revision
                || previous.frame != context.frame
                || previous.roi != context.roi)
        {
            // A reference is a deliberate shot choice and survives a seek or
            // revision within one project, but it must never leak across
            // independently editable project sessions.
            self.invalidate(previous.session_id == context.session_id);
        }
        self.current_context = Some(context);
    }

    /// Accept one bounded full-resolution proof request. The worker owns the
    /// immutable document snapshot and never touches playback/UI state.
    pub(crate) fn request_sample(
        &mut self,
        analysis: Arc<dyn Analysis>,
        document: Arc<Document>,
        session_id: u64,
        revision: u64,
        frame: TimeCode,
    ) {
        self.request_sample_from(
            Arc::new(AnalysisProofSource(analysis)),
            document,
            session_id,
            revision,
            frame,
        );
    }

    /// Latest-request-wins single flight.
    ///
    /// A request that arrives while a worker is still rendering does **not**
    /// start a second one. The cancel flag cannot interrupt a render already in
    /// progress, so spawning per request would let a seek/sample/seek/sample
    /// burst stack up concurrent `FrameRenderer`s, each holding its own cache
    /// budget. Instead the request is parked in `queued`, replacing any earlier
    /// parked request, and `poll` starts it once the running worker finishes.
    ///
    /// The panel is marked pending immediately either way, so the UI reflects
    /// the request the user just made rather than the render still in flight.
    fn request_sample_from(
        &mut self,
        source: Arc<dyn ScopeProofSource>,
        document: Arc<Document>,
        session_id: u64,
        revision: u64,
        frame: TimeCode,
    ) {
        let key = ScopeRequestKey {
            session_id,
            revision,
            frame,
            roi: self.roi,
        };
        self.current_context = Some(key);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = Some(PendingScope { generation, key });
        self.current = None;
        self.error = None;
        let sample = QueuedSample {
            generation,
            key,
            source,
            document,
            frame,
        };
        if self.worker_is_running() {
            // The running worker's result belongs to an older generation and is
            // filtered out on arrival; retiring it also stops it delivering at
            // all. It still has to finish before the next render may start.
            self.retire_active_worker();
            self.queued = Some(sample);
            return;
        }
        self.reap_finished_worker();
        self.spawn_sample(sample);
    }

    /// Start one parked request on its own thread.
    fn spawn_sample(&mut self, sample: QueuedSample) {
        let QueuedSample {
            generation,
            key,
            source,
            document,
            frame,
        } = sample;
        let cancelled = Arc::new(AtomicBool::new(false));
        let response_tx = self.response_tx.clone();
        let roi = self.roi;
        let expected_resolution = document.resolution;
        let worker_cancelled = Arc::clone(&cancelled);
        let spawn_result = thread::Builder::new()
            .name("kinewright-scope-proof".to_owned())
            .spawn(move || {
                let mut completion = WorkerCompletion {
                    generation,
                    key,
                    response_tx,
                    cancelled: worker_cancelled,
                    delivered: false,
                };
                if completion.is_retired() {
                    // Superseded before the render began: skip the work.
                    completion.delivered = true;
                    return;
                }
                let result = source.monitor_proof(document, frame).and_then(|proof| {
                    scope_measurement_from_proof(proof, key, roi, expected_resolution)
                        .map_err(|error| error.to_string())
                });
                completion.deliver(result);
            });
        let Ok(handle) = spawn_result else {
            self.pending = None;
            self.active = None;
            self.error = Some("Could not start the full-resolution scope worker".to_owned());
            return;
        };
        #[cfg(test)]
        {
            self.spawned_workers += 1;
        }
        self.active = Some(ActiveWorker { cancelled, handle });
    }

    fn worker_is_running(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    /// Join a worker that has already finished so its thread resources are
    /// released. `is_finished` is checked first, so this never blocks the UI
    /// thread.
    fn reap_finished_worker(&mut self) {
        if self.active.as_ref().is_some_and(ActiveWorker::is_finished)
            && let Some(worker) = self.active.take()
        {
            let _ = worker.handle.join();
        }
    }

    /// Retire the running worker without waiting for it.
    ///
    /// The handle is kept: the thread is still alive and still holds its
    /// renderer, so the panel has to know when it is safe to start the next
    /// one.
    fn retire_active_worker(&mut self) {
        if let Some(worker) = self.active.as_ref() {
            worker.retire();
        }
    }

    /// Drain all worker responses and accept only the still-live generation
    /// and exact context key. Late results are deliberately silent.
    ///
    /// This is also where a parked request starts, once the thread it was
    /// waiting behind has finished.
    pub(crate) fn poll(&mut self) {
        while let Ok(response) = self.response_rx.try_recv() {
            if !self.response_matches_pending(&response) {
                continue;
            }
            self.pending = None;
            match response.result {
                Ok(measurement) => {
                    self.current = Some(measurement);
                    self.error = None;
                }
                Err(error) => {
                    self.current = None;
                    self.error = Some(error);
                }
            }
        }
        self.reap_finished_worker();
        if self.active.is_none()
            && let Some(sample) = self.queued.take()
        {
            self.spawn_sample(sample);
        }
    }

    fn response_matches_pending(&self, response: &ScopeResponse) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.generation == response.generation
                && pending.key == response.key
                && self.current_context == Some(response.key)
        })
    }

    pub(crate) fn capture_reference(&mut self) -> bool {
        let Some(measurement) = self.current.clone() else {
            return false;
        };
        self.reference = Some(ReferenceMeasurement { measurement });
        true
    }

    pub(crate) fn clear_reference(&mut self) {
        self.reference = None;
    }

    #[must_use]
    pub(crate) fn reference(&self) -> Option<&ScopeMeasurement> {
        self.reference
            .as_ref()
            .map(|reference| &reference.measurement)
    }

    #[must_use]
    pub(crate) fn delta(&self) -> Option<ScopeComparison> {
        let current = self.current.as_ref()?;
        let reference = self.reference()?;
        // CC2 allows the two source rasters to differ; `compare_scope_evidence`
        // still requires the same stage, normalized ROI, and output grids. The
        // dimensions are reported as metadata beside the delta.
        compare_scope_evidence(&reference.evidence, &current.evidence).ok()
    }

    /// Reference and current source rasters for the active comparison.
    #[must_use]
    pub(crate) fn comparison_dimensions(&self) -> Option<((u32, u32), (u32, u32))> {
        let current = self.current.as_ref()?;
        let reference = self.reference()?;
        Some((
            (reference.width, reference.height),
            (current.width, current.height),
        ))
    }

    /// Apply raw ROI controls, recording any clamp so the panel can report it.
    ///
    /// Returns the accepted ROI, or `None` when the rectangle has no positive
    /// area after clamping.
    pub(crate) fn apply_roi_controls(
        &mut self,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    ) -> Option<ScopeRoi> {
        let normalization = ScopeRoi::normalize(left, top, right, bottom);
        self.roi_clamped = normalization.clamped;
        let roi = normalization.roi?;
        self.set_roi(roi).then_some(roi)
    }

    fn invalidate(&mut self, retain_reference: bool) {
        self.generation = self.generation.wrapping_add(1);
        // The in-flight worker is retired here, not merely forgotten: an
        // abandoned worker must never resolve a later request's pending state.
        // Its handle is kept so `poll` can still reap it.
        self.retire_active_worker();
        // A parked request describes the context that just went stale.
        self.queued = None;
        self.pending = None;
        self.current = None;
        self.error = None;
        if !retain_reference {
            self.reference = None;
        }
    }
}

impl KinewrightApp {
    /// Compact monitor-adjacent Scopes surface. Measurements are evidence-only
    /// and never alter the active grade; application is explicitly deferred to
    /// the Primary correction inspector.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn color_scopes_panel(&mut self, ui: &mut egui::Ui) {
        let session_id = self.focused().id;
        let revision = self.focused().revision.0;
        let frame = self.focused().position;
        self.color_scopes
            .observe_context(session_id, revision, frame);
        self.color_scopes.poll();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("SCOPES").font(theme::semibold(type_size::CAPTION)));
            ui.colored_label(color::TEXT_MUTED, "Evidence only");
            ui.separator();
            for kind in ScopeKind::ALL {
                if ui
                    .selectable_label(self.color_scopes.kind == kind, kind.label())
                    .clicked()
                {
                    self.color_scopes.set_kind(kind);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color::TEXT_MUTED, "Stage:");
            ui.monospace("monitoring / post-composite");
            ui.colored_label(color::TEXT_MUTED, "·");
            ui.colored_label(color::TEXT_MUTED, "full-resolution proof required");
            ui.colored_label(color::TEXT_MUTED, format!("· frame {frame}"));
        });
        self.scope_roi_controls(ui);

        let can_sample = !self.color_scopes.is_pending();
        let mut sample = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_sample, egui::Button::new("Sample current frame"))
                .clicked()
            {
                sample = true;
            }
            if self.color_scopes.is_pending() {
                ui.colored_label(color::STATUS_WARNING, "Rendering full-resolution proof…");
            } else if let Some(error) = &self.color_scopes.error {
                ui.colored_label(color::STATUS_DANGER, format!("Scope unavailable: {error}"));
            } else if self.color_scopes.current.is_none() {
                ui.colored_label(color::TEXT_MUTED, "No frame sampled");
            }
        });
        if sample {
            let analysis = Arc::clone(&self.analysis);
            let document = Arc::clone(&self.focused().document);
            self.color_scopes
                .request_sample(analysis, document, session_id, revision, frame);
        }

        if let Some(measurement) = self.color_scopes.current.as_ref() {
            self.paint_scope_measurement(ui, measurement);
        } else {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 112.0),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(rect, radius::SM, color::LETTERBOX);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Sample an exact Program frame to inspect post-composite evidence",
                egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
                color::TEXT_MUTED,
            );
        }

        ui.horizontal(|ui| {
            let capture_enabled = self.color_scopes.current.is_some();
            if ui
                .add_enabled(capture_enabled, egui::Button::new("Capture reference shot"))
                .clicked()
            {
                self.color_scopes.capture_reference();
            }
            if ui
                .add_enabled(
                    self.color_scopes.has_reference(),
                    egui::Button::new("Clear reference"),
                )
                .clicked()
            {
                self.color_scopes.clear_reference();
            }
            if self.color_scopes.has_reference() {
                ui.colored_label(color::STATUS_SUCCESS, "Reference retained");
            } else {
                ui.colored_label(color::TEXT_MUTED, "No reference shot");
            }
        });
        let comparison_dimensions = self.color_scopes.comparison_dimensions();
        if let Some(delta) = self.color_scopes.delta() {
            ui.group(|ui| {
                ui.label(
                    egui::RichText::new("CURRENT − REFERENCE")
                        .font(theme::semibold(type_size::CAPTION)),
                );
                if let Some((reference, current)) = comparison_dimensions {
                    // CC2 compares stage/ROI/grid, not source raster size. The
                    // two rasters are provenance metadata, not a gate.
                    ui.colored_label(
                        color::TEXT_MUTED,
                        format!(
                            "reference {}×{} → current {}×{}",
                            reference.0, reference.1, current.0, current.1
                        ),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    for (label, value) in [
                        ("R", delta.statistics.red.mean.delta),
                        ("G", delta.statistics.green.mean.delta),
                        ("B", delta.statistics.blue.mean.delta),
                        ("Y", delta.statistics.luma.mean.delta),
                    ] {
                        ui.monospace(format!("{label} {value:+} µ‰"));
                    }
                    ui.monospace(format!("pixels {:+}", delta.visible_pixel_count.delta));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(format!("black {:+} bp", delta.clipping.luma.black.delta));
                    ui.monospace(format!("white {:+} bp", delta.clipping.luma.white.delta));
                });
            });
        } else if self.color_scopes.has_reference() && self.color_scopes.current.is_some() {
            // Defensive. `set_roi` drops the reference whenever the ROI moves
            // and both measurements come from the same monitoring stage and
            // grid defaults, so `compare_scope_evidence` is not expected to
            // refuse here. If a future stage or grid option makes two retained
            // measurements incomparable, this says so instead of silently
            // showing no delta.
            ui.colored_label(
                color::STATUS_WARNING,
                "Reference retained, but its stage, ROI, or scope grid contract differs; capture a compatible reference to compare.",
            );
        }
        ui.colored_label(
            color::STATUS_WARNING,
            "Measuring or capturing never changes the grade. Starting match application is deferred; adjust Primary correction explicitly.",
        );
    }

    fn scope_roi_controls(&mut self, ui: &mut egui::Ui) {
        let mut roi = self.color_scopes.roi;
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color::TEXT_MUTED, "ROI bp");
            for (label, value) in [
                ("L", &mut roi.left),
                ("T", &mut roi.top),
                ("R", &mut roi.right),
                ("B", &mut roi.bottom),
            ] {
                ui.label(label);
                ui.add(
                    egui::DragValue::new(value)
                        .range(0..=ROI_MAX)
                        // `range` alone also rewrites the value the panel
                        // supplied, before any interaction, which would make
                        // the widget rather than `apply_roi_controls` the
                        // authority on what region is measured. Only the
                        // panel's own normalization may change the ROI, so the
                        // note below describes the clamp that actually applied.
                        // (egui still clamps a drag or a typed edit to the
                        // range; the panel's clamp is what covers everything
                        // else.)
                        .clamp_existing_to_range(false),
                );
            }
            if ui.small_button("Full frame").clicked() {
                roi = ScopeRoi::full_frame();
            }
        });
        let accepted = self.color_scopes.apply_roi_controls(
            i32::from(roi.left),
            i32::from(roi.top),
            i32::from(roi.right),
            i32::from(roi.bottom),
        );
        if accepted.is_none() {
            ui.colored_label(
                color::STATUS_DANGER,
                "ROI must have positive width and height",
            );
        }
        if self.color_scopes.roi_clamped {
            ui.colored_label(
                color::STATUS_WARNING,
                "ROI clamped into 0..=10000 basis points",
            );
        }
    }

    fn paint_scope_measurement(&self, ui: &mut egui::Ui, measurement: &ScopeMeasurement) {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                if measurement.full_resolution {
                    color::STATUS_SUCCESS
                } else {
                    color::STATUS_DANGER
                },
                if measurement.full_resolution {
                    "FULL RESOLUTION"
                } else {
                    "NOT FULL RESOLUTION"
                },
            );
            ui.colored_label(
                color::TEXT_MUTED,
                format!("{}×{}", measurement.width, measurement.height),
            );
            ui.colored_label(color::TEXT_MUTED, measurement.stage.as_str());
            ui.colored_label(
                color::TEXT_MUTED,
                format!("{} · {}", measurement.backend, measurement.adapter),
            );
            ui.colored_label(
                color::TEXT_MUTED,
                if measurement.gpu_claim && !measurement.software_fallback {
                    "GPU compositor"
                } else {
                    "software fallback"
                },
            );
        });
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 130.0),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, radius::SM, color::LETTERBOX);
        theme::paint_inset_well(&painter, rect, radius::px(radius::SM));
        match self.color_scopes.kind {
            ScopeKind::Waveform => paint_waveform(&painter, rect, &measurement.evidence.waveform),
            ScopeKind::RgbParade => paint_parade(&painter, rect, &measurement.evidence.parade),
            ScopeKind::Vectorscope => {
                paint_vectorscope(&painter, rect, &measurement.evidence.vectorscope);
            }
        }
    }
}

/// Render one proof into immutable, typed core scope evidence. The painter and
/// state machine below intentionally do not inspect pixels or reimplement any
/// scope math.
fn scope_measurement_from_proof(
    proof: MonitorProof,
    key: ScopeRequestKey,
    roi: ScopeRoi,
    expected_resolution: (u32, u32),
) -> Result<ScopeMeasurement, kinewright_core::ScopeError> {
    let request = ScopeRequest {
        stage: ScopeStage::MonitoringPostComposite,
        roi: roi.to_core(),
        resolution: ScopeResolution::default(),
    };
    let evidence = measure_scope(&proof.image, key.frame.0, &request)?;
    Ok(ScopeMeasurement {
        key,
        width: evidence.metadata.source_resolution.width,
        height: evidence.metadata.source_resolution.height,
        full_resolution: proof.metadata.full_resolution
            && evidence.metadata.full_resolution
            && (
                evidence.metadata.source_resolution.width,
                evidence.metadata.source_resolution.height,
            ) == expected_resolution,
        stage: evidence.metadata.stage,
        backend: proof.metadata.backend,
        adapter: proof.metadata.adapter,
        software_fallback: proof.metadata.software_fallback,
        gpu_claim: proof.metadata.gpu_claim,
        evidence,
    })
}

fn paint_waveform(
    painter: &egui::Painter,
    rect: egui::Rect,
    waveform: &kinewright_core::LumaWaveform,
) {
    paint_grid(painter, rect);
    paint_density(
        painter,
        rect,
        waveform.columns,
        waveform.rows,
        &waveform.density,
        color::ACCENT,
    );
}

#[allow(clippy::cast_precision_loss)]
fn paint_parade(painter: &egui::Painter, rect: egui::Rect, parade: &kinewright_core::RgbParade) {
    paint_grid(painter, rect);
    let colors = [
        egui::Color32::from_rgb(230, 100, 105),
        egui::Color32::from_rgb(100, 215, 145),
        egui::Color32::from_rgb(100, 160, 235),
    ];
    let lane_width = rect.width() / 3.0;
    let channels = [
        &parade.red.density,
        &parade.green.density,
        &parade.blue.density,
    ];
    for (channel, density) in channels.into_iter().enumerate() {
        let lane = egui::Rect::from_min_max(
            egui::pos2(rect.left() + lane_width * channel as f32, rect.top()),
            egui::pos2(
                rect.left() + lane_width * (channel + 1) as f32,
                rect.bottom(),
            ),
        );
        paint_density_in_rect(
            painter,
            lane,
            parade.columns,
            parade.rows,
            density,
            colors[channel],
        );
    }
}

#[allow(clippy::cast_precision_loss)]
fn paint_vectorscope(
    painter: &egui::Painter,
    rect: egui::Rect,
    vectorscope: &kinewright_core::VectorscopeDensity,
) {
    paint_grid(painter, rect);
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.38;
    painter.circle_stroke(center, radius, egui::Stroke::new(1.0, color::BORDER_STRONG));
    let size = usize::from(vectorscope.size);
    let max_density = vectorscope
        .density
        .iter()
        .copied()
        .max()
        .unwrap_or(1)
        .max(1);
    for (index, density) in vectorscope.density.iter().copied().enumerate() {
        if density == 0 || size == 0 {
            continue;
        }
        let x = index % size;
        let y = index / size;
        let point = egui::pos2(
            rect.left() + (x as f32 + 0.5) * rect.width() / size as f32,
            rect.top() + (y as f32 + 0.5) * rect.height() / size as f32,
        );
        let alpha =
            u8::try_from((density.saturating_mul(220) / max_density).max(32)).unwrap_or(255);
        painter.circle_filled(
            point,
            1.8,
            egui::Color32::from_rgba_unmultiplied(66, 199, 201, alpha),
        );
    }
}

#[allow(clippy::cast_precision_loss)]
fn paint_density(
    painter: &egui::Painter,
    rect: egui::Rect,
    width: u16,
    height: u16,
    density: &[u64],
    tint: egui::Color32,
) {
    paint_density_in_rect(painter, rect, width, height, density, tint);
}

#[allow(clippy::cast_precision_loss)]
fn paint_density_in_rect(
    painter: &egui::Painter,
    rect: egui::Rect,
    width: u16,
    height: u16,
    density: &[u64],
    tint: egui::Color32,
) {
    let width = usize::from(width);
    let height = usize::from(height);
    if width == 0 || height == 0 || density.is_empty() {
        return;
    }
    let max_density = density.iter().copied().max().unwrap_or(1).max(1);
    let cell_width = rect.width() / width as f32;
    let cell_height = rect.height() / height as f32;
    for (index, value) in density.iter().copied().enumerate() {
        if value == 0 {
            continue;
        }
        let x = index % width;
        let y = index / width;
        if y >= height {
            break;
        }
        let alpha = u8::try_from((value.saturating_mul(230) / max_density).max(18)).unwrap_or(255);
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + x as f32 * cell_width,
                    rect.top() + y as f32 * cell_height,
                ),
                egui::vec2(cell_width.max(1.0), cell_height.max(1.0)),
            ),
            0.0,
            egui::Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), alpha),
        );
    }
}

fn paint_grid(painter: &egui::Painter, rect: egui::Rect) {
    for fraction in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let y = egui::lerp(rect.top()..=rect.bottom(), fraction);
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Condvar, Mutex},
        time::{Duration, Instant},
    };

    use super::*;

    /// An `ActiveWorker` whose thread has already returned, so the panel treats
    /// it as reapable. Tests that drive response handling directly do not need
    /// a real render behind it.
    fn finished_worker(cancelled: &Arc<AtomicBool>) -> ActiveWorker {
        let handle = thread::spawn(|| {});
        while !handle.is_finished() {
            thread::yield_now();
        }
        ActiveWorker {
            cancelled: Arc::clone(cancelled),
            handle,
        }
    }

    /// A proof source that blocks until the test releases it and records every
    /// frame a render actually started for.
    #[derive(Default)]
    struct GatedProofSource {
        started: Mutex<Vec<TimeCode>>,
        started_signal: Condvar,
        released: Mutex<bool>,
        release_signal: Condvar,
    }

    impl GatedProofSource {
        fn started_frames(&self) -> Vec<TimeCode> {
            self.started.lock().unwrap().clone()
        }

        fn wait_for_started(&self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut started = self.started.lock().unwrap();
            while started.len() < count {
                assert!(Instant::now() < deadline, "the worker never started");
                let (next, timeout) = self
                    .started_signal
                    .wait_timeout(started, Duration::from_millis(50))
                    .unwrap();
                started = next;
                let _ = timeout;
            }
        }

        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.release_signal.notify_all();
        }
    }

    impl ScopeProofSource for GatedProofSource {
        fn monitor_proof(
            &self,
            _document: Arc<Document>,
            frame: TimeCode,
        ) -> Result<MonitorProof, String> {
            self.started.lock().unwrap().push(frame);
            self.started_signal.notify_all();
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.release_signal.wait(released).unwrap();
            }
            Ok(test_proof(1, 1, true))
        }
    }

    /// Wait for the panel to reap its finished worker and start whatever it
    /// parked, exactly as the UI would by polling once per frame.
    fn poll_until(state: &mut ColorScopesState, done: impl Fn(&ColorScopesState) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            state.poll();
            if done(state) {
                return;
            }
            assert!(Instant::now() < deadline, "the panel never settled");
            thread::yield_now();
        }
    }

    fn single_frame_document() -> Arc<Document> {
        Arc::new(Document {
            resolution: (1, 1),
            ..Document::default()
        })
    }

    /// A request made while a worker is rendering must not start a second
    /// renderer: the cancel flag cannot interrupt a render in progress, so
    /// spawning per request would stack concurrent frame caches.
    #[test]
    fn a_request_during_a_render_waits_instead_of_spawning_a_second_worker() {
        let source = Arc::new(GatedProofSource::default());
        let document = single_frame_document();
        let mut state = ColorScopesState::default();

        state.request_sample_from(
            Arc::clone(&source) as Arc<_>,
            Arc::clone(&document),
            1,
            0,
            TimeCode(1),
        );
        source.wait_for_started(1);
        assert_eq!(state.spawned_workers, 1);

        // Two more requests while the first render is blocked.
        state.request_sample_from(
            Arc::clone(&source) as Arc<_>,
            Arc::clone(&document),
            1,
            0,
            TimeCode(2),
        );
        state.request_sample_from(
            Arc::clone(&source) as Arc<_>,
            Arc::clone(&document),
            1,
            0,
            TimeCode(3),
        );

        assert_eq!(
            state.spawned_workers, 1,
            "only one renderer may exist at a time"
        );
        assert_eq!(source.started_frames(), vec![TimeCode(1)]);
        assert!(state.queued.is_some(), "the newest request is parked");
        assert!(state.is_pending(), "the panel reflects the newest request");

        // Polling cannot start the parked request while the worker runs.
        state.poll();
        assert_eq!(state.spawned_workers, 1);

        source.release();
        poll_until(&mut state, |state| state.spawned_workers == 2);

        // The intermediate request was superseded and never rendered.
        source.wait_for_started(2);
        assert_eq!(
            source.started_frames(),
            vec![TimeCode(1), TimeCode(3)],
            "latest-request-wins: frame 2 was replaced before it could run"
        );

        poll_until(&mut state, |state| !state.is_pending());
        assert_eq!(
            state
                .current
                .as_ref()
                .map(|measurement| measurement.key.frame),
            Some(TimeCode(3))
        );
        assert!(state.queued.is_none());
    }

    /// The superseded worker's result is still filtered by generation, so it
    /// can never resolve the request that replaced it.
    #[test]
    fn a_superseded_worker_never_resolves_the_request_that_replaced_it() {
        let source = Arc::new(GatedProofSource::default());
        let document = single_frame_document();
        let mut state = ColorScopesState::default();

        state.request_sample_from(
            Arc::clone(&source) as Arc<_>,
            Arc::clone(&document),
            1,
            0,
            TimeCode(1),
        );
        source.wait_for_started(1);
        let first_generation = state.generation;
        state.request_sample_from(
            Arc::clone(&source) as Arc<_>,
            Arc::clone(&document),
            1,
            0,
            TimeCode(5),
        );
        assert_ne!(state.generation, first_generation);
        assert!(
            state.active.as_ref().is_some_and(ActiveWorker::is_retired),
            "the running worker is retired even though it cannot be interrupted"
        );

        source.release();
        poll_until(&mut state, |state| !state.is_pending());

        assert_eq!(
            state
                .current
                .as_ref()
                .map(|measurement| measurement.key.frame),
            Some(TimeCode(5)),
            "only the newest request may resolve the panel"
        );
        assert_eq!(state.spawned_workers, 2);
    }

    #[test]
    fn roi_normalization_clamps_per_field_and_rejects_zero_area() {
        let clamped = ScopeRoi::normalize(-10, 0, 10_500, 10_000);
        assert_eq!(clamped.roi, Some(ScopeRoi::full_frame()));
        assert!(clamped.clamped);

        // A field inside range is never widened to the full frame.
        let partial = ScopeRoi::normalize(-10, 2_000, 6_000, 7_000);
        assert_eq!(
            partial.roi,
            Some(ScopeRoi {
                left: 0,
                top: 2_000,
                right: 6_000,
                bottom: 7_000,
            })
        );
        assert!(partial.clamped);

        let in_range = ScopeRoi::normalize(100, 100, 9_000, 9_000);
        assert!(!in_range.clamped);
        assert!(in_range.roi.is_some());

        assert_eq!(ScopeRoi::normalize(20, 20, 20, 30).roi, None);
        assert_eq!(ScopeRoi::normalize(50, 60, 40, 70).roi, None);
        assert!(ScopeRoi::full_frame().to_core().validate().is_ok());
    }

    /// The UI clamps rather than rejecting, but the clamp is visible state so
    /// the panel can say the measured region is not the one that was typed.
    #[test]
    fn out_of_range_roi_controls_clamp_visibly_instead_of_silently_widening() {
        let mut state = ColorScopesState::default();
        let accepted = state.apply_roi_controls(-500, 1_000, 12_000, 8_000);
        assert_eq!(
            accepted,
            Some(ScopeRoi {
                left: 0,
                top: 1_000,
                right: ROI_MAX,
                bottom: 8_000,
            })
        );
        assert!(state.roi_clamped);
        assert_eq!(state.roi, accepted.expect("accepted ROI"));

        assert_eq!(
            state.apply_roi_controls(2_000, 2_000, 8_000, 8_000),
            Some(ScopeRoi {
                left: 2_000,
                top: 2_000,
                right: 8_000,
                bottom: 8_000,
            })
        );
        assert!(!state.roi_clamped, "an in-range ROI clears the clamp note");
        assert_eq!(state.apply_roi_controls(9_000, 0, 1_000, 10_000), None);
    }

    #[test]
    fn invalidation_clears_pending_and_current_evidence() {
        let mut state = ColorScopesState::default();
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        state.current_context = Some(key);
        state.generation = 1;
        state.pending = Some(PendingScope { generation: 1, key });
        state.invalidate(true);
        assert!(state.current.is_none());
        assert!(!state.is_pending());
        assert_eq!(state.generation, 2);
    }

    #[test]
    fn context_changes_invalidate_current_frame_but_retain_reference() {
        let mut state = ColorScopesState::default();
        state.observe_context(7, 3, TimeCode::ZERO);
        let key = ScopeRequestKey {
            session_id: 7,
            revision: 3,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        state.current = Some(test_measurement(key, [10, 20, 30]));
        assert!(state.capture_reference());
        state.observe_context(7, 3, TimeCode(1));
        assert!(state.current.is_none());
        assert!(state.has_reference());
        state.observe_context(8, 0, TimeCode::ZERO);
        assert!(!state.has_reference());
    }

    #[test]
    fn roi_change_invalidates_reference_and_current_evidence() {
        let mut state = ColorScopesState::default();
        let key = ScopeRequestKey {
            session_id: 7,
            revision: 3,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        state.current = Some(test_measurement(key, [10, 20, 30]));
        assert!(state.capture_reference());
        assert!(
            state.set_roi(
                ScopeRoi::normalize(100, 100, 9_000, 9_000)
                    .roi
                    .expect("non-empty ROI")
            )
        );
        assert!(state.current.is_none());
        assert!(!state.has_reference());
        assert!(!state.set_roi(ScopeRoi {
            left: 9_000,
            top: 0,
            right: 1_000,
            bottom: 10_000,
        }));
    }

    #[test]
    fn late_response_is_rejected_by_generation_and_key() {
        let mut state = ColorScopesState::default();
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        state.current_context = Some(key);
        state.generation = 2;
        state.pending = Some(PendingScope { generation: 2, key });
        let late_response = ScopeResponse {
            generation: 1,
            key,
            result: Ok(test_measurement(key, [10, 20, 30])),
        };
        assert!(!state.response_matches_pending(&late_response));
        state
            .response_tx
            .send(late_response)
            .expect("the panel still owns the receiver");
        state.poll();
        assert!(state.current.is_none());
        assert!(state.is_pending());
    }

    #[test]
    fn reference_is_retained_for_same_roi_and_comparison_is_signed() {
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        let first = test_measurement(key, [10, 20, 30]);
        let mut second = first.clone();
        second.key.frame = TimeCode(1);
        second.evidence = measure_scope(
            &kinewright_core::RgbaImage {
                width: 1,
                height: 1,
                pixels: vec![20, 10, 40, 255],
            },
            1,
            &ScopeRequest::default(),
        )
        .expect("candidate evidence");
        let mut state = ColorScopesState {
            current: Some(first),
            ..ColorScopesState::default()
        };
        assert!(state.capture_reference());
        state.current = Some(second);
        let delta = state.delta().expect("delta");
        assert!(delta.statistics.red.mean.delta > 0);
        assert!(delta.statistics.green.mean.delta < 0);
        assert!(delta.statistics.blue.mean.delta > 0);
        assert!(state.has_reference());
    }

    /// The reviewer's stall: a pending request that later generations of
    /// abandoned workers deliver into first. The live response must still land.
    #[test]
    fn superseded_workers_cannot_strand_the_panel_on_the_live_request() {
        let mut state = ColorScopesState::default();
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        state.current_context = Some(key);
        state.generation = 3;
        state.pending = Some(PendingScope { generation: 3, key });

        for generation in [1, 2] {
            state
                .response_tx
                .send(ScopeResponse {
                    generation,
                    key,
                    result: Ok(test_measurement(key, [1, 2, 3])),
                })
                .expect("an unbounded channel never refuses an abandoned worker");
        }
        state
            .response_tx
            .send(ScopeResponse {
                generation: 3,
                key,
                result: Ok(test_measurement(key, [10, 20, 30])),
            })
            .expect("the live response must always be deliverable");

        state.poll();

        assert!(!state.is_pending(), "the panel must not stay pending");
        assert_eq!(
            state.current.as_ref().map(|measurement| measurement.key),
            Some(key)
        );
        assert!(state.error.is_none());
    }

    /// A worker that unwinds or returns without delivering resolves the panel
    /// with an error instead of leaving it pending forever.
    #[test]
    fn a_worker_that_never_delivers_resolves_the_panel_with_an_error() {
        let mut state = ColorScopesState::default();
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        state.current_context = Some(key);
        state.generation = 1;
        state.pending = Some(PendingScope { generation: 1, key });
        let cancelled = Arc::new(AtomicBool::new(false));
        state.active = Some(finished_worker(&cancelled));

        drop(WorkerCompletion {
            generation: 1,
            key,
            response_tx: state.response_tx.clone(),
            cancelled,
            delivered: false,
        });
        state.poll();

        assert!(!state.is_pending());
        assert!(state.current.is_none());
        assert!(
            state
                .error
                .as_deref()
                .is_some_and(|error| error.contains("stopped before it delivered"))
        );
    }

    #[test]
    fn a_retired_worker_stays_silent_and_leaves_the_next_request_pending() {
        let mut state = ColorScopesState::default();
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        state.active = Some(finished_worker(&cancelled));
        state.current_context = Some(key);
        state.generation = 1;
        state.pending = Some(PendingScope { generation: 1, key });

        // Invalidation must retire the worker, not merely forget it. The
        // handle is retained on purpose: the thread is what the next request
        // has to wait behind, so it is reaped rather than dropped.
        state.invalidate(true);
        assert!(cancelled.load(Ordering::Acquire));
        assert!(
            state.active.as_ref().is_some_and(ActiveWorker::is_retired),
            "a retired worker stays observable until it is reaped"
        );

        drop(WorkerCompletion {
            generation: 1,
            key,
            response_tx: state.response_tx.clone(),
            cancelled,
            delivered: false,
        });
        state.generation = 2;
        state.pending = Some(PendingScope { generation: 2, key });
        state.poll();

        assert!(state.is_pending(), "a retired worker cannot resolve gen 2");
        assert!(state.error.is_none());
    }

    /// CC2 §"Reference comparison": source dimensions may differ. Only the
    /// stage, normalized ROI, and output grids have to match.
    #[test]
    fn comparison_survives_differing_source_dimensions() {
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };
        let reference = test_measurement(key, [10, 20, 30]);
        let mut current = test_measurement(key, [20, 10, 40]);
        current.width = 3_840;
        current.height = 2_160;
        current.evidence = measure_scope(
            &kinewright_core::RgbaImage {
                width: 2,
                height: 2,
                pixels: vec![
                    20, 10, 40, 255, 20, 10, 40, 255, 20, 10, 40, 255, 20, 10, 40, 255,
                ],
            },
            1,
            &ScopeRequest::default(),
        )
        .expect("candidate evidence");

        let mut state = ColorScopesState {
            current: Some(reference),
            ..ColorScopesState::default()
        };
        assert!(state.capture_reference());
        state.current = Some(current);

        let delta = state.delta().expect("differing rasters still compare");
        assert!(delta.statistics.red.mean.delta > 0);
        assert_eq!(
            state.comparison_dimensions(),
            Some(((1, 1), (3_840, 2_160))),
            "the two rasters are reported as metadata"
        );
    }

    #[test]
    fn full_resolution_requires_a_full_raster_proof_of_the_document_resolution() {
        let key = ScopeRequestKey {
            session_id: 1,
            revision: 0,
            frame: TimeCode::ZERO,
            roi: ScopeRoi::full_frame(),
        };

        let full = scope_measurement_from_proof(
            test_proof(4, 2, true),
            key,
            ScopeRoi::full_frame(),
            (4, 2),
        )
        .expect("full raster proof");
        assert!(full.full_resolution);
        assert_eq!((full.width, full.height), (4, 2));

        // A proxy raster is smaller than the document it claims to prove.
        let proxy = scope_measurement_from_proof(
            test_proof(2, 1, true),
            key,
            ScopeRoi::full_frame(),
            (4, 2),
        )
        .expect("proxy raster proof");
        assert!(!proxy.full_resolution);

        // A renderer that does not claim a full raster is never promoted.
        let unclaimed = scope_measurement_from_proof(
            test_proof(4, 2, false),
            key,
            ScopeRoi::full_frame(),
            (4, 2),
        )
        .expect("unclaimed proof");
        assert!(!unclaimed.full_resolution);
    }

    /// Measuring and capturing are evidence-only.
    ///
    /// `ColorScopesState` holds no `Core` handle, no `Command`/`Operation`
    /// sender, and no document: its only channel carries `ScopeResponse`,
    /// whose payload is `Result<ScopeMeasurement, String>`. There is therefore
    /// no operation channel to intercept, and the assertion is by
    /// construction. This test drives the real accept/capture path to prove
    /// those entry points only ever move evidence between panel fields.
    #[test]
    fn measurement_capture_has_no_operation_side_effect() {
        let mut state = ColorScopesState::default();
        // A fresh panel holds no evidence, so there is nothing to capture and
        // no reference to compare against.
        assert!(
            !state.capture_reference(),
            "capturing without a measurement must refuse rather than invent one"
        );
        assert!(state.reference().is_none());
        assert!(!state.has_reference());

        let key = ScopeRequestKey {
            session_id: 4,
            revision: 9,
            frame: TimeCode(12),
            roi: ScopeRoi::full_frame(),
        };
        state.observe_context(key.session_id, key.revision, key.frame);
        state.generation = 1;
        state.pending = Some(PendingScope { generation: 1, key });
        state
            .response_tx
            .send(ScopeResponse {
                generation: 1,
                key,
                result: Ok(test_measurement(key, [10, 20, 30])),
            })
            .expect("synthetic worker response");

        state.poll();
        assert_eq!(
            state.current.as_ref().map(|measurement| measurement.key),
            Some(key)
        );
        assert!(state.capture_reference());
        assert!(state.has_reference());

        // Capturing changes nothing outside the panel's evidence fields: the
        // grade-facing context, ROI, and request generation are untouched.
        assert_eq!(state.generation, 1);
        assert_eq!(state.roi, ScopeRoi::full_frame());
        assert_eq!(state.current_context, Some(key));
        assert!(state.error.is_none());
        assert_eq!(state.reference().map(|reference| reference.key), Some(key));
    }

    fn test_proof(width: u32, height: u32, full_resolution: bool) -> MonitorProof {
        let pixels = (0..width * height)
            .flat_map(|index| {
                let value = u8::try_from(index % 256).unwrap_or(0);
                [value, value, value, 255]
            })
            .collect();
        MonitorProof {
            image: kinewright_core::RgbaImage {
                width,
                height,
                pixels,
            },
            metadata: kinewright_core::MonitorProofMetadata {
                full_resolution,
                ..kinewright_core::MonitorProofMetadata::test_double()
            },
        }
    }

    fn test_measurement(key: ScopeRequestKey, rgb: [u8; 3]) -> ScopeMeasurement {
        let evidence = measure_scope(
            &kinewright_core::RgbaImage {
                width: 1,
                height: 1,
                pixels: vec![rgb[0], rgb[1], rgb[2], 255],
            },
            key.frame.0,
            &ScopeRequest::default(),
        )
        .expect("reference evidence");
        ScopeMeasurement {
            key,
            width: 1,
            height: 1,
            full_resolution: true,
            stage: evidence.metadata.stage,
            backend: "test".to_owned(),
            adapter: "test".to_owned(),
            software_fallback: true,
            gpu_claim: false,
            evidence,
        }
    }
}
