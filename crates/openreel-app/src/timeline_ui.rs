use std::sync::Arc;

use eframe::egui;
use openreel_core::{
    Analysis, Clip, ClipContent, ClipId, Document, FrameRounding, MARKER_COLOR_TOKEN_COUNT, Marker,
    MarkerId, MediaAsset, MediaKind, Operation, Rational, SceneStatus, SilenceStatus, TimeCode,
    Title, TrackId, TrackKind, Transition, WaveformData, map_frames_with_rounding,
    map_source_range_to_project,
};
use openreel_media::timeline_source_at;

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

#[derive(Clone, Copy)]
enum TrimEdge {
    Left,
    Right,
}

impl OpenReelApp {
    pub(crate) fn add_title_at_playhead(&mut self) {
        let at = self.position;
        let duration = TimeCode(i64::from(nominal_fps(self.document.fps)).saturating_mul(3));
        let end = TimeCode(at.0.saturating_add(duration.0));
        let available_track = self
            .document
            .tracks
            .iter()
            .rev()
            .filter(|track| track.kind == TrackKind::Video)
            .find(|track| {
                track.clips.iter().all(|clip| {
                    let clip_end = self.document.clip_duration(clip).map_or(
                        clip.timeline_start,
                        |clip_duration| {
                            TimeCode(clip.timeline_start.0.saturating_add(clip_duration.0))
                        },
                    );
                    clip_end <= at || clip.timeline_start >= end
                })
            })
            .map(|track| track.id);
        let add_title = |track| Operation::AddTitle {
            track,
            at,
            duration,
            title: Title::default(),
        };
        if let Some(track) = available_track {
            self.send_operation(add_title(track));
            return;
        }
        let Some(next_track) = self
            .document
            .tracks
            .iter()
            .map(|track| track.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .map(TrackId)
        else {
            self.record_error("Operations", "Track id space is exhausted");
            return;
        };
        self.send_operations(vec![
            Operation::AddTrack {
                track: openreel_core::Track {
                    id: next_track,
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            },
            add_title(next_track),
        ]);
    }

    pub(crate) fn freeze_frame_at_playhead(&mut self) {
        match freeze_frame_operations(&self.document, self.position) {
            Ok(operations) => self.send_operations(operations),
            Err(error) => self.record_error("Operations", error),
        }
    }

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
        if self.transcript_selection.is_some() {
            self.cut_selected_transcript_words();
            return;
        }
        if let Some(marker) = self.selected_marker {
            self.send_operation(Operation::RemoveMarker { marker });
            return;
        }
        self.delete_selected_clips(self.ripple_mode);
    }

    pub(crate) fn ripple_delete_selected(&mut self) {
        self.delete_selected_clips(true);
    }

    fn delete_selected_clips(&mut self, ripple: bool) {
        let Some(clip) = self.selected_clip else {
            self.record_error("Operations", "Select a clip to delete");
            return;
        };
        self.send_operations(linked_delete_operations(&self.document, clip, ripple));
    }

    pub(crate) fn add_marker_at_playhead(&mut self) {
        let Some(id) = next_marker_id(&self.document) else {
            self.record_error("Operations", "Marker id space is exhausted");
            return;
        };
        let marker = Marker {
            id,
            position: self.position,
            label: format!("Marker {id}"),
            color_token: u8::try_from(id.0.saturating_sub(1) % u64::from(MARKER_COLOR_TOKEN_COUNT))
                .expect("marker color token is bounded by a u8 constant"),
        };
        self.selected_marker = Some(id);
        self.selected_clip = None;
        self.send_operation(Operation::AddMarker { marker });
    }

    pub(crate) fn apply_linked_trim(
        &mut self,
        clip: ClipId,
        new_source: std::ops::Range<TimeCode>,
    ) {
        let Some(original) = self.document.clip(clip) else {
            self.record_error("Operations", format!("Clip {clip} no longer exists"));
            return;
        };
        let edge = if new_source.start == original.source_range.start {
            TrimEdge::Right
        } else {
            TrimEdge::Left
        };
        match linked_trim_operations(&self.document, clip, new_source, edge) {
            Ok(operations) => self.send_operations(operations),
            Err(error) => self.record_error("Operations", error),
        }
    }

    // Timeline interaction intentionally maps exact frames to egui's f32 pixel coordinate space.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation
    )]
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
                if ui
                    .button("T  Title")
                    .on_hover_text("Add a three-second title")
                    .clicked()
                {
                    self.add_title_at_playhead();
                }
                if ui
                    .button("Freeze")
                    .on_hover_text("Freeze the current frame for two seconds")
                    .clicked()
                {
                    self.freeze_frame_at_playhead();
                }
                let ripple = ui
                    .add(
                        egui::Button::new("Ripple")
                            .selected(self.ripple_mode)
                            .min_size(egui::vec2(54.0, 22.0)),
                    )
                    .on_hover_text(
                        "Ripple mode: deletes close space on the edited and sync-locked tracks",
                    );
                if ripple.clicked() {
                    self.ripple_mode = !self.ripple_mode;
                }
                if self.ripple_mode {
                    ui.colored_label(color::ACCENT, "RIPPLE");
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
        let marker_end = document
            .markers
            .iter()
            .map(|marker| marker.position.0.saturating_add(1))
            .max()
            .unwrap_or(0);
        let content_frames = document
            .duration
            .0
            .max(marker_end)
            .max(self.position.0.saturating_add(1))
            .max(i64::from(nominal_fps(document.fps)).saturating_mul(10));
        let viewport_width = (ui.available_width() - TRACK_LABEL_WIDTH - space::TWO).max(100.0);
        let content_width =
            ((content_frames as f32) * self.pixels_per_frame + space::SIX).max(viewport_width);
        let (major_tick, minor_tick) = tick_density(self.pixels_per_frame, document.fps);
        let clip_bounds = collect_clip_bounds(&document);
        let mut pending_operations = None;
        let mut seek = None;
        let mut scrub_started = false;
        let mut scrub_stopped = false;
        let mut snap_guide = None;
        let snapping_disabled = ui.input(|input| input.modifiers.alt);

        ui.horizontal_top(|ui| {
            if let Some(operation) = paint_track_labels(ui, &document, total_height, track_height) {
                pending_operations = Some(vec![operation]);
            }
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
                    let mut marker_pointer_interaction = false;
                    for (track_index, track) in document.tracks.iter().enumerate() {
                        let lane_top =
                            rect.top() + size::RULER_HEIGHT + track_index as f32 * track_height;
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
                            let Ok(duration) = document.clip_duration(clip) else {
                                continue;
                            };
                            let asset = match &clip.content {
                                ClipContent::Media | ClipContent::Freeze(_) => {
                                    document.asset(clip.asset)
                                }
                                ClipContent::Title(_) => None,
                            };
                            if !matches!(&clip.content, ClipContent::Title(_)) && asset.is_none() {
                                continue;
                            }
                            let (source_fps, maximum_source_end) = match &clip.content {
                                ClipContent::Media => asset
                                    .map_or((document.fps, TimeCode(i64::MAX)), |asset| {
                                        (asset.fps, asset.duration)
                                    }),
                                ClipContent::Title(_) | ClipContent::Freeze(_) => {
                                    (document.fps, TimeCode(i64::MAX))
                                }
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
                                self.selected_marker = None;
                                self.selected_asset = asset.map(|asset| asset.id);
                            }
                            if body.double_clicked()
                                && matches!(&clip.content, ClipContent::Title(_))
                            {
                                self.title_text_focus = Some(clip.id);
                            }

                            let interacting = body.dragged()
                                || body.drag_stopped()
                                || left.dragged()
                                || left.drag_stopped()
                                || right.dragged()
                                || right.drag_stopped();
                            let candidates = if interacting {
                                snap_candidates(
                                    &clip_bounds,
                                    &document.markers,
                                    clip.id,
                                    self.position.0,
                                )
                            } else {
                                Vec::new()
                            };
                            let project_delta =
                                (body.drag_delta().x / self.pixels_per_frame).round() as i64;
                            let minimum_start = linked_minimum_primary_start(&document, clip.id);
                            let raw_start = clip
                                .timeline_start
                                .0
                                .saturating_add(project_delta)
                                .max(minimum_start);
                            let (snapped_start, body_guide) = if snapping_disabled {
                                (raw_start, None)
                            } else {
                                snap_move(
                                    raw_start,
                                    duration.0,
                                    &candidates,
                                    minor_tick,
                                    self.pixels_per_frame,
                                )
                            };
                            if body.dragged() {
                                snap_guide = body_guide.or(snap_guide);
                            }
                            if body.drag_stopped() && snapped_start != clip.timeline_start.0 {
                                pending_operations = Some(linked_move_operations(
                                    &document,
                                    clip.id,
                                    track.id,
                                    TimeCode(snapped_start),
                                ));
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
                                let (edge, _) = if snapping_disabled {
                                    (raw_edge, None)
                                } else {
                                    nearest_snap(
                                        raw_edge,
                                        &candidates,
                                        minor_tick,
                                        self.pixels_per_frame,
                                    )
                                };
                                let source_delta = project_delta_to_source(
                                    edge.saturating_sub(clip.timeline_start.0),
                                    document.fps,
                                    source_fps,
                                );
                                let new_start = TimeCode(
                                    clip.source_range
                                        .start
                                        .0
                                        .saturating_add(source_delta)
                                        .clamp(0, clip.source_range.end.0.saturating_sub(1)),
                                );
                                if new_start != clip.source_range.start {
                                    pending_operations = linked_trim_operations(
                                        &document,
                                        clip.id,
                                        new_start..clip.source_range.end,
                                        TrimEdge::Left,
                                    )
                                    .ok();
                                }
                            }
                            if right.drag_stopped() {
                                let clip_end = clip.timeline_start.0.saturating_add(duration.0);
                                let raw_edge = clip_end.saturating_add(
                                    (right.drag_delta().x / self.pixels_per_frame).round() as i64,
                                );
                                let (edge, _) = if snapping_disabled {
                                    (raw_edge, None)
                                } else {
                                    nearest_snap(
                                        raw_edge,
                                        &candidates,
                                        minor_tick,
                                        self.pixels_per_frame,
                                    )
                                };
                                let source_delta = project_delta_to_source(
                                    edge.saturating_sub(clip_end),
                                    document.fps,
                                    source_fps,
                                );
                                let new_end = TimeCode(
                                    clip.source_range.end.0.saturating_add(source_delta).clamp(
                                        clip.source_range.start.0.saturating_add(1),
                                        maximum_source_end.0,
                                    ),
                                );
                                if new_end != clip.source_range.end {
                                    pending_operations = linked_trim_operations(
                                        &document,
                                        clip.id,
                                        clip.source_range.start..new_end,
                                        TrimEdge::Right,
                                    )
                                    .ok();
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
                            match (&clip.content, asset) {
                                (ClipContent::Media, Some(asset)) => paint_clip(
                                    &painter,
                                    ui.clip_rect(),
                                    &mut self.visual_cache,
                                    self.analysis.as_ref(),
                                    asset,
                                    clip.source_range.clone(),
                                    draw_rect,
                                    body.hovered() || left.hovered() || right.hovered(),
                                    selected,
                                    dragging,
                                    clip.transition_in
                                        .as_ref()
                                        .map(|transition| transition.duration),
                                    self.pixels_per_frame,
                                ),
                                (ClipContent::Title(title), _) => paint_title_clip(
                                    &painter,
                                    title,
                                    draw_rect,
                                    body.hovered() || left.hovered() || right.hovered(),
                                    selected,
                                    dragging,
                                    clip.transition_in
                                        .as_ref()
                                        .map(|transition| transition.duration),
                                    self.pixels_per_frame,
                                ),
                                (ClipContent::Freeze(freeze), Some(asset)) => paint_freeze_clip(
                                    &painter,
                                    ui.clip_rect(),
                                    &mut self.visual_cache,
                                    self.analysis.as_ref(),
                                    asset,
                                    freeze.source_frame,
                                    draw_rect,
                                    body.hovered() || left.hovered() || right.hovered(),
                                    selected,
                                    dragging,
                                    clip.transition_in
                                        .as_ref()
                                        .map(|transition| transition.duration),
                                    self.pixels_per_frame,
                                ),
                                (ClipContent::Media | ClipContent::Freeze(_), None) => {}
                            }
                        }
                    }

                    for marker in &document.markers {
                        let x = rect.left() + marker.position.0 as f32 * self.pixels_per_frame;
                        let marker_rect = egui::Rect::from_center_size(
                            egui::pos2(x + 3.0, rect.top() + size::RULER_HEIGHT / 2.0),
                            egui::vec2(14.0, size::RULER_HEIGHT),
                        );
                        let response = ui
                            .interact(
                                marker_rect,
                                ui.make_persistent_id(("timeline-marker", marker.id.0)),
                                egui::Sense::click_and_drag(),
                            )
                            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                            .on_hover_text(&marker.label);
                        marker_pointer_interaction |=
                            response.hovered() || response.dragged() || response.drag_stopped();
                        if response.clicked() || response.drag_started() {
                            self.selected_marker = Some(marker.id);
                            self.selected_clip = None;
                        }
                        if response.secondary_clicked() {
                            self.selected_marker = Some(marker.id);
                            self.selected_clip = None;
                            pending_operations =
                                Some(vec![Operation::RemoveMarker { marker: marker.id }]);
                        }
                        let raw = marker.position.0.saturating_add(
                            (response.drag_delta().x / self.pixels_per_frame).round() as i64,
                        );
                        let candidates = marker_snap_candidates(
                            &clip_bounds,
                            &document.markers,
                            marker.id,
                            self.position.0,
                        );
                        let (snapped, guide) = if snapping_disabled {
                            (raw.max(0), None)
                        } else {
                            nearest_snap(raw.max(0), &candidates, minor_tick, self.pixels_per_frame)
                        };
                        if response.dragged() {
                            snap_guide = guide.or(snap_guide);
                        }
                        if response.drag_stopped() && snapped != marker.position.0 {
                            pending_operations = Some(vec![Operation::MoveMarker {
                                marker: marker.id,
                                to: TimeCode(snapped),
                            }]);
                        }
                        let draw_position = if response.dragged() || response.drag_stopped() {
                            snapped
                        } else {
                            marker.position.0
                        };
                        paint_project_marker(
                            &painter,
                            rect,
                            draw_position,
                            marker.color_token,
                            self.selected_marker == Some(marker.id),
                            response.dragged(),
                            self.pixels_per_frame,
                        );
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
                    if (playhead_response.dragged() || playhead_response.drag_stopped())
                        && let Some(pointer) = playhead_response.interact_pointer_pos()
                    {
                        let raw =
                            ((pointer.x - rect.left()) / self.pixels_per_frame).round() as i64;
                        let candidates = clip_bounds
                            .iter()
                            .flat_map(|bounds| [bounds.start, bounds.end])
                            .chain(document.markers.iter().map(|marker| marker.position.0))
                            .collect::<Vec<_>>();
                        let (snapped, guide) = if snapping_disabled {
                            (raw.max(0), None)
                        } else {
                            nearest_snap(raw.max(0), &candidates, minor_tick, self.pixels_per_frame)
                        };
                        seek = Some(TimeCode(snapped));
                        snap_guide = guide.or(snap_guide);
                    }
                    if playhead_response.drag_stopped() {
                        scrub_stopped = true;
                    }
                    if canvas_response.clicked()
                        && !clip_pointer_interaction
                        && !marker_pointer_interaction
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

        if let Some(operations) = pending_operations {
            self.send_operations(operations);
        }
        if scrub_started {
            self.resume_after_scrub = self.playing;
            if self.playing {
                self.playback.pause();
            }
        }
        if let Some(position) = seek {
            let maximum = self.document.duration.0.saturating_sub(1).max(0);
            self.position = TimeCode(position.0.clamp(0, maximum));
            self.playback.request_frame(self.position);
            if scrub_stopped {
                self.playback.seek(self.position);
            }
        }
        if scrub_stopped {
            if self.resume_after_scrub {
                self.playback.play(self.position);
            }
            self.resume_after_scrub = false;
        }
    }
}

// Track indices are small and intentionally projected into egui's f32 coordinate space.
#[allow(clippy::cast_precision_loss)]
fn paint_track_labels(
    ui: &mut egui::Ui,
    document: &openreel_core::Document,
    total_height: f32,
    track_height: f32,
) -> Option<Operation> {
    let mut pending_operation = None;
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

        if let Some(operation) = paint_sync_lock_toggle(ui, &painter, lane, track) {
            pending_operation = Some(operation);
        }
    }
    painter.line_segment(
        [rect.right_top(), rect.right_bottom()],
        egui::Stroke::new(1.0, color::BORDER_SUBTLE),
    );
    pending_operation
}

fn paint_sync_lock_toggle(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    lane: egui::Rect,
    track: &openreel_core::Track,
) -> Option<Operation> {
    let toggle_rect = egui::Rect::from_center_size(
        egui::pos2(
            lane.right() - space::TWO - size::ICON_BUTTON / 2.0,
            lane.center().y,
        ),
        egui::vec2(size::ICON_BUTTON, size::TIMELINE_TOOLBAR_HEIGHT),
    );
    let tooltip = if track.sync_lock {
        "Sync lock on: ripple edits on other tracks shift this track to preserve sync"
    } else {
        "Sync lock off: this track runs free and stays put during ripple edits on other tracks"
    };
    let response = ui
        .interact(
            toggle_rect,
            ui.make_persistent_id(("track-sync-lock", track.id.0)),
            egui::Sense::click(),
        )
        .on_hover_text(tooltip);
    let lock_icon = if track.sync_lock {
        Icon::Lock
    } else {
        Icon::Unlock
    };
    let lock_color = if track.sync_lock {
        color::TEXT_MUTED
    } else {
        color::STATUS_WARNING
    };
    if response.hovered() || !track.sync_lock {
        painter.rect_filled(toggle_rect, radius::SM, color::SURFACE_RAISED);
    }
    let lock_offset = if track.sync_lock {
        0.0
    } else {
        space::ONE_HALF
    };
    let lock_rect = egui::Rect::from_center_size(
        egui::pos2(toggle_rect.center().x, toggle_rect.center().y + lock_offset),
        egui::vec2(size::ICON_SM, size::ICON_SM),
    );
    lock_icon
        .image(size::ICON_SM)
        .tint(lock_color)
        .paint_at(ui, lock_rect);
    if !track.sync_lock {
        painter.text(
            egui::pos2(toggle_rect.center().x, toggle_rect.top() + space::ONE_HALF),
            egui::Align2::CENTER_CENTER,
            "FREE",
            egui::FontId::new(type_size::MICRO, egui::FontFamily::Proportional),
            color::STATUS_WARNING,
        );
    }
    response.clicked().then_some(Operation::SetTrackSyncLock {
        track: track.id,
        locked: !track.sync_lock,
    })
}

// Ruler bounds intentionally convert between exact frames and f32 viewport pixels.
#[allow(
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]
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

// Marker frame positions intentionally project into the ruler's f32 pixel space.
#[allow(clippy::cast_precision_loss)]
fn paint_project_marker(
    painter: &egui::Painter,
    timeline_rect: egui::Rect,
    position: i64,
    color_token: u8,
    selected: bool,
    dragging: bool,
    pixels_per_frame: f32,
) {
    let x = timeline_rect.left() + position as f32 * pixels_per_frame;
    // Markers exist to draw the eye to moments; their tokens are the chromatic
    // status palette, never the greyscale text ramp (which camouflages against
    // the ruler).
    let token_color = match color_token {
        1 => color::STATUS_SUCCESS,
        2 => color::STATUS_WARNING,
        3 => color::STATUS_DANGER,
        _ => color::ACCENT,
    };
    let marker_color = if selected { color::ACCENT } else { token_color };
    let top = timeline_rect.top() + space::ONE;
    painter.line_segment(
        [
            egui::pos2(x, top),
            egui::pos2(x, timeline_rect.top() + size::RULER_HEIGHT - space::HALF),
        ],
        egui::Stroke::new(if selected || dragging { 2.0 } else { 1.5 }, marker_color),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x, top),
            egui::pos2(x + 10.0, top + 4.0),
            egui::pos2(x, top + 8.0),
        ],
        if dragging {
            color::SURFACE_ACTIVE
        } else {
            marker_color
        },
        egui::Stroke::new(if selected { 1.0 } else { 0.0 }, color::ACCENT),
    ));
}

// One clip paint pass keeps its layered drawing order explicit and reviewable.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn paint_clip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &dyn Analysis,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
    transition_duration: Option<TimeCode>,
    pixels_per_frame: f32,
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
    paint_transition_affordance(painter, rect, transition_duration, pixels_per_frame);

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
    paint_derived_markers(
        painter,
        clip_bounds,
        media,
        asset,
        source_range.clone(),
        rect,
    );
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

#[allow(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn paint_freeze_clip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &dyn Analysis,
    asset: &MediaAsset,
    source_frame: TimeCode,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
    transition_duration: Option<TimeCode>,
    pixels_per_frame: f32,
) {
    painter.rect_filled(rect, radius::SM, color::SURFACE);
    let visible = rect.intersect(clip_bounds);
    if visible.is_positive()
        && let Some(texture) = visual_cache.thumbnail(media, asset, source_frame, THUMBNAIL_WIDTH)
    {
        let first = ((visible.left() - rect.left()) / FILMSTRIP_TILE_WIDTH)
            .floor()
            .max(0.0) as usize;
        let last = ((visible.right() - rect.left()) / FILMSTRIP_TILE_WIDTH)
            .ceil()
            .max(1.0) as usize;
        for tile in first..last {
            let left = rect.left() + tile as f32 * FILMSTRIP_TILE_WIDTH;
            let tile_rect = egui::Rect::from_min_max(
                egui::pos2(left, rect.top()),
                egui::pos2(
                    (left + FILMSTRIP_TILE_WIDTH).min(rect.right()),
                    rect.bottom(),
                ),
            );
            painter.image(
                texture.id(),
                tile_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                color::MEDIA_TINT_78,
            );
        }
    }
    paint_transition_affordance(painter, rect, transition_duration, pixels_per_frame);
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
    painter.text(
        egui::pos2(rect.right() - space::TWO, rect.top() + space::ONE),
        egui::Align2::RIGHT_TOP,
        "HOLD",
        theme::code_font(),
        color::ACCENT,
    );
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
}

#[allow(clippy::too_many_arguments)]
fn paint_title_clip(
    painter: &egui::Painter,
    title: &Title,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    dragging: bool,
    transition_duration: Option<TimeCode>,
    pixels_per_frame: f32,
) {
    // Accent discipline: an unselected title is a neutral raised surface with
    // a small accent badge - the accent field/border is earned by selection,
    // never outranking the playhead at rest.
    let fill = if dragging {
        color::SURFACE_ACTIVE
    } else if selected {
        color::ACCENT_28
    } else {
        color::SURFACE_RAISED
    };
    painter.rect_filled(rect, radius::SM, fill);
    paint_transition_affordance(painter, rect, transition_duration, pixels_per_frame);
    painter.rect_filled(
        egui::Rect::from_min_max(
            rect.min,
            egui::pos2((rect.left() + 28.0).min(rect.right()), rect.bottom()),
        ),
        radius::SM,
        if selected || dragging {
            color::ACCENT_28
        } else {
            color::ACCENT_16
        },
    );
    painter.text(
        egui::pos2(rect.left() + space::TWO, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "T",
        egui::FontId::new(type_size::HEADING, egui::FontFamily::Proportional),
        color::ACCENT,
    );
    let label = title.text.lines().next().unwrap_or("Title");
    painter.text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(type_size::CAPTION, egui::FontFamily::Proportional),
        color::TEXT_PRIMARY,
    );
    if hovered {
        painter.rect_filled(rect, radius::SM, color::ACCENT_10);
    }
    painter.rect_stroke(
        rect,
        radius::SM,
        egui::Stroke::new(
            if dragging { 2.0 } else { 1.0 },
            if selected || dragging {
                color::ACCENT
            } else {
                color::BORDER_STRONG
            },
        ),
        egui::StrokeKind::Inside,
    );
}

// Transition frame widths are intentionally projected into f32 timeline pixels.
#[allow(clippy::cast_precision_loss)]
fn paint_transition_affordance(
    painter: &egui::Painter,
    rect: egui::Rect,
    duration: Option<TimeCode>,
    pixels_per_frame: f32,
) {
    let Some(duration) = duration.filter(|duration| duration.0 > 0) else {
        return;
    };
    let left = rect.left() + EDGE_HANDLE_WIDTH;
    let width = duration.0 as f32 * pixels_per_frame;
    let right = (left + width).min(rect.right() - EDGE_HANDLE_WIDTH);
    if right <= left || rect.height() <= space::ONE * 2.0 {
        return;
    }
    let top = rect.top() + space::ONE;
    let bottom = rect.bottom() - space::ONE;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(left, top),
            egui::pos2(left, bottom),
            egui::pos2(right, bottom),
        ],
        color::MEDIA_SCRIM_78,
        egui::Stroke::new(1.0, color::ACCENT_16),
    ));
}

// Derived source-frame positions are intentionally projected into f32 clip pixels.
#[allow(clippy::cast_precision_loss)]
fn paint_derived_markers(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    media: &dyn Analysis,
    asset: &MediaAsset,
    source_range: std::ops::Range<TimeCode>,
    rect: egui::Rect,
) {
    let visible = rect.intersect(clip_bounds);
    if !visible.is_positive() {
        return;
    }
    let source_span = source_range
        .end
        .0
        .saturating_sub(source_range.start.0)
        .max(1);
    let source_x = |frame: TimeCode| {
        rect.left()
            + frame.0.saturating_sub(source_range.start.0) as f32 / source_span as f32
                * rect.width()
    };

    if let SilenceStatus::Ready(silences) = media.silence_status(asset.id) {
        for span in &silences.spans {
            let start = span.source_start.max(source_range.start);
            let end = span.source_end.min(source_range.end);
            if end.0.saturating_sub(start.0) < 6 {
                continue;
            }
            let underline = egui::Rect::from_min_max(
                egui::pos2(source_x(start).max(visible.left()), rect.bottom() - 3.0),
                egui::pos2(source_x(end).min(visible.right()), rect.bottom() - 1.0),
            );
            if underline.is_positive() {
                painter.rect_filled(underline, radius::NONE, color::TEXT_MUTED);
            }
        }
    }

    if let SceneStatus::Ready(scenes) = media.scene_status(asset.id) {
        for change in &scenes.changes {
            if change.confidence_basis_points < 1_000
                || change.source_frame < source_range.start
                || change.source_frame >= source_range.end
            {
                continue;
            }
            let x = source_x(change.source_frame);
            if x < visible.left() || x > visible.right() {
                continue;
            }
            painter.line_segment(
                [
                    egui::pos2(x, rect.top() + 19.0),
                    egui::pos2(x, rect.bottom() - 3.0),
                ],
                egui::Stroke::new(1.0, color::TEXT_SECONDARY),
            );
        }
    }
}

// Filmstrip sampling intentionally converts between bounded pixel columns and source frames.
#[allow(
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn paint_filmstrip(
    painter: &egui::Painter,
    clip_bounds: egui::Rect,
    visual_cache: &mut VisualCache,
    media: &dyn Analysis,
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

// Waveform rasterization intentionally converts bounded pixels and peak indices across domains.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
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

pub(crate) fn linked_members(document: &Document, primary: ClipId) -> Vec<(TrackId, Clip)> {
    let Some(primary_clip) = document.clip(primary) else {
        return Vec::new();
    };
    let Some(link) = primary_clip.link else {
        return document
            .tracks
            .iter()
            .find_map(|track| {
                track
                    .clips
                    .iter()
                    .find(|clip| clip.id == primary)
                    .cloned()
                    .map(|clip| vec![(track.id, clip)])
            })
            .unwrap_or_default();
    };
    document
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .clips
                .iter()
                .filter(move |clip| clip.link == Some(link))
                .cloned()
                .map(move |clip| (track.id, clip))
        })
        .collect()
}

fn linked_minimum_primary_start(document: &Document, primary: ClipId) -> i64 {
    let Some(primary_start) = document.clip(primary).map(|clip| clip.timeline_start.0) else {
        return 0;
    };
    let minimum_member = linked_members(document, primary)
        .iter()
        .map(|(_, clip)| clip.timeline_start.0)
        .min()
        .unwrap_or(primary_start);
    primary_start.saturating_sub(minimum_member)
}

fn linked_move_operations(
    document: &Document,
    primary: ClipId,
    primary_track: TrackId,
    to: TimeCode,
) -> Vec<Operation> {
    let Some(primary_start) = document.clip(primary).map(|clip| clip.timeline_start.0) else {
        return Vec::new();
    };
    let delta = to.0.saturating_sub(primary_start);
    let mut members = linked_members(document, primary);
    members.sort_by(|(left_track, left), (right_track, right)| {
        let track_order = left_track.cmp(right_track);
        if track_order != std::cmp::Ordering::Equal {
            return track_order;
        }
        if delta > 0 {
            right.timeline_start.cmp(&left.timeline_start)
        } else {
            left.timeline_start.cmp(&right.timeline_start)
        }
    });
    members
        .into_iter()
        .filter_map(|(track, clip)| {
            let target = TimeCode(clip.timeline_start.0.saturating_add(delta));
            (target != clip.timeline_start || (clip.id == primary && track != primary_track))
                .then_some(Operation::MoveClip {
                    clip: clip.id,
                    to_track: if clip.id == primary {
                        primary_track
                    } else {
                        track
                    },
                    to: target,
                })
        })
        .collect()
}

fn linked_trim_operations(
    document: &Document,
    primary: ClipId,
    new_source: std::ops::Range<TimeCode>,
    edge: TrimEdge,
) -> Result<Vec<Operation>, String> {
    let primary_clip = document
        .clip(primary)
        .ok_or_else(|| format!("Clip {primary} no longer exists"))?;
    let primary_fps = match &primary_clip.content {
        ClipContent::Media => {
            document
                .asset(primary_clip.asset)
                .ok_or_else(|| format!("Asset {} no longer exists", primary_clip.asset))?
                .fps
        }
        ClipContent::Title(_) | ClipContent::Freeze(_) => document.fps,
    };
    let (old_boundary, new_boundary) = match edge {
        TrimEdge::Left => (primary_clip.source_range.start, new_source.start),
        TrimEdge::Right => (primary_clip.source_range.end, new_source.end),
    };
    let project_delta =
        source_boundary_project_delta(old_boundary, new_boundary, primary_fps, document.fps)?;
    let mut operations = Vec::new();
    for (_, clip) in linked_members(document, primary) {
        let (source_fps, maximum_end) = match &clip.content {
            ClipContent::Media => {
                let asset = document
                    .asset(clip.asset)
                    .ok_or_else(|| format!("Asset {} no longer exists", clip.asset))?;
                (asset.fps, asset.duration.0)
            }
            ClipContent::Title(_) | ClipContent::Freeze(_) => (document.fps, i64::MAX),
        };
        let linked_source = if clip.id == primary {
            new_source.clone()
        } else {
            let source_delta = project_delta_to_source(project_delta, document.fps, source_fps);
            match edge {
                TrimEdge::Left => {
                    let start = TimeCode(
                        clip.source_range
                            .start
                            .0
                            .saturating_add(source_delta)
                            .clamp(0, clip.source_range.end.0.saturating_sub(1)),
                    );
                    start..clip.source_range.end
                }
                TrimEdge::Right => {
                    let end = TimeCode(
                        clip.source_range
                            .end
                            .0
                            .saturating_add(source_delta)
                            .clamp(clip.source_range.start.0.saturating_add(1), maximum_end),
                    );
                    clip.source_range.start..end
                }
            }
        };
        if linked_source != clip.source_range {
            operations.push(Operation::TrimClip {
                clip: clip.id,
                new_source: linked_source,
            });
        }
    }
    Ok(operations)
}

fn freeze_frame_operations(document: &Document, at: TimeCode) -> Result<Vec<Operation>, String> {
    let source = timeline_source_at(document, at)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "A media clip is required under the playhead".to_owned())?;
    let clip = document
        .clip(source.clip)
        .ok_or_else(|| format!("Clip {} no longer exists", source.clip))?;
    let duration = TimeCode(i64::from(nominal_fps(document.fps)).saturating_mul(2));
    let mut operations = Vec::with_capacity(3);
    if at > clip.timeline_start && at < source.timeline_end {
        operations.push(Operation::SplitClip {
            clip: source.clip,
            at,
        });
    }
    operations.extend([
        Operation::RippleInsertGap {
            track: source.track,
            at,
            duration,
        },
        Operation::AddFreezeFrame {
            track: source.track,
            at,
            duration,
            asset: source.asset,
            source_frame: source.source_at,
        },
    ]);
    Ok(operations)
}

fn source_boundary_project_delta(
    old: TimeCode,
    new: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
) -> Result<i64, String> {
    match new.cmp(&old) {
        std::cmp::Ordering::Greater => {
            map_source_range_to_project(old..new, source_fps, project_fps)
                .map(|delta| delta.0)
                .map_err(|error| error.to_string())
        }
        std::cmp::Ordering::Less => map_source_range_to_project(new..old, source_fps, project_fps)
            .map(|delta| delta.0.saturating_neg())
            .map_err(|error| error.to_string()),
        std::cmp::Ordering::Equal => Ok(0),
    }
}

fn linked_delete_operations(document: &Document, primary: ClipId, ripple: bool) -> Vec<Operation> {
    let mut members = linked_members(document, primary);
    if members.is_empty() {
        return Vec::new();
    }
    members.sort_by(|(left_track, left), (right_track, right)| {
        left_track
            .cmp(right_track)
            .then_with(|| right.timeline_start.cmp(&left.timeline_start))
    });
    if !ripple {
        return members
            .into_iter()
            .map(|(_, clip)| Operation::DeleteClip { clip: clip.id })
            .collect();
    }

    let mut operations = members
        .into_iter()
        .filter(|(_, clip)| clip.id != primary)
        .map(|(_, clip)| Operation::DeleteClip { clip: clip.id })
        .collect::<Vec<_>>();
    operations.push(Operation::RippleDeleteClip { clip: primary });
    operations
}

pub(super) fn linked_transition_operations(
    document: &Document,
    primary: ClipId,
    transition: Option<&Transition>,
) -> Vec<Operation> {
    let mut operations = Vec::new();
    for (_, clip) in linked_members(document, primary) {
        if clip.transition_in.is_some() {
            operations.push(Operation::RemoveTransition { clip: clip.id });
        }
        if let Some(transition) = transition {
            operations.push(Operation::AddTransition {
                clip: clip.id,
                transition: transition.clone(),
            });
        }
    }
    operations
}

fn next_marker_id(document: &Document) -> Option<MarkerId> {
    document
        .markers
        .iter()
        .map(|marker| marker.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(MarkerId)
}

fn collect_clip_bounds(document: &openreel_core::Document) -> Vec<ClipBounds> {
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter_map(|clip| {
            let duration = document.clip_duration(clip).ok()?;
            Some(ClipBounds {
                id: clip.id,
                start: clip.timeline_start.0,
                end: clip.timeline_start.0.saturating_add(duration.0),
            })
        })
        .collect()
}

fn snap_candidates(
    bounds: &[ClipBounds],
    markers: &[Marker],
    exclude: ClipId,
    playhead: i64,
) -> Vec<i64> {
    let mut candidates = vec![playhead];
    candidates.extend(markers.iter().map(|marker| marker.position.0));
    candidates.extend(
        bounds
            .iter()
            .filter(|bounds| bounds.id != exclude)
            .flat_map(|bounds| [bounds.start, bounds.end]),
    );
    candidates
}

fn marker_snap_candidates(
    bounds: &[ClipBounds],
    markers: &[Marker],
    exclude: MarkerId,
    playhead: i64,
) -> Vec<i64> {
    let mut candidates = vec![playhead];
    candidates.extend(bounds.iter().flat_map(|bounds| [bounds.start, bounds.end]));
    candidates.extend(
        markers
            .iter()
            .filter(|marker| marker.id != exclude)
            .map(|marker| marker.position.0),
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

// Snapping rounds between f32 pixel tolerance, f64 ratios, and exact integer frames.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
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

// Tick selection compares exact frame intervals in egui's f32 pixel coordinate space.
#[allow(clippy::cast_precision_loss)]
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
    use std::path::PathBuf;

    use openreel_core::{AssetId, LinkId, MediaAsset, Track};

    use super::*;

    fn linked_fixture() -> Document {
        let fps = Rational::new(30, 1).unwrap();
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("linked.mp4"),
            name: "linked.mp4".to_owned(),
            duration: TimeCode(120),
            fps,
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
        };
        let clip = |id, track_start| Clip {
            id: ClipId(id),
            asset: asset.id,
            source_range: TimeCode(0)..TimeCode(30),
            content: openreel_core::ClipContent::Media,
            timeline_start: TimeCode(track_start),
            effects: Vec::new(),
            transition_in: None,
            link: Some(LinkId(7)),
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
        };
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![clip(1, 0)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![clip(2, 0)],
                },
            ],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(30),
        }
    }

    #[test]
    // This test checks the same intentional frame-to-pixel projection as tick_density.
    #[allow(clippy::cast_precision_loss)]
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
    fn freeze_at_playhead_builds_one_split_gap_add_batch_and_one_undo_reverts_it() {
        use openreel_core::{Command, Core, Event};

        let mut document = linked_fixture();
        document.tracks.truncate(1);
        let operations = freeze_frame_operations(&document, TimeCode(10)).unwrap();
        assert_eq!(
            operations,
            vec![
                Operation::SplitClip {
                    clip: ClipId(1),
                    at: TimeCode(10),
                },
                Operation::RippleInsertGap {
                    track: TrackId(1),
                    at: TimeCode(10),
                    duration: TimeCode(60),
                },
                Operation::AddFreezeFrame {
                    track: TrackId(1),
                    at: TimeCode(10),
                    duration: TimeCode(60),
                    asset: AssetId(1),
                    source_frame: TimeCode(10),
                },
            ]
        );

        let core = Core::spawn(document.clone()).unwrap();
        core.request(Command::DoBatch(operations)).unwrap();
        let undone = match core.request(Command::Undo).unwrap() {
            Event::DocumentChanged { doc, .. } => doc,
            other => panic!("unexpected event: {other:?}"),
        };
        assert_eq!(undone.as_ref(), &document);
    }

    #[test]
    fn freeze_at_clip_start_skips_split_and_requires_media_under_playhead() {
        let document = linked_fixture();
        assert_eq!(
            freeze_frame_operations(&document, TimeCode::ZERO).unwrap(),
            vec![
                Operation::RippleInsertGap {
                    track: TrackId(1),
                    at: TimeCode::ZERO,
                    duration: TimeCode(60),
                },
                Operation::AddFreezeFrame {
                    track: TrackId(1),
                    at: TimeCode::ZERO,
                    duration: TimeCode(60),
                    asset: AssetId(1),
                    source_frame: TimeCode::ZERO,
                },
            ]
        );
        assert_eq!(
            freeze_frame_operations(&document, TimeCode(30)).unwrap_err(),
            "A media clip is required under the playhead"
        );
    }

    #[test]
    fn snapping_prefers_closest_clip_edge_and_respects_screen_tolerance() {
        let candidates = [100, 240];
        assert_eq!(nearest_snap(97, &candidates, 30, 2.0), (100, Some(100)));
        assert_eq!(nearest_snap(80, &candidates, 30, 2.0), (80, None));
        assert_eq!(snap_move(191, 50, &candidates, 30, 2.0), (190, Some(240)));
    }

    #[test]
    fn snapping_candidates_include_markers_and_other_tracks() {
        let bounds = [
            ClipBounds {
                id: ClipId(1),
                start: 10,
                end: 40,
            },
            ClipBounds {
                id: ClipId(2),
                start: 60,
                end: 90,
            },
        ];
        let markers = [Marker {
            id: MarkerId(3),
            position: TimeCode(50),
            label: "Review".to_owned(),
            color_token: 0,
        }];
        assert_eq!(
            snap_candidates(&bounds, &markers, ClipId(1), 25),
            vec![25, 50, 60, 90]
        );
    }

    #[test]
    fn linked_move_trim_and_delete_expand_to_atomic_batch_members() {
        let document = linked_fixture();
        assert_eq!(
            linked_move_operations(&document, ClipId(1), TrackId(1), TimeCode(10)),
            vec![
                Operation::MoveClip {
                    clip: ClipId(1),
                    to_track: TrackId(1),
                    to: TimeCode(10),
                },
                Operation::MoveClip {
                    clip: ClipId(2),
                    to_track: TrackId(2),
                    to: TimeCode(10),
                },
            ]
        );
        assert_eq!(
            linked_trim_operations(
                &document,
                ClipId(1),
                TimeCode(5)..TimeCode(30),
                TrimEdge::Left,
            )
            .unwrap(),
            vec![
                Operation::TrimClip {
                    clip: ClipId(1),
                    new_source: TimeCode(5)..TimeCode(30),
                },
                Operation::TrimClip {
                    clip: ClipId(2),
                    new_source: TimeCode(5)..TimeCode(30),
                },
            ]
        );
        assert_eq!(
            linked_delete_operations(&document, ClipId(1), true),
            vec![
                Operation::DeleteClip { clip: ClipId(2) },
                Operation::RippleDeleteClip { clip: ClipId(1) },
            ]
        );
    }

    #[test]
    fn linked_transition_operations_replace_each_member_in_one_batch() {
        let mut document = linked_fixture();
        document.tracks[0].clips[0].transition_in = Some(Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(6),
        });
        let replacement = Transition {
            name: "fade_from_white".to_owned(),
            duration: TimeCode(12),
        };

        assert_eq!(
            linked_transition_operations(&document, ClipId(1), Some(&replacement)),
            vec![
                Operation::RemoveTransition { clip: ClipId(1) },
                Operation::AddTransition {
                    clip: ClipId(1),
                    transition: replacement.clone(),
                },
                Operation::AddTransition {
                    clip: ClipId(2),
                    transition: replacement,
                },
            ]
        );
        assert_eq!(
            linked_transition_operations(&document, ClipId(1), None),
            vec![Operation::RemoveTransition { clip: ClipId(1) }]
        );
    }
}
