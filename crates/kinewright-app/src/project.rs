use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use kinewright_agent::{ClaudeCodeDriver, CursorAcpDriver};
use kinewright_core::{
    AgentDriver, Analysis, AssetId, ClipId, Core, Document, Event, Export, LutAssetId,
    LutAvailabilityKind, LutAvailabilityStatus, MarkerId, MediaKind, Playback, TimeCode,
    TimelineRevision, TrackId, TrackKind,
};
use kinewright_media::{LutLibrary, LutStore};

use crate::{
    chat_ui::{AgentHarnessChoice, AgentThread, ChatEntry},
    recovery::Recovery,
    transcript_ui::TranscriptSelection,
};

/// What one dialog-free project write did (CC4 §2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectSaveReport {
    /// Where the project JSON was written.
    pub(crate) path: PathBuf,
    /// The store root derived from `path`, absent only when the path yields no
    /// usable root.
    pub(crate) lut_store_root: Option<PathBuf>,
    /// Whether the store root moved, which is what makes a write a Save As for
    /// the purposes of the asset copy.
    pub(crate) store_root_changed: bool,
    /// One entry per asset the Save As copy could not place at the new root.
    /// A project with an unavailable asset is still saved; the asset is simply
    /// `missing` there, with the ordinary recovery path (CC4 §2.2).
    pub(crate) lut_store_copy_failed: Vec<(LutAssetId, String)>,
    /// The typed `lut_store_root_invalid` refusal when the derived root exists
    /// but is unusable — a symlink, or something that is not a directory.
    ///
    /// Kept rather than discarded so the caller can say *why* the just-saved
    /// project still cannot own LUT bytes: reporting `project_not_saved` on a
    /// project that was saved a second ago is a lie (CC4 §2.2).
    pub(crate) lut_store_error: Option<String>,
}

impl ProjectSaveReport {
    /// The human summary of any per-asset copy failure, or `None` when the
    /// whole store followed the project.
    pub(crate) fn copy_failure_summary(&self) -> Option<String> {
        if self.lut_store_copy_failed.is_empty() {
            return None;
        }
        Some(format!(
            "lut_store_copy_failed: {}",
            self.lut_store_copy_failed
                .iter()
                .map(|(asset, reason)| format!("asset {asset}: {reason}"))
                .collect::<Vec<_>>()
                .join("; ")
        ))
    }
}

/// Why a dialog-free project write failed.
///
/// Serialization and the JSON write are fatal; a store-root failure is not
/// reported here, because a project whose path yields no store root is still a
/// valid project — its imported looks are simply unavailable until it is saved
/// somewhere usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectSaveError {
    Serialize(String),
    Write(String),
}

impl std::fmt::Display for ProjectSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(reason) => {
                write!(formatter, "could not serialize the project: {reason}")
            }
            Self::Write(reason) => write!(formatter, "could not write the project file: {reason}"),
        }
    }
}

impl std::error::Error for ProjectSaveError {}

/// Derive a project's LUT store root (CC4 §2.2).
///
/// A project that has never been saved has no root at all, which is the
/// `project_not_saved` shape. A saved project whose derived root is a symlink
/// or a non-directory is a typed refusal, reported here as `Err` so the caller
/// can surface it rather than silently importing nowhere.
pub(crate) fn derive_lut_store(project_path: Option<&Path>) -> Result<Option<LutStore>, String> {
    match project_path {
        None => Ok(None),
        Some(path) => LutStore::for_project(path)
            .map(Some)
            .map_err(|error| error.to_string()),
    }
}

/// Why a project's look controls are disabled, or `None` when it can own LUT
/// bytes (CC4 §2.2).
///
/// A refused root is *not* the `project_not_saved` shape: the project was
/// saved, and telling the operator to save it again would send them round a
/// loop that cannot terminate. The typed `lut_store_root_invalid` reason wins
/// whenever there is one.
pub(crate) fn lut_store_unavailable_reason(
    has_store: bool,
    store_error: Option<&str>,
) -> Option<String> {
    if let Some(reason) = store_error {
        return Some(reason.to_owned());
    }
    if has_store {
        return None;
    }
    Some(crate::media_workflow::PROJECT_NOT_SAVED_MESSAGE.to_owned())
}

/// Whether `focus_project(index)` alone rebinds playback and republishes the
/// focused project's verified LUT library (CC4 §2.4).
///
/// It does not when the requested index is already the focused one, which is
/// exactly why the startup session — focused from the moment it is
/// constructed — has to publish its library explicitly rather than relying on
/// a focus call it can never satisfy.
pub(crate) const fn focus_publishes_lut_library(
    index: usize,
    len: usize,
    focused: usize,
    force_rebind: bool,
) -> bool {
    index < len && (force_rebind || index != focused)
}

/// Serialize one document to `path`, derive the new store, and copy every
/// referenced asset across when the store root moved (CC4 §2.2, §10.3.11).
///
/// This is the dialog-free half of Save/Save As. It is deliberately a free
/// function over borrowed state rather than a method on the app so the
/// relocatability fixture can drive the exact code the UI runs without an
/// eframe render state, a GPU adapter, or a window.
pub(crate) fn write_project_document(
    document: &Document,
    path: &Path,
    previous_store: Option<&LutStore>,
) -> Result<ProjectSaveReport, ProjectSaveError> {
    let json = serde_json::to_string_pretty(document)
        .map_err(|error| ProjectSaveError::Serialize(error.to_string()))?;
    fs::write(path, json).map_err(|error| ProjectSaveError::Write(error.to_string()))?;
    let (next_store, lut_store_error) = match derive_lut_store(Some(path)) {
        Ok(store) => (store, None),
        Err(reason) => (None, Some(reason)),
    };
    let store_root_changed = match (previous_store, next_store.as_ref()) {
        (Some(previous), Some(next)) => previous.root() != next.root(),
        (None, Some(_)) => true,
        _ => false,
    };
    let mut lut_store_copy_failed = Vec::new();
    if let (Some(previous), Some(next)) = (previous_store, next_store.as_ref())
        && store_root_changed
    {
        for (asset, result) in previous.copy_to(next, &document.lut_assets) {
            if let Err(error) = result {
                lut_store_copy_failed.push((asset, error.to_string()));
            }
        }
    }
    Ok(ProjectSaveReport {
        path: path.to_path_buf(),
        lut_store_root: next_store.map(|store| store.root().to_path_buf()),
        store_root_changed,
        lut_store_copy_failed,
        lut_store_error,
    })
}

/// Every LUT asset that is not `verified`, in document order, for the
/// inspector's status banner and the export gate's `project_not_saved` case.
pub(crate) fn unavailable_lut_assets(
    document: &Document,
    availability: &BTreeMap<LutAssetId, LutAvailabilityStatus>,
) -> Vec<LutAssetId> {
    document
        .lut_assets
        .iter()
        .filter(|asset| {
            availability
                .get(&asset.id)
                .is_none_or(|status| status.kind != LutAvailabilityKind::Verified)
        })
        .map(|asset| asset.id)
        .collect()
}

/// The one saved-project-path handle a session shares with every MCP server it
/// owns — the live one and every branch (CC4 §2.2, §8).
///
/// The servers derive their own `<stem>.kinewright-assets` root from it on
/// every use and never persist it, so a single write is how one Save As
/// becomes visible to every agent thread at once.
pub(crate) type ProjectPathHandle = std::sync::Arc<std::sync::RwLock<Option<PathBuf>>>;

/// One independently editable project and all UI/agent state that must follow it.
pub(crate) struct ProjectSession {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) core: Core,
    pub(crate) core_events: crossbeam_channel::Receiver<Event>,
    pub(crate) document: Arc<Document>,
    pub(crate) revision: TimelineRevision,
    pub(crate) project_path: Option<PathBuf>,
    /// The project-relative LUT store, derived from `project_path` at runtime
    /// and never serialized (CC4 §2.2). `None` for a project that has never
    /// been saved.
    pub(crate) lut_store: Option<LutStore>,
    /// The typed `lut_store_root_invalid` reason when this project has a path
    /// but its derived store root is unusable (CC4 §2.2). `None` covers both
    /// "never saved" and "the root is fine"; the disabled look controls tell
    /// those apart by also looking at `lut_store`.
    pub(crate) lut_store_error: Option<String>,
    /// The saved project path handle every MCP server in this session shares.
    ///
    /// Handed to each branch server at construction, so a thread created
    /// before the project was saved — or a branch replaced after a Save As —
    /// resolves exactly the store the live server does, with no window in
    /// which its look tools report `project_not_saved` on a saved project.
    pub(crate) agent_project_path: ProjectPathHandle,
    /// Runtime, never-serialized LUT availability, one entry per document
    /// asset, refreshed whenever the library is rebuilt (CC4 §2.3).
    pub(crate) lut_availability: BTreeMap<LutAssetId, LutAvailabilityStatus>,
    /// The most recently published library, kept so a card can report how many
    /// looks actually resolved without rebuilding.
    pub(crate) lut_library: Arc<LutLibrary>,
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
        // A store-root refusal on open is reported by the caller through the
        // ordinary error log; the session still opens, with imported looks
        // reporting `missing` until the root is usable. The reason is kept on
        // the session so every disabled look control can name it instead of
        // claiming the project was never saved (CC4 §2.2).
        let (lut_store, lut_store_error) = match derive_lut_store(project_path.as_deref()) {
            Ok(store) => (store, None),
            Err(reason) => (None, Some(reason)),
        };
        let (library, statuses) = LutLibrary::build(&document.lut_assets, lut_store.as_ref());
        let agent_project_path: ProjectPathHandle =
            std::sync::Arc::new(std::sync::RwLock::new(project_path.clone()));
        let session = Self {
            id,
            name,
            core,
            core_events,
            document: Arc::clone(&document),
            revision: TimelineRevision::default(),
            project_path,
            lut_store,
            lut_store_error,
            agent_project_path: std::sync::Arc::clone(&agent_project_path),
            lut_availability: statuses.into_iter().collect(),
            lut_library: Arc::new(library),
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
                &agent_project_path,
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
        };
        session.publish_project_path_to_agents();
        Ok(session)
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.saved_document
            .as_deref()
            .is_none_or(|saved| saved != self.document.as_ref())
    }

    /// Adopt a freshly derived store.
    ///
    /// The MCP servers do not read this: they derive their own root from the
    /// project path handle (`publish_project_path_to_agents`), so there is one
    /// derivation rule and no second copy of the root to fall out of date.
    pub(crate) fn set_lut_store(&mut self, store: Option<LutStore>, error: Option<String>) {
        self.lut_store = store;
        self.lut_store_error = error;
    }

    /// Why the look controls are disabled, or `None` when this project can own
    /// LUT bytes (CC4 §2.2).
    pub(crate) fn lut_store_unavailable_reason(&self) -> Option<String> {
        lut_store_unavailable_reason(self.has_lut_store(), self.lut_store_error.as_deref())
    }

    /// Rebuild the verified render-time library from the live document and the
    /// current store, recording one availability status per asset (CC4 §2.4).
    ///
    /// The library is rebuilt rather than patched because an asset's bytes are
    /// machine-local: a hash-verified rebuild is the only thing that can tell
    /// `verified` from `changed`.
    pub(crate) fn rebuild_lut_library(&mut self) -> Arc<LutLibrary> {
        let (library, statuses) =
            LutLibrary::build(&self.document.lut_assets, self.lut_store.as_ref());
        self.lut_availability = statuses.into_iter().collect();
        let library = Arc::new(library);
        self.lut_library = Arc::clone(&library);
        library
    }

    /// Whether this project can own LUT bytes at all (CC4 §2.2).
    pub(crate) const fn has_lut_store(&self) -> bool {
        self.lut_store.is_some()
    }

    /// Publish the saved project path to every agent server in this session.
    ///
    /// The servers derive their own store root from the path, so this handle
    /// is the only way `import_lut_asset` and `list_look_assets` learn where
    /// the project owns its bytes. Until a path is published they report
    /// `project_not_saved` rather than inventing a store (CC4 §8).
    ///
    /// Written to the shared handle rather than to each server, because a
    /// session whose live server failed to start still has to record the path
    /// for the branch servers a later thread creates from the same handle.
    pub(crate) fn publish_project_path_to_agents(&self) {
        if let Ok(mut slot) = self.agent_project_path.write() {
            slot.clone_from(&self.project_path);
        }
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
    use std::collections::BTreeMap;

    use super::*;
    use kinewright_core::{
        Clip, ClipContent, ColorDescription, Document, Effect, EffectId, LUT_ASSET_ID_PARAMETER,
        LutAsset, MediaAsset, MediaSourceFingerprint, Operation, ParamValue, Rational, Track,
        apply_batch,
    };
    use kinewright_media::{BuiltinLook, LutAssetImport, test_support::TempDirectory};

    /// A media backend that does nothing, so the real `AgentThread` seam can
    /// be driven without a GPU adapter, a decoder, or a window.
    struct StubMedia;

    impl Playback for StubMedia {
        fn set_document(&self, _document: Arc<Document>) {}
        fn request_frame(&self, _at: TimeCode) {}
        fn frames(&self) -> crossbeam_channel::Receiver<(TimeCode, kinewright_core::FrameTexture)> {
            crossbeam_channel::bounded(0).1
        }
        fn events(&self) -> crossbeam_channel::Receiver<kinewright_core::MediaEvent> {
            crossbeam_channel::bounded(0).1
        }
        fn play(&self, _from: TimeCode) {}
        fn pause(&self) {}
        fn seek(&self, _to: TimeCode) {}
        fn position(&self) -> TimeCode {
            TimeCode::ZERO
        }
        fn output_peaks(&self) -> [f32; 2] {
            [0.0, 0.0]
        }
    }

    impl Analysis for StubMedia {
        fn probe(&self, _path: &Path) -> Result<MediaAsset, kinewright_core::MediaError> {
            Err(kinewright_core::MediaError::NotImplemented)
        }
        fn thumbnail_at(
            &self,
            _at: TimeCode,
            _max_width: u32,
        ) -> Result<kinewright_core::RgbaImage, kinewright_core::MediaError> {
            Err(kinewright_core::MediaError::NotImplemented)
        }
        fn request_transcription(&self, _asset: MediaAsset) {}
        fn transcript_status(&self, _asset: &MediaAsset) -> kinewright_core::TranscriptStatus {
            kinewright_core::TranscriptStatus::NotRequested
        }
        fn timeline_transcript(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
        ) -> Result<Vec<kinewright_core::TimelineTranscriptWord>, kinewright_core::MediaError>
        {
            Ok(Vec::new())
        }
        fn request_silence_detection(&self, _asset: MediaAsset) {}
        fn silence_status(&self, _asset: &MediaAsset) -> kinewright_core::SilenceStatus {
            kinewright_core::SilenceStatus::NotRequested
        }
        fn timeline_silences(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_source_frames: TimeCode,
        ) -> Result<Vec<kinewright_core::TimelineSilenceSpan>, kinewright_core::MediaError>
        {
            Ok(Vec::new())
        }
        fn request_scene_detection(&self, _asset: MediaAsset) {}
        fn scene_status(&self, _asset: &MediaAsset) -> kinewright_core::SceneStatus {
            kinewright_core::SceneStatus::NotRequested
        }
        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<kinewright_core::TimelineSceneChange>, kinewright_core::MediaError>
        {
            Ok(Vec::new())
        }
        fn request_waveform(&self, _asset: MediaAsset, _request_generation: u64) -> bool {
            false
        }
        fn request_thumbnail(
            &self,
            _asset: MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
            _request_generation: u64,
        ) -> bool {
            false
        }
        fn visual_asset_results(
            &self,
        ) -> crossbeam_channel::Receiver<kinewright_core::VisualAssetResult> {
            crossbeam_channel::bounded(0).1
        }
    }

    impl Export for StubMedia {
        fn export(
            &self,
            _out: &Path,
            _settings: kinewright_core::ExportSettings,
            _progress: kinewright_core::ProgressSink,
        ) -> Result<(), kinewright_core::MediaError> {
            Err(kinewright_core::MediaError::NotImplemented)
        }
    }

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

    // -----------------------------------------------------------------------
    // CC4 §2.2 store derivation, §10.3.11 relocatable project proof
    // -----------------------------------------------------------------------

    /// A hand-made `S = 2`, `[0, 1]` identity `.cube` with non-trivial
    /// samples, written out literally rather than produced by the code under
    /// test, per the CC4 §10.1 fixture-quality rule.
    const SAMPLE_CUBE: &str = "TITLE \"Fixture look\"\n\
         LUT_3D_SIZE 2\n\
         DOMAIN_MIN 0.000000 0.000000 0.000000\n\
         DOMAIN_MAX 1.000000 1.000000 1.000000\n\
         0.000000 0.000000 0.000000\n\
         0.500000 0.000000 0.000000\n\
         0.000000 0.500000 0.000000\n\
         0.500000 0.500000 0.000000\n\
         0.000000 0.000000 0.500000\n\
         0.500000 0.000000 0.500000\n\
         0.000000 0.500000 0.500000\n\
         1.000000 1.000000 1.000000\n";

    /// Lattice samples as raw bits, so equality is bit-identity rather than a
    /// float comparison.
    fn bits(values: &[f32]) -> Vec<u32> {
        values.iter().map(|value| value.to_bits()).collect()
    }

    fn write_source_cube(directory: &TempDirectory, name: &str) -> PathBuf {
        let path = directory.path(name);
        fs::write(&path, SAMPLE_CUBE).expect("fixture .cube must be writable");
        path
    }

    /// A one-clip document carrying an imported LUT asset and a `creative_look`
    /// bound to it.
    fn look_document(asset: LutAsset) -> Document {
        let effect = Effect {
            id: EffectId(1),
            name: "creative_look".to_owned(),
            parameters: BTreeMap::from([(
                LUT_ASSET_ID_PARAMETER.to_owned(),
                ParamValue::Integer(
                    i64::try_from(asset.id.0).expect("a fixture id is far below 2^53 - 1"),
                ),
            )]),
            keyframes: BTreeMap::new(),
        };
        let mut document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: "shot.mov".into(),
                name: "Shot".to_owned(),
                duration: TimeCode(48),
                fps: Rational::new(24, 1).expect("valid fps"),
                kind: MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: MediaSourceFingerprint::default(),
                color_description: ColorDescription::default(),
            }],
            fps: Rational::new(24, 1).expect("valid fps"),
            resolution: (1920, 1080),
            lut_assets: vec![asset],
            ..Document::default()
        };
        document.tracks[0].clips.push(Clip {
            id: ClipId(1),
            asset: AssetId(1),
            timeline_start: TimeCode::ZERO,
            source_range: TimeCode::ZERO..TimeCode(48),
            content: ClipContent::Media,
            effects: vec![effect],
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        });
        document.duration = TimeCode(48);
        document.validate().expect("the fixture document is valid");
        document
    }

    #[test]
    fn the_store_root_is_the_project_stem_regardless_of_extension() {
        let temporary = TempDirectory::new("cc4-store-root");
        let kinewright = LutStore::for_project(&temporary.path("edit.kinewright"))
            .expect("a plain temp directory yields a store root");
        let json = LutStore::for_project(&temporary.path("edit.json"))
            .expect("a .json project derives the same stem");
        assert_eq!(kinewright.root(), json.root());
        assert_eq!(
            kinewright.root(),
            temporary.root().join("edit.kinewright-assets")
        );
        assert_eq!(
            kinewright.luts_dir(),
            temporary.root().join("edit.kinewright-assets").join("luts")
        );
    }

    /// CC4 §2.4: the startup session is index 0 and is focused from the
    /// moment it is constructed, so `focus_project(0)` early-returns and can
    /// never publish its library. Launching on a saved project with LUT nodes
    /// would render against an empty library, which is a silently wrong
    /// picture rather than an error.
    #[test]
    fn focusing_the_already_focused_startup_session_publishes_nothing() {
        assert!(
            !focus_publishes_lut_library(0, 1, 0, false),
            "the startup session must publish at construction, not through focus"
        );
        // A rebind — the forced focus a project close performs — does publish.
        assert!(focus_publishes_lut_library(0, 1, 0, true));
        // So does focusing a different project.
        assert!(focus_publishes_lut_library(1, 2, 0, false));
        // An out-of-range index never does, forced or not.
        assert!(!focus_publishes_lut_library(2, 2, 0, true));
    }

    /// The library the startup session builds is not empty for a saved
    /// project, which is what makes the missing publish a visible bug.
    #[test]
    fn a_startup_session_on_a_saved_project_builds_a_non_empty_library() {
        let temporary = TempDirectory::new("cc4-startup-publish");
        let project = temporary.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let import = store
            .import_lut_asset(&write_source_cube(&temporary, "look.cube"))
            .expect("the fixture .cube imports");
        let document = look_document(import.into_lut_asset(LutAssetId(1)));
        write_project_document(&document, &project, None).expect("write succeeds");

        // The open path `ProjectSession::create` runs.
        let reloaded: Document =
            serde_json::from_str(&fs::read_to_string(&project).expect("read")).expect("parse");
        let startup_store = derive_lut_store(Some(&project))
            .expect("the root is usable")
            .expect("a saved project has a root");
        let (library, statuses) = LutLibrary::build(&reloaded.lut_assets, Some(&startup_store));
        assert_eq!(library.len(), 1);
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Verified);
    }

    /// CC4 §2.2: a refused store root is not `project_not_saved`. The project
    /// *was* saved; telling the operator to save it again is a loop that
    /// cannot terminate, so the typed reason is kept and reported.
    #[test]
    fn a_refused_store_root_is_reported_with_its_reason_not_as_unsaved() {
        let temporary = TempDirectory::new("cc4-refused-root");
        let project = temporary.path("edit.kinewright");
        // A regular file sits exactly where the store directory belongs.
        fs::write(temporary.path("edit.kinewright-assets"), b"not a directory")
            .expect("the blocking file writes");

        let refusal = derive_lut_store(Some(&project)).expect_err("a file is not a store root");
        assert!(refusal.contains("lut_store_root_invalid: "), "{refusal}");
        assert!(refusal.contains("is not a directory"), "{refusal}");

        let report = write_project_document(&Document::default(), &project, None)
            .expect("the project is still saved");
        assert!(project.is_file(), "a refused root never blocks the save");
        assert_eq!(report.lut_store_root, None);
        assert_eq!(report.lut_store_error.as_deref(), Some(refusal.as_str()));

        // The disabled look controls name the refusal, not the save recovery.
        let message = lut_store_unavailable_reason(false, Some(&refusal))
            .expect("a refused root disables the look controls");
        assert_eq!(message, refusal);
        assert!(
            !message.contains("project_not_saved"),
            "a saved project must never be told to save itself: {message}"
        );

        // The same invariant at the export gate: a refused root blocks with
        // the typed refusal and the recovery that can clear it, never with the
        // save recovery (CC4 §2.2, §2.3).
        let gate = crate::export_ui::export_store_refusal_reason(&refusal);
        assert!(gate.contains("lut_store_root_invalid: "), "{gate}");
        assert!(
            gate.contains(crate::export_ui::EXPORT_LUT_STORE_ROOT_RECOVERY),
            "{gate}"
        );
        assert!(
            !gate.contains("project_not_saved"),
            "a saved project must never be told to save itself: {gate}"
        );
    }

    /// The other two shapes of the same decision.
    #[test]
    fn an_unsaved_project_still_reports_the_save_recovery_and_a_good_root_none() {
        assert_eq!(
            lut_store_unavailable_reason(false, None).as_deref(),
            Some(crate::media_workflow::PROJECT_NOT_SAVED_MESSAGE)
        );
        assert_eq!(lut_store_unavailable_reason(true, None), None);
    }

    #[test]
    fn a_usable_root_records_no_store_error() {
        let temporary = TempDirectory::new("cc4-usable-root");
        let report = write_project_document(
            &Document::default(),
            &temporary.path("edit.kinewright"),
            None,
        )
        .expect("write succeeds");
        assert!(report.lut_store_error.is_none());
        assert!(report.lut_store_root.is_some());
    }

    /// CC4 §2.2, §8: every MCP server a session owns — the live one and every
    /// branch — shares one saved-project-path handle, so a branch server can
    /// never be store-blind on a saved project and one Save As reaches every
    /// thread at once.
    ///
    /// Driven through the real `AgentThread::new` seam rather than through a
    /// hand-built `Arc`: the invariant is that `chat_ui.rs` starts every branch
    /// server *with the session's handle*, and only the server's own
    /// `project_path_handle` can witness that. A branch started with a fresh
    /// `Arc::new(RwLock::new(None))` still satisfies every property an
    /// `Arc::clone` chain has, which is exactly why the previous shape of this
    /// test passed with the wiring reverted.
    #[test]
    fn every_branch_server_shares_one_project_path_handle() {
        let temporary = TempDirectory::new("cc4-branch-path-handle");
        let project = temporary.path("edit.kinewright");

        let handle: ProjectPathHandle =
            std::sync::Arc::new(std::sync::RwLock::new(Some(project.clone())));
        let document = Arc::new(Document::default());
        let playback: Arc<dyn Playback> = Arc::new(StubMedia);
        let analysis: Arc<dyn Analysis> = Arc::new(StubMedia);
        let exporter: Arc<dyn Export> = Arc::new(StubMedia);
        let thread = AgentThread::new(
            "Thread 1",
            AgentHarnessChoice::Codex,
            Vec::new(),
            TimelineRevision::default(),
            &document,
            &playback,
            &analysis,
            &exporter,
            &handle,
        )
        .expect("the branch builds");
        let served = thread
            .mcp_server
            .as_ref()
            .expect("the branch server starts")
            .project_path_handle();
        assert!(
            std::sync::Arc::ptr_eq(&served, &handle),
            "a branch server must be started with the session's own handle, not a copy"
        );

        // Because it is the same handle, the branch already carries the path
        // the session was opened on: no window in which its look tools would
        // report `project_not_saved` on a saved project.
        assert_eq!(
            served.read().expect("readable").clone(),
            Some(project.clone())
        );

        // The store root the branch derives is the one the session derives.
        let session_root = derive_lut_store(Some(&project))
            .expect("usable")
            .expect("a saved project has a root");
        let branch_root = derive_lut_store(served.read().expect("readable").as_deref())
            .expect("usable")
            .expect("the branch resolves the same root");
        assert_eq!(session_root.root(), branch_root.root());

        // A Save As writes the session's handle once; the branch sees it with
        // no republishing.
        let moved = temporary.path("renamed.kinewright");
        *handle.write().expect("writable") = Some(moved.clone());
        assert_eq!(served.read().expect("readable").clone(), Some(moved));

        // Clearing it is the `project_not_saved` shape, for the branch too.
        *handle.write().expect("writable") = None;
        assert_eq!(served.read().expect("readable").clone(), None);
        assert_eq!(
            derive_lut_store(served.read().expect("readable").as_deref())
                .expect("no path is not a failure"),
            None
        );

        if let Some(server) = thread.mcp_server {
            server.shutdown();
        }
    }

    #[test]
    fn a_project_that_was_never_saved_has_no_store_root() {
        assert_eq!(
            derive_lut_store(None).expect("no path is not a failure"),
            None
        );
    }

    #[test]
    fn write_project_writes_json_and_derives_the_store() {
        let temporary = TempDirectory::new("cc4-write-project");
        let project = temporary.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let import = store
            .import_lut_asset(&write_source_cube(&temporary, "look.cube"))
            .expect("the fixture .cube imports");
        let sha256 = import.sha256.clone();
        let document = look_document(import.into_lut_asset(LutAssetId(1)));

        let report = write_project_document(&document, &project, None).expect("write succeeds");

        assert_eq!(report.path, project);
        assert_eq!(
            report.lut_store_root,
            Some(temporary.root().join("edit.kinewright-assets"))
        );
        assert!(report.lut_store_copy_failed.is_empty());
        let written = fs::read_to_string(&project).expect("the project file exists");
        let reloaded: Document = serde_json::from_str(&written).expect("valid project JSON");
        assert_eq!(reloaded.lut_assets.len(), 1);
        assert_eq!(reloaded.lut_assets[0].sha256, sha256);
        // The store file is content-addressed and lives beside the project.
        assert!(
            temporary
                .root()
                .join("edit.kinewright-assets")
                .join("luts")
                .join(format!("{sha256}.cube"))
                .is_file()
        );
    }

    /// CC4 §10.3.11: copying the project file plus one directory reproduces
    /// every look, and copying it without the directory reports `missing` with
    /// the expected store path rather than inventing a frame.
    #[test]
    fn a_project_relocates_with_its_store_and_reports_missing_without_it() {
        let origin = TempDirectory::new("cc4-relocate-origin");
        let project = origin.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let source_cube = write_source_cube(&origin, "look.cube");
        let import = store
            .import_lut_asset(&source_cube)
            .expect("the fixture .cube imports");
        let sha256 = import.sha256.clone();
        let asset = import.into_lut_asset(LutAssetId(1));
        let document = look_document(asset.clone());
        write_project_document(&document, &project, None).expect("write succeeds");

        // The origin resolves the look and reports it verified.
        let (origin_library, origin_statuses) =
            LutLibrary::build(&document.lut_assets, Some(&store));
        assert_eq!(origin_library.len(), 1);
        assert_eq!(origin_statuses[0].1.kind, LutAvailabilityKind::Verified);

        // Copy the project file *and* the store into a fresh directory whose
        // parent has a different name.
        let moved = TempDirectory::new("cc4-relocate-moved");
        let moved_project = moved.path("edit.kinewright");
        fs::copy(&project, &moved_project).expect("the project file copies");
        let moved_luts = moved.root().join("edit.kinewright-assets").join("luts");
        fs::create_dir_all(&moved_luts).expect("the store directory is creatable");
        fs::copy(
            store.luts_dir().join(format!("{sha256}.cube")),
            moved_luts.join(format!("{sha256}.cube")),
        )
        .expect("the store file copies");

        // The open-equivalent load: read the document, derive the store from
        // the *new* path, and build the library.
        let json = fs::read_to_string(&moved_project).expect("the copied project reads");
        let moved_document: Document = serde_json::from_str(&json).expect("valid project JSON");
        moved_document
            .validate()
            .expect("the copied project is valid");
        let moved_store = derive_lut_store(Some(&moved_project))
            .expect("the new root is usable")
            .expect("a saved project has a root");
        assert_ne!(moved_store.root(), store.root());
        let (moved_library, moved_statuses) =
            LutLibrary::build(&moved_document.lut_assets, Some(&moved_store));

        // The relocated project reproduces the look bit-identically. The two
        // handles may legitimately be the same `Arc`: CC4 §2.4 mandates a
        // parse cache keyed by `sha256`, so identical bytes resolve to one
        // in-process lattice no matter which store they were read through.
        // Independence from the *origin store* is proven by the bare case
        // below, which resolves nothing.
        assert_eq!(moved_library.len(), origin_library.len());
        let origin_lut = origin_library.get(LutAssetId(1)).expect("origin lattice");
        let moved_lut = moved_library.get(LutAssetId(1)).expect("moved lattice");
        assert_eq!(origin_lut.size, moved_lut.size);
        // Bit-identity, not tolerance: CC4 §2.2's relocatability rule is that
        // the look reproduces *bit-identically*, so no epsilon is admitted.
        assert_eq!(bits(&origin_lut.domain_min), bits(&moved_lut.domain_min));
        assert_eq!(bits(&origin_lut.domain_max), bits(&moved_lut.domain_max));
        assert_eq!(bits(&origin_lut.rgba), bits(&moved_lut.rgba));
        assert_eq!(moved_document.lut_assets[0].sha256, sha256);
        assert_eq!(moved_statuses[0].1.kind, LutAvailabilityKind::Verified);

        // Copy the project *without* the store: the asset is `missing` and the
        // status names the store path it was looked for at.
        let bare = TempDirectory::new("cc4-relocate-bare");
        let bare_project = bare.path("edit.kinewright");
        fs::copy(&project, &bare_project).expect("the project file copies");
        let bare_store = derive_lut_store(Some(&bare_project))
            .expect("the bare root is usable")
            .expect("a saved project has a root");
        let (bare_library, bare_statuses) =
            LutLibrary::build(&moved_document.lut_assets, Some(&bare_store));
        assert!(bare_library.get(LutAssetId(1)).is_none());
        assert_eq!(bare_statuses[0].1.kind, LutAvailabilityKind::Missing);
        assert_eq!(
            bare_statuses[0].1.path.as_deref(),
            Some(
                bare.root()
                    .join("edit.kinewright-assets")
                    .join("luts")
                    .join(format!("{sha256}.cube"))
                    .as_path()
            )
        );

        // Restore with the original file returns it to verified.
        bare_store
            .restore(&moved_document.lut_assets[0], &source_cube)
            .expect("the original bytes hash to the recorded identity");
        let (restored_library, restored_statuses) =
            LutLibrary::build(&moved_document.lut_assets, Some(&bare_store));
        assert_eq!(restored_statuses[0].1.kind, LutAvailabilityKind::Verified);
        assert_eq!(
            bits(
                &restored_library
                    .get(LutAssetId(1))
                    .expect("restored lattice")
                    .rgba
            ),
            bits(&origin_lut.rgba)
        );

        // Save As into a third directory copies the store again.
        let third = TempDirectory::new("cc4-relocate-third");
        let third_project = third.path("other-name.kinewright");
        let report = write_project_document(&moved_document, &third_project, Some(&bare_store))
            .expect("Save As succeeds");
        assert!(report.store_root_changed);
        assert!(
            report.lut_store_copy_failed.is_empty(),
            "Save As reported {:?}",
            report.lut_store_copy_failed
        );
        let third_store = derive_lut_store(Some(&third_project))
            .expect("usable")
            .expect("a saved project has a root");
        let (_, third_statuses) = LutLibrary::build(&moved_document.lut_assets, Some(&third_store));
        assert_eq!(third_statuses[0].1.kind, LutAvailabilityKind::Verified);
    }

    #[test]
    fn save_as_reports_the_asset_it_could_not_copy_and_still_saves() {
        let origin = TempDirectory::new("cc4-copy-failed-origin");
        let project = origin.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let import = store
            .import_lut_asset(&write_source_cube(&origin, "look.cube"))
            .expect("the fixture .cube imports");
        let sha256 = import.sha256.clone();
        let document = look_document(import.into_lut_asset(LutAssetId(1)));
        write_project_document(&document, &project, None).expect("write succeeds");
        // Remove the bytes the project claims to own, then Save As.
        fs::remove_file(store.luts_dir().join(format!("{sha256}.cube")))
            .expect("the store file is removable");

        let destination = TempDirectory::new("cc4-copy-failed-destination");
        let report = write_project_document(
            &document,
            &destination.path("edit.kinewright"),
            Some(&store),
        )
        .expect("a project with an unavailable asset is still saved");

        assert!(destination.path("edit.kinewright").is_file());
        assert_eq!(report.lut_store_copy_failed.len(), 1);
        assert_eq!(report.lut_store_copy_failed[0].0, LutAssetId(1));
        assert!(report.copy_failure_summary().is_some_and(|summary| {
            summary.contains("lut_store_copy_failed") && summary.contains("asset 1")
        }));
    }

    #[test]
    fn a_plain_save_over_the_same_path_copies_nothing() {
        let temporary = TempDirectory::new("cc4-plain-save");
        let project = temporary.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let import = store
            .import_lut_asset(&write_source_cube(&temporary, "look.cube"))
            .expect("import");
        let document = look_document(import.into_lut_asset(LutAssetId(1)));
        write_project_document(&document, &project, None).expect("first write");

        let report = write_project_document(&document, &project, Some(&store)).expect("re-save");

        assert!(!report.store_root_changed);
        assert!(report.lut_store_copy_failed.is_empty());
    }

    #[test]
    fn a_built_in_look_is_verified_without_any_store_file() {
        let temporary = TempDirectory::new("cc4-builtin-availability");
        let store = LutStore::for_project(&temporary.path("edit.kinewright")).expect("store root");
        let asset = BuiltinLook::Warm.to_lut_asset(LutAssetId(1));
        let (library, statuses) = LutLibrary::build(std::slice::from_ref(&asset), Some(&store));
        assert_eq!(statuses[0].1.kind, LutAvailabilityKind::Verified);
        assert!(library.get(LutAssetId(1)).is_some());
        assert!(!store.luts_dir().exists());
    }

    #[test]
    fn unavailable_assets_are_listed_in_document_order() {
        let missing = LutAsset {
            id: LutAssetId(2),
            ..BuiltinLook::Cool.to_lut_asset(LutAssetId(2))
        };
        let document = Document {
            lut_assets: vec![BuiltinLook::Warm.to_lut_asset(LutAssetId(1)), missing],
            ..Document::default()
        };
        let availability = BTreeMap::from([(
            LutAssetId(1),
            LutAvailabilityStatus {
                kind: LutAvailabilityKind::Verified,
                observed_sha256: None,
                reason: None,
                path: None,
            },
        )]);
        // Asset 2 has no observation at all, which is treated conservatively.
        assert_eq!(
            unavailable_lut_assets(&document, &availability),
            vec![LutAssetId(2)]
        );
    }

    #[test]
    fn an_imported_look_batch_round_trips_through_core_and_reopens() {
        let temporary = TempDirectory::new("cc4-batch-round-trip");
        let project = temporary.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let import: LutAssetImport = store
            .import_lut_asset(&write_source_cube(&temporary, "look.cube"))
            .expect("import");
        let mut document = look_document(import.clone().into_lut_asset(LutAssetId(1)));
        // A second import of the same bytes is the same store file and a
        // second record, which Core accepts under a fresh id.
        let second = import.into_lut_asset(document.next_lut_asset_id().expect("id space"));
        apply_batch(&mut document, &[Operation::AddLutAsset { asset: second }])
            .expect("AddLutAsset is accepted");
        assert_eq!(document.lut_assets.len(), 2);
        write_project_document(&document, &project, Some(&store)).expect("write");
        let reloaded: Document =
            serde_json::from_str(&fs::read_to_string(&project).expect("read")).expect("parse");
        assert_eq!(reloaded.lut_assets.len(), 2);
        reloaded.validate().expect("the reopened project is valid");
    }
}
