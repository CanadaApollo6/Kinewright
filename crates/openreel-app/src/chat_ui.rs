use std::{path::Path, time::Duration};

use eframe::egui;
use openreel_agent::{CODEX_SANDBOX_NOTICE, ClaudeCodeDriver, CodexDriver};
use openreel_core::{AgentDriver, AgentEvent, AuthenticationStatus, HarnessInfo, SessionConfig};

use crate::{
    app::OpenReelApp,
    icons::Icon,
    theme::{self, color, radius, size, space, type_size},
};

const AGENT_HARNESS_MEMORY_ID: &str = "openreel-agent-harness";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentHarnessChoice {
    ClaudeCode,
    Codex,
}

impl AgentHarnessChoice {
    fn key(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "claude-code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

pub(crate) enum ChatEntry {
    User(String),
    Text(String),
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        result: String,
    },
    Cost {
        input_tokens: u64,
        output_tokens: u64,
    },
}

/// Session token usage. Dollar figures are deliberately not surfaced: the
/// supported harnesses run on flat-fee subscriptions, so a running cost
/// readout is noise; the turn cap bounds runaway sessions instead.
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageAccumulator {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
}

impl UsageAccumulator {
    pub(crate) fn record(&mut self, event: &AgentEvent) {
        if let AgentEvent::Cost {
            input_tokens,
            output_tokens,
            ..
        } = event
        {
            self.input_tokens = self.input_tokens.saturating_add(*input_tokens);
            self.output_tokens = self.output_tokens.saturating_add(*output_tokens);
        }
    }

    pub(crate) fn reset_usage(&mut self) {
        self.input_tokens = 0;
        self.output_tokens = 0;
    }
}

impl OpenReelApp {
    pub(crate) fn start_agent_turn(&mut self) {
        let message = self.agent_input.trim().to_owned();
        if message.is_empty() || self.agent_running {
            return;
        }
        let Some(endpoint) = self
            .mcp_server
            .as_ref()
            .map(|server| server.endpoint().to_owned())
        else {
            self.record_error("Agent", "The OpenReel agent server is unavailable");
            return;
        };
        let harness_info = match self.agent_harness {
            AgentHarnessChoice::ClaudeCode => self.claude_info.as_ref(),
            AgentHarnessChoice::Codex => self.codex_info.as_ref(),
        };
        if harness_info.is_none() {
            self.record_error(
                "Agent",
                format!("{} is not installed on PATH", self.agent_harness.label()),
            );
            return;
        }

        if self.agent_session.is_none() {
            self.agent_usage.reset_usage();
            let working_directory = self
                .project_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok());
            let config = SessionConfig {
                working_directory,
                model: None,
                // Subscription harnesses are flat fee and the Stop button is
                // always available, so sessions run without a turn ceiling.
                max_turns: None,
                mcp_url: Some(endpoint),
            };
            let session = match self.agent_harness {
                AgentHarnessChoice::ClaudeCode => ClaudeCodeDriver.start_session(config),
                AgentHarnessChoice::Codex => CodexDriver.start_session(config),
            };
            match session {
                Ok(session) => {
                    self.agent_events = Some(session.events());
                    self.agent_session = Some(session);
                }
                Err(error) => {
                    self.record_error(
                        "Agent",
                        format!("Could not start {}: {error}", self.agent_harness.label()),
                    );
                    return;
                }
            }
        }

        let result = self
            .agent_session
            .as_mut()
            .expect("agent session was initialized")
            .send_user_message(message.clone());
        match result {
            Ok(()) => {
                self.chat.push(ChatEntry::User(message));
                self.agent_input.clear();
                self.agent_running = true;
                self.status = format!("{} is editing the timeline", self.agent_harness.label());
            }
            Err(error) => {
                self.record_error("Agent", format!("Could not send agent message: {error}"));
            }
        }
    }

    pub(crate) fn stop_agent(&mut self) {
        if let Some(confirmations) = &self.confirmations {
            confirmations.reject_all("the agent session was interrupted");
        }
        if let Some(session) = &mut self.agent_session {
            session.interrupt();
        }
        self.agent_session = None;
        self.agent_events = None;
        self.agent_running = false;
        "Agent stopped".clone_into(&mut self.status);
    }

    pub(crate) fn poll_agent(&mut self, ctx: &egui::Context) {
        if let Some(confirmations) = &self.confirmations {
            self.pending_confirmations
                .retain(|request| confirmations.is_pending(request.id));
            self.pending_confirmations
                .extend(confirmations.pending_requests());
        }
        let events = self
            .agent_events
            .as_ref()
            .map(|receiver| receiver.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            self.agent_usage.record(&event);
            match event {
                AgentEvent::Text(text) => self.chat.push(ChatEntry::Text(text)),
                AgentEvent::Error(error) => {
                    self.chat.push(ChatEntry::Text(error.clone()));
                    self.record_error("Agent", error);
                }
                AgentEvent::ToolCall { name, arguments } => {
                    self.chat.push(ChatEntry::ToolCall { name, arguments });
                }
                AgentEvent::ToolResult { name, result } => {
                    self.chat.push(ChatEntry::ToolResult { name, result });
                }
                AgentEvent::Cost {
                    input_tokens,
                    output_tokens,
                    ..
                } => self.chat.push(ChatEntry::Cost {
                    input_tokens,
                    output_tokens,
                }),
                AgentEvent::Done => {
                    self.agent_running = false;
                    "Agent turn finished".clone_into(&mut self.status);
                }
            }
        }
        if self.agent_running {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    // The agent panel is one ordered immediate-mode UI pass over session and confirmation state.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn agent_panel(&mut self, ui: &mut egui::Ui) {
        if !self.agent_running && self.agent_session.is_none() {
            if self.claude_info.is_some() && self.codex_info.is_none() {
                self.agent_harness = AgentHarnessChoice::ClaudeCode;
            } else if self.codex_info.is_some() && self.claude_info.is_none() {
                self.agent_harness = AgentHarnessChoice::Codex;
            } else if self.claude_info.is_some() && self.codex_info.is_some() {
                let remembered = ui.ctx().data_mut(|data| {
                    data.get_persisted::<String>(egui::Id::new(AGENT_HARNESS_MEMORY_ID))
                });
                if let Some(remembered) =
                    remembered.and_then(|key| AgentHarnessChoice::from_key(&key))
                {
                    self.agent_harness = remembered;
                }
            }
        }
        let any_harness = self.claude_info.is_some() || self.codex_info.is_some();

        ui.horizontal(|ui| {
            ui.add(Icon::Chat.image(size::ICON_MD).tint(color::TEXT_SECONDARY));
            ui.label(
                egui::RichText::new("AGENT")
                    .strong()
                    .size(type_size::HEADING),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The pill tells the truth: no detected harness is not "ready".
                let (state, state_color) = if self.agent_running {
                    ("RUNNING", color::ACCENT)
                } else if any_harness {
                    ("READY", color::STATUS_SUCCESS)
                } else {
                    ("NO HARNESS", color::STATUS_WARNING)
                };
                ui.colored_label(
                    state_color,
                    egui::RichText::new(state).size(type_size::MICRO),
                );
            });
        });
        ui.add_space(space::ONE);

        egui::CollapsingHeader::new("Harness detection")
            .default_open(!any_harness)
            .show(ui, |ui| {
                // Absence is normal, not an emergency: a muted dot, not red.
                // Red is reserved for the selected harness being unusable.
                harness_row(ui, "Claude Code", self.claude_info.as_ref());
                harness_row(ui, "Codex", self.codex_info.as_ref());
                if !any_harness {
                    ui.separator();
                    ui.label("Install and authenticate a supported agent CLI to use chat.");
                    ui.hyperlink_to(
                        "Install Claude Code",
                        "https://docs.anthropic.com/en/docs/claude-code/getting-started",
                    );
                    ui.hyperlink_to(
                        "Install Codex CLI",
                        "https://developers.openai.com/codex/cli",
                    );
                }
            });

        if self.claude_info.is_some() && self.codex_info.is_some() {
            let before = self.agent_harness;
            ui.add_enabled_ui(!self.agent_running, |ui| {
                egui::ComboBox::from_label("Harness")
                    .selected_text(self.agent_harness.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.agent_harness,
                            AgentHarnessChoice::ClaudeCode,
                            "Claude Code",
                        );
                        ui.selectable_value(
                            &mut self.agent_harness,
                            AgentHarnessChoice::Codex,
                            "Codex",
                        );
                    });
            });
            if before != self.agent_harness {
                self.stop_agent();
                ui.ctx().data_mut(|data| {
                    data.insert_persisted(
                        egui::Id::new(AGENT_HARNESS_MEMORY_ID),
                        self.agent_harness.key().to_owned(),
                    );
                });
            }
        }

        let selected_info = match self.agent_harness {
            AgentHarnessChoice::ClaudeCode => self.claude_info.as_ref(),
            AgentHarnessChoice::Codex => self.codex_info.as_ref(),
        };
        match selected_info {
            Some(info) => {
                // CLI version strings often repeat the product name; keep the
                // number only.
                let version = info.version.as_deref().map_or("version unknown", |value| {
                    value.split_whitespace().next().unwrap_or(value)
                });
                ui.label(format!(
                    "Using {} {} · {}",
                    self.agent_harness.label(),
                    version,
                    authentication_label(info.authentication)
                ))
            }
            None => ui.colored_label(
                color::STATUS_DANGER,
                format!("{} not found on PATH", self.agent_harness.label()),
            ),
        };
        let selected_available = selected_info.is_some();
        if self.agent_harness == AgentHarnessChoice::Codex {
            ui.small(CODEX_SANDBOX_NOTICE);
        }
        ui.label(format!(
            "Session: {} in / {} out tokens",
            self.agent_usage.input_tokens, self.agent_usage.output_tokens
        ));
        ui.separator();

        let mut confirmation_decision = None;
        for request in &self.pending_confirmations {
            egui::Frame::new()
                .fill(color::SURFACE_RAISED)
                .stroke(egui::Stroke::new(1.0, color::STATUS_WARNING))
                .corner_radius(radius::MD)
                .inner_margin(egui::Margin::same(theme::margin(space::TWO)))
                .show(ui, |ui| {
                    ui.colored_label(color::STATUS_WARNING, "AGENT CONFIRMATION REQUIRED");
                    ui.strong(&request.tool_name);
                    ui.label(&request.description);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new("Approve").fill(color::ACCENT_28))
                            .clicked()
                        {
                            confirmation_decision = Some((request.id, true));
                        }
                        if ui
                            .add(egui::Button::new("Reject").fill(color::SURFACE_ACTIVE))
                            .clicked()
                        {
                            confirmation_decision = Some((request.id, false));
                        }
                    });
                });
        }
        if let Some((id, approve)) = confirmation_decision
            && let Some(confirmations) = &self.confirmations
        {
            let resolved = if approve {
                confirmations.approve(id)
            } else {
                confirmations.reject(id, "rejected by user")
            };
            if resolved {
                self.pending_confirmations
                    .retain(|request| request.id != id);
                self.chat.push(ChatEntry::Text(if approve {
                    "Approved destructive edit.".to_owned()
                } else {
                    "Rejected destructive edit.".to_owned()
                }));
            }
        }

        // Reserve room below the history for the composer and send row - an
        // uncapped scroll area consumes the whole dock and pushes the input
        // out of the clipped panel, leaving no visible way to talk to the
        // agent.
        let composer_reserve = 132.0;
        // Vertical auto-shrink keeps an empty or short history compact - the
        // composer sits right below the last message instead of being pinned
        // under a column of dead space - while long histories cap out and
        // scroll with the composer held visible.
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .stick_to_bottom(true)
            .max_height((ui.available_height() - composer_reserve).max(96.0))
            .show(ui, |ui| {
                if self.chat.is_empty() {
                    ui.add_space(space::TWO);
                    ui.label(
                        egui::RichText::new(
                            "Ask for an edit in plain language. The agent sees your timeline, \
                             transcript, silences, and scene changes, and every change it makes \
                             is one undo away.",
                        )
                        .color(color::TEXT_MUTED)
                        .italics(),
                    );
                }
                for (index, entry) in self.chat.iter().enumerate() {
                    match entry {
                        ChatEntry::User(text) => {
                            chat_frame(color::ACCENT_16, color::ACCENT_72).show(ui, |ui| {
                                ui.colored_label(
                                    color::ACCENT,
                                    egui::RichText::new("YOU").strong().size(type_size::MICRO),
                                );
                                ui.label(text);
                            });
                        }
                        ChatEntry::Text(text) => {
                            chat_frame(color::SURFACE, color::BORDER_SUBTLE).show(ui, |ui| {
                                ui.colored_label(
                                    color::TEXT_SECONDARY,
                                    egui::RichText::new("AGENT").strong().size(type_size::MICRO),
                                );
                                ui.label(text);
                            });
                        }
                        ChatEntry::ToolCall { name, arguments } => {
                            chat_frame(color::SURFACE_RAISED, color::BORDER_STRONG).show(
                                ui,
                                |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            Icon::Waveform
                                                .image(size::ICON_SM)
                                                .tint(color::TEXT_MUTED),
                                        );
                                        ui.colored_label(
                                            color::TEXT_SECONDARY,
                                            egui::RichText::new(format!("TOOL · {name}"))
                                                .strong()
                                                .size(type_size::MICRO),
                                        );
                                    });
                                    ui.label(
                                        egui::RichText::new(summarize(arguments, 180))
                                            .font(theme::code_font())
                                            .color(color::TEXT_SECONDARY),
                                    );
                                },
                            );
                        }
                        ChatEntry::ToolResult { name, result } => {
                            chat_frame(color::SURFACE, color::BORDER_SUBTLE).show(ui, |ui| {
                                egui::CollapsingHeader::new(format!("RESULT · {name}"))
                                    .id_salt(("agent-result", index))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(summarize(result, 500))
                                                .font(theme::code_font())
                                                .color(color::TEXT_SECONDARY),
                                        );
                                    });
                            });
                        }
                        ChatEntry::Cost {
                            input_tokens,
                            output_tokens,
                        } => {
                            ui.colored_label(
                                color::TEXT_MUTED,
                                egui::RichText::new(format!(
                                    "{input_tokens} input / {output_tokens} output tokens"
                                ))
                                .size(type_size::MICRO),
                            );
                        }
                    }
                    ui.add_space(space::ONE_HALF);
                }
            });
        ui.add_space(space::ONE);
        egui::Frame::new()
            .fill(color::CANVAS)
            .stroke(egui::Stroke::new(1.0, color::BORDER_STRONG))
            .corner_radius(radius::MD)
            .inner_margin(egui::Margin::same(theme::margin(space::ONE)))
            .show(ui, |ui| {
                ui.add_enabled(
                    !self.agent_running,
                    egui::TextEdit::multiline(&mut self.agent_input)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE)
                        .hint_text("Describe an edit…"),
                );
            });
        ui.horizontal(|ui| {
            let can_send = !self.agent_running
                && !self.agent_input.trim().is_empty()
                && selected_available
                && self.mcp_server.is_some();
            if ui
                .add_enabled(
                    can_send,
                    egui::Button::image_and_text(Icon::Send.image(size::ICON_MD), "Send")
                        .fill(color::ACCENT_28)
                        .stroke(egui::Stroke::new(1.0, color::ACCENT_72)),
                )
                .clicked()
            {
                self.start_agent_turn();
            }
            if ui
                .add_enabled(
                    self.agent_running,
                    egui::Button::image_and_text(Icon::Stop.image(size::ICON_MD), "Stop")
                        .fill(color::SURFACE_RAISED),
                )
                .clicked()
            {
                self.stop_agent();
            }
        });
    }
}

fn chat_frame(fill: egui::Color32, stroke: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(radius::MD)
        .inner_margin(egui::Margin::same(theme::margin(space::TWO)))
}

fn harness_row(ui: &mut egui::Ui, name: &str, info: Option<&HarnessInfo>) {
    ui.horizontal(|ui| {
        if let Some(info) = info {
            ui.colored_label(color::STATUS_SUCCESS, "●");
            ui.label(format!(
                "{name} {}",
                info.version.as_deref().unwrap_or("(version unknown)")
            ));
        } else {
            ui.colored_label(color::TEXT_MUTED, "○");
            ui.colored_label(color::TEXT_MUTED, format!("{name} not detected"));
        }
    });
    if let Some(info) = info {
        ui.small(format!(
            "{} · {}",
            info.executable.display(),
            authentication_label(info.authentication)
        ));
    }
}

fn authentication_label(authentication: AuthenticationStatus) -> &'static str {
    match authentication {
        AuthenticationStatus::Authenticated => "authenticated",
        AuthenticationStatus::Unauthenticated => "not authenticated",
        AuthenticationStatus::Unknown => "authentication unknown",
    }
}

fn summarize(value: &str, maximum_chars: usize) -> String {
    let mut summary = value.chars().take(maximum_chars).collect::<String>();
    if value.chars().count() > maximum_chars {
        summary.push('…');
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_events_accumulate_token_usage() {
        let mut usage = UsageAccumulator::default();
        let events = [
            AgentEvent::Cost {
                input_tokens: 100,
                output_tokens: 20,
                cost_usd: Some(0.02),
            },
            AgentEvent::Text("not a cost".to_owned()),
            AgentEvent::Cost {
                input_tokens: 50,
                output_tokens: 10,
                cost_usd: None,
            },
            AgentEvent::Cost {
                input_tokens: 75,
                output_tokens: 15,
                cost_usd: Some(0.03),
            },
        ];
        for event in &events {
            usage.record(event);
        }
        assert_eq!(usage.input_tokens, 225);
        assert_eq!(usage.output_tokens, 45);
        usage.reset_usage();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }
}
