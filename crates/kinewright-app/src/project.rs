use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use kinewright_agent::{ClaudeCodeDriver, CursorAcpDriver};
use kinewright_core::{
    AgentDriver, Analysis, AssetId, ClipId, Core, Document, Event, Export, MarkerId, MediaKind,
    Playback, TimeCode, TimelineRevision, TrackId, TrackKind,
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
    pub(crate) document: Arc<Document>,
    pub(crate) revision: TimelineRevision,
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
    /// Ephemeral source-monitor cursor in the selected asset's frame domain.
    /// This intentionally never enters the serialized Document.
    pub(crate) source_position: TimeCode,
    pub(crate) source_in: TimeCode,
    pub(crate) source_out: TimeCode,
    /// Explicit source patch destinations. `None` means that component is
    /// disabled; a selected track is always validated against the live
    /// document before an edit is dispatched.
    pub(crate) source_video_target: Option<TrackId>,
    pub(crate) source_audio_target: Option<TrackId>,
    pub(crate) title_text_draft: Option<(ClipId, String)>,
    pub(crate) marker_label_draft: Option<(MarkerId, String)>,
    pub(crate) title_text_focus: Option<ClipId>,
    pub(crate) transcript_selection: Option<TranscriptSelection>,
    pub(crate) pixels_per_frame: f32,
    pub(crate) timeline_zoom_target: f32,
    pub(crate) timeline_scroll_target: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceMonitorState {
    selected_asset: Option<AssetId>,
    source_position: TimeCode,
    source_in: TimeCode,
    source_out: TimeCode,
    source_video_target: Option<TrackId>,
    source_audio_target: Option<TrackId>,
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
        exporter: &Arc<dyn Export>,
    ) -> Result<Self, String> {
        let name = name.into();
        let core = Core::spawn(document.clone()).map_err(|error| error.to_string())?;
        let core_events = core.subscribe().map_err(|error| error.to_string())?;
        let chat = vec![ChatEntry::Text(
            "Drop a clip anywhere (or /import), then describe your edit.".to_owned(),
        )];
        let agent_harness = if ClaudeCodeDriver.detect().is_some() {
            AgentHarnessChoice::ClaudeCode
        } else if CursorAcpDriver.detect().is_some() {
            AgentHarnessChoice::Cursor
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
        let document = Arc::new(document);
        Ok(Self {
            id,
            name,
            core,
            core_events,
            document: Arc::clone(&document),
            revision: TimelineRevision::default(),
            project_path,
            saved_document: None,
            recovery,
            threads: vec![AgentThread::new(
                "Thread 1",
                agent_harness,
                chat,
                TimelineRevision::default(),
                &document,
                playback,
                analysis,
                exporter,
            )?],
            active_thread: 0,
            next_thread_number: 2,
            pending_timeline_adds: Vec::new(),
            position: TimeCode::ZERO,
            selected_clip: None,
            selected_marker: None,
            selected_asset: None,
            source_position: TimeCode::ZERO,
            source_in: TimeCode::ZERO,
            source_out: TimeCode::ZERO,
            source_video_target: None,
            source_audio_target: None,
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

    /// Cue an asset in the Source viewer without changing the Program
    /// playhead. New source selections start with the complete source range
    /// and deterministic first-compatible patch destinations.
    pub(crate) fn cue_source_asset(&mut self, asset_id: AssetId) {
        self.apply_source_state(cue_source_state(
            &self.document,
            self.source_state(),
            asset_id,
        ));
    }

    /// Reconcile ephemeral Source state after a document revision. A route
    /// that disappeared or changed kind is cleared rather than silently
    /// retargeted; a later asset cue gets a visible deterministic default.
    pub(crate) fn reconcile_source_state(&mut self) {
        self.apply_source_state(reconcile_source_state(&self.document, self.source_state()));
    }

    fn source_state(&self) -> SourceMonitorState {
        SourceMonitorState {
            selected_asset: self.selected_asset,
            source_position: self.source_position,
            source_in: self.source_in,
            source_out: self.source_out,
            source_video_target: self.source_video_target,
            source_audio_target: self.source_audio_target,
        }
    }

    fn apply_source_state(&mut self, state: SourceMonitorState) {
        self.selected_asset = state.selected_asset;
        self.source_position = state.source_position;
        self.source_in = state.source_in;
        self.source_out = state.source_out;
        self.source_video_target = state.source_video_target;
        self.source_audio_target = state.source_audio_target;
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
            if let Some(confirmations) = &thread.confirmations {
                confirmations.reject_all(reason);
            }
            thread.pending_confirmations.clear();
        }
    }
}

fn empty_source_state() -> SourceMonitorState {
    SourceMonitorState {
        selected_asset: None,
        source_position: TimeCode::ZERO,
        source_in: TimeCode::ZERO,
        source_out: TimeCode::ZERO,
        source_video_target: None,
        source_audio_target: None,
    }
}

fn cue_source_state(
    document: &Document,
    mut state: SourceMonitorState,
    asset_id: AssetId,
) -> SourceMonitorState {
    let Some(asset) = document.asset(asset_id) else {
        return empty_source_state();
    };
    if state.selected_asset == Some(asset_id) {
        return reconcile_source_state(document, state);
    }
    state.selected_asset = Some(asset_id);
    state.source_position = TimeCode::ZERO;
    state.source_in = TimeCode::ZERO;
    state.source_out = asset.duration;
    state.source_video_target = first_compatible_track(document, asset.kind, TrackKind::Video);
    state.source_audio_target = first_compatible_track(document, asset.kind, TrackKind::Audio);
    state
}

fn reconcile_source_state(
    document: &Document,
    mut state: SourceMonitorState,
) -> SourceMonitorState {
    let Some(asset_id) = state.selected_asset else {
        return empty_source_state();
    };
    let Some(asset) = document.asset(asset_id) else {
        return empty_source_state();
    };
    state.source_position = TimeCode(
        state
            .source_position
            .0
            .clamp(0, asset.duration.0.saturating_sub(1).max(0)),
    );
    if asset.duration <= TimeCode::ZERO {
        state.source_in = TimeCode::ZERO;
        state.source_out = TimeCode::ZERO;
    } else {
        state.source_in = TimeCode(
            state
                .source_in
                .0
                .clamp(0, asset.duration.0.saturating_sub(1)),
        );
        state.source_out = TimeCode(
            state
                .source_out
                .0
                .clamp(state.source_in.0.saturating_add(1), asset.duration.0),
        );
    }
    state.source_video_target = valid_target(
        document,
        state.source_video_target,
        asset.kind,
        TrackKind::Video,
    );
    state.source_audio_target = valid_target(
        document,
        state.source_audio_target,
        asset.kind,
        TrackKind::Audio,
    );
    state
}

fn first_compatible_track(
    document: &Document,
    asset_kind: MediaKind,
    track_kind: TrackKind,
) -> Option<TrackId> {
    asset_kind.supports(track_kind).then(|| {
        document
            .tracks
            .iter()
            .find(|track| track.kind == track_kind)
            .map(|track| track.id)
    })?
}

fn valid_target(
    document: &Document,
    target: Option<TrackId>,
    asset_kind: MediaKind,
    track_kind: TrackKind,
) -> Option<TrackId> {
    target.filter(|target| {
        asset_kind.supports(track_kind)
            && document
                .tracks
                .iter()
                .any(|track| track.id == *target && track.kind == track_kind)
    })
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
    use kinewright_core::{
        ColorDescription, Document, MediaAsset, MediaSourceFingerprint, Rational, Track,
    };

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
                Some(Path::new("C:/cuts/Interview.kinewright")),
                "Project 2",
                false
            ),
            "Interview"
        );
        assert_eq!(project_display_name(None, "Project 2", true), "Project 2 *");
        assert_eq!(project_name(Some(Path::new("")), "Fallback"), "Fallback");
    }

    #[test]
    fn source_route_defaults_are_deterministic_and_kind_aware() {
        let document = Document {
            tracks: vec![
                Track {
                    id: TrackId(7),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(3),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            ],
            ..Document::default()
        };
        assert_eq!(
            first_compatible_track(&document, MediaKind::AudioVideo, TrackKind::Video),
            Some(TrackId(3))
        );
        assert_eq!(
            first_compatible_track(&document, MediaKind::Audio, TrackKind::Video),
            None
        );
        assert_eq!(
            valid_target(
                &document,
                Some(TrackId(3)),
                MediaKind::AudioVideo,
                TrackKind::Video
            ),
            Some(TrackId(3))
        );
        assert_eq!(
            valid_target(
                &document,
                Some(TrackId(7)),
                MediaKind::AudioVideo,
                TrackKind::Video
            ),
            None
        );
    }

    fn source_document() -> Document {
        Document {
            tracks: vec![
                Track {
                    id: TrackId(7),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(3),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            ],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: "shot.mov".into(),
                name: "Shot".to_owned(),
                duration: TimeCode(10),
                fps: Rational::new(24, 1).expect("valid fps"),
                kind: MediaKind::AudioVideo,
                resolution: Some((1920, 1080)),
                source_fingerprint: MediaSourceFingerprint::default(),
                color_description: ColorDescription::default(),
            }],
            ..Document::default()
        }
    }

    #[test]
    fn cueing_an_asset_resets_source_cursor_marks_and_routes() {
        let document = source_document();
        let state = SourceMonitorState {
            selected_asset: Some(AssetId(99)),
            source_position: TimeCode(8),
            source_in: TimeCode(4),
            source_out: TimeCode(9),
            source_video_target: Some(TrackId(99)),
            source_audio_target: Some(TrackId(99)),
        };
        let cued = cue_source_state(&document, state, AssetId(1));
        assert_eq!(cued.selected_asset, Some(AssetId(1)));
        assert_eq!(cued.source_position, TimeCode::ZERO);
        assert_eq!(cued.source_in, TimeCode::ZERO);
        assert_eq!(cued.source_out, TimeCode(10));
        assert_eq!(cued.source_video_target, Some(TrackId(3)));
        assert_eq!(cued.source_audio_target, Some(TrackId(7)));
    }

    #[test]
    fn source_revision_reconciliation_clamps_marks_and_clears_stale_routes() {
        let document = source_document();
        let state = SourceMonitorState {
            selected_asset: Some(AssetId(1)),
            source_position: TimeCode(99),
            source_in: TimeCode(99),
            source_out: TimeCode(999),
            source_video_target: Some(TrackId(99)),
            source_audio_target: Some(TrackId(3)),
        };
        let reconciled = reconcile_source_state(&document, state);
        assert_eq!(reconciled.source_position, TimeCode(9));
        assert_eq!(reconciled.source_in, TimeCode(9));
        assert_eq!(reconciled.source_out, TimeCode(10));
        assert_eq!(reconciled.source_video_target, None);
        assert_eq!(reconciled.source_audio_target, None);
    }
}
