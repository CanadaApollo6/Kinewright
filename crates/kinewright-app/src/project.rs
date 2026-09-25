use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime},
};

use kinewright_core::{
    Analysis, AssetId, AudioChain, ClipId, Core, Document, Event, Export, IncidentId, IncidentLog,
    IncidentState, IncidentSubject, LutAssetId, LutAvailabilityKind, LutAvailabilityStatus,
    MarkerId, MediaKind, Operation, Playback, RunningInvestigation, TimeCode, TimelineRevision,
    TrackId, TrackKind,
};
use kinewright_media::{LutLibrary, LutStore};
use kinewright_project::{
    FlushOutcome, RefuseRename, SidecarMode, SidecarSession, SidecarWriter, derive_lut_store,
    sidecar_write_failed_observation,
};

use crate::{
    chat_ui::{AgentHarnessChoice, AgentThread, ChatEntry},
    investigator::InvestigatorSession,
    recovery::Recovery,
    transcript_ui::TranscriptSelection,
};

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

/// The incident log handle a session shares with every MCP server it owns, in
/// the shape `ProjectPathHandle` already uses (IN1 §5.1 rule 2).
pub(crate) type IncidentLogHandle = std::sync::Arc<std::sync::RwLock<IncidentLog>>;

/// The memoized result of one [`ProjectSession::is_dirty`] deep comparison.
///
/// The keys are `Arc` clones, not raw pointers: holding the allocations alive
/// is what keeps `Arc::ptr_eq` on them sound against pointer reuse after a
/// drop, and the cache's own clones also keep a third party from mutating
/// either document through `Arc::get_mut` while a cached verdict is
/// outstanding (the strong count stays above one). No `unsafe` anywhere.
struct DirtyCacheEntry {
    current: Arc<Document>,
    saved: Arc<Document>,
    dirty: bool,
}

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
    /// The UI-free sidecar half (AW1 S1); this session derefs to it.
    pub(crate) sidecar: SidecarSession,
    /// Every incident id this session's open-time restore loaded (`IN2B`
    /// §3 rule 9): feeds the card's "from an earlier session" marker.
    /// Display membership never expires.
    pub(crate) loaded_ids: BTreeSet<IncidentId>,
    /// The loaded ids this run has not yet re-seen (`IN2B` §3 rules 11–12):
    /// consumed by first-dedups and Investigate presses. Queue eligibility
    /// expires; display membership (above) does not.
    pub(crate) loaded_open_ids: BTreeSet<IncidentId>,
    /// Restored ids whose subject resolves to nothing in the loaded
    /// document (`IN2B` §3 rule 17): the card reads "no longer in this
    /// project" and offers no enabled operation.
    pub(crate) subject_missing: BTreeSet<IncidentId>,
    /// The loaded wall stamp per restored id (`IN2B` §3 rule 14), snapshotted
    /// from the parsed records at load: the card's recency derives from it
    /// app-side, so core's `loaded_wall` stays private. A `None` stamp
    /// shows no recency rather than a lie.
    pub(crate) loaded_walls: BTreeMap<IncidentId, Option<i64>>,
    /// The `format_version` this session read (`IN2B` §4 rule 3): the
    /// envelope's version at open, or the journal's writer version after a
    /// recovery restore. Gates overwrite-save via
    /// [`can_overwrite_save`](kinewright_project::can_overwrite_save);
    /// Save As resets it — the bytes on the new path are this build's.
    pub(crate) format_version: u32,
    /// The recovery cause of the sidecar suspension (N6.1/J5): only a
    /// recovered copy routes overwrite-save to Save As — an H3 suspension
    /// blocks sidecar writes, never the project save.
    pub(crate) recovery_suspended: bool,
    /// Runtime, never-serialized LUT availability, one entry per document
    /// asset, refreshed whenever the library is rebuilt (CC4 §2.3).
    pub(crate) lut_availability: BTreeMap<LutAssetId, LutAvailabilityStatus>,
    /// The most recently published library, kept so a card can report how many
    /// looks actually resolved without rebuilding.
    pub(crate) lut_library: Arc<LutLibrary>,
    pub(crate) saved_document: Option<Arc<Document>>,
    /// Memoized [`ProjectSession::is_dirty`] verdict for the last compared
    /// `(current, saved)` pair. `is_dirty` takes `&self` and runs on hot UI
    /// paths (window title, close guards), so the cache lives behind a
    /// `RefCell` rather than requiring `&mut self`. Keyed on the identity of
    /// *both* Arcs: an edit, an undo back to saved content, a no-op edit, or
    /// a `saved_document` swap without an edit all miss and recompute, keeping
    /// the exact structural-equality semantics of the uncached comparison.
    dirty_cache: RefCell<Option<DirtyCacheEntry>>,
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
    /// AU4 §5.1 rule 100: whether the timeline paints clip gain envelopes.
    ///
    /// Session state beside `pixels_per_frame`, never document state, and on
    /// by default: the overlay is a view of the document, so hiding it must
    /// not be something undo can restore. Off hides the band and with it all
    /// envelope hit-testing. A modifier key was rejected — Alt is already the
    /// snap bypass — and so was a mode toggle that disables clip drags.
    pub(crate) show_envelopes: bool,
    /// AU4 §5.1 rule 101 (AU4 §0 E49): the envelope key the timeline saw under
    /// the pointer at the end of the last frame.
    ///
    /// Session state, rewritten every frame the timeline draws.
    /// `keyboard_shortcuts` runs before the timeline paints, so this report is
    /// how Delete/Backspace tells "remove this key" from "delete this clip" —
    /// the matte overlay's `report_expanded` pattern, one frame old.
    pub(crate) envelope_hover: Option<crate::timeline_ui::EnvelopeHover>,
    /// MO1 R23: the key-lane diamond the timeline saw under the pointer at
    /// the end of the last frame. The E49 pattern for effect keys — session
    /// state, rewritten every frame the timeline draws, taken one-shot by
    /// `keyboard_shortcuts`.
    pub(crate) key_lane_hover: Option<crate::timeline_ui::KeyLaneHover>,
    /// The IN2 investigator state for this project: session queue, running
    /// session, and per-project mutes. Always present after `create`, with
    /// the mutes starting from the opened document's; session state lives
    /// here, never on the app struct.
    pub(crate) investigator: Option<InvestigatorSession>,
}

// AW1 S1: the session derefs to its sidecar half (cutover compatibility).
impl Deref for ProjectSession {
    type Target = SidecarSession;
    fn deref(&self) -> &SidecarSession {
        &self.sidecar
    }
}

impl DerefMut for ProjectSession {
    fn deref_mut(&mut self) -> &mut SidecarSession {
        &mut self.sidecar
    }
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

/// Snapshot the loaded sets after the open-time restore (`IN2B` §3 rules 9,
/// 11, 12, 17).
///
/// Restore runs into an empty log, so every entry present is either a
/// restored record or a load-time note (the unknown-codes aggregate, a
/// `sidecar_refused`). Restored records carry no provenance and always
/// load non-transient; both load-time notes are transient — so excluding
/// transient entries snapshots exactly the restored ids (N6/H4). All
/// restored ids join `loaded_ids`, open ones join `loaded_open_ids`, and
/// each subject resolves against the loaded document — unresolvable
/// subjects join `subject_missing`.
///
/// `pub(crate)` so item 27 resolves through the production path instead of
/// a test double (S-15a).
pub(crate) fn capture_loaded_sets(
    incidents: &IncidentLogHandle,
    document: &Document,
) -> (
    BTreeSet<IncidentId>,
    BTreeSet<IncidentId>,
    BTreeSet<IncidentId>,
) {
    let log = incidents
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut loaded = BTreeSet::new();
    let mut loaded_open = BTreeSet::new();
    let mut missing = BTreeSet::new();
    for incident in log.all() {
        if incident.transient {
            continue;
        }
        loaded.insert(incident.id);
        if incident.state == IncidentState::Open {
            loaded_open.insert(incident.id);
        }
        if !subject_resolves(document, &incident.subject) {
            missing.insert(incident.id);
        }
    }
    (loaded, loaded_open, missing)
}

/// Whether a restored subject names something in the loaded document
/// (`IN2B` §3 rule 17: asset/clip/look/bus lookups).
///
/// `Track` has no document lookup, and `Chain(Master)`/`ExportJob`/`Project`/
/// `Agent` name no document member, so those always resolve — only a failed
/// lookup marks a subject missing.
fn subject_resolves(document: &Document, subject: &IncidentSubject) -> bool {
    match subject {
        IncidentSubject::Asset(id) => document.asset(*id).is_some(),
        IncidentSubject::LutAsset(id) => document.lut_asset(*id).is_some(),
        IncidentSubject::Clip(id) => document.clip(*id).is_some(),
        IncidentSubject::Chain(AudioChain::Bus(id)) => document.audio_mix.bus(*id).is_some(),
        IncidentSubject::Chain(AudioChain::Master)
        | IncidentSubject::Track(_)
        | IncidentSubject::ExportJob
        | IncidentSubject::Project
        | IncidentSubject::Agent => true,
    }
}

impl ProjectSession {
    /// Build every actor, channel, recovery recorder, and initial thread for a project.
    ///
    /// `sidecar_mode` decides the open-time history load (`IN2B` §2 rule 5,
    /// N2/B-1): startup reopen and `open_project` pass `Load` carrying the
    /// digest `load_document` read (no second read, §4 rule 2), recovery
    /// restore passes `RecoveryNoDigest`, new projects pass `None`.
    /// `sidecar_writer` is the app's shared writer; `None` spawns a private
    /// one for headless-test isolation. `refuse_rename` injects the `.bak`
    /// rename (N6/H3); `None` renames for real. `format_version` is the
    /// envelope version the session read (§4 rule 3).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    pub(crate) fn create(
        id: u64,
        name: impl Into<String>,
        document: Document,
        project_path: Option<PathBuf>,
        playback: &Arc<dyn Playback>,
        analysis: &Arc<dyn Analysis>,
        exporter: &Arc<dyn Export>,
        sidecar_mode: &SidecarMode,
        sidecar_writer: Option<Arc<SidecarWriter>>,
        format_version: u32,
        refuse_rename: Option<&RefuseRename>,
    ) -> Result<Self, String> {
        let name = name.into();
        let core = Core::spawn(document.clone()).map_err(|error| error.to_string())?;
        let core_events = core.subscribe().map_err(|error| error.to_string())?;
        let chat = vec![ChatEntry::Text(
            "Drop a clip anywhere (or /import), then describe your edit.".to_owned(),
        )];
        // Seeded, not detected: `detect_harness` spawns the CLI, and opening
        // a project happens on the frame thread. The agent panel re-picks
        // this from the detected-and-enabled set (and the remembered
        // choice) on every frame while no session is running, so the seed
        // only has to be a valid variant.
        let agent_harness = AgentHarnessChoice::ALL[0];
        let recovery = if id == 1 {
            Recovery::start(&core, project_path.as_deref())
        } else {
            Recovery::start_attached(&core, project_path.as_deref())
        };
        let document = Arc::new(document);
        let (lut_store, lut_store_error) = match derive_lut_store(project_path.as_deref()) {
            Ok(store) => (store, None),
            Err(reason) => (None, Some(reason)),
        };
        let (library, statuses) = LutLibrary::build(&document.lut_assets, lut_store.as_ref());
        let agent_project_path: ProjectPathHandle =
            std::sync::Arc::new(std::sync::RwLock::new(project_path.clone()));
        // The wall origin is injected at session creation (`IN2B` §3 rule 14,
        // N1/B4, N-8): wall stamps derive at sidecar write as origin +
        // `opened_at`, and no other construction site passes one.
        let incidents: IncidentLogHandle = std::sync::Arc::new(std::sync::RwLock::new(
            IncidentLog::with_start(Instant::now(), Some(SystemTime::now())),
        ));
        // The open-time history load runs here, before any note and before
        // the log handle is shared with the first agent thread below (N4.1):
        // restore requires an empty log and no id may collide with it.
        let loaded = SidecarSession::load(
            sidecar_mode,
            project_path.as_deref(),
            std::sync::Arc::clone(&incidents),
            TimelineRevision::default(),
            sidecar_writer,
            refuse_rename,
        );
        // Restored ids snapshot before the session exists (`IN2B` §3 rules 9,
        // 11, 12, 17): the log holds only restored entries, so every entry
        // present is one.
        let (loaded_ids, loaded_open_ids, subject_missing) =
            capture_loaded_sets(&incidents, &document);
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
            sidecar: loaded.session,
            loaded_ids,
            loaded_open_ids,
            subject_missing,
            loaded_walls: loaded.loaded_walls,
            format_version,
            // The load only ever suspends for H3 — the recovery cause is
            // set by `apply_restore_request` (N6.1/J5).
            recovery_suspended: false,
            lut_availability: statuses.into_iter().collect(),
            lut_library: Arc::new(library),
            saved_document: None,
            dirty_cache: RefCell::new(None),
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
                &incidents,
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
            show_envelopes: true,
            envelope_hover: None,
            key_lane_hover: None,
            investigator: Some(InvestigatorSession::with_muted_codes(
                document
                    .investigator
                    .as_ref()
                    .map(|preferences| preferences.muted_codes.clone())
                    .unwrap_or_default(),
            )),
        };
        session.publish_project_path_to_agents();
        Ok(session)
    }

    /// Whether the project differs from the last save: the live document
    /// differs structurally, or the investigator mutes changed since the save.
    pub(crate) fn is_dirty(&self) -> bool {
        self.document_dirty()
            || self
                .investigator
                .as_ref()
                .is_some_and(InvestigatorSession::mutes_dirty)
    }

    /// The document half of [`Self::is_dirty`].
    ///
    /// An unsaved project (no `saved_document`) is always dirty. Otherwise the
    /// verdict is exact structural equality, memoized on the identity of both
    /// the current and the saved `Arc`: repeated polls with the same pair —
    /// the steady state of every frame's title and close-guard checks — return
    /// the cached verdict without a deep comparison, while any edit, undo, or
    /// save swap misses and recomputes. A same-allocation pair is clean
    /// without comparing at all.
    fn document_dirty(&self) -> bool {
        let Some(saved) = self.saved_document.as_ref() else {
            self.dirty_cache.borrow_mut().take();
            return true;
        };
        if Arc::ptr_eq(saved, &self.document) {
            // A save can make this pair identical. Release any older pair
            // instead of retaining obsolete document snapshots indefinitely.
            self.dirty_cache.borrow_mut().take();
            return false;
        }
        {
            let cached = self.dirty_cache.borrow();
            if let Some(entry) = cached.as_ref()
                && Arc::ptr_eq(&entry.current, &self.document)
                && Arc::ptr_eq(&entry.saved, saved)
            {
                return entry.dirty;
            }
        }
        let dirty = saved.as_ref() != self.document.as_ref();
        *self.dirty_cache.borrow_mut() = Some(DirtyCacheEntry {
            current: Arc::clone(&self.document),
            saved: Arc::clone(saved),
            dirty,
        });
        dirty
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

    /// Build this session's sidecar bytes (test-only; production builds
    /// through the joining flush).
    #[cfg(test)]
    pub(crate) fn sidecar_bytes_for_save(
        &mut self,
        project_digest: &str,
        previous_digest: &str,
    ) -> Result<(Vec<u8>, kinewright_core::WriteReport), String> {
        self.sidecar.sidecar_bytes_for_save(
            project_digest,
            previous_digest,
            prepare_investigator_context(self.investigator.as_ref()),
        )
    }

    /// The one synchronous sidecar seam, through the session.
    pub(crate) fn flush_incidents(
        &mut self,
        project_digest: &str,
        previous_digest: &str,
    ) -> std::io::Result<FlushOutcome> {
        self.sidecar.flush_incidents(
            self.project_path.as_deref(),
            project_digest,
            previous_digest,
            prepare_investigator_context(self.investigator.as_ref()),
        )
    }

    /// [`Self::flush_incidents`] when the writer has not confirmed the
    /// current generation, through the session.
    pub(crate) fn flush_incidents_if_changed(&mut self) -> std::io::Result<FlushOutcome> {
        self.sidecar.flush_incidents_if_changed(
            self.project_path.as_deref(),
            prepare_investigator_context(self.investigator.as_ref()),
        )
    }

    /// Queue a debounced background flush without joining, through the
    /// session.
    pub(crate) fn queue_incidents_flush(&mut self) {
        self.sidecar.queue_incidents_flush(
            self.project_path.as_deref(),
            prepare_investigator_context(self.investigator.as_ref()),
        );
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
        // Cloned before the investigator borrow: `incidents` now resolves
        // through the sidecar deref, which borrows the whole session.
        let incidents = std::sync::Arc::clone(&self.incidents);
        if let Some(investigator) = self.investigator.as_mut() {
            investigator.shutdown_for_close(reason, &incidents);
        }
        // `IN2B` §2 rule 4 (N2/B-5): the sidecar flushes after
        // `shutdown_for_close` — close-time records already carry the real
        // close reason — whether or not the project is dirty. A failed flush
        // notes one `sidecar_write_failed` directly (the router is not
        // running on this path) and never fails the close.
        if let Err(error) = self.flush_incidents_if_changed() {
            let revision = self.revision;
            let mut log = self
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = log.observe(sidecar_write_failed_observation(
                error.to_string(),
                revision,
            ));
        }
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

/// Investigator context for a flush (F2): base order; skips never run it.
fn prepare_investigator_context(
    investigator: Option<&InvestigatorSession>,
) -> impl FnOnce(&mut BTreeMap<IncidentId, Operation>) -> Option<RunningInvestigation> + '_ {
    move |stash| {
        let running = investigator.and_then(InvestigatorSession::running_investigation);
        if let Some(session) = investigator {
            session.copy_queued_refused_into(stash);
        }
        running
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
    use std::{collections::BTreeMap, fs};

    use super::*;
    use kinewright_core::{
        Clip, ClipContent, ColorDescription, Document, Effect, EffectId, IncidentCode,
        IncidentEvidence, IncidentObservation, LUT_ASSET_ID_PARAMETER, LabelIncident, LutAsset,
        MediaAsset, MediaSourceFingerprint, Operation, PROJECT_FORMAT_VERSION, ParamValue,
        Rational, Track, apply_batch,
    };
    use kinewright_media::{BuiltinLook, LutAssetImport, test_support::TempDirectory};
    #[cfg(unix)]
    use kinewright_project::write_file_atomic;
    use kinewright_project::{
        ProjectFile, build_sidecar_bytes, can_overwrite_save, canonical_session_key,
        derive_lut_store, digest_bytes, load_sidecar, serialize_project_document,
        sidecar_matches_project, sidecar_path_for_project, write_project_document,
    };

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
                assumed_from: None,
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
            enabled: true,
            enabled_curve: None,
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
                assumed_from: None,
            }],
            fps: Rational::new(24, 1).expect("valid fps"),
            resolution: (1920, 1080),
            lut_assets: vec![asset],
            ..Document::default()
        };
        document.tracks[0].clips.push(Clip {
            enabled: true,
            enabled_curve: None,
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
            audio_gain_curve: None,
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
        let incidents: IncidentLogHandle =
            std::sync::Arc::new(std::sync::RwLock::new(IncidentLog::default()));
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
            &incidents,
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

    /// IN1 §5.1 rule 3: every MCP server a session owns shares that session's
    /// incident log handle, so the agent in the chat panel reads exactly the
    /// incidents the person sees — and no other project's.
    ///
    /// Driven through the real `AgentThread::new` seam, mirroring the project
    /// path handle test above: only the server's own `incident_log_handle`
    /// can witness that it was started *with the session's handle*.
    #[test]
    fn every_branch_server_shares_the_session_incident_handle() {
        let handle: ProjectPathHandle = std::sync::Arc::new(std::sync::RwLock::new(None));
        let incidents: IncidentLogHandle =
            std::sync::Arc::new(std::sync::RwLock::new(IncidentLog::default()));
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
            &incidents,
        )
        .expect("the branch builds");
        let served = thread
            .mcp_server
            .as_ref()
            .expect("the branch server starts")
            .incident_log_handle();
        assert!(
            std::sync::Arc::ptr_eq(&served, &incidents),
            "a branch server must be started with the session's own incident handle, not a copy"
        );
        assert!(
            served.read().expect("readable").is_empty(),
            "a fresh session opens no incident"
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

        assert_eq!(moved_library.len(), origin_library.len());
        let origin_lut = origin_library.get(LutAssetId(1)).expect("origin lattice");
        let moved_lut = moved_library.get(LutAssetId(1)).expect("moved lattice");
        assert_eq!(origin_lut.size, moved_lut.size);
        assert_eq!(bits(&origin_lut.domain_min), bits(&moved_lut.domain_min));
        assert_eq!(bits(&origin_lut.domain_max), bits(&moved_lut.domain_max));
        assert_eq!(bits(&origin_lut.rgba), bits(&moved_lut.rgba));
        assert_eq!(moved_document.lut_assets[0].sha256, sha256);
        assert_eq!(moved_statuses[0].1.kind, LutAvailabilityKind::Verified);

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

    /// A valid synthetic document with `clip_count` media clips on one video
    /// track, mirroring `kinewright_core`'s `generated_document` fixture shape
    /// (packed 30-frame clips, one source asset, derived duration).
    fn synthetic_document(clip_count: usize) -> Document {
        let fps = Rational::new(30, 1).expect("valid fps");
        let mut timeline_start = 0_i64;
        let mut clips = Vec::with_capacity(clip_count);
        for index in 0..clip_count {
            clips.push(Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(index as u64 + 1),
                asset: AssetId(1),
                source_range: TimeCode::ZERO..TimeCode(30),
                content: ClipContent::Media,
                timeline_start: TimeCode(timeline_start),
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
            });
            timeline_start += 30;
        }
        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips,
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: "synthetic.mov".into(),
                name: "Synthetic".to_owned(),
                duration: TimeCode(300),
                fps,
                kind: MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: MediaSourceFingerprint::default(),
                color_description: ColorDescription::default(),
                assumed_from: None,
            }],
            fps,
            resolution: (1920, 1080),
            duration: TimeCode(timeline_start),
            ..Document::default()
        };
        document
            .validate()
            .expect("the synthetic document is valid");
        document
    }

    /// A real session behind the stub media backends, so `is_dirty` runs
    /// through the exact struct the app polls. Id 2 takes the attached
    /// recovery path (no process-startup scan).
    fn dirty_test_session(document: Document) -> ProjectSession {
        let playback: Arc<dyn Playback> = Arc::new(StubMedia);
        let analysis: Arc<dyn Analysis> = Arc::new(StubMedia);
        let exporter: Arc<dyn Export> = Arc::new(StubMedia);
        ProjectSession::create(
            2,
            "dirty-test",
            document,
            None,
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::None,
            None,
            PROJECT_FORMAT_VERSION,
            None,
        )
        .expect("the dirty-test session builds")
    }

    fn shutdown_test_session(session: &mut ProjectSession) {
        for thread in &mut session.threads {
            if let Some(server) = thread.mcp_server.take() {
                server.shutdown();
            }
        }
    }

    #[test]
    fn an_unsaved_project_is_dirty_no_matter_the_document() {
        let mut session = dirty_test_session(synthetic_document(4));
        assert!(session.is_dirty());
        // Swapping the live document without ever saving stays dirty, whether
        // the replacement matches the old content or not.
        session.document = Arc::new(synthetic_document(4));
        assert!(session.is_dirty());
        session.document = Arc::new(synthetic_document(5));
        assert!(session.is_dirty());
        shutdown_test_session(&mut session);
    }

    #[test]
    fn saving_makes_independently_allocated_equal_documents_clean() {
        let mut session = dirty_test_session(synthetic_document(4));
        // A save records the same allocation: clean via the pointer fast path.
        session.saved_document = Some(Arc::clone(&session.document));
        assert!(!session.is_dirty());
        // An independently allocated but structurally equal save is clean too:
        // one deep comparison, then the memoized verdict on repeat polls.
        let separate = Arc::new(synthetic_document(4));
        assert!(!Arc::ptr_eq(&separate, &session.document));
        session.saved_document = Some(separate);
        assert!(!session.is_dirty());
        assert!(!session.is_dirty(), "the equal pair memoizes clean");
        shutdown_test_session(&mut session);
    }

    #[test]
    fn an_edit_dirties_and_an_undo_back_to_saved_cleans() {
        let mut session = dirty_test_session(synthetic_document(4));
        session.saved_document = Some(Arc::clone(&session.document));
        assert!(!session.is_dirty());

        // An edit swaps in a new allocation with different content.
        let mut edited = (*session.document).clone();
        edited.resolution = (1_280, 720);
        session.document = Arc::new(edited);
        assert!(session.is_dirty());
        assert!(session.is_dirty(), "the dirty pair memoizes dirty");

        // Undo restores the saved content on a fresh allocation: clean again,
        // not stuck on the cached dirty verdict.
        let saved = session.saved_document.clone().expect("saved");
        session.document = Arc::new((*saved).clone());
        assert!(!Arc::ptr_eq(
            &session.document,
            session.saved_document.as_ref().expect("saved")
        ));
        assert!(!session.is_dirty());
        assert!(!session.is_dirty(), "the restored pair memoizes clean");
        shutdown_test_session(&mut session);
    }

    #[test]
    fn a_noop_edit_keeps_a_clean_project_clean() {
        let mut session = dirty_test_session(synthetic_document(4));
        session.saved_document = Some(Arc::new(synthetic_document(4)));
        assert!(!session.is_dirty());
        // A no-op "edit" swaps in a new allocation with identical content.
        session.document = Arc::new(synthetic_document(4));
        assert!(!Arc::ptr_eq(
            &session.document,
            session.saved_document.as_ref().expect("saved")
        ));
        assert!(!session.is_dirty());
        shutdown_test_session(&mut session);
    }

    #[test]
    fn replacing_the_save_without_an_edit_invalidates_the_verdict() {
        let mut session = dirty_test_session(synthetic_document(4));
        session.saved_document = Some(Arc::clone(&session.document));
        assert!(!session.is_dirty());

        // The live document never moves; only the save does. The cache is
        // keyed on both identities, so it must miss and recompute.
        session.saved_document = Some(Arc::new(synthetic_document(5)));
        assert!(session.is_dirty());
        assert!(session.is_dirty(), "the swapped pair memoizes dirty");

        session.saved_document = Some(Arc::clone(&session.document));
        assert!(!session.is_dirty());
        shutdown_test_session(&mut session);
    }

    #[test]
    fn is_dirty_fast_paths_release_obsolete_cached_documents() {
        for saved in [true, false] {
            let mut session = dirty_test_session(Document::default());
            // Use distinct allocations that no Core history owns, so weak
            // references observe whether the cache prolongs their lifetime.
            session.document = Arc::new(Document::default());
            session.saved_document = Some(Arc::new(Document {
                resolution: (640, 360),
                ..Document::default()
            }));
            let old_current = Arc::downgrade(&session.document);
            let old_saved = Arc::downgrade(session.saved_document.as_ref().unwrap());
            assert!(session.is_dirty());

            session.document = Arc::new(Document::default());
            session.saved_document = saved.then(|| Arc::clone(&session.document));
            assert!(old_current.upgrade().is_some(), "the pair is cached");
            assert!(old_saved.upgrade().is_some(), "the pair is cached");
            assert_eq!(session.is_dirty(), !saved);
            assert!(old_current.upgrade().is_none(), "old current is released");
            assert!(old_saved.upgrade().is_none(), "old save is released");
        }
    }

    /// Release-only measurement of the memoized `is_dirty` poll, deliberately
    /// excluded from correctness CI. Run with
    /// `cargo test -p kinewright-app --release -- --ignored --nocapture
    /// is_dirty_memoized_poll_release_perf`.
    ///
    /// Each lane polls `is_dirty` 20k times against a save that is
    /// structurally equal but separately allocated — the shape that forces the
    /// expensive deep comparison without memoization — and compares it against
    /// the previous direct deep-equality implementation as a control.
    #[test]
    #[ignore = "manual release performance measurement"]
    fn is_dirty_memoized_poll_release_perf() {
        use std::{hint::black_box, time::Instant};

        const POLLS: usize = 20_000;
        for (lane, clips) in [("typical", 48), ("heavy", 480), ("agent", 1_200)] {
            let mut session = dirty_test_session(synthetic_document(clips));
            // Semantically equal but separately allocated: `ptr_eq` cannot
            // save either path, so the control pays the full deep comparison.
            session.saved_document = Some(Arc::new(synthetic_document(clips)));
            assert!(!Arc::ptr_eq(
                &session.document,
                session.saved_document.as_ref().expect("saved")
            ));
            assert!(!session.is_dirty(), "the {lane} lane starts clean");

            let began = Instant::now();
            for _ in 0..POLLS {
                black_box(black_box(&session).is_dirty());
            }
            let memoized_ms = began.elapsed().as_secs_f64() * 1_000.0;

            // The previous implementation, measured as a control: the raw deep
            // comparison with its inputs behind `black_box` so the loop cannot
            // be elided or hoisted.
            let current = Arc::clone(&session.document);
            let saved = session.saved_document.clone().expect("saved");
            let mut control_dirty = false;
            let began = Instant::now();
            for _ in 0..POLLS {
                let live: &Document = black_box(current.as_ref());
                let baseline: &Document = black_box(saved.as_ref());
                control_dirty |= black_box(live != baseline);
            }
            let control_ms = began.elapsed().as_secs_f64() * 1_000.0;
            black_box(&current);
            black_box(&saved);

            assert!(
                !control_dirty,
                "the {lane} control agrees the pair is clean"
            );
            assert!(
                !session.is_dirty(),
                "the {lane} memoized verdict still agrees"
            );
            println!(
                "{}",
                serde_json::json!({
                    "test": "is_dirty_memoized_poll",
                    "lane": lane,
                    "clips": clips,
                    "polls": POLLS,
                    "memoized_ms": memoized_ms,
                    "deep_control_ms": control_ms,
                    "memoized_dirty": false,
                    "control_dirty": control_dirty,
                })
            );
            shutdown_test_session(&mut session);
        }
    }

    /// A session on a project file with the digest-gated load, behind the stub
    /// media backends. Each caller shuts its session down.
    fn sidecar_session(id: u64, path: &Path) -> ProjectSession {
        let playback: Arc<dyn Playback> = Arc::new(StubMedia);
        let analysis: Arc<dyn Analysis> = Arc::new(StubMedia);
        let exporter: Arc<dyn Export> = Arc::new(StubMedia);
        ProjectSession::create(
            id,
            "sidecar-test",
            Document::default(),
            Some(path.to_path_buf()),
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::Load {
                project_digest: project_digest_of(path),
            },
            None,
            PROJECT_FORMAT_VERSION,
            None,
        )
        .expect("the sidecar-test session builds")
    }

    fn sorted_entries(directory: &Path) -> Vec<String> {
        let mut entries: Vec<String> = fs::read_dir(directory)
            .expect("the watched directory reads")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        entries
    }

    /// Item 17: an unsaved project derives no sidecar path, reports `Skipped`,
    /// and writes nothing on close.
    #[test]
    fn in2b_an_unsaved_project_does_no_sidecar_io() {
        // Asserted, not assumed (N-5).
        assert_eq!(sidecar_path_for_project(None), None);

        let mut session = dirty_test_session(Document::default());
        // Give the flush something it would write, so `Skipped` is a decision.
        {
            let mut log = session
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "in2b unsaved flush",
                TimelineRevision::default(),
            ));
            assert!(log.generation() > 0);
        }
        assert_eq!(
            session
                .flush_incidents("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb")
                .expect("an unsaved flush reports"),
            FlushOutcome::Skipped
        );
        assert_eq!(
            session
                .flush_incidents_if_changed()
                .expect("an unsaved conditional flush reports"),
            FlushOutcome::Skipped
        );
        // Close performs zero filesystem writes, watched.
        let watched = TempDirectory::new("in2b-unsaved-io");
        let before = sorted_entries(watched.root());
        session.stop_threads("in2b test close");
        assert_eq!(sorted_entries(watched.root()), before);
        // And no failure incident: nothing failed, nothing was attempted.
        let log = session
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(log.open_count(), 1);
        drop(log);
        shutdown_test_session(&mut session);
    }

    /// One fresh sidecar-test project file in `dir`.
    fn sidecar_project_file(dir: &TempDirectory, name: &str) -> PathBuf {
        let path = dir.path(name);
        write_project_document(&Document::default(), &path, None).expect("project writes");
        path
    }

    /// The pairing digest of project bytes on disk.
    fn project_digest_of(path: &Path) -> String {
        digest_bytes(&fs::read(path).expect("the project file reads"))
    }

    /// Two records through the production builder, with distinct revisions so
    /// the rebase is visible.
    fn two_records() -> Vec<kinewright_core::IncidentRecord> {
        let mut log = IncidentLog::with_start(
            std::time::Instant::now(),
            Some(std::time::SystemTime::UNIX_EPOCH),
        );
        for (n, revision) in [41u64, 42].into_iter().enumerate() {
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                format!("in2b arm record {n}"),
                TimelineRevision(revision),
            ));
        }
        let (records, _) = log.records(None, &BTreeMap::new());
        assert_eq!(records.len(), 2);
        records
    }

    /// Item 15: every sidecar arm behaves — missing, corrupt, versionless,
    /// newer, mismatched, older, second refusal, carried records, torn-read
    /// window, oversize version, and the carried-only generation rule.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn in2b_each_sidecar_arm_behaves() {
        use kinewright_project::SidecarLoad;

        // Missing → `Absent`: empty log, silent.
        let missing_dir = TempDirectory::new("in2b-arm-missing");
        let missing_project = sidecar_project_file(&missing_dir, "edit.kinewright");
        let missing_sidecar =
            sidecar_path_for_project(Some(&missing_project)).expect("a saved project derives");
        assert_eq!(load_sidecar(&missing_sidecar), SidecarLoad::Absent);
        let mut session = sidecar_session(21, &missing_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(log.is_empty());
            assert_eq!(log.open_count(), 0);
        }
        assert!(session.last_restore_report.is_none());
        shutdown_test_session(&mut session);

        // Truncated → `Corrupt`: 1 `sidecar_refused`, the project opens, the
        // refused bytes move to first-free `.bak`.
        let corrupt_dir = TempDirectory::new("in2b-arm-corrupt");
        let corrupt_project = sidecar_project_file(&corrupt_dir, "edit.kinewright");
        let corrupt_sidecar =
            sidecar_path_for_project(Some(&corrupt_project)).expect("a saved project derives");
        fs::write(&corrupt_sidecar, b"{ truncated").expect("the torn sidecar writes");
        assert!(matches!(
            load_sidecar(&corrupt_sidecar),
            SidecarLoad::Corrupt(_)
        ));
        let mut session = sidecar_session(22, &corrupt_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1);
            let only = log.all().next().expect("the refusal noted");
            assert_eq!(
                only.code,
                IncidentCode::Label(LabelIncident::SidecarRefused)
            );
        }
        let bak = corrupt_sidecar.with_extension("kinewright-incidents.bak");
        assert!(!corrupt_sidecar.exists(), "the refused file moves away");
        assert_eq!(fs::read(&bak).expect("bak reads"), b"{ truncated");
        // A second refusal lands `.bak.1`, leaving `.bak` byte-identical.
        fs::write(&corrupt_sidecar, b"truncated again").expect("the second torn sidecar writes");
        shutdown_test_session(&mut session);
        let mut session = sidecar_session(23, &corrupt_project);
        let mut bak1_name = corrupt_sidecar.as_os_str().to_owned();
        bak1_name.push(".bak.1");
        let bak1 = PathBuf::from(bak1_name);
        assert_eq!(fs::read(&bak).expect("first bak reads"), b"{ truncated");
        assert_eq!(
            fs::read(&bak1).expect("second bak reads"),
            b"truncated again"
        );
        shutdown_test_session(&mut session);

        // Versionless → `Corrupt("missing format_version")` + `.bak`.
        let versionless_dir = TempDirectory::new("in2b-arm-versionless");
        let versionless_project = sidecar_project_file(&versionless_dir, "edit.kinewright");
        let versionless_sidecar =
            sidecar_path_for_project(Some(&versionless_project)).expect("a saved project derives");
        fs::write(
            &versionless_sidecar,
            r#"{"project_digest":"aa","previous_digest":"aa","records":[]}"#,
        )
        .expect("the versionless sidecar writes");
        assert_eq!(
            load_sidecar(&versionless_sidecar),
            SidecarLoad::Corrupt("missing format_version".to_owned())
        );
        let mut session = sidecar_session(24, &versionless_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1);
        }
        assert!(!versionless_sidecar.exists());
        shutdown_test_session(&mut session);

        // Newer → `Newer(999)`: 1 incident, `.bak` keeps the original bytes.
        let newer_dir = TempDirectory::new("in2b-arm-newer");
        let newer_project = sidecar_project_file(&newer_dir, "edit.kinewright");
        let newer_sidecar =
            sidecar_path_for_project(Some(&newer_project)).expect("a saved project derives");
        let newer_bytes =
            r#"{"format_version":999,"project_digest":"aa","previous_digest":"aa","records":[]}"#;
        fs::write(&newer_sidecar, newer_bytes).expect("the newer sidecar writes");
        assert_eq!(load_sidecar(&newer_sidecar), SidecarLoad::Newer(999));
        let mut session = sidecar_session(25, &newer_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1);
            let only = log.all().next().expect("the refusal noted");
            assert!(
                only.observed.contains("newer Kinewright"),
                "the card names the newer writer: {}",
                only.observed
            );
        }
        let newer_bak = newer_sidecar.with_extension("kinewright-incidents.bak");
        assert_eq!(
            fs::read(&newer_bak).expect("bak reads"),
            newer_bytes.as_bytes()
        );
        assert!(!newer_sidecar.exists(), "the original path is gone");
        shutdown_test_session(&mut session);

        // Both digests swapped → refused with 1 incident + `.bak`.
        let mismatch_dir = TempDirectory::new("in2b-arm-mismatch");
        let mismatch_project = sidecar_project_file(&mismatch_dir, "edit.kinewright");
        let mismatch_sidecar =
            sidecar_path_for_project(Some(&mismatch_project)).expect("a saved project derives");
        let records = two_records();
        let mismatch_bytes =
            build_sidecar_bytes(&records, &[], "0000000000000000", "ffffffffffffffff")
                .expect("the mismatched sidecar builds");
        fs::write(&mismatch_sidecar, &mismatch_bytes).expect("the mismatched sidecar writes");
        let mut session = sidecar_session(26, &mismatch_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1);
            let only = log.all().next().expect("the refusal noted");
            assert_eq!(
                only.code,
                IncidentCode::Label(LabelIncident::SidecarRefused)
            );
            assert!(
                only.observed.contains("different project"),
                "the card names the mix-up: {}",
                only.observed
            );
        }
        let mismatch_bak = mismatch_sidecar.with_extension("kinewright-incidents.bak");
        assert_eq!(fs::read(&mismatch_bak).expect("bak reads"), mismatch_bytes);
        shutdown_test_session(&mut session);

        // Version 0 → `Current`: loads, 0 incidents.
        let zero_dir = TempDirectory::new("in2b-arm-zero");
        let zero_project = sidecar_project_file(&zero_dir, "edit.kinewright");
        let zero_sidecar =
            sidecar_path_for_project(Some(&zero_project)).expect("a saved project derives");
        let digest = project_digest_of(&zero_project);
        let zero_bytes =
            build_sidecar_bytes(&records, &[], &digest, &digest).expect("the v0 body builds");
        let zero_text = String::from_utf8(zero_bytes)
            .expect("sidecars are UTF-8")
            .replacen("\"format_version\": 1", "\"format_version\": 0", 1);
        fs::write(&zero_sidecar, &zero_text).expect("the v0 sidecar writes");
        let mut session = sidecar_session(27, &zero_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 2);
            assert_eq!(log.len(), 2);
        }
        let report = session
            .last_restore_report
            .as_ref()
            .expect("a version-0 load reports");
        assert_eq!(report.restored_open, 2);
        shutdown_test_session(&mut session);

        // One unparseable record among good ones → `Current`: the good load,
        // the carried bytes re-emit verbatim, and the floor resumes above the
        // carried id (F2) — with no aggregate when no code is unknown.
        let carry_dir = TempDirectory::new("in2b-arm-carry");
        let carry_project = sidecar_project_file(&carry_dir, "edit.kinewright");
        let carry_sidecar =
            sidecar_path_for_project(Some(&carry_project)).expect("a saved project derives");
        let digest = project_digest_of(&carry_project);
        let good_text = serde_json::to_string(&records[0]).expect("the good record serialises");
        let bad_text = r#"{"id": 9000, "bogus": [1, 2, {"nested": true}]}"#;
        let envelope = format!(
            "{{\"format_version\": 1, \"project_digest\": \"{digest}\", \
             \"previous_digest\": \"{digest}\", \"records\": [{good_text}, {bad_text}]}}"
        );
        fs::write(&carry_sidecar, &envelope).expect("the carrying sidecar writes");
        let mut session = sidecar_session(28, &carry_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1, "the good record loads");
            assert!(
                log.all().all(|incident| incident.code
                    != IncidentCode::Label(LabelIncident::SidecarUnknownCodes)),
                "no unknown code, no aggregate"
            );
        }
        assert_eq!(
            session.carried_sidecar_records,
            vec![bad_text.to_owned()],
            "the unparseable record carries"
        );
        // The floor resumes above the carried id: the restored record holds
        // id 1, so without the floor the fresh id would be 2.
        let fresh = {
            let mut log = session
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "in2b after carried floor",
                TimelineRevision::default(),
            ))
        };
        assert_eq!(fresh, kinewright_core::Observed::Opened(IncidentId(9001)));
        // The carried bytes re-emit verbatim on the next write.
        let outcome = session
            .flush_incidents(&digest, &digest)
            .expect("the carrying flush lands");
        assert!(matches!(outcome, FlushOutcome::Written(_)));
        let rewritten = fs::read(&carry_sidecar).expect("the rewritten sidecar reads");
        let rewritten = String::from_utf8(rewritten).expect("sidecars are UTF-8");
        assert!(
            rewritten.contains(bad_text),
            "carried bytes re-emit verbatim"
        );
        shutdown_test_session(&mut session);

        // Unknown-code records carry verbatim AND aggregate (N4/F3): the raw
        // text rides in `carried` while `restore` still counts the aggregate,
        // so body #5 stays true.
        let unknown_dir = TempDirectory::new("in2b-arm-unknown");
        let unknown_project = sidecar_project_file(&unknown_dir, "edit.kinewright");
        let unknown_sidecar =
            sidecar_path_for_project(Some(&unknown_project)).expect("a saved project derives");
        let digest = project_digest_of(&unknown_project);
        let mut unknown_record = records[1].clone();
        unknown_record.code = "no_such_code_xyz".to_owned();
        let unknown_text =
            serde_json::to_string(&unknown_record).expect("the unknown record serialises");
        let envelope = format!(
            "{{\"format_version\": 1, \"project_digest\": \"{digest}\", \
             \"previous_digest\": \"{digest}\", \"records\": [{unknown_text}]}}"
        );
        fs::write(&unknown_sidecar, &envelope).expect("the unknown sidecar writes");
        let mut session = sidecar_session(29, &unknown_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1);
            let only = log.all().next().expect("the aggregate noted");
            assert_eq!(
                only.code,
                IncidentCode::Label(LabelIncident::SidecarUnknownCodes)
            );
        }
        assert_eq!(
            session.carried_sidecar_records,
            vec![unknown_text.clone()],
            "the unknown-code raw text carries verbatim"
        );
        let report = session
            .last_restore_report
            .as_ref()
            .expect("an unknown-code load reports");
        assert_eq!(
            report.unknown_codes,
            vec![("no_such_code_xyz".to_owned(), 1)]
        );
        shutdown_test_session(&mut session);

        // Carried-only restore moves no generation, and the first write-back
        // of carried bytes does not rely on it: the conditional flush skips
        // (nothing new), while the joining flush writes the carried bytes.
        let carried_only_dir = TempDirectory::new("in2b-arm-carried-only");
        let carried_only_project = sidecar_project_file(&carried_only_dir, "edit.kinewright");
        let carried_only_sidecar =
            sidecar_path_for_project(Some(&carried_only_project)).expect("a saved project derives");
        let digest = project_digest_of(&carried_only_project);
        let lone_text = r#"{"id": 5, "future": {"shape": true}}"#;
        let envelope = format!(
            "{{\"format_version\": 1, \"project_digest\": \"{digest}\", \
             \"previous_digest\": \"{digest}\", \"records\": [{lone_text}]}}"
        );
        fs::write(&carried_only_sidecar, &envelope).expect("the carried-only sidecar writes");
        let mut session = sidecar_session(30, &carried_only_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(log.is_empty());
            assert_eq!(log.generation(), 0, "carried-only restore moves nothing");
        }
        assert_eq!(
            session
                .flush_incidents_if_changed()
                .expect("the conditional flush reports"),
            FlushOutcome::Skipped
        );
        assert!(matches!(
            session
                .flush_incidents(&digest, &digest)
                .expect("the joining flush lands"),
            FlushOutcome::Written(_)
        ));
        let rewritten = fs::read(&carried_only_sidecar).expect("the rewritten sidecar reads");
        assert!(
            String::from_utf8(rewritten)
                .expect("sidecars are UTF-8")
                .contains(lone_text),
            "the write-back carries without relying on generation"
        );
        shutdown_test_session(&mut session);

        // S-12 box: a load paused between the temp write and the rename
        // returns the prior `Current`, never torn bytes.
        let torn_dir = TempDirectory::new("in2b-arm-torn");
        let torn_project = sidecar_project_file(&torn_dir, "edit.kinewright");
        let torn_sidecar =
            sidecar_path_for_project(Some(&torn_project)).expect("a saved project derives");
        let digest = project_digest_of(&torn_project);
        let prior =
            build_sidecar_bytes(&records, &[], &digest, &digest).expect("the prior sidecar builds");
        fs::write(&torn_sidecar, &prior).expect("the prior sidecar writes");
        let writer = SidecarWriter::new();
        let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        writer.set_rename_hook(Arc::new(move || {
            let _ = paused_tx.send(());
            if let Ok(slot) = release_rx.lock() {
                let _ = slot.recv();
            }
        }));
        let next =
            build_sidecar_bytes(&[], &[], &digest, &digest).expect("the next sidecar builds");
        let writer_thread = {
            let writer = Arc::clone(&writer);
            let torn_sidecar = torn_sidecar.clone();
            std::thread::spawn(move || writer.submit_and_join(torn_sidecar, next))
        };
        paused_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the writer pauses between temp write and rename");
        // The load in the window: captured now, asserted after the release so
        // a failure cannot strand the writer thread mid-hook.
        let in_window = load_sidecar(&torn_sidecar);
        let _ = release_tx.send(());
        writer_thread
            .join()
            .expect("the writer thread joins")
            .expect("the paused write lands");
        let SidecarLoad::Current(prior_loaded) = in_window else {
            panic!("the in-window load returns the prior Current, got {in_window:?}");
        };
        assert_eq!(prior_loaded.records.len(), 2, "prior bytes, never torn");
        assert!(
            sidecar_matches_project(&prior_loaded, &digest),
            "the prior pair still verifies in the window"
        );
        assert_eq!(
            fs::read(&torn_sidecar).expect("the landed sidecar reads"),
            build_sidecar_bytes(&[], &[], &digest, &digest).expect("the next sidecar rebuilds"),
            "the release lands the new bytes"
        );

        // BR40 at open: an oversize `format_version` refuses as `Corrupt`
        // (never `Newer`), with 1 incident and the bytes kept in `.bak`.
        let oversize_dir = TempDirectory::new("in2b-arm-oversize");
        let oversize_project = sidecar_project_file(&oversize_dir, "edit.kinewright");
        let oversize_sidecar =
            sidecar_path_for_project(Some(&oversize_project)).expect("a saved project derives");
        let oversize_bytes = r#"{"format_version":99999999999,"project_digest":"aa","previous_digest":"aa","records":[]}"#;
        fs::write(&oversize_sidecar, oversize_bytes).expect("the oversize sidecar writes");
        assert!(matches!(
            load_sidecar(&oversize_sidecar),
            SidecarLoad::Corrupt(_)
        ));
        let mut session = sidecar_session(31, &oversize_project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(log.open_count(), 1);
            assert_eq!(
                log.all().next().expect("the refusal noted").code,
                IncidentCode::Label(LabelIncident::SidecarRefused)
            );
        }
        let oversize_bak = oversize_sidecar.with_extension("kinewright-incidents.bak");
        assert_eq!(
            fs::read(&oversize_bak).expect("bak reads"),
            oversize_bytes.as_bytes()
        );
        shutdown_test_session(&mut session);
    }

    /// `canonical_session_key` collapses non-canonical spellings of one file
    /// (N2/S-13): `.`/`..` segments resolve, so the one-session guard sees
    /// through them. A missing file keys by its raw path — still matching
    /// itself.
    #[test]
    fn canonical_session_key_collapses_spellings_of_one_file() {
        let temporary = TempDirectory::new("in2b-canonical-key");
        let file = temporary.path("edit.kinewright");
        fs::write(&file, b"{}").expect("the file writes");
        fs::create_dir(temporary.path("sub")).expect("the segment exists");
        let alias = temporary
            .root()
            .join("sub")
            .join("..")
            .join("edit.kinewright");
        assert_ne!(alias, file, "the spellings really differ");
        assert_eq!(
            canonical_session_key(&alias),
            canonical_session_key(&file),
            "one file, one key"
        );
        let missing = temporary.path("gone.kinewright");
        assert_eq!(
            canonical_session_key(&missing),
            missing,
            "a missing file keys by its raw path"
        );
    }

    /// `can_overwrite_save` gates on the read version (`IN2B` §4 rule 3):
    /// current and older overwrite, newer never does.
    #[test]
    fn can_overwrite_save_gates_on_the_read_version() {
        assert!(can_overwrite_save(0));
        assert!(can_overwrite_save(1));
        assert!(can_overwrite_save(PROJECT_FORMAT_VERSION));
        assert!(!can_overwrite_save(PROJECT_FORMAT_VERSION + 1));
        assert!(!can_overwrite_save(999));
        assert!(!can_overwrite_save(u32::MAX));
    }

    /// Item 19: the legacy fixture round-trips to the pinned bytes through
    /// the envelope — and a v1 write emits no version key at all.
    #[test]
    fn in2b_v1_project_bytes_are_identical() {
        let fixture: &[u8] =
            include_bytes!("../../kinewright-core/tests/fixtures/pre_m13_project.json");
        // Through the envelope: legacy bytes parse with the version default.
        let file: ProjectFile = serde_json::from_slice(fixture).expect("the legacy fixture parses");
        assert_eq!(file.format_version, 1);
        assert!(file.document.investigator.is_none());
        // The pinned round trip (the IN2 §9.1 item 12 shape, now read through
        // the envelope): compact bytes, pinned length, pinned FNV digest via
        // the production digest fn.
        let round_tripped = serde_json::to_vec(&file.document).expect("the document serialises");
        assert_eq!(round_tripped.len(), 1_215);
        assert_eq!(
            digest_bytes(&round_tripped),
            "c9da3186e131e4fd",
            "the fixture keeps its pinned digest through the envelope"
        );
        // A v1 write is byte-identical to today: the key is skipped.
        let pretty = serialize_project_document(&file.document).expect("the envelope serialises");
        assert!(
            !pretty.contains("format_version"),
            "v1 writes emit no version key"
        );
        let reparsed: ProjectFile =
            serde_json::from_str(&pretty).expect("the envelope output re-parses");
        assert_eq!(reparsed.format_version, 1);
        assert_eq!(reparsed.document, file.document);
    }

    /// N6.1/J6: the atomic write resolves symlinks — the temp lands
    /// beside the real file and the rename replaces it, so the link
    /// itself survives the save.
    #[cfg(unix)]
    #[test]
    fn project_atomic_write_keeps_the_symlink() {
        let dir = TempDirectory::new("in2b-j6-symlink");
        let real = dir.path("real.kinewright");
        let link = dir.path("link.kinewright");
        fs::write(&real, b"old").expect("the target writes");
        std::os::unix::fs::symlink(&real, &link).expect("the link lands");
        write_file_atomic(&link, b"new").expect("the atomic write succeeds");
        assert!(
            fs::symlink_metadata(&link)
                .expect("the link stats")
                .file_type()
                .is_symlink(),
            "the link itself survives the save"
        );
        assert_eq!(
            fs::read(&link).expect("the link reads"),
            b"new",
            "the target carries the new bytes"
        );
    }

    /// N6.1/J6: the atomic write copies the existing file's permissions
    /// onto the temp, so the mode survives the inode swap. The 755 mode
    /// is red on any umask — a fresh temp never carries exec bits.
    #[cfg(unix)]
    #[test]
    fn project_atomic_write_keeps_the_file_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDirectory::new("in2b-j6-permissions");
        let file = dir.path("edit.kinewright");
        fs::write(&file, b"old").expect("the file writes");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod 755");
        write_file_atomic(&file, b"new").expect("the atomic write succeeds");
        assert_eq!(fs::read(&file).expect("the file reads"), b"new");
        let mode = fs::metadata(&file)
            .expect("the file stats")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "the mode survives the save");
    }

    /// N6/H3: a failed `.bak` rename suspends sidecar writes for the
    /// session and carries the IO error in the note — the refused file is
    /// never overwritten. The failure is injected: no portable fixture
    /// fails a real rename.
    #[test]
    fn in2b_failed_bak_rename_suspends_and_keeps_the_file() {
        let dir = TempDirectory::new("in2b-h3-refuse-fail");
        let project = sidecar_project_file(&dir, "edit.kinewright");
        let digest = project_digest_of(&project);
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let corrupt = b"{ torn";
        fs::write(&sidecar, corrupt).expect("the corrupt fixture writes");
        let failing = |_: &Path, _: &Path| -> std::io::Result<()> {
            Err(std::io::Error::other("injected refuse failure"))
        };
        let playback: Arc<dyn Playback> = Arc::new(StubMedia);
        let analysis: Arc<dyn Analysis> = Arc::new(StubMedia);
        let exporter: Arc<dyn Export> = Arc::new(StubMedia);
        let mut session = ProjectSession::create(
            31,
            "refuse-test",
            Document::default(),
            Some(project.clone()),
            &playback,
            &analysis,
            &exporter,
            &SidecarMode::Load {
                project_digest: digest.clone(),
            },
            None,
            PROJECT_FORMAT_VERSION,
            Some(&failing),
        )
        .expect("the session builds");
        assert!(
            session.sidecar_suspended,
            "a failed refuse suspends the session"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the stem reads"),
            corrupt,
            "the refused file stays at the stem"
        );
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let refused = log
                .all()
                .find(|incident| {
                    incident.code == IncidentCode::Label(LabelIncident::SidecarRefused)
                })
                .expect("the refusal notes");
            assert!(
                refused.observed.contains("injected refuse failure"),
                "the note carries the IO error: {}",
                refused.observed
            );
        }
        assert_eq!(
            session
                .flush_incidents(&digest, &digest)
                .expect("the flush reports"),
            FlushOutcome::Skipped,
            "a suspended session writes nothing"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the stem re-reads"),
            corrupt,
            "never overwritten"
        );
        shutdown_test_session(&mut session);
    }

    /// N6/H4: the loaded sets hold restored ids only — the unknown-codes
    /// aggregate the restore noted is not a restored row.
    #[test]
    fn in2b_loaded_sets_exclude_the_unknown_codes_aggregate() {
        use kinewright_core::{IncidentRecord, IncidentTelemetry};

        let dir = TempDirectory::new("in2b-h4-aggregate");
        let project = sidecar_project_file(&dir, "edit.kinewright");
        let digest = project_digest_of(&project);
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let mut records = two_records();
        records.push(IncidentRecord {
            id: IncidentId(9),
            code: "zz_unknown_code".to_owned(),
            subject: IncidentSubject::Project,
            observed: "from a newer build".to_owned(),
            allowed: None,
            evidence: IncidentEvidence::Plain,
            revision: TimelineRevision(43),
            opened_wall_millis: None,
            opened_offset_nanos: 9_000,
            count: 1,
            state: IncidentState::Open,
            telemetry: IncidentTelemetry::default(),
            proposal: None,
            subject_name: None,
            refused_op: None,
        });
        let bytes =
            build_sidecar_bytes(&records, &[], &digest, &digest).expect("the sidecar builds");
        fs::write(&sidecar, bytes).expect("the sidecar writes");
        let mut session = sidecar_session(41, &project);
        let aggregate = {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.all()
                .find(|incident| {
                    incident.code == IncidentCode::Label(LabelIncident::SidecarUnknownCodes)
                })
                .expect("the aggregate noted")
                .id
        };
        assert!(session.loaded_ids.contains(&IncidentId(1)));
        assert!(session.loaded_ids.contains(&IncidentId(2)));
        assert_eq!(session.loaded_ids.len(), 2, "two restored rows");
        assert_eq!(session.loaded_open_ids.len(), 2, "both open");
        assert!(
            !session.loaded_ids.contains(&aggregate),
            "the aggregate is not a restored id"
        );
        assert!(
            !session.loaded_open_ids.contains(&aggregate),
            "nor a loaded open"
        );
        shutdown_test_session(&mut session);
    }

    /// N6/H4: a refused load restores nothing — the `sidecar_refused` note
    /// is not a restored row either.
    #[test]
    fn in2b_loaded_sets_exclude_the_refusal_note() {
        let dir = TempDirectory::new("in2b-h4-refused");
        let project = sidecar_project_file(&dir, "edit.kinewright");
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        fs::write(&sidecar, b"{ torn").expect("the corrupt fixture writes");
        let mut session = sidecar_session(42, &project);
        {
            let log = session
                .incidents
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(
                log.all()
                    .any(|incident| incident.code
                        == IncidentCode::Label(LabelIncident::SidecarRefused)),
                "the refusal notes"
            );
        }
        assert!(
            session.loaded_ids.is_empty(),
            "nothing restored, nothing loaded"
        );
        assert!(session.loaded_open_ids.is_empty(), "nor loaded open");
        shutdown_test_session(&mut session);
    }

    /// N6/H6: a failed background flush retries at close — close/exit
    /// write-and-wait whenever the confirmed generation trails the log.
    #[test]
    fn in2b_a_failed_background_flush_retries_at_close() {
        use kinewright_project::SidecarLoad;

        let dir = TempDirectory::new("in2b-h6-retry");
        let project = sidecar_project_file(&dir, "edit.kinewright");
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let mut session = sidecar_session(43, &project);
        {
            let mut log = session
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "in2b h6 open",
                TimelineRevision::default(),
            ));
        }
        // The portable failure (item 36's shape): a directory at the stem
        // fails temp + rename on both lanes.
        fs::create_dir(&sidecar).expect("the stem blocks");
        session.queue_incidents_flush();
        // The background job fails asynchronously: wait for its error, so
        // the unblock below cannot accidentally let it succeed.
        let expiry = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if !session.sidecar_writer.take_errors().is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < expiry,
                "the background write reports its failure"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        fs::remove_dir(&sidecar).expect("the stem unblocks");
        session.stop_threads("in2b h6 close");
        let SidecarLoad::Current(current) = load_sidecar(&sidecar) else {
            panic!("the close retry landed a sidecar");
        };
        assert_eq!(
            current.records.len(),
            1,
            "the retried flush carries the history"
        );
        shutdown_test_session(&mut session);
    }

    /// N6/H10 survivor 6: the write drops refused ops for closed incidents —
    /// only the open ids' stashes ride.
    #[test]
    fn in2b_sidecar_bytes_drop_refused_ops_for_closed_incidents() {
        use kinewright_core::{IncidentOutcome, Operation};

        let dir = TempDirectory::new("in2b-h10-retain");
        let project = sidecar_project_file(&dir, "edit.kinewright");
        let mut session = sidecar_session(44, &project);
        {
            let mut log = session
                .incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "first open",
                TimelineRevision(41),
            ));
            log.observe(IncidentObservation::plain(
                IncidentCode::Label(LabelIncident::Project),
                IncidentSubject::Project,
                "second open",
                TimelineRevision(42),
            ));
            assert!(log.resolve(IncidentId(1), IncidentOutcome::Explained));
        }
        session.refused_by_id.insert(
            IncidentId(1),
            Operation::DeleteClip {
                clip: kinewright_core::ClipId(1),
            },
        );
        session.refused_by_id.insert(
            IncidentId(2),
            Operation::DeleteClip {
                clip: kinewright_core::ClipId(2),
            },
        );
        session
            .sidecar_bytes_for_save("aa", "aa")
            .expect("the bytes build");
        assert_eq!(session.refused_by_id.len(), 1, "the closed stash drops");
        assert!(
            session.refused_by_id.contains_key(&IncidentId(2)),
            "the open stash rides"
        );
        shutdown_test_session(&mut session);
    }

    /// N6/H10 survivor 7a: `loaded_walls` snapshots every restored stamp —
    /// the card's recency reads app-side, never core's private wall.
    #[test]
    fn in2b_loaded_walls_snapshot_the_restored_stamps() {
        let dir = TempDirectory::new("in2b-h10-walls");
        let project = sidecar_project_file(&dir, "edit.kinewright");
        let digest = project_digest_of(&project);
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let bytes =
            build_sidecar_bytes(&two_records(), &[], &digest, &digest).expect("the sidecar builds");
        fs::write(&sidecar, bytes).expect("the sidecar writes");
        let mut session = sidecar_session(45, &project);
        assert_eq!(
            session.loaded_walls.len(),
            2,
            "every restored row snapshots"
        );
        assert!(session.loaded_walls.contains_key(&IncidentId(1)));
        assert!(session.loaded_walls.contains_key(&IncidentId(2)));
        assert!(
            session.loaded_walls.values().all(Option::is_some),
            "stamped rows snapshot stamps"
        );
        shutdown_test_session(&mut session);
    }

    /// N6/H10 survivor 7b: the load baselines the generation it restored —
    /// no write follows a load with no new notes.
    #[test]
    fn in2b_load_baselines_the_generation_it_restored() {
        let dir = TempDirectory::new("in2b-h10-baseline");
        let project = sidecar_project_file(&dir, "edit.kinewright");
        let digest = project_digest_of(&project);
        let sidecar = sidecar_path_for_project(Some(&project)).expect("a saved project derives");
        let bytes =
            build_sidecar_bytes(&two_records(), &[], &digest, &digest).expect("the sidecar builds");
        fs::write(&sidecar, bytes).expect("the sidecar writes");
        let before = fs::read(&sidecar).expect("the sidecar reads");
        let mut session = sidecar_session(46, &project);
        assert_eq!(
            session
                .flush_incidents_if_changed()
                .expect("the flush reports"),
            FlushOutcome::Skipped,
            "a load with no new notes writes nothing"
        );
        assert_eq!(
            fs::read(&sidecar).expect("the sidecar re-reads"),
            before,
            "byte-identical, not just skipped"
        );
        shutdown_test_session(&mut session);
    }
}
