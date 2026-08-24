use std::collections::BTreeMap;

use eframe::egui;
use kinewright_core::{
    Clip, ClipContent, ClipId, EFFECT_DESCRIPTORS, Effect, EffectId, MARKER_COLOR_TOKEN_COUNT,
    Marker, MarkerId, MediaKind, Operation, ParamValue, TITLE_COLORS, TITLE_FONT_SIZES,
    TRANSITION_DESCRIPTORS, TimeCode, Title, TitlePosition, Transition, effect_compatibility_stage,
    is_audio_effect, is_legacy_display_effect,
};

use crate::{
    app::KinewrightApp,
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

/// Whether a slider change belongs to a drag gesture that is still one undo
/// entry.
///
/// egui reports the frame the pointer is released as `changed() == true` with
/// `dragged() == false`, so testing `dragged()` alone drops the final value out
/// of the gesture and files it as a second undo entry. `drag_stopped()` marks
/// exactly that release frame, and it carries the same coalesce key so the
/// whole drag — release included — stays one entry.
fn is_live_drag(slider: &egui::Response) -> bool {
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
    for effect in &clip.effects {
        if effect.name == "primary_correction" {
            primary_correction_section(ui, clip, effect, pending);
            continue;
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
                pending.extend(primary_reset_operations(clip.id, effect, descriptor));
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

fn primary_reset_operations(
    clip: ClipId,
    effect: &Effect,
    descriptor: &kinewright_core::EffectDescriptor,
) -> Vec<Operation> {
    let mut operations = Vec::with_capacity(descriptor.parameters.len() + effect.keyframes.len());
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
        let reset = primary_reset_operations(clip.id, &reset_effect, descriptor);
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
