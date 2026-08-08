use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use openreel_core::{
    Command, Core, Document, Event, FrameRounding, MediaAsset, MediaEngine, MediaError, MediaEvent,
    Operation, PlaybackState, TimeCode, map_frames_with_rounding,
};
use openreel_media::FfmpegMediaEngine;

struct OpenReelApp {
    core: Core,
    core_events: crossbeam_channel::Receiver<Event>,
    media: Arc<FfmpegMediaEngine>,
    frames: crossbeam_channel::Receiver<(TimeCode, openreel_core::FrameTexture)>,
    media_events: crossbeam_channel::Receiver<MediaEvent>,
    probe_tx: mpsc::Sender<(PathBuf, Result<MediaAsset, MediaError>)>,
    probe_rx: mpsc::Receiver<(PathBuf, Result<MediaAsset, MediaError>)>,
    document: Arc<Document>,
    texture: Option<egui::TextureHandle>,
    position: TimeCode,
    duration: TimeCode,
    playing: bool,
    resume_after_scrub: bool,
    status: String,
}

impl OpenReelApp {
    fn new(media: Arc<FfmpegMediaEngine>) -> Self {
        let document = Document::default();
        let core = Core::spawn(document.clone()).expect("default document must be valid");
        let core_events = core.subscribe().expect("Core actor must accept subscribers");
        let frames = media.frames();
        let media_events = media.events();
        let (probe_tx, probe_rx) = mpsc::channel();
        Self {
            core,
            core_events,
            media,
            frames,
            media_events,
            probe_tx,
            probe_rx,
            document: Arc::new(document),
            texture: None,
            position: TimeCode::ZERO,
            duration: TimeCode::ZERO,
            playing: false,
            resume_after_scrub: false,
            status: "Open an MP4 to begin".to_owned(),
        }
    }

    fn choose_media(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi"])
            .pick_file()
        else {
            return;
        };
        self.status = format!("Probing {}…", path.display());
        let media = Arc::clone(&self.media);
        let result_tx = self.probe_tx.clone();
        thread::Builder::new()
            .name("openreel-probe".to_owned())
            .spawn(move || {
                let result = media.probe(&path);
                let _ = result_tx.send((path, result));
            })
            .expect("failed to spawn media probe worker");
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        while let Ok((path, result)) = self.probe_rx.try_recv() {
            match result {
                Ok(asset) => {
                    self.status = format!("Opening {}…", path.display());
                    if self
                        .core
                        .send(Command::Do(Operation::AddAsset { asset }))
                        .is_err()
                    {
                        self.status = "Core actor stopped while importing media".to_owned();
                    }
                }
                Err(error) => self.status = format!("Could not open {}: {error}", path.display()),
            }
        }

        while let Ok(event) = self.core_events.try_recv() {
            match event {
                Event::DocumentChanged { doc, .. } => {
                    self.document = Arc::clone(&doc);
                    self.media.set_document(Arc::clone(&doc));
                    if let Some(asset) = doc.media_pool.last() {
                        self.duration = map_frames_with_rounding(
                            asset.duration,
                            asset.fps,
                            doc.fps,
                            FrameRounding::Ceil,
                        )
                        .unwrap_or(asset.duration);
                        self.position = TimeCode::ZERO;
                        self.playing = false;
                        self.status = format!(
                            "{} — {}×{}, {}/{} fps, {} frames",
                            asset.name,
                            asset.resolution.map_or(0, |size| size.0),
                            asset.resolution.map_or(0, |size| size.1),
                            asset.fps.numerator(),
                            asset.fps.denominator(),
                            asset.duration.0
                        );
                        self.media.request_frame(TimeCode::ZERO);
                    }
                }
                Event::OpRejected { error, .. } => {
                    self.status = format!("Import rejected: {error}");
                }
                Event::QueryResult(_) => {}
            }
        }

        while let Ok(event) = self.media_events.try_recv() {
            match event {
                MediaEvent::Position(position) => {
                    if !self.resume_after_scrub {
                        self.position = position;
                    }
                }
                MediaEvent::PlaybackStateChanged(state) => {
                    self.playing = state == PlaybackState::Playing;
                }
                MediaEvent::Error(error) => {
                    self.playing = false;
                    self.status = format!("Playback error: {error}");
                }
            }
        }

        let mut newest_frame = None;
        while let Ok(frame) = self.frames.try_recv() {
            newest_frame = Some(frame);
        }
        if let Some((at, frame)) = newest_frame {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [
                    usize::try_from(frame.width).unwrap_or_default(),
                    usize::try_from(frame.height).unwrap_or_default(),
                ],
                frame.rgba.as_slice(),
            );
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture = Some(ctx.load_texture(
                    "openreel-preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            if !self.resume_after_scrub {
                self.position = at;
            }
        }

        if self.playing {
            self.position = self.media.position();
            ctx.request_repaint_after(Duration::from_millis(10));
        }
    }

    fn toggle_playback(&mut self) {
        if self.duration <= TimeCode::ZERO {
            return;
        }
        if self.playing {
            self.media.pause();
        } else {
            if self.position >= self.duration {
                self.position = TimeCode::ZERO;
            }
            self.media.play(self.position);
        }
    }

    fn preview(&self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let preview_height = (available.y - 72.0).max(120.0);
        if let Some(texture) = &self.texture {
            let source = texture.size_vec2();
            let scale = (available.x / source.x)
                .min(preview_height / source.y)
                .min(1.0);
            let size = source * scale;
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new((texture.id(), source))
                        .fit_to_exact_size(size)
                        .maintain_aspect_ratio(true),
                );
            });
        } else {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(available.x, preview_height),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(rect, 4.0, egui::Color32::BLACK);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No media loaded",
                egui::FontId::proportional(18.0),
                egui::Color32::GRAY,
            );
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(if self.playing { "⏸ Pause" } else { "▶ Play" })
                .clicked()
            {
                self.toggle_playback();
            }
            let maximum = self.duration.0.saturating_sub(1).max(0);
            let mut slider_position = self.position.0.clamp(0, maximum);
            let response = ui.add_enabled(
                maximum > 0,
                egui::Slider::new(&mut slider_position, 0..=maximum)
                    .show_value(false)
                    .text("Position"),
            );
            if response.drag_started() {
                self.resume_after_scrub = self.playing;
                if self.playing {
                    self.media.pause();
                }
            }
            if response.changed() {
                self.position = TimeCode(slider_position);
                self.media.request_frame(self.position);
            }
            if response.drag_stopped() || (response.changed() && !response.dragged()) {
                self.media.seek(self.position);
                if self.resume_after_scrub {
                    self.media.play(self.position);
                }
                self.resume_after_scrub = false;
            }
            ui.monospace(format!("{} / {}", self.position.0, maximum));
        });
    }
}

impl eframe::App for OpenReelApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_background(ui.ctx());
        if ui.input(|input| input.key_pressed(egui::Key::Space))
            && !ui.ctx().egui_wants_keyboard_input()
        {
            self.toggle_playback();
        }

        ui.horizontal(|ui| {
            if ui.button("Open media…").clicked() {
                self.choose_media();
            }
            ui.label(&self.status);
        });
        ui.separator();
        self.preview(ui);
        ui.separator();
        self.transport(ui);
    }
}

fn main() -> eframe::Result {
    let media = Arc::new(FfmpegMediaEngine::new().expect("FFmpeg media engine must initialize"));
    eframe::run_native(
        "OpenReel",
        eframe::NativeOptions::default(),
        Box::new(move |_creation_context| Ok(Box::new(OpenReelApp::new(media)))),
    )
}
