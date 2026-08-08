use std::sync::Arc;

use eframe::egui;
use openreel_core::{
    AssetId, FrameRounding, Operation, TimeCode, TrackKind, map_frames_with_rounding,
    map_source_range_to_project,
};

use crate::app::OpenReelApp;
use openreel_media::timeline_source_at;

const TIMELINE_HEIGHT: f32 = 112.0;
const CLIP_HEIGHT: f32 = 56.0;
const EDGE_HANDLE_WIDTH: f32 = 7.0;

impl OpenReelApp {
    pub(crate) fn split_at_playhead(&mut self) {
        let clip = self.selected_clip.or_else(|| {
            timeline_source_at(&self.document, self.position)
                .ok()
                .flatten()
                .map(|source| source.clip)
        });
        let Some(clip) = clip else {
            self.record_error(
                "Operations",
                "No clip is selected or active at the playhead",
            );
            return;
        };
        self.send_operation(Operation::SplitClip {
            clip,
            at: self.position,
        });
    }

    pub(crate) fn delete_selected(&mut self) {
        let Some(clip) = self.selected_clip else {
            self.record_error("Operations", "Select a clip to delete");
            return;
        };
        self.send_operation(Operation::DeleteClip { clip });
    }

    pub(crate) fn timeline(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Timeline");
            if ui.button("Split (S)").clicked() {
                self.split_at_playhead();
            }
            if ui.button("Delete").clicked() {
                self.delete_selected();
            }
            if ui.button("Undo").clicked() {
                self.undo();
            }
            if ui.button("Redo").clicked() {
                self.redo();
            }
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.pixels_per_frame, 1.0..=20.0));
        });

        let document = Arc::clone(&self.document);
        let Some(track) = document
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Video)
        else {
            ui.label("No video track");
            return;
        };
        let content_frames = document
            .duration
            .0
            .max(self.position.0.saturating_add(1))
            .max(60);
        let width = ((content_frames as f32) * self.pixels_per_frame + 180.0)
            .max(ui.available_width());
        let mut pending_operation = None;
        let mut seek = None;

        egui::ScrollArea::horizontal()
            .id_salt("timeline-scroll")
            .show(ui, |ui| {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(width, TIMELINE_HEIGHT),
                    egui::Sense::click(),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, egui::Color32::from_gray(26));
                let strip = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top() + 30.0),
                    egui::pos2(rect.right(), rect.top() + 30.0 + CLIP_HEIGHT),
                );
                painter.rect_filled(strip, 2.0, egui::Color32::from_gray(42));

                let tick = i64::from(
                    (document.fps.numerator() / document.fps.denominator()).max(1),
                );
                let mut frame = 0_i64;
                while frame <= content_frames {
                    let x = rect.left() + (frame as f32) * self.pixels_per_frame;
                    painter.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.top() + 12.0)],
                        egui::Stroke::new(1.0, egui::Color32::DARK_GRAY),
                    );
                    painter.text(
                        egui::pos2(x + 3.0, rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        frame,
                        egui::FontId::monospace(10.0),
                        egui::Color32::LIGHT_GRAY,
                    );
                    frame = frame.saturating_add(tick);
                }

                for clip in &track.clips {
                    let Some(asset) = document.asset(clip.asset) else {
                        continue;
                    };
                    let Ok(duration) = map_source_range_to_project(
                        clip.source_range.clone(),
                        asset.fps,
                        document.fps,
                    ) else {
                        continue;
                    };
                    let x = rect.left()
                        + (clip.timeline_start.0 as f32) * self.pixels_per_frame;
                    let clip_width = ((duration.0 as f32) * self.pixels_per_frame).max(30.0);
                    let clip_rect = egui::Rect::from_min_size(
                        egui::pos2(x, strip.top()),
                        egui::vec2(clip_width, CLIP_HEIGHT),
                    );
                    let body_rect = egui::Rect::from_min_max(
                        egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.top()),
                        egui::pos2(clip_rect.right() - EDGE_HANDLE_WIDTH, clip_rect.bottom()),
                    );
                    let body = ui
                        .interact(
                            body_rect,
                            ui.make_persistent_id(("clip-body", clip.id.0)),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_text("Drag to move; click to select");
                    let left_rect = egui::Rect::from_min_max(
                        clip_rect.min,
                        egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.bottom()),
                    );
                    let right_rect = egui::Rect::from_min_max(
                        egui::pos2(clip_rect.right() - EDGE_HANDLE_WIDTH, clip_rect.top()),
                        clip_rect.max,
                    );
                    let left = ui
                        .interact(
                            left_rect,
                            ui.make_persistent_id(("clip-left", clip.id.0)),
                            egui::Sense::drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                    let right = ui
                        .interact(
                            right_rect,
                            ui.make_persistent_id(("clip-right", clip.id.0)),
                            egui::Sense::drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);

                    if body.clicked() {
                        self.selected_clip = Some(clip.id);
                    }
                    if body.drag_stopped() {
                        let delta = (body.drag_delta().x / self.pixels_per_frame).round() as i64;
                        if delta != 0 {
                            pending_operation = Some(Operation::MoveClip {
                                clip: clip.id,
                                to_track: track.id,
                                to: TimeCode(clip.timeline_start.0.saturating_add(delta).max(0)),
                            });
                        }
                    }
                    if left.drag_stopped() {
                        let project_delta =
                            (left.drag_delta().x / self.pixels_per_frame).round() as i64;
                        let source_delta = project_delta_to_source(
                            project_delta,
                            document.fps,
                            asset.fps,
                        );
                        let new_start = TimeCode(
                            clip.source_range
                                .start
                                .0
                                .saturating_add(source_delta)
                                .clamp(0, clip.source_range.end.0.saturating_sub(1)),
                        );
                        if new_start != clip.source_range.start {
                            pending_operation = Some(Operation::TrimClip {
                                clip: clip.id,
                                new_source: new_start..clip.source_range.end,
                            });
                        }
                    }
                    if right.drag_stopped() {
                        let project_delta =
                            (right.drag_delta().x / self.pixels_per_frame).round() as i64;
                        let source_delta = project_delta_to_source(
                            project_delta,
                            document.fps,
                            asset.fps,
                        );
                        let new_end = TimeCode(
                            clip.source_range
                                .end
                                .0
                                .saturating_add(source_delta)
                                .clamp(clip.source_range.start.0.saturating_add(1), asset.duration.0),
                        );
                        if new_end != clip.source_range.end {
                            pending_operation = Some(Operation::TrimClip {
                                clip: clip.id,
                                new_source: clip.source_range.start..new_end,
                            });
                        }
                    }

                    let drag = if body.dragged() {
                        body.drag_delta()
                    } else {
                        egui::Vec2::ZERO
                    };
                    let draw_rect = clip_rect.translate(drag);
                    let selected = self.selected_clip == Some(clip.id);
                    let color = clip_color(clip.asset, selected);
                    painter.rect_filled(draw_rect, 4.0, color);
                    painter.rect_stroke(
                        draw_rect,
                        4.0,
                        egui::Stroke::new(
                            if selected { 2.0 } else { 1.0 },
                            if selected {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_gray(150)
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            draw_rect.min,
                            egui::pos2(draw_rect.left() + EDGE_HANDLE_WIDTH, draw_rect.bottom()),
                        ),
                        2.0,
                        egui::Color32::from_white_alpha(70),
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(draw_rect.right() - EDGE_HANDLE_WIDTH, draw_rect.top()),
                            draw_rect.max,
                        ),
                        2.0,
                        egui::Color32::from_white_alpha(70),
                    );
                    painter.text(
                        draw_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &asset.name,
                        egui::FontId::proportional(13.0),
                        egui::Color32::WHITE,
                    );
                }

                let playhead_x =
                    rect.left() + (self.position.0 as f32) * self.pixels_per_frame;
                painter.line_segment(
                    [
                        egui::pos2(playhead_x, rect.top()),
                        egui::pos2(playhead_x, rect.bottom()),
                    ],
                    egui::Stroke::new(2.0, egui::Color32::RED),
                );

                if response.clicked() {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let frame = ((pointer.x - rect.left()) / self.pixels_per_frame)
                            .round() as i64;
                        seek = Some(TimeCode(frame));
                    }
                }
            });

        if let Some(operation) = pending_operation {
            self.send_operation(operation);
        }
        if let Some(position) = seek {
            self.seek_to(position);
        }
    }
}

fn project_delta_to_source(
    project_delta: i64,
    project_fps: openreel_core::Rational,
    source_fps: openreel_core::Rational,
) -> i64 {
    let sign = project_delta.signum();
    let magnitude = TimeCode(project_delta.saturating_abs());
    map_frames_with_rounding(magnitude, project_fps, source_fps, FrameRounding::Nearest)
        .map_or(0, |frames| frames.0.saturating_mul(sign))
}

fn clip_color(asset: AssetId, selected: bool) -> egui::Color32 {
    if selected {
        return egui::Color32::from_rgb(55, 125, 210);
    }
    let seed = u8::try_from(asset.0 % 80).unwrap_or_default();
    egui::Color32::from_rgb(
        45_u8.saturating_add(seed),
        80_u8.saturating_add(seed / 2),
        135_u8.saturating_add(seed / 3),
    )
}
