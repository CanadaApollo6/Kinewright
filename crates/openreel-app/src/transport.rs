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
        if self.focused().document.duration <= TimeCode::ZERO {
            self.record_error("Media", "Add a clip to the timeline before playing");
            return;
        }
        if self.playing {
            self.playback.pause();
        } else {
            if self.focused().position >= self.focused().document.duration {
                self.focused_mut().position = TimeCode::ZERO;
            }
            let position = self.focused().position;
            self.playback.play(position);
        }
    }

    pub(crate) fn seek_to(&mut self, position: TimeCode) {
        let maximum = self.focused().document.duration.0.saturating_sub(1).max(0);
        let position = TimeCode(position.0.clamp(0, maximum));
        self.focused_mut().position = position;
        self.playback.seek(position);
        self.playback.request_frame(position);
    }

    pub(crate) fn transport(&mut self, ui: &mut egui::Ui) {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), size::TRANSPORT_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let leading_space = ((ui.available_width() - 430.0) / 2.0).max(0.0);
                ui.add_space(leading_space);
                if icons::transport_button(ui, Icon::StepBack, "Previous frame", false).clicked() {
                    let position = self.focused().position;
                    self.seek_to(TimeCode(position.0.saturating_sub(1)));
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
                    let position = self.focused().position;
                    self.seek_to(TimeCode(position.0.saturating_add(1)));
                }
                ui.add_space(space::TWO);
                let maximum = self.focused().document.duration.0.saturating_sub(1).max(0);
                let mut slider_position = self.focused().position.0.clamp(0, maximum);
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
                    let position = TimeCode(slider_position);
                    self.focused_mut().position = position;
                    self.playback.request_frame(position);
                }
                if response.drag_stopped() || (response.changed() && !response.dragged()) {
                    let position = self.focused().position;
                    self.playback.seek(position);
                    if self.resume_after_scrub {
                        self.playback.play(position);
                    }
                    self.resume_after_scrub = false;
                }
                ui.add_space(space::TWO);
                let position = self.focused().position;
                let fps = self.focused().document.fps;
                ui.label(
                    egui::RichText::new(format_timecode(position, fps))
                        .font(theme::timecode_font())
                        .color(color::TEXT_PRIMARY),
                );
                ui.colored_label(
                    color::TEXT_MUTED,
                    egui::RichText::new(format!("/ {}", format_timecode(TimeCode(maximum), fps)))
                        .font(theme::code_font()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.master_output_meter(ui);
                });
            },
        );
    }

    fn master_output_meter(&mut self, ui: &mut egui::Ui) {
        const DECAY_PER_SECOND: f32 = 0.9;
        let live = if self.playing {
            self.playback.output_peaks()
        } else {
            [0.0; 2]
        };
        let elapsed = ui.input(|input| input.stable_dt).clamp(0.0, 0.1);
        for (display, peak) in self.meter_levels.iter_mut().zip(live) {
            let target = peak_to_meter_level(peak);
            *display = target.max((*display - DECAY_PER_SECOND * elapsed).max(0.0));
        }
        if self.meter_levels.iter().any(|level| *level > 0.0) {
            ui.ctx().request_repaint();
        }

        // A left-to-right horizontal nested bare in the transport's
        // right-to-left layout overlaps its own children (the thread-row
        // lesson); the meter block allocates its exact size instead.
        ui.allocate_ui_with_layout(
            egui::vec2(124.0, 14.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for level in self.meter_levels {
                        draw_meter_bar(ui, level);
                    }
                });
                ui.colored_label(color::TEXT_MUTED, "L/R");
            },
        );
    }
}

fn draw_meter_bar(ui: &mut egui::Ui, level: f32) {
    const WIDTH: f32 = 88.0;
    const HEIGHT: f32 = 4.0;
    const WARNING_START: f32 = 0.8;
    const DANGER_START: f32 = 0.95;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(WIDTH, HEIGHT), egui::Sense::hover());
    ui.painter().rect_filled(rect, 1.0, color::SURFACE_ACTIVE);

    let level = level.clamp(0.0, 1.0);
    draw_meter_segment(
        ui,
        rect,
        0.0,
        level.min(WARNING_START),
        color::STATUS_SUCCESS,
    );
    draw_meter_segment(
        ui,
        rect,
        WARNING_START,
        level.min(DANGER_START),
        color::STATUS_WARNING,
    );
    draw_meter_segment(ui, rect, DANGER_START, level, color::STATUS_DANGER);
}

fn draw_meter_segment(ui: &egui::Ui, rect: egui::Rect, start: f32, end: f32, fill: egui::Color32) {
    if end <= start {
        return;
    }
    let left = egui::lerp(rect.x_range(), start);
    let right = egui::lerp(rect.x_range(), end);
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(right, rect.bottom()),
        ),
        0.0,
        fill,
    );
}

fn peak_to_meter_level(peak: f32) -> f32 {
    const FLOOR_DB: f32 = -60.0;
    if peak <= 0.0 {
        return 0.0;
    }
    let db = (20.0 * peak.log10()).clamp(FLOOR_DB, 0.0);
    (db - FLOOR_DB) / -FLOOR_DB
}

#[cfg(test)]
mod tests {
    use super::peak_to_meter_level;

    #[test]
    fn meter_mapping_uses_a_minus_sixty_db_floor() {
        for (peak, expected) in [
            (0.0, 0.0),
            (0.001, 0.0),
            (0.25, 0.799_313_3),
            (1.0, 1.0),
            (2.0, 1.0),
        ] {
            assert!((peak_to_meter_level(peak) - expected).abs() < 1.0e-6);
        }
    }
}
