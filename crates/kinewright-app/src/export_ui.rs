use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use kinewright_core::{
    CaptionCue, DeliveryAspect, DeliveryVariant, ExportCancellation, ExportMediaPreflightReport,
    ExportProgress, ExportSettings, MediaError, Operation, Rational, TimeCode,
    document_for_delivery_variant, export_media_preflight, srt, vtt,
};

use crate::{
    app::KinewrightApp,
    color_ui::{color_pipeline_summary, managed_sdr_reset_needed},
    icons::Icon,
    theme::{self, color, size, space},
};

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

fn run_export_after_media_preflight(
    report: &ExportMediaPreflightReport,
    export: impl FnOnce() -> Result<(), MediaError>,
) -> Result<(), MediaError> {
    if report.export_ready() {
        export()
    } else {
        Err(MediaError::Backend(report.summary()))
    }
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

    pub(crate) fn start_export(&mut self) {
        if self.export_job.is_some() {
            return;
        }
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
        let document = if let Some(aspect) = self.export_dialog.delivery_aspect {
            let variant = match DeliveryVariant::new(
                aspect,
                self.export_dialog.focus_x_percent,
                self.export_dialog.focus_y_percent,
            ) {
                Ok(variant) => variant,
                Err(error) => {
                    self.record_error("Export", error.to_string());
                    return;
                }
            };
            match document_for_delivery_variant(&self.focused().document, variant) {
                Ok(document) => Arc::new(document),
                Err(error) => {
                    self.record_error("Export", error.to_string());
                    return;
                }
            }
        } else {
            Arc::clone(&self.focused().document)
        };
        let media_preflight = export_media_preflight(&document, self.analysis.as_ref());
        if !media_preflight.export_ready() {
            self.record_error("Export", media_preflight.summary());
            return;
        }
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
                let result = run_export_after_media_preflight(&media_preflight, || {
                    media.export_document(worker_document, &worker_output, settings, progress_tx)
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
        egui::Window::new("Export")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
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
                            ui.add(
                                egui::DragValue::new(&mut self.export_dialog.width)
                                    .range(2..=16_384),
                            );
                            ui.label("×");
                            ui.add(
                                egui::DragValue::new(&mut self.export_dialog.height)
                                    .range(2..=16_384),
                            );
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
                                !color_pipeline_reset_needed,
                                egui::Button::image_and_text(
                                    Icon::Export.image(size::ICON_MD),
                                    "Export MP4",
                                )
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)),
                            )
                            .on_disabled_hover_text(
                                "Reset the project colour pipeline before exporting.",
                            )
                            .clicked()
                        {
                            start = true;
                        }
                    });
                }
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
        AssetId, ExportMediaPreflightIssue, MediaAvailabilityKind, MediaAvailabilityStatus,
    };

    use super::*;

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

        let result = run_export_after_media_preflight(&blocked, || {
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
