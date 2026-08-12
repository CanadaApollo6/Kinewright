use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use openreel_agent::{ClaudeCodeDriver, ConfirmationBroker, ConfirmationRequest, McpServer};
use openreel_core::{
    AgentDriver, Analysis, AssetId, ClipId, Core, Document, Event, MarkerId, Playback, TimeCode,
};

use crate::{
    chat_ui::{AgentHarnessChoice, AgentThread, ChatEntry},
    recovery::Recovery,
    transcript_ui::TranscriptSelection,
};

/// One independently editable project and all UI/agent state that must follow it.
pub(crate) struct ProjectSession {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) core: Core,
    pub(crate) core_events: crossbeam_channel::Receiver<Event>,
    pub(crate) mcp_server: Option<McpServer>,
    pub(crate) confirmations: Option<ConfirmationBroker>,
    pub(crate) pending_confirmations: Vec<ConfirmationRequest>,
    pub(crate) document: Arc<Document>,
    pub(crate) project_path: Option<PathBuf>,
    pub(crate) saved_document: Option<Arc<Document>>,
    pub(crate) recovery: Recovery,
    pub(crate) threads: Vec<AgentThread>,
    pub(crate) active_thread: usize,
    pub(crate) next_thread_number: usize,
    pub(crate) pending_timeline_adds: Vec<AssetId>,
    pub(crate) position: TimeCode,
    pub(crate) selected_clip: Option<ClipId>,
    pub(crate) selected_marker: Option<MarkerId>,
    pub(crate) selected_asset: Option<AssetId>,
    pub(crate) title_text_draft: Option<(ClipId, String)>,
    pub(crate) marker_label_draft: Option<(MarkerId, String)>,
    pub(crate) title_text_focus: Option<ClipId>,
    pub(crate) transcript_selection: Option<TranscriptSelection>,
    pub(crate) pixels_per_frame: f32,
    pub(crate) timeline_zoom_target: f32,
    pub(crate) timeline_scroll_target: f32,
}

impl ProjectSession {
    /// Build every actor, channel, recovery recorder, and initial thread for a project.
    pub(crate) fn create(
        id: u64,
        name: impl Into<String>,
        document: Document,
        project_path: Option<PathBuf>,
        playback: &Arc<dyn Playback>,
        analysis: &Arc<dyn Analysis>,
    ) -> Result<Self, String> {
        let core = Core::spawn(document.clone()).map_err(|error| error.to_string())?;
        let core_events = core.subscribe().map_err(|error| error.to_string())?;
        let mut chat = vec![ChatEntry::Text(
            "Import footage and describe your edit.".to_owned(),
        )];
        let mcp_server =
            match McpServer::start(core.clone(), Arc::clone(playback), Arc::clone(analysis)) {
                Ok(server) => Some(server),
                Err(error) => {
                    chat.push(ChatEntry::Text(format!(
                        "Could not start the OpenReel agent server: {error}"
                    )));
                    None
                }
            };
        let confirmations = mcp_server.as_ref().map(McpServer::confirmations);
        let agent_harness = if ClaudeCodeDriver.detect().is_some() {
            AgentHarnessChoice::ClaudeCode
        } else {
            AgentHarnessChoice::Codex
        };
        // Session 1 is the process-startup owner of the scanned pending list;
        // later sessions only attach their own recorder.
        let recovery = if id == 1 {
            Recovery::start(&core, project_path.as_deref())
        } else {
            Recovery::start_attached(&core, project_path.as_deref())
        };
        Ok(Self {
            id,
            name: name.into(),
            core,
            core_events,
            mcp_server,
            confirmations,
            pending_confirmations: Vec::new(),
            document: Arc::new(document),
            project_path,
            saved_document: None,
            recovery,
            threads: vec![AgentThread::new("Thread 1", agent_harness, chat)],
            active_thread: 0,
            next_thread_number: 2,
            pending_timeline_adds: Vec::new(),
            position: TimeCode::ZERO,
            selected_clip: None,
            selected_marker: None,
            selected_asset: None,
            title_text_draft: None,
            marker_label_draft: None,
            title_text_focus: None,
            transcript_selection: None,
            pixels_per_frame: 6.0,
            timeline_zoom_target: 6.0,
            timeline_scroll_target: 0.0,
        })
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.saved_document
            .as_deref()
            .is_none_or(|saved| saved != self.document.as_ref())
    }

    pub(crate) fn display_name(&self) -> String {
        project_display_name(self.project_path.as_deref(), &self.name, self.is_dirty())
    }

    pub(crate) fn stop_threads(&mut self, reason: &str) {
        for thread in &mut self.threads {
            if let Some(session) = &mut thread.session {
                session.interrupt();
            }
            thread.session = None;
            thread.events = None;
            thread.running = false;
        }
        if let Some(confirmations) = &self.confirmations {
            confirmations.reject_all(reason);
        }
        self.pending_confirmations.clear();
    }
}

pub(crate) trait HasSessionId {
    fn session_id(&self) -> u64;
}

impl HasSessionId for ProjectSession {
    fn session_id(&self) -> u64 {
        self.id
    }
}

/// Resolve a stable session identity after vector indices may have shifted.
pub(crate) fn session_index_by_id<T: HasSessionId>(
    session_id: u64,
    sessions: &[T],
) -> Option<usize> {
    sessions
        .iter()
        .position(|session| session.session_id() == session_id)
}

/// Keep a focused/active index valid after removing one item.
pub(crate) fn index_after_close(active: usize, closing: usize, count: usize) -> usize {
    debug_assert!(count > 1);
    debug_assert!(active < count);
    debug_assert!(closing < count);
    match closing.cmp(&active) {
        std::cmp::Ordering::Less => active - 1,
        std::cmp::Ordering::Equal => active.min(count - 2),
        std::cmp::Ordering::Greater => active,
    }
}

pub(crate) fn project_name(project_path: Option<&Path>, fallback: &str) -> String {
    project_path
        .and_then(Path::file_stem)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

pub(crate) fn project_display_name(
    project_path: Option<&Path>,
    fallback: &str,
    dirty: bool,
) -> String {
    let name = project_name(project_path, fallback);
    if dirty { format!("{name} *") } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubSession(u64);

    impl HasSessionId for StubSession {
        fn session_id(&self) -> u64 {
            self.0
        }
    }

    #[test]
    fn stable_session_id_routing_survives_index_changes() {
        let sessions = [StubSession(10), StubSession(30), StubSession(40)];
        assert_eq!(session_index_by_id(30, &sessions), Some(1));
        assert_eq!(session_index_by_id(20, &sessions), None);
        let shifted = [StubSession(30), StubSession(40)];
        assert_eq!(session_index_by_id(30, &shifted), Some(0));
    }

    #[test]
    fn closing_an_item_keeps_or_moves_focus_predictably() {
        assert_eq!(index_after_close(2, 0, 4), 1);
        assert_eq!(index_after_close(1, 1, 4), 1);
        assert_eq!(index_after_close(3, 3, 4), 2);
        assert_eq!(index_after_close(0, 2, 4), 0);
    }

    #[test]
    fn project_names_use_path_stems_fallbacks_and_dirty_markers() {
        assert_eq!(
            project_display_name(
                Some(Path::new("C:/cuts/Interview.openreel")),
                "Project 2",
                false
            ),
            "Interview"
        );
        assert_eq!(project_display_name(None, "Project 2", true), "Project 2 *");
        assert_eq!(project_name(Some(Path::new("")), "Fallback"), "Fallback");
    }
}
