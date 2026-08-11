use std::collections::BTreeMap;

use eframe::egui;
use openreel_core::{
    Clip, ClipContent, ClipId, EFFECT_DESCRIPTORS, Effect, EffectId, MARKER_COLOR_TOKEN_COUNT,
    Marker, MarkerId, Operation, ParamValue, TITLE_COLORS, TITLE_FONT_SIZES, TimeCode, Title,
    TitlePosition, Transition,
};

use crate::{
    app::OpenReelApp,
    theme::{color, space},
};

const INSPECTOR_MAX_HEIGHT: f32 = 360.0;

impl OpenReelApp {
    pub(crate) fn right_dock(&mut self, ui: &mut egui::Ui) {
        let id = ui.make_persistent_id("inspector-panel");
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
        if self.title_text_focus.is_some() {
            state.set_open(true);
        }
        state
            .show_header(ui, |ui| ui.strong("Inspector"))
            .body(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector-scroll")
                    .max_height(INSPECTOR_MAX_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| self.inspector(ui));
            });
        ui.add_space(space::ONE);
        ui.separator();
        ui.add_space(space::ONE);
        self.agent_panel(ui);
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        if let Some(clip) = self
            .selected_clip
            .and_then(|id| self.document.clip(id))
            .cloned()
        {
            match &clip.content {
                ClipContent::Media => self.media_clip_inspector(ui, &clip),
                ClipContent::Title(title) => self.title_inspector(ui, &clip, title),
            }
        } else if let Some(marker) = self
            .selected_marker
            .and_then(|id| self.document.marker(id))
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
        let Some(asset) = self.document.asset(clip.asset).cloned() else {
            ui.colored_label(color::STATUS_DANGER, "Media asset is missing");
            return;
        };
        ui.strong(&asset.name);
        ui.colored_label(color::TEXT_MUTED, format!("{:?}", asset.kind));
        ui.add_space(space::ONE);
        data_row(ui, "Path", &asset.path.display().to_string());
        data_row(ui, "Source", &range_readout(&clip.source_range, asset.fps));
        let timeline_end = self
            .document
            .clip_duration(clip)
            .map_or(clip.timeline_start, |duration| {
                TimeCode(clip.timeline_start.0.saturating_add(duration.0))
            });
        data_row(
            ui,
            "Timeline",
            &range_readout(&(clip.timeline_start..timeline_end), self.document.fps),
        );
        if let Some((width, height)) = asset.resolution {
            data_row(ui, "Raster", &format!("{width} × {height}"));
        }

        ui.add_space(space::TWO);
        ui.strong("Effects");
        let mut pending = Vec::new();
        for effect in &clip.effects {
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
            });
        }
        ui.menu_button("+ Effect", |ui| {
            for descriptor in EFFECT_DESCRIPTORS {
                if clip
                    .effects
                    .iter()
                    .any(|effect| effect.name == descriptor.name)
                {
                    continue;
                }
                if ui.button(descriptor.name).clicked() {
                    pending.push(add_effect_operation(clip, descriptor));
                    ui.close();
                }
            }
        });

        ui.add_space(space::TWO);
        ui.strong("Transition in");
        if let Some(transition) = &clip.transition_in {
            let maximum = self
                .document
                .clip_duration(clip)
                .map_or(1, |value| value.0.max(1));
            let mut duration = transition.duration.0;
            if ui
                .add(
                    egui::Slider::new(&mut duration, 1..=maximum)
                        .text("frames")
                        .integer(),
                )
                .changed()
            {
                pending.extend(transition_duration_operations(clip.id, duration));
            }
            if ui.small_button("Remove transition").clicked() {
                pending.push(Operation::RemoveTransition { clip: clip.id });
            }
        } else if ui.button("Add crossfade").clicked() {
            let duration = self
                .document
                .clip_duration(clip)
                .map_or(1, |value| value.0.clamp(1, 15));
            pending.push(Operation::AddTransition {
                clip: clip.id,
                transition: Transition {
                    name: "crossfade".to_owned(),
                    duration: TimeCode(duration),
                },
            });
        }
        self.send_operations(pending);
    }

    #[allow(clippy::too_many_lines)]
    fn title_inspector(&mut self, ui: &mut egui::Ui, clip: &Clip, title: &Title) {
        ui.strong("Title");
        let timeline_end = self
            .document
            .clip_duration(clip)
            .map_or(clip.timeline_start, |duration| {
                TimeCode(clip.timeline_start.0.saturating_add(duration.0))
            });
        data_row(
            ui,
            "Timeline",
            &range_readout(&(clip.timeline_start..timeline_end), self.document.fps),
        );
        let draft = self
            .title_text_draft
            .get_or_insert_with(|| (clip.id, title.text.clone()));
        if draft.0 != clip.id {
            *draft = (clip.id, title.text.clone());
        }
        let response = ui.add(
            egui::TextEdit::multiline(&mut draft.1)
                .desired_rows(2)
                .hint_text("Title text"),
        );
        if self.title_text_focus == Some(clip.id) {
            response.request_focus();
            self.title_text_focus = None;
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
            &frame_readout(marker.position, self.document.fps),
        );
        let draft = self
            .marker_label_draft
            .get_or_insert_with(|| (marker.id, marker.label.clone()));
        if draft.0 != marker.id {
            *draft = (marker.id, marker.label.clone());
        }
        let response = ui.text_edit_singleline(&mut draft.1);
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

fn title_param_operation(clip: ClipId, name: &str, value: ParamValue) -> Operation {
    Operation::SetTitleParam {
        clip,
        name: name.to_owned(),
        value,
    }
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

fn add_effect_operation(clip: &Clip, descriptor: &openreel_core::EffectDescriptor) -> Operation {
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
        },
    }
}

fn transition_duration_operations(clip: ClipId, duration: i64) -> Vec<Operation> {
    vec![
        Operation::RemoveTransition { clip },
        Operation::AddTransition {
            clip,
            transition: Transition {
                name: "crossfade".to_owned(),
                duration: TimeCode(duration),
            },
        },
    ]
}

fn data_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color::TEXT_MUTED, label);
        ui.monospace(value);
    });
}

#[allow(clippy::cast_precision_loss)]
fn frame_readout(frame: TimeCode, fps: openreel_core::Rational) -> String {
    let seconds = frame.0 as f64 * f64::from(fps.denominator()) / f64::from(fps.numerator());
    format!("{}f · {seconds:.3}s", frame.0)
}

fn range_readout(range: &std::ops::Range<TimeCode>, fps: openreel_core::Rational) -> String {
    format!(
        "{} → {}",
        frame_readout(range.start, fps),
        frame_readout(range.end, fps)
    )
}

#[cfg(test)]
mod tests {
    use openreel_core::{AssetId, EffectDescriptor, EffectParameterDescriptor, EffectUniform};

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
        assert_eq!(
            transition_duration_operations(ClipId(3), 12),
            vec![
                Operation::RemoveTransition { clip: ClipId(3) },
                Operation::AddTransition {
                    clip: ClipId(3),
                    transition: Transition {
                        name: "crossfade".to_owned(),
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
            }],
            transition_in: None,
            link: None,
        };
        assert_eq!(
            add_effect_operation(&clip, &descriptor),
            Operation::AddEffect {
                clip: ClipId(1),
                effect: Effect {
                    id: EffectId(9),
                    name: "brightness".to_owned(),
                    parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(0),)]),
                },
            }
        );
    }
}
