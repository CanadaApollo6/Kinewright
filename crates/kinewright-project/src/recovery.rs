//! Recovery-journal naming, allocation, and retire (AW1 §2, S7), moved
//! from the app in AW1 S1. Replay, the recorder, and the dialog stay in the
//! app — headless never replays journals.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use kinewright_core::{
    IncidentCode, IncidentObservation, IncidentSubject, LabelIncident, TimelineRevision,
};
use serde::Deserialize;

use crate::project::canonical_project_identity;

/// The recovery-journal magic, shared with the app's writer/inspector.
pub const JOURNAL_MAGIC: &[u8] = b"KINEWRIGHT-JOURNAL 1\n";

pub fn default_recovery_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("Kinewright")
        .join("recovery")
}

/// FNV-1a, chosen over the standard hasher because journal names must stay
/// stable across builds and Rust versions to find their project again.
///
/// Shared with the sidecar pairing digest (`IN2B` §0.4 d2), which needs the
/// same stability for the same reason: one implementation, two callers.
#[must_use]
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `MyVideo-1a2b3c4d5e6f7081.journal` - readable stem, collision-proof hash.
fn journal_name_for(path: &Path) -> String {
    let stem: String = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(24)
        .collect();
    let stem = if stem.is_empty() {
        "project".to_owned()
    } else {
        stem
    };
    let hash = fnv1a_64(path.to_string_lossy().as_bytes());
    format!("{stem}-{hash:016x}.journal")
}

/// `MyVideo-1a2b3c4d5e6f7081.journal` - readable stem, collision-proof hash.
/// Both halves derive from the canonical identity (F4): one file, one name.
/// A path with no identity (H5) names by its raw spelling — it can never
/// be locked or scanned (both refuse typed), so the name owns nothing.
#[must_use]
pub fn journal_file_name(project_path: &Path) -> String {
    let identity = canonical_project_identity(project_path);
    journal_name_for(identity.as_deref().unwrap_or(project_path))
}

/// The pre-F4 journal name (G5): main hashed the RAW path spelling, so a
/// legacy journal is found by this name when the project is queried with
/// the spelling that created it.
fn legacy_journal_file_name(project_path: &Path) -> String {
    journal_name_for(project_path)
}

/// The ordinary spelling of a verbatim Windows path, when it has one:
/// `\\?\C:\…` → `C:\…`, `\\?\UNC\s\s` → `\\s\s`. Pre-F4 journals hashed the
/// ordinary spelling while `canonicalize` returns the verbatim one (G5).
/// Pure, so the unit test runs on every OS.
#[cfg(any(windows, test))]
fn ordinary_spelling(verbatim: &str) -> Option<String> {
    if let Some(rest) = verbatim.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    verbatim.strip_prefix(r"\\?\").map(str::to_owned)
}

/// The journal path for a project, never colliding with a reserved (pending)
/// file: undecided crash data must not be truncated by a new session. An
/// unsaved project keeps its current `unsaved-N` file across baselines.
#[must_use]
pub fn allocate_journal_path(
    directory: &Path,
    project_path: Option<&Path>,
    current: &Path,
    reserved: &[&Path],
) -> PathBuf {
    let is_reserved = |candidate: &Path| reserved.contains(&candidate);
    if let Some(project_path) = project_path {
        let base = journal_file_name(project_path);
        let first = directory.join(&base);
        if !is_reserved(&first) && (first == current || !first.exists()) {
            return first;
        }
        let stem = base.trim_end_matches(".journal");
        for suffix in 2.. {
            let candidate = directory.join(format!("{stem}-{suffix}.journal"));
            if !is_reserved(&candidate) && (candidate == current || !candidate.exists()) {
                return candidate;
            }
        }
        unreachable!("an unreserved journal suffix always exists");
    }
    let keeps_current = current
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("unsaved-"))
        && !is_reserved(current);
    if keeps_current {
        return current.to_path_buf();
    }
    for number in 1.. {
        let candidate = directory.join(format!("unsaved-{number}.journal"));
        if !is_reserved(&candidate) && !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unreserved unsaved journal name always exists");
}

/// The status line a finished crash-recovery restore writes, and the
/// observation a failed one opens (`IN1b` §5.7, Appendix B row 28).
///
/// The success arm returns its string as it always did. The `Err` arm returns
/// **no** string: it used to compose *"Could not restore unsaved work: …"*
/// straight into `self.status` with no log write on the path at all, which is
/// the third of the three sinks that reached a person without touching
/// `ErrorLog`. The caller queues the observation and `note_incident` writes
/// the status line. The observation is boxed because it is much larger than
/// the success string and `clippy::result_large_err` is part of the house
/// `-D warnings` gate.
/// # Errors
/// Returns the boxed failure observation when the restore failed.
pub fn restore_status(result: Result<(), String>) -> Result<String, Box<IncidentObservation>> {
    match result {
        Ok(()) => Ok("Recovered unsaved work".to_owned()),
        Err(error) => Err(Box::new(IncidentObservation::plain(
            IncidentCode::Label(LabelIncident::Project),
            IncidentSubject::Project,
            format!("Could not restore unsaved work: {error}"),
            TimelineRevision::default(),
        ))),
    }
}

/// Whether a name is the identity's base or a `-N` suffix.
fn journal_name_matches_for(identity_base: &str, file_name: &str) -> bool {
    if file_name == identity_base {
        return true;
    }
    let stem = identity_base.trim_end_matches(".journal");
    file_name
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.strip_suffix(".journal"))
        .is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// The takeover check reads one header field: the owning project.
#[derive(Deserialize)]
struct JournalIdentityHeader {
    #[serde(default)]
    project_path: Option<PathBuf>,
}

/// The most a header scan reads past the magic (G6): headers embed the
/// initial document but stay small, and name-matched journals refuse
/// without any header read — so a non-name-matched journal whose header
/// exceeds this is ignored as unverifiable, never blocking a project, and
/// the limit can never weaken a name match into a pass.
const JOURNAL_HEADER_LIMIT: u64 = 1024 * 1024;

/// Whether a journal's header names the project (F5's alias arm). Only an
/// absolute header path claims, by canonical identity (G5): a relative
/// header is ambiguous — never rebound to the current cwd — and torn,
/// missing, or over-limit headers never match, so a non-name-matched
/// journal with one is ignored, never blocking an unrelated project.
/// Name-matched journals refuse without consulting the header at all.
/// The scan streams (`BufReader`, one magic read, one parsed header value)
/// so unrelated journals cost kilobytes, not their full bodies (G6/RS3);
/// only a read error on a regular file fails closed. Vanished reads as gone.
fn journal_header_names(journal: &Path, identity: &Path) -> Result<bool, io::Error> {
    use std::io::{BufReader, Read as _};
    let file = match fs::File::open(journal) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut file = BufReader::new(file);
    let mut magic = [0u8; JOURNAL_MAGIC.len()];
    if file.read_exact(&mut magic).is_err() || magic.as_slice() != JOURNAL_MAGIC {
        return Ok(false);
    }
    let mut values = serde_json::Deserializer::from_reader(file.take(JOURNAL_HEADER_LIMIT))
        .into_iter::<JournalIdentityHeader>();
    let Some(header) = values.next() else {
        return Ok(false);
    };
    let Ok(header) = header else {
        return Ok(false);
    };
    let Some(project) = header.project_path else {
        return Ok(false);
    };
    if !project.is_absolute() {
        return Ok(false);
    }
    Ok(canonical_project_identity(&project).is_ok_and(|named| named == identity))
}

/// Pending journal (AW1 §5/S7, F5/G5/G6): canonical and legacy bases,
/// every `-N` allocator suffix of each, and header-matched aliases — first
/// in name order. Name matches refuse first, whatever the entry type (H4);
/// only regular non-matched files header-scan (anything else is skipped
/// unopened).
/// Missing dir means none pending.
/// # Errors
/// Returns the lookup IO error (fail-closed).
pub fn pending_journal_for_project(
    recovery_dir: &Path,
    project_path: &Path,
) -> Result<Option<PathBuf>, io::Error> {
    let identity = canonical_project_identity(project_path).map_err(io::Error::other)?;
    let base = journal_file_name(project_path);
    let legacy = legacy_journal_file_name(project_path);
    // Windows only: the ordinary spelling main hashed, recovered from a
    // verbatim identity (unit-pinned on every OS via `ordinary_spelling`).
    #[cfg(windows)]
    let ordinary_base = ordinary_spelling(&identity.to_string_lossy())
        .map(|ordinary| legacy_journal_file_name(Path::new(&ordinary)));
    #[cfg(not(windows))]
    let ordinary_base: Option<String> = None;
    let entries = match fs::read_dir(recovery_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut pending = Vec::new();
    for entry in entries {
        let entry = entry?;
        let journal = entry.path();
        if journal
            .extension()
            .is_none_or(|extension| extension != "journal")
        {
            continue;
        }
        let named = journal
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                journal_name_matches_for(&base, name)
                    || journal_name_matches_for(&legacy, name)
                    || ordinary_base
                        .as_deref()
                        .is_some_and(|third| journal_name_matches_for(third, name))
            });
        // H4: a name match refuses whatever the entry is (symlink, FIFO,
        // dir) — checked first, without opening it.
        if named {
            pending.push(journal);
            continue;
        }
        // G6/N1: only regular files header-scan — a non-matched directory,
        // symlink, FIFO or other entry is skipped before any open (a FIFO
        // would block), never blocking an unrelated project.
        // Indeterminate metadata fails closed.
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if kind.is_file() && journal_header_names(&journal, &identity)? {
            pending.push(journal);
        }
    }
    Ok(pending.into_iter().min())
}

/// Retire a base journal you own (F5): only data your session replayed
/// and superseded. Headless never calls this.
/// # Errors
/// Any removal IO error other than absence.
pub fn retire_journal_for_project(recovery_dir: &Path, project_path: &Path) -> io::Result<bool> {
    let candidate = recovery_dir.join(journal_file_name(project_path));
    match fs::remove_file(&candidate) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use kinewright_media::test_support::TempDirectory;

    use super::*;

    /// G5/RB3: a legacy raw-path-hash journal refuses by NAME even with a
    /// torn header — and so does its allocator `-N` suffix.
    #[test]
    fn g5_legacy_name_torn_header_refuses() {
        let dir = TempDirectory::new("aw1-g5-legacy-name");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        // The RB3 repro's spelling: main hashed the raw path, `./` included.
        let crooked = dir.root().join(".").join("source.kinewright");
        let legacy = recovery.join(legacy_journal_file_name(&crooked));
        assert_ne!(
            legacy.file_name().expect("a file name").to_string_lossy(),
            journal_file_name(&crooked),
            "the legacy name differs from the canonical one"
        );
        fs::write(&legacy, b"KINEWRIGHT-JOURNAL 1\n{\"project_path\":")
            .expect("the torn legacy journal writes");
        assert_eq!(
            pending_journal_for_project(&recovery, &crooked).expect("the lookup lands"),
            Some(legacy.clone()),
            "a legacy-name torn header refuses"
        );
        let suffixed = recovery.join(
            legacy
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .trim_end_matches(".journal")
                .to_owned()
                + "-2.journal",
        );
        fs::write(&suffixed, b"crash").expect("the suffixed legacy journal writes");
        fs::remove_file(&legacy).expect("only the suffix pends");
        assert_eq!(
            pending_journal_for_project(&recovery, &crooked).expect("the lookup lands"),
            Some(suffixed),
            "a legacy allocator suffix refuses"
        );
    }

    /// G5/RB3: a legacy RELATIVE header is never rebound to the current
    /// cwd — it claims nothing, so an unrelated same-named project is not
    /// blocked; the original spelling is still found via the legacy NAME.
    #[test]
    fn g5_relative_header_never_claims() {
        let dir = TempDirectory::new("aw1-g5-relative-header");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        let journal = recovery.join(legacy_journal_file_name(Path::new("edit.kinewright")));
        let document = serde_json::to_string(&kinewright_core::Document::default())
            .expect("the document serialises");
        let header = format!(
            "KINEWRIGHT-JOURNAL 1\n{{\"format_version\":1,\"writer_format_version\":1,\
             \"project_path\":\"edit.kinewright\",\"initial_document\":{document}}}\n"
        );
        fs::write(&journal, header).expect("the legacy journal writes");
        // The same-named path under the CURRENT cwd: resolving the header
        // against the cwd would falsely block it. (No chdir — the cwd is
        // process-global — the test constructs the collision instead.)
        let same_named = std::env::current_dir()
            .expect("the cwd reads")
            .join("edit.kinewright");
        assert_eq!(
            pending_journal_for_project(&recovery, &same_named).expect("the lookup lands"),
            None,
            "a relative header never blocks an unrelated same-named project"
        );
        assert_eq!(
            pending_journal_for_project(&recovery, Path::new("edit.kinewright"))
                .expect("the lookup lands"),
            Some(journal),
            "the original spelling is found via the legacy name"
        );
    }

    /// G6: the streamed scan claims an alias header past a big body — the
    /// tail is never materialised to find it.
    #[test]
    fn g6_alias_header_with_big_body_claims() {
        let dir = TempDirectory::new("aw1-g6-big-body");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let journal = recovery.join("elsewhere-0123456789abcdef.journal");
        let document = serde_json::to_string(&kinewright_core::Document::default())
            .expect("the document serialises");
        let header = format!(
            "KINEWRIGHT-JOURNAL 1\n{{\"format_version\":1,\"writer_format_version\":1,\
             \"project_path\":{},\"initial_document\":{document}}}\n",
            serde_json::to_string(&project).expect("the path serialises")
        );
        let mut body = header.into_bytes();
        body.extend(vec![b'x'; 8 * 1024 * 1024]);
        fs::write(&journal, body).expect("the big journal writes");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            Some(journal),
            "an alias header past a big body claims"
        );
    }

    /// G6: a header past the byte limit is unverifiable — ignored when
    /// unmatched, and never weakening a name match.
    #[test]
    fn g6_over_limit_header_ignores_unmatched_but_not_a_match() {
        let dir = TempDirectory::new("aw1-g6-over-limit");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let big = "x".repeat(2 * 1024 * 1024);
        let header = format!(
            "KINEWRIGHT-JOURNAL 1\n{{\"format_version\":1,\"writer_format_version\":1,\
             \"project_path\":{},\"initial_document\":{}}}\n",
            serde_json::to_string(&project).expect("the path serialises"),
            serde_json::to_string(&big).expect("the body serialises")
        );
        assert!(
            header.len() as u64 > JOURNAL_HEADER_LIMIT,
            "the fixture exceeds the limit"
        );
        let alias = recovery.join("elsewhere-0123456789abcdef.journal");
        fs::write(&alias, &header).expect("the alias journal writes");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            None,
            "an over-limit header is ignored when unmatched"
        );
        let matched = recovery.join(journal_file_name(&project));
        fs::write(&matched, &header).expect("the matched journal writes");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            Some(matched),
            "the limit never weakens a name match"
        );
    }

    /// G5: the verbatim→ordinary spelling map is unit-pinned on every OS.
    #[test]
    fn g5_ordinary_spelling_maps_verbatim_paths() {
        assert_eq!(
            ordinary_spelling(r"\\?\C:\videos\edit.kinewright").as_deref(),
            Some(r"C:\videos\edit.kinewright")
        );
        assert_eq!(
            ordinary_spelling(r"\\?\UNC\host\share\edit.kinewright").as_deref(),
            Some(r"\\host\share\edit.kinewright")
        );
        assert_eq!(ordinary_spelling(r"C:\videos\edit.kinewright"), None);
        assert_eq!(ordinary_spelling("/home/u/edit.kinewright"), None);
    }

    #[test]
    fn retire_removes_the_base_journal_and_tolerates_absence() {
        let dir = TempDirectory::new("aw1-retire-journal");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        let project = dir.path("edit.kinewright");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            None,
            "no journal pends"
        );
        assert!(
            !retire_journal_for_project(&recovery, &project).expect("absence retires"),
            "absent retires to false"
        );
        let journal = recovery.join(journal_file_name(&project));
        fs::write(&journal, b"stale crash bytes").expect("the stale journal writes");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            Some(journal.clone()),
            "the stale journal pends"
        );
        assert!(
            retire_journal_for_project(&recovery, &project).expect("the retire lands"),
            "a present journal retires to true"
        );
        assert!(!journal.exists(), "the stale journal is gone");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            None,
            "nothing pends after the retire"
        );
    }
}
