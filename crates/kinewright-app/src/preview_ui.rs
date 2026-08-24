use eframe::egui;
use kinewright_core::{MediaKind, ThreePointMode, TimeCode, TrackId, TrackKind};

use crate::{
    app::KinewrightApp,
    icons::Icon,
    media_workflow::{
        paint_source_status, should_display_source_texture, source_access_is_allowed,
        source_display_state, source_edit_controls_are_enabled,
    },
    theme::{self, color, radius, space, type_size},
};

impl KinewrightApp {
    /// The monitor dock deliberately keeps Source and Program side by side.
    /// Source is a frame-accurate still/scrub monitor backed by `VisualCache`;
    /// Program remains the live project playback output.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn preview(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let (width_px, height_px) = self.focused().document.resolution;
        let aspect = height_px.max(1) as f32 / width_px.max(1) as f32;
        let panel_width = ((available.x - space::TWO) / 2.0).max(120.0);
        let frame_height = (panel_width * aspect).clamp(112.0, (available.y * 0.46).max(112.0));
        ui.columns(2, |columns| {
            self.source_viewer(&mut columns[0], frame_height);
            self.program_viewer(&mut columns[1], frame_height);
        });
    }

    #[allow(clippy::cast_precision_loss)]
    fn source_viewer(&mut self, ui: &mut egui::Ui, frame_height: f32) {
        let available_width = ui.available_width();
        let Some(asset_id) = self.focused().selected_asset else {
            Self::paint_empty_viewer(
                ui,
                egui::vec2(available_width, frame_height),
                "SOURCE",
                "Select an asset to cue Source",
                color::TEXT_MUTED,
            );
            return;
        };
        let Some(asset) = self.focused().document.asset(asset_id).cloned() else {
            self.focused_mut().reconcile_source_state();
            Self::paint_empty_viewer(
                ui,
                egui::vec2(available_width, frame_height),
                "SOURCE",
                "Source asset is no longer in this project",
                color::STATUS_DANGER,
            );
            return;
        };
        let source_status = self.source_media_status_for_asset(&asset);
        if let Some(refresh_after) = source_status.refresh_after {
            ui.ctx().request_repaint_after(refresh_after);
        }
        let source_state = source_display_state(source_status.status.as_ref());
        let revalidation_pending = self.source_edit_revalidation_pending();
        let blocked = source_state.blocks_preview();
        let source_access_allowed = source_access_is_allowed(source_state, revalidation_pending);
        let source_position = self
            .focused()
            .source_position
            .0
            .clamp(0, asset.duration.0.saturating_sub(1).max(0));
        self.focused_mut().source_position = TimeCode(source_position);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("SOURCE").font(theme::semibold(type_size::CAPTION)));
            ui.colored_label(color::TEXT_MUTED, &asset.name);
            paint_source_status(ui, source_state);
        });
        let source_texture = if source_access_allowed
            && matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo)
        {
            let analysis = std::sync::Arc::clone(&self.analysis);
            self.visual_cache
                .thumbnail(analysis.as_ref(), &asset, TimeCode(source_position), 512)
        } else {
            None
        };
        let source_texture =
            source_texture.filter(|_| should_display_source_texture(source_state, true));
        Self::paint_viewer_frame(
            ui,
            egui::vec2(available_width, frame_height),
            "SOURCE",
            source_texture.as_ref(),
            if blocked {
                source_state.label()
            } else if revalidation_pending {
                "Verifying source before edit"
            } else if !source_access_allowed {
                "Source verification required"
            } else if asset.kind == MediaKind::Audio {
                "Audio-only source (no picture)"
            } else {
                "Waiting for source frame"
            },
            if blocked {
                color::STATUS_DANGER
            } else {
                color::TEXT_MUTED
            },
        );
        self.source_controls(ui, &asset, source_state, revalidation_pending);
    }

    fn program_viewer(&mut self, ui: &mut egui::Ui, frame_height: f32) {
        let available_width = ui.available_width();
        let playhead_state = self.playhead_media_state();
        let blocked = playhead_state
            .as_ref()
            .is_some_and(|(state, _)| state.blocks_preview());
        let texture = self.texture.clone();
        Self::paint_viewer_frame(
            ui,
            egui::vec2(available_width, frame_height),
            "PROGRAM",
            if blocked { None } else { texture.as_ref() },
            if let Some((state, _)) = playhead_state {
                state.label()
            } else {
                "No timeline frame"
            },
            if blocked {
                color::STATUS_DANGER
            } else {
                color::TEXT_MUTED
            },
        );
    }

    fn paint_empty_viewer(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        label: &str,
        message: &str,
        message_color: egui::Color32,
    ) {
        Self::paint_viewer_frame(ui, size, label, None, message, message_color);
    }

    fn paint_viewer_frame(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        label: &str,
        texture: Option<&egui::TextureHandle>,
        message: &str,
        message_color: egui::Color32,
    ) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, radius::SM, color::LETTERBOX);
        theme::paint_inset_well(&painter, rect, radius::px(radius::SM));
        theme::paint_caps(
            &painter,
            rect.left_top() + egui::vec2(space::TWO, space::TWO),
            egui::Align2::LEFT_TOP,
            label,
            color::TEXT_MUTED,
        );
        if let Some(texture) = texture {
            let source = texture.size_vec2();
            if source.x > 0.0 && source.y > 0.0 {
                let inset = rect.shrink2(egui::vec2(space::FOUR, space::FOUR));
                let scale = (inset.width() / source.x).min(inset.height() / source.y);
                let image_rect = egui::Rect::from_center_size(inset.center(), source * scale);
                painter.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                return;
            }
        }
        let inset = rect.shrink2(egui::vec2(space::FOUR, space::FOUR));
        painter.rect_stroke(
            inset,
            radius::XS,
            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
            egui::StrokeKind::Inside,
        );
        Icon::Filmstrip
            .image(24.0)
            .tint(color::TEXT_MUTED)
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    inset.center() - egui::vec2(0.0, 10.0),
                    egui::vec2(24.0, 24.0),
                ),
            );
        painter.text(
            inset.center() + egui::vec2(0.0, 16.0),
            egui::Align2::CENTER_CENTER,
            message,
            egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
            message_color,
        );
    }

    #[allow(clippy::too_many_lines)]
    fn source_controls(
        &mut self,
        ui: &mut egui::Ui,
        asset: &kinewright_core::MediaAsset,
        source_state: crate::media_workflow::SourceDisplayState,
        revalidation_pending: bool,
    ) {
        let duration = asset.duration.0.max(0);
        let max_frame = duration.saturating_sub(1);
        let session = self.focused();
        let mut source_position = session.source_position.0.clamp(0, max_frame);
        let mut source_in = session.source_in.0.clamp(0, max_frame);
        let mut source_out = session.source_out.0.clamp(0, duration);
        if duration > 0 {
            source_out = source_out.max(source_in.saturating_add(1)).min(duration);
        } else {
            source_in = 0;
            source_out = 0;
        }
        let mut video_target = session.source_video_target;
        let mut audio_target = session.source_audio_target;
        if revalidation_pending {
            ui.colored_label(
                color::STATUS_WARNING,
                "Verifying source before edit… source controls are temporarily locked",
            );
        }
        ui.add_enabled_ui(!revalidation_pending, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("−1").clicked() {
                    source_position = source_position.saturating_sub(1);
                }
                if ui.small_button("+1").clicked() {
                    source_position = source_position.saturating_add(1).min(max_frame);
                }
                ui.label(format!("Frame {source_position}/{max_frame}"));
            });
            ui.add(
                egui::Slider::new(&mut source_position, 0..=max_frame)
                    .text("Source")
                    .show_value(false),
            );
            ui.horizontal(|ui| {
                if ui.button("Mark In").clicked() && duration > 0 {
                    source_in = source_position.min(duration.saturating_sub(1));
                    source_out = source_out.max(source_in.saturating_add(1)).min(duration);
                }
                if ui.button("Mark Out").clicked() && duration > 0 {
                    source_out = source_position
                        .saturating_add(1)
                        .clamp(source_in.saturating_add(1), duration);
                }
                ui.label("In");
                ui.add(egui::DragValue::new(&mut source_in).range(0..=max_frame));
                ui.label("Out");
                ui.add(egui::DragValue::new(&mut source_out).range(1..=duration.max(1)));
            });
            if duration > 0 {
                source_in = source_in.clamp(0, max_frame);
                source_out = source_out.clamp(source_in.saturating_add(1), duration);
            }

            ui.separator();
            ui.label(
                egui::RichText::new("PATCH DESTINATIONS").font(theme::semibold(type_size::CAPTION)),
            );
            let video_enabled = asset.kind.supports(TrackKind::Video);
            let audio_enabled = asset.kind.supports(TrackKind::Audio);
            Self::route_selector(
                self,
                ui,
                "Video",
                TrackKind::Video,
                video_enabled,
                &mut video_target,
            );
            Self::route_selector(
                self,
                ui,
                "Audio",
                TrackKind::Audio,
                audio_enabled,
                &mut audio_target,
            );
            let route_valid = self.valid_route(asset.kind, video_target, audio_target);
            let can_edit = source_edit_controls_are_enabled(
                source_state,
                duration,
                source_in,
                source_out,
                route_valid,
                revalidation_pending,
            );

            // Persist the exact interactive context before dispatching. The
            // async completion compares it with the live session again.
            let session = self.focused_mut();
            session.source_position = TimeCode(source_position);
            session.source_in = TimeCode(source_in);
            session.source_out = TimeCode(source_out);
            session.source_video_target = video_target;
            session.source_audio_target = audio_target;

            ui.horizontal(|ui| {
                for (label, mode) in [
                    ("Insert", ThreePointMode::Insert),
                    ("Overwrite", ThreePointMode::Overwrite),
                ] {
                    if ui.add_enabled(can_edit, egui::Button::new(label)).clicked() {
                        self.dispatch_source_edit(
                            asset.id,
                            source_in,
                            source_out,
                            video_target,
                            audio_target,
                            mode,
                        );
                    }
                }
                if !route_valid {
                    ui.colored_label(
                        color::STATUS_WARNING,
                        "Choose at least one compatible route",
                    );
                }
            });
        });
        self.focused_mut().source_position = TimeCode(source_position);
        self.focused_mut().source_in = TimeCode(source_in);
        self.focused_mut().source_out = TimeCode(source_out);
        self.focused_mut().source_video_target = video_target;
        self.focused_mut().source_audio_target = audio_target;
    }

    fn route_selector(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        kind: TrackKind,
        enabled: bool,
        target: &mut Option<TrackId>,
    ) {
        let tracks = self
            .focused()
            .document
            .tracks
            .iter()
            .filter(|track| track.kind == kind)
            .map(|track| track.id)
            .collect::<Vec<_>>();
        if !enabled {
            *target = None;
        }
        let selected = (*target).filter(|id| tracks.contains(id));
        *target = selected;
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(("source-route", label))
                .selected_text(
                    selected.map_or_else(|| "Off".to_owned(), |id| format!("{kind:?} · {id}")),
                )
                .show_ui(ui, |ui| {
                    if ui.selectable_label(selected.is_none(), "Off").clicked() {
                        *target = None;
                        ui.close();
                    }
                    for track in &tracks {
                        if ui
                            .selectable_label(
                                Some(*track) == selected,
                                format!("{kind:?} · {track}"),
                            )
                            .clicked()
                        {
                            *target = Some(*track);
                            ui.close();
                        }
                    }
                });
        });
    }

    fn valid_route(
        &self,
        kind: MediaKind,
        video_target: Option<TrackId>,
        audio_target: Option<TrackId>,
    ) -> bool {
        patch_routes_valid(&self.focused().document, kind, video_target, audio_target)
    }
}

fn patch_routes_valid(
    document: &kinewright_core::Document,
    kind: MediaKind,
    video_target: Option<TrackId>,
    audio_target: Option<TrackId>,
) -> bool {
    let valid_or_off = |target: Option<TrackId>, track_kind: TrackKind| {
        target.is_none_or(|target| {
            kind.supports(track_kind)
                && document
                    .tracks
                    .iter()
                    .any(|track| track.id == target && track.kind == track_kind)
        })
    };
    (video_target.is_some() || audio_target.is_some())
        && valid_or_off(video_target, TrackKind::Video)
        && valid_or_off(audio_target, TrackKind::Audio)
}

impl KinewrightApp {
    fn dispatch_source_edit(
        &mut self,
        asset: kinewright_core::AssetId,
        source_in: i64,
        source_out: i64,
        video_target: Option<TrackId>,
        audio_target: Option<TrackId>,
        mode: ThreePointMode,
    ) {
        if self.source_edit_revalidation_pending() {
            self.record_error(
                "Source monitor",
                "Source verification is already in progress; wait for it to finish before editing",
            );
            return;
        }
        let Some(current_asset) = self.focused().document.asset(asset).cloned() else {
            self.record_error("Source monitor", "Selected source asset no longer exists");
            return;
        };
        if self.focused().selected_asset != Some(asset) {
            self.record_error(
                "Source monitor",
                "Selected source changed before the edit could be checked",
            );
            return;
        }
        if source_in < 0 || source_out <= source_in || source_out > current_asset.duration.0 {
            self.record_error("Source monitor", "Source In/Out marks are no longer valid");
            return;
        }
        if !self.valid_route(current_asset.kind, video_target, audio_target) {
            self.record_error(
                "Source monitor",
                "Source patch destination is stale or incompatible; choose a current track",
            );
            return;
        }
        let session = self.focused();
        let pending = crate::media_workflow::PendingSourceEdit {
            session_id: session.id,
            request_id: 0,
            asset_id: asset,
            path: current_asset.path.clone(),
            fingerprint: current_asset.source_fingerprint.clone(),
            expected_revision: session.revision,
            selected_asset: session.selected_asset,
            source_position: session.source_position,
            timeline_in: session.position,
            source_in: TimeCode(source_in),
            source_out: TimeCode(source_out),
            video_target,
            audio_target,
            mode,
        };
        let Some(request_id) = self.force_source_edit_media_revalidation(&current_asset) else {
            self.record_error(
                "Source monitor",
                "Could not start mandatory source verification; no edit was applied",
            );
            return;
        };
        self.pending_source_edit = Some(crate::media_workflow::PendingSourceEdit {
            request_id,
            ..pending
        });
        self.status = format!(
            "Verifying Source before {mode:?} at revision {}…",
            self.focused().revision
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{Document, Track};

    fn document() -> Document {
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            ],
            ..Document::default()
        }
    }

    #[test]
    fn patch_routes_require_one_route_and_reject_stale_or_wrong_kind_routes() {
        let document = document();
        assert!(!patch_routes_valid(
            &document,
            MediaKind::AudioVideo,
            None,
            None
        ));
        assert!(patch_routes_valid(
            &document,
            MediaKind::AudioVideo,
            Some(TrackId(1)),
            Some(TrackId(2))
        ));
        assert!(patch_routes_valid(
            &document,
            MediaKind::Video,
            Some(TrackId(1)),
            None
        ));
        assert!(!patch_routes_valid(
            &document,
            MediaKind::Video,
            Some(TrackId(1)),
            Some(TrackId(2))
        ));
        assert!(!patch_routes_valid(
            &document,
            MediaKind::AudioVideo,
            Some(TrackId(99)),
            Some(TrackId(2))
        ));
    }
}
