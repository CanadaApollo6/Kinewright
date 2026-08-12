//! The Settings window (T3-style): a providers page showing each harness's
//! brand mark, detected version, authentication state, executable, and an
//! enable toggle.
//!
//! Provider toggles gate which harnesses the composer offers for new turns;
//! they never interrupt a session that is already running.

use eframe::egui;
use openreel_agent::{CODEX_SANDBOX_NOTICE, CURSOR_SANDBOX_NOTICE};
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
        AgentHarnessChoice::Cursor => "openreel-provider-enabled-cursor",
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
                ui.add_space(space::ONE);
                provider_card(
                    ui,
                    AgentHarnessChoice::Cursor,
                    self.cursor_info.as_ref(),
                    "https://docs.cursor.com/en/cli/installation",
                );
            });
        self.settings_open = open;
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
            // Identity facts only: version above, authentication here. The
            // executable path is deliberately not shown - filesystem details
            // are not something to display (or screenshot).
            if let Some(info) = info {
                let identity = info.subscription_tier.as_ref().map_or_else(
                    || authentication_label(info.authentication).to_owned(),
                    |tier| format!("{} · {tier}", authentication_label(info.authentication)),
                );
                ui.colored_label(color::TEXT_MUTED, identity);
                if harness == AgentHarnessChoice::Codex {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(CODEX_SANDBOX_NOTICE).size(type_size::CAPTION),
                    );
                } else if harness == AgentHarnessChoice::Cursor {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(CURSOR_SANDBOX_NOTICE).size(type_size::CAPTION),
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
