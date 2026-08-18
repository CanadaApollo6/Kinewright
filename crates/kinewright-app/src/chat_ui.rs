use std::{
    collections::BTreeSet,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use eframe::egui;
use kinewright_agent::{
    BranchApplyOutcome, CODEX_SANDBOX_NOTICE, CURSOR_SANDBOX_NOTICE, ClaudeCodeDriver, CodexDriver,
    ConfirmationBroker, ConfirmationRequest, CursorAcpDriver, McpServer, TimelineBranch,
    compact_tool_names,
};
use kinewright_core::{
    AgentDriver, AgentEvent, AgentSession, Analysis, AuthenticationStatus, Command, Document,
    Event, Export, HarnessInfo, Playback, QaSeverity, SessionConfig, TimeCode, TimelineRevision,
    qa_document,
};
use serde::Serialize;

use crate::{
    app::KinewrightApp,
    icons::Icon,
    project::{ProjectSession, index_after_close},
    theme::{self, color, radius, size, space, type_size},
};

const AGENT_HARNESS_MEMORY_ID: &str = "kinewright-agent-harness";
const CLAUDE_MODEL_MEMORY_ID: &str = "kinewright-agent-model-claude-code";
const CODEX_MODEL_MEMORY_ID: &str = "kinewright-agent-model-codex";
const CURSOR_MODEL_MEMORY_ID: &str = "kinewright-agent-model-cursor";
const CLAUDE_EFFORT_MEMORY_ID: &str = "kinewright-agent-effort-claude-code";
const CODEX_EFFORT_MEMORY_ID: &str = "kinewright-agent-effort-codex";
const CURSOR_EFFORT_MEMORY_ID: &str = "kinewright-agent-effort-cursor";
const CLAUDE_TIER_MEMORY_ID: &str = "kinewright-agent-tier-claude-code";
const CODEX_TIER_MEMORY_ID: &str = "kinewright-agent-tier-codex";
const CURSOR_TIER_MEMORY_ID: &str = "kinewright-agent-tier-cursor";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentHarnessChoice {
    ClaudeCode,
    Codex,
    Cursor,
}

impl AgentHarnessChoice {
    fn key(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "claude-code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    /// The harness's brand mark (already in its brand color - not tinted).
    pub(crate) fn brand_icon(self) -> Icon {
        match self {
            Self::ClaudeCode => Icon::BrandClaude,
            Self::Codex => Icon::BrandOpenAi,
            Self::Cursor => Icon::BrandCursor,
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
        cached_input_tokens: Option<u64>,
        output_tokens: u64,
        reasoning_output_tokens: Option<u64>,
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

enum BranchReviewAction {
    Review(Arc<Document>),
    Merge,
    CherryPick,
    Discard,
}

/// Session token usage. Dollar figures are deliberately not surfaced: the
/// supported harnesses run on flat-fee subscriptions, so a running cost
/// readout is noise; the per-thread Stop control remains available instead.
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageAccumulator {
    pub(crate) input: u64,
    pub(crate) cached_input: u64,
    pub(crate) output: u64,
    pub(crate) reasoning_output: u64,
}

impl UsageAccumulator {
    pub(crate) fn record(&mut self, event: &AgentEvent) {
        if let AgentEvent::Cost {
            input_tokens,
            cached_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            ..
        } = event
        {
            self.input = self.input.saturating_add(*input_tokens);
            self.cached_input = self
                .cached_input
                .saturating_add(cached_input_tokens.unwrap_or(0));
            self.output = self.output.saturating_add(*output_tokens);
            self.reasoning_output = self
                .reasoning_output
                .saturating_add(reasoning_output_tokens.unwrap_or(0));
        }
    }

    pub(crate) fn reset_usage(&mut self) {
        self.input = 0;
        self.cached_input = 0;
        self.output = 0;
        self.reasoning_output = 0;
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
    pub(crate) branch: TimelineBranch,
    pub(crate) mcp_server: Option<McpServer>,
    pub(crate) confirmations: Option<ConfirmationBroker>,
    pub(crate) pending_confirmations: Vec<ConfirmationRequest>,
    pub(crate) selected_operations: BTreeSet<usize>,
    pub(crate) provenance: BranchProvenance,
    last_activity: Instant,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProvenanceKind {
    Prompt,
    Inspection,
    Proof,
    ToolResult,
    Approval,
    OperationSnapshot,
    Qa,
    Decision,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProvenanceEvent {
    sequence: u64,
    kind: ProvenanceKind,
    detail: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct BranchProvenance {
    events: Vec<ProvenanceEvent>,
    #[serde(skip)]
    next_sequence: u64,
}

impl BranchProvenance {
    fn record(&mut self, kind: ProvenanceKind, detail: impl Into<String>) {
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.events.push(ProvenanceEvent {
            sequence: self.next_sequence,
            kind,
            detail: detail.into(),
        });
    }

    fn json(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|error| format!("could not serialize provenance: {error}"))
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl AgentThread {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        name: impl Into<String>,
        harness: AgentHarnessChoice,
        mut chat: Vec<ChatEntry>,
        base_revision: TimelineRevision,
        base_document: &Arc<Document>,
        playback: &Arc<dyn Playback>,
        analysis: &Arc<dyn Analysis>,
        exporter: &Arc<dyn Export>,
    ) -> Result<Self, String> {
        let name = name.into();
        let branch = TimelineBranch::new(name.clone(), base_revision, Arc::clone(base_document))
            .map_err(|error| error.to_string())?;
        let mcp_server = match McpServer::start_isolated_with_exporter(
            branch.core(),
            Arc::clone(playback),
            Arc::clone(analysis),
            Arc::clone(exporter),
        ) {
            Ok(server) => Some(server),
            Err(error) => {
                chat.push(ChatEntry::Text(format!(
                    "Could not start this thread's Kinewright agent server: {error}"
                )));
                None
            }
        };
        let confirmations = mcp_server.as_ref().map(McpServer::confirmations);
        Ok(Self {
            name,
            harness,
            session: None,
            events: None,
            running: false,
            input: String::new(),
            usage: UsageAccumulator::default(),
            chat,
            branch,
            mcp_server,
            confirmations,
            pending_confirmations: Vec::new(),
            selected_operations: BTreeSet::new(),
            provenance: BranchProvenance::default(),
            last_activity: Instant::now(),
        })
    }

    fn replace_branch(
        &mut self,
        base_revision: TimelineRevision,
        base_document: Arc<Document>,
        playback: Arc<dyn Playback>,
        analysis: Arc<dyn Analysis>,
        exporter: Arc<dyn Export>,
    ) -> Result<(), String> {
        if let Some(confirmations) = &self.confirmations {
            confirmations.reject_all("the agent branch was replaced");
        }
        self.pending_confirmations.clear();
        self.selected_operations.clear();
        let branch = TimelineBranch::new(self.name.clone(), base_revision, base_document)
            .map_err(|error| error.to_string())?;
        let server =
            McpServer::start_isolated_with_exporter(branch.core(), playback, analysis, exporter)
                .map_err(|error| error.to_string())?;
        self.confirmations = Some(server.confirmations());
        self.mcp_server = Some(server);
        self.branch = branch;
        Ok(())
    }
}

impl KinewrightApp {
    pub(crate) fn active_thread(&self) -> &AgentThread {
        let project_index = self.focused_project;
        &self.projects[project_index].threads[self.projects[project_index].active_thread]
    }

    pub(crate) fn active_thread_mut(&mut self) -> &mut AgentThread {
        let project_index = self.focused_project;
        let active_thread = self.projects[project_index].active_thread;
        &mut self.projects[project_index].threads[active_thread]
    }

    /// The model the Codex side of the picker effectively runs: the chosen
    /// one, else the CLI config's default - so "Default" offers that model's
    /// real efforts and tiers instead of a lowest-common-denominator set.
    fn codex_model_or_default(&self) -> Option<&str> {
        self.codex_model
            .as_deref()
            .or(self.codex_default_model.as_deref())
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
            AgentHarnessChoice::Cursor => (
                self.cursor_model.clone(),
                self.cursor_effort.clone(),
                self.cursor_tier.clone(),
            ),
        }
    }

    #[allow(clippy::too_many_lines)]
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
        let refresh_branch = self.projects[project_index].threads[thread_index]
            .branch
            .compare()
            .is_ok_and(|comparison| {
                comparison.operations.is_empty()
                    && comparison.base_revision != self.projects[project_index].revision
            });
        if refresh_branch {
            self.stop_agent(thread_index);
            let revision = self.projects[project_index].revision;
            let document = Arc::clone(&self.projects[project_index].document);
            if !self.replace_agent_branch(project_index, thread_index, revision, document) {
                return;
            }
        }
        let Some(endpoint) = self.projects[project_index].threads[thread_index]
            .mcp_server
            .as_ref()
            .map(|server| server.endpoint().to_owned())
        else {
            self.record_error("Agent", "The Kinewright agent server is unavailable");
            return;
        };
        let harness = self.projects[project_index].threads[thread_index].harness;
        let harness_info = match harness {
            AgentHarnessChoice::ClaudeCode => self.claude_info.as_ref(),
            AgentHarnessChoice::Codex => self.codex_info.as_ref(),
            AgentHarnessChoice::Cursor => self.cursor_info.as_ref(),
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
                tool_names: Some(compact_tool_names()),
            };
            let session = match harness {
                AgentHarnessChoice::ClaudeCode => ClaudeCodeDriver.start_session(config),
                AgentHarnessChoice::Codex => CodexDriver.start_session(config),
                AgentHarnessChoice::Cursor => CursorAcpDriver.start_session(config),
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
                thread.chat.push(ChatEntry::User(message.clone()));
                thread
                    .provenance
                    .record(ProvenanceKind::Prompt, message.clone());
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
            SlashAction::Settings => self.settings_open = true,
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
        if had_session && let Some(confirmations) = &thread.confirmations {
            confirmations.reject_all("the agent session was interrupted");
        }
        "Agent stopped".clone_into(&mut self.status);
    }

    fn replace_agent_branch(
        &mut self,
        project_index: usize,
        thread_index: usize,
        revision: TimelineRevision,
        document: Arc<Document>,
    ) -> bool {
        let result = self.projects[project_index].threads[thread_index].replace_branch(
            revision,
            document,
            Arc::clone(&self.playback),
            Arc::clone(&self.analysis),
            Arc::clone(&self.exporter),
        );
        if let Err(error) = result {
            self.record_error("Agent branch", error);
            false
        } else {
            true
        }
    }

    fn record_applied_branch(
        &mut self,
        project_index: usize,
        thread_index: usize,
        before: &Document,
        after: &Document,
        summary: String,
    ) {
        if let Some(range) = crate::edit_diff::changed_project_range(before, after) {
            let cue = TimeCode(
                range
                    .start
                    .0
                    .saturating_sub(crate::app::review_preroll_frames(after.fps))
                    .max(0),
            );
            self.projects[project_index].threads[thread_index]
                .chat
                .push(ChatEntry::EditCard {
                    summary,
                    start: range.start,
                    end: range.end,
                    cue,
                });
            if project_index == self.focused_project {
                self.projects[project_index].position =
                    TimeCode(cue.0.min(after.duration.0.saturating_sub(1).max(0)));
            }
        } else {
            self.projects[project_index].threads[thread_index]
                .chat
                .push(ChatEntry::Text(summary));
        }
    }

    fn merge_agent_branch(&mut self, project_index: usize, thread_index: usize) {
        self.stop_agent(thread_index);
        let branch = self.projects[project_index].threads[thread_index]
            .branch
            .clone();
        let live = self.projects[project_index].core.clone();
        let before = Arc::clone(&self.projects[project_index].document);
        match branch.merge_into(&live) {
            Ok(BranchApplyOutcome::Applied {
                revision,
                document,
                operation_count,
            }) => {
                self.projects[project_index].threads[thread_index]
                    .provenance
                    .record(
                        ProvenanceKind::Decision,
                        format!(
                            "merged {operation_count} operation(s) at live revision {revision}"
                        ),
                    );
                self.record_applied_branch(
                    project_index,
                    thread_index,
                    &before,
                    &document,
                    format!("Merged {operation_count} branch edit(s)"),
                );
                let _ = self.replace_agent_branch(
                    project_index,
                    thread_index,
                    revision,
                    Arc::clone(&document),
                );
                self.status = format!("Merged {operation_count} agent edits");
            }
            Ok(BranchApplyOutcome::NoChanges) => {
                "The agent branch has no edits".clone_into(&mut self.status);
            }
            Ok(BranchApplyOutcome::Conflict { expected, actual }) => {
                let message = format!(
                    "Branch merge stopped: it was based on live revision {expected}, but live is now {actual}. Review and cherry-pick compatible operations or discard the branch."
                );
                self.projects[project_index].threads[thread_index]
                    .chat
                    .push(ChatEntry::Text(message.clone()));
                self.status = message;
            }
            Ok(BranchApplyOutcome::Rejected { error, .. }) => {
                self.record_error("Agent branch", format!("Branch merge rejected: {error}"));
            }
            Err(error) => self.record_error("Agent branch", error.to_string()),
        }
    }

    fn cherry_pick_agent_branch(&mut self, project_index: usize, thread_index: usize) {
        self.stop_agent(thread_index);
        let branch = self.projects[project_index].threads[thread_index]
            .branch
            .clone();
        let comparison = match branch.compare() {
            Ok(comparison) => comparison,
            Err(error) => {
                self.record_error("Agent branch", error.to_string());
                return;
            }
        };
        let selected = self.projects[project_index].threads[thread_index]
            .selected_operations
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let expected = self.projects[project_index].revision;
        let live = self.projects[project_index].core.clone();
        let before = Arc::clone(&self.projects[project_index].document);
        match branch.cherry_pick_into(&live, expected, &selected) {
            Ok(BranchApplyOutcome::Applied {
                revision,
                document,
                operation_count,
            }) => {
                let selected_set = selected.iter().copied().collect::<BTreeSet<_>>();
                let remaining = comparison
                    .operations
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !selected_set.contains(&(index + 1)))
                    .map(|(_, operation)| operation.clone())
                    .collect::<Vec<_>>();
                self.projects[project_index].threads[thread_index]
                    .provenance
                    .record(
                        ProvenanceKind::Decision,
                        format!("cherry-picked branch operations {selected:?} at {revision}"),
                    );
                self.record_applied_branch(
                    project_index,
                    thread_index,
                    &before,
                    &document,
                    format!("Cherry-picked {operation_count} branch edit(s)"),
                );
                if self.replace_agent_branch(
                    project_index,
                    thread_index,
                    revision,
                    Arc::clone(&document),
                ) && !remaining.is_empty()
                {
                    let event = self.projects[project_index].threads[thread_index]
                        .branch
                        .core()
                        .request(Command::DoBatchIfRevision {
                            expected: TimelineRevision::default(),
                            operations: remaining,
                        });
                    if !matches!(event, Ok(Event::DocumentChanged { .. })) {
                        self.projects[project_index].threads[thread_index]
                            .chat
                            .push(ChatEntry::Text(
                                "The selected edits were applied, but the remaining branch edits no longer form a valid plan on the new live cut. They were discarded."
                                    .to_owned(),
                            ));
                    }
                }
                self.status = format!("Cherry-picked {operation_count} agent edits");
            }
            Ok(BranchApplyOutcome::NoChanges) => {
                "Select at least one branch operation".clone_into(&mut self.status);
            }
            Ok(BranchApplyOutcome::Conflict { expected, actual }) => {
                self.status = format!(
                    "Cherry-pick stopped: expected live revision {expected}, actual {actual}"
                );
            }
            Ok(BranchApplyOutcome::Rejected { error, .. }) => {
                self.record_error("Agent branch", format!("Cherry-pick rejected: {error}"));
            }
            Err(error) => self.record_error("Agent branch", error.to_string()),
        }
    }

    fn discard_agent_branch(&mut self, project_index: usize, thread_index: usize) {
        self.stop_agent(thread_index);
        let revision = self.projects[project_index].revision;
        let document = Arc::clone(&self.projects[project_index].document);
        let operation_count = self.projects[project_index].threads[thread_index]
            .branch
            .compare()
            .map_or(0, |comparison| comparison.operations.len());
        self.projects[project_index].threads[thread_index]
            .provenance
            .record(
                ProvenanceKind::Decision,
                format!("discarded {operation_count} branch operation(s)"),
            );
        if self.replace_agent_branch(project_index, thread_index, revision, document) {
            self.projects[project_index].threads[thread_index]
                .chat
                .push(ChatEntry::Text(format!(
                    "Discarded {operation_count} unmerged branch edit(s)."
                )));
            "Agent branch discarded".clone_into(&mut self.status);
        }
    }

    fn review_agent_branch(
        &mut self,
        ctx: &egui::Context,
        project_index: usize,
        thread_index: usize,
        document: Arc<Document>,
    ) {
        if document.duration <= TimeCode::ZERO {
            "The branch timeline is empty".clone_into(&mut self.status);
            return;
        }
        let at = TimeCode(
            self.projects[project_index]
                .position
                .0
                .clamp(0, document.duration.0.saturating_sub(1)),
        );
        match self.analysis.thumbnail_for_document(document, at, 1_280) {
            Ok(frame) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &frame.pixels,
                );
                if let Some(texture) = &mut self.texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.texture = Some(ctx.load_texture(
                        "kinewright-branch-preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                self.projects[project_index].threads[thread_index]
                    .provenance
                    .record(
                        ProvenanceKind::Proof,
                        format!("reviewed branch frame {}", at.0),
                    );
                self.status = format!("Reviewing isolated branch frame {}", at.0);
            }
            Err(error) => self.record_error("Branch preview", error.to_string()),
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn poll_agent(&mut self, ctx: &egui::Context) {
        for project in &mut self.projects {
            for thread in &mut project.threads {
                if let Some(confirmations) = &thread.confirmations {
                    thread
                        .pending_confirmations
                        .retain(|request| confirmations.is_pending(request.id));
                    thread
                        .pending_confirmations
                        .extend(confirmations.pending_requests());
                }
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
                    let kind = if name == "get_frame_at" || name.contains("storyboard") {
                        ProvenanceKind::Proof
                    } else if name.starts_with("get_") {
                        ProvenanceKind::Inspection
                    } else {
                        ProvenanceKind::OperationSnapshot
                    };
                    self.projects[project_index].threads[thread_index]
                        .provenance
                        .record(kind, format!("{name}: {arguments}"));
                    self.projects[project_index].threads[thread_index]
                        .chat
                        .push(ChatEntry::ToolCall { name, arguments });
                }
                AgentEvent::ToolResult { name, result } => {
                    self.projects[project_index].threads[thread_index]
                        .provenance
                        .record(ProvenanceKind::ToolResult, format!("{name}: {result}"));
                    self.projects[project_index].threads[thread_index]
                        .chat
                        .push(ChatEntry::ToolResult { name, result });
                }
                AgentEvent::Cost {
                    input_tokens,
                    cached_input_tokens,
                    output_tokens,
                    reasoning_output_tokens,
                    ..
                } => self.projects[project_index].threads[thread_index]
                    .chat
                    .push(ChatEntry::Cost {
                        input_tokens,
                        cached_input_tokens,
                        output_tokens,
                        reasoning_output_tokens,
                    }),
                AgentEvent::Done => {
                    let thread = &mut self.projects[project_index].threads[thread_index];
                    thread.running = false;
                    if let Ok(comparison) = thread.branch.compare() {
                        let detail = serde_json::to_string(&*comparison.operations)
                            .unwrap_or_else(|error| error.to_string());
                        thread
                            .provenance
                            .record(ProvenanceKind::OperationSnapshot, detail);
                        let qa = qa_document(&comparison.document);
                        let detail =
                            serde_json::to_string(&qa).unwrap_or_else(|error| error.to_string());
                        thread.provenance.record(ProvenanceKind::Qa, detail);
                    }
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
        let base_revision = self.projects[project_index].revision;
        let base_document = Arc::clone(&self.projects[project_index].document);
        let thread = AgentThread::new(
            format!("Thread {next_number}"),
            harness,
            Vec::new(),
            base_revision,
            &base_document,
            &self.playback,
            &self.analysis,
            &self.exporter,
        );
        let Ok(thread) = thread else {
            self.record_error("Agent branch", "Could not create an isolated agent branch");
            return;
        };
        self.projects[project_index].threads.push(thread);
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

        // Settings lives in the rail's bottom corner, T3/Discord-style: the
        // rail is the app's hub, and identity/configuration anchors its foot.
        egui::Panel::bottom("rail-settings")
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(theme::margin(space::ONE))))
            .show_separator_line(false)
            .show(ui, |ui| {
                let button = egui::Button::image_and_text(
                    Icon::Settings.image(size::ICON_SM),
                    egui::RichText::new("Settings")
                        .size(type_size::CAPTION)
                        .color(color::TEXT_MUTED),
                )
                .image_tint_follows_text_color(true)
                .fill(egui::Color32::TRANSPARENT);
                if ui.add(button).clicked() {
                    self.settings_open = true;
                }
            });

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
                    let confirms = project
                        .threads
                        .iter()
                        .any(|thread| !thread.pending_confirmations.is_empty());
                    let can_close_project = self.projects.len() > 1;
                    let collapsed_caption = background_project_caption(project);
                    let mut close_clicked = false;
                    // Rail rows are a flat list, not cards (Riel's review):
                    // the focused row steps up one ladder fill and nothing
                    // pops, floats, or catches the light.
                    let frame = egui::Frame::new()
                        .fill(if focused {
                            color::SURFACE
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(radius::XS)
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
                                        ui.label(theme::caps_label(
                                            "CONFIRM",
                                            color::STATUS_WARNING,
                                        ));
                                    }
                                    if running {
                                        ui.label(theme::caps_label("RUNNING", color::ACCENT));
                                    }
                                    ui.with_layout(
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(display_name)
                                                        .font(theme::semibold(type_size::BODY)),
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
        let project_id = self.projects[project_index].id;
        let active_thread = self.projects[project_index].active_thread;
        let mut focus_thread = None;
        let mut close_thread = None;
        ui.scope(|ui| {
            for (index, thread) in self.projects[project_index].threads.iter().enumerate() {
                match show_thread_row(
                    ui,
                    thread,
                    project_id,
                    index,
                    index == active_thread,
                    can_close,
                    fps,
                ) {
                    ThreadRowAction::Close => close_thread = Some(index),
                    ThreadRowAction::Focus => focus_thread = Some(index),
                    ThreadRowAction::None => {}
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

    #[allow(clippy::too_many_lines)]
    fn branch_review_panel(
        &mut self,
        ui: &mut egui::Ui,
        project_index: usize,
        thread_index: usize,
    ) {
        let comparison = match self.projects[project_index].threads[thread_index]
            .branch
            .compare()
        {
            Ok(comparison) => comparison,
            Err(error) => {
                ui.colored_label(color::STATUS_DANGER, format!("Branch unavailable: {error}"));
                return;
            }
        };
        let provenance_empty = self.projects[project_index].threads[thread_index]
            .provenance
            .is_empty();
        if comparison.operations.is_empty() && provenance_empty {
            return;
        }
        let live_revision = self.projects[project_index].revision;
        let running = self.projects[project_index].threads[thread_index].running;
        let operation_summaries = comparison
            .operations
            .iter()
            .map(crate::app::operation_status)
            .collect::<Vec<_>>();
        let qa = qa_document(&comparison.document);
        let mut action = None;
        let card = egui::Frame::new()
            .fill(color::SURFACE_RAISED)
            .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER))
            .corner_radius(radius::MD)
            .inner_margin(egui::Margin::same(theme::margin(space::TWO)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::caps_label("ISOLATED BRANCH", color::ACCENT));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} edit(s) · base r{} · branch r{}",
                            comparison.operations.len(),
                            comparison.base_revision,
                            comparison.branch_revision,
                        ))
                        .size(type_size::MICRO)
                        .color(color::TEXT_MUTED),
                    );
                });
                if comparison.base_revision != live_revision {
                    ui.colored_label(
                        color::STATUS_WARNING,
                        format!(
                            "Live moved to r{live_revision}. Merge-all will remain blocked by revision safety; select compatible edits to cherry-pick."
                        ),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "QA Â· {} error(s) Â· {} warning(s) Â· {} note(s)",
                        qa.count(QaSeverity::Error),
                        qa.count(QaSeverity::Warning),
                        qa.count(QaSeverity::Info),
                    ))
                    .size(type_size::MICRO)
                    .color(if qa.export_ready() {
                        color::STATUS_SUCCESS
                    } else {
                        color::STATUS_DANGER
                    }),
                );
                if !qa.issues.is_empty() {
                    egui::CollapsingHeader::new("QA details").show(ui, |ui| {
                        for issue in &qa.issues {
                            ui.label(format!(
                                "{:?} Â· {} Â· {}",
                                issue.severity, issue.code, issue.message
                            ));
                        }
                    });
                }
                if !operation_summaries.is_empty() {
                    egui::CollapsingHeader::new("Review operations")
                        .default_open(true)
                        .show(ui, |ui| {
                            for (offset, summary) in operation_summaries.iter().enumerate() {
                                let index = offset + 1;
                                let mut selected = self.projects[project_index].threads
                                    [thread_index]
                                    .selected_operations
                                    .contains(&index);
                                if ui
                                    .checkbox(&mut selected, format!("{index}. {summary}"))
                                    .changed()
                                {
                                    if selected {
                                        self.projects[project_index].threads[thread_index]
                                            .selected_operations
                                            .insert(index);
                                    } else {
                                        self.projects[project_index].threads[thread_index]
                                            .selected_operations
                                            .remove(&index);
                                    }
                                }
                            }
                        });
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !running && comparison.document.duration > TimeCode::ZERO,
                            egui::Button::new("Review frame"),
                        )
                        .on_hover_text("Render this branch at the live playhead without changing live playback")
                        .clicked()
                    {
                        action = Some(BranchReviewAction::Review(Arc::clone(
                            &comparison.document,
                        )));
                    }
                    if ui
                        .add_enabled(
                            !running && !comparison.operations.is_empty(),
                            egui::Button::new("Merge all"),
                        )
                        .clicked()
                    {
                        action = Some(BranchReviewAction::Merge);
                    }
                    let selected_count = self.projects[project_index].threads[thread_index]
                        .selected_operations
                        .len();
                    if ui
                        .add_enabled(
                            !running && selected_count > 0,
                            egui::Button::new(format!("Cherry-pick {selected_count}")),
                        )
                        .clicked()
                    {
                        action = Some(BranchReviewAction::CherryPick);
                    }
                    if ui
                        .add_enabled(
                            !running && !comparison.operations.is_empty(),
                            egui::Button::new("Discard"),
                        )
                        .clicked()
                    {
                        action = Some(BranchReviewAction::Discard);
                    }
                });
                if !provenance_empty {
                    egui::CollapsingHeader::new("Provenance")
                        .id_salt(("branch-provenance", project_index, thread_index))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    self.projects[project_index].threads[thread_index]
                                        .provenance
                                        .json(),
                                )
                                .font(theme::code_font())
                                .color(color::TEXT_SECONDARY),
                            );
                        });
                }
            });
        theme::paint_raised_lighting(ui.painter(), card.response.rect, radius::px(radius::MD));
        ui.add_space(space::ONE);

        match action {
            Some(BranchReviewAction::Review(document)) => {
                self.review_agent_branch(ui.ctx(), project_index, thread_index, document);
            }
            Some(BranchReviewAction::Merge) => {
                self.merge_agent_branch(project_index, thread_index);
            }
            Some(BranchReviewAction::CherryPick) => {
                self.cherry_pick_agent_branch(project_index, thread_index);
            }
            Some(BranchReviewAction::Discard) => {
                self.discard_agent_branch(project_index, thread_index);
            }
            None => {}
        }
    }

    // The agent panel is one ordered immediate-mode UI pass over session and confirmation state.
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    pub(crate) fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let project_index = self.focused_project;
        let project_id = self.projects[project_index].id;
        let active_thread = self.projects[project_index].active_thread;
        // A provider is offered only when it is both detected and enabled in
        // Settings; a disabled provider vanishes from the picker but running
        // sessions on it are never interrupted.
        let claude_ready = self.claude_info.is_some()
            && crate::settings_ui::provider_enabled(ui.ctx(), AgentHarnessChoice::ClaudeCode);
        let codex_ready = self.codex_info.is_some()
            && crate::settings_ui::provider_enabled(ui.ctx(), AgentHarnessChoice::Codex);
        let cursor_ready = self.cursor_info.is_some()
            && crate::settings_ui::provider_enabled(ui.ctx(), AgentHarnessChoice::Cursor);
        let ready_harnesses = [
            (AgentHarnessChoice::ClaudeCode, claude_ready),
            (AgentHarnessChoice::Codex, codex_ready),
            (AgentHarnessChoice::Cursor, cursor_ready),
        ]
        .into_iter()
        .filter_map(|(harness, ready)| ready.then_some(harness))
        .collect::<Vec<_>>();
        if !self.projects[project_index].threads[active_thread].running
            && self.projects[project_index].threads[active_thread]
                .session
                .is_none()
        {
            if let [only] = ready_harnesses.as_slice() {
                self.projects[project_index].threads[active_thread].harness = *only;
            } else if ready_harnesses.len() > 1 {
                let remembered = ui.ctx().data_mut(|data| {
                    data.get_persisted::<String>(egui::Id::new(AGENT_HARNESS_MEMORY_ID))
                });
                if let Some(remembered) =
                    remembered.and_then(|key| AgentHarnessChoice::from_key(&key))
                    && ready_harnesses.contains(&remembered)
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
            self.cursor_model = restore_choice(ui.ctx(), CURSOR_MODEL_MEMORY_ID, |id| {
                self.cursor_models.iter().any(|model| model.id == id)
            });
            let claude_efforts = effort_options(&self.claude_models, self.claude_model.as_deref());
            let codex_efforts = effort_options(&self.codex_models, self.codex_model_or_default());
            let cursor_efforts = effort_options(&self.cursor_models, self.cursor_model.as_deref());
            self.claude_effort = restore_choice(ui.ctx(), CLAUDE_EFFORT_MEMORY_ID, |effort| {
                claude_efforts.iter().any(|level| level == effort)
            });
            self.codex_effort = restore_choice(ui.ctx(), CODEX_EFFORT_MEMORY_ID, |effort| {
                codex_efforts.iter().any(|level| level == effort)
            });
            self.cursor_effort = restore_choice(ui.ctx(), CURSOR_EFFORT_MEMORY_ID, |effort| {
                cursor_efforts.iter().any(|level| level == effort)
            });
            let claude_tiers = tier_options(&self.claude_models, self.claude_model.as_deref());
            let codex_tiers = tier_options(&self.codex_models, self.codex_model_or_default());
            let cursor_tiers = tier_options(&self.cursor_models, self.cursor_model.as_deref());
            self.claude_tier = restore_choice(ui.ctx(), CLAUDE_TIER_MEMORY_ID, |id| {
                claude_tiers.iter().any(|tier| tier.id == id)
            });
            self.codex_tier = restore_choice(ui.ctx(), CODEX_TIER_MEMORY_ID, |id| {
                codex_tiers.iter().any(|tier| tier.id == id)
            });
            self.cursor_tier = restore_choice(ui.ctx(), CURSOR_TIER_MEMORY_ID, |id| {
                cursor_tiers.iter().any(|tier| tier.id == id)
            });
        }
        let any_harness = !ready_harnesses.is_empty();

        // No header chrome (M24): the stream is the surface, and the harness
        // controls live in the composer row like T3 Code's model row. The one
        // exception is the no-harness state, which explains itself up front -
        // distinguishing "nothing installed" from "everything switched off".
        if !any_harness {
            let any_installed = self.claude_info.is_some()
                || self.codex_info.is_some()
                || self.cursor_info.is_some();
            chat_frame(color::SURFACE).show(ui, |ui| {
                if any_installed {
                    ui.label("Every provider is switched off.");
                    if ui.button("Open Settings").clicked() {
                        self.settings_open = true;
                    }
                } else {
                    harness_row(
                        ui,
                        Icon::BrandClaude,
                        "Claude Code",
                        self.claude_info.as_ref(),
                    );
                    harness_row(ui, Icon::BrandOpenAi, "Codex", self.codex_info.as_ref());
                    harness_row(ui, Icon::BrandCursor, "Cursor", self.cursor_info.as_ref());
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
                    ui.hyperlink_to(
                        "Install Cursor Agent",
                        "https://docs.cursor.com/en/cli/installation",
                    );
                }
            });
            ui.add_space(space::ONE);
        }

        let harness = self.projects[project_index].threads[active_thread].harness;
        let selected_info = match harness {
            AgentHarnessChoice::ClaudeCode => self.claude_info.as_ref(),
            AgentHarnessChoice::Codex => self.codex_info.as_ref(),
            AgentHarnessChoice::Cursor => self.cursor_info.as_ref(),
        };
        let selected_available = match harness {
            AgentHarnessChoice::ClaudeCode => claude_ready,
            AgentHarnessChoice::Codex => codex_ready,
            AgentHarnessChoice::Cursor => cursor_ready,
        };
        // Owned summary so the composer row can render it while self mutates.
        let harness_summary = selected_info.map(|info| {
            // CLI version strings often repeat the product name; keep the
            // number only.
            let version = info.version.as_deref().map_or("version unknown", |value| {
                value.split_whitespace().next().unwrap_or(value)
            });
            let tier = info
                .subscription_tier
                .as_ref()
                .map(|tier| format!(" · {tier}"))
                .unwrap_or_default();
            format!(
                "{} · {}{tier}",
                version,
                authentication_label(info.authentication)
            )
        });
        // The summary rides the brand mark as a tooltip: inline it was the
        // first thing to collide once the thread rail narrowed the column.
        let harness_hover = harness_summary.map(|summary| {
            if harness == AgentHarnessChoice::Codex {
                format!("{summary}\n{CODEX_SANDBOX_NOTICE}")
            } else if harness == AgentHarnessChoice::Cursor {
                format!("{summary}\n{CURSOR_SANDBOX_NOTICE}")
            } else {
                summary
            }
        });

        let mut confirmation_decision = None;
        for request in &self.projects[project_index].threads[active_thread].pending_confirmations {
            let card = egui::Frame::new()
                .fill(color::SURFACE_RAISED)
                .stroke(egui::Stroke::new(1.0, color::STATUS_WARNING))
                .corner_radius(radius::MD)
                .inner_margin(egui::Margin::same(theme::margin(space::TWO)))
                .show(ui, |ui| {
                    ui.label(theme::caps_label(
                        "AGENT CONFIRMATION REQUIRED",
                        color::STATUS_WARNING,
                    ));
                    ui.label(
                        egui::RichText::new(&request.tool_name)
                            .font(theme::semibold(type_size::BODY)),
                    );
                    ui.label(&request.description);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new("Approve")
                                    .fill(color::ACCENT_WASH)
                                    .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)),
                            )
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
            theme::paint_raised_lighting(ui.painter(), card.response.rect, radius::px(radius::MD));
        }
        if let Some((id, approve)) = confirmation_decision
            && let Some(confirmations) =
                &self.projects[project_index].threads[active_thread].confirmations
        {
            let resolved = if approve {
                confirmations.approve(id)
            } else {
                confirmations.reject(id, "rejected by user")
            };
            if resolved {
                self.projects[project_index].threads[active_thread]
                    .pending_confirmations
                    .retain(|request| request.id != id);
                let thread = &mut self.projects[project_index].threads[active_thread];
                thread.provenance.record(
                    ProvenanceKind::Approval,
                    if approve { "approved" } else { "rejected" },
                );
                thread.chat.push(ChatEntry::Text(if approve {
                    "Approved destructive edit.".to_owned()
                } else {
                    "Rejected destructive edit.".to_owned()
                }));
            }
        }

        self.branch_review_panel(ui, project_index, active_thread);

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
                // An untouched session gets an art-directed empty state
                // instead of one hint line above a void: quiet glyph, one
                // invitation, centered in the column (M28).
                if chat.len() <= 1 {
                    ui.add_space((stream_height * 0.5 - 64.0).max(0.0));
                    ui.vertical_centered(|ui| {
                        ui.add(Icon::Filmstrip.image(36.0).tint(color::TEXT_MUTED));
                        ui.add_space(space::TWO);
                        ui.label(
                            egui::RichText::new("Drop a clip anywhere")
                                .font(theme::semibold(type_size::HEADING))
                                .color(color::TEXT_SECONDARY),
                        );
                        ui.add_space(space::HALF);
                        ui.colored_label(color::TEXT_MUTED, "or /import - then describe your edit");
                    });
                    return;
                }
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
            // Slash suggestions sit on SURFACE_RAISED over PANEL; the fill
            // step is the edge (M28).
            let slash = chat_frame(color::SURFACE_RAISED).show(ui, |ui| {
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
            theme::paint_raised_lighting(ui.painter(), slash.response.rect, radius::px(radius::MD));
        }
        ui.add_space(space::ONE);
        let composer_id = egui::Id::new(("agent-composer", project_id, active_thread));
        let composer_focused = ui.ctx().memory(|memory| memory.has_focus(composer_id));
        // The composer is ONE card (Riel's review): the input is the card's
        // top face and the controls row its foot, sharing a fill with no seam
        // between them; a focus ring wraps the whole card while writing.
        let input_frame = egui::Frame::new()
            .fill(color::SURFACE)
            .corner_radius(egui::CornerRadius {
                nw: 6,
                ne: 6,
                sw: 0,
                se: 0,
            })
            .inner_margin(egui::Margin {
                left: theme::margin(space::TWO),
                right: theme::margin(space::TWO),
                top: theme::margin(space::TWO),
                bottom: theme::margin(space::ONE),
            })
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
            });
        let input_response = input_frame.inner;
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
        // Its card fill is painted after layout (rect known then) into a
        // placeholder shape reserved before the row draws, so the row and the
        // input above read as one continuous surface.
        ui.add_space(-ui.spacing().item_spacing.y);
        let controls_bg = ui.painter().add(egui::Shape::Noop);
        // Wrapped, not rigid: on narrow columns the pickers flow onto a
        // second line instead of clipping at the card's edge.
        let controls_row = ui.horizontal_wrapped(|ui| {
            // The row shares the card's inner margins on both sides: the max
            // width shrinks so the right-anchored Send keeps its inset (a
            // leading add_space in the RTL section pushes past the clip edge
            // instead), and the leading space insets the brand mark.
            ui.set_max_width(ui.available_width() - f32::from(theme::margin(space::TWO)));
            ui.add_space(f32::from(theme::margin(space::TWO)));
            if ready_harnesses.len() > 1 {
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
                            if claude_ready {
                                ui.horizontal(|ui| {
                                    ui.add(Icon::BrandClaude.image(size::ICON_SM));
                                    ui.selectable_value(
                                        &mut choice,
                                        AgentHarnessChoice::ClaudeCode,
                                        "Claude Code",
                                    );
                                });
                            }
                            if codex_ready {
                                ui.horizontal(|ui| {
                                    ui.add(Icon::BrandOpenAi.image(size::ICON_SM));
                                    ui.selectable_value(
                                        &mut choice,
                                        AgentHarnessChoice::Codex,
                                        "Codex",
                                    );
                                });
                            }
                            if cursor_ready {
                                ui.horizontal(|ui| {
                                    ui.add(Icon::BrandCursor.image(size::ICON_SM));
                                    ui.selectable_value(
                                        &mut choice,
                                        AgentHarnessChoice::Cursor,
                                        "Cursor",
                                    );
                                });
                            }
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
                    AgentHarnessChoice::Cursor => (
                        &self.cursor_models,
                        &mut self.cursor_model,
                        CURSOR_MODEL_MEMORY_ID,
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
                        effort_options(&self.codex_models, self.codex_model_or_default()),
                        &mut self.codex_effort,
                        CODEX_EFFORT_MEMORY_ID,
                    ),
                    AgentHarnessChoice::Cursor => (
                        effort_options(&self.cursor_models, self.cursor_model.as_deref()),
                        &mut self.cursor_effort,
                        CURSOR_EFFORT_MEMORY_ID,
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
                        tier_options(&self.codex_models, self.codex_model_or_default()),
                        &mut self.codex_tier,
                        CODEX_TIER_MEMORY_ID,
                    ),
                    AgentHarnessChoice::Cursor => (
                        tier_options(&self.cursor_models, self.cursor_model.as_deref()),
                        &mut self.cursor_tier,
                        CURSOR_TIER_MEMORY_ID,
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
                    ui.label(theme::caps_label("RUNNING", color::ACCENT));
                } else {
                    let can_send = !self.projects[project_index].threads[active_thread]
                        .input
                        .trim()
                        .is_empty()
                        && selected_available
                        && self.projects[project_index].threads[active_thread]
                            .mcp_server
                            .is_some();
                    if ui
                        .add_enabled(
                            can_send,
                            egui::Button::image(Icon::Send.image(size::ICON_MD))
                                .image_tint_follows_text_color(true)
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)),
                        )
                        .on_hover_text("Send (Enter)")
                        .clicked()
                    {
                        self.start_agent_turn();
                    }
                }
                let usage = &self.projects[project_index].threads[active_thread].usage;
                if usage.input > 0 || usage.output > 0 {
                    let cache = (usage.cached_input > 0)
                        .then(|| format!(" · {} cached", usage.cached_input));
                    ui.colored_label(
                        color::TEXT_MUTED,
                        egui::RichText::new(format!(
                            "{} in / {} out{}",
                            usage.input,
                            usage.output,
                            cache.as_deref().unwrap_or_default()
                        ))
                        .size(type_size::MICRO),
                    );
                }
            });
        });
        // The painted foot spans the input face's exact width regardless of
        // how the row's content laid out inside it.
        let mut controls_rect = controls_row
            .response
            .rect
            .expand2(egui::vec2(0.0, f32::from(theme::margin(space::ONE_HALF))));
        controls_rect.min.x = input_frame.response.rect.min.x;
        controls_rect.max.x = input_frame.response.rect.max.x;
        ui.painter().set(
            controls_bg,
            egui::Shape::rect_filled(
                controls_rect,
                egui::CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 6,
                    se: 6,
                },
                color::SURFACE,
            ),
        );
        if composer_focused {
            ui.painter().rect_stroke(
                input_frame.response.rect.union(controls_rect),
                radius::MD,
                theme::input_stroke(true),
                egui::StrokeKind::Outside,
            );
        }
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

fn latest_activity_snippet(chat: &[ChatEntry], fps: kinewright_core::Rational) -> String {
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
fn activity_summary(entries: &[ChatEntry], fps: kinewright_core::Rational) -> String {
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
    fps: kinewright_core::Rational,
    card_action: &mut Option<EditCardAction>,
) {
    match entry {
        ChatEntry::User(text) => {
            let card = chat_frame(color::SURFACE_RAISED).show(ui, |ui| {
                ui.label(theme::caps_label("YOU", color::TEXT_SECONDARY));
                ui.label(text);
            });
            theme::paint_raised_lighting(ui.painter(), card.response.rect, radius::px(radius::MD));
        }
        ChatEntry::Text(text) => {
            // The agent's words are the conversation itself: no container,
            // just the role label and prose (T3-style).
            ui.label(theme::caps_label("AGENT", color::TEXT_SECONDARY));
            ui.label(text);
        }
        ChatEntry::ToolCall { name, arguments } => {
            let card = chat_frame(color::SURFACE_RAISED).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(Icon::Waveform.image(size::ICON_SM).tint(color::TEXT_MUTED));
                    ui.label(theme::caps_prefix(
                        "TOOL",
                        &format!(" · {name}"),
                        color::TEXT_SECONDARY,
                    ));
                });
                ui.label(
                    egui::RichText::new(summarize(arguments, 180))
                        .font(theme::code_font())
                        .color(color::TEXT_SECONDARY),
                );
            });
            theme::paint_raised_lighting(ui.painter(), card.response.rect, radius::px(radius::MD));
        }
        ChatEntry::ToolResult { name, result } => {
            chat_frame(color::SURFACE).show(ui, |ui| {
                egui::CollapsingHeader::new(theme::caps_prefix(
                    "RESULT",
                    &format!(" · {name}"),
                    color::TEXT_SECONDARY,
                ))
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
            cached_input_tokens,
            output_tokens,
            reasoning_output_tokens,
        } => {
            let cache = cached_input_tokens
                .filter(|tokens| *tokens > 0)
                .map(|tokens| format!(" / {tokens} cached input"))
                .unwrap_or_default();
            let reasoning = reasoning_output_tokens
                .filter(|tokens| *tokens > 0)
                .map(|tokens| format!(" / {tokens} reasoning"))
                .unwrap_or_default();
            ui.colored_label(
                color::TEXT_MUTED,
                egui::RichText::new(format!(
                    "{input_tokens} input{cache} / {output_tokens} output{reasoning} tokens"
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
            let card = chat_frame(color::SURFACE_RAISED).show(ui, |ui| {
                ui.label(theme::caps_label("EDIT", color::TEXT_SECONDARY));
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
            theme::paint_raised_lighting(ui.painter(), card.response.rect, radius::px(radius::MD));
        }
    }
}

enum ThreadRowAction {
    None,
    Focus,
    Close,
}

fn show_thread_row(
    ui: &mut egui::Ui,
    thread: &AgentThread,
    project_id: u64,
    index: usize,
    active: bool,
    can_close: bool,
    fps: kinewright_core::Rational,
) -> ThreadRowAction {
    let mut close_clicked = false;
    // Tree depth (M25): thread rows sit one indent step inside
    // their project header, so the two raised surfaces read as
    // parent and child rather than neighbors.
    let frame = egui::Frame::new()
        .fill(if active {
            color::SURFACE
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(radius::XS)
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
                    if can_close && ui.small_button("×").on_hover_text("Close thread").clicked() {
                        close_clicked = true;
                    }
                    if thread.running {
                        ui.label(theme::caps_label("RUNNING", color::ACCENT));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(thread.harness.brand_icon().image(size::ICON_SM));
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&thread.name)
                                    .font(theme::semibold(type_size::BODY)),
                            )
                            .truncate(),
                        );
                    });
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
        ui.make_persistent_id(("thread-row", project_id, index)),
        egui::Sense::click(),
    );
    if close_clicked {
        ThreadRowAction::Close
    } else if row_response.clicked() {
        ThreadRowAction::Focus
    } else {
        ThreadRowAction::None
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
fn effort_options(models: &[kinewright_agent::ModelChoice], model: Option<&str>) -> Vec<String> {
    match model {
        Some(id) => models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.efforts.clone())
            .unwrap_or_default(),
        None => kinewright_agent::common_efforts(models),
    }
}

/// The service tiers valid for the chosen model, same default-model rule as
/// `effort_options`. Empty for providers without tiers, hiding the picker.
fn tier_options(
    models: &[kinewright_agent::ModelChoice],
    model: Option<&str>,
) -> Vec<kinewright_agent::ServiceTier> {
    match model {
        Some(id) => models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.tiers.clone())
            .unwrap_or_default(),
        None => kinewright_agent::common_tiers(models),
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
        let tier = info
            .subscription_tier
            .as_ref()
            .map(|tier| format!(" · {tier}"))
            .unwrap_or_default();
        ui.small(format!(
            "{} · {}{tier}",
            info.executable.display(),
            authentication_label(info.authentication)
        ));
    }
}

pub(crate) fn authentication_label(authentication: AuthenticationStatus) -> &'static str {
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
                cached_input_tokens: Some(80),
                cache_creation_input_tokens: Some(5),
                output_tokens: 20,
                reasoning_output_tokens: Some(12),
                cost_usd: Some(0.02),
            },
            AgentEvent::Text("not a cost".to_owned()),
            AgentEvent::Cost {
                input_tokens: 50,
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                output_tokens: 10,
                reasoning_output_tokens: None,
                cost_usd: None,
            },
            AgentEvent::Cost {
                input_tokens: 75,
                cached_input_tokens: Some(60),
                cache_creation_input_tokens: Some(3),
                output_tokens: 15,
                reasoning_output_tokens: Some(7),
                cost_usd: Some(0.03),
            },
        ];
        for event in &events {
            usage.record(event);
        }
        assert_eq!(usage.input, 225);
        assert_eq!(usage.cached_input, 140);
        assert_eq!(usage.output, 45);
        assert_eq!(usage.reasoning_output, 19);
        usage.reset_usage();
        assert_eq!(usage.input, 0);
        assert_eq!(usage.cached_input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.reasoning_output, 0);
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
        let fps = kinewright_core::Rational::new(30, 1).unwrap();
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
        let fps = kinewright_core::Rational::new(30, 1).unwrap();
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
