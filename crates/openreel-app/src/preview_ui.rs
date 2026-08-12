use eframe::egui;

use crate::{
    app::OpenReelApp,
    icons::Icon,
    theme::{color, radius, space, type_size},
};

impl OpenReelApp {
    // Project pixel dimensions are intentionally projected into egui's f32 coordinate space.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn preview(&self, ui: &mut egui::Ui) {
        // The monitor owns its dock (M24): its height follows the dock width
        // at the project aspect, capped so the transport and inspector below
        // always keep room.
        let available = ui.available_size();
        let (width_px, height_px) = self.focused().document.resolution;
        let aspect = height_px.max(1) as f32 / width_px.max(1) as f32;
        let preview_height =
            (available.x * aspect + space::FOUR * 2.0).clamp(160.0, (available.y * 0.6).max(160.0));
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(available.x, preview_height),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, radius::SM, color::CANVAS);
        painter.text(
            rect.left_top() + egui::vec2(space::TWO, space::TWO),
            egui::Align2::LEFT_TOP,
            "PROGRAM",
            egui::FontId::new(type_size::MICRO, egui::FontFamily::Proportional),
            color::TEXT_MUTED,
        );
        if let Some(texture) = &self.texture {
            let source = texture.size_vec2();
            let inset = rect.shrink2(egui::vec2(space::FOUR, space::FOUR));
            let scale = (inset.width() / source.x).min(inset.height() / source.y);
            let image_rect = egui::Rect::from_center_size(inset.center(), source * scale);
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            painter.rect_stroke(
                image_rect,
                radius::XS,
                egui::Stroke::new(1.0, color::BORDER_STRONG),
                egui::StrokeKind::Inside,
            );
        } else {
            // Empty state: a letterboxed screen at the project aspect, so the
            // monitor reads as a waiting display instead of a void.
            let (width, height) = self.focused().document.resolution;
            let aspect = egui::vec2(width.max(1) as f32, height.max(1) as f32);
            let inset = rect.shrink2(egui::vec2(space::FOUR, space::FOUR));
            let scale = (inset.width() / aspect.x).min(inset.height() / aspect.y);
            let screen_rect = egui::Rect::from_center_size(inset.center(), aspect * scale);
            painter.rect_filled(screen_rect, radius::XS, color::MEDIA_SHADOW);
            painter.rect_stroke(
                screen_rect,
                radius::XS,
                egui::Stroke::new(1.0, color::BORDER_SUBTLE),
                egui::StrokeKind::Inside,
            );
            Icon::Filmstrip
                .image(28.0)
                .tint(color::TEXT_MUTED)
                .paint_at(
                    ui,
                    egui::Rect::from_center_size(
                        screen_rect.center() - egui::vec2(0.0, 12.0),
                        egui::vec2(28.0, 28.0),
                    ),
                );
            painter.text(
                screen_rect.center() + egui::vec2(0.0, 16.0),
                egui::Align2::CENTER_CENTER,
                "No timeline frame",
                egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
                color::TEXT_MUTED,
            );
        }
    }
}
