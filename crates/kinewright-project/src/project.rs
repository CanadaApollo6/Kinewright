//! Project envelopes, atomic saves, and the load gate (AW1 §2), moved
//! from the app in AW1 S1. The app-only save prologue stays in the app;
//! headless saves orchestrate via [`crate::headless::save_headless`].

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use kinewright_core::{
    BlendMode, ClipContent, Document, IncidentCode, IncidentEvidence, IncidentObservation,
    IncidentSubject, LabelIncident, LutAssetId, PROJECT_FORMAT_VERSION, RejectionIncident,
    TimelineRevision,
};
use kinewright_media::LutStore;

use crate::sidecar::{RefuseRename, digest_bytes};

/// What one dialog-free project write did (CC4 §2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSaveReport {
    /// Where the project JSON was written.
    pub path: PathBuf,
    /// The store root derived from `path`, absent only when the path yields no
    /// usable root.
    pub lut_store_root: Option<PathBuf>,
    /// Whether the store root moved, which is what makes a write a Save As for
    /// the purposes of the asset copy.
    pub store_root_changed: bool,
    /// One entry per asset the Save As copy could not place at the new root.
    /// A project with an unavailable asset is still saved; the asset is simply
    /// `missing` there, with the ordinary recovery path (CC4 §2.2).
    pub lut_store_copy_failed: Vec<(LutAssetId, String)>,
    /// The typed `lut_store_root_invalid` refusal when the derived root exists
    /// but is unusable — a symlink, or something that is not a directory.
    ///
    /// Kept rather than discarded so the caller can say *why* the just-saved
    /// project still cannot own LUT bytes: reporting `project_not_saved` on a
    /// project that was saved a second ago is a lie (CC4 §2.2).
    pub lut_store_error: Option<String>,
    /// The FNV-1a pairing digest over the exact bytes written (`IN2B` §4
    /// rule 2, §2 rule 9): the save path hands it to the sidecar flush, so
    /// one serialisation serves both files.
    pub digest: String,
}

impl ProjectSaveReport {
    /// The human summary of any per-asset copy failure, or `None` when the
    /// whole store followed the project.
    #[must_use]
    pub fn copy_failure_summary(&self) -> Option<String> {
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
pub enum ProjectSaveError {
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
    /// Headless save refused: the session never loaded (AW1 S1 D5).
    SessionNotLoaded,
    /// Headless save refused: the lock object was deleted under the live
    /// saver (G9) — not writing. Only the `.lock.json` discovery may be
    /// hand-deleted.
    LockLost {
        path: PathBuf,
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
            Self::SessionNotLoaded => write!(
                formatter,
                "cannot save headless: the sidecar session never loaded its project history"
            ),
            Self::LockLost { path } => write!(
                formatter,
                "cannot save {}: the lock was deleted under this live owner; not writing",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ProjectSaveError {}

impl ProjectSaveError {
    /// The incident code a failed project write carries
    /// (`IN1b` §2.2 rule 8).
    ///
    /// Declared beside the save path, which owns the trigger; core owns the code
    /// and the policy class (IN1 §2.1 rule 2).
    #[must_use]
    pub const fn incident_code(&self) -> IncidentCode {
        match self {
            // The refusal variants are unreachable in practice: the save path
            // intercepts them before noting (belt and braces, as for
            // `NewerFormat`). A refusal must never open an incident — they
            // share the save-failure code only so the type stays total.
            // `SessionNotLoaded` is headless-only; the app never observes it.
            Self::Serialize(_)
            | Self::Write(_)
            | Self::SaveAsRequired { .. }
            | Self::PathOpenElsewhere { .. }
            | Self::SessionNotLoaded
            | Self::LockLost { .. } => IncidentCode::Rejection(RejectionIncident::ProjectSave),
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
    #[must_use]
    pub fn incident_observation(
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
/// # Errors
/// Returns the typed `lut_store_root_invalid` refusal.
pub fn derive_lut_store(project_path: Option<&Path>) -> Result<Option<LutStore>, String> {
    match project_path {
        None => Ok(None),
        Some(path) => LutStore::for_project(path)
            .map(Some)
            .map_err(|error| error.to_string()),
    }
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
pub struct ProjectFile {
    #[serde(default = "v1", skip_serializing_if = "is_v1")]
    pub format_version: u32,
    #[serde(flatten)]
    pub document: Document,
}

/// Whether the session that read `read_version` may save over its own file
/// (`IN2B` §4 rule 3): at most the maximum version this build supports.
///
/// Newer files open (rule 4) but never overwrite: the in-memory document has
/// already lost the newer writer's fields at parse, and overwriting would
/// launder that loss into the original. Save As stays enabled — the loss goes
/// into a new file the person chose, with the card explaining.
#[must_use]
pub fn can_overwrite_save(read_version: u32) -> bool {
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
pub fn project_newer_format_observation(
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
pub fn canonical_session_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The Linux `MAXSYMLINKS` (H5), applied to the LEAF link chain only
/// (J4): directory-component links are not counted, so a spelling past the
/// kernel's whole-lookup limit may still unify, but can never be opened.
const IDENTITY_LINK_HOPS: u32 = 40;

/// A path with no canonical project identity (H5): its symlink chain
/// still names a link after [`IDENTITY_LINK_HOPS`] hops (too long, or a
/// cycle), or a link in it cannot be read. Typed, never a fallback —
/// aliases must not derive distinct lock/journal identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIdentityError {
    pub path: PathBuf,
}

impl std::fmt::Display for ProjectIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} has no project identity: its symlink chain is longer than \
             {IDENTITY_LINK_HOPS} links, loops, or cannot be read",
            self.path.display()
        )
    }
}

impl std::error::Error for ProjectIdentityError {}

/// Resolve a symlink chain one `read_link` at a time (G8/H5): each
/// relative target resolves against its link's parent. Follows at most
/// [`IDENTITY_LINK_HOPS`] links, then checks the reached path — a non-link
/// terminal resolves (as the kernel would), a link still there refuses.
fn resolve_link_chain(path: &Path) -> Result<PathBuf, ProjectIdentityError> {
    let is_link =
        |path: &Path| fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    let refuse = || ProjectIdentityError {
        path: path.to_path_buf(),
    };
    let mut current = path.to_path_buf();
    for _ in 0..IDENTITY_LINK_HOPS {
        if !is_link(&current) {
            return Ok(current);
        }
        let target = fs::read_link(&current).map_err(|_| refuse())?;
        current = if target.is_absolute() {
            target
        } else {
            current.parent().unwrap_or(Path::new(".")).join(target)
        };
    }
    if is_link(&current) {
        Err(refuse())
    } else {
        Ok(current)
    }
}

/// One canonical identity (F4): full path if it exists, else canonical
/// parent plus name; relative resolves at the cwd. Raw is the last resort.
/// G8: a dangling symlink leaf resolves through its chain first, so
/// aliases share their target's identity (canonical parent of the target
/// plus the target's name when it is missing).
/// # Errors
/// [`ProjectIdentityError`] when the leaf chain exceeds
/// [`IDENTITY_LINK_HOPS`], loops, or cannot be read (H5).
pub fn canonical_project_identity(path: &Path) -> Result<PathBuf, ProjectIdentityError> {
    let resolved = resolve_link_chain(path)?;
    if let Ok(canonical) = fs::canonicalize(&resolved) {
        return Ok(canonical);
    }
    let parent = resolved
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(match (fs::canonicalize(parent), resolved.file_name()) {
        (Ok(dir), Some(name)) => dir.join(name),
        _ => resolved,
    })
}

/// MO2 R7: the lowest format version able to read `document` — the version
/// the writer stamps.
///
/// 2 iff the document uses adjustment or solid content, a non-`normal`
/// blend, or a transition outside the M20 three; else 1, so a file without
/// MO2 features keeps its v1 bytes. v2 is one union: CC8 S2 adds its
/// disjuncts here (N6 R-B), never a rival predicate.
#[must_use]
pub fn min_required_format_version(document: &Document) -> u32 {
    let uses_v2 = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .any(|clip| {
            matches!(
                clip.content,
                ClipContent::Adjustment | ClipContent::Solid(_)
            ) || clip.blend_mode != BlendMode::Normal
                || clip.transition_in.as_ref().is_some_and(|transition| {
                    !matches!(
                        transition.name.as_str(),
                        "crossfade" | "fade_from_black" | "fade_from_white"
                    )
                })
        });
    if uses_v2 { 2 } else { 1 }
}

/// Serialise a document inside the file envelope (`IN2B` §4 rules 1–2).
///
/// The writer stamps [`min_required_format_version`] — never the session's
/// read version: the bytes are this build's, whatever it opened (MO2 R7).
/// # Errors
/// Returns `ProjectSaveError::Serialize` when the document does not serialise.
pub fn serialize_project_document(document: &Document) -> Result<String, ProjectSaveError> {
    let file = ProjectFile {
        format_version: min_required_format_version(document),
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
/// # Errors
/// Returns `ProjectSaveError::Serialize` or `ProjectSaveError::Write`.
pub fn write_project_document(
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
/// # Errors
/// Returns the temp-write, permission, rename, or fallback-write IO error.
pub fn write_file_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
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
/// # Errors
/// Returns `ProjectSaveError::Write` when the atomic write fails.
pub fn write_project_bytes(
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

/// Load a project file: one read, one parse (`IN2B` §4 rule 2).
///
/// Returns the parsed document with the envelope version it read (missing → 1,
/// every legacy file) and the FNV-1a digest of the file bytes (§2 rule 9 —
/// the sidecar gate reuses it, no second read, no TOCTOU). The parse is
/// `from_slice::<ProjectFile>` then unwrap: the envelope parses legacy files
/// directly via the default, never probe-then-parse.
/// # Errors
/// Returns the read, parse, or validation failure as a string.
pub fn load_document(path: &Path) -> Result<(Document, u32, String), String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let file: ProjectFile = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    file.document
        .validate()
        .map_err(|error| error.to_string())?;
    let digest = digest_bytes(&bytes);
    Ok((file.document, file.format_version, digest))
}

#[cfg(test)]
mod tests {
    use kinewright_media::test_support::TempDirectory;

    use super::*;

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

    /// F4: the identity resolves an existing file fully, a missing target
    /// via its canonical parent, a relative spelling against the working
    /// dir — and falls back to the raw path when nothing resolves.
    #[test]
    fn canonical_identity_pins_existing_missing_and_unresolvable() {
        let identity = |path: &Path| canonical_project_identity(path).expect("an identity");
        let dir = TempDirectory::new("aw1-f4-identity");
        let real = dir.path("edit.kinewright");
        fs::write(&real, b"{}").expect("the project writes");
        assert_eq!(
            identity(&real),
            fs::canonicalize(&real).expect("the real path resolves")
        );
        let missing = dir.path("new.kinewright");
        assert_eq!(
            identity(&missing),
            fs::canonicalize(dir.root())
                .expect("the parent resolves")
                .join("new.kinewright")
        );
        let nowhere = Path::new("/no/such/kinewright-dir/edit.kinewright");
        assert_eq!(identity(nowhere), nowhere.to_path_buf());
        let relative = identity(Path::new("missing.kinewright"));
        assert!(
            relative.is_absolute(),
            "relative resolves against the working dir"
        );
        assert_eq!(
            relative.file_name().expect("a file name"),
            "missing.kinewright"
        );
    }

    /// G8: a dangling symlink leaf shares its target's identity.
    #[cfg(unix)]
    #[test]
    fn g8_dangling_leaf_unifies() {
        let dir = TempDirectory::new("aw1-g8-dangling");
        let target = dir.path("real").join("new.kinewright");
        let alias = dir.path("alias.kinewright");
        std::os::unix::fs::symlink(&target, &alias).expect("the dangling link plants");
        assert_eq!(
            canonical_project_identity(&alias),
            canonical_project_identity(&target),
            "a dangling alias shares its target's identity"
        );
    }

    /// H5: a dangling chain of N relative links (`link0 → link1 → … →
    /// missing`) unifies with its target up to the kernel's 40 hops — the
    /// 40th hop's non-link terminal resolves normally — and 41 refuses
    /// typed, as does a cycle: never a silent fallback identity.
    #[cfg(unix)]
    #[test]
    fn h5_link_chain_depth_unifies_to_40_and_refuses_past_it() {
        let dir = TempDirectory::new("aw1-h5-depth");
        let target = canonical_project_identity(&dir.path("missing.kinewright"))
            .expect("the target identity");
        for links in [0_usize, 1, 39, 40, 41] {
            let chain = dir.path(&format!("chain-{links}"));
            fs::create_dir(&chain).expect("the chain dir creates");
            for hop in 0..links {
                let next = if hop + 1 == links {
                    Path::new("..").join("missing.kinewright")
                } else {
                    PathBuf::from(format!("link{}", hop + 1))
                };
                std::os::unix::fs::symlink(next, chain.join(format!("link{hop}")))
                    .expect("a relative link plants");
            }
            let head = if links == 0 {
                dir.path("missing.kinewright")
            } else {
                chain.join("link0")
            };
            let got = canonical_project_identity(&head);
            if links <= 40 {
                assert_eq!(got.as_ref(), Ok(&target), "{links} links unify");
            } else {
                assert_eq!(got, Err(ProjectIdentityError { path: head }), "41 refuse");
            }
        }
        let loop_a = dir.path("loop-a");
        let loop_b = dir.path("loop-b");
        std::os::unix::fs::symlink(&loop_b, &loop_a).expect("loop a plants");
        std::os::unix::fs::symlink(&loop_a, &loop_b).expect("loop b plants");
        assert!(
            canonical_project_identity(&loop_a).is_err(),
            "a cycle refuses typed"
        );
        let refused = crate::lockfile::acquire_project_lock_with_policy(
            &dir.path("chain-41").join("link0"),
            crate::lockfile::LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &dir.path("recovery"),
            1,
            std::time::Duration::ZERO,
        );
        assert!(
            matches!(refused, Err(crate::lockfile::LockfileError::Identity(_))),
            "the lock refuses typed, creating nothing"
        );
    }

    /// R1: save bytes are byte-identical pre/post cutover over the corpus.
    /// Goldens captured on unmodified `6c2bdec` before the AW1 S1 move; the
    /// moved serialiser is byte-identical code, so these pin it against drift.
    #[test]
    fn r1_save_bytes_are_byte_identical_pre_post_cutover() {
        let default_json =
            serialize_project_document(&Document::default()).expect("the default serialises");
        assert_eq!(default_json.len(), 1017);
        assert_eq!(
            digest_bytes(default_json.as_bytes()),
            "5228b756413bcab5",
            "the default envelope keeps its pre-cutover digest"
        );
        let fixture: &[u8] =
            include_bytes!("../../kinewright-core/tests/fixtures/pre_m13_project.json");
        let file: ProjectFile = serde_json::from_slice(fixture).expect("the fixture parses");
        let envelope = serialize_project_document(&file.document).expect("the envelope serialises");
        assert_eq!(envelope.len(), 1891);
        assert_eq!(
            digest_bytes(envelope.as_bytes()),
            "7f899f0203ea23fb",
            "the fixture envelope keeps its pre-cutover digest"
        );
    }

    /// The M20-corpus envelope bytes on `13e3cb1`, before MO2 R7.
    const V1_RICH_LEN: usize = 2818;
    const V1_RICH_DIGEST: &str = "973547bf2726430b";

    fn mo2_v1_document() -> Document {
        use kinewright_core::{
            AssetId, MediaAsset, MediaKind, MediaSourceFingerprint, Operation, Rational, TimeCode,
            Track, TrackId, TrackKind, Transition,
        };
        let mut doc = Document {
            fps: Rational::new(30, 1).expect("30 fps"),
            ..Document::default()
        };
        let asset = MediaAsset {
            id: AssetId(1),
            path: PathBuf::from("a1.mp4"),
            name: "a1".to_owned(),
            duration: TimeCode(600),
            fps: Rational::new(30, 1).expect("30 fps"),
            kind: MediaKind::AudioVideo,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        };
        let track = Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        };
        Operation::AddTrack { track }
            .apply(&mut doc)
            .expect("the track adds");
        Operation::AddAsset { asset }
            .apply(&mut doc)
            .expect("the asset adds");
        for at in [0, 60, 120, 180] {
            Operation::AddClip {
                track: TrackId(1),
                asset: AssetId(1),
                at: TimeCode(at),
                source: TimeCode(0)..TimeCode(60),
            }
            .apply(&mut doc)
            .expect("the clip adds");
        }
        let ids: Vec<_> = doc.tracks[0].clips.iter().map(|clip| clip.id).collect();
        for (clip, name) in
            ids.into_iter()
                .skip(1)
                .zip(["crossfade", "fade_from_black", "fade_from_white"])
        {
            Operation::AddTransition {
                clip,
                transition: Transition {
                    name: name.to_owned(),
                    duration: TimeCode(10),
                },
            }
            .apply(&mut doc)
            .expect("an M20 transition adds");
        }
        doc
    }

    /// MO2 §13 gate 11 (R7): MO2-feature files stamp 2 and reopen as 2; the
    /// v1 corpus — every M20 transition included — stays 1 and writes the
    /// pre-MO2 bytes (digests captured on `13e3cb1`, before the predicate).
    #[test]
    fn v2_stamps_only_when_used() {
        use kinewright_core::{BlendMode, Operation, SolidColor, TimeCode, TrackId, Transition};
        let fixture: &[u8] =
            include_bytes!("../../kinewright-core/tests/fixtures/pre_m13_project.json");
        let legacy: ProjectFile = serde_json::from_slice(fixture).expect("the fixture parses");
        let v1 = mo2_v1_document();
        for doc in [&Document::default(), &legacy.document, &v1] {
            assert_eq!(min_required_format_version(doc), 1);
            let json = serialize_project_document(doc).expect("v1 serialises");
            assert!(!json.contains("format_version"), "v1 skips the key");
        }
        let v1_json = serialize_project_document(&v1).expect("v1 serialises");
        assert_eq!(
            (v1_json.len(), digest_bytes(v1_json.as_bytes()).as_str()),
            (V1_RICH_LEN, V1_RICH_DIGEST),
            "the M20 corpus writes its pre-MO2 bytes"
        );

        let first = v1.tracks[0].clips[0].id;
        let features = [
            Operation::AddAdjustmentClip {
                track: TrackId(1),
                timeline_start: TimeCode(240),
                duration: TimeCode(30),
                effects: Vec::new(),
            },
            Operation::AddSolidClip {
                track: TrackId(1),
                timeline_start: TimeCode(240),
                duration: TimeCode(30),
                color: SolidColor { r: 1, g: 2, b: 3 },
            },
            Operation::SetClipBlendMode {
                clip: first,
                blend_mode: BlendMode::Screen,
            },
            Operation::AddTransition {
                clip: first,
                transition: Transition {
                    name: "push_left".to_owned(),
                    duration: TimeCode(10),
                },
            },
        ];
        let dir = TempDirectory::new("mo2-gate11-v2");
        for (index, feature) in features.into_iter().enumerate() {
            let mut doc = v1.clone();
            feature.apply(&mut doc).expect("the MO2 feature applies");
            assert_eq!(min_required_format_version(&doc), 2, "{feature:?}");
            let path = dir.path(&format!("mo2-{index}.kinewright"));
            let report = write_project_document(&doc, &path, None).expect("v2 writes");
            let bytes = fs::read_to_string(&path).expect("v2 reads");
            assert!(
                bytes.starts_with("{\n  \"format_version\": 2,"),
                "v2 stamps first: {feature:?}"
            );
            let (reopened, version, digest) = load_document(&path).expect("v2 reopens");
            assert_eq!((reopened, version), (doc, 2));
            assert_eq!(digest, report.digest);
            assert!(can_overwrite_save(version), "this build overwrites v2");
        }
    }

    /// The four MO2 R7 features on `mo2_v1_document`, each with the clip
    /// that carries it: an adjustment, a solid, a non-`normal` blend and a
    /// geometric transition.
    fn mo2_feature_documents() -> Vec<(&'static str, Document, kinewright_core::ClipId)> {
        use kinewright_core::{BlendMode, Operation, SolidColor, TimeCode, TrackId, Transition};
        let v1 = mo2_v1_document();
        let first = v1.tracks[0].clips[0].id;
        let features = [
            (
                "adjustment",
                Operation::AddAdjustmentClip {
                    track: TrackId(1),
                    timeline_start: TimeCode(240),
                    duration: TimeCode(30),
                    effects: Vec::new(),
                },
            ),
            (
                "solid",
                Operation::AddSolidClip {
                    track: TrackId(1),
                    timeline_start: TimeCode(240),
                    duration: TimeCode(30),
                    color: SolidColor { r: 1, g: 2, b: 3 },
                },
            ),
            (
                "blend",
                Operation::SetClipBlendMode {
                    clip: first,
                    blend_mode: BlendMode::Screen,
                },
            ),
            (
                "transition",
                Operation::AddTransition {
                    clip: first,
                    transition: Transition {
                        name: "push_left".to_owned(),
                        duration: TimeCode(10),
                    },
                },
            ),
        ];
        features
            .into_iter()
            .map(|(name, feature)| {
                let mut doc = v1.clone();
                feature.apply(&mut doc).expect("the MO2 feature applies");
                let carrier = doc.tracks[0]
                    .clips
                    .iter()
                    .find(|clip| {
                        clip.content != kinewright_core::ClipContent::Media
                            || !clip.blend_mode.is_normal()
                            || clip
                                .transition_in
                                .as_ref()
                                .is_some_and(|transition| transition.name == "push_left")
                    })
                    .expect("one clip carries the feature")
                    .id;
                (name, doc, carrier)
            })
            .collect()
    }

    /// MO2 R7 (review 2 S1): the predicate scans every clip, active or not —
    /// a feature on a disabled clip, a key-disabled clip or a muted track
    /// still stamps 2 through the real writer and reopens as 2, since the
    /// bytes carry the feature whether or not it renders today.
    #[test]
    fn mo2_inactive_features_still_stamp_v2() {
        use kinewright_core::{KeyframeInterpolation, Operation, TimeCode, TrackId};
        let dir = TempDirectory::new("mo2-r7-inactive");
        for (name, doc, carrier) in mo2_feature_documents() {
            let hold_off = kinewright_core::AutomationCurve {
                keyframes: vec![kinewright_core::Keyframe {
                    at: TimeCode(0),
                    value: 0,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                }],
            };
            let states = [
                (
                    "disabled",
                    Operation::SetClipEnabled {
                        clip: carrier,
                        enabled: false,
                    },
                ),
                (
                    "key-disabled",
                    Operation::SetClipEnabledCurve {
                        clip: carrier,
                        curve: Some(hold_off),
                    },
                ),
                (
                    "muted",
                    Operation::SetTrackMix {
                        track: TrackId(1),
                        gain_tenth_db: 0,
                        pan_percent: 0,
                        mute: true,
                        solo: false,
                    },
                ),
            ];
            for (state, operation) in states {
                let mut inactive = doc.clone();
                operation.apply(&mut inactive).expect("the state applies");
                match state {
                    "disabled" => assert!(!inactive.clip(carrier).expect("carrier").enabled),
                    "key-disabled" => assert!(
                        !inactive
                            .clip(carrier)
                            .expect("carrier")
                            .is_enabled_at(TimeCode(0))
                    ),
                    _ => assert!(inactive.track_mix(TrackId(1)).mute),
                }
                assert_eq!(min_required_format_version(&inactive), 2, "{name}/{state}");
                let path = dir.path(&format!("{name}-{state}.kinewright"));
                let report = write_project_document(&inactive, &path, None).expect("v2 writes");
                let bytes = fs::read_to_string(&path).expect("v2 reads");
                assert!(
                    bytes.starts_with("{\n  \"format_version\": 2,"),
                    "{name}/{state} stamps 2"
                );
                let (reopened, version, digest) = load_document(&path).expect("v2 reopens");
                assert_eq!((reopened, version), (inactive, 2), "{name}/{state}");
                assert_eq!(digest, report.digest);
            }

            // Review 2 N1: a feature on a clip past the document duration is
            // an invalid document (never loaded), but the pure predicate
            // still counts it — the scan is unconditional.
            let mut outside = doc.clone();
            outside.duration = outside.clip(carrier).expect("carrier").timeline_start;
            assert_eq!(min_required_format_version(&outside), 2, "{name}/outside");
        }
    }

    /// MO2 R7 old-reader behaviour, simulated with a v3 file against this
    /// reader: unknown content fails at parse; an unknown transition parses
    /// then fails validation; an otherwise parseable newer file opens
    /// advisory with its version, and overwrite-save is refused.
    #[test]
    fn mo2_old_readers_distinguish_parse_validation_and_advisory() {
        use kinewright_core::{OpError, Operation, TimeCode, TrackId, Transition};
        let mut doc = mo2_v1_document();
        let first = doc.tracks[0].clips[0].id;
        for op in [
            Operation::AddAdjustmentClip {
                track: TrackId(1),
                timeline_start: TimeCode(240),
                duration: TimeCode(30),
                effects: Vec::new(),
            },
            Operation::AddTransition {
                clip: first,
                transition: Transition {
                    name: "push_left".to_owned(),
                    duration: TimeCode(10),
                },
            },
        ] {
            op.apply(&mut doc).expect("the MO2 feature applies");
        }
        let v2 = serialize_project_document(&doc).expect("v2 serialises");
        let newer = v2.replace("\"format_version\": 2", "\"format_version\": 3");
        let dir = TempDirectory::new("mo2-old-readers");
        let load = |name: &str, bytes: &str| {
            let path = dir.path(name);
            fs::write(&path, bytes).expect("the file writes");
            load_document(&path)
        };

        let error = load(
            "content.kinewright",
            &newer.replace("\"adjustment\"", "\"hologram\""),
        )
        .expect_err("unknown content fails");
        assert!(error.contains("unknown variant `hologram`"), "{error}");

        let error = load(
            "transition.kinewright",
            &newer.replace("push_left", "iris_open"),
        )
        .expect_err("an unknown transition fails");
        assert_eq!(
            error,
            OpError::UnknownTransition("iris_open".to_owned()).to_string()
        );

        let (opened, version, _) =
            load("advisory.kinewright", &newer).expect("a parseable newer file opens");
        assert!(opened == doc, "the newer file keeps its content");
        assert_eq!(version, 3, "the newer file reports its version");
        assert!(!can_overwrite_save(version), "overwrite-save refuses");
        assert!(can_overwrite_save(2) && can_overwrite_save(1));
    }

    /// MO2 R7 blend-only old reader (review 2 S5, lead ruling N10), simulated
    /// against this reader: the real f241aa5 reader opens a blend-only v2
    /// file, silently drops the `blend_mode` field it does not know, and
    /// refuses overwrite only because the retained file version is newer.
    /// Here a v3 file carries a clip field this build does not know: it
    /// opens advisory with version 3, the field is gone from the document
    /// and from any re-save (which would even stamp a lower version), and
    /// overwrite-save is refused, so the loss never launders into the file.
    /// The real two-build check is `scripts/mo2-old-reader-check.sh`.
    #[test]
    fn mo2_old_reader_drops_an_unknown_blend_and_refuses_overwrite() {
        let (_, blended, carrier) = mo2_feature_documents()
            .into_iter()
            .find(|(name, ..)| *name == "blend")
            .expect("the blend feature");
        let v2 = serialize_project_document(&blended).expect("v2 serialises");
        assert!(v2.starts_with("{\n  \"format_version\": 2,"));
        assert_eq!(v2.matches("\"blend_mode\": \"screen\"").count(), 1);
        let newer = v2
            .replace("\"format_version\": 2", "\"format_version\": 3")
            .replace("\"blend_mode\": \"screen\"", "\"future_blend\": \"screen\"");
        let dir = TempDirectory::new("mo2-old-reader-blend");
        let path = dir.path("blend.kinewright");
        fs::write(&path, &newer).expect("the file writes");

        let (opened, version, _) = load_document(&path).expect("a parseable newer file opens");
        assert_eq!(version, 3, "the retained version is the file's");
        let mut dropped = blended.clone();
        dropped
            .tracks
            .iter_mut()
            .flat_map(|track| &mut track.clips)
            .find(|clip| clip.id == carrier)
            .expect("carrier")
            .blend_mode = kinewright_core::BlendMode::Normal;
        assert_eq!(opened, dropped, "the unknown blend is dropped at parse");
        let resaved = serialize_project_document(&opened).expect("re-serialises");
        assert!(!resaved.contains("future_blend") && !resaved.contains("blend_mode"));
        assert_eq!(
            min_required_format_version(&opened),
            1,
            "what survives the drop would stamp an older version"
        );
        assert!(
            !can_overwrite_save(version),
            "the retained newer version refuses overwrite"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("reads"),
            newer,
            "opening leaves the source bytes untouched"
        );
    }

    /// The head half of `scripts/mo2-old-reader-check.sh` (review 2 S5, lead
    /// ruling N10): this build's shared writer emits the gate-11 corpus into
    /// `MO2_OLD_READER_CORPUS` for the frozen f241aa5 reader to consume — the
    /// three v1 controls, the four MO2 features, and `blend-dropped`, the
    /// blend document with only its blend removed (the exact bytes an old
    /// re-save must produce). A manual pre-land check, never a CI job.
    #[test]
    #[ignore = "manual pre-land check: scripts/mo2-old-reader-check.sh"]
    fn mo2_old_reader_corpus() {
        let out = PathBuf::from(
            std::env::var_os("MO2_OLD_READER_CORPUS")
                .expect("MO2_OLD_READER_CORPUS names the corpus directory"),
        );
        fs::create_dir_all(&out).expect("the corpus directory exists");
        let fixture: ProjectFile = serde_json::from_slice(include_bytes!(
            "../../kinewright-core/tests/fixtures/pre_m13_project.json"
        ))
        .expect("the fixture parses");
        let mut files = vec![
            ("v1-default", Document::default(), 1),
            ("v1-fixture", fixture.document, 1),
            ("v1-m20", mo2_v1_document(), 1),
        ];
        for (name, doc, carrier) in mo2_feature_documents() {
            if name == "blend" {
                let mut dropped = doc.clone();
                dropped
                    .tracks
                    .iter_mut()
                    .flat_map(|track| &mut track.clips)
                    .find(|clip| clip.id == carrier)
                    .expect("carrier")
                    .blend_mode = kinewright_core::BlendMode::Normal;
                files.push(("blend-dropped", dropped, 1));
            }
            files.push((name, doc, 2));
        }
        for (name, doc, version) in files {
            doc.validate().expect("the corpus document is valid");
            assert_eq!(min_required_format_version(&doc), version, "{name}");
            let path = out.join(format!("{name}.kinewright"));
            write_project_document(&doc, &path, None).expect("the shared writer writes");
            let (reopened, read_version, _) = load_document(&path).expect("head reopens");
            assert_eq!((reopened, read_version), (doc, version), "{name}");
            println!("head wrote {name}.kinewright as v{version}");
        }
    }
}
