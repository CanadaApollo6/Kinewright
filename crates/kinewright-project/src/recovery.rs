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

/// Open for reading without ever blocking (race N-c): Unix opens
/// `O_NONBLOCK` (a FIFO opens at once; regular files ignore the flag), and
/// the fd itself must be a regular file (`fstat`) — `None` otherwise.
/// # Errors
/// The open or `fstat` IO error.
pub(crate) fn open_regular(path: &Path) -> io::Result<Option<fs::File>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    let file = options.open(path)?;
    Ok(file.metadata()?.is_file().then_some(file))
}

/// The generous ceiling on one header value (H3). The parse streams, so
/// memory stays O(path) whatever the header size; this only bounds the
/// time one scan can take. A header reaching it is refused typed
/// (`FileTooLarge`, fail closed) — never ignored.
const JOURNAL_HEADER_CEILING: u64 = 1 << 30;

/// Whether a journal's header names the project (F5's alias arm). Only an
/// absolute header path claims, by canonical identity (G5): a relative
/// header is ambiguous — never rebound to the current cwd — and torn,
/// missing, or uncommitted headers never match, so a non-name-matched
/// journal with one is ignored, never blocking an unrelated project.
/// Name-matched journals refuse without consulting the header at all.
/// Vanished reads as gone.
fn journal_header_names(journal: &Path, identity: &Path) -> Result<bool, io::Error> {
    // N-c: an entry swapped to a FIFO (or anything non-regular) since the
    // type check is skipped like any other non-regular entry, never opened
    // blocking.
    #[cfg(any(test, feature = "test-util"))]
    crate::test_hook("journal_before_open");
    let file = match open_regular(journal) {
        Ok(Some(file)) => file,
        Ok(None) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    header_names_from(file, identity, JOURNAL_HEADER_CEILING)
}

/// The streaming header parse (H3), over any reader (the IO tests inject
/// one): one magic read, then a single JSON value parsed in place —
/// `project_path` is captured and every other value (the embedded
/// `initial_document`) skipped as `IgnoredAny`, never buffered — then the
/// byte right after the value must be `\n`, the writer's commit mark
/// (anything else, or EOF, is uncommitted: no claim). Torn or malformed
/// data is merely unmatched; any other IO error propagates (fail closed).
fn header_names_from(
    reader: impl io::Read,
    identity: &Path,
    ceiling: u64,
) -> Result<bool, io::Error> {
    use std::io::{BufReader, Read as _};
    let mut reader = BufReader::new(reader);
    let mut magic = [0u8; JOURNAL_MAGIC.len()];
    match reader.read_exact(&mut magic) {
        Ok(()) if magic.as_slice() == JOURNAL_MAGIC => {}
        Err(error) if error.kind() != io::ErrorKind::UnexpectedEof => return Err(error),
        _ => return Ok(false),
    }
    let mut body = reader.take(ceiling);
    let parsed =
        JournalIdentityHeader::deserialize(&mut serde_json::Deserializer::from_reader(&mut body));
    let mut next = [0u8; 1];
    let committed = match parsed {
        Err(error) if error.is_io() => return Err(error.into()),
        Err(_) => None,
        Ok(header) => match body.read_exact(&mut next) {
            Ok(()) => (next == *b"\n").then_some(header),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => None,
            Err(error) => return Err(error),
        },
    };
    if body.limit() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("a recovery journal header reaches the {ceiling}-byte ceiling"),
        ));
    }
    let Some(JournalIdentityHeader {
        project_path: Some(project),
    }) = committed
    else {
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
    pending_journal_for_identity(recovery_dir, project_path, &identity)
}

/// [`pending_journal_for_project`] for an identity the caller resolved
/// once (J2: the acquire's single resolution); `project_path` only names
/// the legacy raw-spelling journal.
pub(crate) fn pending_journal_for_identity(
    recovery_dir: &Path,
    project_path: &Path,
    identity: &Path,
) -> Result<Option<PathBuf>, io::Error> {
    let base = journal_name_for(identity);
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
        if kind.is_file() && journal_header_names(&journal, identity)? {
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

    /// H3 (race SW2): a big alias header is parsed in full and IDENTIFIED
    /// — no 1 MiB cut-off ignores big projects — while a header reaching
    /// the ceiling refuses typed (fail closed) and a name match still
    /// refuses without any header read.
    #[test]
    fn h3_big_header_identifies_and_the_ceiling_refuses() {
        let dir = TempDirectory::new("aw1-h3-big-header");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let big = "x".repeat(2 * 1024 * 1024);
        let header = format!(
            "KINEWRIGHT-JOURNAL 1\n{{\"format_version\":1,\"writer_format_version\":1,\
             \"initial_document\":{},\"project_path\":{}}}\n",
            serde_json::to_string(&big).expect("the body serialises"),
            serde_json::to_string(&project).expect("the path serialises")
        );
        let alias = recovery.join("elsewhere-0123456789abcdef.journal");
        fs::write(&alias, &header).expect("the alias journal writes");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            Some(alias.clone()),
            "a 2 MiB alias header identifies its project"
        );
        let identity = canonical_project_identity(&project).expect("the identity");
        let refused = header_names_from(header.as_bytes(), &identity, 1024)
            .expect_err("a header reaching the ceiling refuses");
        assert_eq!(refused.kind(), io::ErrorKind::FileTooLarge);
        fs::remove_file(&alias).expect("the alias goes");
        let matched = recovery.join(journal_file_name(&project));
        fs::write(&matched, b"KINEWRIGHT-JOURNAL 1\n{ torn").expect("the matched journal writes");
        assert_eq!(
            pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
            Some(matched),
            "a name match refuses without a header read"
        );
    }

    /// A reader that fails (not EOF) once its bytes run out: an in-process
    /// IO seam, no interposition (R2 B1's probe).
    struct FailingReader(std::io::Cursor<Vec<u8>>);

    impl io::Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match io::Read::read(&mut self.0, buf)? {
                0 => Err(io::Error::other("injected reader failure")),
                read => Ok(read),
            }
        }
    }

    /// H3 (R1 S1 / R2 B1): an IO error during the magic read propagates —
    /// never "no journal".
    #[test]
    fn rr2_g6_magic_io_fails_closed() {
        let failing = FailingReader(std::io::Cursor::new(b"KINEWRIGHT".to_vec()));
        assert!(
            header_names_from(failing, Path::new("/p"), JOURNAL_HEADER_CEILING).is_err(),
            "magic read IO must propagate"
        );
    }

    /// H3: an IO error inside the header value propagates too.
    #[test]
    fn rr2_g6_json_io_fails_closed() {
        let mut bytes = JOURNAL_MAGIC.to_vec();
        bytes.extend_from_slice(b"{\"project_path\":");
        let failing = FailingReader(std::io::Cursor::new(bytes));
        assert!(
            header_names_from(failing, Path::new("/p"), JOURNAL_HEADER_CEILING).is_err(),
            "JSON read IO must propagate"
        );
    }

    /// H3 (R1 S2 / R2 S1): only a committed header line claims — the value
    /// J5 (Astra R2 S2): the production journal entry point opens through
    /// `open_regular` — a directory at the journal name is rejected as a
    /// non-regular fd, never read (a plain `File::open` reads it as
    /// `IsADirectory`).
    #[cfg(unix)]
    #[test]
    fn rr3_journal_entry_rejects_nonregular_fd() {
        let dir = TempDirectory::new("rr3-journal-entry");
        let result = journal_header_names(dir.root(), Path::new("/p"));
        assert!(
            matches!(result, Ok(false)),
            "the entry point rejects the non-regular fd before reading: {result:?}"
        );
    }

    /// J5 (race N-c, journal side): a FIFO at the journal name returns
    /// promptly through the entry point — the open never blocks.
    #[cfg(unix)]
    #[test]
    fn j5_journal_entry_fifo_never_blocks() {
        let dir = TempDirectory::new("j5-journal-fifo");
        let fifo = dir.path("x.journal");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo runs")
                .success()
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        let probe = fifo.clone();
        std::thread::spawn(move || {
            let _ = sender.send(format!(
                "{:?}",
                journal_header_names(&probe, Path::new("/p"))
            ));
        });
        let verdict = receiver.recv_timeout(std::time::Duration::from_secs(5));
        if verdict.is_err() {
            // Unblock a regressed open: a writer opens and closes — EOF.
            drop(fs::OpenOptions::new().write(true).open(&fifo));
        }
        assert_eq!(
            verdict.as_deref(),
            Ok("Ok(false)"),
            "the FIFO is skipped unopened"
        );
    }

    /// followed immediately by `\n`. No newline, or trailing garbage before
    /// it, is uncommitted and never blocks.
    #[test]
    fn rr2_g6_committed_header() {
        let dir = TempDirectory::new("aw1-rr2-committed");
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        let project = dir.path("p.kinewright");
        let value = serde_json::json!({
            "format_version": 1,
            "writer_format_version": 1,
            "project_path": project,
            "initial_document": kinewright_core::Document::default(),
        });
        let journal = recovery.join("alias.journal");
        for (tail, claims) in [
            ("\n", true),
            ("", false),
            ("garbage\n", false),
            (" \n", false),
        ] {
            let text = format!(
                "{}{value}{tail}",
                std::str::from_utf8(JOURNAL_MAGIC).expect("the magic is text")
            );
            fs::write(&journal, text).expect("the journal writes");
            assert_eq!(
                pending_journal_for_project(&recovery, &project).expect("the lookup lands"),
                claims.then(|| journal.clone()),
                "tail {tail:?}"
            );
        }
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
