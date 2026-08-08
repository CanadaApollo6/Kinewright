use eframe::egui;
use openreel_core::{MediaEngine, TimeCode};

use crate::app::OpenReelApp;

impl OpenReelApp {
    pub(crate) fn toggle_playback(&mut self) {
        if self.document.duration <= TimeCode::ZERO {
            self.record_error("Media", "Add a clip to the timeline before playing");
            return;
        }
        if self.playing {
            self.media.pause();
        } else {
            if self.position >= self.document.duration {
                self.position = TimeCode::ZERO;
            }
            self.media.play(self.position);
        }
    }

    pub(crate) fn seek_to(&mut self, position: TimeCode) {
        let maximum = self.document.duration.0.saturating_sub(1).max(0);
        self.position = TimeCode(position.0.clamp(0, maximum));
        self.media.seek(self.position);
        self.media.request_frame(self.position);
    }

    pub(crate) fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(if self.playing { "⏸ Pause" } else { "▶ Play" })
                .clicked()
            {
                self.toggle_playback();
            }
            let maximum = self.document.duration.0.saturating_sub(1).max(0);
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
