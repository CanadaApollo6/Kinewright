use std::sync::Arc;

use eframe::egui;
use openreel_core::{
    ClipId, FrameRounding, MediaAsset, MediaEngine, MediaKind, Operation, Rational, TimeCode,
    TrackKind, map_frames_with_rounding, map_source_range_to_project,
};
use openreel_media::{WaveformData, timeline_source_at};

use crate::{
    app::OpenReelApp,
    icons::{self, Icon},
    theme::{self, color, radius, size, space, type_size},
    visual_cache::VisualCache,
};

const TRACK_LABEL_WIDTH: f32 = 76.0;
const EDGE_HANDLE_WIDTH: f32 = 6.0;
const SNAP_TOLERANCE: f32 = 8.0;
const FILMSTRIP_TILE_WIDTH: f32 = 96.0;
const THUMBNAIL_WIDTH: u32 = 128;

#[derive(Clone, Copy)]
struct ClipBounds {
    id: ClipId,
    start: i64,
    end: i64,
}

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
        let old_zoom_target = self.timeline_zoom_target;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), size::TIMELINE_TOOLBAR_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.strong("Timeline");
                ui.add_space(space::ONE);
                if icons::button(ui, Icon::Split, "Split at playhead (S)").clicked() {
                    self.split_at_playhead();
                }
                if icons::button(ui, Icon::Delete, "Delete selected clip").clicked() {
                    self.delete_selected();
                }
                ui.separator();
                if icons::button(ui, Icon::Undo, "Undo (Ctrl+Z)").clicked() {
                    self.undo();
                }
                if icons::button(ui, Icon::Redo, "Redo (Ctrl+Shift+Z)").clicked() {
                    self.redo();
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("−").min_size(egui::vec2(22.0, 22.0)))
                    .on_hover_text("Zoom out")
                    .clicked()
                {
                    self.timeline_zoom_target = (self.timeline_zoom_target / 1.25).max(0.25);
                }
                ui.add(
                    egui::Slider::new(&mut self.timeline_zoom_target, 0.25..=20.0)
                        .logarithmic(true)
                        .show_value(false),
                )
                .on_hover_text(format!(
                    "Timeline zoom · {:.2} px per frame",
                    self.timeline_zoom_target
                ));
                if ui
                    .add(egui::Button::new("+").min_size(egui::vec2(22.0, 22.0)))
                    .on_hover_text("Zoom in")
                    .clicked()
                {
                    self.timeline_zoom_target = (self.timeline_zoom_target * 1.25).min(20.0);
                }
                if ui
                    .button("Fit")
                    .on_hover_text("Fit the whole project in view")
                    .clicked()
                {
                    let frames = self.document.duration.0.max(1) as f32;
                    let width = (ui.available_width() - 120.0).max(240.0);
                    self.timeline_zoom_target = (width / frames).clamp(0.25, 20.0);
                    self.timeline_scroll_target = 0.0;
                }
            },
        );

        if (old_zoom_target - self.timeline_zoom_target).abs() > f32::EPSILON {
            let playhead_before = self.position.0 as f32 * old_zoom_target;
            let playhead_after = self.position.0 as f32 * self.timeline_zoom_target;
            self.timeline_scroll_target =
                (self.timeline_scroll_target + playhead_after - playhead_before).max(0.0);
        }
        self.pixels_per_frame = ui.ctx().animate_value_with_time(
            egui::Id::new("timeline-zoom-animation"),
            self.timeline_zoom_target,
            theme::motion::NAVIGATION,
        );
        let animated_scroll = ui.ctx().animate_value_with_time(
            egui::Id::new("timeline-scroll-animation"),
            self.timeline_scroll_target,
            theme::motion::NAVIGATION,
        );

        let document = Arc::clone(&self.document);
        if document.tracks.is_empty() {
            ui.colored_label(color::TEXT_MUTED, "No tracks in this project");
            return;
        }
        let track_count = document.tracks.len().max(1) as f32;
        // Spare vertical space belongs to the editing surface: lanes stretch
        // (within bounds) so filmstrips and waveforms get taller, instead of
        // slack pooling at the bottom of the window.
        let transcript_reserve = 160.0;
        let track_height = ((ui.available_height()
            - size::RULER_HEIGHT
            - size::CONTROL_HEIGHT
            - transcript_reserve)
            / track_count)
            .clamp(size::TRACK_HEIGHT, 132.0);
        let total_height = size::RULER_HEIGHT + track_height * track_count;
        let content_frames = document
            .duration
            .0
            .max(self.position.0.saturating_add(1))
            .max(i64::from(nominal_fps(document.fps)).saturating_mul(10));
        let viewport_width = (ui.available_width() - TRACK_LABEL_WIDTH - space::TWO).max(100.0);
        let content_width =
            ((content_frames as f32) * self.pixels_per_frame + space::SIX).max(viewport_width);
        let (major_tick, minor_tick) = tick_density(self.pixels_per_frame, document.fps);
        let clip_bounds = collect_clip_bounds(&document);
        let mut pending_operation = None;
        let mut seek = None;
        let mut scrub_started = false;
        let mut scrub_stopped = false;
        let mut snap_guide = None;

        ui.horizontal_top(|ui| {
            paint_track_labels(ui, &document, total_height, track_height);
            let output = egui::ScrollArea::horizontal()
                .id_salt("timeline-scroll")
                .auto_shrink([false, false])
                .max_height(total_height + size::CONTROL_HEIGHT)
                .horizontal_scroll_offset(animated_scroll)
                .show(ui, |ui| {
                    let (rect, canvas_response) = ui.allocate_exact_size(
                        egui::vec2(content_width, total_height),
                        egui::Sense::click(),
                    );
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, radius::NONE, color::CANVAS);
                    paint_ruler(
                        &painter,
                        rect,
                        ui.clip_rect(),
                        self.pixels_per_frame,
                        document.fps,
                        content_frames,
                        major_tick,
                        minor_tick,
                    );

                    let mut clip_pointer_interaction = false;
                    for (track_index, track) in document.tracks.iter().enumerate() {
                        let lane_top = rect.top()
                            + size::RULER_HEIGHT
                            + track_index as f32 * track_height;
                        let lane = egui::Rect::from_min_size(
                            egui::pos2(rect.left(), lane_top),
                            egui::vec2(rect.width(), track_height),
                        );
                        painter.rect_filled(lane, radius::NONE, color::SURFACE);
                        painter.line_segment(
                            [lane.left_bottom(), lane.right_bottom()],
                            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
                        );

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
                            let x =
                                rect.left() + clip.timeline_start.0 as f32 * self.pixels_per_frame;
                            let clip_width = (duration.0 as f32 * self.pixels_per_frame).max(24.0);
                            let clip_rect = egui::Rect::from_min_size(
                                egui::pos2(x, lane.top() + space::ONE),
                                egui::vec2(clip_width, lane.height() - space::TWO),
                            );
                            if !clip_rect.intersects(ui.clip_rect()) {
                                continue;
                            }
                            let body_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_rect.left() + EDGE_HANDLE_WIDTH, clip_rect.top()),
                                egui::pos2(
                                    clip_rect.right() - EDGE_HANDLE_WIDTH,
                                    clip_rect.bottom(),
                                ),
                            );
                            let body = ui
                                .interact(
                                    body_rect,
                                    ui.make_persistent_id(("clip-body", clip.id.0)),
                                    egui::Sense::click_and_drag(),
                                )
                                .on_hover_text("Select or drag clip");
                            let left = ui
                                .interact(
                                    egui::Rect::from_min_max(
                                        clip_rect.min,
                                        egui::pos2(
                                            clip_rect.left() + EDGE_HANDLE_WIDTH,
                                            clip_rect.bottom(),
                                        ),
                                    ),
                                    ui.make_persistent_id(("clip-left", clip.id.0)),
                                    egui::Sense::drag(),
                                )
                                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                            let right = ui
                                .interact(
                                    egui::Rect::from_min_max(
                                        egui::pos2(
                                            clip_rect.right() - EDGE_HANDLE_WIDTH,
                                            clip_rect.top(),
                                        ),
                                        clip_rect.max,
                                    ),
                                    ui.make_persistent_id(("clip-right", clip.id.0)),
                                    egui::Sense::drag(),
                                )
                                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);

                            clip_pointer_interaction |= body.hovered()
                                || body.dragged()
                                || left.hovered()
                                || left.dragged()
                                || right.hovered()
                                || right.dragged();
                            if body.clicked() {
                                self.selected_clip = Some(clip.id);
                                self.selected_asset = Some(clip.asset);
                            }

                            let interacting = body.dragged()
                                || body.drag_stopped()
                                || left.dragged()
                                || left.drag_stopped()
                                || right.dragged()
                                || right.drag_stopped();
                            let candidates = if interacting {
                                snap_candidates(&clip_bounds, clip.id, self.position.0)
                            } else {
                                Vec::new()
                            };
                            let project_delta =
                                (body.drag_delta().x / self.pixels_per_frame).round() as i64;
                            let raw_start =
                                clip.timeline_start.0.saturating_add(project_delta).max(0);
                            let (snapped_start, body_guide) = snap_move(
                                raw_start,
                                duration.0,
                                &candidates,
                                minor_tick,
                                self.pixels_per_frame,
                            );
                            if body.dragged() {
                                snap_guide = body_guide.or(snap_guide);
                            }
                            if body.drag_stopped() && snapped_start != clip.timeline_start.0 {
                                pending_operation = Some(Operation::MoveClip {
                                    clip: clip.id,
                                    to_track: track.id,
                                    to: TimeCode(snapped_start),
                                });
                            }

                            if left.drag_stopped() {
                                let raw_edge = clip
                                    .timeline_start
                                    .0
                                    .saturating_add(
                                        (left.drag_delta().x / self.pixels_per_frame).round()
                                            as i64,
                                    )
                                    .max(0);
                                let (edge, _) = nearest_snap(
                                    raw_edge,
                                    &candidates,
                                    minor_tick,
                                    self.pixels_per_frame,
                                );
                                let source_delta = project_delta_to_source(
                                    edge.saturating_sub(clip.timeline_start.0),
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
                                let clip_end = clip.timeline_start.0.saturating_add(duration.0);
                                let raw_edge = clip_end.saturating_add(
                                    (right.drag_delta().x / self.pixels_per_frame).round() as i64,
                                );
                                let (edge, _) = nearest_snap(
                                    raw_edge,
                                    &candidates,
                                    minor_tick,
                                    self.pixels_per_frame,
                                );
                                let source_delta = project_delta_to_source(
                                    edge.saturating_sub(clip_end),
                                    document.fps,
                                    asset.fps,
                                );
                                let new_end = TimeCode(
                                    clip.source_range.end.0.saturating_add(source_delta).clamp(
                                        clip.source_range.start.0.saturating_add(1),
                                        asset.duration.0,
                                    ),
                                );
                                if new_end != clip.source_range.end {
                                    pending_operation = Some(Operation::TrimClip {
                                        clip: clip.id,
                                        new_source: clip.source_range.start..new_end,
                                    });
                                }
                            }

                            let draw_delta = if body.dragged() || body.drag_stopped() {
                                egui::vec2(
                                    (snapped_start - clip.timeline_start.0) as f32
                                        * self.pixels_per_frame,
                                    0.0,
                                )
                            } else {
                                egui::Vec2::ZERO
                            };
                            let draw_rect = clip_rect.translate(draw_delta);
                            let selected = self.selected_clip == Some(clip.id);
                            let dragging = body.dragged() || left.dragged() || right.dragged();
                            paint_clip(
                                &painter,
                                ui.clip_rect(),
                                &mut self.visual_cache,
                                self.media.as_ref(),
                                asset,
                                clip.source_range.clone(),
                                draw_rect,
                                body.hovered() || left.hovered() || right.hovered(),
                                selected,
                                dragging,
                            );
                        }
                    }

                    let playhead_x = rect.left() + self.position.0 as f32 * self.pixels_per_frame;
                    painter.line_segment(
                        [
                            egui::pos2(playhead_x, rect.top()),
                            egui::pos2(playhead_x, rect.bottom()),
                        ],
                        egui::Stroke::new(2.0, color::ACCENT),
                    );
                    let handle_points = vec![
                        egui::pos2(playhead_x - 5.0, rect.top()),
                        egui::pos2(playhead_x + 5.0, rect.top()),
                        egui::pos2(playhead_x + 5.0, rect.top() + 5.0),
                        egui::pos2(playhead_x, rect.top() + 9.0),
                        egui::pos2(playhead_x - 5.0, rect.top() + 5.0),
                    ];
                    painter.add(egui::Shape::convex_polygon(
                        handle_points,
                        color::ACCENT,
                        egui::Stroke::NONE,
                    ));
                    let playhead_response = ui
                        .interact(
                            egui::Rect::from_center_size(
                                egui::pos2(playhead_x, rect.top() + size::RULER_HEIGHT / 2.0),
                                egui::vec2(14.0, size::RULER_HEIGHT),
                            ),
                            ui.make_persistent_id("timeline-playhead"),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                        .on_hover_text("Drag playhead");
                    if playhead_response.drag_started() {
                        scrub_started = true;
                    }
                    if playhead_response.dragged() || playhead_response.drag_stopped() {
                        if let Some(pointer) = playhead_response.interact_pointer_pos() {
                            let raw =
                                ((pointer.x - rect.left()) / self.pixels_per_frame).round() as i64;
                            let candidates = clip_bounds
                                .iter()
                                .flat_map(|bounds| [bounds.start, bounds.end])
                                .collect::<Vec<_>>();
                            let (snapped, guide) = nearest_snap(
                                raw.max(0),
                                &candidates,
                                minor_tick,
                                self.pixels_per_frame,
                            );
                            seek = Some(TimeCode(snapped));
                            snap_guide = guide.or(snap_guide);
                        }
                    }
                    if playhead_response.drag_stopped() {
                        scrub_stopped = true;
                    }
                    if canvas_response.clicked()
                        && !clip_pointer_interaction
                        && !playhead_response.hovered()
                        && let Some(pointer) = canvas_response.interact_pointer_pos()
                    {
                        let frame =
                            ((pointer.x - rect.left()) / self.pixels_per_frame).round() as i64;
                        seek = Some(TimeCode(frame.max(0)));
                        scrub_stopped = true;
                    }

                    if let Some(guide) = snap_guide {
                        let x = rect.left() + guide as f32 * self.pixels_per_frame;
                        painter.line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(1.0, color::ACCENT),
                        );
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(x, rect.top() + size::RULER_HEIGHT - 7.0),
                                egui::pos2(x + 4.0, rect.top() + size::RULER_HEIGHT - 3.0),
                                egui::pos2(x, rect.top() + size::RULER_HEIGHT + 1.0),
                                egui::pos2(x - 4.0, rect.top() + size::RULER_HEIGHT - 3.0),
                            ],
                            color::ACCENT,
                            egui::Stroke::NONE,
                        ));
                    }
                });
            if (output.state.offset.x - animated_scroll).abs() > 0.5 {
                self.timeline_scroll_target = output.state.offset.x;
            }
        });

        if let Some(operation) = pending_operation {
            self.send_operation(operation);
        }
        if scrub_started {
            self.resume_after_scrub = self.playing;
            if self.playing {
                self.media.pause();
            }
        }
        if let Some(position) = seek {
            let maximum = self.document.duration.0.saturating_sub(1).max(0);
            self.position = TimeCode(position.0.clamp(0, maximum));
            self.media.request_frame(self.position);
            if scrub_stopped {
                self.media.seek(self.position);
            }
        }
        if scrub_stopped {
            if self.resume_after_scrub {
                self.media.play(self.position);
            }
            self.resume_after_scrub = false;
        }
    }
}

fn paint_track_labels(
    ui: &mut egui::Ui,
    document: &openreel_core::Document,
    total_height: f32,
    track_height: f32,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(TRACK_LABEL_WIDTH, total_height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, radius::NONE, color::PANEL);
    painter.text(
        egui::pos2(
            rect.left() + space::TWO,
            rect.top() + size::RULER_HEIGHT / 2.0,
        ),
        egui::Align2::LEFT_CENTER,
        "TRACKS",
        egui::FontId::new(type_size::MICRO, egui::FontFamily::Proportional),
        color::TEXT_MUTED,
    );
    for (index, track) in document.tracks.iter().enumerate() {
        let top = rect.top() + size::RULER_HEIGHT + index as f32 * track_height;
        let lane = egui::Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(rect.width(), track_height),
        );
        painter.rect_filled(lane, radius::NONE, color::PANEL);
        painter.line_segment(
            [lane.left_bottom(), lane.right_bottom()],
            egui::Stroke::new(1.0, color::BORDER_SUBTLE),
        );
        let (label, icon) = match track.kind {
            TrackKind::Video => (format!("V{}", index + 1), Icon::Filmstrip),
            TrackKind::Audio => (format!("A{}", index + 1), Icon::Waveform),
        };
        painter.text(
            egui::pos2(lane.left() + space::TWO, lane.center().y - 8.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
            color::TEXT_PRIMARY,
        );
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(lane.left() + space::TWO, lane.center().y + 2.0),
            egui::vec2(size::ICON_SM, size::ICON_SM),
        );
        icon.image(size::ICON_SM)
            .tint(color::TEXT_MUTED)
            .paint_at(ui, icon_rect);
    }
    painter.line_segment(
        [rect.right_top(), rect.right_bottom()],
        egui::Stroke::new(1.0, color::BORDER_SUBTLE),
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_ruler(
    painter: &egui::Painter,
    rect: egui::Rect,
    clip_rect: egui::Rect,
    pixels_per_frame: f32,
    fps: Rational,
    content_frames: i64,
    major_tick: i64,
    minor_tick: i64,
) {
    let ruler = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), size::RULER_HEIGHT));
    painter.rect_filled(ruler, radius::NONE, color::PANEL);
    painter.line_segment(
        [ruler.left_bottom(), ruler.right_bottom()],
        egui::Stroke::new(1.0, color::BORDER_STRONG),
    );
    let visible_start =
        (((clip_rect.left() - rect.left()) / pixels_per_frame).floor() as i64).max(0);
    let visible_end =
        (((clip_rect.right() - rect.left()) / pixels_per_frame).ceil() as i64).min(content_frames);
    let mut frame = visible_start - visible_start.rem_euclid(minor_tick);
    while frame <= visible_end {
        let x = rect.left() + frame as f32 * pixels_per_frame;
        let major = frame.rem_euclid(major_tick) == 0;
        painter.line_segment(
            [
                egui::pos2(x, ruler.bottom()),
                egui::pos2(x, ruler.bottom() - if major { 9.0 } else { 4.0 }),
            ],
            egui::Stroke::new(
                1.0,
                if major {
                    color::TEXT_SECONDARY
                } else {
                    color::BORDER_STRONG
                },
            ),
        );
        if major {
            painter.text(
                egui::pos2(x + space::ONE, ruler.top() + space::ONE),
                egui::Align2::LEFT_TOP,
                format_timecode(TimeCode(frame), fps),
                theme::ruler_font(),
                color::TEXT_SECONDARY,
            );
        }
        frame = frame.saturating_add(minor_tick);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_clip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &openreel_media::FfmpegMediaEngine,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
) {
    painter.rect_filled(rect, radius::SM, color::SURFACE);
    if rect.intersects(clip_bounds)
        && matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo)
    {
        paint_filmstrip(
            painter,
            clip_bounds,
            visual_cache,
            media,
            asset,
            source_range.clone(),
            rect,
        );
    }
    if rect.intersects(clip_bounds)
        && matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo)
    {
        let band_top = if matches!(asset.kind, MediaKind::Audio) {
            rect.top() + 18.0
        } else {
            rect.bottom() - rect.height() * 0.42
        };
        let band = egui::Rect::from_min_max(egui::pos2(rect.left(), band_top), rect.max);
        // A strong scrim keeps waveforms legible over saturated footage.
        painter.rect_filled(band, radius::XS, color::MEDIA_SCRIM_78);
        if let Some(waveform) = visual_cache.waveform(media, asset) {
            paint_waveform(
                painter,
                clip_bounds,
                waveform.as_ref(),
                asset,
                source_range.clone(),
                band.shrink2(egui::vec2(space::HALF, space::ONE)),
                selected,
            );
        }
    }

    let label_strip = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), (rect.top() + 19.0).min(rect.bottom())),
    );
    painter.rect_filled(label_strip, radius::SM, color::MEDIA_SCRIM_78);
    painter.text(
        egui::pos2(rect.left() + space::TWO, rect.top() + space::ONE),
        egui::Align2::LEFT_TOP,
        &asset.name,
        egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
        color::TEXT_PRIMARY,
    );
    if rect.width() >= 140.0 {
        painter.text(
            egui::pos2(rect.right() - space::TWO, rect.top() + space::ONE),
            egui::Align2::RIGHT_TOP,
            format_timecode(
                TimeCode(source_range.end.0.saturating_sub(source_range.start.0)),
                asset.fps,
            ),
            theme::code_font(),
            color::TEXT_SECONDARY,
        );
    }
    if hovered {
        painter.rect_filled(rect, radius::SM, color::ACCENT_16);
    }
    if selected {
        painter.rect_filled(rect, radius::SM, color::ACCENT_28);
    }
    if dragging {
        painter.rect_filled(rect, radius::SM, color::SURFACE_ACTIVE);
    }
    painter.rect_stroke(
        rect,
        radius::SM,
        egui::Stroke::new(
            if dragging { 2.0 } else { 1.0 },
            if dragging {
                color::ACCENT
            } else if selected {
                color::ACCENT_72
            } else if hovered {
                color::BORDER_STRONG
            } else {
                color::BORDER_SUBTLE
            },
        ),
        egui::StrokeKind::Inside,
    );
    if hovered || selected || dragging {
        painter.rect_filled(
            egui::Rect::from_min_max(
                rect.min,
                egui::pos2(rect.left() + EDGE_HANDLE_WIDTH, rect.bottom()),
            ),
            radius::XS,
            color::ACCENT_72,
        );
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.right() - EDGE_HANDLE_WIDTH, rect.top()),
                rect.max,
            ),
            radius::XS,
            color::ACCENT_72,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_filmstrip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &openreel_media::FfmpegMediaEngine,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
) {
    let visible = rect.intersect(clip_bounds);
    if !visible.is_positive() {
        return;
    }
    let tile_count = (rect.width() / FILMSTRIP_TILE_WIDTH).ceil().max(1.0) as usize;
    let first = (((visible.left() - rect.left()) / FILMSTRIP_TILE_WIDTH).floor() as usize)
        .min(tile_count.saturating_sub(1));
    let last =
        (((visible.right() - rect.left()) / FILMSTRIP_TILE_WIDTH).ceil() as usize).min(tile_count);
    let source_span = source_range.end.0.saturating_sub(source_range.start.0);
    for tile in first..last.max(first + 1) {
        let left = rect.left() + tile as f32 * FILMSTRIP_TILE_WIDTH;
        let tile_rect = egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(
                (left + FILMSTRIP_TILE_WIDTH).min(rect.right()),
                rect.bottom(),
            ),
        );
        let ratio = (tile as f64 + 0.5) / tile_count as f64;
        let source_at = TimeCode(
            source_range
                .start
                .0
                .saturating_add((source_span as f64 * ratio).round() as i64)
                .min(source_range.end.0.saturating_sub(1)),
        );
        if let Some(texture) = visual_cache.thumbnail(media, asset, source_at, THUMBNAIL_WIDTH) {
            painter.image(
                texture.id(),
                tile_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                color::MEDIA_TINT_78,
            );
        }
    }
}

fn paint_waveform(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    waveform: &WaveformData,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
    selected: bool,
) {
    if waveform.peaks.is_empty() || asset.duration.0 <= 0 {
        return;
    }
    let visible = rect.intersect(clip_bounds);
    if !visible.is_positive() {
        return;
    }
    let columns = visible.width().ceil().max(1.0) as usize;
    let center = rect.center().y;
    let amplitude = rect.height() / 2.0;
    let source_span = source_range
        .end
        .0
        .saturating_sub(source_range.start.0)
        .max(1);
    let peak_count = waveform.peaks.len();
    for column in 0..columns {
        let x = visible.left() + column as f32;
        let clip_ratio = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        let source_frame = source_range.start.0 as f64 + source_span as f64 * f64::from(clip_ratio);
        let peak_index =
            ((source_frame / asset.duration.0 as f64) * peak_count as f64).floor() as usize;
        let peak = waveform.peaks[peak_index.min(peak_count.saturating_sub(1))];
        let minimum = f32::from(peak.minimum) / f32::from(i16::MAX);
        let maximum = f32::from(peak.maximum) / f32::from(i16::MAX);
        painter.line_segment(
            [
                egui::pos2(x, center - maximum * amplitude),
                egui::pos2(x, center - minimum * amplitude),
            ],
            egui::Stroke::new(
                1.0,
                if selected {
                    color::ACCENT_72
                } else {
                    color::TEXT_PRIMARY_64
                },
            ),
        );
    }
}

fn collect_clip_bounds(document: &openreel_core::Document) -> Vec<ClipBounds> {
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter_map(|clip| {
            let asset = document.asset(clip.asset)?;
            let duration =
                map_source_range_to_project(clip.source_range.clone(), asset.fps, document.fps)
                    .ok()?;
            Some(ClipBounds {
                id: clip.id,
                start: clip.timeline_start.0,
                end: clip.timeline_start.0.saturating_add(duration.0),
            })
        })
        .collect()
}

fn snap_candidates(bounds: &[ClipBounds], exclude: ClipId, playhead: i64) -> Vec<i64> {
    let mut candidates = vec![playhead];
    candidates.extend(
        bounds
            .iter()
            .filter(|bounds| bounds.id != exclude)
            .flat_map(|bounds| [bounds.start, bounds.end]),
    );
    candidates
}

fn snap_move(
    raw_start: i64,
    duration: i64,
    candidates: &[i64],
    ruler_interval: i64,
    pixels_per_frame: f32,
) -> (i64, Option<i64>) {
    let (start_snap, start_guide) =
        nearest_snap(raw_start, candidates, ruler_interval, pixels_per_frame);
    let raw_end = raw_start.saturating_add(duration);
    let (end_snap, end_guide) = nearest_snap(raw_end, candidates, ruler_interval, pixels_per_frame);
    let start_distance = start_snap.saturating_sub(raw_start).saturating_abs();
    let end_distance = end_snap.saturating_sub(raw_end).saturating_abs();
    if end_guide.is_some() && (start_guide.is_none() || end_distance < start_distance) {
        (end_snap.saturating_sub(duration).max(0), end_guide)
    } else {
        (start_snap.max(0), start_guide)
    }
}

fn nearest_snap(
    raw: i64,
    candidates: &[i64],
    ruler_interval: i64,
    pixels_per_frame: f32,
) -> (i64, Option<i64>) {
    let tolerance = (SNAP_TOLERANCE / pixels_per_frame.max(0.01)).ceil() as i64;
    let ruler = ((raw as f64 / ruler_interval.max(1) as f64).round() as i64)
        .saturating_mul(ruler_interval.max(1));
    let mut best = (ruler, ruler.saturating_sub(raw).saturating_abs());
    for candidate in candidates {
        let distance = candidate.saturating_sub(raw).saturating_abs();
        if distance < best.1 {
            best = (*candidate, distance);
        }
    }
    if best.1 <= tolerance {
        (best.0, Some(best.0))
    } else {
        (raw, None)
    }
}

fn tick_density(pixels_per_frame: f32, fps: Rational) -> (i64, i64) {
    let nominal = i64::from(nominal_fps(fps));
    let candidates = [
        1_i64,
        2,
        5,
        10,
        nominal / 2,
        nominal,
        nominal * 2,
        nominal * 5,
        nominal * 10,
        nominal * 30,
        nominal * 60,
        nominal * 300,
        nominal * 600,
    ];
    let major = candidates
        .into_iter()
        .filter(|candidate| *candidate > 0)
        .find(|candidate| *candidate as f32 * pixels_per_frame >= 72.0)
        .unwrap_or(nominal.saturating_mul(600).max(1));
    let minor = [major / 10, major / 5, major / 2, major]
        .into_iter()
        .filter(|candidate| *candidate > 0)
        .find(|candidate| *candidate as f32 * pixels_per_frame >= 8.0)
        .unwrap_or(major)
        .max(1);
    (major.max(1), minor)
}

fn nominal_fps(fps: Rational) -> u32 {
    fps.numerator().saturating_add(fps.denominator() / 2) / fps.denominator().max(1)
}

pub(crate) fn format_timecode(frame: TimeCode, fps: Rational) -> String {
    let nominal = i64::from(nominal_fps(fps).max(1));
    let frame = frame.0.max(0);
    let frames = frame % nominal;
    let seconds_total = frame / nominal;
    let seconds = seconds_total % 60;
    let minutes_total = seconds_total / 60;
    let minutes = minutes_total % 60;
    let hours = minutes_total / 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}:{frames:02}")
}

fn project_delta_to_source(project_delta: i64, project_fps: Rational, source_fps: Rational) -> i64 {
    let sign = project_delta.signum();
    let magnitude = TimeCode(project_delta.saturating_abs());
    map_frames_with_rounding(magnitude, project_fps, source_fps, FrameRounding::Nearest)
        .map_or(0, |frames| frames.0.saturating_mul(sign))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_density_keeps_labels_and_minor_ticks_readable() {
        let fps = Rational::new(30, 1).unwrap();
        for zoom in [0.25, 1.0, 6.0, 20.0] {
            let (major, minor) = tick_density(zoom, fps);
            assert!(major as f32 * zoom >= 72.0);
            assert!(minor as f32 * zoom >= 8.0);
            assert!(major >= minor);
        }
    }

    #[test]
    fn timecode_uses_hour_minute_second_frame_fields() {
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(format_timecode(TimeCode(0), fps), "00:00:00:00");
        assert_eq!(format_timecode(TimeCode(108_029), fps), "01:00:00:29");
    }

    #[test]
    fn snapping_prefers_closest_clip_edge_and_respects_screen_tolerance() {
        let candidates = [100, 240];
        assert_eq!(nearest_snap(97, &candidates, 30, 2.0), (100, Some(100)));
        assert_eq!(nearest_snap(80, &candidates, 30, 2.0), (80, None));
        assert_eq!(snap_move(191, 50, &candidates, 30, 2.0), (190, Some(240)));
    }
}
