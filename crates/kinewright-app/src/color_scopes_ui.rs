//! Human-facing colour scopes and reference-shot inspection.
//!
//! The panel deliberately has no edit path.  A scope request renders an
//! immutable, full-resolution monitor proof on a worker thread and tags the
//! response with the project, revision, playhead, ROI, and a monotonic
//! generation.  A response that no longer describes the live editor context
//! is discarded before it can reach the paint path.

use std::{
    sync::{Arc, mpsc},
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
    /// A zero-area rectangle is rejected rather than silently becoming a
    /// full-frame measurement.
    #[must_use]
    pub(crate) fn normalize(left: i32, top: i32, right: i32, bottom: i32) -> Option<Self> {
        let left = left.clamp(0, i32::from(ROI_MAX));
        let top = top.clamp(0, i32::from(ROI_MAX));
        let right = right.clamp(0, i32::from(ROI_MAX));
        let bottom = bottom.clamp(0, i32::from(ROI_MAX));
        (right > left && bottom > top).then_some(Self {
            left: u16::try_from(left).ok()?,
            top: u16::try_from(top).ok()?,
            right: u16::try_from(right).ok()?,
            bottom: u16::try_from(bottom).ok()?,
        })
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
    generation: u64,
    response_tx: mpsc::SyncSender<ScopeResponse>,
    response_rx: mpsc::Receiver<ScopeResponse>,
    pub(crate) error: Option<String>,
}

impl Default for ColorScopesState {
    fn default() -> Self {
        let (response_tx, response_rx) = mpsc::sync_channel(2);
        Self {
            kind: ScopeKind::default(),
            roi: ScopeRoi::default(),
            current_context: None,
            current: None,
            reference: None,
            pending: None,
            generation: 0,
            response_tx,
            response_rx,
            error: None,
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

    /// Start one bounded full-resolution proof request. The worker owns the
    /// immutable document snapshot and never touches playback/UI state.
    pub(crate) fn request_sample(
        &mut self,
        analysis: Arc<dyn Analysis>,
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
        let response_tx = self.response_tx.clone();
        let roi = self.roi;
        let expected_resolution = document.resolution;
        let spawn_result = thread::Builder::new()
            .name("kinewright-scope-proof".to_owned())
            .spawn(move || {
                let result = analysis
                    .monitor_proof_for_document(document, frame)
                    .map_err(|error| error.to_string())
                    .and_then(|proof| {
                        scope_measurement_from_proof(proof, key, roi, expected_resolution)
                            .map_err(|error| error.to_string())
                    });
                // A full channel means a newer request has already superseded
                // this one. Dropping the stale response is the bounded policy.
                let _ = response_tx.try_send(ScopeResponse {
                    generation,
                    key,
                    result,
                });
            });
        if spawn_result.is_err() {
            self.pending = None;
            self.error = Some("Could not start the full-resolution scope worker".to_owned());
        }
    }

    /// Drain all worker responses and accept only the still-live generation
    /// and exact context key. Late results are deliberately silent.
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
        if current.width != reference.width || current.height != reference.height {
            return None;
        }
        compare_scope_evidence(&reference.evidence, &current.evidence).ok()
    }

    fn invalidate(&mut self, retain_reference: bool) {
        self.generation = self.generation.wrapping_add(1);
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
        if let Some(delta) = self.color_scopes.delta() {
            ui.group(|ui| {
                ui.label(
                    egui::RichText::new("CURRENT − REFERENCE")
                        .font(theme::semibold(type_size::CAPTION)),
                );
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
            ui.colored_label(
                color::STATUS_WARNING,
                "Reference retained, but its raster/ROI contract differs; capture a compatible reference to compare.",
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
                ui.add(egui::DragValue::new(value).range(0..=ROI_MAX));
            }
            if ui.small_button("Full frame").clicked() {
                roi = ScopeRoi::full_frame();
            }
        });
        let normalized = ScopeRoi::normalize(
            i32::from(roi.left),
            i32::from(roi.top),
            i32::from(roi.right),
            i32::from(roi.bottom),
        );
        if normalized.is_none() {
            ui.colored_label(
                color::STATUS_DANGER,
                "ROI must have positive width and height",
            );
        } else if let Some(roi) = normalized
            && roi != self.color_scopes.roi
        {
            let _ = self.color_scopes.set_roi(roi);
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
    use super::*;

    #[test]
    fn roi_normalization_clamps_and_rejects_zero_area() {
        assert_eq!(
            ScopeRoi::normalize(-10, 0, 10_500, 10_000),
            Some(ScopeRoi::full_frame())
        );
        assert_eq!(ScopeRoi::normalize(20, 20, 20, 30), None);
        assert_eq!(ScopeRoi::normalize(50, 60, 40, 70), None);
        assert!(ScopeRoi::full_frame().to_core().validate().is_ok());
    }

    #[test]
    fn stale_response_is_rejected_after_context_invalidation() {
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
        assert!(state.set_roi(ScopeRoi::normalize(100, 100, 9_000, 9_000).expect("non-empty ROI")));
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
            .try_send(late_response)
            .expect("bounded response");
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

    #[test]
    fn measurement_capture_has_no_operation_side_effect() {
        let before = ColorScopesState::default().generation;
        let mut state = ColorScopesState::default();
        assert!(!state.capture_reference());
        assert_eq!(state.generation, before);
        assert!(state.reference().is_none());
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
