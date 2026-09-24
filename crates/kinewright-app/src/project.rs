use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime},
};

use serde::{Deserialize, Serialize};

use kinewright_core::{
    Analysis, AssetId, AudioChain, ClipId, Core, Document, Event, Export, IncidentCode,
    IncidentEvidence, IncidentId, IncidentLog, IncidentObservation, IncidentState, IncidentSubject,
    LabelIncident, LutAssetId, LutAvailabilityKind, LutAvailabilityStatus, MarkerId, MediaKind,
    Operation, PROJECT_FORMAT_VERSION, Playback, RejectionIncident, RestoreReport,
    RunningInvestigation, TimeCode, TimelineRevision, TrackId, TrackKind, should_flush,
};
use kinewright_media::{LutLibrary, LutStore};

use crate::{
    chat_ui::{AgentHarnessChoice, AgentThread, ChatEntry},
    investigator::InvestigatorSession,
    recovery::Recovery,
    sidecar::{
        FlushOutcome, RefuseRename, SidecarLoad, SidecarMode, SidecarWriter, build_sidecar_bytes,
        digest_bytes, load_sidecar, refuse_sidecar, refuse_sidecar_with, sidecar_matches_project,
        sidecar_path_for_project, sidecar_refused_observation, sidecar_write_failed_observation,
    },
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
    /// The FNV-1a pairing digest over the exact bytes written (`IN2B` §4
    /// rule 2, §2 rule 9): the save path hands it to the sidecar flush, so
    /// one serialisation serves both files.
    pub(crate) digest: String,
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
    /// Overwrite-save refused: the session read a newer file (`IN2B` §4
    /// rule 3). The save path re-surfaces the rule-4 card instead of noting,
    /// so this variant never opens a second incident.
    NewerFormat {
        read_version: u32,
    },
    /// Overwrite-save refused: route to Save As (N6/H2 — a suspended
    /// session, or a path open in another session, cannot overwrite its
    /// file). The save path posts the notice to the status line instead of
    /// noting: a refusal, not an incident.
    SaveAsRequired {
        notice: String,
    },
    /// Save As refused: the target path is open in another session (N6/H5).
    /// Same status-line treatment as [`Self::SaveAsRequired`].
    PathOpenElsewhere {
        notice: String,
    },
}

impl std::fmt::Display for ProjectSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(reason) => {
                write!(formatter, "could not serialize the project: {reason}")
            }
            Self::Write(reason) => write!(formatter, "could not write the project file: {reason}"),
            Self::NewerFormat { read_version } => write!(
                formatter,
                "saving over this project is disabled: it was written by a newer Kinewright \
                 (format_version {read_version}) — use Save As"
            ),
            Self::SaveAsRequired { notice } | Self::PathOpenElsewhere { notice } => {
                write!(formatter, "{notice}")
            }
        }
    }
}

impl std::error::Error for ProjectSaveError {}

impl ProjectSaveError {
    /// The incident code a failed project write carries
    /// (`IN1b` §2.2 rule 8).
    ///
    /// Declared in the app crate, which owns the trigger; core owns the code
    /// and the policy class (IN1 §2.1 rule 2).
    pub(crate) const fn incident_code(&self) -> IncidentCode {
        match self {
            // The refusal variants are unreachable in practice: the save path
            // intercepts them before noting (belt and braces, as for
            // `NewerFormat`). A refusal must never open an incident — they
            // share the save-failure code only so the type stays total.
            Self::Serialize(_)
            | Self::Write(_)
            | Self::SaveAsRequired { .. }
            | Self::PathOpenElsewhere { .. } => {
                IncidentCode::Rejection(RejectionIncident::ProjectSave)
            }
            Self::NewerFormat { .. } => IncidentCode::Label(LabelIncident::ProjectNewerFormat),
        }
    }

    /// An observation from this refusal, with the caller's subject.
    ///
    /// `ProjectSave { reason }` evidence keeps which half failed —
    /// serialising or writing — which is what the written body tells the
    /// person to act on. The newer-format arm builds the shared rule-4
    /// observation instead (same text as the open-time note, so even a
    /// double-note dedups) — the save path intercepts the variant before it
    /// can note, so this arm is belt and braces.
    pub(crate) fn incident_observation(
        &self,
        subject: IncidentSubject,
        revision: TimelineRevision,
    ) -> IncidentObservation {
        if let Self::NewerFormat { read_version } = self {
            let mut observation = project_newer_format_observation(*read_version, revision);
            observation.subject = subject;
            return observation;
        }
        IncidentObservation {
            code: self.incident_code(),
            subject,
            observed: self.to_string(),
            allowed: None,
            evidence: IncidentEvidence::ProjectSave {
                reason: self.to_string(),
            },
            revision,
            name: None,
            transient: false,
        }
    }
}

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
/// The default `format_version`: every legacy file without the key reads as 1
/// (`IN2B` §4 rule 1). A named fn, so the default is greppable, not magic.
fn v1() -> u32 {
    1
}

/// Whether a version serialises away: v1 writes are byte-identical to the
/// pre-envelope shape, so the key appears only after the first real bump.
/// Takes the reference because `skip_serializing_if` mandates `fn(&u32)`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_v1(version: &u32) -> bool {
    *version == 1
}

/// A project file on disk: the app-side envelope over the core-owned version
/// const (`IN2B` §4 rule 1).
///
/// `#[serde(flatten)]` keeps the document's fields at top level with the
/// version key first; missing parses as 1 (every legacy file) and a v1 write
/// skips the key entirely. No `Document` field, no struct-level serde
/// attribute on `Document`, no 55-site pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProjectFile {
    #[serde(default = "v1", skip_serializing_if = "is_v1")]
    pub format_version: u32,
    #[serde(flatten)]
    pub document: Document,
}

/// Whether the session that read `read_version` may save over its own file
/// (`IN2B` §4 rule 3).
///
/// Newer files open (rule 4) but never overwrite: the in-memory document has
/// already lost the newer writer's fields at parse, and overwriting would
/// launder that loss into the original. Save As stays enabled — the loss goes
/// into a new file the person chose, with the card explaining.
#[must_use]
pub(crate) fn can_overwrite_save(read_version: u32) -> bool {
    read_version <= PROJECT_FORMAT_VERSION
}

/// The observation a newer-format encounter notes: exactly one
/// `project_newer_format` per file (`IN2B` §4 rules 4–5, §5 rule 1 #4).
///
/// One constructor for the newer-file open, the cross-version recovery
/// refusal, and the refused overwrite-save — same code, same subject, same
/// text for the same version, so even a double-note dedups instead of
/// doubling. Transient, like every §5 note.
#[must_use]
pub(crate) fn project_newer_format_observation(
    read_version: u32,
    revision: TimelineRevision,
) -> IncidentObservation {
    let mut observation = IncidentObservation::plain(
        IncidentCode::Label(LabelIncident::ProjectNewerFormat),
        IncidentSubject::Project,
        format!(
            "this project was written by a newer Kinewright (format_version {read_version}); \
             saving over it is disabled — use Save As"
        ),
        revision,
    );
    observation.transient = true;
    observation
}

/// The canonical key two project paths share iff they name one file (N2/S-13).
///
/// `canonicalize` resolves symlinks, `.`/`..` and (on Windows) case; when the
/// file is gone the raw path is the key, which still matches itself.
#[must_use]
pub(crate) fn canonical_session_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Serialise a document inside the file envelope (`IN2B` §4 rules 1–2).
///
/// The writer stamps its own `PROJECT_FORMAT_VERSION` const — never the
/// session's read version: the bytes are this build's, whatever it opened.
pub(crate) fn serialize_project_document(document: &Document) -> Result<String, ProjectSaveError> {
    let file = ProjectFile {
        format_version: PROJECT_FORMAT_VERSION,
        document: document.clone(),
    };
    serde_json::to_string_pretty(&file)
        .map_err(|error| ProjectSaveError::Serialize(error.to_string()))
}

/// Compose the halves: serialize, then write (`IN2B` §4 rule 2).
///
/// The fixture-writing tests use this; production `write_project` calls the
/// halves directly so the sidecar flush (which needs the digest) lands
/// between them.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_project_document(
    document: &Document,
    path: &Path,
    previous_store: Option<&LutStore>,
) -> Result<ProjectSaveReport, ProjectSaveError> {
    let json = serialize_project_document(document)?;
    write_project_bytes(&json, document, path, previous_store)
}

/// Write bytes atomically: temp beside the target, then rename (N6/H12).
/// A failed rename removes its temp, best-effort, so failures do not
/// litter the project directory. The temp carries the process id; project
/// writes are synchronous, so no sequence is needed.
///
/// N6.1/J6: the write targets the symlink's resolved path, the temp
/// inherits the existing file's permissions and syncs before the rename,
/// and a permission/sharing rename failure falls back to the pre-H12
/// in-place write — never worse than before H12.
pub(crate) fn write_file_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_file_atomic_with_rename(path, contents, None)
}

/// Whether a rename failure is the permission/sharing kind the atomic
/// write falls back from (N6.1/J6).
fn is_permission_or_sharing(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied || raw_sharing_violation(error)
}

/// Windows' sharing-violation code, which some builds report under a
/// non-permission kind (N6.1/J6). No code elsewhere.
#[cfg(windows)]
fn raw_sharing_violation(error: &std::io::Error) -> bool {
    // ERROR_SHARING_VIOLATION.
    error.raw_os_error() == Some(32)
}

/// Windows' sharing-violation code, which some builds report under a
/// non-permission kind (N6.1/J6). No code elsewhere.
#[cfg(not(windows))]
fn raw_sharing_violation(_error: &std::io::Error) -> bool {
    false
}

/// [`write_file_atomic`] with the rename injected (N6.1/J6): `None`
/// renames for real. The seam exists so the permission-failure fallback
/// has a portable test — no portable fixture fails a real rename.
fn write_file_atomic_with_rename(
    path: &Path,
    contents: &[u8],
    rename: Option<&RefuseRename>,
) -> std::io::Result<()> {
    // The write targets the symlink's resolved path — the temp lands
    // beside the real file and the rename replaces it, so the link
    // itself survives the save. An unresolvable path writes as given.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // The temp inherits the existing file's permissions, so the mode
    // survives the inode swap. A first save keeps fresh-file perms.
    let permissions = fs::metadata(&target).ok().map(|file| file.permissions());
    let mut temp = target.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    // The temp's bytes reach the disk before the rename does, so a crash
    // between the two cannot surface torn bytes (the H9 shape).
    crate::sidecar::write_synced(&temp, contents)?;
    if let Some(permissions) = permissions
        && let Err(error) = fs::set_permissions(&temp, permissions)
    {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let renamed = match rename {
        Some(hook) => hook(&temp, &target),
        None => fs::rename(&temp, &target),
    };
    if let Err(error) = renamed {
        let _ = fs::remove_file(&temp);
        if is_permission_or_sharing(&error) {
            return fs::write(path, contents);
        }
        return Err(error);
    }
    Ok(())
}

/// [`write_project_document`], split so the save path can flush the sidecar
/// (which needs the digest) between serialising and writing, with one
/// serialisation total (`IN2B` §4 rule 2, §2 rule 9).
///
/// The digest in the report is over these exact bytes. `document` rides along
/// for the Save As asset copy only — the bytes on disk come from `json`.
pub(crate) fn write_project_bytes(
    json: &str,
    document: &Document,
    path: &Path,
    previous_store: Option<&LutStore>,
) -> Result<ProjectSaveReport, ProjectSaveError> {
    write_file_atomic(path, json.as_bytes())
        .map_err(|error| ProjectSaveError::Write(error.to_string()))?;
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
        digest: digest_bytes(json.as_bytes()),
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
    /// The incident log every MCP server in this session shares, in the shape
    /// `agent_project_path` already uses (CC4 §2.2).
    pub(crate) incidents: IncidentLogHandle,
    /// The digest of the project bytes on disk as this session last saw them
    /// (`IN2B` §2 rule 9). Recovery restores seed it from the file on disk
    /// (N6/H1); `""` means no save is known (unsaved projects, refused
    /// loads, file-less recoveries). Updated on every save.
    pub(crate) saved_digest: String,
    /// The app's ONE sidecar writer thread, shared by every session (N2/B-5).
    /// Headless tests that pass `None` get a private writer for isolation.
    pub(crate) sidecar_writer: Arc<SidecarWriter>,
    /// Raw sidecar record texts carried verbatim across this run (`IN2B` §2
    /// rule 8b, N4/F3): re-emitted byte-equal on every write, never pruned.
    pub(crate) carried_sidecar_records: Vec<String>,
    /// Refused opening-context operations by incident id (`IN2B` §3 rule 13):
    /// filled from `RestoreReport.refused` at load; the queue drain fills it
    /// at write, and the first session for the incident consumes it (C5).
    pub(crate) refused_by_id: BTreeMap<IncidentId, Operation>,
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
    /// The log generation the last flush wrote, for [`should_flush`].
    pub(crate) last_written_gen: u64,
    /// The log generation the writer confirmed (N6/H6): advanced on a
    /// joined success, never on submit. Close/exit write-and-wait whenever
    /// this trails the log, so a failed background flush retries at close
    /// instead of reading as done.
    pub(crate) confirmed_written_gen: u64,
    /// What the open-time sidecar load did, when one ran. `None` for new
    /// projects and refused loads; the report's `carried`/`refused` halves
    /// move into the fields above, the counts stay here for the gate.
    ///
    /// Read by the gate (item 14) and by nothing else: production consumes
    /// the halves, not the counts.
    #[allow(dead_code)]
    pub(crate) last_restore_report: Option<RestoreReport>,
    /// The `format_version` this session read (`IN2B` §4 rule 3): the
    /// envelope's version at open, or the journal's writer version after a
    /// recovery restore. Gates overwrite-save via [`can_overwrite_save`];
    /// Save As resets it to [`PROJECT_FORMAT_VERSION`] — the bytes on the
    /// new path are this build's.
    pub(crate) format_version: u32,
    /// Sidecar writes suspended: a recovery restore onto an already-open
    /// path must not clobber the open session's history (`IN2B` §2 rule 10,
    /// N2/S-13), and a refused sidecar the load could not move aside must
    /// never be overwritten (N6/H3). Loads still run; every flush reports
    /// `Skipped` until Save As clears this on the new path.
    pub(crate) sidecar_suspended: bool,
    /// The recovery cause of [`Self::sidecar_suspended`] (N6.1/J5): only a
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceMonitorState {
    selected_asset: Option<AssetId>,
    source_position: TimeCode,
    source_in: TimeCode,
    source_out: TimeCode,
    source_video_target: Option<TrackId>,
    source_audio_target: Option<TrackId>,
}

/// What the open-time sidecar load hands the new session (`IN2B` §2 rule 5).
struct LoadedSessionSidecar {
    saved_digest: String,
    carried: Vec<String>,
    refused: BTreeMap<IncidentId, Operation>,
    walls: BTreeMap<IncidentId, Option<i64>>,
    last_written_gen: u64,
    report: Option<RestoreReport>,
    /// A refused sidecar the load could not move aside (N6/H3): the session
    /// suspends sidecar writes, so the still-at-the-stem file is never
    /// overwritten.
    suspended: bool,
}

impl LoadedSessionSidecar {
    fn empty() -> Self {
        Self {
            saved_digest: String::new(),
            carried: Vec::new(),
            refused: BTreeMap::new(),
            walls: BTreeMap::new(),
            last_written_gen: 0,
            report: None,
            suspended: false,
        }
    }
}

/// Load a session's history at creation: parse, gate, restore or refuse.
///
/// Every arm opens the project — incidents are recoverable state, the
/// timeline is not — and no arm deletes a sidecar it cannot read (rule 8).
/// Refusals rename to first-free `.bak` and note exactly one
/// `sidecar_refused` directly into the still-private log (the router is not
/// running yet; the code is off-allowlist, so no session could start
/// anyway). Restore runs before any note (N4.1).
///
/// `Load` carries the digest `load_document` read — single read, no TOCTOU
/// (§4 rule 2). `RecoveryNoDigest` skips the gate by rule; version arms
/// still apply.
fn load_session_sidecar(
    mode: &SidecarMode,
    project_path: Option<&Path>,
    incidents: &IncidentLogHandle,
    opening: TimelineRevision,
    rename: Option<&RefuseRename>,
) -> LoadedSessionSidecar {
    let (gate_digest, seed) = match mode {
        SidecarMode::Load { project_digest } => {
            (Some(project_digest.clone()), Some(project_digest.clone()))
        }
        // N6/H1: recovery skips the gate (the recovered document is newer
        // than the last save), but the seed still comes from the file on
        // disk — the first flush pairs instead of writing `{"",""}`.
        SidecarMode::RecoveryNoDigest => (
            None,
            project_path
                .and_then(|path| fs::read(path).ok())
                .map(|bytes| digest_bytes(&bytes)),
        ),
        SidecarMode::None => return LoadedSessionSidecar::empty(),
    };
    let Some(project_path) = project_path else {
        return LoadedSessionSidecar::empty();
    };
    let Some(sidecar_path) = sidecar_path_for_project(Some(project_path)) else {
        return LoadedSessionSidecar::empty();
    };
    let mut loaded = LoadedSessionSidecar::empty();
    if let Some(digest) = &seed {
        loaded.saved_digest.clone_from(digest);
    }
    // N6/H3: a failed `.bak` rename suspends the session — the refused
    // file stays at the stem and must never be overwritten — and the note
    // carries the IO error. Returns whether the rename failed.
    let refused = |reason: String| -> bool {
        let renamed = match rename {
            Some(injected) => refuse_sidecar_with(&sidecar_path, injected),
            None => refuse_sidecar(&sidecar_path),
        };
        let (reason, failed) = match renamed {
            Ok(_) => (reason, false),
            Err(error) => (
                format!(
                    "{reason}; the refused file could not be moved aside ({error}), so the \
                     incidents file is set aside and sidecar writes are suspended for this session"
                ),
                true,
            ),
        };
        let mut log = incidents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = log.observe(sidecar_refused_observation(reason, opening));
        failed
    };
    let mut refuse_failed = false;
    match load_sidecar(&sidecar_path) {
        SidecarLoad::Absent => {}
        SidecarLoad::Newer(version) => {
            refuse_failed |= refused(format!(
                "the sidecar was written by a newer Kinewright (format_version {version}); \
                 the project opens with an empty history and the file is kept beside it"
            ));
        }
        SidecarLoad::Corrupt(reason) => {
            refuse_failed |= refused(format!(
                "the sidecar could not be read ({reason}); \
                 the project opens with an empty history and the file is kept beside it"
            ));
        }
        SidecarLoad::Current(current) => {
            let gated = match &gate_digest {
                Some(digest) => sidecar_matches_project(&current, digest),
                None => true,
            };
            if !gated {
                refuse_failed |= refused(
                    "the sidecar belongs to a different project file (neither digest matches); \
                     the project opens with an empty history and the file is kept beside it"
                        .to_owned(),
                );
                loaded.suspended = refuse_failed;
                return loaded;
            }
            // N6/H1 fallback: no file on disk (an unsaved recovery target)
            // seeds from the loaded sidecar's own digest instead of `""`.
            // The `gate_digest.is_none()` guard restricts this to recovery
            // mode — a gated Load keeps whatever its read seeded, so the
            // fallback cannot mask a refusal.
            if loaded.saved_digest.is_empty() && gate_digest.is_none() {
                loaded.saved_digest.clone_from(&current.project_digest);
            }
            // The loaded wall stamps snapshot before the records move into
            // restore (`IN2B` §3 rule 14): the card's recency derives from
            // them app-side.
            loaded.walls = current
                .records
                .iter()
                .map(|record| (record.id, record.opened_wall_millis))
                .collect();
            let mut log = incidents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let report = log.restore(current.records, current.carried, opening, current.id_floor);
            // The disk now matches memory (carried bytes included — a
            // carried-only restore moves no generation, and must not force a
            // rewrite of bytes already on disk), so the flush baseline is the
            // post-restore generation, whatever it is.
            loaded.last_written_gen = log.generation();
            loaded.carried.clone_from(&report.carried);
            loaded.refused.clone_from(&report.refused);
            loaded.report = Some(report);
        }
    }
    loaded.suspended = refuse_failed;
    loaded
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
        let sidecar_writer = sidecar_writer.unwrap_or_else(SidecarWriter::new);
        let loaded = load_session_sidecar(
            sidecar_mode,
            project_path.as_deref(),
            &incidents,
            TimelineRevision::default(),
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
            incidents: std::sync::Arc::clone(&incidents),
            saved_digest: loaded.saved_digest,
            sidecar_writer,
            carried_sidecar_records: loaded.carried,
            refused_by_id: loaded.refused,
            loaded_ids,
            loaded_open_ids,
            subject_missing,
            loaded_walls: loaded.walls,
            last_written_gen: loaded.last_written_gen,
            confirmed_written_gen: loaded.last_written_gen,
            last_restore_report: loaded.report,
            format_version,
            sidecar_suspended: loaded.suspended,
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

    /// Build this session's sidecar bytes without writing them (`IN2B` §2
    /// rule 5).
    ///
    /// Records are built on the caller under a read lock; the running
    /// session's turns and visible counters flush into an `Investigating`
    /// entry exactly as a live end would (§3 rule 4), the restored stash
    /// rides by id (rule 13), and carried texts re-emit verbatim (rule 8b).
    /// Both the joining flush and the background submit build through here,
    /// so one builder means one bytes shape.
    pub(crate) fn sidecar_bytes_for_save(
        &mut self,
        project_digest: &str,
        previous_digest: &str,
    ) -> Result<(Vec<u8>, kinewright_core::WriteReport), String> {
        let running: Option<RunningInvestigation> = self
            .investigator
            .as_ref()
            .and_then(InvestigatorSession::running_investigation);
        // The queue drain fills the stash at write (`IN2B` §3 rule 13):
        // queued refused ops ride by id, and entries whose incidents are no
        // longer open drop — the stash round-trips only until its incident
        // resolves or its entry is consumed.
        if let Some(session) = self.investigator.as_ref() {
            session.copy_queued_refused_into(&mut self.refused_by_id);
        }
        let log = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let open: BTreeSet<IncidentId> = log.open().map(|incident| incident.id).collect();
        self.refused_by_id.retain(|id, _| open.contains(id));
        let (records, report) = log.records(running.as_ref(), &self.refused_by_id);
        let bytes = build_sidecar_bytes(
            &records,
            &self.carried_sidecar_records,
            project_digest,
            previous_digest,
        )?;
        Ok((bytes, report))
    }

    /// The one synchronous sidecar seam: build every write and land it
    /// through the writer thread, joining (`IN2B` §2 rules 4–5).
    ///
    /// `Ok` after a successful temp + rename (report populated), `Err` after
    /// a failed one — the caller notes `sidecar_write_failed` per rule 6 and
    /// carries on. Unsaved projects report `Skipped` and attempt no IO
    /// (rule 10, N-5), as do sidecar-suspended recovery restores (§2
    /// rule 10).
    pub(crate) fn flush_incidents(
        &mut self,
        project_digest: &str,
        previous_digest: &str,
    ) -> std::io::Result<FlushOutcome> {
        if self.sidecar_suspended {
            return Ok(FlushOutcome::Skipped);
        }
        let Some(sidecar_path) = sidecar_path_for_project(self.project_path.as_deref()) else {
            return Ok(FlushOutcome::Skipped);
        };
        let (bytes, report) = self
            .sidecar_bytes_for_save(project_digest, previous_digest)
            .map_err(std::io::Error::other)?;
        self.sidecar_writer.submit_and_join(sidecar_path, bytes)?;
        let generation = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation();
        self.last_written_gen = generation;
        // N6/H6: a joined success confirms; a failure propagates before
        // either baseline moves, so close retries it.
        self.confirmed_written_gen = generation;
        Ok(FlushOutcome::Written(report))
    }

    /// [`Self::flush_incidents`] when the writer has not confirmed the
    /// current generation, `Skipped` otherwise — the close/exit shape
    /// (`IN2B` §2 rule 4).
    ///
    /// Close write-and-waits whenever `confirmed_written_gen` trails the
    /// log (N6/H6): a failed background flush advanced the submit baseline
    /// but never confirmed, so it retries here instead of reading as done.
    /// A successful background flush still lands one redundant identical
    /// rewrite at close — async jobs carry no ack, and one idempotent write
    /// per close is cheaper than success tracking. Saves do not call this —
    /// a save always rewrites the pair over new bytes.
    pub(crate) fn flush_incidents_if_changed(&mut self) -> std::io::Result<FlushOutcome> {
        if self.sidecar_suspended || self.project_path.is_none() {
            return Ok(FlushOutcome::Skipped);
        }
        let generation = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation();
        if generation == self.confirmed_written_gen {
            return Ok(FlushOutcome::Skipped);
        }
        let digest = self.saved_digest.clone();
        self.flush_incidents(&digest, &digest)
    }

    /// Queue a debounced background flush without joining (`IN2B` §2 rule 4).
    ///
    /// Unchanged logs and unsaved projects queue nothing. Failures surface
    /// through [`SidecarWriter::take_errors`], which the frame thread drains
    /// into `sidecar_write_failed` notes. The submit baseline advances
    /// optimistically at submit — a failed submit's incident is the retry
    /// signal, not a 2 s resubmit churn — while `confirmed_written_gen`
    /// waits for a joined success, so close retries what the background
    /// could not land (N6/H6).
    pub(crate) fn queue_incidents_flush(&mut self) {
        if self.sidecar_suspended {
            return;
        }
        let Some(sidecar_path) = sidecar_path_for_project(self.project_path.as_deref()) else {
            return;
        };
        let generation = self
            .incidents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation();
        if !should_flush(generation, self.last_written_gen, Instant::now()) {
            return;
        }
        let digest = self.saved_digest.clone();
        if let Ok((bytes, _)) = self.sidecar_bytes_for_save(&digest, &digest) {
            self.sidecar_writer.submit(sidecar_path, bytes);
            self.last_written_gen = generation;
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
        if let Some(investigator) = self.investigator.as_mut() {
            let incidents = std::sync::Arc::clone(&self.incidents);
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
        LabelIncident, LutAsset, MediaAsset, MediaSourceFingerprint, Operation, ParamValue,
        Rational, Track, apply_batch,
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
        use crate::sidecar::SidecarLoad;

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

    /// N6.1/J6: a permission/sharing rename failure falls back to the
    /// pre-H12 in-place write — never worse than before H12 — while any
    /// other rename error still propagates. The failure is injected: no
    /// portable fixture fails a real rename.
    #[test]
    fn project_atomic_write_falls_back_when_rename_is_refused() {
        let dir = TempDirectory::new("in2b-j6-fallback");
        let file = dir.path("edit.kinewright");
        fs::write(&file, b"old").expect("the file writes");
        let refused = |_: &Path, _: &Path| -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected sharing violation",
            ))
        };
        write_file_atomic_with_rename(&file, b"new", Some(&refused))
            .expect("the fallback lands the bytes");
        assert_eq!(
            fs::read(&file).expect("the file reads"),
            b"new",
            "the in-place fallback lands the bytes"
        );
        let torn = |_: &Path, _: &Path| -> std::io::Result<()> {
            Err(std::io::Error::other("injected unsplittable failure"))
        };
        assert!(
            write_file_atomic_with_rename(&file, b"third", Some(&torn)).is_err(),
            "a non-permission rename error still propagates"
        );
        assert_eq!(
            fs::read(&file).expect("the file re-reads"),
            b"new",
            "the propagated failure writes nothing"
        );
        for entry in fs::read_dir(dir.root()).expect("the dir reads") {
            let entry = entry.expect("a readable entry");
            assert!(
                entry.path().extension().is_none_or(|ext| ext != "tmp"),
                "no temp litter: {}",
                entry.file_name().to_string_lossy()
            );
        }
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
        use crate::sidecar::SidecarLoad;

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
