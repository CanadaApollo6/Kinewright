use std::{
    path::Path,
    time::{Duration, Instant},
};

use eframe::egui;
use openreel_agent::{CODEX_SANDBOX_NOTICE, ClaudeCodeDriver, CodexDriver};
use openreel_core::{
    AgentDriver, AgentEvent, AgentSession, AuthenticationStatus, HarnessInfo, SessionConfig,
    TimeCode,
};

use crate::{
    app::OpenReelApp,
    icons::Icon,
    project::{ProjectSession, index_after_close},
    theme::{self, color, radius, size, space, type_size},
};

const AGENT_HARNESS_MEMORY_ID: &str = "openreel-agent-harness";
const CLAUDE_MODEL_MEMORY_ID: &str = "openreel-agent-model-claude-code";
const CODEX_MODEL_MEMORY_ID: &str = "openreel-agent-model-codex";
const CLAUDE_EFFORT_MEMORY_ID: &str = "openreel-agent-effort-claude-code";
const CODEX_EFFORT_MEMORY_ID: &str = "openreel-agent-effort-codex";
const CLAUDE_TIER_MEMORY_ID: &str = "openreel-agent-tier-claude-code";
const CODEX_TIER_MEMORY_ID: &str = "openreel-agent-tier-codex";

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

    /// The harness's brand mark (already in its brand color - not tinted).
    fn brand_icon(self) -> Icon {
        match self {
            Self::ClaudeCode => Icon::BrandClaude,
            Self::Codex => Icon::BrandOpenAi,
        }
    }
}

#[derive(Clone)]
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
/// readout is noise; the per-thread Stop control remains available instead.
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

pub(crate) struct AgentThread {
    pub(crate) name: String,
    pub(crate) harness: AgentHarnessChoice,
    pub(crate) session: Option<Box<dyn AgentSession>>,
    pub(crate) events: Option<crossbeam_channel::Receiver<AgentEvent>>,
    pub(crate) running: bool,
    pub(crate) input: String,
    pub(crate) usage: UsageAccumulator,
    pub(crate) chat: Vec<ChatEntry>,
    last_activity: Instant,
}

impl AgentThread {
    pub(crate) fn new(
        name: impl Into<String>,
        harness: AgentHarnessChoice,
        chat: Vec<ChatEntry>,
    ) -> Self {
        Self {
            name: name.into(),
            harness,
            session: None,
            events: None,
            running: false,
            input: String::new(),
            usage: UsageAccumulator::default(),
            chat,
            last_activity: Instant::now(),
        }
    }
}

impl OpenReelApp {
    pub(crate) fn active_thread(&self) -> &AgentThread {
        let project_index = self.focused_project;
        &self.projects[project_index].threads[self.projects[project_index].active_thread]
    }

    pub(crate) fn active_thread_mut(&mut self) -> &mut AgentThread {
        let project_index = self.focused_project;
        let active_thread = self.projects[project_index].active_thread;
        &mut self.projects[project_index].threads[active_thread]
    }

    /// The remembered model, effort, and service tier for one harness.
    fn harness_choices(
        &self,
        harness: AgentHarnessChoice,
    ) -> (Option<String>, Option<String>, Option<String>) {
        match harness {
            AgentHarnessChoice::ClaudeCode => (
                self.claude_model.clone(),
                self.claude_effort.clone(),
                self.claude_tier.clone(),
            ),
            AgentHarnessChoice::Codex => (
                self.codex_model.clone(),
                self.codex_effort.clone(),
                self.codex_tier.clone(),
            ),
        }
    }

    pub(crate) fn start_agent_turn(&mut self) {
        let project_index = self.focused_project;
        let thread_index = self.projects[project_index].active_thread;
        let message = self.projects[project_index].threads[thread_index]
            .input
            .trim()
            .to_owned();
        if message.is_empty() || self.projects[project_index].threads[thread_index].running {
            return;
        }
        let Some(endpoint) = self.projects[project_index]
            .mcp_server
            .as_ref()
            .map(|server| server.endpoint().to_owned())
        else {
            self.record_error("Agent", "The OpenReel agent server is unavailable");
            return;
        };
        let harness = self.projects[project_index].threads[thread_index].harness;
        let harness_info = match harness {
            AgentHarnessChoice::ClaudeCode => self.claude_info.as_ref(),
            AgentHarnessChoice::Codex => self.codex_info.as_ref(),
        };
        if harness_info.is_none() {
            self.record_error(
                "Agent",
                format!("{} is not installed on PATH", harness.label()),
            );
            return;
        }

        if self.projects[project_index].threads[thread_index]
            .session
            .is_none()
        {
            self.projects[project_index].threads[thread_index]
                .usage
                .reset_usage();
            let working_directory = self.projects[project_index]
                .project_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok());
            let (model, effort, service_tier) = self.harness_choices(harness);
            let config = SessionConfig {
                working_directory,
                model,
                effort,
                service_tier,
                // Subscription harnesses are flat fee and the Stop button is
                // always available, so sessions run without a turn ceiling.
                max_turns: None,
                mcp_url: Some(endpoint),
            };
            let session = match harness {
                AgentHarnessChoice::ClaudeCode => ClaudeCodeDriver.start_session(config),
                AgentHarnessChoice::Codex => CodexDriver.start_session(config),
            };
            match session {
                Ok(session) => {
                    self.projects[project_index].threads[thread_index].events =
                        Some(session.events());
                    self.projects[project_index].threads[thread_index].session = Some(session);
                }
                Err(error) => {
                    self.record_error(
                        "Agent",
                        format!("Could not start {}: {error}", harness.label()),
                    );
                    return;
                }
            }
        }

        let result = self.projects[project_index].threads[thread_index]
            .session
            .as_mut()
            .expect("agent session was initialized")
            .send_user_message(message.clone());
        match result {
            Ok(()) => {
                let thread = &mut self.projects[project_index].threads[thread_index];
                let first_user_message = !thread
                    .chat
                    .iter()
                    .any(|entry| matches!(entry, ChatEntry::User(_)));
                if first_user_message {
                    thread.name = thread_title(&message);
                }
                thread.chat.push(ChatEntry::User(message));
                thread.input.clear();
                thread.running = true;
                thread.last_activity = Instant::now();
                self.status = format!("{} is editing the timeline", harness.label());
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
            template.clone_into(&mut self.active_thread_mut().input);
            self.start_agent_turn();
            return;
        }
        self.active_thread_mut()
            .chat
            .push(ChatEntry::User(format!("/{}", command.name)));
        match command.action {
            SlashAction::Import => self.choose_media(),
            SlashAction::RemoveFillers => self.remove_filler_words(),
            SlashAction::AddCaptions => self.add_captions(),
            SlashAction::FreezeFrame => self.freeze_frame_at_playhead(),
            SlashAction::Record => self.open_record_dialog(),
            SlashAction::Export => self.open_export_dialog(),
            SlashAction::Undo => self.undo(),
            SlashAction::Redo => self.redo(),
            SlashAction::Help => self
                .active_thread_mut()
                .chat
                .push(ChatEntry::Text(crate::slash::help_text())),
            SlashAction::Prompt(_) => {}
        }
    }

    pub(crate) fn stop_agent(&mut self, thread_index: usize) {
        let project_index = self.focused_project;
        let Some(thread) = self.projects[project_index].threads.get_mut(thread_index) else {
            return;
        };
        let had_session = thread.session.is_some();
        if let Some(session) = &mut thread.session {
            session.interrupt();
        }
        thread.session = None;
        thread.events = None;
        thread.running = false;
        if had_session && let Some(confirmations) = &self.projects[project_index].confirmations {
            confirmations.reject_all("the agent session was interrupted");
        }
        "Agent stopped".clone_into(&mut self.status);
    }

    pub(crate) fn poll_agent(&mut self, ctx: &egui::Context) {
        for project in &mut self.projects {
            if let Some(confirmations) = &project.confirmations {
                project
                    .pending_confirmations
                    .retain(|request| confirmations.is_pending(request.id));
                project
                    .pending_confirmations
                    .extend(confirmations.pending_requests());
            }
        }
        let events = self
            .projects
            .iter()
            .enumerate()
            .flat_map(|(project_index, project)| {
                project
                    .threads
                    .iter()
                    .enumerate()
                    .flat_map(move |(thread_index, thread)| {
                        thread.events.as_ref().map_or_else(Vec::new, |receiver| {
                            receiver
                                .try_iter()
                                .map(move |event| (project_index, thread_index, event))
                                .collect::<Vec<_>>()
                        })
                    })
            })
            .collect::<Vec<_>>();
        for (project_index, thread_index, event) in events {
            let thread = &mut self.projects[project_index].threads[thread_index];
            thread.usage.record(&event);
            thread.last_activity = Instant::now();
            match event {
                AgentEvent::Text(text) => {
                    self.projects[project_index].threads[thread_index]
                        .chat
                        .push(ChatEntry::Text(text));
                }
                AgentEvent::Error(error) => {
                    self.projects[project_index].threads[thread_index]
                        .chat
                        .push(ChatEntry::Text(error.clone()));
                    let project_name = self.projects[project_index].name.clone();
                    self.record_error("Agent", format!("{project_name}: {error}"));
                }
                AgentEvent::ToolCall { name, arguments } => {
                    self.projects[project_index].threads[thread_index]
                        .chat
                        .push(ChatEntry::ToolCall { name, arguments });
                }
                AgentEvent::ToolResult { name, result } => {
                    self.projects[project_index].threads[thread_index]
                        .chat
                        .push(ChatEntry::ToolResult { name, result });
                }
                AgentEvent::Cost {
                    input_tokens,
                    output_tokens,
                    ..
                } => self.projects[project_index].threads[thread_index]
                    .chat
                    .push(ChatEntry::Cost {
                        input_tokens,
                        output_tokens,
                    }),
                AgentEvent::Done => {
                    self.projects[project_index].threads[thread_index].running = false;
                    if project_index == self.focused_project {
                        "Agent turn finished".clone_into(&mut self.status);
                    }
                }
            }
        }
        if self
            .projects
            .iter()
            .any(|project| project.threads.iter().any(|thread| thread.running))
        {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    fn add_agent_thread(&mut self) {
        let project_index = self.focused_project;
        let harness = self.active_thread().harness;
        let next_number = self.projects[project_index].next_thread_number;
        self.projects[project_index].next_thread_number = self.projects[project_index]
            .next_thread_number
            .saturating_add(1);
        self.projects[project_index].threads.push(AgentThread::new(
            format!("Thread {next_number}"),
            harness,
            Vec::new(),
        ));
        self.projects[project_index].active_thread = self.projects[project_index].threads.len() - 1;
    }

    fn close_agent_thread(&mut self, thread_index: usize) {
        let project_index = self.focused_project;
        if self.projects[project_index].threads.len() <= 1
            || thread_index >= self.projects[project_index].threads.len()
        {
            return;
        }
        let next_active = index_after_close(
            self.projects[project_index].active_thread,
            thread_index,
            self.projects[project_index].threads.len(),
        );
        self.stop_agent(thread_index);
        self.projects[project_index].threads.remove(thread_index);
        self.projects[project_index].active_thread = next_active;
    }

    // The two-level rail is one immediate-mode render pass with deferred click actions.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn thread_rail(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.small_button("+ New project").clicked() {
                self.new_project();
            }
            if ui.small_button("+ New thread").clicked() {
                self.add_agent_thread();
                let project = self.focused();
                let composer_id =
                    egui::Id::new(("agent-composer", project.id, project.active_thread));
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(composer_id));
            }
        });
        ui.add_space(space::ONE);

        let mut focus_project = None;
        let mut close_project = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for project_index in 0..self.projects.len() {
                    let project = &self.projects[project_index];
                    let project_id = project.id;
                    let focused = project_index == self.focused_project;
                    let display_name = project.display_name();
                    let running = project.threads.iter().any(|thread| thread.running);
                    let confirms = !project.pending_confirmations.is_empty();
                    let can_close_project = self.projects.len() > 1;
                    let collapsed_caption = background_project_caption(project);
                    let mut close_clicked = false;
                    let frame = egui::Frame::new()
                        .fill(if focused {
                            color::SURFACE_RAISED
                        } else {
                            color::PANEL
                        })
                        .corner_radius(radius::SM)
                        .inner_margin(egui::Margin::same(theme::margin(space::ONE)))
                        .show(ui, |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), size::ICON_SM + space::ONE),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if can_close_project
                                        && ui
                                            .small_button("×")
                                            .on_hover_text("Close project")
                                            .clicked()
                                    {
                                        close_clicked = true;
                                    }
                                    if confirms {
                                        ui.colored_label(
                                            color::STATUS_WARNING,
                                            egui::RichText::new("CONFIRM").size(type_size::MICRO),
                                        );
                                    }
                                    if running {
                                        ui.colored_label(
                                            color::ACCENT,
                                            egui::RichText::new("RUNNING").size(type_size::MICRO),
                                        );
                                    }
                                    ui.with_layout(
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(display_name).strong(),
                                                )
                                                .truncate(),
                                            );
                                        },
                                    );
                                },
                            );
                            if !focused {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(collapsed_caption)
                                            .size(type_size::CAPTION)
                                            .color(color::TEXT_MUTED),
                                    )
                                    .truncate(),
                                );
                            }
                        });
                    let response = ui.interact(
                        frame.response.rect,
                        ui.make_persistent_id(("project-row", project_id)),
                        egui::Sense::click(),
                    );
                    if close_clicked {
                        close_project = Some(project_index);
                    } else if response.clicked() {
                        focus_project = Some(project_index);
                    }
                    if focused {
                        self.focused_thread_rows(ui, project_index);
                    }
                    ui.add_space(space::ONE);
                }
            });

        if let Some(index) = close_project {
            self.request_close_project(index);
        } else if let Some(index) = focus_project {
            self.focus_project(index);
        }
    }

    fn focused_thread_rows(&mut self, ui: &mut egui::Ui, project_index: usize) {
        let fps = self.projects[project_index].document.fps;
        let can_close = self.projects[project_index].threads.len() > 1;
        let mut focus_thread = None;
        let mut close_thread = None;
        ui.scope(|ui| {
            for (index, thread) in self.projects[project_index].threads.iter().enumerate() {
                let mut close_clicked = false;
                // Tree depth (M25): thread rows sit one indent step inside
                // their project header, so the two raised surfaces read as
                // parent and child rather than neighbors.
                let frame = egui::Frame::new()
                    .fill(if index == self.projects[project_index].active_thread {
                        color::SURFACE_RAISED
                    } else {
                        color::PANEL
                    })
                    .corner_radius(radius::SM)
                    .outer_margin(egui::Margin {
                        left: theme::margin(space::THREE),
                        ..egui::Margin::ZERO
                    })
                    .inner_margin(egui::Margin::same(theme::margin(space::ONE)))
                    .show(ui, |ui| {
                        // A bare with_layout claims the rail's full
                        // height; the row must allocate exactly one
                        // line. Trailing controls pack from the right,
                        // the identity anchors left and truncates.
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), size::ICON_SM + space::ONE),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if can_close
                                    && ui.small_button("×").on_hover_text("Close thread").clicked()
                                {
                                    close_clicked = true;
                                }
                                if thread.running {
                                    ui.colored_label(
                                        color::ACCENT,
                                        egui::RichText::new("RUNNING").size(type_size::MICRO),
                                    );
                                }
                                ui.with_layout(
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.add(thread.harness.brand_icon().image(size::ICON_SM));
                                        ui.add(egui::Label::new(&thread.name).truncate());
                                    },
                                );
                            },
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(latest_activity_snippet(&thread.chat, fps))
                                    .size(type_size::CAPTION)
                                    .color(color::TEXT_MUTED),
                            )
                            .truncate(),
                        );
                    });
                let row_response = ui.interact(
                    frame.response.rect,
                    ui.make_persistent_id(("thread-row", self.projects[project_index].id, index)),
                    egui::Sense::click(),
                );
                if close_clicked {
                    close_thread = Some(index);
                } else if row_response.clicked() {
                    focus_thread = Some(index);
                }
                ui.add_space(space::ONE_HALF);
            }
        });

        // Media enters through the project hub, T3-style: the focused
        // project's expanded section ends with a quiet import row, so the
        // media column never has to exist to get footage in.
        let mut import_media = false;
        egui::Frame::new()
            .outer_margin(egui::Margin {
                left: theme::margin(space::THREE),
                ..egui::Margin::ZERO
            })
            .show(ui, |ui| {
                let button = egui::Button::image_and_text(
                    Icon::Import.image(size::ICON_SM),
                    egui::RichText::new("Import media")
                        .size(type_size::CAPTION)
                        .color(color::TEXT_MUTED),
                )
                .image_tint_follows_text_color(true)
                .fill(egui::Color32::TRANSPARENT);
                if ui
                    .add(button)
                    .on_hover_text("Import a clip into this project (or drop a file anywhere)")
                    .clicked()
                {
                    import_media = true;
                }
            });
        if import_media {
            self.choose_media();
        }

        if let Some(index) = close_thread {
            self.close_agent_thread(index);
        } else if let Some(index) = focus_thread {
            self.projects[project_index].active_thread = index;
        }
    }

    // The agent panel is one ordered immediate-mode UI pass over session and confirmation state.
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    pub(crate) fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let project_index = self.focused_project;
        let project_id = self.projects[project_index].id;
        let active_thread = self.projects[project_index].active_thread;
        if !self.projects[project_index].threads[active_thread].running
            && self.projects[project_index].threads[active_thread]
                .session
                .is_none()
        {
            if self.claude_info.is_some() && self.codex_info.is_none() {
                self.projects[project_index].threads[active_thread].harness =
                    AgentHarnessChoice::ClaudeCode;
            } else if self.codex_info.is_some() && self.claude_info.is_none() {
                self.projects[project_index].threads[active_thread].harness =
                    AgentHarnessChoice::Codex;
            } else if self.claude_info.is_some() && self.codex_info.is_some() {
                let remembered = ui.ctx().data_mut(|data| {
                    data.get_persisted::<String>(egui::Id::new(AGENT_HARNESS_MEMORY_ID))
                });
                if let Some(remembered) =
                    remembered.and_then(|key| AgentHarnessChoice::from_key(&key))
                {
                    self.projects[project_index].threads[active_thread].harness = remembered;
                }
            }
            // Model and effort choices follow the same idle-restore pattern;
            // ids no longer valid for the current catalog (or the currently
            // chosen model) fall back to Default. An effort remembered for a
            // model that stops offering it resurfaces if the model returns.
            self.claude_model = restore_choice(ui.ctx(), CLAUDE_MODEL_MEMORY_ID, |id| {
                self.claude_models.iter().any(|model| model.id == id)
            });
            self.codex_model = restore_choice(ui.ctx(), CODEX_MODEL_MEMORY_ID, |id| {
                self.codex_models.iter().any(|model| model.id == id)
            });
            let claude_efforts = effort_options(&self.claude_models, self.claude_model.as_deref());
            let codex_efforts = effort_options(&self.codex_models, self.codex_model.as_deref());
            self.claude_effort = restore_choice(ui.ctx(), CLAUDE_EFFORT_MEMORY_ID, |effort| {
                claude_efforts.iter().any(|level| level == effort)
            });
            self.codex_effort = restore_choice(ui.ctx(), CODEX_EFFORT_MEMORY_ID, |effort| {
                codex_efforts.iter().any(|level| level == effort)
            });
            let claude_tiers = tier_options(&self.claude_models, self.claude_model.as_deref());
            let codex_tiers = tier_options(&self.codex_models, self.codex_model.as_deref());
            self.claude_tier = restore_choice(ui.ctx(), CLAUDE_TIER_MEMORY_ID, |id| {
                claude_tiers.iter().any(|tier| tier.id == id)
            });
            self.codex_tier = restore_choice(ui.ctx(), CODEX_TIER_MEMORY_ID, |id| {
                codex_tiers.iter().any(|tier| tier.id == id)
            });
        }
        let any_harness = self.claude_info.is_some() || self.codex_info.is_some();

        // No header chrome (M24): the stream is the surface, and the harness
        // controls live in the composer row like T3 Code's model row. The one
        // exception is the no-harness state, which explains itself up front.
        if !any_harness {
            chat_frame(color::SURFACE).show(ui, |ui| {
                harness_row(
                    ui,
                    Icon::BrandClaude,
                    "Claude Code",
                    self.claude_info.as_ref(),
                );
                harness_row(ui, Icon::BrandOpenAi, "Codex", self.codex_info.as_ref());
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

        let harness = self.projects[project_index].threads[active_thread].harness;
        let selected_info = match harness {
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
        // The summary rides the brand mark as a tooltip: inline it was the
        // first thing to collide once the thread rail narrowed the column.
        let harness_hover = harness_summary.map(|summary| {
            if harness == AgentHarnessChoice::Codex {
                format!("{summary}\n{CODEX_SANDBOX_NOTICE}")
            } else {
                summary
            }
        });

        let mut confirmation_decision = None;
        for request in &self.projects[project_index].pending_confirmations {
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
            && let Some(confirmations) = &self.projects[project_index].confirmations
        {
            let resolved = if approve {
                confirmations.approve(id)
            } else {
                confirmations.reject(id, "rejected by user")
            };
            if resolved {
                self.projects[project_index]
                    .pending_confirmations
                    .retain(|request| request.id != id);
                self.projects[project_index].threads[active_thread]
                    .chat
                    .push(ChatEntry::Text(if approve {
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
        // Everything below the stream (suggestion rows, input, controls) must
        // come out of the stream's share or it gets pushed out of the clipped
        // panel. Estimating those heights proved fragile, so the block is
        // MEASURED: reserve what it actually used last time at this
        // suggestion count, with a generous estimate covering only the first
        // frame a given count appears.
        let matches = crate::slash::matching_commands(
            &self.projects[project_index].threads[active_thread].input,
        );
        let reserve_id =
            egui::Id::new(("composer-reserve", project_id, active_thread, matches.len()));
        let composer_reserve = ui
            .ctx()
            .data(|data| data.get_temp::<f32>(reserve_id))
            .unwrap_or_else(|| {
                let suggestions = if matches.is_empty() {
                    0.0
                } else {
                    32.0 * matches.len() as f32 + 32.0
                };
                148.0 + suggestions
            });
        // The composer anchors to the bottom of the session column (T3-style):
        // the stream owns everything above it and sticks to its latest entry.
        let stream_height = (ui.available_height() - composer_reserve).max(96.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height(stream_height)
            .min_scrolled_height(stream_height)
            .show(ui, |ui| {
                // Machine activity collapses into one compact dropdown per
                // run (T3-style): the header keeps updating with the latest
                // step while the agent works, and expanding it reveals the
                // full cards, review actions included. Messages stay
                // first-class.
                let fps = self.projects[project_index].document.fps;
                let chat = &self.projects[project_index].threads[active_thread].chat;
                let mut index = 0;
                while index < chat.len() {
                    if is_activity(&chat[index]) {
                        let group_start = index;
                        while index < chat.len() && is_activity(&chat[index]) {
                            index += 1;
                        }
                        let entries = &chat[group_start..index];
                        egui::CollapsingHeader::new(
                            egui::RichText::new(activity_summary(entries, fps))
                                .size(type_size::CAPTION)
                                .color(color::TEXT_SECONDARY),
                        )
                        .id_salt(("activity", project_id, active_thread, group_start))
                        .default_open(false)
                        .show(ui, |ui| {
                            for (offset, entry) in entries.iter().enumerate() {
                                render_stream_entry(
                                    ui,
                                    entry,
                                    group_start + offset,
                                    fps,
                                    &mut card_action,
                                );
                                ui.add_space(space::ONE_HALF);
                            }
                        });
                    } else {
                        render_stream_entry(ui, &chat[index], index, fps, &mut card_action);
                        index += 1;
                    }
                    ui.add_space(space::ONE_HALF);
                }
            });
        match card_action {
            Some(EditCardAction::Review(cue)) => {
                self.seek_to(cue);
                self.playback.play(self.projects[project_index].position);
            }
            Some(EditCardAction::Undo) => self.undo(),
            None => {}
        }
        let composer_block_top = ui.cursor().top();
        // Slash suggestions float directly above the composer while typing.
        let mut run_command: Option<&'static crate::slash::SlashCommand> = None;
        if !matches.is_empty() {
            // The popup floats over the stream, so it earns one of the few
            // hairlines left in the app.
            chat_frame(color::SURFACE_RAISED)
                .stroke(egui::Stroke::new(1.0, color::BORDER_SUBTLE))
                .show(ui, |ui| {
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
        let composer_id = egui::Id::new(("agent-composer", project_id, active_thread));
        let input_response = egui::Frame::new()
            .fill(color::CANVAS)
            .stroke(egui::Stroke::new(1.0, color::BORDER_SUBTLE))
            .corner_radius(radius::MD)
            .inner_margin(egui::Margin::same(theme::margin(space::ONE)))
            .show(ui, |ui| {
                ui.add_enabled(
                    !self.projects[project_index].threads[active_thread].running,
                    egui::TextEdit::multiline(
                        &mut self.projects[project_index].threads[active_thread].input,
                    )
                    .id(composer_id)
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
            } else if !self.projects[project_index].threads[active_thread]
                .input
                .trim()
                .is_empty()
            {
                let input =
                    std::mem::take(&mut self.projects[project_index].threads[active_thread].input);
                input
                    .trim()
                    .clone_into(&mut self.projects[project_index].threads[active_thread].input);
                self.start_agent_turn();
            }
        }
        if let Some(command) = run_command {
            self.projects[project_index].threads[active_thread]
                .input
                .clear();
            self.run_slash_command(command);
        }
        // The composer row carries the session controls, T3-style: harness on
        // the left, transport on the right, everything else is the stream.
        ui.horizontal(|ui| {
            if self.claude_info.is_some() && self.codex_info.is_some() {
                let before = self.projects[project_index].threads[active_thread].harness;
                let mut choice = before;
                let icon = ui.add(before.brand_icon().image(size::ICON_SM));
                if let Some(hover) = &harness_hover {
                    icon.on_hover_text(hover);
                }
                ui.add_enabled_ui(
                    !self.projects[project_index].threads[active_thread].running,
                    |ui| {
                        egui::ComboBox::from_id_salt((
                            "composer-harness",
                            project_id,
                            active_thread,
                        ))
                        .selected_text(choice.label())
                        .show_ui(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add(Icon::BrandClaude.image(size::ICON_SM));
                                ui.selectable_value(
                                    &mut choice,
                                    AgentHarnessChoice::ClaudeCode,
                                    "Claude Code",
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.add(Icon::BrandOpenAi.image(size::ICON_SM));
                                ui.selectable_value(
                                    &mut choice,
                                    AgentHarnessChoice::Codex,
                                    "Codex",
                                );
                            });
                        });
                    },
                );
                if before != choice {
                    self.stop_agent(active_thread);
                    self.projects[project_index].threads[active_thread].harness = choice;
                    ui.ctx().data_mut(|data| {
                        data.insert_persisted(
                            egui::Id::new(AGENT_HARNESS_MEMORY_ID),
                            choice.key().to_owned(),
                        );
                    });
                }
            } else if any_harness {
                let icon = ui.add(
                    self.projects[project_index].threads[active_thread]
                        .harness
                        .brand_icon()
                        .image(size::ICON_SM),
                );
                if let Some(hover) = &harness_hover {
                    icon.on_hover_text(hover);
                }
                ui.colored_label(
                    color::TEXT_SECONDARY,
                    self.projects[project_index].threads[active_thread]
                        .harness
                        .label(),
                );
            }
            // Model picker for the selected harness. Default defers to the
            // CLI's configured model; a change restarts the session, same as
            // switching harnesses.
            if selected_available {
                let running = self.projects[project_index].threads[active_thread].running;
                let composer_harness = self.projects[project_index].threads[active_thread].harness;
                let (models, choice, memory_id) = match composer_harness {
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
                        .map_or("Model", |id| {
                            models
                                .iter()
                                .find(|model| model.id == id)
                                .map_or(id, |model| model.label.as_str())
                        })
                        .to_owned();
                    ui.add_enabled_ui(!running, |ui| {
                        egui::ComboBox::from_id_salt((
                            "composer-model",
                            project_id,
                            active_thread,
                            composer_harness.key(),
                        ))
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(choice, None, "Default");
                            for model in models {
                                ui.selectable_value(choice, Some(model.id.clone()), &model.label);
                            }
                        });
                    });
                    let changed = *choice != before;
                    let persisted = choice.clone().unwrap_or_default();
                    if changed {
                        self.stop_agent(active_thread);
                        ui.ctx().data_mut(|data| {
                            data.insert_persisted(egui::Id::new(memory_id), persisted);
                        });
                    }
                }
            }
            // Effort picker: only levels the chosen model supports (or that
            // every catalog model supports when the model is Default).
            if selected_available {
                let running = self.projects[project_index].threads[active_thread].running;
                let composer_harness = self.projects[project_index].threads[active_thread].harness;
                let (options, choice, memory_id) = match composer_harness {
                    AgentHarnessChoice::ClaudeCode => (
                        effort_options(&self.claude_models, self.claude_model.as_deref()),
                        &mut self.claude_effort,
                        CLAUDE_EFFORT_MEMORY_ID,
                    ),
                    AgentHarnessChoice::Codex => (
                        effort_options(&self.codex_models, self.codex_model.as_deref()),
                        &mut self.codex_effort,
                        CODEX_EFFORT_MEMORY_ID,
                    ),
                };
                if !options.is_empty() {
                    let before = choice.clone();
                    let selected_text = choice.as_deref().unwrap_or("Effort").to_owned();
                    ui.add_enabled_ui(!running, |ui| {
                        egui::ComboBox::from_id_salt((
                            "composer-effort",
                            project_id,
                            active_thread,
                            composer_harness.key(),
                        ))
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(choice, None, "Default");
                            for effort in &options {
                                ui.selectable_value(choice, Some(effort.clone()), effort);
                            }
                        });
                    });
                    let changed = *choice != before;
                    let persisted = choice.clone().unwrap_or_default();
                    if changed {
                        self.stop_agent(active_thread);
                        ui.ctx().data_mut(|data| {
                            data.insert_persisted(egui::Id::new(memory_id), persisted);
                        });
                    }
                }
            }
            // Speed picker: shown only when the harness catalog advertises
            // faster-than-standard service tiers (Codex's "Fast" = 1.5x at
            // increased usage). Standard is the reset entry, like Default.
            if selected_available {
                let running = self.projects[project_index].threads[active_thread].running;
                let composer_harness = self.projects[project_index].threads[active_thread].harness;
                let (options, choice, memory_id) = match composer_harness {
                    AgentHarnessChoice::ClaudeCode => (
                        tier_options(&self.claude_models, self.claude_model.as_deref()),
                        &mut self.claude_tier,
                        CLAUDE_TIER_MEMORY_ID,
                    ),
                    AgentHarnessChoice::Codex => (
                        tier_options(&self.codex_models, self.codex_model.as_deref()),
                        &mut self.codex_tier,
                        CODEX_TIER_MEMORY_ID,
                    ),
                };
                if !options.is_empty() {
                    let before = choice.clone();
                    let selected_text = choice
                        .as_deref()
                        .map_or("Speed", |id| {
                            options
                                .iter()
                                .find(|tier| tier.id == id)
                                .map_or(id, |tier| tier.name.as_str())
                        })
                        .to_owned();
                    ui.add_enabled_ui(!running, |ui| {
                        egui::ComboBox::from_id_salt((
                            "composer-tier",
                            project_id,
                            active_thread,
                            composer_harness.key(),
                        ))
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(choice, None, "Standard");
                            for tier in &options {
                                ui.selectable_value(choice, Some(tier.id.clone()), &tier.name);
                            }
                        });
                    });
                    let changed = *choice != before;
                    let persisted = choice.clone().unwrap_or_default();
                    if changed {
                        self.stop_agent(active_thread);
                        ui.ctx().data_mut(|data| {
                            data.insert_persisted(egui::Id::new(memory_id), persisted);
                        });
                    }
                }
            }
            if harness_hover.is_none() {
                ui.colored_label(
                    color::STATUS_DANGER,
                    format!(
                        "{} not found on PATH",
                        self.projects[project_index].threads[active_thread]
                            .harness
                            .label()
                    ),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // One transport slot (M25): Send while idle, Stop while
                // running. A permanently visible disabled twin is noise and
                // the row shares its width with three pickers.
                let running = self.projects[project_index].threads[active_thread].running;
                if running {
                    if ui
                        .add(
                            egui::Button::image(Icon::Stop.image(size::ICON_MD))
                                .image_tint_follows_text_color(true)
                                .fill(color::SURFACE_RAISED),
                        )
                        .on_hover_text("Stop this thread")
                        .clicked()
                    {
                        self.stop_agent(active_thread);
                    }
                    ui.colored_label(
                        color::ACCENT,
                        egui::RichText::new("RUNNING").size(type_size::MICRO),
                    );
                } else {
                    let can_send = !self.projects[project_index].threads[active_thread]
                        .input
                        .trim()
                        .is_empty()
                        && selected_available
                        && self.projects[project_index].mcp_server.is_some();
                    if ui
                        .add_enabled(
                            can_send,
                            egui::Button::image(Icon::Send.image(size::ICON_MD))
                                .image_tint_follows_text_color(true)
                                .fill(color::ACCENT_28),
                        )
                        .on_hover_text("Send (Enter)")
                        .clicked()
                    {
                        self.start_agent_turn();
                    }
                }
                let usage = &self.projects[project_index].threads[active_thread].usage;
                if usage.input_tokens > 0 || usage.output_tokens > 0 {
                    ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(format!(
                            "{} in / {} out",
                            usage.input_tokens, usage.output_tokens
                        ))
                        .size(type_size::MICRO),
                    );
                }
            });
        });
        // Record what the block below the stream actually used so the next
        // frame at this suggestion count reserves exactly that.
        let composer_block_height = ui.cursor().top() - composer_block_top;
        ui.ctx()
            .data_mut(|data| data.insert_temp(reserve_id, composer_block_height));
    }
}

fn thread_title(message: &str) -> String {
    truncate_text(message.trim(), 32)
}

fn latest_activity_snippet(chat: &[ChatEntry], fps: openreel_core::Rational) -> String {
    let Some(latest) = chat.last() else {
        return "New session".to_owned();
    };
    if is_activity(latest) {
        let start = chat
            .iter()
            .rposition(|entry| !is_activity(entry))
            .map_or(0, |index| index + 1);
        return activity_summary(&chat[start..], fps);
    }
    let (ChatEntry::User(text) | ChatEntry::Text(text)) = latest else {
        return "New session".to_owned();
    };
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_text(&one_line, 40)
}

fn background_project_caption(project: &ProjectSession) -> String {
    let activity = project
        .threads
        .iter()
        .filter(|thread| thread.running)
        .max_by_key(|thread| thread.last_activity)
        .map_or_else(
            || "idle".to_owned(),
            |thread| latest_activity_snippet(&thread.chat, project.document.fps),
        );
    let count = project.threads.len();
    format!(
        "{count} thread{} · {activity}",
        if count == 1 { "" } else { "s" }
    )
}

fn truncate_text(value: &str, maximum_chars: usize) -> String {
    if value.chars().count() <= maximum_chars {
        return value.to_owned();
    }
    if maximum_chars == 0 {
        return String::new();
    }
    let mut truncated = value
        .chars()
        .take(maximum_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

/// Whether a stream entry is machine activity (collapsed into a dropdown)
/// rather than a first-class message.
fn is_activity(entry: &ChatEntry) -> bool {
    matches!(
        entry,
        ChatEntry::ToolCall { .. }
            | ChatEntry::ToolResult { .. }
            | ChatEntry::Cost { .. }
            | ChatEntry::EditCard { .. }
    )
}

/// The collapsed group's header: the latest step, updating as work lands,
/// with a step count once the run grows.
fn activity_summary(entries: &[ChatEntry], fps: openreel_core::Rational) -> String {
    let steps = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                ChatEntry::ToolCall { .. } | ChatEntry::EditCard { .. }
            )
        })
        .count();
    let latest = entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            ChatEntry::EditCard { start, end, .. } => Some(format!(
                "Edited {} – {}",
                crate::timeline_ui::format_timecode(*start, fps),
                crate::timeline_ui::format_timecode(*end, fps),
            )),
            ChatEntry::ToolResult { name, .. } => Some(format!("Ran {name}")),
            ChatEntry::ToolCall { name, .. } => Some(format!("Running {name}")),
            _ => None,
        })
        .unwrap_or_else(|| "Working".to_owned());
    if steps > 1 {
        format!("{latest} · {steps} steps")
    } else {
        latest
    }
}

/// One stream entry, rendered the same whether it stands alone or sits
/// inside a collapsed activity group.
#[allow(clippy::too_many_lines)]
fn render_stream_entry(
    ui: &mut egui::Ui,
    entry: &ChatEntry,
    salt: usize,
    fps: openreel_core::Rational,
    card_action: &mut Option<EditCardAction>,
) {
    match entry {
        ChatEntry::User(text) => {
            chat_frame(color::ACCENT_16).show(ui, |ui| {
                ui.colored_label(
                    color::ACCENT,
                    egui::RichText::new("YOU").strong().size(type_size::MICRO),
                );
                ui.label(text);
            });
        }
        ChatEntry::Text(text) => {
            // The agent's words are the conversation itself: no container,
            // just the role label and prose (T3-style).
            ui.colored_label(
                color::TEXT_SECONDARY,
                egui::RichText::new("AGENT").strong().size(type_size::MICRO),
            );
            ui.label(text);
        }
        ChatEntry::ToolCall { name, arguments } => {
            chat_frame(color::SURFACE_RAISED).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(Icon::Waveform.image(size::ICON_SM).tint(color::TEXT_MUTED));
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
            });
        }
        ChatEntry::ToolResult { name, result } => {
            chat_frame(color::SURFACE).show(ui, |ui| {
                egui::CollapsingHeader::new(format!("RESULT · {name}"))
                    .id_salt(("agent-result", salt))
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
            chat_frame(color::SURFACE_RAISED).show(ui, |ui| {
                ui.colored_label(
                    color::TEXT_SECONDARY,
                    egui::RichText::new("EDIT").strong().size(type_size::MICRO),
                );
                ui.label(summary);
                ui.horizontal(|ui| {
                    if ui
                        .small_button("Review")
                        .on_hover_text("Play the changed span with a two-second lead-in")
                        .clicked()
                    {
                        *card_action = Some(EditCardAction::Review(*cue));
                    }
                    if ui
                        .small_button("Undo")
                        .on_hover_text("Revert this edit")
                        .clicked()
                    {
                        *card_action = Some(EditCardAction::Undo);
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{} – {}",
                            crate::timeline_ui::format_timecode(*start, fps),
                            crate::timeline_ui::format_timecode(*end, fps),
                        ))
                        .font(theme::code_font())
                        .color(color::TEXT_MUTED),
                    );
                });
            });
        }
    }
}

/// A remembered choice survives only while it is still on offer.
fn restore_choice(
    ctx: &egui::Context,
    memory_id: &str,
    is_valid: impl Fn(&str) -> bool,
) -> Option<String> {
    ctx.data_mut(|data| data.get_persisted::<String>(egui::Id::new(memory_id)))
        .filter(|id| is_valid(id))
}

/// The effort levels valid for the chosen model - or, when the model is the
/// CLI's (unknown) default, the levels every model in the catalog supports.
fn effort_options(models: &[openreel_agent::ModelChoice], model: Option<&str>) -> Vec<String> {
    match model {
        Some(id) => models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.efforts.clone())
            .unwrap_or_default(),
        None => openreel_agent::common_efforts(models),
    }
}

/// The service tiers valid for the chosen model, same default-model rule as
/// `effort_options`. Empty for providers without tiers, hiding the picker.
fn tier_options(
    models: &[openreel_agent::ModelChoice],
    model: Option<&str>,
) -> Vec<openreel_agent::ServiceTier> {
    match model {
        Some(id) => models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.tiers.clone())
            .unwrap_or_default(),
        None => openreel_agent::common_tiers(models),
    }
}

/// Stream containers are borderless fills (M25): the surface ladder carries
/// hierarchy, and outlines are reserved for floating popups and alerts.
fn chat_frame(fill: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(radius::MD)
        .inner_margin(egui::Margin::same(theme::margin(space::TWO)))
}

fn harness_row(ui: &mut egui::Ui, icon: Icon, name: &str, info: Option<&HarnessInfo>) {
    ui.horizontal(|ui| {
        ui.add(icon.image(size::ICON_SM));
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

    #[test]
    fn messages_are_not_activity_but_tool_traffic_and_edits_are() {
        assert!(!is_activity(&ChatEntry::User("cut it".to_owned())));
        assert!(!is_activity(&ChatEntry::Text("done".to_owned())));
        assert!(is_activity(&ChatEntry::ToolCall {
            name: "split_clip".to_owned(),
            arguments: "{}".to_owned(),
        }));
        assert!(is_activity(&ChatEntry::EditCard {
            summary: "Split clip".to_owned(),
            start: TimeCode(0),
            end: TimeCode(10),
            cue: TimeCode(0),
        }));
    }

    #[test]
    fn activity_headers_show_the_latest_step_and_count() {
        let fps = openreel_core::Rational::new(30, 1).unwrap();
        let call = |name: &str| ChatEntry::ToolCall {
            name: name.to_owned(),
            arguments: "{}".to_owned(),
        };
        let result = |name: &str| ChatEntry::ToolResult {
            name: name.to_owned(),
            result: "ok".to_owned(),
        };
        // An in-flight call reads as running; a finished one as ran.
        assert_eq!(
            activity_summary(&[call("get_timeline_state")], fps),
            "Running get_timeline_state"
        );
        assert_eq!(
            activity_summary(
                &[call("get_timeline_state"), result("get_timeline_state")],
                fps
            ),
            "Ran get_timeline_state"
        );
        // The newest edit wins the header, and steps count calls plus edits.
        let entries = [
            call("split_clip"),
            result("split_clip"),
            ChatEntry::EditCard {
                summary: "Split clip".to_owned(),
                start: TimeCode(60),
                end: TimeCode(120),
                cue: TimeCode(0),
            },
        ];
        assert_eq!(
            activity_summary(&entries, fps),
            "Edited 00:00:02:00 – 00:00:04:00 · 2 steps"
        );
    }

    #[test]
    fn thread_titles_truncate_to_32_characters_on_a_char_boundary() {
        assert_eq!(thread_title("  Cut the intro  "), "Cut the intro");
        let long = "é".repeat(33);
        let title = thread_title(&long);
        assert_eq!(title.chars().count(), 32);
        assert_eq!(title, format!("{}…", "é".repeat(31)));
    }

    #[test]
    fn closing_a_thread_keeps_or_moves_focus_predictably() {
        assert_eq!(index_after_close(2, 0, 4), 1);
        assert_eq!(index_after_close(1, 1, 4), 1);
        assert_eq!(index_after_close(3, 3, 4), 2);
        assert_eq!(index_after_close(0, 2, 4), 0);
    }

    #[test]
    fn latest_activity_snippets_cover_empty_messages_and_machine_runs() {
        let fps = openreel_core::Rational::new(30, 1).unwrap();
        assert_eq!(latest_activity_snippet(&[], fps), "New session");
        assert_eq!(
            latest_activity_snippet(
                &[ChatEntry::Text(
                    "A message with\nnewlines and   uneven spacing".to_owned()
                )],
                fps
            ),
            "A message with newlines and uneven spac…"
        );
        let chat = [
            ChatEntry::User("Do the edit".to_owned()),
            ChatEntry::ToolCall {
                name: "split_clip".to_owned(),
                arguments: "{}".to_owned(),
            },
            ChatEntry::ToolResult {
                name: "split_clip".to_owned(),
                result: "ok".to_owned(),
            },
        ];
        assert_eq!(latest_activity_snippet(&chat, fps), "Ran split_clip");
    }
}
