use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use kinewright_core::{
    CaptionCue, DeliveryAspect, DeliveryConformanceReport, DeliveryProfile, DeliveryVariant,
    DeliveryVariantError, Document, ExportCancellation, ExportMediaPreflightReport, ExportProgress,
    ExportSettings, MediaError, Operation, QaIssue, QaSeverity, Rational, TimeCode,
    delivery_conformance, document_for_delivery_variant, export_media_preflight, srt, vtt,
};

use crate::{
    app::KinewrightApp,
    color_ui::{color_pipeline_summary, managed_sdr_reset_needed},
    icons::Icon,
    theme::{self, color, size, space},
};

/// Advisory findings shown before the list is summarized.
///
/// The window is fixed-size; beyond roughly this many lines the export controls
/// stop being reachable without scrolling past the reason to read them.
const MAX_ADVISORY_LINES: usize = 6;

/// Height budget for the scrollable dialog body.
const EXPORT_DIALOG_MAX_BODY_HEIGHT: f32 = 420.0;

pub(crate) struct ExportDialog {
    pub(crate) open: bool,
    pub(crate) output: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps_numerator: u32,
    pub(crate) fps_denominator: u32,
    pub(crate) delivery_aspect: Option<DeliveryAspect>,
    pub(crate) focus_x_percent: u8,
    pub(crate) focus_y_percent: u8,
    /// Last conformance result and the exact inputs that produced it.
    ///
    /// The gate clones the document, materializes the delivery reframe, and
    /// stats every source file. That is far too much to redo on every
    /// immediate-mode repaint of an open dialog, and none of it can change
    /// unless one of the keyed inputs does.
    pub(crate) conformance_cache: Option<(ConformanceKey, Result<ExportConformance, String>)>,
}

/// Everything `export_conformance` reads, plus the raster the gate is claiming
/// to have validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConformanceKey {
    revision: u64,
    aspect: Option<DeliveryAspect>,
    focus_x_percent: u8,
    focus_y_percent: u8,
    width: u32,
    height: u32,
}

pub(crate) struct ExportJob {
    pub(crate) cancellation: ExportCancellation,
    pub(crate) progress_rx: crossbeam_channel::Receiver<ExportProgress>,
    pub(crate) result_rx: mpsc::Receiver<(PathBuf, Result<(), MediaError>)>,
    pub(crate) progress: ExportProgress,
}

#[derive(Clone, Copy)]
enum CaptionFormat {
    Srt,
    Vtt,
}

impl CaptionFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    const fn filter_name(self) -> &'static str {
        match self {
            Self::Srt => "SubRip captions",
            Self::Vtt => "WebVTT captions",
        }
    }
}

/// The stable delivery profile the human export dialog's aspect choice maps to.
///
/// The dialog offers a master export plus the three CC/delivery aspects; the
/// agent export queue already gates on the matching profile, so the human path
/// runs the same conformance contract.
const fn export_delivery_profile(aspect: Option<DeliveryAspect>) -> DeliveryProfile {
    match aspect {
        None => DeliveryProfile::SourceMaster,
        Some(DeliveryAspect::Widescreen) => DeliveryProfile::Youtube1080p,
        Some(DeliveryAspect::Vertical) => DeliveryProfile::VerticalShort,
        Some(DeliveryAspect::Square) => DeliveryProfile::SquareSocial,
    }
}

/// The raster the encoder must render for the dialog's current settings.
///
/// A delivery aspect owns its raster: `delivery_conformance` validates
/// `DeliveryProfile::resolution`, which is exactly `aspect.resolution()`. If
/// the dialog's editable frame size were also allowed to apply, the gate would
/// validate one raster while the encoder rendered another. Master export has no
/// profile raster to conflict with, so it stays editable.
const fn export_frame_size(aspect: Option<DeliveryAspect>, width: u32, height: u32) -> (u32, u32) {
    match aspect {
        Some(aspect) => aspect.resolution(),
        None => (width, height),
    }
}

/// Delivery-conformance findings split into what refuses an export and what is
/// only advisory.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ExportConformance {
    pub(crate) blocking: Vec<QaIssue>,
    pub(crate) advisory: Vec<QaIssue>,
}

impl ExportConformance {
    /// Split a conformance report into what the export dialog must show.
    ///
    /// `QaSeverity::Info` findings are deliberately dropped. They are
    /// descriptive rather than actionable — `abrupt_cut` alone emits one line
    /// per hard cut in the timeline — so including them buries the warnings a
    /// person actually has to read before delivering. The full report is still
    /// available from `delivery_conformance` and from the agent export queue.
    fn from_report(report: &DeliveryConformanceReport) -> Self {
        let (blocking, advisory) = report
            .issues
            .iter()
            .filter(|issue| matches!(issue.severity, QaSeverity::Error | QaSeverity::Warning))
            .cloned()
            .partition(|issue| issue.severity == QaSeverity::Error);
        Self { blocking, advisory }
    }

    #[must_use]
    pub(crate) fn export_ready(&self) -> bool {
        self.blocking.is_empty()
    }

    /// One line per blocking issue, keeping the machine-readable code with the
    /// human sentence so a recorded error stays diagnosable.
    #[must_use]
    pub(crate) fn summary(&self) -> String {
        if self.export_ready() {
            return "Delivery conformance passed".to_owned();
        }
        let detail = self
            .blocking
            .iter()
            .map(|issue| format!("[{}] {}", issue.code, issue.message))
            .collect::<Vec<_>>()
            .join(" · ");
        format!("Delivery conformance refused this export: {detail}")
    }
}

/// Run the exact structural, pipeline, and source-colour contract the export
/// will render against.
#[allow(clippy::similar_names)]
fn export_conformance(
    document: &Document,
    aspect: Option<DeliveryAspect>,
    focus_x_percent: u8,
    focus_y_percent: u8,
) -> Result<ExportConformance, DeliveryVariantError> {
    delivery_conformance(
        document,
        export_delivery_profile(aspect),
        focus_x_percent,
        focus_y_percent,
    )
    .map(|report| ExportConformance::from_report(&report))
}

/// Re-check both fail-closed gates on the worker, against the exact documents
/// and sources the encoder is about to read.
fn run_export_after_preflight(
    conformance: &ExportConformance,
    media: &ExportMediaPreflightReport,
    export: impl FnOnce() -> Result<(), MediaError>,
) -> Result<(), MediaError> {
    if !conformance.export_ready() {
        return Err(MediaError::Backend(conformance.summary()));
    }
    if !media.export_ready() {
        return Err(MediaError::Backend(media.summary()));
    }
    export()
}

impl KinewrightApp {
    pub(crate) fn open_export_dialog(&mut self) {
        let resolution = self.export_dialog.delivery_aspect.map_or(
            self.focused().document.resolution,
            DeliveryAspect::resolution,
        );
        self.export_dialog.width = resolution.0;
        self.export_dialog.height = resolution.1;
        self.export_dialog.fps_numerator = self.focused().document.fps.numerator();
        self.export_dialog.fps_denominator = self.focused().document.fps.denominator();
        if let Some(project_path) = &self.focused().project_path {
            self.export_dialog.output = project_path.with_extension("mp4").display().to_string();
        }
        self.export_dialog.open = true;
    }

    pub(crate) fn choose_export_output(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("MPEG-4 video", &["mp4"])
            .set_file_name("export.mp4")
            .save_file()
        else {
            return;
        };
        self.export_dialog.output = path.display().to_string();
    }

    /// Materialize the exact document the export will render and run both
    /// fail-closed gates before any encoder work starts: the delivery
    /// colour/structural conformance contract, then source media availability.
    ///
    /// Returns `None` after recording the human-readable refusal.
    fn export_delivery_document(&mut self) -> Option<Arc<Document>> {
        let document = if let Some(aspect) = self.export_dialog.delivery_aspect {
            let variant = match DeliveryVariant::new(
                aspect,
                self.export_dialog.focus_x_percent,
                self.export_dialog.focus_y_percent,
            ) {
                Ok(variant) => variant,
                Err(error) => {
                    self.record_error("Export", error.to_string());
                    return None;
                }
            };
            match document_for_delivery_variant(&self.focused().document, variant) {
                Ok(document) => Arc::new(document),
                Err(error) => {
                    self.record_error("Export", error.to_string());
                    return None;
                }
            }
        } else {
            Arc::clone(&self.focused().document)
        };
        let conformance = match export_conformance(
            &self.focused().document,
            self.export_dialog.delivery_aspect,
            self.export_dialog.focus_x_percent,
            self.export_dialog.focus_y_percent,
        ) {
            Ok(conformance) => conformance,
            Err(error) => {
                self.record_error("Export", error.to_string());
                return None;
            }
        };
        if !conformance.export_ready() {
            self.record_error("Export", conformance.summary());
            return None;
        }
        let media_preflight = export_media_preflight(&document, self.analysis.as_ref());
        if !media_preflight.export_ready() {
            self.record_error("Export", media_preflight.summary());
            return None;
        }
        Some(document)
    }

    /// Keep the dialog's frame size equal to the raster the delivery gate
    /// validates. A no-op for a Master export, whose frame size is the
    /// operator's to choose.
    fn lock_frame_size_to_delivery_aspect(&mut self) {
        (self.export_dialog.width, self.export_dialog.height) = export_frame_size(
            self.export_dialog.delivery_aspect,
            self.export_dialog.width,
            self.export_dialog.height,
        );
    }

    /// The conformance report for the dialog's current settings, recomputed
    /// only when one of its inputs actually changes.
    ///
    /// `delivery_conformance` clones the document, materializes the delivery
    /// reframe, and touches the filesystem once per source asset. An open
    /// dialog repaints continuously, so running it unconditionally put all of
    /// that on the UI thread every frame. `start_export` re-validates
    /// independently, so a stale cache can never admit an export.
    ///
    /// The error is kept as a `String`: `DeliveryVariantError` is not `Clone`,
    /// and the dialog only ever renders it.
    fn cached_export_conformance(&mut self) -> Result<ExportConformance, String> {
        let key = ConformanceKey {
            revision: self.focused().revision.0,
            aspect: self.export_dialog.delivery_aspect,
            focus_x_percent: self.export_dialog.focus_x_percent,
            focus_y_percent: self.export_dialog.focus_y_percent,
            width: self.export_dialog.width,
            height: self.export_dialog.height,
        };
        if let Some((cached_key, cached)) = &self.export_dialog.conformance_cache
            && *cached_key == key
        {
            return cached.clone();
        }
        let conformance = export_conformance(
            &self.focused().document,
            key.aspect,
            key.focus_x_percent,
            key.focus_y_percent,
        )
        .map_err(|error| error.to_string());
        self.export_dialog.conformance_cache = Some((key, conformance.clone()));
        conformance
    }

    pub(crate) fn start_export(&mut self) {
        if self.export_job.is_some() {
            return;
        }
        // The encoder renders this raster and the gate below validates the
        // delivery profile's. Re-apply the lock so they cannot disagree even if
        // the export was started without the dialog having drawn a frame.
        self.lock_frame_size_to_delivery_aspect();
        if self.focused().document.duration <= TimeCode::ZERO {
            self.record_error("Export", "Add a clip to the timeline before exporting");
            return;
        }
        if !self.export_dialog.width.is_multiple_of(2)
            || !self.export_dialog.height.is_multiple_of(2)
        {
            self.record_error("Export", "H.264 export width and height must be even");
            return;
        }
        let fps = match Rational::new(
            self.export_dialog.fps_numerator,
            self.export_dialog.fps_denominator,
        ) {
            Ok(fps) => fps,
            Err(error) => {
                self.record_error("Export", format!("Invalid export frame rate: {error}"));
                return;
            }
        };
        let mut output = PathBuf::from(self.export_dialog.output.trim());
        if output.as_os_str().is_empty() {
            self.record_error("Export", "Choose an export output path");
            return;
        }
        if output.extension().is_none() {
            output.set_extension("mp4");
            self.export_dialog.output = output.display().to_string();
        }
        let cancellation = ExportCancellation::default();
        let Some(document) = self.export_delivery_document() else {
            return;
        };
        let settings = ExportSettings {
            fps,
            resolution: (self.export_dialog.width, self.export_dialog.height),
            delivery_color: document.color_context.delivery.clone(),
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 8_000_000,
            audio_bitrate: 192_000,
            cancellation: cancellation.clone(),
        };
        let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = mpsc::channel();
        let media = Arc::clone(&self.exporter);
        let worker_analysis = Arc::clone(&self.analysis);
        let worker_document = document;
        let worker_output = output.clone();
        let spawn = thread::Builder::new()
            .name("kinewright-export".to_owned())
            .spawn(move || {
                let media_preflight =
                    export_media_preflight(&worker_document, worker_analysis.as_ref());
                // The delivery document is already materialized, so the worker
                // re-checks it as a master: no reframe is applied twice and the
                // colour contract is measured on the exact rendered document.
                let result =
                    delivery_conformance(&worker_document, DeliveryProfile::SourceMaster, 50, 50)
                        .map_err(|error| MediaError::Backend(error.to_string()))
                        .and_then(|report| {
                            run_export_after_preflight(
                                &ExportConformance::from_report(&report),
                                &media_preflight,
                                || {
                                    media.export_document(
                                        worker_document,
                                        &worker_output,
                                        settings,
                                        progress_tx,
                                    )
                                },
                            )
                        });
                let _ = result_tx.send((worker_output, result));
            });
        if let Err(error) = spawn {
            self.record_error("Export", format!("Could not start export: {error}"));
            return;
        }
        self.status = format!("Exporting {}…", output.display());
        self.export_job = Some(ExportJob {
            cancellation,
            progress_rx,
            result_rx,
            progress: ExportProgress {
                completed_frames: 0,
                total_frames: 0,
            },
        });
    }

    fn save_caption_sidecar(&mut self, format: CaptionFormat, cues: &[CaptionCue]) {
        let extension = format.extension();
        let default_name = self.caption_default_file_name(extension);
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter(format.filter_name(), &[extension])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension(extension);
        }
        let contents = match format {
            CaptionFormat::Srt => srt(cues, self.focused().document.fps),
            CaptionFormat::Vtt => vtt(cues, self.focused().document.fps),
        };
        match std::fs::write(&path, contents) {
            Ok(()) => self.status = format!("Saved captions to {}", path.display()),
            Err(error) => self.record_error(
                "Captions",
                format!("Could not save {}: {error}", path.display()),
            ),
        }
    }

    fn caption_default_file_name(&self, extension: &str) -> String {
        let output = PathBuf::from(self.export_dialog.output.trim());
        let stem = output
            .file_stem()
            .filter(|stem| !stem.is_empty())
            .map(|stem| stem.to_string_lossy().into_owned())
            .or_else(|| {
                self.focused().document.media_pool.first().map(|asset| {
                    std::path::Path::new(&asset.name)
                        .file_stem()
                        .unwrap_or(asset.name.as_ref())
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .unwrap_or_else(|| "captions".to_owned());
        format!("{stem}.{extension}")
    }

    pub(crate) fn poll_export(&mut self, ctx: &egui::Context) {
        let mut completed = None;
        if let Some(job) = &mut self.export_job {
            while let Ok(progress) = job.progress_rx.try_recv() {
                job.progress = progress;
            }
            match job.result_rx.try_recv() {
                Ok(result) => completed = Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    completed = Some((
                        PathBuf::from(&self.export_dialog.output),
                        Err(MediaError::Backend("export worker stopped".to_owned())),
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }
        if let Some((path, result)) = completed {
            self.export_job = None;
            match result {
                Ok(()) => self.status = format!("Exported {}", path.display()),
                Err(MediaError::Cancelled) => {
                    "Export cancelled".clone_into(&mut self.status);
                }
                Err(error) => self.record_error("Export", format!("Export failed: {error}")),
            }
        }
    }

    // Export settings, validation, progress, and cancellation share one immediate-mode dialog.
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    pub(crate) fn show_export_dialog(&mut self, ctx: &egui::Context) {
        if !self.export_dialog.open {
            return;
        }
        let mut open = self.export_dialog.open;
        let mut browse = false;
        let mut start = false;
        let mut cancel = false;
        let mut reset_color_pipeline = false;
        let caption_cues = self.timeline_caption_cues();
        let mut caption_format = None;
        let project_color_pipeline = color_pipeline_summary(&self.focused().document.color_context);
        let color_pipeline_reset_needed =
            managed_sdr_reset_needed(&self.focused().document.color_context);
        // Applied before the gate runs so the cache key, the displayed frame
        // size, and the raster the encoder will render are the same value.
        self.lock_frame_size_to_delivery_aspect();
        // Immediate mode: this reflects the aspect chosen on the previous
        // frame. `start_export` re-runs the same gate before it spawns.
        let conformance = self.cached_export_conformance();
        let conformance_ready = conformance
            .as_ref()
            .is_ok_and(ExportConformance::export_ready);
        let export_blocked = color_pipeline_reset_needed || !conformance_ready;
        egui::Window::new("Export")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
              // The window is not resizable and the findings list is
              // data-dependent, so the body scrolls rather than growing past
              // the screen and hiding the Export button.
              egui::ScrollArea::vertical()
                .max_height(EXPORT_DIALOG_MAX_BODY_HEIGHT)
                .show(ui, |ui| {
                ui.label(theme::caps_label("DELIVERABLE", color::TEXT_MUTED));
                ui.label(
                    egui::RichText::new("H.264 video · AAC audio · MP4 container")
                        .color(color::TEXT_SECONDARY),
                );
                ui.add_space(space::TWO);
                ui.label(theme::caps_label("COLOR PIPELINE", color::TEXT_MUTED));
                for stage in &project_color_pipeline {
                    ui.add(
                        egui::Label::new(egui::RichText::new(stage).color(color::TEXT_SECONDARY))
                            .wrap(),
                    );
                }
                if color_pipeline_reset_needed {
                    ui.colored_label(
                        color::STATUS_DANGER,
                        "BLOCKED · Managed SDR export requires a compatible project colour pipeline.",
                    );
                    if ui
                        .add(
                            egui::Button::new("Reset to Managed SDR")
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::STATUS_DANGER)),
                        )
                        .clicked()
                    {
                        reset_color_pipeline = true;
                    }
                }
                match &conformance {
                    Ok(conformance) => {
                        for issue in &conformance.blocking {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "BLOCKED · {} ({})",
                                        issue.message, issue.code
                                    ))
                                    .color(color::STATUS_DANGER),
                                )
                                .wrap(),
                            );
                        }
                        // The window is fixed-size, so an unbounded advisory
                        // list would push the Export button out of reach. The
                        // remainder is counted rather than silently dropped.
                        for issue in conformance.advisory.iter().take(MAX_ADVISORY_LINES) {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "REVIEW · {} ({})",
                                        issue.message, issue.code
                                    ))
                                    .color(color::STATUS_WARNING),
                                )
                                .wrap(),
                            );
                        }
                        if let Some(hidden) = conformance
                            .advisory
                            .len()
                            .checked_sub(MAX_ADVISORY_LINES)
                            .filter(|hidden| *hidden > 0)
                        {
                            ui.colored_label(
                                color::TEXT_MUTED,
                                format!("… and {hidden} more advisory finding(s)"),
                            );
                        }
                    }
                    Err(error) => {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "BLOCKED · delivery conformance could not run: {error}"
                                ))
                                .color(color::STATUS_DANGER),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.add_space(space::TWO);
                let before_aspect = self.export_dialog.delivery_aspect;
                ui.horizontal(|ui| {
                    ui.label("Delivery");
                    egui::ComboBox::from_id_salt("export-delivery-aspect")
                        .selected_text(
                            self.export_dialog
                                .delivery_aspect
                                .map_or("Master", DeliveryAspect::as_str),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.export_dialog.delivery_aspect,
                                None,
                                "Master",
                            );
                            for aspect in DeliveryAspect::ALL {
                                ui.selectable_value(
                                    &mut self.export_dialog.delivery_aspect,
                                    Some(aspect),
                                    aspect.as_str(),
                                );
                            }
                        });
                    if self.export_dialog.delivery_aspect.is_some() {
                        ui.label("Focal point");
                        ui.add(
                            egui::DragValue::new(&mut self.export_dialog.focus_x_percent)
                                .range(0..=100)
                                .suffix("% x"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.export_dialog.focus_y_percent)
                                .range(0..=100)
                                .suffix("% y"),
                        );
                    }
                });
                if self.export_dialog.delivery_aspect != before_aspect
                    && let Some(aspect) = self.export_dialog.delivery_aspect
                {
                    (self.export_dialog.width, self.export_dialog.height) = aspect.resolution();
                }
                ui.add_space(space::TWO);
                egui::Grid::new("export-settings")
                    .num_columns(2)
                    .spacing(egui::vec2(space::THREE, space::TWO))
                    .show(ui, |ui| {
                        ui.label("Output");
                        ui.horizontal(|ui| {
                            ui.scope(|ui| {
                                theme::apply_input_visuals(ui);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.export_dialog.output)
                                        .desired_width(320.0),
                                );
                            });
                            if ui
                                .add(
                                    egui::Button::image_and_text(
                                        Icon::Folder.image(size::ICON_MD),
                                        "Browse…",
                                    )
                                    .fill(color::SURFACE_RAISED),
                                )
                                .clicked()
                            {
                                browse = true;
                            }
                        });
                        ui.end_row();
                        ui.label("Frame size");
                        ui.horizontal(|ui| {
                            // The conformance gate validates the delivery
                            // profile's raster, but the encoder renders this
                            // value. An editable frame size under a delivery
                            // aspect lets those disagree, so the profile's
                            // raster is shown read-only instead.
                            if let Some(aspect) = self.export_dialog.delivery_aspect {
                                let (width, height) = aspect.resolution();
                                ui.colored_label(
                                    color::TEXT_SECONDARY,
                                    format!("{width} × {height}"),
                                );
                                ui.colored_label(
                                    color::TEXT_MUTED,
                                    format!(
                                        "locked by the {} delivery profile",
                                        aspect.as_str()
                                    ),
                                );
                            } else {
                                ui.add(
                                    egui::DragValue::new(&mut self.export_dialog.width)
                                        .range(2..=16_384),
                                );
                                ui.label("×");
                                ui.add(
                                    egui::DragValue::new(&mut self.export_dialog.height)
                                        .range(2..=16_384),
                                );
                            }
                        });
                        ui.end_row();
                        ui.label("FPS");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut self.export_dialog.fps_numerator)
                                    .range(1..=120_000),
                            );
                            ui.label("/");
                            ui.add(
                                egui::DragValue::new(&mut self.export_dialog.fps_denominator)
                                    .range(1..=10_000),
                            );
                        });
                        ui.end_row();
                        ui.label("Captions");
                        ui.horizontal(|ui| {
                            let enabled = caption_cues.is_ok();
                            let disabled_reason =
                                caption_cues.as_ref().err().map_or("", String::as_str);
                            if ui
                                .add_enabled(enabled, egui::Button::new("Save .srt"))
                                .on_disabled_hover_text(disabled_reason)
                                .clicked()
                            {
                                caption_format = Some(CaptionFormat::Srt);
                            }
                            if ui
                                .add_enabled(enabled, egui::Button::new("Save .vtt"))
                                .on_disabled_hover_text(disabled_reason)
                                .clicked()
                            {
                                caption_format = Some(CaptionFormat::Vtt);
                            }
                        });
                        ui.end_row();
                    });
                ui.separator();
                if let Some(job) = &self.export_job {
                    let fraction = if job.progress.total_frames == 0 {
                        0.0
                    } else {
                        job.progress.completed_frames as f32 / job.progress.total_frames as f32
                    };
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .show_percentage()
                            .text(format!(
                                "{} / {} frames",
                                job.progress.completed_frames, job.progress.total_frames
                            )),
                    );
                    ui.colored_label(color::TEXT_SECONDARY, "Encoding on background worker");
                    if ui
                        .add(egui::Button::image_and_text(
                            Icon::Stop.image(size::ICON_MD),
                            "Cancel export",
                        ))
                        .clicked()
                    {
                        cancel = true;
                    }
                } else {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !export_blocked,
                                egui::Button::image_and_text(
                                    Icon::Export.image(size::ICON_MD),
                                    "Export MP4",
                                )
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)),
                            )
                            .on_disabled_hover_text(if color_pipeline_reset_needed {
                                "Reset the project colour pipeline before exporting."
                            } else {
                                "Resolve every blocking delivery-conformance issue before exporting."
                            })
                            .clicked()
                        {
                            start = true;
                        }
                    });
                }
                });
            });
        self.export_dialog.open = open || self.export_job.is_some();
        if browse {
            self.choose_export_output();
        }
        if reset_color_pipeline {
            self.send_operation(Operation::SetColorContext {
                color_context: kinewright_core::ColorContext::sdr_rec709(),
            });
        }
        if start {
            self.start_export();
        }
        if let (Some(format), Ok(cues)) = (caption_format, caption_cues) {
            self.save_caption_sidecar(format, &cues);
        }
        if cancel && let Some(job) = &self.export_job {
            job.cancellation.cancel();
            "Cancelling export…".clone_into(&mut self.status);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use kinewright_core::{
        AssetId, Clip, ClipContent, ClipId, ColorContext, ColorDescription, ColorPrimaries,
        ColorProvenance, ColorTransfer, ExportMediaPreflightIssue, MediaAsset,
        MediaAvailabilityKind, MediaAvailabilityStatus, MediaKind, Track, TrackId, TrackKind,
    };

    use super::*;

    fn ready_conformance() -> ExportConformance {
        ExportConformance::default()
    }

    /// One BT.2020/PQ source on the timeline. Its file exists so the only
    /// blocking finding is the source colour contract.
    fn document_with_hdr_source() -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: "hdr-master.mov".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: ColorDescription {
                primaries: ColorPrimaries::Bt2020,
                transfer: ColorTransfer::Smpte2084,
                provenance: ColorProvenance::UserOverride,
                ..ColorContext::sdr_rec709().delivery
            },
        };
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
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
                }],
            }],
            media_pool: vec![asset],
            color_context: ColorContext::sdr_rec709(),
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    #[test]
    fn unsupported_source_colour_refuses_the_export_with_its_code_and_field() {
        let conformance = export_conformance(&document_with_hdr_source(), None, 50, 50)
            .expect("source colour findings are reported, not returned as an error");

        assert!(!conformance.export_ready());
        let issue = conformance
            .blocking
            .iter()
            .find(|issue| issue.code == "unsupported_source_color")
            .expect("BT.2020/PQ source must block the export");
        assert_eq!(issue.asset, Some(AssetId(1)));
        assert!(issue.message.contains("hdr-master.mov"));
        assert!(issue.message.contains("code=unsupported_source_primaries"));
        assert!(issue.message.contains("field=primaries"));
        assert!(issue.message.contains("observed=Bt2020"));
        assert!(issue.message.contains("allowed="));
        assert!(
            issue
                .message
                .contains("Apply an explicit supported source-colour override")
        );

        let summary = conformance.summary();
        assert!(summary.contains("unsupported_source_color"));
        assert!(summary.contains("field=primaries"));

        // The encoder is never reached for a refused document.
        let export_called = Cell::new(false);
        let result = run_export_after_preflight(
            &conformance,
            &ExportMediaPreflightReport {
                checked_assets: vec![AssetId(1)],
                issues: Vec::new(),
            },
            || {
                export_called.set(true);
                Ok(())
            },
        );
        assert!(!export_called.get());
        assert!(matches!(
            result,
            Err(MediaError::Backend(message)) if message.contains("unsupported_source_color")
        ));
    }

    #[test]
    fn a_conformant_managed_sdr_document_stays_exportable_and_keeps_advisories_visible() {
        let mut document = document_with_hdr_source();
        document.media_pool[0].color_description = ColorContext::sdr_rec709().delivery;
        document.tracks[0].clips[0]
            .effects
            .push(kinewright_core::Effect {
                id: kinewright_core::EffectId(1),
                name: "brightness".to_owned(),
                parameters: std::collections::BTreeMap::new(),
                keyframes: std::collections::BTreeMap::new(),
            });

        let conformance =
            export_conformance(&document, None, 50, 50).expect("conformance must run");

        assert!(conformance.export_ready());
        assert!(
            conformance
                .advisory
                .iter()
                .any(|issue| issue.code == "legacy_colour_semantics"),
            "legacy colour semantics stay visible without blocking the export"
        );
    }

    #[test]
    fn every_dialog_aspect_maps_to_the_delivery_profile_the_agent_queue_gates_on() {
        assert_eq!(export_delivery_profile(None), DeliveryProfile::SourceMaster);
        for aspect in DeliveryAspect::ALL {
            let profile = export_delivery_profile(Some(aspect));
            assert_eq!(profile.aspect(), Some(aspect));
        }
    }

    /// The gate validates the delivery profile's raster while the encoder
    /// renders the dialog's frame size. Under a delivery aspect those must be
    /// the same number, so the frame size is locked to the profile.
    #[test]
    fn a_delivery_aspect_locks_the_frame_size_to_the_raster_the_gate_validates() {
        let source = (3_840, 2_160);
        for aspect in DeliveryAspect::ALL {
            let locked = export_frame_size(Some(aspect), 1_234, 5_678);
            assert_eq!(
                locked,
                aspect.resolution(),
                "an edited frame size cannot override a delivery aspect"
            );
            assert_eq!(
                locked,
                export_delivery_profile(Some(aspect)).resolution(source),
                "the locked raster is exactly the one delivery_conformance checks"
            );
        }

        // Master export has no profile raster to conflict with.
        assert_eq!(export_frame_size(None, 1_234, 5_678), (1_234, 5_678));
        assert_eq!(
            DeliveryProfile::SourceMaster.resolution(source),
            source,
            "a master export renders the project raster, whatever the dialog holds"
        );
    }

    /// `Info` findings are descriptive and emitted once per cut. Showing them
    /// beside real warnings in a fixed-size window buries what has to be read.
    #[test]
    fn informational_findings_are_dropped_and_warnings_are_kept() {
        let report = DeliveryConformanceReport {
            issues: vec![
                qa_issue(QaSeverity::Error, "unsupported_delivery_color"),
                qa_issue(QaSeverity::Warning, "legacy_colour_semantics"),
                qa_issue(QaSeverity::Info, "abrupt_cut"),
                qa_issue(QaSeverity::Info, "abrupt_cut"),
            ],
            ..conformance_report_shell()
        };

        let conformance = ExportConformance::from_report(&report);
        assert_eq!(conformance.blocking.len(), 1);
        assert_eq!(
            conformance.advisory.len(),
            1,
            "only actionable warnings reach the dialog: {:?}",
            conformance.advisory
        );
        assert_eq!(conformance.advisory[0].code, "legacy_colour_semantics");
        assert!(!conformance.export_ready());
    }

    /// The advisory list is capped, and the remainder is counted rather than
    /// silently dropped.
    #[test]
    fn the_advisory_list_is_capped_and_reports_the_remainder() {
        let report = DeliveryConformanceReport {
            issues: (0..MAX_ADVISORY_LINES + 3)
                .map(|_| qa_issue(QaSeverity::Warning, "retimed_audio_muted"))
                .collect(),
            ..conformance_report_shell()
        };

        let conformance = ExportConformance::from_report(&report);
        assert!(conformance.export_ready());
        assert_eq!(conformance.advisory.len(), MAX_ADVISORY_LINES + 3);
        assert_eq!(
            conformance
                .advisory
                .len()
                .checked_sub(MAX_ADVISORY_LINES)
                .unwrap(),
            3,
            "the dialog shows the cap and says how many it withheld"
        );
    }

    fn qa_issue(severity: QaSeverity, code: &str) -> QaIssue {
        QaIssue {
            severity,
            code: code.to_owned(),
            message: format!("{code} finding"),
            asset: None,
            track: None,
            clip: None,
            range: None,
        }
    }

    /// A conformance report carrying no issues, for tests that only care about
    /// how the dialog partitions them.
    fn conformance_report_shell() -> DeliveryConformanceReport {
        let mut document = document_with_hdr_source();
        document.media_pool[0].color_description = ColorContext::sdr_rec709().delivery;
        delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
            .expect("the fixture conforms")
    }

    #[test]
    fn worker_preflight_failure_reaches_the_result_and_skips_export() {
        let export_called = Cell::new(false);
        let blocked = ExportMediaPreflightReport {
            checked_assets: vec![AssetId(7)],
            issues: vec![ExportMediaPreflightIssue {
                asset: AssetId(7),
                asset_name: "changed-source".to_owned(),
                availability: MediaAvailabilityStatus {
                    kind: MediaAvailabilityKind::Changed,
                    observed_fingerprint: None,
                    reason: Some("source changed after the export was queued".to_owned()),
                },
            }],
        };

        let result = run_export_after_preflight(&ready_conformance(), &blocked, || {
            export_called.set(true);
            Ok(())
        });

        assert!(!export_called.get());
        assert!(matches!(
            result,
            Err(MediaError::Backend(message))
                if message.contains("changed-source") && message.contains("Changed")
        ));
    }
}
