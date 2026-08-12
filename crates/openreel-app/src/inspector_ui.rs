use std::collections::BTreeMap;

use eframe::egui;
use openreel_core::{
    Clip, ClipContent, ClipId, EFFECT_DESCRIPTORS, Effect, EffectId, MARKER_COLOR_TOKEN_COUNT,
    Marker, MarkerId, MediaKind, Operation, ParamValue, TITLE_COLORS, TITLE_FONT_SIZES,
    TRANSITION_DESCRIPTORS, TimeCode, Title, TitlePosition, Transition,
};

use crate::{
    app::OpenReelApp,
    theme::{self, color, space, type_size},
    timeline_ui::{linked_members, linked_transition_operations},
};

const INSPECTOR_MAX_HEIGHT: f32 = 360.0;

impl OpenReelApp {
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

        let mut pending = Vec::new();
        ui.add_space(space::TWO);
        ui.strong("Speed");
        let mut speed_percent = clip.speed_percent;
        let speed_changed = ui
            .add(
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
            )
            .changed();
        if clip.speed_percent != 100 {
            ui.colored_label(
                color::TEXT_MUTED,
                "Audio is muted while the speed is not 1.00x",
            );
        }
        if speed_changed {
            match crate::timeline_ui::clip_speed_operations(
                &self.focused().document,
                clip.id,
                speed_percent,
            ) {
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
            let mut changed = ui
                .add(
                    egui::Slider::new(&mut gain_tenth_db, -600..=120)
                        .text("Gain")
                        .integer()
                        .custom_formatter(|value, _| format!("{:+.1} dB", value / 10.0)),
                )
                .changed();
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
                pending.push(clip_audio_operation(
                    audio_clip.id,
                    gain_tenth_db,
                    fade_in_frames,
                    fade_out_frames,
                ));
            }
        }

        effects_section(ui, clip, &mut pending);
        transition_section(ui, &self.focused().document, clip, &mut pending);
        self.send_operations(pending);
    }

    fn freeze_clip_inspector(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        freeze: &openreel_core::FreezeFrame,
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
        let mut pending = Vec::new();
        effects_section(ui, clip, &mut pending);
        transition_section(ui, &self.focused().document, clip, &mut pending);
        self.send_operations(pending);
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

fn effects_section(ui: &mut egui::Ui, clip: &Clip, pending: &mut Vec<Operation>) {
    ui.add_space(space::TWO);
    ui.strong("Effects");
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
}

fn transition_section(
    ui: &mut egui::Ui,
    document: &openreel_core::Document,
    clip: &Clip,
    pending: &mut Vec<Operation>,
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

fn audio_target_clip(document: &openreel_core::Document, selected: ClipId) -> Option<Clip> {
    let mut members = linked_members(document, selected);
    members.sort_by_key(|(_, clip)| clip.id != selected);
    members
        .into_iter()
        .map(|(_, clip)| clip)
        .find(|clip| clip_carries_audio(document, clip))
}

fn clip_carries_audio(document: &openreel_core::Document, clip: &Clip) -> bool {
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

fn transition_duration_operations(
    document: &openreel_core::Document,
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
    use std::path::PathBuf;

    use openreel_core::{
        AssetId, Document, EffectDescriptor, EffectParameterDescriptor, EffectUniform, LinkId,
        MediaAsset, Rational, Track, TrackId, TrackKind,
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
                },
            }
        );
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
        freeze.content = ClipContent::Freeze(openreel_core::FreezeFrame {
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
                },
                MediaAsset {
                    id: AssetId(2),
                    path: PathBuf::from("sound.wav"),
                    name: "Sound".to_owned(),
                    duration: TimeCode(30),
                    fps: Rational::new(30, 1).expect("valid fps"),
                    kind: MediaKind::Audio,
                    resolution: None,
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
