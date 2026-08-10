use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use openreel_core::{
    ExportCancellation, ExportProgress, ExportSettings, MediaEngine, MediaError, Rational, TimeCode,
};

use crate::{
    app::OpenReelApp,
    icons::Icon,
    theme::{color, size, space, type_size},
};

pub(crate) struct ExportDialog {
    pub(crate) open: bool,
    pub(crate) output: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps_numerator: u32,
    pub(crate) fps_denominator: u32,
}

pub(crate) struct ExportJob {
    pub(crate) cancellation: ExportCancellation,
    pub(crate) progress_rx: crossbeam_channel::Receiver<ExportProgress>,
    pub(crate) result_rx: mpsc::Receiver<(PathBuf, Result<(), MediaError>)>,
    pub(crate) progress: ExportProgress,
}

impl OpenReelApp {
    pub(crate) fn open_export_dialog(&mut self) {
        self.export_dialog.width = self.document.resolution.0;
        self.export_dialog.height = self.document.resolution.1;
        self.export_dialog.fps_numerator = self.document.fps.numerator();
        self.export_dialog.fps_denominator = self.document.fps.denominator();
        if let Some(project_path) = &self.project_path {
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
        if self.document.duration <= TimeCode::ZERO {
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
        let settings = ExportSettings {
            fps,
            resolution: (self.export_dialog.width, self.export_dialog.height),
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 8_000_000,
            audio_bitrate: 192_000,
            cancellation: cancellation.clone(),
        };
        let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = mpsc::channel();
        let media = Arc::clone(&self.media);
        let worker_output = output.clone();
        let spawn = thread::Builder::new()
            .name("openreel-export".to_owned())
            .spawn(move || {
                let result = media.export(&worker_output, settings, progress_tx);
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
                Err(MediaError::Cancelled) => self.status = "Export cancelled".to_owned(),
                Err(error) => self.record_error("Export", format!("Export failed: {error}")),
            }
        }
    }

    pub(crate) fn show_export_dialog(&mut self, ctx: &egui::Context) {
        if !self.export_dialog.open {
            return;
        }
        let mut open = self.export_dialog.open;
        let mut browse = false;
        let mut start = false;
        let mut cancel = false;
        egui::Window::new("Export")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("DELIVERABLE")
                        .strong()
                        .size(type_size::MICRO)
                        .color(color::TEXT_MUTED),
                );
                ui.label(
                    egui::RichText::new("H.264 video · AAC audio · MP4 container")
                        .color(color::TEXT_SECONDARY),
                );
                ui.add_space(space::TWO);
                egui::Grid::new("export-settings")
                    .num_columns(2)
                    .spacing(egui::vec2(space::THREE, space::TWO))
                    .show(ui, |ui| {
                        ui.label("Output");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.export_dialog.output)
                                    .desired_width(320.0),
                            );
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
                            .add(
                                egui::Button::image_and_text(
                                    Icon::Export.image(size::ICON_MD),
                                    "Export MP4",
                                )
                                .fill(color::ACCENT_28)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_72)),
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
        if start {
            self.start_export();
        }
        if cancel && let Some(job) = &self.export_job {
            job.cancellation.cancel();
            self.status = "Cancelling export…".to_owned();
        }
    }
}
