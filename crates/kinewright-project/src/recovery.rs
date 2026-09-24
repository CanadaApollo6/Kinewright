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
#[must_use]
pub fn journal_file_name(project_path: &Path) -> String {
    let stem: String = project_path
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
    let hash = fnv1a_64(project_path.to_string_lossy().as_bytes());
    format!("{stem}-{hash:016x}.journal")
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

/// The pending journal for a project: the takeover check (AW1 §5/S7).
#[must_use]
pub fn pending_journal_for_project(recovery_dir: &Path, project_path: &Path) -> Option<PathBuf> {
    let candidate = recovery_dir.join(journal_file_name(project_path));
    candidate.is_file().then_some(candidate)
}

/// Retire a project's journal after a save: the headless checkpoint.
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

    #[test]
    fn retire_removes_the_base_journal_and_tolerates_absence() {
        let dir = TempDirectory::new("aw1-retire-journal");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        let project = dir.path("edit.kinewright");
        assert_eq!(
            pending_journal_for_project(&recovery, &project),
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
            pending_journal_for_project(&recovery, &project),
            Some(journal.clone()),
            "the stale journal pends"
        );
        assert!(
            retire_journal_for_project(&recovery, &project).expect("the retire lands"),
            "a present journal retires to true"
        );
        assert!(!journal.exists(), "the stale journal is gone");
        assert_eq!(
            pending_journal_for_project(&recovery, &project),
            None,
            "nothing pends after the retire"
        );
    }
}
