//! The Settings window (T3-style): a providers page showing each harness's
//! brand mark, detected version, authentication state, executable, and an
//! enable toggle.
//!
//! Provider toggles gate which harnesses the composer offers for new turns;
//! they never interrupt a session that is already running.

use eframe::egui;
use openreel_agent::CODEX_SANDBOX_NOTICE;
use openreel_core::HarnessInfo;

use crate::{
    app::OpenReelApp,
    chat_ui::{AgentHarnessChoice, authentication_label},
    theme::{self, color, radius, size, space, type_size},
};

const fn provider_memory_id(harness: AgentHarnessChoice) -> &'static str {
    match harness {
        AgentHarnessChoice::ClaudeCode => "openreel-provider-enabled-claude-code",
        AgentHarnessChoice::Codex => "openreel-provider-enabled-codex",
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

impl OpenReelApp {
    pub(crate) fn show_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(theme::caps_label("PROVIDERS", color::TEXT_MUTED));
                ui.add_space(space::ONE);
                provider_card(
                    ui,
                    AgentHarnessChoice::ClaudeCode,
                    self.claude_info.as_ref(),
                    "https://docs.anthropic.com/en/docs/claude-code/getting-started",
                );
                ui.add_space(space::ONE);
                provider_card(
                    ui,
                    AgentHarnessChoice::Codex,
                    self.codex_info.as_ref(),
                    "https://developers.openai.com/codex/cli",
                );
            });
        self.settings_open = open;
    }
}

fn provider_card(
    ui: &mut egui::Ui,
    harness: AgentHarnessChoice,
    info: Option<&HarnessInfo>,
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
                    if ui.checkbox(&mut enabled, "Enabled").changed() {
                        set_provider_enabled(&ctx, harness, enabled);
                    }
                });
            });
            if let Some(info) = info {
                ui.colored_label(
                    color::TEXT_MUTED,
                    format!(
                        "{} · {}",
                        authentication_label(info.authentication),
                        info.executable.display()
                    ),
                );
                if harness == AgentHarnessChoice::Codex {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(CODEX_SANDBOX_NOTICE).size(type_size::CAPTION),
                    );
                }
            } else {
                ui.horizontal(|ui| {
                    ui.colored_label(color::TEXT_MUTED, "Not detected on this machine.");
                    ui.hyperlink_to("Install", install_url.to_owned());
                });
            }
        });
    theme::paint_raised_lighting(ui.painter(), card.response.rect, radius::px(radius::MD));
}
