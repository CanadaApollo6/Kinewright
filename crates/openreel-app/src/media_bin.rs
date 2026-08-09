use std::{sync::Arc, thread};

use eframe::egui;
use openreel_core::{AssetId, MediaEngine, MediaKind, Operation, TimeCode};

use crate::{
    app::OpenReelApp,
    icons::{self, Icon},
    theme::{self, color, radius, size, space, type_size},
    timeline_ui::format_timecode,
};

impl OpenReelApp {
    pub(crate) fn choose_media(&mut self) {
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

    pub(crate) fn add_asset_to_timeline(&mut self, asset_id: AssetId) {
        let Some(asset) = self.document.asset(asset_id) else {
            self.record_error("Operations", format!("Asset {asset_id} no longer exists"));
            return;
        };
        let Some(track) = self
            .document
            .tracks
            .iter()
            .find(|track| asset.kind.supports(track.kind))
        else {
            self.record_error(
                "Operations",
                format!("No compatible track exists for {}", asset.name),
            );
            return;
        };
        self.send_operation(Operation::AddClip {
            track: track.id,
            asset: asset.id,
            at: self.document.duration,
            source: TimeCode::ZERO..asset.duration,
        });
        self.selected_asset = Some(asset_id);
    }

    pub(crate) fn media_bin(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Media");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button =
                    egui::Button::image_and_text(Icon::Import.image(size::ICON_MD), "Import")
                        .image_tint_follows_text_color(true)
                        .corner_radius(radius::SM);
                if ui.add(button).on_hover_text("Import media…").clicked() {
                    self.choose_media();
                }
            });
        });
        ui.add_space(space::ONE);
        ui.separator();
        ui.add_space(space::ONE);
        if self.document.media_pool.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(space::EIGHT);
                Icon::Filmstrip
                    .image(32.0)
                    .tint(color::TEXT_MUTED)
                    .paint_at(
                        ui,
                        egui::Rect::from_center_size(
                            ui.next_widget_position()
                                + egui::vec2(ui.available_width() / 2.0, 16.0),
                            egui::vec2(32.0, 32.0),
                        ),
                    );
                ui.add_space(40.0);
                ui.label("No media imported");
                ui.colored_label(color::TEXT_MUTED, "Import a clip to begin editing.");
            });
            return;
        }
        let assets = self.document.media_pool.clone();
        let media = Arc::clone(&self.media);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for asset in assets {
                    let selected = self.selected_asset == Some(asset.id);
                    let response = theme::card_frame(selected).show(ui, |ui| {
                        let width = ui.available_width().max(120.0);
                        let height = (width * 9.0 / 16.0).clamp(72.0, 126.0);
                        let (thumbnail_rect, thumbnail_response) =
                            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
                        ui.painter()
                            .rect_filled(thumbnail_rect, radius::SM, color::MEDIA_SHADOW);
                        if ui.clip_rect().intersects(thumbnail_rect)
                            && matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo)
                        {
                            if let Some(texture) = self.visual_cache.thumbnail(
                                media.as_ref(),
                                &asset,
                                TimeCode::ZERO,
                                256,
                            ) {
                                ui.painter().image(
                                    texture.id(),
                                    thumbnail_rect,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    color::MEDIA_TINT_78,
                                );
                            }
                        } else {
                            Icon::Waveform.image(28.0).tint(color::TEXT_MUTED).paint_at(
                                ui,
                                egui::Rect::from_center_size(
                                    thumbnail_rect.center(),
                                    egui::vec2(28.0, 28.0),
                                ),
                            );
                        }
                        let duration = format_timecode(asset.duration, asset.fps);
                        let duration_size = egui::vec2(76.0, 18.0);
                        let badge = egui::Rect::from_min_size(
                            thumbnail_rect.right_bottom()
                                - duration_size
                                - egui::vec2(space::ONE, space::ONE),
                            duration_size,
                        );
                        ui.painter()
                            .rect_filled(badge, radius::XS, color::MEDIA_SCRIM_78);
                        ui.painter().text(
                            badge.center(),
                            egui::Align2::CENTER_CENTER,
                            duration,
                            egui::FontId::new(type_size::CAPTION, egui::FontFamily::Monospace),
                            color::TEXT_PRIMARY,
                        );
                        ui.add_space(space::ONE);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.strong(&asset.name);
                                ui.colored_label(color::TEXT_MUTED, asset_metadata(&asset));
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icons::button(ui, Icon::Add, "Add to timeline").clicked() {
                                        self.add_asset_to_timeline(asset.id);
                                    }
                                },
                            );
                        });
                        thumbnail_response
                    });
                    let card_response = ui.interact(
                        response.response.rect,
                        ui.make_persistent_id(("media-card", asset.id.0)),
                        egui::Sense::click(),
                    );
                    if card_response.clicked() || response.inner.clicked() {
                        self.selected_asset = Some(asset.id);
                    }
                    ui.add_space(space::ONE);
                }
            });
    }
}

fn asset_metadata(asset: &openreel_core::MediaAsset) -> String {
    let kind = match asset.kind {
        MediaKind::Video => "VIDEO",
        MediaKind::Audio => "AUDIO",
        MediaKind::AudioVideo => "A/V",
    };
    let resolution = asset
        .resolution
        .map(|(width, height)| format!(" · {width}×{height}"))
        .unwrap_or_default();
    format!(
        "{kind}{resolution} · {}/{} fps",
        asset.fps.numerator(),
        asset.fps.denominator()
    )
}
