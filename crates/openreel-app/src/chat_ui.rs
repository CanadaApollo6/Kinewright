use std::{path::Path, time::Duration};

use eframe::egui;
use openreel_agent::{CODEX_SANDBOX_NOTICE, ClaudeCodeDriver, CodexDriver};
use openreel_core::{
    AgentDriver, AgentEvent, AuthenticationStatus, HarnessInfo, SessionConfig, TimeCode,
};

use crate::{
    app::OpenReelApp,
    icons::Icon,
    theme::{self, color, radius, size, space, type_size},
};

const AGENT_HARNESS_MEMORY_ID: &str = "openreel-agent-harness";
const CLAUDE_MODEL_MEMORY_ID: &str = "openreel-agent-model-claude-code";
const CODEX_MODEL_MEMORY_ID: &str = "openreel-agent-model-codex";

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
    /// A watchable diff: one applied agent edit with its changed span.
    EditCard {
        summary: String,
        start: TimeCode,
        end: TimeCode,
        /// Pre-rolled review position (a couple of seconds before `start`).
        cue: TimeCode,
    },
}

/// A deferred action from an edit card in the session stream.
enum EditCardAction {
    Review(TimeCode),
    Undo,
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
                model: match self.agent_harness {
                    AgentHarnessChoice::ClaudeCode => self.claude_model.clone(),
                    AgentHarnessChoice::Codex => self.codex_model.clone(),
                },
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

    /// Run one registry command: prompt commands expand into an agent turn;
    /// local commands act instantly and log themselves to the stream.
    fn run_slash_command(&mut self, command: &'static crate::slash::SlashCommand) {
        use crate::slash::SlashAction;
        if let SlashAction::Prompt(template) = command.action {
            template.clone_into(&mut self.agent_input);
            self.start_agent_turn();
            return;
        }
        self.chat
            .push(ChatEntry::User(format!("/{}", command.name)));
        match command.action {
            SlashAction::RemoveFillers => self.remove_filler_words(),
            SlashAction::AddCaptions => self.add_captions(),
            SlashAction::FreezeFrame => self.freeze_frame_at_playhead(),
            SlashAction::Export => self.open_export_dialog(),
            SlashAction::Undo => self.undo(),
            SlashAction::Redo => self.redo(),
            SlashAction::Help => self.chat.push(ChatEntry::Text(crate::slash::help_text())),
            SlashAction::Prompt(_) => {}
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
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
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
            // Model choices follow the same idle-restore pattern; ids that no
            // longer exist in the harness's catalog fall back to Default.
            self.claude_model =
                restore_model_choice(ui.ctx(), CLAUDE_MODEL_MEMORY_ID, &self.claude_models);
            self.codex_model =
                restore_model_choice(ui.ctx(), CODEX_MODEL_MEMORY_ID, &self.codex_models);
        }
        let any_harness = self.claude_info.is_some() || self.codex_info.is_some();

        // No header chrome (M24): the stream is the surface, and the harness
        // controls live in the composer row like T3 Code's model row. The one
        // exception is the no-harness state, which explains itself up front.
        if !any_harness {
            chat_frame(color::SURFACE, color::BORDER_SUBTLE).show(ui, |ui| {
                harness_row(ui, "Claude Code", self.claude_info.as_ref());
                harness_row(ui, "Codex", self.codex_info.as_ref());
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
            });
            ui.add_space(space::ONE);
        }

        let selected_info = match self.agent_harness {
            AgentHarnessChoice::ClaudeCode => self.claude_info.as_ref(),
            AgentHarnessChoice::Codex => self.codex_info.as_ref(),
        };
        let selected_available = selected_info.is_some();
        // Owned summary so the composer row can render it while self mutates.
        let harness_summary = selected_info.map(|info| {
            // CLI version strings often repeat the product name; keep the
            // number only.
            let version = info.version.as_deref().map_or("version unknown", |value| {
                value.split_whitespace().next().unwrap_or(value)
            });
            format!(
                "{} · {}",
                version,
                authentication_label(info.authentication)
            )
        });

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
        let mut card_action: Option<EditCardAction> = None;
        // Slash suggestions render between the stream and the composer, so
        // their height comes out of the stream's share - otherwise the popup
        // pushes the composer clean out of the clipped panel.
        let matches = crate::slash::matching_commands(&self.agent_input);
        let suggestions_reserve = if matches.is_empty() {
            0.0
        } else {
            24.0 * matches.len() as f32 + 24.0
        };
        let composer_reserve = 132.0 + suggestions_reserve;
        // The composer anchors to the bottom of the session column (T3-style):
        // the stream owns everything above it and sticks to its latest entry.
        let stream_height = (ui.available_height() - composer_reserve).max(96.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height(stream_height)
            .min_scrolled_height(stream_height)
            .show(ui, |ui| {
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
                        ChatEntry::EditCard {
                            summary,
                            start,
                            end,
                            cue,
                        } => {
                            chat_frame(color::SURFACE_RAISED, color::BORDER_STRONG).show(
                                ui,
                                |ui| {
                                    ui.colored_label(
                                        color::TEXT_SECONDARY,
                                        egui::RichText::new("EDIT").strong().size(type_size::MICRO),
                                    );
                                    ui.label(summary);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .small_button("Review")
                                            .on_hover_text(
                                                "Play the changed span with a two-second lead-in",
                                            )
                                            .clicked()
                                        {
                                            card_action = Some(EditCardAction::Review(*cue));
                                        }
                                        if ui
                                            .small_button("Undo")
                                            .on_hover_text("Revert this edit")
                                            .clicked()
                                        {
                                            card_action = Some(EditCardAction::Undo);
                                        }
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} – {}",
                                                crate::timeline_ui::format_timecode(
                                                    *start,
                                                    self.document.fps
                                                ),
                                                crate::timeline_ui::format_timecode(
                                                    *end,
                                                    self.document.fps
                                                ),
                                            ))
                                            .font(theme::code_font())
                                            .color(color::TEXT_MUTED),
                                        );
                                    });
                                },
                            );
                        }
                    }
                    ui.add_space(space::ONE_HALF);
                }
            });
        match card_action {
            Some(EditCardAction::Review(cue)) => {
                self.seek_to(cue);
                self.playback.play(self.position);
            }
            Some(EditCardAction::Undo) => self.undo(),
            None => {}
        }
        // Slash suggestions float directly above the composer while typing.
        let mut run_command: Option<&'static crate::slash::SlashCommand> = None;
        if !matches.is_empty() {
            chat_frame(color::SURFACE_RAISED, color::BORDER_SUBTLE).show(ui, |ui| {
                for command in &matches {
                    let label = format!("/{}", command.name);
                    ui.horizontal(|ui| {
                        if ui.small_button(&label).clicked() {
                            run_command = Some(*command);
                        }
                        ui.colored_label(
                            color::TEXT_MUTED,
                            egui::RichText::new(command.description).size(type_size::CAPTION),
                        );
                    });
                }
            });
        }
        ui.add_space(space::ONE);
        let input_response = egui::Frame::new()
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
                        .hint_text("Describe an edit, or / for commands"),
                )
            })
            .inner;
        // Enter sends (Shift+Enter for a newline); with a slash query active,
        // Enter runs the top match.
        if input_response.has_focus()
            && ui.input(|input| input.key_pressed(egui::Key::Enter) && !input.modifiers.shift)
        {
            if let Some(first) = matches.first() {
                run_command = Some(first);
            } else if !self.agent_input.trim().is_empty() {
                self.agent_input = self.agent_input.trim().to_owned();
                self.start_agent_turn();
            }
        }
        if let Some(command) = run_command {
            self.agent_input.clear();
            self.run_slash_command(command);
        }
        // The composer row carries the session controls, T3-style: harness on
        // the left, transport on the right, everything else is the stream.
        ui.horizontal(|ui| {
            if self.claude_info.is_some() && self.codex_info.is_some() {
                let before = self.agent_harness;
                ui.add_enabled_ui(!self.agent_running, |ui| {
                    egui::ComboBox::from_id_salt("composer-harness")
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
            } else if any_harness {
                ui.colored_label(color::TEXT_SECONDARY, self.agent_harness.label());
            }
            // Model picker for the selected harness. Default defers to the
            // CLI's configured model; a change restarts the session, same as
            // switching harnesses.
            if selected_available {
                let running = self.agent_running;
                let (models, choice, memory_id) = match self.agent_harness {
                    AgentHarnessChoice::ClaudeCode => (
                        &self.claude_models,
                        &mut self.claude_model,
                        CLAUDE_MODEL_MEMORY_ID,
                    ),
                    AgentHarnessChoice::Codex => (
                        &self.codex_models,
                        &mut self.codex_model,
                        CODEX_MODEL_MEMORY_ID,
                    ),
                };
                if !models.is_empty() {
                    let before = choice.clone();
                    let selected_text = choice
                        .as_deref()
                        .map_or("Default", |id| {
                            models
                                .iter()
                                .find(|model| model.id == id)
                                .map_or(id, |model| model.label.as_str())
                        })
                        .to_owned();
                    ui.add_enabled_ui(!running, |ui| {
                        egui::ComboBox::from_id_salt("composer-model")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(choice, None, "Default");
                                for model in models {
                                    ui.selectable_value(
                                        choice,
                                        Some(model.id.clone()),
                                        &model.label,
                                    );
                                }
                            });
                    });
                    let changed = *choice != before;
                    let persisted = choice.clone().unwrap_or_default();
                    if changed {
                        self.stop_agent();
                        ui.ctx().data_mut(|data| {
                            data.insert_persisted(egui::Id::new(memory_id), persisted);
                        });
                    }
                }
            }
            match &harness_summary {
                Some(summary) => {
                    let label = ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(summary).size(type_size::CAPTION),
                    );
                    if self.agent_harness == AgentHarnessChoice::Codex {
                        label.on_hover_text(CODEX_SANDBOX_NOTICE);
                    }
                }
                None => {
                    ui.colored_label(
                        color::STATUS_DANGER,
                        format!("{} not found on PATH", self.agent_harness.label()),
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                if self.agent_running {
                    ui.colored_label(
                        color::ACCENT,
                        egui::RichText::new("RUNNING").size(type_size::MICRO),
                    );
                }
                if self.agent_usage.input_tokens > 0 || self.agent_usage.output_tokens > 0 {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(format!(
                            "{} in / {} out",
                            self.agent_usage.input_tokens, self.agent_usage.output_tokens
                        ))
                        .size(type_size::MICRO),
                    );
                }
            });
        });
    }
}

/// A remembered model id survives only while the harness still offers it.
fn restore_model_choice(
    ctx: &egui::Context,
    memory_id: &str,
    models: &[openreel_agent::ModelChoice],
) -> Option<String> {
    ctx.data_mut(|data| data.get_persisted::<String>(egui::Id::new(memory_id)))
        .filter(|id| models.iter().any(|model| model.id == *id))
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
