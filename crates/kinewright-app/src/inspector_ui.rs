use std::collections::BTreeMap;

use eframe::egui;
use kinewright_core::{
    COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_MAX_POINTS,
    COLOR_CURVE_MIN_POINTS, COLOR_CURVE_WHITE_BASIS_POINTS, COLOR_NODE_BYPASS_PARAMETER, Clip,
    ClipContent, ClipId, ColorCurveChannel, ColorWheelChannel, ColorWheelControl,
    ColorWheelControlSet, ColorWheelsParams, EFFECT_DESCRIPTORS, Effect, EffectId,
    MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId, MediaKind, Operation, ParamValue, ResolvedCurves,
    TITLE_COLORS, TITLE_FONT_SIZES, TRANSITION_DESCRIPTORS, TimeCode, Title, TitlePosition,
    Transition, color_node_inactive_reason, effect_compatibility_stage, is_audio_effect,
    is_legacy_display_effect,
};

use crate::{
    app::KinewrightApp,
    color_wheel_widget::{self, ColorWheelState, color_wheel},
    curve_editor_widget::{self, curve_editor},
    media_workflow::{paint_source_status, source_display_state},
    theme::{self, color, space, type_size},
    timeline_ui::{is_internal_marker, linked_members, linked_transition_operations},
};

const INSPECTOR_MAX_HEIGHT: f32 = 360.0;

/// Edits gathered from one inspector frame.
///
/// A dragged slider emits one operation per frame so the preview stays live.
/// `coalesce_key` marks those frames so the whole drag lands in a single undo
/// entry; a frame that also carries a discrete edit (a button, a typed value)
/// drops the key and becomes an ordinary batch.
#[derive(Debug, Default)]
pub(crate) struct InspectorEdits {
    operations: Vec<Operation>,
    coalesce_key: Option<String>,
    /// Set on the frame a drag begins so the app opens a fresh gesture
    /// identity. Without it a second drag over the same control would merge
    /// into the previous drag's undo entry.
    gesture_started: bool,
}

impl InspectorEdits {
    /// Record a discrete edit. Discrete edits are never coalesced.
    fn push(&mut self, operation: Operation) {
        self.coalesce_key = None;
        self.operations.push(operation);
    }

    fn extend(&mut self, operations: impl IntoIterator<Item = Operation>) {
        let before = self.operations.len();
        self.operations.extend(operations);
        if self.operations.len() != before {
            self.coalesce_key = None;
        }
    }

    /// Record one frame of a live control gesture.
    fn push_live(&mut self, operation: Operation, coalesce_key: String) {
        if self.operations.is_empty() {
            self.coalesce_key = Some(coalesce_key);
        }
        self.operations.push(operation);
    }

    /// Record one frame of a live control gesture that needs several
    /// operations to express a single value, such as a speed change that also
    /// retimes the linked audio.
    fn extend_live(
        &mut self,
        operations: impl IntoIterator<Item = Operation>,
        coalesce_key: String,
    ) {
        let before = self.operations.len();
        self.operations.extend(operations);
        if before == 0 && self.operations.len() != before {
            self.coalesce_key = Some(coalesce_key);
        }
    }

    fn begin_gesture(&mut self) {
        self.gesture_started = true;
    }

    #[cfg(test)]
    fn operations(&self) -> &[Operation] {
        &self.operations
    }

    #[cfg(test)]
    fn coalesce_key(&self) -> Option<&str> {
        self.coalesce_key.as_deref()
    }
}

/// Stable per-parameter coalesce key for one live primary-correction drag.
fn primary_coalesce_key(clip: ClipId, effect: EffectId, parameter: &str) -> String {
    format!("primary:{}:{}:{parameter}", clip.0, effect.0)
}

/// Stable coalesce key for one live clip-speed drag.
fn speed_coalesce_key(clip: ClipId) -> String {
    format!("speed:{}", clip.0)
}

/// Stable coalesce key for one live audio-gain drag.
fn audio_gain_coalesce_key(clip: ClipId) -> String {
    format!("audio_gain:{}", clip.0)
}

/// Whether a control change belongs to a drag gesture that is still one undo
/// entry.
///
/// Shared with the CC3 trackball and curve widgets so the rule has exactly one
/// definition.
///
/// egui reports the frame the pointer is released as `changed() == true` with
/// `dragged() == false`, so testing `dragged()` alone drops the final value out
/// of the gesture and files it as a second undo entry. `drag_stopped()` marks
/// exactly that release frame, and it carries the same coalesce key so the
/// whole drag — release included — stays one entry.
pub(crate) fn is_live_drag(slider: &egui::Response) -> bool {
    slider.dragged() || slider.drag_stopped()
}

impl KinewrightApp {
    /// Route one inspector frame's edits to the core actor.
    fn submit_inspector_edits(&mut self, edits: InspectorEdits) {
        if edits.gesture_started {
            // Open the new gesture even when this frame produced no operation:
            // a mouse-down without movement still ends the previous gesture.
            self.begin_edit_gesture();
        }
        if edits.operations.is_empty() {
            return;
        }
        match edits.coalesce_key {
            Some(key) => {
                let gesture = self.edit_gesture();
                self.send_operations_coalesced(edits.operations, format!("{key}#{gesture}"));
            }
            None => self.send_operations(edits.operations),
        }
    }

    pub(crate) fn inspector_dock(&mut self, ui: &mut egui::Ui) {
        let id = ui.make_persistent_id("inspector-panel");
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
        if self.focused().title_text_focus.is_some() {
            state.set_open(true);
        }
        state
            .show_header(ui, |ui| {
                ui.label(egui::RichText::new("Inspector").font(theme::semibold(type_size::BODY)))
            })
            .body(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector-scroll")
                    .max_height(INSPECTOR_MAX_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| self.inspector(ui));
            });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        if let Some(clip) = self
            .focused()
            .selected_clip
            .and_then(|id| self.focused().document.clip(id))
            .cloned()
        {
            match &clip.content {
                ClipContent::Media => self.media_clip_inspector(ui, &clip),
                ClipContent::Title(title) => self.title_inspector(ui, &clip, title),
                ClipContent::Freeze(freeze) => self.freeze_clip_inspector(ui, &clip, freeze),
            }
        } else if let Some(marker) = self
            .focused()
            .selected_marker
            .and_then(|id| self.focused().document.marker(id))
            .filter(|marker| !is_internal_marker(marker))
            .cloned()
        {
            self.marker_inspector(ui, &marker);
        } else {
            ui.add_space(space::THREE);
            ui.colored_label(color::TEXT_MUTED, "Select a clip, title, or marker.");
            ui.add_space(space::THREE);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn media_clip_inspector(&mut self, ui: &mut egui::Ui, clip: &Clip) {
        let Some(asset) = self.focused().document.asset(clip.asset).cloned() else {
            ui.colored_label(color::STATUS_DANGER, "Media asset is missing");
            return;
        };
        ui.label(egui::RichText::new(&asset.name).font(theme::semibold(type_size::BODY)));
        ui.colored_label(color::TEXT_MUTED, format!("{:?}", asset.kind));
        ui.add_space(space::ONE);
        data_row(ui, "Path", &asset.path.display().to_string());
        let status = self.media_status_for_asset(&asset);
        let source_state = source_display_state(status.as_ref());
        ui.horizontal(|ui| {
            paint_source_status(ui, source_state);
            if ui
                .button("Relink…")
                .on_hover_text("Choose a replacement and verify its source fingerprint")
                .clicked()
            {
                self.choose_relink_for_asset(asset.id);
            }
        });
        ui.colored_label(
            if source_state.blocks_preview() {
                color::STATUS_DANGER
            } else {
                color::TEXT_MUTED
            },
            source_state.description(),
        );
        data_row(ui, "Source", &range_readout(&clip.source_range, asset.fps));
        let timeline_end = self
            .focused()
            .document
            .clip_duration(clip)
            .map_or(clip.timeline_start, |duration| {
                TimeCode(clip.timeline_start.0.saturating_add(duration.0))
            });
        data_row(
            ui,
            "Timeline",
            &range_readout(
                &(clip.timeline_start..timeline_end),
                self.focused().document.fps,
            ),
        );
        if let Some((width, height)) = asset.resolution {
            data_row(ui, "Raster", &format!("{width} × {height}"));
        }

        let mut pending = InspectorEdits::default();
        ui.add_space(space::TWO);
        ui.strong("Speed");
        let mut speed_percent = clip.speed_percent;
        let speed = ui.add(
            egui::Slider::new(&mut speed_percent, 10..=1000)
                .integer()
                .custom_formatter(|value, _| format!("{:.2}x", value / 100.0))
                .custom_parser(|text| {
                    text.trim()
                        .trim_end_matches(['x', 'X'])
                        .parse::<f64>()
                        .ok()
                        .map(|value| value * 100.0)
                }),
        );
        if speed.drag_started() {
            pending.begin_gesture();
        }
        if clip.speed_percent != 100 {
            ui.colored_label(
                color::TEXT_MUTED,
                "Audio is muted while the speed is not 1.00x",
            );
        }
        if speed.changed() {
            match crate::timeline_ui::clip_speed_operations(
                &self.focused().document,
                clip.id,
                speed_percent,
            ) {
                // A drag emits one batch per frame so the preview stays live;
                // the shared key files the whole drag as one undo entry
                // instead of one per frame.
                Ok(operations) if is_live_drag(&speed) => {
                    pending.extend_live(operations, speed_coalesce_key(clip.id));
                }
                Ok(operations) => pending.extend(operations),
                Err(error) => self.record_error("Operations", error),
            }
        }
        if let Some(audio_clip) = audio_target_clip(&self.focused().document, clip.id) {
            ui.add_space(space::TWO);
            ui.strong("Audio");
            let duration = self
                .focused()
                .document
                .clip_duration(&audio_clip)
                .map_or(0, |duration| duration.0.max(0));
            let mut gain_tenth_db = audio_clip.audio_gain_tenth_db;
            let mut fade_in_frames = audio_clip.audio_fade_in_frames.0;
            let mut fade_out_frames = audio_clip.audio_fade_out_frames.0;
            let gain = ui.add(
                egui::Slider::new(&mut gain_tenth_db, -600..=120)
                    .text("Gain")
                    .integer()
                    .custom_formatter(|value, _| format!("{:+.1} dB", value / 10.0)),
            );
            if gain.drag_started() {
                pending.begin_gesture();
            }
            let mut changed = gain.changed();
            ui.horizontal(|ui| {
                ui.label("Fade in");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut fade_in_frames)
                            .range(0..=duration.saturating_sub(fade_out_frames))
                            .suffix(" f"),
                    )
                    .changed();
                ui.label("Fade out");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut fade_out_frames)
                            .range(0..=duration.saturating_sub(fade_in_frames))
                            .suffix(" f"),
                    )
                    .changed();
            });
            if audio_clip.audio_gain_tenth_db != 0
                || audio_clip.audio_fade_in_frames != TimeCode::ZERO
                || audio_clip.audio_fade_out_frames != TimeCode::ZERO
            {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        format!(
                            "gain:{:+.1} dB  fade_in:{}f  fade_out:{}f",
                            tenth_db_to_db(audio_clip.audio_gain_tenth_db),
                            audio_clip.audio_fade_in_frames.0,
                            audio_clip.audio_fade_out_frames.0
                        ),
                    );
                    if ui.small_button("Reset").clicked() {
                        gain_tenth_db = 0;
                        fade_in_frames = 0;
                        fade_out_frames = 0;
                        changed = true;
                    }
                });
            }
            if changed {
                let operation = clip_audio_operation(
                    audio_clip.id,
                    gain_tenth_db,
                    fade_in_frames,
                    fade_out_frames,
                );
                // Only a live gain drag coalesces. A fade edit or Reset click
                // cannot happen while the gain slider is dragged, so the gain
                // response alone decides.
                if is_live_drag(&gain) {
                    pending.push_live(operation, audio_gain_coalesce_key(audio_clip.id));
                } else {
                    pending.push(operation);
                }
            }
        }

        effects_section(ui, clip, &mut pending);
        transition_section(ui, &self.focused().document, clip, &mut pending);
        self.submit_inspector_edits(pending);
    }

    fn freeze_clip_inspector(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        freeze: &kinewright_core::FreezeFrame,
    ) {
        let Some(asset) = self.focused().document.asset(clip.asset).cloned() else {
            ui.colored_label(color::STATUS_DANGER, "Freeze source asset is missing");
            return;
        };
        ui.label(egui::RichText::new(&asset.name).font(theme::semibold(type_size::BODY)));
        ui.colored_label(color::TEXT_MUTED, "Freeze frame");
        ui.add_space(space::ONE);
        data_row(
            ui,
            "Frozen source",
            &frame_readout(freeze.source_frame, asset.fps),
        );
        let duration = self
            .focused()
            .document
            .clip_duration(clip)
            .unwrap_or(TimeCode::ZERO);
        data_row(
            ui,
            "Duration",
            &frame_readout(duration, self.focused().document.fps),
        );
        let mut pending = InspectorEdits::default();
        effects_section(ui, clip, &mut pending);
        transition_section(ui, &self.focused().document, clip, &mut pending);
        self.submit_inspector_edits(pending);
    }

    #[allow(clippy::too_many_lines)]
    fn title_inspector(&mut self, ui: &mut egui::Ui, clip: &Clip, title: &Title) {
        ui.strong("Title");
        let timeline_end = self
            .focused()
            .document
            .clip_duration(clip)
            .map_or(clip.timeline_start, |duration| {
                TimeCode(clip.timeline_start.0.saturating_add(duration.0))
            });
        data_row(
            ui,
            "Timeline",
            &range_readout(
                &(clip.timeline_start..timeline_end),
                self.focused().document.fps,
            ),
        );
        let focus_title = self.focused().title_text_focus == Some(clip.id);
        if focus_title {
            self.focused_mut().title_text_focus = None;
        }
        let draft = self
            .focused_mut()
            .title_text_draft
            .get_or_insert_with(|| (clip.id, title.text.clone()));
        if draft.0 != clip.id {
            *draft = (clip.id, title.text.clone());
        }
        let response = ui
            .scope(|ui| {
                theme::apply_input_visuals(ui);
                ui.add(
                    egui::TextEdit::multiline(&mut draft.1)
                        .desired_rows(2)
                        .hint_text("Title text"),
                )
            })
            .inner;
        if focus_title {
            response.request_focus();
        }
        let submit_text = response.lost_focus()
            || (response.has_focus()
                && ui
                    .input(|input| input.modifiers.command && input.key_pressed(egui::Key::Enter)));
        let mut pending = Vec::new();
        if submit_text && draft.1 != title.text {
            pending.push(title_param_operation(
                clip.id,
                "text",
                ParamValue::Text(draft.1.clone()),
            ));
        }

        let mut size_token = title.font_size_token;
        egui::ComboBox::from_id_salt(("title-size", clip.id.0))
            .selected_text(
                TITLE_FONT_SIZES
                    .iter()
                    .find(|item| item.token == size_token)
                    .map_or("Unknown", |item| item.name),
            )
            .show_ui(ui, |ui| {
                for item in TITLE_FONT_SIZES {
                    ui.selectable_value(&mut size_token, item.token, item.name);
                }
            });
        if size_token != title.font_size_token {
            pending.push(title_param_operation(
                clip.id,
                "font_size_token",
                ParamValue::Integer(i64::from(size_token)),
            ));
        }

        let mut color_token = title.color_token;
        egui::ComboBox::from_id_salt(("title-color", clip.id.0))
            .selected_text(
                TITLE_COLORS
                    .iter()
                    .find(|item| item.token == color_token)
                    .map_or("Unknown", |item| item.name),
            )
            .show_ui(ui, |ui| {
                for item in TITLE_COLORS {
                    ui.selectable_value(&mut color_token, item.token, item.name);
                }
            });
        if color_token != title.color_token {
            pending.push(title_param_operation(
                clip.id,
                "color_token",
                ParamValue::Integer(i64::from(color_token)),
            ));
        }

        let mut position = title.position;
        egui::ComboBox::from_id_salt(("title-position", clip.id.0))
            .selected_text(position.as_str())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut position, TitlePosition::Top, "top");
                ui.selectable_value(&mut position, TitlePosition::Center, "center");
                ui.selectable_value(&mut position, TitlePosition::LowerThird, "lower third");
            });
        if position != title.position {
            pending.push(title_param_operation(
                clip.id,
                "position",
                ParamValue::Text(position.as_str().to_owned()),
            ));
        }

        let mut scrim = title.background_scrim;
        if ui.checkbox(&mut scrim, "Background scrim").changed() {
            pending.push(title_param_operation(
                clip.id,
                "background_scrim",
                ParamValue::Boolean(scrim),
            ));
        }
        let maximum = self
            .focused()
            .document
            .clip_duration(clip)
            .map_or(0, |value| value.0.max(0));
        for (name, label, current) in [
            ("fade_in_frames", "Fade in", title.fade_in_frames.0),
            ("fade_out_frames", "Fade out", title.fade_out_frames.0),
        ] {
            let mut value = current;
            if ui
                .add(
                    egui::Slider::new(&mut value, 0..=maximum)
                        .text(label)
                        .integer(),
                )
                .changed()
            {
                pending.push(title_param_operation(
                    clip.id,
                    name,
                    ParamValue::Integer(value),
                ));
            }
        }
        self.send_operations(pending);
    }

    fn marker_inspector(&mut self, ui: &mut egui::Ui, marker: &Marker) {
        ui.strong("Marker");
        data_row(
            ui,
            "Position",
            &frame_readout(marker.position, self.focused().document.fps),
        );
        let draft = self
            .focused_mut()
            .marker_label_draft
            .get_or_insert_with(|| (marker.id, marker.label.clone()));
        if draft.0 != marker.id {
            *draft = (marker.id, marker.label.clone());
        }
        let response = ui
            .scope(|ui| {
                theme::apply_input_visuals(ui);
                ui.text_edit_singleline(&mut draft.1)
            })
            .inner;
        let mut pending = Vec::new();
        if response.lost_focus() && draft.1 != marker.label {
            pending.push(marker_param_operation(
                marker.id,
                "label",
                ParamValue::Text(draft.1.clone()),
            ));
        }
        let mut color_token = marker.color_token;
        egui::ComboBox::from_id_salt(("marker-color", marker.id.0))
            .selected_text(format!("Color {}", color_token + 1))
            .show_ui(ui, |ui| {
                for token in 0..MARKER_COLOR_TOKEN_COUNT {
                    ui.selectable_value(&mut color_token, token, format!("Color {}", token + 1));
                }
            });
        if color_token != marker.color_token {
            pending.push(marker_param_operation(
                marker.id,
                "color_token",
                ParamValue::Integer(i64::from(color_token)),
            ));
        }
        let mut position = marker.position.0;
        if ui
            .add(
                egui::DragValue::new(&mut position)
                    .range(0..=i64::MAX)
                    .prefix("Frame "),
            )
            .changed()
        {
            pending.push(marker_param_operation(
                marker.id,
                "position",
                ParamValue::Integer(position),
            ));
        }
        self.send_operations(pending);
    }
}

fn effects_section(ui: &mut egui::Ui, clip: &Clip, pending: &mut InspectorEdits) {
    ui.add_space(space::TWO);
    ui.strong("Effects");
    for (stage_index, effect) in clip.effects.iter().enumerate() {
        match effect.name.as_str() {
            "primary_correction" => {
                primary_correction_section(ui, clip, effect, pending);
                continue;
            }
            "color_wheels" => {
                color_wheels_section(ui, clip, effect, stage_index, pending);
                continue;
            }
            "color_curves" => {
                color_curves_section(ui, clip, effect, stage_index, pending);
                continue;
            }
            _ => {}
        }
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(&effect.name);
                if ui.small_button("Remove").clicked() {
                    pending.push(Operation::RemoveEffect {
                        clip: clip.id,
                        effect: effect.id,
                    });
                }
            });
            if let Some(descriptor) = EFFECT_DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.name == effect.name)
            {
                for parameter in descriptor.parameters {
                    if !should_render_effect_parameter(descriptor, parameter.name) {
                        continue;
                    }
                    let mut value = effect
                        .parameters
                        .get(parameter.name)
                        .and_then(|value| match value {
                            ParamValue::Integer(value) => Some(*value),
                            ParamValue::Boolean(_) | ParamValue::Text(_) => None,
                        })
                        .unwrap_or(parameter.neutral);
                    if ui
                        .add(
                            egui::Slider::new(&mut value, parameter.min..=parameter.max)
                                .text(parameter.name)
                                .integer(),
                        )
                        .changed()
                    {
                        pending.push(effect_param_operation(
                            clip.id,
                            effect.id,
                            parameter.name,
                            value,
                        ));
                    }
                }
            }
            if let Some(stage) = effect_compatibility_stage(&effect.name) {
                ui.colored_label(color::STATUS_WARNING, stage.inspector_warning());
            }
        });
    }
    ui.menu_button("+ Effect", |ui| {
        for descriptor in EFFECT_DESCRIPTORS {
            if !is_effect_insertable(descriptor.name) {
                continue;
            }
            if clip
                .effects
                .iter()
                .any(|effect| effect.name == descriptor.name)
            {
                continue;
            }
            if ui.button(effect_display_name(descriptor.name)).clicked() {
                pending.push(add_effect_operation(clip, descriptor));
                ui.close();
            }
        }
    });
}

fn primary_correction_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    pending: &mut InspectorEdits,
) {
    let Some(descriptor) = EFFECT_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.name == "primary_correction")
    else {
        return;
    };

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Primary correction").strong());
            ui.colored_label(color::TEXT_MUTED, "Managed SDR");
            if ui.small_button("Remove").clicked() {
                pending.push(Operation::RemoveEffect {
                    clip: clip.id,
                    effect: effect.id,
                });
            }
        });
        ui.horizontal(|ui| {
            ui.colored_label(
                color::TEXT_MUTED,
                "Exposure · white balance · tone · saturation",
            );
            if ui.small_button("Reset Primary").clicked() {
                pending.extend(color_node_reset_operations(clip.id, effect, descriptor));
            }
        });

        for parameter in descriptor.parameters {
            let mut value = effect
                .parameters
                .get(parameter.name)
                .and_then(|value| match value {
                    ParamValue::Integer(value) => Some(*value),
                    ParamValue::Boolean(_) | ParamValue::Text(_) => None,
                })
                .unwrap_or(parameter.neutral);
            let keyframed = parameter_is_keyframed(effect, parameter.name);
            ui.horizontal(|ui| {
                let mut slider = ui.add(
                    egui::Slider::new(&mut value, parameter.min..=parameter.max)
                        .text(primary_parameter_label(parameter.name))
                        .integer(),
                );
                if keyframed {
                    slider = slider.on_hover_text(
                        "Automation drives this parameter. The slider shows the static value; \
                         clear the keyframes to grade it directly.",
                    );
                }
                ui.monospace(primary_parameter_readout(parameter.name, value));
                if slider.drag_started() {
                    pending.begin_gesture();
                }
                if slider.changed() {
                    let operation =
                        effect_param_operation(clip.id, effect.id, parameter.name, value);
                    if is_live_drag(&slider) {
                        // One batch per frame keeps the preview live; the key
                        // keeps the whole drag as one undo entry.
                        pending.push_live(
                            operation,
                            primary_coalesce_key(clip.id, effect.id, parameter.name),
                        );
                    } else {
                        pending.push(operation);
                    }
                }
                if keyframed {
                    ui.colored_label(color::STATUS_WARNING, "KEYFRAMED");
                    if ui
                        .small_button("Clear keyframes")
                        .on_hover_text(
                            "Remove this parameter's automation so the slider value applies.",
                        )
                        .clicked()
                    {
                        pending.push(clear_keyframes_operation(
                            clip.id,
                            effect.id,
                            parameter.name,
                        ));
                    }
                }
            });
        }
    });
}

/// Reset one effect: every descriptor parameter set to its neutral, plus
/// `ClearEffectKeyframes` for each parameter that carries automation, emitted
/// as one batch and therefore one undo entry.
///
/// CC3 §5 names this the `primary_reset_operations` pattern; it is the same
/// code for `primary_correction`, `color_wheels`, and `color_curves`, which is
/// why CC3 introduces no new operation kind.
///
/// `color_curves` is the one node whose parameters cannot be written in
/// descriptor order: core re-validates the strictly-increasing-`x` rule on
/// every intermediate document, and descriptor order writes `x0` before `x1`.
/// From a stored `x0 = -2000, x1 = -1000` that first write already crosses, so
/// the operation - and with it the whole reset batch - would be rejected. The
/// curve half is therefore routed through the same ordering strategy the curve
/// editor uses.
fn color_node_reset_operations(
    clip: ClipId,
    effect: &Effect,
    descriptor: &kinewright_core::EffectDescriptor,
) -> Vec<Operation> {
    let mut operations = Vec::with_capacity(descriptor.parameters.len() + effect.keyframes.len());
    if kinewright_core::ColorNodeKind::from_effect_name(descriptor.name)
        == Some(kinewright_core::ColorNodeKind::Curves)
    {
        for curve in ColorCurveChannel::ALL {
            operations.extend(curve_reset_parameter_operations(clip, effect, curve));
        }
        // `bypass` is node-owned rather than curve-owned; it is written after
        // every curve is back at the structural identity so the batch never
        // depends on the order the two halves happen to land in.
        operations.push(effect_param_operation(
            clip,
            effect.id,
            COLOR_NODE_BYPASS_PARAMETER,
            0,
        ));
        for parameter in descriptor.parameters {
            if parameter_is_keyframed(effect, parameter.name) {
                operations.push(clear_keyframes_operation(clip, effect.id, parameter.name));
            }
        }
        return operations;
    }
    for parameter in descriptor.parameters {
        operations.push(effect_param_operation(
            clip,
            effect.id,
            parameter.name,
            parameter.neutral,
        ));
        if parameter_is_keyframed(effect, parameter.name) {
            operations.push(clear_keyframes_operation(clip, effect.id, parameter.name));
        }
    }
    operations
}

/// Stable per-wheel coalesce key for one live trackball or master drag.
fn wheels_coalesce_key(clip: ClipId, effect: EffectId, control: ColorWheelControl) -> String {
    format!(
        "wheels:{}:{}:{}",
        clip.0,
        effect.0,
        color_wheel_widget::control_token(control)
    )
}

/// Stable per-curve coalesce key for one live curve-point drag.
fn curves_coalesce_key(clip: ClipId, effect: EffectId, curve: ColorCurveChannel) -> String {
    format!("curves:{}:{}:{}", clip.0, effect.0, curve.name())
}

/// The CC3 §7 `color_wheels` card: three trackballs, a bypass toggle, a reset,
/// and the keyframe state of every control.
fn color_wheels_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    stage_index: usize,
    pending: &mut InspectorEdits,
) {
    let Some(descriptor) = kinewright_core::effect_descriptor("color_wheels") else {
        return;
    };
    let params = ColorWheelsParams::from_effect(effect);
    ui.group(|ui| {
        color_node_header(
            ui,
            clip,
            effect,
            &descriptor,
            stage_index,
            "Colour wheels",
            pending,
        );
        // Wrapped so a narrow inspector stacks the balls instead of clipping
        // the third one out of reach.
        ui.horizontal_wrapped(|ui| {
            for control in ColorWheelControl::ALL {
                let state = wheel_state(effect, params, control);
                let response = color_wheel(ui, &state);
                apply_wheel_response(clip.id, effect, control, &response, pending);
            }
        });
        let names: Vec<&'static str> = descriptor
            .parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect();
        color_node_keyframe_rows(ui, clip.id, effect, &names, pending);
    });
}

/// One trackball's document state, read from the stored integers.
fn wheel_state(
    effect: &Effect,
    params: ColorWheelsParams,
    control: ColorWheelControl,
) -> ColorWheelState {
    ColorWheelState {
        control,
        values: ColorWheelControlSet {
            master: params.control(control, ColorWheelChannel::Master),
            red: params.control(control, ColorWheelChannel::Red),
            green: params.control(control, ColorWheelChannel::Green),
            blue: params.control(control, ColorWheelChannel::Blue),
        },
        keyframed: ColorWheelChannel::ALL
            .map(|channel| parameter_is_keyframed(effect, control.parameter_name(channel))),
    }
}

/// Turn one frame of trackball interaction into operations.
///
/// A drag emits one batch per frame under the wheel's coalesce key, so the
/// preview stays live while the whole gesture collapses to a single undo entry
/// (CC3 §7). A double-click is a discrete reset of that wheel's four controls.
fn apply_wheel_response(
    clip: ClipId,
    effect: &Effect,
    control: ColorWheelControl,
    response: &crate::color_wheel_widget::ColorWheelResponse,
    pending: &mut InspectorEdits,
) {
    if response.gesture_started {
        pending.begin_gesture();
    }
    if response.reset {
        pending.extend(wheel_reset_operations(clip, effect, control));
        return;
    }
    if response.changes.is_empty() {
        return;
    }
    let operations = response.changes.iter().map(|(channel, value)| {
        effect_param_operation(clip, effect.id, control.parameter_name(*channel), *value)
    });
    if response.live {
        pending.extend_live(operations, wheels_coalesce_key(clip, effect.id, control));
    } else {
        pending.extend(operations);
    }
}

/// Reset one wheel: its four controls to their neutrals, plus a keyframe clear
/// for each that carries automation.
fn wheel_reset_operations(
    clip: ClipId,
    effect: &Effect,
    control: ColorWheelControl,
) -> Vec<Operation> {
    let (_, _, neutral) = control.bounds();
    let mut operations = Vec::with_capacity(ColorWheelChannel::ALL.len());
    for channel in ColorWheelChannel::ALL {
        let name = control.parameter_name(channel);
        operations.push(effect_param_operation(clip, effect.id, name, neutral));
        if parameter_is_keyframed(effect, name) {
            operations.push(clear_keyframes_operation(clip, effect.id, name));
        }
    }
    operations
}

/// The CC3 §7 `color_curves` card: a channel selector, the curve editor, a
/// per-curve reset, and the automation-truncation warning.
fn color_curves_section(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    stage_index: usize,
    pending: &mut InspectorEdits,
) {
    let Some(descriptor) = kinewright_core::effect_descriptor("color_curves") else {
        return;
    };
    let resolved = ResolvedCurves::from_effect(effect);
    ui.group(|ui| {
        color_node_header(
            ui,
            clip,
            effect,
            &descriptor,
            stage_index,
            "Colour curves",
            pending,
        );

        let truncated = automation_truncated_curves_cached(ui, effect, &resolved);
        if !truncated.is_empty() {
            let names = truncated
                .iter()
                .map(|curve| curve.name())
                .collect::<Vec<_>>()
                .join(", ");
            ui.colored_label(
                color::STATUS_WARNING,
                format!(
                    "curve_truncated_by_automation: {names} resolves without strictly increasing \
                     x, so the curve renders as its longest valid prefix (CC3 §3.4)."
                ),
            );
        }

        let selection_id = ui.make_persistent_id(("color-curves-channel", clip.id.0, effect.id.0));
        let mut selected = ui
            .data(|data| data.get_temp::<ColorCurveChannel>(selection_id))
            .unwrap_or(ColorCurveChannel::Master);
        ui.horizontal(|ui| {
            for curve in ColorCurveChannel::ALL {
                if ui
                    .selectable_label(selected == curve, curve_editor_widget::curve_label(curve))
                    .clicked()
                {
                    selected = curve;
                }
            }
            if ui
                .small_button("Reset curve")
                .on_hover_text("Restore this curve to (0, 0) and (10000, 10000).")
                .clicked()
            {
                pending.extend(curve_reset_operations(clip.id, effect, selected));
            }
        });
        ui.data_mut(|data| data.insert_temp(selection_id, selected));

        let points = resolved.curve(selected).points.clone();
        let editor_id = ui.make_persistent_id(("color-curve-editor", clip.id.0, effect.id.0));
        let response = curve_editor(ui, &points, selected, editor_id);
        apply_curve_response(clip.id, effect, selected, &points, &response, pending);
        ui.colored_label(
            color::TEXT_MUTED,
            format!(
                "{} points · click to add · drag to shape · right-click or Delete to remove · \
                 double-click to reset",
                points.len()
            ),
        );

        color_curve_keyframe_rows(ui, clip.id, effect, pending);
    });
}

/// The keyframe indicators of a `color_curves` card (CC3 §7).
///
/// §7 requires an indicator per keyframed control, not per keyframed control
/// *of the curve that happens to be selected*: automation on the red curve
/// must stay visible while the master curve is on screen, otherwise switching
/// tabs is the only way to discover it. Rows are grouped by owning curve so a
/// bare `red_y3` is still attributable.
fn color_curve_keyframe_rows(
    ui: &mut egui::Ui,
    clip: ClipId,
    effect: &Effect,
    pending: &mut InspectorEdits,
) {
    let groups = color_curve_keyframe_groups(effect);
    if groups.is_empty() {
        return;
    }
    ui.colored_label(color::STATUS_WARNING, KEYFRAME_ROWS_NOTE);
    for (label, names) in groups {
        ui.label(egui::RichText::new(label).size(type_size::CAPTION).strong());
        for name in names {
            keyframe_row(ui, clip, effect, name, pending);
        }
    }
}

/// The keyframed controls of a `color_curves` node, grouped by owner in
/// `ColorCurveChannel::ALL` order with the node-owned `bypass` last.
fn color_curve_keyframe_groups(effect: &Effect) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut groups: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for curve in ColorCurveChannel::ALL {
        let keyframed: Vec<&'static str> = curve
            .parameter_names()
            .iter()
            .copied()
            .filter(|name| parameter_is_keyframed(effect, name))
            .collect();
        if !keyframed.is_empty() {
            groups.push((curve_group_label(curve), keyframed));
        }
    }
    if parameter_is_keyframed(effect, COLOR_NODE_BYPASS_PARAMETER) {
        groups.push(("Node", vec![COLOR_NODE_BYPASS_PARAMETER]));
    }
    groups
}

/// The heading of one keyframe group. The tab strip abbreviates the three
/// channels to `R`/`G`/`B`; a group heading standing above a bare `red_y3` has
/// to spell the curve out.
const fn curve_group_label(curve: ColorCurveChannel) -> &'static str {
    match curve {
        ColorCurveChannel::Master => "Master",
        ColorCurveChannel::Red => "Red",
        ColorCurveChannel::Green => "Green",
        ColorCurveChannel::Blue => "Blue",
    }
}

/// Turn one frame of curve interaction into operations.
fn apply_curve_response(
    clip: ClipId,
    effect: &Effect,
    curve: ColorCurveChannel,
    current: &[(i32, i32)],
    response: &crate::curve_editor_widget::CurveEditorResponse,
    pending: &mut InspectorEdits,
) {
    if response.gesture_started {
        pending.begin_gesture();
    }
    if response.reset {
        pending.extend(curve_reset_operations(clip, effect, curve));
        return;
    }
    let Some(points) = response.points.as_deref() else {
        return;
    };
    let operations = curve_edit_operations(clip, effect.id, curve, current, points);
    if response.live {
        pending.extend_live(operations, curves_coalesce_key(clip, effect.id, curve));
    } else {
        pending.extend(operations);
    }
}

/// The operations that turn one curve's stored points into `next`.
///
/// CC3 §2.4: only `{curve}_point_count` and the active points' coordinates are
/// written, because an omitted parameter resolves to its neutral.
///
/// Core validates every `SetEffectParam` against the document the change would
/// produce, so the *order* inside the batch is load-bearing: an intermediate
/// state whose active prefix is not strictly increasing in `x` would be
/// rejected even though both the start and the end state are legal. Inserting a
/// point moves every later coordinate to a smaller `x`, so ascending writes are
/// safe; removing one moves them to a larger `x`, so the count shrinks first
/// and the writes run descending. Anything else collapses the active prefix to
/// two points, rewrites, and restores the count.
fn curve_edit_operations(
    clip: ClipId,
    effect: EffectId,
    curve: ColorCurveChannel,
    current: &[(i32, i32)],
    next: &[(i32, i32)],
) -> Vec<Operation> {
    let mut operations = Vec::with_capacity(2 + next.len() * 2);
    let count = |operations: &mut Vec<Operation>, value: usize| {
        operations.push(effect_param_operation(
            clip,
            effect,
            curve.point_count_parameter(),
            i64::try_from(value).unwrap_or(i64::MAX),
        ));
    };
    let point = |operations: &mut Vec<Operation>, index: usize| {
        let (Some(x_name), Some(y_name)) = (curve.x_parameter(index), curve.y_parameter(index))
        else {
            return;
        };
        let (x, y) = next[index];
        operations.push(effect_param_operation(clip, effect, x_name, i64::from(x)));
        operations.push(effect_param_operation(clip, effect, y_name, i64::from(y)));
    };

    let moves_left = next
        .iter()
        .zip(current)
        .all(|(next, current)| next.0 <= current.0);
    // The descending branch writes `{curve}_point_count` *first*, so it must
    // never grow the active prefix: growing it would expose the colliding
    // `(10000, 10000)` neutrals of the points that are still unwritten, and
    // core would reject the count. `zip` stops at the shorter list, so the
    // length guard is not implied by the coordinate comparison and has to be
    // stated.
    let moves_right = next.len() <= current.len()
        && next
            .iter()
            .zip(current)
            .all(|(next, current)| next.0 >= current.0);
    debug_assert!(
        !moves_right || next.len() <= current.len(),
        "the count-first curve branch must never grow the active prefix",
    );
    if moves_left {
        for index in 0..next.len() {
            point(&mut operations, index);
        }
        count(&mut operations, next.len());
    } else if moves_right {
        count(&mut operations, next.len());
        for index in (0..next.len()).rev() {
            point(&mut operations, index);
        }
    } else {
        count(&mut operations, kinewright_core::COLOR_CURVE_MIN_POINTS);
        for index in kinewright_core::COLOR_CURVE_MIN_POINTS..next.len() {
            point(&mut operations, index);
        }
        if next
            .first()
            .is_some_and(|first| current.get(1).is_some_and(|second| first.0 < second.0))
        {
            point(&mut operations, 0);
            point(&mut operations, 1);
        } else {
            point(&mut operations, 1);
            point(&mut operations, 0);
        }
        count(&mut operations, next.len());
    }
    operations
}

/// The structural identity a curve reset targets: `(0, 0)` and
/// `(10000, 10000)` (CC3 §2.3).
const CURVE_RESET_POINTS: [(i32, i32); COLOR_CURVE_MIN_POINTS] = [
    (0, 0),
    (
        COLOR_CURVE_WHITE_BASIS_POINTS,
        COLOR_CURVE_WHITE_BASIS_POINTS,
    ),
];

/// Reset one curve: its 33 parameters to their neutrals, plus a keyframe clear
/// for each that carries automation (CC3 §5).
fn curve_reset_operations(
    clip: ClipId,
    effect: &Effect,
    curve: ColorCurveChannel,
) -> Vec<Operation> {
    let mut operations = curve_reset_parameter_operations(clip, effect, curve);
    for parameter in curve.parameters() {
        if parameter_is_keyframed(effect, parameter.name) {
            operations.push(clear_keyframes_operation(clip, effect.id, parameter.name));
        }
    }
    operations
}

/// The `SetEffectParam`s of one curve reset, in an order core accepts.
///
/// Descriptor order is *not* accepted: it writes `x0` first, and from a stored
/// `x0 = -2000, x1 = -1000` the intermediate `x0 = 0, x1 = -1000` is not
/// strictly increasing, so core rejects the operation and `apply_batch`
/// discards the entire reset. The active pair therefore goes through
/// [`curve_edit_operations`], which already owns the proof that one of the two
/// write orders is always legal; the remaining points are written afterwards,
/// while `{curve}_point_count` is back at two and they are inactive.
fn curve_reset_parameter_operations(
    clip: ClipId,
    effect: &Effect,
    curve: ColorCurveChannel,
) -> Vec<Operation> {
    let stored = stored_curve_points(effect, curve);
    let mut operations =
        curve_edit_operations(clip, effect.id, curve, &stored, &CURVE_RESET_POINTS);
    let descriptors = curve.parameters();
    let neutral = |name: &str| {
        descriptors
            .iter()
            .find(|parameter| parameter.name == name)
            .map_or(i64::from(COLOR_CURVE_WHITE_BASIS_POINTS), |parameter| {
                parameter.neutral
            })
    };
    // Points 2..16 are inactive once the count is back at two, so their
    // deliberately colliding `(10000, 10000)` neutrals are never examined by
    // the strict-`x` check (CC3 §2.3).
    for index in COLOR_CURVE_MIN_POINTS..COLOR_CURVE_MAX_POINTS {
        let (Some(x_name), Some(y_name)) = (curve.x_parameter(index), curve.y_parameter(index))
        else {
            break;
        };
        operations.push(effect_param_operation(
            clip,
            effect.id,
            x_name,
            neutral(x_name),
        ));
        operations.push(effect_param_operation(
            clip,
            effect.id,
            y_name,
            neutral(y_name),
        ));
    }
    operations
}

/// The *stored*, untruncated point list of one curve.
///
/// Reset ordering must reason about the prefix core validates, which is
/// `{curve}_point_count` raw parameters, not about the §3.4-truncated list the
/// editor draws.
fn stored_curve_points(effect: &Effect, curve: ColorCurveChannel) -> Vec<(i32, i32)> {
    let descriptors = curve.parameters();
    let stored = |index: usize| -> i64 {
        let descriptor = &descriptors[index];
        effect
            .parameters
            .get(descriptor.name)
            .and_then(|value| match value {
                ParamValue::Integer(value) => Some(*value),
                ParamValue::Boolean(_) | ParamValue::Text(_) => None,
            })
            .unwrap_or(descriptor.neutral)
    };
    let minimum = i64::try_from(COLOR_CURVE_MIN_POINTS).unwrap_or(2);
    let maximum = i64::try_from(COLOR_CURVE_MAX_POINTS).unwrap_or(16);
    let count =
        usize::try_from(stored(0).clamp(minimum, maximum)).unwrap_or(COLOR_CURVE_MIN_POINTS);
    let coordinate = |value: i64| {
        i32::try_from(value.clamp(COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_COORDINATE_MAX))
            .unwrap_or(COLOR_CURVE_WHITE_BASIS_POINTS)
    };
    (0..count)
        .map(|index| {
            (
                coordinate(stored(1 + index * 2)),
                coordinate(stored(2 + index * 2)),
            )
        })
        .collect()
}

/// The curves CC3 §3.4 truncation shortens at any keyframe boundary.
///
/// Truncation is a property of the *resolved* curve, so the scan evaluates the
/// node at frame zero and at every curve keyframe. The list is bounded so a
/// pathological automation curve cannot stall the inspector.
fn automation_truncated_curves(effect: &Effect) -> Vec<ColorCurveChannel> {
    const SCAN_LIMIT: usize = 64;
    let mut frames = vec![TimeCode::ZERO];
    for (name, curve) in &effect.keyframes {
        // `bypass` is node-owned rather than curve-owned, but it decides
        // whether a truncation is visible at all. A node that is bypassed at
        // frame zero and live from frame ten would otherwise never be scanned
        // at a frame where its truncation matters.
        if ColorCurveChannel::owning(name).is_none() && *name != COLOR_NODE_BYPASS_PARAMETER {
            continue;
        }
        frames.extend(curve.keyframes.iter().map(|keyframe| keyframe.at));
    }
    frames.sort_unstable();
    frames.dedup();
    frames.truncate(SCAN_LIMIT);
    let mut truncated = Vec::new();
    for at in frames {
        let resolved = ResolvedCurves::from_effect(&effect.evaluated_at(at));
        if resolved.bypass() {
            continue;
        }
        for curve in resolved.truncated_curves() {
            if !truncated.contains(&curve) {
                truncated.push(curve);
            }
        }
    }
    truncated
}

/// [`automation_truncated_curves`] at UI cost.
///
/// The scan clones the node's 133-parameter map once per scanned frame, so
/// running it unconditionally every frame costs up to 64 clones for a warning
/// that changes only when the automation does. A node with no automation needs
/// no scan at all: `resolved` is already the answer. A node with automation is
/// scanned once and memoised in egui's temporary store under a fingerprint of
/// its keyframes, so editing them invalidates the entry.
fn automation_truncated_curves_cached(
    ui: &egui::Ui,
    effect: &Effect,
    resolved: &ResolvedCurves,
) -> Vec<ColorCurveChannel> {
    if effect.keyframes.is_empty() {
        return if resolved.bypass() {
            Vec::new()
        } else {
            resolved.truncated_curves()
        };
    }
    let id = ui.make_persistent_id(("color-curves-truncation", effect.id.0));
    let fingerprint = keyframe_fingerprint(effect);
    if let Some((cached, curves)) =
        ui.data(|data| data.get_temp::<(u64, Vec<ColorCurveChannel>)>(id))
        && cached == fingerprint
    {
        return curves;
    }
    let curves = automation_truncated_curves(effect);
    ui.data_mut(|data| data.insert_temp(id, (fingerprint, curves.clone())));
    curves
}

/// A cheap hash of everything the truncation scan reads out of a node's
/// automation.
///
/// Interpolation is deliberately excluded: the scan only evaluates the effect
/// *at* keyframe frames, where every interpolation mode agrees.
fn keyframe_fingerprint(effect: &Effect) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (name, curve) in &effect.keyframes {
        name.hash(&mut hasher);
        curve.keyframes.len().hash(&mut hasher);
        for keyframe in &curve.keyframes {
            keyframe.at.0.hash(&mut hasher);
            keyframe.value.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// The shared header of a CC3 colour-node card: name, stage index, bypass,
/// reset, and remove.
fn color_node_header(
    ui: &mut egui::Ui,
    clip: &Clip,
    effect: &Effect,
    descriptor: &kinewright_core::EffectDescriptor,
    stage_index: usize,
    title: &str,
    pending: &mut InspectorEdits,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.colored_label(color::TEXT_MUTED, format!("Stage {stage_index}"));
        let mut bypass = bypass_token(effect) >= 1;
        if ui
            .checkbox(&mut bypass, "Bypass")
            .on_hover_text(
                "A bypassed node keeps its position and every value and renders as the exact \
                 identity (CC3 §5).",
            )
            .changed()
        {
            pending.push(effect_param_operation(
                clip.id,
                effect.id,
                COLOR_NODE_BYPASS_PARAMETER,
                i64::from(bypass),
            ));
        }
        if ui
            .small_button("Reset")
            .on_hover_text("Return every control to its neutral and clear its automation.")
            .clicked()
        {
            pending.extend(color_node_reset_operations(clip.id, effect, descriptor));
        }
        if ui.small_button("Remove").clicked() {
            pending.push(Operation::RemoveEffect {
                clip: clip.id,
                effect: effect.id,
            });
        }
    });
    if let Some(reason) = color_node_inactive_reason(effect) {
        ui.colored_label(
            color::TEXT_MUTED,
            format!("Inactive for this frame: {}", reason.as_str()),
        );
    }
}

/// The stored `bypass` token of a colour node.
fn bypass_token(effect: &Effect) -> i64 {
    effect
        .parameters
        .get(COLOR_NODE_BYPASS_PARAMETER)
        .and_then(|value| match value {
            ParamValue::Integer(value) => Some(*value),
            ParamValue::Boolean(_) | ParamValue::Text(_) => None,
        })
        .unwrap_or(0)
}

/// The keyframe indicator rows of a colour-node card (CC3 §7).
///
/// A keyframed control is badged, is clearable in one click, and carries the
/// note that direct editing writes the static value rather than a keyframe.
fn color_node_keyframe_rows(
    ui: &mut egui::Ui,
    clip: ClipId,
    effect: &Effect,
    names: &[&'static str],
    pending: &mut InspectorEdits,
) {
    let keyframed: Vec<&&str> = names
        .iter()
        .filter(|name| parameter_is_keyframed(effect, name))
        .collect();
    if keyframed.is_empty() {
        return;
    }
    ui.colored_label(color::STATUS_WARNING, KEYFRAME_ROWS_NOTE);
    for name in keyframed {
        keyframe_row(ui, clip, effect, name, pending);
    }
}

const KEYFRAME_ROWS_NOTE: &str =
    "Automation drives these controls. Editing here writes the static value, not a keyframe.";

/// One keyframed control's badge and its one-click clear.
fn keyframe_row(
    ui: &mut egui::Ui,
    clip: ClipId,
    effect: &Effect,
    name: &str,
    pending: &mut InspectorEdits,
) {
    ui.horizontal(|ui| {
        ui.monospace(egui::RichText::new(name).size(type_size::CAPTION));
        ui.colored_label(color::STATUS_WARNING, "KEYFRAMED");
        if ui
            .small_button("Clear keyframes")
            .on_hover_text("Remove this parameter's automation so the edited value applies.")
            .clicked()
        {
            pending.push(clear_keyframes_operation(clip, effect.id, name));
        }
    });
}

/// True when automation, not the static parameter value, drives the render.
/// The inspector badges these parameters so a slider that appears inert is
/// explained instead of looking broken.
fn parameter_is_keyframed(effect: &Effect, parameter: &str) -> bool {
    effect.keyframes.contains_key(parameter)
}

fn clear_keyframes_operation(clip: ClipId, effect: EffectId, name: &str) -> Operation {
    Operation::ClearEffectKeyframes {
        clip,
        effect,
        name: name.to_owned(),
    }
}

fn primary_parameter_label(name: &str) -> &str {
    match name {
        "exposure_milli_stops" => "Exposure",
        "temperature_percent" => "Temperature",
        "tint_percent" => "Tint",
        "contrast_percent" => "Contrast",
        "contrast_pivot_basis_points" => "Pivot",
        "blacks_percent" => "Blacks",
        "shadows_percent" => "Shadows",
        "highlights_percent" => "Highlights",
        "whites_percent" => "Whites",
        "saturation_percent" => "Saturation",
        _ => name,
    }
}

#[allow(clippy::cast_precision_loss)]
fn primary_parameter_readout(name: &str, value: i64) -> String {
    match name {
        "exposure_milli_stops" => format!("{:+.3} stops", value as f64 / 1_000.0),
        "contrast_pivot_basis_points" => format!("{:.4}", value as f64 / 10_000.0),
        _ => format!("{value:+}%"),
    }
}

fn effect_display_name(name: &str) -> &str {
    match name {
        "primary_correction" => "Primary correction",
        "color_wheels" => "Colour wheels",
        "color_curves" => "Colour curves",
        _ => name,
    }
}

fn is_effect_insertable(name: &str) -> bool {
    !is_audio_effect(name)
        && !is_legacy_display_effect(name)
        && !matches!(name, "color_grade" | "cube_lut")
}

/// Keep internal, high-precision reframe storage out of the generic inspector
/// when the matching percent control is available. The basis-point parameters
/// remain in the core descriptor for agent-authored edits and rendering.
fn should_render_effect_parameter(
    descriptor: &kinewright_core::EffectDescriptor,
    parameter_name: &str,
) -> bool {
    // CC3 §7: the wheels and curves nodes own dedicated cards. Their 13 and 133
    // integers must never reach the generic slider loop, and `AddEffect` must
    // insert them with no parameters at all, because an omitted parameter
    // resolves to its neutral (CC3 §2.4).
    if matches!(descriptor.name, "color_wheels" | "color_curves") {
        return false;
    }
    let Some(legacy_name) = (match (descriptor.name, parameter_name) {
        ("reframe", "focus_x_basis_points") => Some("focus_x_percent"),
        ("reframe", "focus_y_basis_points") => Some("focus_y_percent"),
        _ => None,
    }) else {
        return true;
    };

    !descriptor
        .parameters
        .iter()
        .any(|parameter| parameter.name == legacy_name)
}

fn transition_section(
    ui: &mut egui::Ui,
    document: &kinewright_core::Document,
    clip: &Clip,
    pending: &mut InspectorEdits,
) {
    ui.add_space(space::TWO);
    ui.strong("Transition in");
    if let Some(transition) = &clip.transition_in {
        let maximum = document
            .clip_duration(clip)
            .map_or(1, |value| value.0.max(1));
        let mut name = transition.name.clone();
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Type");
            egui::ComboBox::from_id_salt(("transition-type", clip.id.0))
                .selected_text(&name)
                .show_ui(ui, |ui| {
                    for descriptor in TRANSITION_DESCRIPTORS {
                        changed |= ui
                            .selectable_value(
                                &mut name,
                                descriptor.name.to_owned(),
                                descriptor.name,
                            )
                            .on_hover_text(descriptor.description)
                            .changed();
                    }
                });
        });
        let mut duration = transition.duration.0;
        changed |= ui
            .add(
                egui::Slider::new(&mut duration, 1..=maximum)
                    .text("frames")
                    .integer(),
            )
            .changed();
        if changed {
            pending.extend(transition_duration_operations(
                document, clip.id, &name, duration,
            ));
        }
        if ui.small_button("Remove transition").clicked() {
            pending.extend(linked_transition_operations(document, clip.id, None));
        }
    } else {
        let duration = document
            .clip_duration(clip)
            .map_or(1, |value| value.0.clamp(1, 15));
        ui.menu_button("+ Transition", |ui| {
            for descriptor in TRANSITION_DESCRIPTORS {
                if ui
                    .button(descriptor.name)
                    .on_hover_text(descriptor.description)
                    .clicked()
                {
                    let transition = Transition {
                        name: descriptor.name.to_owned(),
                        duration: TimeCode(duration),
                    };
                    pending.extend(linked_transition_operations(
                        document,
                        clip.id,
                        Some(&transition),
                    ));
                    ui.close();
                }
            }
        });
    }
}

fn title_param_operation(clip: ClipId, name: &str, value: ParamValue) -> Operation {
    Operation::SetTitleParam {
        clip,
        name: name.to_owned(),
        value,
    }
}

fn audio_target_clip(document: &kinewright_core::Document, selected: ClipId) -> Option<Clip> {
    let mut members = linked_members(document, selected);
    members.sort_by_key(|(_, clip)| clip.id != selected);
    members
        .into_iter()
        .map(|(_, clip)| clip)
        .find(|clip| clip_carries_audio(document, clip))
}

fn clip_carries_audio(document: &kinewright_core::Document, clip: &Clip) -> bool {
    clip.content.is_media()
        && document
            .asset(clip.asset)
            .is_some_and(|asset| matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo))
}

const fn clip_audio_operation(
    clip: ClipId,
    gain_tenth_db: i32,
    fade_in_frames: i64,
    fade_out_frames: i64,
) -> Operation {
    Operation::SetClipAudio {
        clip,
        gain_tenth_db,
        fade_in_frames: TimeCode(fade_in_frames),
        fade_out_frames: TimeCode(fade_out_frames),
    }
}

fn tenth_db_to_db(value: i32) -> f64 {
    f64::from(value) / 10.0
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
fn db_to_tenth_db(value: f64) -> i32 {
    (value * 10.0).round() as i32
}

fn marker_param_operation(marker: MarkerId, name: &str, value: ParamValue) -> Operation {
    Operation::SetMarkerParam {
        marker,
        name: name.to_owned(),
        value,
    }
}

fn effect_param_operation(clip: ClipId, effect: EffectId, name: &str, value: i64) -> Operation {
    Operation::SetEffectParam {
        clip,
        effect,
        name: name.to_owned(),
        value: ParamValue::Integer(value),
    }
}

fn add_effect_operation(clip: &Clip, descriptor: &kinewright_core::EffectDescriptor) -> Operation {
    let id = clip
        .effects
        .iter()
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let parameters = descriptor
        .parameters
        .iter()
        .filter(|parameter| should_render_effect_parameter(descriptor, parameter.name))
        .map(|parameter| {
            (
                parameter.name.to_owned(),
                ParamValue::Integer(parameter.neutral),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Operation::AddEffect {
        clip: clip.id,
        effect: Effect {
            id: EffectId(id),
            name: descriptor.name.to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        },
    }
}

fn transition_duration_operations(
    document: &kinewright_core::Document,
    clip: ClipId,
    name: &str,
    duration: i64,
) -> Vec<Operation> {
    let transition = Transition {
        name: name.to_owned(),
        duration: TimeCode(duration),
    };
    linked_transition_operations(document, clip, Some(&transition))
}

fn data_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, label);
        ui.monospace(value);
    });
}

#[allow(clippy::cast_precision_loss)]
fn frame_readout(frame: TimeCode, fps: kinewright_core::Rational) -> String {
    let seconds = frame.0 as f64 * f64::from(fps.denominator()) / f64::from(fps.numerator());
    format!("{}f · {seconds:.3}s", frame.0)
}

fn range_readout(range: &std::ops::Range<TimeCode>, fps: kinewright_core::Rational) -> String {
    format!(
        "{} → {}",
        frame_readout(range.start, fps),
        frame_readout(range.end, fps)
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{
        AssetId, AutomationCurve, Document, EffectDescriptor, EffectParameterDescriptor,
        EffectUniform, Keyframe, KeyframeInterpolation, LinkId, MediaAsset, Rational, Track,
        TrackId, TrackKind,
    };

    use super::*;
    use crate::{color_wheel_widget::ColorWheelResponse, curve_editor_widget::CurveEditorResponse};

    #[test]
    fn inspector_control_builders_emit_only_operations() {
        assert_eq!(
            title_param_operation(ClipId(3), "text", ParamValue::Text("New".to_owned())),
            Operation::SetTitleParam {
                clip: ClipId(3),
                name: "text".to_owned(),
                value: ParamValue::Text("New".to_owned()),
            }
        );
        assert_eq!(
            marker_param_operation(MarkerId(4), "position", ParamValue::Integer(90)),
            Operation::SetMarkerParam {
                marker: MarkerId(4),
                name: "position".to_owned(),
                value: ParamValue::Integer(90),
            }
        );
        let mut document = Document::default();
        document.tracks.push(Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(3),
                asset: AssetId(1),
                source_range: TimeCode(0)..TimeCode(30),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: Some(Transition {
                    name: "crossfade".to_owned(),
                    duration: TimeCode(6),
                }),
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });
        assert_eq!(
            transition_duration_operations(&document, ClipId(3), "fade_from_black", 12),
            vec![
                Operation::RemoveTransition { clip: ClipId(3) },
                Operation::AddTransition {
                    clip: ClipId(3),
                    transition: Transition {
                        name: "fade_from_black".to_owned(),
                        duration: TimeCode(12),
                    },
                },
            ]
        );
    }

    #[test]
    fn descriptor_driven_add_effect_uses_neutral_integer_values() {
        static PARAMETERS: &[EffectParameterDescriptor] = &[EffectParameterDescriptor {
            name: "percent",
            min: -100,
            max: 100,
            neutral: 0,
            uniform: EffectUniform::Brightness,
        }];
        let descriptor = EffectDescriptor {
            name: "brightness",
            parameters: PARAMETERS,
        };
        let clip = Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: vec![Effect {
                id: EffectId(8),
                name: "contrast".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            }],
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        assert_eq!(
            add_effect_operation(&clip, &descriptor),
            Operation::AddEffect {
                clip: ClipId(1),
                effect: Effect {
                    id: EffectId(9),
                    name: "brightness".to_owned(),
                    parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(0),)]),
                    keyframes: BTreeMap::new(),
                },
            }
        );
    }

    #[test]
    fn primary_correction_card_uses_contract_defaults_and_reset_batch() {
        let descriptor = EFFECT_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.name == "primary_correction")
            .expect("CC1 descriptor");
        let clip = Clip {
            id: ClipId(3),
            asset: AssetId(1),
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };

        let Operation::AddEffect { effect, .. } = add_effect_operation(&clip, descriptor) else {
            panic!("primary correction must emit AddEffect");
        };
        assert_eq!(effect.parameters.len(), descriptor.parameters.len());
        assert!(effect.parameters.iter().all(|(name, value)| value
            == &ParamValue::Integer(descriptor.parameter(name).unwrap().neutral)));

        let reset_effect = Effect {
            id: EffectId(8),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([(
                "shadows_percent".to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 40,
                        interpolation: KeyframeInterpolation::Linear,
                    }],
                },
            )]),
        };
        let reset = color_node_reset_operations(clip.id, &reset_effect, descriptor);
        assert_eq!(reset.len(), descriptor.parameters.len() + 1);
        let reset_values = reset
            .iter()
            .filter(|operation| matches!(operation, Operation::SetEffectParam { .. }));
        for (operation, parameter) in reset_values.zip(descriptor.parameters) {
            assert_eq!(
                operation,
                &Operation::SetEffectParam {
                    clip: clip.id,
                    effect: EffectId(8),
                    name: parameter.name.to_owned(),
                    value: ParamValue::Integer(parameter.neutral),
                }
            );
        }
        assert!(reset.contains(&Operation::ClearEffectKeyframes {
            clip: clip.id,
            effect: EffectId(8),
            name: "shadows_percent".to_owned(),
        }));
    }

    #[test]
    fn live_slider_frames_coalesce_while_discrete_edits_stay_separate() {
        let key = primary_coalesce_key(ClipId(3), EffectId(8), "exposure_milli_stops");
        assert_eq!(key, "primary:3:8:exposure_milli_stops");

        let mut edits = InspectorEdits::default();
        edits.begin_gesture();
        for value in [10, 20, 30] {
            edits.push_live(
                effect_param_operation(ClipId(3), EffectId(8), "exposure_milli_stops", value),
                key.clone(),
            );
        }
        assert_eq!(edits.operations().len(), 3);
        assert_eq!(edits.coalesce_key(), Some(key.as_str()));
        assert!(edits.gesture_started);

        // A discrete edit in the same frame is never folded into a drag.
        edits.push(clear_keyframes_operation(
            ClipId(3),
            EffectId(8),
            "exposure_milli_stops",
        ));
        assert_eq!(edits.coalesce_key(), None);
        assert_eq!(edits.operations().len(), 4);

        // A frame with no drag stays an ordinary batch.
        let mut typed = InspectorEdits::default();
        typed.push(effect_param_operation(
            ClipId(3),
            EffectId(8),
            "exposure_milli_stops",
            40,
        ));
        assert_eq!(typed.coalesce_key(), None);
        assert!(!typed.gesture_started);
    }

    /// egui reports the release frame of a drag as `changed() == true` with
    /// `dragged() == false`. Gating coalescing on `dragged()` alone therefore
    /// files the final value of every drag as a second undo entry.
    #[test]
    fn the_release_frame_of_a_drag_keeps_the_gesture_key() {
        let key = primary_coalesce_key(ClipId(3), EffectId(8), "exposure_milli_stops");

        // Frames 1..n of the drag, then the release frame, all share one key.
        let mut edits = InspectorEdits::default();
        edits.begin_gesture();
        for value in [10, 20] {
            edits.push_live(
                effect_param_operation(ClipId(3), EffectId(8), "exposure_milli_stops", value),
                key.clone(),
            );
        }
        edits.push_live(
            effect_param_operation(ClipId(3), EffectId(8), "exposure_milli_stops", 30),
            key.clone(),
        );
        assert_eq!(edits.operations().len(), 3);
        assert_eq!(
            edits.coalesce_key(),
            Some(key.as_str()),
            "the release frame must not open a second undo entry"
        );
    }

    /// The Speed and Audio-Gain sliders coalesce like the primary controls, on
    /// their own keys so two different controls never merge.
    #[test]
    fn speed_and_audio_gain_drags_coalesce_on_their_own_keys() {
        assert_eq!(speed_coalesce_key(ClipId(3)), "speed:3");
        assert_eq!(audio_gain_coalesce_key(ClipId(7)), "audio_gain:7");
        assert_ne!(
            speed_coalesce_key(ClipId(3)),
            audio_gain_coalesce_key(ClipId(3)),
            "two controls on one clip must not merge into one undo entry"
        );
        assert_ne!(
            speed_coalesce_key(ClipId(3)),
            primary_coalesce_key(ClipId(3), EffectId(8), "exposure_milli_stops")
        );

        // A speed change is several operations per frame; they still form one
        // coalesced batch.
        let mut edits = InspectorEdits::default();
        edits.begin_gesture();
        edits.extend_live(
            [
                Operation::SetClipSpeed {
                    clip: ClipId(3),
                    speed_percent: 200,
                },
                Operation::SetClipSpeed {
                    clip: ClipId(4),
                    speed_percent: 200,
                },
            ],
            speed_coalesce_key(ClipId(3)),
        );
        assert_eq!(edits.operations().len(), 2);
        assert_eq!(edits.coalesce_key(), Some("speed:3"));

        // Yielding no operation leaves no key behind to attach to a later edit.
        let mut empty = InspectorEdits::default();
        empty.extend_live(Vec::new(), speed_coalesce_key(ClipId(3)));
        assert_eq!(empty.coalesce_key(), None);
        assert!(empty.operations().is_empty());

        let mut gain = InspectorEdits::default();
        gain.push_live(
            clip_audio_operation(ClipId(7), -120, 0, 0),
            audio_gain_coalesce_key(ClipId(7)),
        );
        assert_eq!(gain.coalesce_key(), Some("audio_gain:7"));
    }

    #[test]
    fn keyframed_primary_parameters_are_badged_and_clearable() {
        let effect = Effect {
            id: EffectId(8),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 250,
                        interpolation: KeyframeInterpolation::Linear,
                    }],
                },
            )]),
        };

        assert!(parameter_is_keyframed(&effect, "exposure_milli_stops"));
        assert!(!parameter_is_keyframed(&effect, "saturation_percent"));
        assert_eq!(
            clear_keyframes_operation(ClipId(3), effect.id, "exposure_milli_stops"),
            Operation::ClearEffectKeyframes {
                clip: ClipId(3),
                effect: EffectId(8),
                name: "exposure_milli_stops".to_owned(),
            }
        );
    }

    #[test]
    fn primary_correction_readouts_use_human_units() {
        assert_eq!(primary_parameter_label("exposure_milli_stops"), "Exposure");
        assert_eq!(
            primary_parameter_label("contrast_pivot_basis_points"),
            "Pivot"
        );
        assert_eq!(primary_parameter_label("blacks_percent"), "Blacks");
        assert_eq!(primary_parameter_label("saturation_percent"), "Saturation");
        assert_eq!(
            primary_parameter_readout("exposure_milli_stops", 1_250),
            "+1.250 stops"
        );
        assert_eq!(
            primary_parameter_readout("contrast_pivot_basis_points", 5_000),
            "0.5000"
        );
        assert_eq!(primary_parameter_readout("whites_percent", -25), "-25%");
    }

    #[test]
    fn legacy_display_effects_are_visible_but_not_offered_for_new_insertion() {
        for name in ["brightness", "contrast", "saturation"] {
            assert!(!is_effect_insertable(name));
            assert!(is_legacy_display_effect(name));
        }
        assert!(is_effect_insertable("primary_correction"));
        assert!(is_effect_insertable("look_lut"));
        assert!(!is_effect_insertable("color_grade"));
        assert!(!is_effect_insertable("cube_lut"));
        for name in ["look_lut", "cube_lut"] {
            assert_eq!(
                effect_compatibility_stage(name)
                    .expect("LUT compatibility stage")
                    .issue_code(),
                "legacy_lut_stage"
            );
        }
    }

    #[test]
    fn inspector_hides_reframe_basis_points_only_when_percent_control_exists() {
        const BASIS_ONLY_PARAMETERS: &[EffectParameterDescriptor] = &[EffectParameterDescriptor {
            name: "focus_x_basis_points",
            min: 0,
            max: 10_000,
            neutral: 5_000,
            uniform: EffectUniform::ReframeFocusX,
        }];
        let descriptor = EFFECT_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.name == "reframe")
            .expect("reframe descriptor");

        assert!(should_render_effect_parameter(
            descriptor,
            "focus_x_percent"
        ));
        assert!(!should_render_effect_parameter(
            descriptor,
            "focus_x_basis_points"
        ));
        assert!(!should_render_effect_parameter(
            descriptor,
            "focus_y_basis_points"
        ));
        assert!(should_render_effect_parameter(
            descriptor,
            "target_aspect_basis_points"
        ));

        let basis_only_descriptor = EffectDescriptor {
            name: "reframe",
            parameters: BASIS_ONLY_PARAMETERS,
        };
        assert!(should_render_effect_parameter(
            &basis_only_descriptor,
            "focus_x_basis_points"
        ));

        let clip = Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        let Operation::AddEffect { effect, .. } = add_effect_operation(&clip, descriptor) else {
            panic!("expected add effect operation");
        };
        assert!(!effect.parameters.contains_key("focus_x_basis_points"));
        assert!(!effect.parameters.contains_key("focus_y_basis_points"));
    }

    #[test]
    fn crop_neutral_add_and_shared_freeze_controls_emit_media_style_ops() {
        let descriptor = EFFECT_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.name == "crop")
            .unwrap();
        let media = Clip {
            id: ClipId(1),
            asset: AssetId(1),
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        let mut freeze = media.clone();
        freeze.content = ClipContent::Freeze(kinewright_core::FreezeFrame {
            source_frame: TimeCode(12),
        });

        let media_effect = add_effect_operation(&media, descriptor);
        let freeze_effect = add_effect_operation(&freeze, descriptor);
        assert_eq!(media_effect, freeze_effect);
        let Operation::AddEffect { effect, .. } = freeze_effect else {
            panic!("crop control must emit AddEffect");
        };
        assert_eq!(
            effect.parameters,
            BTreeMap::from([
                ("bottom_percent".to_owned(), ParamValue::Integer(0)),
                ("left_percent".to_owned(), ParamValue::Integer(0)),
                ("right_percent".to_owned(), ParamValue::Integer(0)),
                ("top_percent".to_owned(), ParamValue::Integer(0)),
            ])
        );

        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![freeze],
            }],
            color_context: kinewright_core::ColorContext::default(),
            ..Document::default()
        };
        assert_eq!(
            transition_duration_operations(&document, ClipId(1), "crossfade", 6),
            vec![Operation::AddTransition {
                clip: ClipId(1),
                transition: Transition {
                    name: "crossfade".to_owned(),
                    duration: TimeCode(6),
                },
            }]
        );
    }

    #[test]
    fn audio_controls_route_to_the_linked_audio_member() {
        let link = Some(LinkId(7));
        let document = Document {
            media_pool: vec![
                MediaAsset {
                    id: AssetId(1),
                    path: PathBuf::from("picture.mov"),
                    name: "Picture".to_owned(),
                    duration: TimeCode(30),
                    fps: Rational::new(30, 1).expect("valid fps"),
                    kind: MediaKind::Video,
                    resolution: Some((1920, 1080)),
                    source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                    color_description: kinewright_core::ColorDescription::default(),
                },
                MediaAsset {
                    id: AssetId(2),
                    path: PathBuf::from("sound.wav"),
                    name: "Sound".to_owned(),
                    duration: TimeCode(30),
                    fps: Rational::new(30, 1).expect("valid fps"),
                    kind: MediaKind::Audio,
                    resolution: None,
                    source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                    color_description: kinewright_core::ColorDescription::default(),
                },
            ],
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![media_clip(ClipId(10), AssetId(1), link)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![media_clip(ClipId(11), AssetId(2), link)],
                },
            ],
            color_context: kinewright_core::ColorContext::default(),
            ..Document::default()
        };

        let target = audio_target_clip(&document, ClipId(10)).expect("linked audio target");
        assert_eq!(target.id, ClipId(11));
        assert_eq!(
            clip_audio_operation(target.id, -60, 12, 4),
            Operation::SetClipAudio {
                clip: ClipId(11),
                gain_tenth_db: -60,
                fade_in_frames: TimeCode(12),
                fade_out_frames: TimeCode(4),
            }
        );
        assert_eq!(
            audio_target_clip(&document, ClipId(11)).map(|clip| clip.id),
            Some(ClipId(11))
        );
    }

    #[test]
    fn gain_slider_boundaries_round_trip_through_tenth_decibels() {
        for value in [-600, 120] {
            assert_eq!(db_to_tenth_db(tenth_db_to_db(value)), value);
        }
    }

    /// CC3 §7 makes both nodes first-class inserts, and CC3 §2.4 makes an
    /// omitted parameter resolve to its neutral: a fresh node therefore carries
    /// no parameters at all rather than 13 or 133 redundant neutrals.
    #[test]
    fn colour_nodes_insert_with_no_parameters_and_legacy_effects_stay_excluded() {
        assert!(is_effect_insertable("color_wheels"));
        assert!(is_effect_insertable("color_curves"));
        assert!(!is_effect_insertable("color_grade"));
        assert!(!is_effect_insertable("cube_lut"));
        for name in ["brightness", "contrast", "saturation"] {
            assert!(!is_effect_insertable(name));
        }

        let clip = media_clip(ClipId(10), AssetId(1), None);
        for (name, parameter_count) in [("color_wheels", 13), ("color_curves", 133)] {
            let descriptor = kinewright_core::effect_descriptor(name).expect("CC3 descriptor");
            assert_eq!(descriptor.parameters.len(), parameter_count);
            let Operation::AddEffect { effect, .. } = add_effect_operation(&clip, &descriptor)
            else {
                panic!("{name} must emit AddEffect");
            };
            assert_eq!(effect.name, name);
            assert!(
                effect.parameters.is_empty(),
                "{name} must insert at neutral by omission"
            );
            // The raw integers never reach the generic slider loop.
            for parameter in descriptor.parameters {
                assert!(!should_render_effect_parameter(&descriptor, parameter.name));
            }
        }
    }

    /// CC3 §5: a node reset is one `SetEffectParam` per descriptor parameter at
    /// its neutral plus a `ClearEffectKeyframes` for each automated one, in one
    /// batch and therefore one undo entry. `bypass` is an ordinary parameter and
    /// resets to `0` with everything else.
    #[test]
    fn a_wheels_reset_writes_thirteen_neutrals_and_clears_automation() {
        let descriptor = kinewright_core::effect_descriptor("color_wheels").expect("descriptor");
        let effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        let reset = color_node_reset_operations(ClipId(3), &effect, &descriptor);

        let sets: Vec<&Operation> = reset
            .iter()
            .filter(|operation| matches!(operation, Operation::SetEffectParam { .. }))
            .collect();
        assert_eq!(sets.len(), 13);
        assert_eq!(reset.len(), 14);
        for (operation, parameter) in sets.iter().zip(descriptor.parameters) {
            assert_eq!(
                **operation,
                Operation::SetEffectParam {
                    clip: ClipId(3),
                    effect: EffectId(8),
                    name: parameter.name.to_owned(),
                    value: ParamValue::Integer(parameter.neutral),
                }
            );
        }
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            value: ParamValue::Integer(0),
        }));
        assert!(reset.contains(&Operation::ClearEffectKeyframes {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "gain_red_thousandths".to_owned(),
        }));
    }

    /// Double-clicking one ball resets only that wheel's four controls.
    #[test]
    fn a_wheel_reset_touches_only_its_own_four_controls() {
        let effect = keyframed_effect("color_wheels", "lift_red_basis_points", 900);
        let reset = wheel_reset_operations(ClipId(3), &effect, ColorWheelControl::Lift);
        assert_eq!(reset.len(), 5);
        for channel in ColorWheelChannel::ALL {
            assert!(reset.contains(&Operation::SetEffectParam {
                clip: ClipId(3),
                effect: EffectId(8),
                name: ColorWheelControl::Lift.parameter_name(channel).to_owned(),
                value: ParamValue::Integer(0),
            }));
        }
        assert!(reset.contains(&Operation::ClearEffectKeyframes {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "lift_red_basis_points".to_owned(),
        }));
        assert!(
            wheel_reset_operations(ClipId(3), &effect, ColorWheelControl::Gain)
                .iter()
                .all(|operation| !format!("{operation:?}").contains("lift_"))
        );
    }

    /// CC3 §5: a curve reset is the node reset restricted to one curve's 33
    /// parameters.
    #[test]
    fn a_curve_reset_covers_exactly_its_thirty_three_parameters() {
        let effect = keyframed_effect("color_curves", "master_y1", 8_000);
        let reset = curve_reset_operations(ClipId(3), &effect, ColorCurveChannel::Master);
        let sets: Vec<&Operation> = reset
            .iter()
            .filter(|operation| matches!(operation, Operation::SetEffectParam { .. }))
            .collect();
        assert_eq!(sets.len(), 33);
        assert_eq!(reset.len(), 34);
        for operation in &reset {
            let name = match operation {
                Operation::SetEffectParam { name, .. }
                | Operation::ClearEffectKeyframes { name, .. } => name.clone(),
                other => panic!("unexpected reset operation {other:?}"),
            };
            assert!(
                name.starts_with("master_"),
                "{name} is not a master control"
            );
        }
        // The neutrals are the structural identity: (0, 0) and (10000, 10000).
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "master_x0".to_owned(),
            value: ParamValue::Integer(0),
        }));
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "master_x1".to_owned(),
            value: ParamValue::Integer(10_000),
        }));
        assert!(reset.contains(&Operation::SetEffectParam {
            clip: ClipId(3),
            effect: EffectId(8),
            name: "master_point_count".to_owned(),
            value: ParamValue::Integer(2),
        }));
    }

    /// CC3 §2.4: an edit writes `{curve}_point_count` and the coordinates of the
    /// active points only. Nothing at index `>= point_count` is touched.
    #[test]
    fn a_curve_edit_writes_point_count_and_only_the_active_points() {
        let operations = curve_edit_operations(
            ClipId(10),
            EffectId(4),
            ColorCurveChannel::Master,
            &[(0, 0), (10_000, 10_000)],
            &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
        );
        let written: Vec<(String, i64)> = operations
            .iter()
            .map(|operation| match operation {
                Operation::SetEffectParam { name, value, .. } => {
                    let ParamValue::Integer(value) = value else {
                        panic!("curves are integer-only");
                    };
                    (name.clone(), *value)
                }
                other => panic!("unexpected curve operation {other:?}"),
            })
            .collect();
        assert_eq!(
            written,
            vec![
                ("master_x0".to_owned(), 0),
                ("master_y0".to_owned(), 0),
                ("master_x1".to_owned(), 5_000),
                ("master_y1".to_owned(), 6_000),
                ("master_x2".to_owned(), 10_000),
                ("master_y2".to_owned(), 10_000),
                ("master_point_count".to_owned(), 3),
            ]
        );
        for index in 3..16 {
            let inactive = format!("master_x{index}");
            assert!(
                written.iter().all(|(name, _)| name != &inactive),
                "{inactive} must stay omitted so its neutral resolves"
            );
        }
    }

    /// Core validates every `SetEffectParam` against the document the change
    /// would produce, so a batch whose *intermediate* state has a non-increasing
    /// `x` is rejected even when its start and end states are legal. Every edit
    /// the widget can produce must therefore apply cleanly.
    #[test]
    fn every_curve_edit_batch_is_accepted_by_core_in_order() {
        let mut document = curves_document();
        let mut points = vec![(0, 0), (10_000, 10_000)];
        for next in [
            // Add a point: later coordinates move left, so writes run ascending.
            vec![(0, 0), (5_000, 6_000), (10_000, 10_000)],
            // Drag it left, then right, inside its neighbours.
            vec![(0, 0), (2_000, 6_000), (10_000, 10_000)],
            vec![(0, 0), (7_000, 6_000), (10_000, 10_000)],
            vec![(0, 0), (7_000, 6_000), (8_000, 9_000), (10_000, 10_000)],
            // Remove one: later coordinates move right, so the count shrinks
            // first and the writes run descending.
            vec![(0, 0), (8_000, 9_000), (10_000, 10_000)],
            // A mixed edit moves points in both directions at once.
            vec![(0, 0), (3_000, 1_000), (12_000, 12_000)],
            vec![(-2_000, -2_000), (10_000, 10_000)],
        ] {
            let operations = curve_edit_operations(
                ClipId(10),
                EffectId(4),
                ColorCurveChannel::Master,
                &points,
                &next,
            );
            kinewright_core::apply_batch(&mut document, &operations)
                .unwrap_or_else(|error| panic!("core rejected {next:?}: {error}"));
            let effect = &document.tracks[0].clips[0].effects[0];
            assert_eq!(ResolvedCurves::from_effect(effect).master.points, next);
            points = next;
        }
    }

    /// Stored states a user can reach through the curve editor, each legal on
    /// its own but hostile to a descriptor-ordered reset.
    fn adversarial_curve_states() -> Vec<(&'static str, Vec<(i32, i32)>)> {
        let mut sixteen = Vec::new();
        for index in 0..16 {
            let x = -2_000 + index * 500;
            sixteen.push((x, x));
        }
        vec![
            // The reported repro: both active points sit left of zero, so a
            // descriptor-ordered `master_x0 = 0` crosses `master_x1 = -1000`.
            ("negative pair", vec![(-2_000, -1_000), (-1_000, 500)]),
            // Both active points sit right of white, so `master_x1 = 10000`
            // would cross if it were written first instead.
            ("far right", vec![(9_000, 9_000), (11_000, 11_500)]),
            (
                "far right, three points",
                vec![(9_000, 0), (11_000, 5_000), (12_000, 12_000)],
            ),
            // One point must move right and the other left, so neither the
            // ascending nor the descending branch applies.
            ("reversed pair", vec![(-2_000, 4_000), (12_000, 1_000)]),
            ("sixteen points", sixteen),
        ]
    }

    /// Install a stored point list directly, bypassing the editor, so the reset
    /// is exercised against states the widget can reach but the batch builder
    /// has never seen produced.
    fn store_curve(document: &mut Document, curve: ColorCurveChannel, points: &[(i32, i32)]) {
        let effect = &mut document.tracks[0].clips[0].effects[0];
        effect.parameters.insert(
            curve.point_count_parameter().to_owned(),
            ParamValue::Integer(i64::try_from(points.len()).expect("a point count fits in i64")),
        );
        for (index, (x, y)) in points.iter().enumerate() {
            let x_name = curve.x_parameter(index).expect("an active x parameter");
            let y_name = curve.y_parameter(index).expect("an active y parameter");
            effect
                .parameters
                .insert(x_name.to_owned(), ParamValue::Integer(i64::from(*x)));
            effect
                .parameters
                .insert(y_name.to_owned(), ParamValue::Integer(i64::from(*y)));
        }
    }

    /// Core validates the strictly-increasing-`x` rule against every
    /// intermediate document, so descriptor order - `point_count`, `x0`, `y0`,
    /// `x1`, ... - is *not* a legal reset order: from a stored
    /// `x0 = -2000, x1 = -1000` the very first write crosses and `apply_batch`
    /// discards the whole reset without a visible failure.
    #[test]
    fn a_curve_reset_is_accepted_from_every_adversarial_stored_state() {
        for (label, stored) in adversarial_curve_states() {
            let mut document = curves_document();
            store_curve(&mut document, ColorCurveChannel::Master, &stored);
            document
                .validate()
                .unwrap_or_else(|error| panic!("{label} must be a legal stored state: {error}"));

            let effect = document.tracks[0].clips[0].effects[0].clone();
            let reset = curve_reset_operations(ClipId(10), &effect, ColorCurveChannel::Master);
            kinewright_core::apply_batch(&mut document, &reset)
                .unwrap_or_else(|error| panic!("core rejected the {label} reset: {error}"));

            let effect = &document.tracks[0].clips[0].effects[0];
            let resolved = ResolvedCurves::from_effect(effect);
            assert!(
                resolved.master.is_structural_identity(),
                "the {label} reset must restore (0, 0) and (10000, 10000)",
            );
            assert!(!resolved.master.truncated);
            // Every one of the curve's 33 parameters ends at its neutral,
            // including the inactive points 2..16.
            for parameter in ColorCurveChannel::Master.parameters() {
                assert_eq!(
                    effect.parameters.get(parameter.name),
                    Some(&ParamValue::Integer(parameter.neutral)),
                    "{} must end at its neutral after the {label} reset",
                    parameter.name,
                );
            }
        }
    }

    /// The regression the ordering fix exists for: writing the same neutrals in
    /// descriptor order is rejected, so a reset that did so would be silently
    /// discarded by `apply_batch`.
    #[test]
    fn a_descriptor_ordered_curve_reset_would_be_rejected() {
        let mut document = curves_document();
        store_curve(
            &mut document,
            ColorCurveChannel::Master,
            &[(-2_000, -1_000), (-1_000, 500)],
        );
        let descriptor_order: Vec<Operation> = ColorCurveChannel::Master
            .parameters()
            .iter()
            .map(|parameter| {
                effect_param_operation(ClipId(10), EffectId(4), parameter.name, parameter.neutral)
            })
            .collect();
        let error = kinewright_core::apply_batch(&mut document.clone(), &descriptor_order)
            .expect_err("descriptor order must cross x0 over x1");
        assert!(
            format!("{error}").contains("strictly increasing"),
            "unexpected rejection: {error}"
        );
    }

    /// The whole-node reset covers all four curves plus the node-owned
    /// `bypass`, and must be accepted from the same hostile stored states.
    #[test]
    fn a_whole_node_curves_reset_is_accepted_from_every_adversarial_stored_state() {
        let descriptor = kinewright_core::effect_descriptor("color_curves").expect("descriptor");
        for (label, stored) in adversarial_curve_states() {
            let mut document = curves_document();
            for curve in ColorCurveChannel::ALL {
                store_curve(&mut document, curve, &stored);
            }
            document.tracks[0].clips[0].effects[0].parameters.insert(
                COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                ParamValue::Integer(1),
            );
            document
                .validate()
                .unwrap_or_else(|error| panic!("{label} must be a legal stored state: {error}"));

            let effect = document.tracks[0].clips[0].effects[0].clone();
            let reset = color_node_reset_operations(ClipId(10), &effect, &descriptor);
            kinewright_core::apply_batch(&mut document, &reset)
                .unwrap_or_else(|error| panic!("core rejected the {label} node reset: {error}"));

            let effect = &document.tracks[0].clips[0].effects[0];
            let resolved = ResolvedCurves::from_effect(effect);
            assert!(resolved.is_neutral(), "the {label} node reset must be flat");
            assert!(
                !resolved.bypass(),
                "the {label} node reset must clear bypass"
            );
            for parameter in descriptor.parameters {
                assert_eq!(
                    effect.parameters.get(parameter.name),
                    Some(&ParamValue::Integer(parameter.neutral)),
                    "{} must end at its neutral after the {label} node reset",
                    parameter.name,
                );
            }
        }
    }

    /// The descending branch writes `{curve}_point_count` before the points, so
    /// it must never grow the active prefix: growing it would expose the
    /// colliding `(10000, 10000)` neutrals of the points still unwritten. This
    /// edit satisfies the coordinate half of the test - every shared point moves
    /// right - and would be rejected without the length guard.
    #[test]
    fn a_growing_curve_edit_never_takes_the_count_first_branch() {
        let mut document = curves_document();
        let current = [(0, 0), (11_000, 11_000)];
        let next = [(0, 0), (11_500, 11_500), (11_800, 11_800)];
        kinewright_core::apply_batch(
            &mut document,
            &curve_edit_operations(
                ClipId(10),
                EffectId(4),
                ColorCurveChannel::Master,
                &[(0, 0), (10_000, 10_000)],
                &current,
            ),
        )
        .expect("the setup edit must apply");

        let operations = curve_edit_operations(
            ClipId(10),
            EffectId(4),
            ColorCurveChannel::Master,
            &current,
            &next,
        );
        let first_count = operations
            .iter()
            .find_map(|operation| match operation {
                Operation::SetEffectParam { name, value, .. } if name == "master_point_count" => {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("a curve edit always writes its point count");
        assert_eq!(
            first_count,
            ParamValue::Integer(2),
            "a growing edit must shrink the prefix first, never grow it"
        );
        kinewright_core::apply_batch(&mut document, &operations)
            .expect("core must accept a growing edit past white");
        assert_eq!(
            ResolvedCurves::from_effect(&document.tracks[0].clips[0].effects[0])
                .master
                .points,
            next.to_vec()
        );
    }

    /// CC3 §7 requires an indicator per keyframed control, not per keyframed
    /// control of the selected curve. Automation on the red curve stays visible
    /// while the master curve is on screen.
    #[test]
    fn keyframe_badges_cover_every_curve_and_the_node_bypass() {
        let mut effect = keyframed_effect("color_curves", "master_y1", 8_000);
        for name in ["red_x1", "red_point_count", "blue_y0"] {
            effect.keyframes.insert(
                name.to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value: 2,
                        interpolation: KeyframeInterpolation::Hold,
                    }],
                },
            );
        }
        effect.keyframes.insert(
            COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode::ZERO,
                    value: 1,
                    interpolation: KeyframeInterpolation::Hold,
                }],
            },
        );
        // Every keyframed control appears exactly once, grouped under the curve
        // that owns it, in `ColorCurveChannel::ALL` order with the node-owned
        // `bypass` last. Green carries no automation and is omitted.
        assert_eq!(
            color_curve_keyframe_groups(&effect),
            vec![
                ("Master", vec!["master_y1"]),
                ("Red", vec!["red_point_count", "red_x1"]),
                ("Blue", vec!["blue_y0"]),
                ("Node", vec![COLOR_NODE_BYPASS_PARAMETER]),
            ]
        );
        assert!(
            color_curve_keyframe_groups(&Effect {
                keyframes: BTreeMap::new(),
                ..effect
            })
            .is_empty(),
            "a node with no automation shows no badges"
        );
    }

    /// CC3 §7: the card reports `curve_truncated_by_automation` even when the
    /// only automation is the node-owned `bypass`. `bypass` is not curve-owned,
    /// so a scan built from curve keyframes alone would only ever look at frame
    /// zero, where this node is still bypassed.
    #[test]
    fn a_keyframed_bypass_is_part_of_the_truncation_scan() {
        let mut effect = Effect {
            id: EffectId(4),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([
                // A whole-curve step to sixteen points whose coordinates are
                // omitted: every point past the first resolves to the colliding
                // `(10000, 10000)` neutral, so the curve truncates to two.
                (
                    "master_point_count".to_owned(),
                    AutomationCurve {
                        keyframes: vec![Keyframe {
                            at: TimeCode::ZERO,
                            value: 16,
                            interpolation: KeyframeInterpolation::Hold,
                        }],
                    },
                ),
            ]),
        };
        effect.keyframes.insert(
            COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 1,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 0,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                ],
            },
        );
        assert_eq!(
            automation_truncated_curves(&effect),
            vec![ColorCurveChannel::Master],
            "frame 10 releases the bypass and must be scanned"
        );
    }

    /// CC3 §7: a drag applies one batch per frame under a stable gesture key so
    /// the preview stays live and the whole gesture is one undo entry. A
    /// discrete edit never carries a key.
    #[test]
    fn wheel_and_curve_gestures_coalesce_under_their_own_keys() {
        assert_eq!(
            wheels_coalesce_key(ClipId(3), EffectId(8), ColorWheelControl::Lift),
            "wheels:3:8:lift"
        );
        assert_eq!(
            wheels_coalesce_key(ClipId(3), EffectId(8), ColorWheelControl::Gain),
            "wheels:3:8:gain"
        );
        assert_eq!(
            curves_coalesce_key(ClipId(3), EffectId(8), ColorCurveChannel::Red),
            "curves:3:8:red"
        );
        assert_ne!(
            wheels_coalesce_key(ClipId(3), EffectId(8), ColorWheelControl::Lift),
            wheels_coalesce_key(ClipId(3), EffectId(9), ColorWheelControl::Lift),
            "two nodes on one clip must never merge into one undo entry"
        );

        let effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        let mut edits = InspectorEdits::default();
        for (frame, value) in [1_100, 1_200, 1_300].into_iter().enumerate() {
            let response = ColorWheelResponse {
                changes: vec![(ColorWheelChannel::Red, value)],
                live: true,
                gesture_started: frame == 0,
                reset: false,
            };
            apply_wheel_response(
                ClipId(3),
                &effect,
                ColorWheelControl::Gain,
                &response,
                &mut edits,
            );
        }
        assert_eq!(edits.operations().len(), 3);
        assert_eq!(edits.coalesce_key(), Some("wheels:3:8:gain"));
        assert!(edits.gesture_started);

        // A double-click reset in the same frame drops the key: it is discrete.
        apply_wheel_response(
            ClipId(3),
            &effect,
            ColorWheelControl::Gain,
            &ColorWheelResponse {
                reset: true,
                ..ColorWheelResponse::default()
            },
            &mut edits,
        );
        assert_eq!(edits.coalesce_key(), None);

        // A click that adds a curve point is discrete; a point drag is live.
        let curves = keyframed_effect("color_curves", "master_y1", 8_000);
        let mut click = InspectorEdits::default();
        apply_curve_response(
            ClipId(3),
            &curves,
            ColorCurveChannel::Master,
            &[(0, 0), (10_000, 10_000)],
            &CurveEditorResponse {
                points: Some(vec![(0, 0), (5_000, 6_000), (10_000, 10_000)]),
                ..CurveEditorResponse::default()
            },
            &mut click,
        );
        assert_eq!(click.operations().len(), 7);
        assert_eq!(click.coalesce_key(), None);
        assert!(!click.gesture_started);

        let mut drag = InspectorEdits::default();
        for (frame, x) in [4_000, 4_500, 5_000].into_iter().enumerate() {
            apply_curve_response(
                ClipId(3),
                &curves,
                ColorCurveChannel::Master,
                &[(0, 0), (3_000, 6_000), (10_000, 10_000)],
                &CurveEditorResponse {
                    points: Some(vec![(0, 0), (x, 6_000), (10_000, 10_000)]),
                    live: true,
                    gesture_started: frame == 0,
                    reset: false,
                },
                &mut drag,
            );
        }
        assert_eq!(drag.coalesce_key(), Some("curves:3:8:master"));
        assert!(drag.gesture_started);
        assert_eq!(drag.operations().len(), 21);
    }

    /// CC3 §5: bypass is an ordinary integer parameter set with one
    /// `SetEffectParam`, never a UI-only flag and never a node removal.
    #[test]
    fn the_bypass_toggle_emits_exactly_one_set_effect_param() {
        let mut edits = InspectorEdits::default();
        edits.push(effect_param_operation(
            ClipId(3),
            EffectId(8),
            COLOR_NODE_BYPASS_PARAMETER,
            1,
        ));
        assert_eq!(edits.operations().len(), 1);
        assert_eq!(edits.coalesce_key(), None);
        assert_eq!(
            edits.operations()[0],
            Operation::SetEffectParam {
                clip: ClipId(3),
                effect: EffectId(8),
                name: "bypass".to_owned(),
                value: ParamValue::Integer(1),
            }
        );

        let mut effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        assert_eq!(bypass_token(&effect), 0, "an omitted bypass resolves to 0");
        effect
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        assert_eq!(bypass_token(&effect), 1);
        assert_eq!(
            color_node_inactive_reason(&effect),
            Some(kinewright_core::ColorNodeInactiveReason::Bypassed)
        );
    }

    /// CC3 §7: the card reports `curve_truncated_by_automation` when keyframe
    /// evaluation leaves a curve without strictly increasing `x` (CC3 §3.4).
    #[test]
    fn automation_that_crosses_points_is_reported_as_truncation() {
        let mut effect = Effect {
            id: EffectId(4),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::from([
                ("master_point_count".to_owned(), ParamValue::Integer(3)),
                ("master_x1".to_owned(), ParamValue::Integer(5_000)),
                ("master_y1".to_owned(), ParamValue::Integer(6_000)),
                ("master_x2".to_owned(), ParamValue::Integer(10_000)),
                ("master_y2".to_owned(), ParamValue::Integer(10_000)),
            ]),
            keyframes: BTreeMap::new(),
        };
        assert!(automation_truncated_curves(&effect).is_empty());

        effect.keyframes.insert(
            "master_x1".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 5_000,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(5),
                        value: 11_000,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        assert_eq!(
            automation_truncated_curves(&effect),
            vec![ColorCurveChannel::Master]
        );

        // A bypassed node is the exact identity, so it reports nothing.
        effect
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        assert!(automation_truncated_curves(&effect).is_empty());
    }

    /// The ball reads the stored integers, not a cached float, and badges the
    /// controls automation drives.
    #[test]
    fn a_wheel_reads_its_stored_integers_and_keyframe_state() {
        let mut effect = keyframed_effect("color_wheels", "gain_red_thousandths", 1_800);
        effect.parameters.insert(
            "gain_master_thousandths".to_owned(),
            ParamValue::Integer(1_400),
        );
        effect
            .parameters
            .insert("gain_red_thousandths".to_owned(), ParamValue::Integer(900));
        let params = ColorWheelsParams::from_effect(&effect);
        let state = wheel_state(&effect, params, ColorWheelControl::Gain);
        assert_eq!(state.values.master, 1_400);
        assert_eq!(state.values.red, 900);
        assert_eq!(state.values.green, 1_000);
        assert_eq!(state.keyframed, [false, true, false, false]);

        let lift = wheel_state(&effect, params, ColorWheelControl::Lift);
        assert_eq!(lift.values.master, 0);
        assert_eq!(lift.keyframed, [false; 4]);
    }

    fn keyframed_effect(name: &str, parameter: &str, value: i64) -> Effect {
        Effect {
            id: EffectId(8),
            name: name.to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::from([(
                parameter.to_owned(),
                AutomationCurve {
                    keyframes: vec![Keyframe {
                        at: TimeCode::ZERO,
                        value,
                        interpolation: KeyframeInterpolation::Hold,
                    }],
                },
            )]),
        }
    }

    /// A one-clip document carrying an all-neutral `color_curves` node.
    fn curves_document() -> Document {
        let mut clip = media_clip(ClipId(10), AssetId(1), None);
        clip.effects = vec![Effect {
            id: EffectId(4),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        }];
        Document {
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: PathBuf::from("picture.mov"),
                name: "Picture".to_owned(),
                duration: TimeCode(30),
                fps: Rational::new(30, 1).expect("valid fps"),
                kind: MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
            }],
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![clip],
            }],
            color_context: kinewright_core::ColorContext::default(),
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    fn media_clip(id: ClipId, asset: AssetId, link: Option<LinkId>) -> Clip {
        Clip {
            id,
            asset,
            source_range: TimeCode(0)..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }
}
