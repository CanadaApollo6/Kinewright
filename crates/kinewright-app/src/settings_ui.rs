//! The Settings window (T3-style): a providers page showing each harness's
//! brand mark, detected version, authentication state, executable, and an
//! enable toggle.
//!
//! Provider toggles gate which harnesses the composer offers for new turns;
//! they never interrupt a session that is already running.

use eframe::egui;
use kinewright_core::HarnessInfo;

use crate::{
    app::KinewrightApp,
    chat_ui::{AgentHarnessChoice, authentication_label},
    investigator::{
        harness_disabled_by_one_strike, investigator_config_path, note_settings_window,
    },
    theme::{self, color, radius, size, space, type_size},
};

const fn provider_memory_id(harness: AgentHarnessChoice) -> &'static str {
    match harness {
        AgentHarnessChoice::ClaudeCode => "kinewright-provider-enabled-claude-code",
        AgentHarnessChoice::Codex => "kinewright-provider-enabled-codex",
        AgentHarnessChoice::Cursor => "kinewright-provider-enabled-cursor",
        AgentHarnessChoice::Muse => "kinewright-provider-enabled-muse",
        AgentHarnessChoice::OpenCode => "kinewright-provider-enabled-opencode",
        AgentHarnessChoice::Qwen => "kinewright-provider-enabled-qwen",
        AgentHarnessChoice::Kimi => "kinewright-provider-enabled-kimi",
        AgentHarnessChoice::Kiro => "kinewright-provider-enabled-kiro",
        AgentHarnessChoice::Devin => "kinewright-provider-enabled-devin",
        AgentHarnessChoice::Copilot => "kinewright-provider-enabled-copilot",
    }
}

/// Whether the user has this provider switched on (default: yes). Installed
/// state is a separate fact - a provider can be enabled but not detected.
pub(crate) fn provider_enabled(ctx: &egui::Context, harness: AgentHarnessChoice) -> bool {
    ctx.data_mut(|data| data.get_persisted::<bool>(egui::Id::new(provider_memory_id(harness))))
        .unwrap_or(true)
}

fn set_provider_enabled(ctx: &egui::Context, harness: AgentHarnessChoice, enabled: bool) {
    ctx.data_mut(|data| {
        data.insert_persisted(egui::Id::new(provider_memory_id(harness)), enabled);
    });
}

impl KinewrightApp {
    pub(crate) fn show_settings_dialog(&mut self, ctx: &egui::Context) {
        // The closed-to-open edge re-runs harness detection (IN2 §2.2 rule 7).
        note_settings_window(self.settings_open);
        if !self.settings_open {
            return;
        }
        let was_open = self.settings_open;
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(440.0);
                self.show_investigator_settings(ui);
                ui.add_space(space::TWO);
                ui.label(theme::caps_label("PROVIDERS", color::TEXT_MUTED));
                ui.add_space(space::ONE);
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for harness in AgentHarnessChoice::ALL {
                            provider_card(
                                ui,
                                harness,
                                self.harness[harness.index()].info.as_ref(),
                                self.harness[harness.index()].detected,
                                harness.install_url(),
                            );
                            ui.add_space(space::ONE);
                        }
                    });
            });
        self.settings_open = open;
        if was_open && !open {
            self.commit_investigator_settings();
        }
    }

    /// The Investigator section: the off switch, the harness and budget
    /// editors, and the focused project's mutes (IN2 §2).
    ///
    /// Untested by design, exactly as the provider cards are: the tested
    /// things are the settings file round-trip and the session behaviour.
    #[allow(
        clippy::too_many_lines,
        reason = "the settings page keeps one coherent investigator section"
    )]
    fn show_investigator_settings(&mut self, ui: &mut egui::Ui) {
        let settings = self
            .focused()
            .investigator
            .as_ref()
            .map(|session| session.settings.clone())
            .unwrap_or_default();
        let any_detected = self.harness.iter().any(|state| state.info.is_some());
        ui.label(theme::caps_label("INVESTIGATOR", color::TEXT_MUTED));
        ui.add_space(space::ONE);
        // The off switch (§2.3 rule 9): switchable on only when a harness is
        // detected, always switchable off.
        let mut enabled = settings.enabled;
        let mut enabled_changed = None;
        ui.add_enabled_ui(any_detected || enabled, |ui| {
            ui.horizontal(|ui| {
                if toggle_switch(ui, &mut enabled).changed() {
                    enabled_changed = Some(enabled);
                }
                ui.label("Investigate refused edits with an agent session");
            });
        });
        if let Some(enabled) = enabled_changed {
            self.update_investigator_settings(|settings| settings.enabled = enabled);
            self.commit_investigator_settings();
        }
        if !any_detected {
            ui.colored_label(color::TEXT_MUTED, "No harness detected on this machine.");
        }
        // The harness picker (§2.2): automatic, or one named harness.
        let mut picked = settings.harness.clone();
        let selected = settings.harness.as_deref().and_then(|key| {
            AgentHarnessChoice::from_key(key).map(|choice| choice.label().to_owned())
        });
        let mut harness_changed = None;
        egui::ComboBox::from_id_salt("investigator-harness")
            .selected_text(selected.as_deref().unwrap_or("Automatic"))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(&mut picked, None, "Automatic")
                    .changed()
                {
                    harness_changed = Some(None);
                }
                for choice in AgentHarnessChoice::ALL {
                    let state = &self.harness[choice.index()];
                    let status = match (&state.info, state.detected) {
                        (Some(info), _) => format!(
                            "{} · {}",
                            authentication_label(info.authentication),
                            info.version.as_deref().unwrap_or("version unknown")
                        ),
                        (None, true) => "not detected".to_owned(),
                        (None, false) => "detecting…".to_owned(),
                    };
                    let value = Some(choice.key().to_owned());
                    if ui
                        .selectable_value(
                            &mut picked,
                            value.clone(),
                            format!("{} ({status})", choice.label()),
                        )
                        .changed()
                    {
                        harness_changed = Some(value);
                    }
                }
            });
        if let Some(harness) = harness_changed {
            self.update_investigator_settings(|settings| settings.harness.clone_from(&harness));
            self.commit_investigator_settings();
        }
        // The named-harness states (§2.2 rules 7–8): missing, or struck out.
        if let Some(key) = settings.harness.as_deref() {
            let missing = AgentHarnessChoice::from_key(key)
                .is_none_or(|choice| self.harness[choice.index()].info.is_none());
            if missing {
                ui.colored_label(color::TEXT_MUTED, "harness not detected");
            } else if harness_disabled_by_one_strike(key) {
                ui.colored_label(
                    color::TEXT_MUTED,
                    "Harness disabled for this session after a failure.",
                );
            }
        }
        // Model, effort, and tier overrides: empty means the CLI's default.
        let mut model = settings.model.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Model");
            let response = ui.text_edit_singleline(&mut model);
            if response.changed() {
                let trimmed = model.trim();
                let value = (!trimmed.is_empty()).then(|| trimmed.to_owned());
                self.update_investigator_settings(|settings| settings.model.clone_from(&value));
            }
            if response.lost_focus() {
                self.commit_investigator_settings();
            }
        });
        let mut effort = settings.effort.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Effort");
            let response = ui.text_edit_singleline(&mut effort);
            if response.changed() {
                let trimmed = effort.trim();
                let value = (!trimmed.is_empty()).then(|| trimmed.to_owned());
                self.update_investigator_settings(|settings| settings.effort.clone_from(&value));
            }
            if response.lost_focus() {
                self.commit_investigator_settings();
            }
        });
        let mut tier = settings.service_tier.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Service tier");
            let response = ui.text_edit_singleline(&mut tier);
            if response.changed() {
                let trimmed = tier.trim();
                let value = (!trimmed.is_empty()).then(|| trimmed.to_owned());
                self.update_investigator_settings(|settings| {
                    settings.service_tier.clone_from(&value);
                });
            }
            if response.lost_focus() {
                self.commit_investigator_settings();
            }
        });
        // The three budgets (§2.1 rule 5, §5.1 rule 2): clamped, never refused.
        let mut turns = settings.budgets.max_turns;
        let mut turns_commit = false;
        ui.horizontal(|ui| {
            ui.label("Turns");
            let response = ui.add(egui::DragValue::new(&mut turns).range(1..=64));
            turns_commit = response.drag_stopped() || response.lost_focus();
        });
        if turns != settings.budgets.max_turns {
            self.update_investigator_settings(|settings| {
                settings.budgets.max_turns = turns;
                settings.clamp();
            });
        }
        if turns_commit {
            self.commit_investigator_settings();
        }
        let mut wall = settings.budgets.max_wall_time_seconds;
        let mut wall_commit = false;
        ui.horizontal(|ui| {
            ui.label("Wall time (s)");
            let response = ui.add(egui::DragValue::new(&mut wall).range(5..=900));
            wall_commit = response.drag_stopped() || response.lost_focus();
        });
        if wall != settings.budgets.max_wall_time_seconds {
            self.update_investigator_settings(|settings| {
                settings.budgets.max_wall_time_seconds = wall;
                settings.clamp();
            });
        }
        if wall_commit {
            self.commit_investigator_settings();
        }
        let mut tokens = settings.budgets.max_tokens;
        let mut tokens_commit = false;
        ui.horizontal(|ui| {
            ui.label("Tokens");
            let response = ui.add(egui::DragValue::new(&mut tokens).range(1_000..=2_000_000));
            tokens_commit = response.drag_stopped() || response.lost_focus();
        });
        if tokens != settings.budgets.max_tokens {
            self.update_investigator_settings(|settings| {
                settings.budgets.max_tokens = tokens;
                settings.clamp();
            });
        }
        if tokens_commit {
            self.commit_investigator_settings();
        }
        // The non-durable-path notice (§2.1 rule 1).
        let (path, durable) = investigator_config_path();
        if !durable {
            ui.colored_label(
                color::TEXT_MUTED,
                format!(
                    "Settings will not persist: no config directory was found, using {}.",
                    path.display()
                ),
            );
        }
        // The focused project's mutes, each removable (§2.4 rule 14).
        let mutes = self
            .focused()
            .investigator
            .as_ref()
            .map(|session| session.muted_codes().to_vec())
            .unwrap_or_default();
        if !mutes.is_empty() {
            ui.label(theme::caps_label("MUTED CODES", color::TEXT_MUTED));
            let mut unmute = None;
            for code in &mutes {
                ui.horizontal(|ui| {
                    ui.label(code);
                    if ui.button("Remove").clicked() {
                        unmute = Some(code.clone());
                    }
                });
            }
            if let Some(code) = unmute {
                let project_index = self.focused_project;
                self.unmute_investigator_code(project_index, &code);
            }
        }
    }
}

/// A small pill toggle - reads as a switch, not a form checkbox. Toggles are
/// one of the accent's four earned places.
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(30.0, 16.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let progress = ui
        .ctx()
        .animate_bool_responsive(response.id.with("toggle"), *on);
    let track = if *on {
        color::ACCENT_DIM_BORDER
    } else {
        color::SURFACE_ACTIVE
    };
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(rect, radius, track);
    let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), progress);
    let knob = if *on {
        color::ACCENT
    } else {
        color::TEXT_MUTED
    };
    ui.painter()
        .circle_filled(egui::pos2(knob_x, rect.center().y), radius - 3.0, knob);
    response.on_hover_text(if *on { "Enabled" } else { "Disabled" })
}

fn provider_card(
    ui: &mut egui::Ui,
    harness: AgentHarnessChoice,
    info: Option<&HarnessInfo>,
    detected: bool,
    install_url: &str,
) {
    let ctx = ui.ctx().clone();
    let card = egui::Frame::new()
        .fill(color::SURFACE)
        .corner_radius(radius::MD)
        .inner_margin(egui::Margin::same(theme::margin(space::TWO)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(harness.brand_icon().image(size::ICON_MD));
                ui.label(
                    egui::RichText::new(harness.label()).font(theme::semibold(type_size::BODY)),
                );
                if let Some(info) = info {
                    ui.colored_label(
                        color::TEXT_SECONDARY,
                        info.version.as_deref().unwrap_or("version unknown"),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut enabled = provider_enabled(&ctx, harness);
                    if toggle_switch(ui, &mut enabled).changed() {
                        set_provider_enabled(&ctx, harness, enabled);
                    }
                });
            });
            if let Some(info) = info {
                let identity = info.subscription_tier.as_ref().map_or_else(
                    || authentication_label(info.authentication).to_owned(),
                    |tier| format!("{} · {tier}", authentication_label(info.authentication)),
                );
                ui.colored_label(color::TEXT_MUTED, identity);
                if let Some(notice) = harness.sandbox_notice() {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(notice).size(type_size::CAPTION),
                    );
                }
            } else if detected {
                ui.horizontal(|ui| {
                    ui.colored_label(color::TEXT_MUTED, "Not detected on this machine.");
                    ui.hyperlink_to("Install", install_url.to_owned());
                });
            } else {
                // The probe thread has not reached this harness yet, so
                // "not detected" would be a guess rather than an answer.
                ui.colored_label(color::TEXT_MUTED, "Detecting…");
            }
        });
    theme::paint_raised_lighting(ui.painter(), card.response.rect, radius::px(radius::MD));
}
