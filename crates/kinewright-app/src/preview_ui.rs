use std::sync::Arc;

use eframe::egui;
use kinewright_core::{
    Clip, EffectUniform, MatteParams, MediaKind, ThreePointMode, TimeCode, TrackId, TrackKind,
};

use crate::{
    app::KinewrightApp,
    icons::Icon,
    inspector_ui::{InspectorEdits, matte_gesture_coalesce_key, matte_window_drag_operations},
    matte_overlay_ui::{
        AnalysisMatteProofSource, LayerTransform, MatteDrag, MatteFrame, MatteTarget, MatteViewKey,
        MatteViewStatus, coverage_color_image, matte_hit_test, paint_matte_overlay,
    },
    media_workflow::{
        paint_source_status, should_display_source_texture, source_access_is_allowed,
        source_display_state, source_edit_controls_are_enabled,
    },
    theme::{self, color, radius, space, type_size},
};

/// What one painted viewer frame occupies on screen.
///
/// `image_rect` is the letterboxed rectangle the picture was drawn into, and
/// is `None` when no texture was available. The matte overlay draws and
/// hit-tests through exactly that rectangle (CC5 §6), so it has to leave the
/// painter rather than stay a local.
struct ViewerFrame {
    response: egui::Response,
    image_rect: Option<egui::Rect>,
}

/// The Program viewer's pointer contract (CC5 §6).
///
/// The viewer takes click-and-drag input **only** while a matte section is
/// expanded for the selected clip; otherwise it keeps today's hover-only
/// behaviour exactly. Pure, so the decision is provable without a window —
/// giving the viewer input is the first interactive change to the preview
/// (CC5 §12) and the narrowness of that change is the mitigation.
#[must_use]
pub(crate) fn viewer_sense(matte_expanded: bool) -> egui::Sense {
    if matte_expanded {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    }
}

/// Everything the overlay needs about the node whose matte section is open.
struct MatteOverlayContext {
    target: MatteTarget,
    /// The keyframe-evaluated matte at the playhead: the overlay draws what the
    /// renderer renders, not what the card's sliders show.
    matte: MatteParams,
    /// The output raster aspect `a = W / H` (CC5 §2.3), supplied by the
    /// document rather than sniffed from the texture.
    aspect: f64,
    /// The clip's own `transform`, evaluated at the same clip-local frame as
    /// the matte. The shader evaluates the matte at the *layer* quad's uv while
    /// `image_rect` is the *composited* output, so without this a reframed clip
    /// draws its outline, its handles and its drag results displaced by the
    /// whole transform (CC5 §5.2).
    transform: LayerTransform,
    key: MatteViewKey,
}

impl MatteOverlayContext {
    /// Where this node's matte lands inside a painted viewer rectangle.
    fn frame(&self, image_rect: egui::Rect) -> MatteFrame {
        MatteFrame::new(self.aspect, image_rect, self.transform)
    }
}

/// One layer's resolved `transform`, at one clip-local frame.
///
/// Read through the descriptor's `EffectUniform`, not by effect name, so a
/// future effect that drives the same uniform is picked up here exactly as the
/// compositor picks it up (`compositor.rs`'s `LayerParams` accumulation); and
/// keyframe-evaluated at `local_at`, so an animated reframe moves the overlay
/// with the picture instead of pinning it to the static value.
fn resolved_layer_transform(clip: &Clip, local_at: TimeCode) -> LayerTransform {
    let mut transform = LayerTransform::IDENTITY;
    for effect in &clip.effects {
        let Some(descriptor) = kinewright_core::effect_descriptor(&effect.name) else {
            continue;
        };
        for parameter in descriptor.parameters {
            let value = effect
                .integer_parameter_at(parameter.name, local_at)
                .unwrap_or(parameter.neutral);
            #[allow(clippy::cast_precision_loss)]
            let value = value as f64;
            match parameter.uniform {
                EffectUniform::Scale => transform.scale *= value / 100.0,
                EffectUniform::OffsetX => transform.offset_x += value / 50.0,
                EffectUniform::OffsetY => transform.offset_y += value / 50.0,
                _ => {}
            }
        }
    }
    transform
}

/// Which texture the Program viewer shows.
///
/// `blocked` is consulted **first**: a source whose verification blocks the
/// preview must not leak through the Matte view, which renders the same media
/// through the same decoder. Only then does a coverage render, when one is
/// ready, stand in for the picture.
fn viewer_picture<'a, T>(
    blocked: bool,
    matte: Option<&'a T>,
    texture: Option<&'a T>,
) -> Option<&'a T> {
    if blocked {
        return None;
    }
    matte.or(texture)
}

/// The overlay context for one expanded matte section, as a pure function of
/// the document and the playhead (CC5 §6).
///
/// `None` — and therefore no overlay and no pointer capture — whenever the
/// report names a clip that is not selected, a clip or effect that is gone, or
/// a playhead that is **not over the clip**: outside `[timeline_start,
/// timeline_start + duration)` the renderer is showing some other clip's
/// picture, so an overlay there would draw one node's windows on top of another
/// node's frame and a drag would edit a matte the user cannot see. A matte with
/// no window still yields a context: the qualifier, the mix, and the Matte view
/// are all editable without one.
fn matte_overlay_context_for(
    document: &kinewright_core::Document,
    selected_clip: Option<kinewright_core::ClipId>,
    position: TimeCode,
    target: MatteTarget,
    session_id: u64,
    revision: u64,
) -> Option<MatteOverlayContext> {
    if selected_clip != Some(target.clip) {
        return None;
    }
    let clip = document.clip(target.clip)?;
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.id == target.effect)?;
    // Effect keyframes are clip-local (CC3 §3), so the overlay evaluates at the
    // playhead's local frame and draws the geometry the renderer used.
    //
    // `TimeCode` is a signed frame count, so `checked_sub` only fails on
    // overflow — a playhead *before* the clip yields a negative local frame
    // rather than `None`, which is why the range is tested explicitly. The old
    // `unwrap_or(TimeCode::ZERO)` fallback therefore never fired at all: it
    // evaluated a negative frame instead.
    let local_at = position.checked_sub(clip.timeline_start)?;
    let duration = document.clip_duration(clip).ok()?;
    if local_at < TimeCode::ZERO || local_at >= duration {
        return None;
    }
    let matte = MatteParams::from_effect(&effect.evaluated_at(local_at));
    let (width, height) = document.resolution;
    Some(MatteOverlayContext {
        target,
        matte,
        aspect: f64::from(width.max(1)) / f64::from(height.max(1)),
        transform: resolved_layer_transform(clip, local_at),
        key: MatteViewKey {
            session_id,
            revision,
            frame: position,
            target,
        },
    })
}

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
            egui::Sense::hover(),
        );
        self.source_controls(ui, &asset, source_state, revalidation_pending);
    }

    fn program_viewer(&mut self, ui: &mut egui::Ui, frame_height: f32) {
        let available_width = ui.available_width();
        let playhead_state = self.playhead_media_state();
        let blocked = playhead_state
            .as_ref()
            .is_some_and(|(state, _)| state.blocks_preview());
        self.matte_overlay.poll();
        let overlay = self.matte_overlay_context();
        // `blocked` reaches the *request*, not just the picture: the coverage
        // worker decodes the same media through its own renderer, so asking for
        // one while the source is blocked would decode what the block exists to
        // withhold (CC5 §4.1).
        let matte_texture = overlay
            .as_ref()
            .and_then(|context| self.matte_view_texture(ui.ctx(), blocked, context));
        let texture = self.texture.clone();
        let picture = viewer_picture(blocked, matte_texture.as_ref(), texture.as_ref());
        let frame = Self::paint_viewer_frame(
            ui,
            egui::vec2(available_width, frame_height),
            "PROGRAM",
            picture,
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
            viewer_sense(overlay.is_some()),
        );
        if let Some(context) = overlay {
            if let Some(image_rect) = frame.image_rect {
                paint_matte_overlay(
                    &ui.painter_at(frame.response.rect),
                    context.frame(image_rect),
                    &context.matte,
                    &self.matte_overlay,
                );
                self.handle_matte_pointer(&frame.response, image_rect, &context);
            }
            self.matte_viewer_controls(ui, &context);
        }
    }

    /// The node whose matte section the inspector reported as expanded, with
    /// everything the overlay needs to draw and edit it (CC5 §6).
    ///
    /// `None` — and therefore no overlay and no pointer capture — whenever the
    /// report names a clip that is not selected, or a clip or effect that is
    /// gone. A matte with no window still yields a context: the qualifier, the
    /// mix, and the Matte view are all editable without one.
    fn matte_overlay_context(&self) -> Option<MatteOverlayContext> {
        let target = self.matte_overlay.expanded()?;
        let session = self.focused();
        matte_overlay_context_for(
            &session.document,
            session.selected_clip,
            session.position,
            target,
            session.id,
            session.revision.0,
        )
    }

    /// Turn one frame of viewer pointer interaction into coalesced edits.
    ///
    /// One gesture is one undo entry: every frame of a drag goes out under the
    /// CC5 §6 coalesce key for what was grabbed, and a move — which writes two
    /// parameters — uses the multi-operation live push.
    fn handle_matte_pointer(
        &mut self,
        response: &egui::Response,
        image_rect: egui::Rect,
        context: &MatteOverlayContext,
    ) {
        let mut edits = InspectorEdits::default();
        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some((window, hit)) = self.matte_hit(pointer, image_rect, context)
        {
            edits.begin_gesture();
            self.matte_overlay.begin_drag(MatteDrag {
                target: context.target,
                window,
                hit,
                start: context.matte.windows[window],
                start_pointer: pointer,
            });
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some((window, _)) = self.matte_hit(pointer, image_rect, context)
        {
            self.matte_overlay
                .select_window(window, context.matte.window_count);
        }
        if let Some(drag) = self.matte_overlay.drag()
            && drag.target == context.target
            && (response.dragged() || response.drag_stopped())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let next =
                crate::matte_overlay_ui::drag_to_params(&drag, pointer, context.frame(image_rect));
            // A frame that asks for the values the document already holds is
            // not an edit: the press frame, and any frame the pointer has not
            // moved far enough to change a basis point, write nothing.
            if next != context.matte.windows[drag.window] {
                let operations = matte_window_drag_operations(
                    context.target.clip,
                    context.target.effect,
                    drag.window,
                    drag.hit,
                    &next,
                );
                edits.extend_live(
                    operations,
                    matte_gesture_coalesce_key(
                        drag.hit,
                        context.target.clip,
                        context.target.effect,
                        drag.window,
                    ),
                );
            }
        }
        if response.drag_stopped() {
            self.matte_overlay.end_drag();
        }
        self.submit_inspector_edits(edits);
    }

    /// The window a pointer grabbed, testing the selected window first so a
    /// window drawn under another stays reachable.
    ///
    /// Only the selected window offers handles and a rotation arm, because only
    /// the selected window draws them: an unselected window is select-then-edit
    /// (CC5 §6). The selection is read clamped to the count the document holds
    /// this frame, so it is exactly the one the overlay painted.
    fn matte_hit(
        &self,
        pointer: egui::Pos2,
        image_rect: egui::Rect,
        context: &MatteOverlayContext,
    ) -> Option<(usize, crate::matte_overlay_ui::MatteHit)> {
        matte_hit_test(
            pointer,
            &context.matte,
            context.frame(image_rect),
            self.matte_overlay
                .selected_window(context.matte.window_count),
        )
    }

    /// The Matte view toggle, the window selector, and the coverage status.
    fn matte_viewer_controls(&mut self, ui: &mut egui::Ui, context: &MatteOverlayContext) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("MATTE").font(theme::semibold(type_size::CAPTION)));
            let mut view = self.matte_overlay.matte_view();
            if ui
                .checkbox(&mut view, "Matte view")
                .on_hover_text(
                    "Show this node's coverage instead of the picture: white is fully \
                     affected, black is untouched (CC5 §4.1).",
                )
                .changed()
            {
                self.matte_overlay.set_matte_view(view);
            }
            let selected = self
                .matte_overlay
                .selected_window(context.matte.window_count);
            for index in 0..context.matte.window_count {
                if ui
                    .selectable_label(selected == Some(index), format!("W{index}"))
                    .clicked()
                {
                    self.matte_overlay
                        .select_window(index, context.matte.window_count);
                }
            }
            if context.matte.window_count == 0 {
                ui.colored_label(color::TEXT_MUTED, "No windows to draw");
            }
            match self.matte_overlay.view_status(context.key) {
                MatteViewStatus::Off => {}
                MatteViewStatus::Pending => {
                    ui.colored_label(color::STATUS_WARNING, "Rendering coverage…");
                }
                MatteViewStatus::Ready => {
                    ui.colored_label(color::STATUS_SUCCESS, "Coverage");
                }
                MatteViewStatus::Unavailable(message) => {
                    ui.colored_label(
                        color::STATUS_DANGER,
                        format!("Matte view unavailable: {message}"),
                    );
                }
            }
        });
        ui.colored_label(
            color::TEXT_MUTED,
            "Windows are stored in the layer's own frame and drawn through this clip's \
             resolved transform, so a reframed clip's outline sits on its reframed \
             picture and a drag writes layer coordinates (CC5 §3.3, §5.2).",
        );
    }

    /// Fetch, cache, and upload the coverage image behind the Matte view.
    ///
    /// The render happens on a worker thread through [`MatteProofSource`], with
    /// the scope panel's single-flight policy: at most one `FrameRenderer` and
    /// its cache budget exist at a time, and the newest request wins.
    ///
    /// [`MatteProofSource`]: crate::matte_overlay_ui::MatteProofSource
    fn matte_view_texture(
        &mut self,
        ctx: &egui::Context,
        blocked: bool,
        context: &MatteOverlayContext,
    ) -> Option<egui::TextureHandle> {
        if !self.matte_overlay.matte_view() {
            return None;
        }
        let key = context.key;
        // Two `Arc` clones a frame, and the state decides whether a worker is
        // wanted: the "is this frame blocked?" rule lives in one place rather
        // than being restated by every caller.
        let source = Arc::new(AnalysisMatteProofSource(Arc::clone(&self.analysis)));
        let document = Arc::clone(&self.focused().document);
        self.matte_overlay
            .request_view_if_needed(blocked, source, document, key);
        if let Some(texture) = self.matte_overlay.texture_for(key) {
            return Some(texture.clone());
        }
        let image = coverage_color_image(self.matte_overlay.coverage_for(key)?);
        // Point sampling: a coverage code is evidence, and a bilinear filter
        // would invent partial coverage that no pixel has.
        let texture = ctx.load_texture("matte-coverage", image, egui::TextureOptions::NEAREST);
        self.matte_overlay.set_texture(key, texture.clone());
        Some(texture)
    }

    fn paint_empty_viewer(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        label: &str,
        message: &str,
        message_color: egui::Color32,
    ) {
        Self::paint_viewer_frame(
            ui,
            size,
            label,
            None,
            message,
            message_color,
            egui::Sense::hover(),
        );
    }

    fn paint_viewer_frame(
        ui: &mut egui::Ui,
        size: egui::Vec2,
        label: &str,
        texture: Option<&egui::TextureHandle>,
        message: &str,
        message_color: egui::Color32,
        sense: egui::Sense,
    ) -> ViewerFrame {
        let (rect, response) = ui.allocate_exact_size(size, sense);
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
                return ViewerFrame {
                    response,
                    image_rect: Some(image_rect),
                };
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
        ViewerFrame {
            response,
            image_rect: None,
        }
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

    /// CC5 §6: the viewer takes pointer input only while a matte section is
    /// expanded. The decision is a pure function so it is provable without a
    /// window, and so the "otherwise unchanged" half is a test rather than a
    /// promise.
    #[test]
    fn the_viewer_takes_pointer_input_only_for_an_expanded_matte_section() {
        let idle = viewer_sense(false);
        assert!(!idle.senses_click(), "an idle viewer is not clickable");
        assert!(!idle.senses_drag(), "an idle viewer is not draggable");
        assert_eq!(idle, egui::Sense::hover(), "today's behaviour, exactly");

        let editing = viewer_sense(true);
        assert!(editing.senses_click());
        assert!(editing.senses_drag());
        assert_eq!(editing, egui::Sense::click_and_drag());
    }

    // -----------------------------------------------------------------------
    // CC5 §6 overlay context
    // -----------------------------------------------------------------------

    const CLIP: kinewright_core::ClipId = kinewright_core::ClipId(10);
    const EFFECT: kinewright_core::EffectId = kinewright_core::EffectId(1);

    /// A one-clip 1920 × 1080 document whose clip starts at `timeline_start`,
    /// runs 30 frames, and carries a `color_wheels` node plus whatever
    /// `transform` percentages are asked for.
    fn matte_document(timeline_start: TimeCode, transform: &[(&str, i64)]) -> Document {
        use std::collections::BTreeMap;

        use kinewright_core::{
            AssetId, ClipContent, Effect, EffectId, MediaAsset, ParamValue, Rational,
        };

        let mut effects = vec![Effect {
            id: EFFECT,
            name: "color_wheels".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        }];
        if !transform.is_empty() {
            effects.push(Effect {
                id: EffectId(2),
                name: "transform".to_owned(),
                parameters: transform
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
                    .collect(),
                keyframes: BTreeMap::new(),
            });
        }
        let mut document = document();
        document.resolution = (1920, 1080);
        document.media_pool = vec![MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from("picture.mov"),
            name: "Picture".to_owned(),
            duration: TimeCode(120),
            fps: Rational::new(30, 1).expect("valid fps"),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
        }];
        document.tracks[0].clips = vec![kinewright_core::Clip {
            id: CLIP,
            asset: AssetId(1),
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start,
            effects,
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }];
        document.duration = TimeCode(timeline_start.0 + 30);
        document.validate().expect("the fixture is a legal project");
        document
    }

    fn context_at(document: &Document, position: TimeCode) -> Option<MatteOverlayContext> {
        matte_overlay_context_for(
            document,
            Some(CLIP),
            position,
            MatteTarget::new(CLIP, EFFECT),
            1,
            0,
        )
    }

    /// CC5 §6: the overlay belongs to the clip under the playhead. With the
    /// playhead off the clip the renderer is showing somebody else's picture,
    /// and `checked_sub`'s old `unwrap_or(ZERO)` fallback drew this node's
    /// windows over it — and let a drag edit a matte nobody could see.
    #[test]
    fn no_overlay_when_the_playhead_is_off_the_selected_clip() {
        // The clip occupies timeline frames 20..50.
        let document = matte_document(TimeCode(20), &[]);

        assert!(
            context_at(&document, TimeCode(19)).is_none(),
            "one frame before the clip is not on the clip"
        );
        assert!(
            context_at(&document, TimeCode(20)).is_some(),
            "the first frame of the clip is"
        );
        assert!(
            context_at(&document, TimeCode(49)).is_some(),
            "and so is the last"
        );
        assert!(
            context_at(&document, TimeCode(50)).is_none(),
            "the end is exclusive: frame 50 belongs to whatever follows"
        );
        assert!(
            context_at(&document, TimeCode(500)).is_none(),
            "and a playhead far past the clip is not a clip-local frame 0"
        );

        // A clip that is not the selected one never yields a context either.
        assert!(
            matte_overlay_context_for(
                &document,
                Some(kinewright_core::ClipId(99)),
                TimeCode(25),
                MatteTarget::new(CLIP, EFFECT),
                1,
                0,
            )
            .is_none()
        );
    }

    /// CC5 §5.2: the overlay resolves the clip's own `transform` at the same
    /// clip-local frame as the matte, in the compositor's units — `scale` a
    /// product of `scale_percent / 100`, the offsets sums of `percent / 50`.
    #[test]
    fn the_overlay_context_resolves_the_layer_transform() {
        let plain = matte_document(TimeCode::ZERO, &[]);
        assert_eq!(
            context_at(&plain, TimeCode(3))
                .expect("a context")
                .transform,
            LayerTransform::IDENTITY,
            "an unreframed clip is the identity, so CC4 projects are unchanged"
        );

        let reframed = matte_document(
            TimeCode(20),
            &[("scale_percent", 50), ("x_percent", 25), ("y_percent", -10)],
        );
        let context = context_at(&reframed, TimeCode(25)).expect("a context");
        assert!(
            (context.transform.scale - 0.5).abs() < 1e-12
                && (context.transform.offset_x - 0.5).abs() < 1e-12
                && (context.transform.offset_y + 0.2).abs() < 1e-12,
            "resolved transform: {:?}",
            context.transform
        );

        // And it reaches the geometry: the window centre is drawn where the
        // reframed picture puts it, not where the raster centre is.
        let image_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(64.0, 36.0));
        let centre = crate::matte_overlay_ui::window_centre_point(
            &context.matte.windows[0],
            context.frame(image_rect),
        );
        assert!(
            (centre.x - 48.0).abs() < 1e-3 && (centre.y - 14.4).abs() < 1e-3,
            "reframed centre: {centre:?}"
        );
    }

    /// CC5 §4.1 and the source-verification contract: a blocked source shows no
    /// picture at all. The Matte view renders the same media through the same
    /// decoder, so it must not become the way that picture reaches the screen.
    #[test]
    fn a_blocked_source_shows_no_picture_not_even_a_matte_view() {
        let matte = 1_u8;
        let texture = 2_u8;

        assert_eq!(
            viewer_picture(false, Some(&matte), Some(&texture)),
            Some(&matte),
            "an available coverage stands in for the picture"
        );
        assert_eq!(
            viewer_picture(false, None, Some(&texture)),
            Some(&texture),
            "and the picture is shown when there is no coverage"
        );
        assert_eq!(
            viewer_picture(true, Some(&matte), Some(&texture)),
            None,
            "a blocked source is blocked, coverage or not"
        );
        assert_eq!(viewer_picture(true, None, Some(&texture)), None);
        assert_eq!(viewer_picture::<u8>(false, None, None), None);
    }
}
