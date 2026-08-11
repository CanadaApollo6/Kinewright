use eframe::egui;
use openreel_core::TimeCode;

use crate::{
    app::OpenReelApp,
    icons::{self, Icon},
    theme::{self, color, size, space},
    timeline_ui::format_timecode,
};

impl OpenReelApp {
    pub(crate) fn toggle_playback(&mut self) {
        if self.document.duration <= TimeCode::ZERO {
            self.record_error("Media", "Add a clip to the timeline before playing");
            return;
        }
        if self.playing {
            self.playback.pause();
        } else {
            if self.position >= self.document.duration {
                self.position = TimeCode::ZERO;
            }
            self.playback.play(self.position);
        }
    }

    pub(crate) fn seek_to(&mut self, position: TimeCode) {
        let maximum = self.document.duration.0.saturating_sub(1).max(0);
        self.position = TimeCode(position.0.clamp(0, maximum));
        self.playback.seek(self.position);
        self.playback.request_frame(self.position);
    }

    pub(crate) fn transport(&mut self, ui: &mut egui::Ui) {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), size::TRANSPORT_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let leading_space = ((ui.available_width() - 430.0) / 2.0).max(0.0);
                ui.add_space(leading_space);
                if icons::transport_button(ui, Icon::StepBack, "Previous frame", false).clicked() {
                    self.seek_to(TimeCode(self.position.0.saturating_sub(1)));
                }
                let playback_icon = if self.playing {
                    Icon::Pause
                } else {
                    Icon::Play
                };
                let playback_label = if self.playing { "Pause" } else { "Play" };
                if icons::transport_button(ui, playback_icon, playback_label, self.playing)
                    .clicked()
                {
                    self.toggle_playback();
                }
                if icons::transport_button(ui, Icon::StepForward, "Next frame", false).clicked() {
                    self.seek_to(TimeCode(self.position.0.saturating_add(1)));
                }
                ui.add_space(space::TWO);
                let maximum = self.document.duration.0.saturating_sub(1).max(0);
                let mut slider_position = self.position.0.clamp(0, maximum);
                let response = ui.add_sized(
                    [150.0, size::CONTROL_HEIGHT],
                    egui::Slider::new(&mut slider_position, 0..=maximum)
                        .show_value(false)
                        .trailing_fill(true)
                        .text("Position"),
                );
                if response.drag_started() {
                    self.resume_after_scrub = self.playing;
                    if self.playing {
                        self.playback.pause();
                    }
                }
                if response.changed() {
                    self.position = TimeCode(slider_position);
                    self.playback.request_frame(self.position);
                }
                if response.drag_stopped() || (response.changed() && !response.dragged()) {
                    self.playback.seek(self.position);
                    if self.resume_after_scrub {
                        self.playback.play(self.position);
                    }
                    self.resume_after_scrub = false;
                }
                ui.add_space(space::TWO);
                ui.label(
                    egui::RichText::new(format_timecode(self.position, self.document.fps))
                        .font(theme::timecode_font())
                        .color(color::TEXT_PRIMARY),
                );
                ui.colored_label(
                    color::TEXT_MUTED,
                    egui::RichText::new(format!(
                        "/ {}",
                        format_timecode(TimeCode(maximum), self.document.fps)
                    ))
                    .font(theme::code_font()),
                );
            },
        );
    }
}
