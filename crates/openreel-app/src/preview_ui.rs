use eframe::egui;

use crate::app::OpenReelApp;

impl OpenReelApp {
    pub(crate) fn preview(&self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let preview_height = (available.y - 220.0).max(120.0);
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
                "No timeline frame",
                egui::FontId::proportional(18.0),
                egui::Color32::GRAY,
            );
        }
    }
}
