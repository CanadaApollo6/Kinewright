//! The project lockfile: discovery with flock liveness (AW1 §5, R19, R40).
//!
//! `<stem>.kinewright.lock` holds pid/host/endpoint ONLY — no secret. Created
//! `O_EXCL`; the owner holds an OS advisory lock (`fs2`, both OSes) for its
//! lifetime, so liveness is the held flock. A flock-free lockfile reclaims
//! after the pending-journal check. Writes sync through the one open handle
//! (Windows-safe); any flock failure reads as *held* (fail-closed).

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};

use crate::recovery::{fnv1a_64, pending_journal_for_project};

/// The lockfile suffix: `<stem>.kinewright.lock` beside the project.
pub const LOCKFILE_SUFFIX: &str = "kinewright.lock";

/// The lockfile format version this build writes.
pub const LOCKFILE_FORMAT_VERSION: u32 = 1;

/// Acquire attempts before contention (AW1 §5: 3 × 250 ms). Retries cover a
/// dying flock and the Windows delete-pending window — never a steal.
pub const LOCK_ACQUIRE_ATTEMPTS: u32 = 3;

/// The delay between acquire attempts (AW1 §5).
pub const LOCK_ACQUIRE_RETRY_DELAY: Duration = Duration::from_millis(250);

/// Who owns a project lock (AW1 §5 `mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockMode {
    /// The GUI's live-core server.
    Gui,
    /// A headless `kinewright mcp` with its own endpoint.
    Headless,
}

/// The previous owner's identity, kept on a reclaim (`reclaimed_from`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReclaimedOwner {
    pub pid: u32,
    pub hostname: String,
    pub endpoint: String,
}

/// The lockfile claim (AW1 §5): pid/host/endpoint, no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockfileClaim {
    pub format_version: u32,
    pub mode: LockMode,
    /// The canonical project path.
    pub project: String,
    pub pid: u32,
    pub hostname: String,
    /// The owner's loopback MCP endpoint.
    pub endpoint: String,
    /// The token file *reference* — a name, never the secret.
    pub token_ref: String,
    pub kinewright_version: String,
    /// Owner start, unix seconds.
    pub started_at_unix: u64,
    /// The reclaimed owner, `None` on a fresh acquire.
    pub reclaimed_from: Option<ReclaimedOwner>,
}

/// Derive a project's lockfile path: the sibling `<stem>.kinewright.lock`.
/// `None` yields `None` and paths without a file stem likewise do.
#[must_use]
pub fn lockfile_path_for_project(project_path: Option<&Path>) -> Option<PathBuf> {
    let path = project_path?;
    let stem = path.file_stem()?;
    let mut name = stem.to_os_string();
    name.push(".");
    name.push(LOCKFILE_SUFFIX);
    Some(path.parent().unwrap_or_else(|| Path::new("")).join(name))
}

/// This machine's hostname: `HOSTNAME`, `COMPUTERNAME`, then `"unknown"`.
fn current_hostname() -> String {
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Some(value) = std::env::var_os(key)
            && let Some(name) = value.to_str().filter(|name| !name.is_empty())
        {
            return name.to_owned();
        }
    }
    "unknown".to_owned()
}

/// Why a project lock acquire failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockfileError {
    /// The lockfile exists and its flock is held. `owner` is the holding
    /// triple when its claim parses (the full claim overflows
    /// `result_large_err`; the refusal only names the triple).
    Contention {
        path: PathBuf,
        owner: Option<ReclaimedOwner>,
    },
    /// Takeover refused: a journal pends — restore or discard it in the GUI.
    PendingRecovery {
        journal: PathBuf,
    },
    Io(String),
}

impl std::fmt::Display for LockfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Contention { path, owner } => {
                write!(formatter, "{} is live elsewhere", path.display())?;
                match owner {
                    Some(owner) => write!(
                        formatter,
                        " (owner pid {} on {}, endpoint {})",
                        owner.pid, owner.hostname, owner.endpoint
                    ),
                    None => write!(formatter, " (its lock is held)"),
                }
            }
            Self::PendingRecovery { journal } => write!(
                formatter,
                "not taking over: a recovery journal is pending ({}) — \
                 restore or discard it in the GUI first",
                journal.display()
            ),
            Self::Io(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for LockfileError {}

/// A held project lock: the open lockfile with its flock. Dropping frees
/// the flock and leaves a reclaimable file; [`Self::release`] removes it.
#[derive(Debug)]
pub struct LockfileHandle {
    /// The lockfile path.
    pub path: PathBuf,
    /// This handle's claim.
    pub claim: LockfileClaim,
    file: File,
}

impl LockfileHandle {
    fn held(path: PathBuf, claim: LockfileClaim, file: File) -> Self {
        Self { path, claim, file }
    }

    /// Release: remove the lockfile while holding the flock, then close.
    /// A failed removal degrades to a reclaimable file, never a lost unlock.
    /// # Errors
    /// Returns the removal IO error, if any.
    pub fn release(self) -> io::Result<()> {
        let Self { path, file, .. } = self;
        let removed = fs::remove_file(&path);
        drop(file);
        match removed {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// What one acquire did.
#[derive(Debug)]
pub struct AcquiredLock {
    pub handle: LockfileHandle,
    /// The reclaimed owner, when set (warn via [`reclaim_warning_json`]).
    pub reclaimed: Option<ReclaimedOwner>,
}

/// The typed JSON warning a reclaim owes on stderr (AW1 §5).
#[must_use]
pub fn reclaim_warning_json(previous: &ReclaimedOwner) -> String {
    serde_json::json!({
        "code": "lock_reclaimed",
        "previous_pid": previous.pid,
        "previous_hostname": previous.hostname,
        "previous_endpoint": previous.endpoint,
    })
    .to_string()
}

/// The identity triple claimed at a path: `None` when missing/unparseable.
fn read_owner(lock_path: &Path) -> Option<ReclaimedOwner> {
    let bytes = fs::read(lock_path).ok()?;
    let claim: LockfileClaim = serde_json::from_slice(&bytes).ok()?;
    Some(ReclaimedOwner {
        pid: claim.pid,
        hostname: claim.hostname,
        endpoint: claim.endpoint,
    })
}

/// Write a claim through the one open handle and sync it there.
fn write_claim(file: &mut File, claim: &LockfileClaim) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(claim)
        .map_err(|error| io::Error::other(format!("could not serialize the lockfile: {error}")))?;
    file.set_len(0)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn build_claim(
    project: &Path,
    mode: LockMode,
    endpoint: &str,
    reclaimed_from: Option<ReclaimedOwner>,
) -> (PathBuf, LockfileClaim) {
    let canonical = crate::project::canonical_session_key(project);
    let canonical_text = canonical.to_string_lossy().into_owned();
    let lock_path = lockfile_path_for_project(Some(project))
        .unwrap_or_else(|| PathBuf::from(format!("{}.{LOCKFILE_SUFFIX}", project.display())));
    let claim = LockfileClaim {
        format_version: LOCKFILE_FORMAT_VERSION,
        mode,
        project: canonical_text.clone(),
        pid: std::process::id(),
        hostname: current_hostname(),
        endpoint: endpoint.to_owned(),
        token_ref: format!(
            "kinewright/{:016x}.token",
            fnv1a_64(canonical_text.as_bytes())
        ),
        kinewright_version: env!("CARGO_PKG_VERSION").to_owned(),
        started_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
        reclaimed_from,
    };
    (lock_path, claim)
}

/// Acquire the project lock with the §5 policy (`recovery_dir` feeds the
/// pending-journal check).
/// # Errors
/// `Contention`, `PendingRecovery`, or `Io`.
pub fn acquire_project_lock(
    project_path: &Path,
    mode: LockMode,
    endpoint: &str,
    recovery_dir: &Path,
) -> Result<AcquiredLock, LockfileError> {
    acquire_project_lock_with_policy(
        project_path,
        mode,
        endpoint,
        recovery_dir,
        LOCK_ACQUIRE_ATTEMPTS,
        LOCK_ACQUIRE_RETRY_DELAY,
    )
}

/// [`acquire_project_lock`] with an explicit retry policy (tests pass a
/// fast one; production uses the §5 constants).
/// # Errors
/// As [`acquire_project_lock`].
pub fn acquire_project_lock_with_policy(
    project_path: &Path,
    mode: LockMode,
    endpoint: &str,
    recovery_dir: &Path,
    attempts: u32,
    retry_delay: Duration,
) -> Result<AcquiredLock, LockfileError> {
    let (lock_path, mut claim) = build_claim(project_path, mode, endpoint, None);
    let attempts = attempts.max(1);
    for attempt in 1..=attempts {
        let last = attempt == attempts;
        match File::options()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                // Fresh file: lock first, then write. Our own process
                // holding this path fails the lock — remove what we made.
                if file.try_lock_exclusive().is_err() {
                    let _ = fs::remove_file(&lock_path);
                    return Err(LockfileError::Contention {
                        path: project_path.to_path_buf(),
                        owner: None,
                    });
                }
                if let Err(error) = write_claim(&mut file, &claim) {
                    let _ = fs::remove_file(&lock_path);
                    return Err(LockfileError::Io(error.to_string()));
                }
                return Ok(AcquiredLock {
                    handle: LockfileHandle::held(lock_path, claim, file),
                    reclaimed: None,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                match File::options().read(true).write(true).open(&lock_path) {
                    // A raced release (gone file) and every other open
                    // failure alike: retry, then fail.
                    Err(error) => {
                        if last {
                            return Err(LockfileError::Io(error.to_string()));
                        }
                        std::thread::sleep(retry_delay);
                    }
                    Ok(mut file) => {
                        if file.try_lock_exclusive().is_err() {
                            // Held: contention, never a steal. The owner
                            // read is best-effort; the verdict never is.
                            let owner = read_owner(&lock_path);
                            if last {
                                return Err(LockfileError::Contention {
                                    path: project_path.to_path_buf(),
                                    owner,
                                });
                            }
                            std::thread::sleep(retry_delay);
                            continue;
                        }
                        // Flock-free: the owner died. Takeover first checks
                        // for a pending journal (S7).
                        if let Some(journal) =
                            pending_journal_for_project(recovery_dir, project_path)
                        {
                            drop(file);
                            return Err(LockfileError::PendingRecovery { journal });
                        }
                        let previous = read_owner(&lock_path);
                        claim.reclaimed_from.clone_from(&previous);
                        if let Err(error) = write_claim(&mut file, &claim) {
                            return Err(LockfileError::Io(error.to_string()));
                        }
                        return Ok(AcquiredLock {
                            handle: LockfileHandle::held(lock_path, claim, file),
                            reclaimed: previous,
                        });
                    }
                }
            }
            // A missing parent never resolves by retrying.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(LockfileError::Io(format!(
                    "could not create {}: {error}",
                    lock_path.display()
                )));
            }
            // Permission, Windows delete-pending, and the like: retry.
            Err(error) => {
                if last {
                    return Err(LockfileError::Io(error.to_string()));
                }
                std::thread::sleep(retry_delay);
            }
        }
    }
    unreachable!("the loop above always returns");
}

#[cfg(test)]
mod tests {
    use kinewright_media::test_support::TempDirectory;

    use crate::recovery::journal_file_name;

    use super::*;

    fn recovery_dir(dir: &TempDirectory) -> PathBuf {
        let recovery = dir.path("recovery");
        fs::create_dir(&recovery).expect("the recovery dir creates");
        recovery
    }

    /// R40: the lock carries pid + hostname and no secret — the file holds
    /// exactly the §5 keys, and the token is a reference, never bytes.
    #[test]
    fn r40_lock_carries_pid_hostname_and_no_secret() {
        let dir = TempDirectory::new("aw1-r40-claim");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let acquired = acquire_project_lock(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &recovery,
        )
        .expect("the acquire lands");
        let claim = acquired.handle.claim.clone();
        assert_eq!(claim.format_version, LOCKFILE_FORMAT_VERSION);
        assert_eq!(claim.mode, LockMode::Headless);
        assert_eq!(claim.pid, std::process::id(), "the lock carries the pid");
        assert!(!claim.hostname.is_empty(), "and the hostname");
        assert_eq!(claim.endpoint, "http://127.0.0.1:9/mcp");
        assert!(
            claim.reclaimed_from.is_none(),
            "a fresh acquire reclaims nothing"
        );
        let canonical = crate::project::canonical_session_key(&project)
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            claim.token_ref,
            format!(
                "kinewright/{:016x}.token",
                crate::recovery::fnv1a_64(canonical.as_bytes())
            ),
            "the token is a canonical-path reference, never bytes"
        );
        let text = fs::read_to_string(&acquired.handle.path).expect("the lockfile reads");
        let value: serde_json::Value = serde_json::from_str(&text).expect("the lockfile is JSON");
        let keys: Vec<&str> = value
            .as_object()
            .expect("the lockfile is an object")
            .keys()
            .map(String::as_str)
            .collect();
        for expected in [
            "format_version",
            "mode",
            "project",
            "pid",
            "hostname",
            "endpoint",
            "token_ref",
            "kinewright_version",
            "started_at_unix",
            "reclaimed_from",
        ] {
            assert!(
                keys.contains(&expected),
                "the lock carries {expected}: {text}"
            );
        }
        assert_eq!(keys.len(), 10, "and nothing else — no secret: {text}");
        assert!(
            !text.contains("bearer") && !text.contains("secret") && !text.contains("\"token\""),
            "no secret material (`token_ref` is a name, not bytes): {text}"
        );
        acquired.handle.release().expect("the release lands");
    }

    /// R19: a second acquire while the flock is held refuses with the
    /// owner's identity — and the lockfile bytes are never stolen.
    #[test]
    fn r19_second_acquire_refuses_while_held() {
        let dir = TempDirectory::new("aw1-r19-held");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let first = acquire_project_lock_with_policy(
            &project,
            LockMode::Gui,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the first acquire lands");
        let before = fs::read(&first.handle.path).expect("the lockfile reads");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("a held flock is never stolen");
        assert!(
            error.to_string().contains("live elsewhere"),
            "the refusal names the live owner: {error}"
        );
        let LockfileError::Contention { owner, .. } = &error else {
            panic!("contention, got {error}");
        };
        let owner = owner.as_ref().expect("the refusal names its owner");
        assert_eq!(owner.pid, std::process::id());
        assert_eq!(owner.endpoint, "http://127.0.0.1:9/mcp");
        assert_eq!(
            fs::read(&first.handle.path).expect("the lockfile re-reads"),
            before,
            "the refused acquire writes nothing"
        );
        first.handle.release().expect("the release lands");
    }

    /// R19: a gone lockfile acquires clean — release removes the file, and
    /// the next acquire is fresh with no reclaim.
    #[test]
    fn r19_gone_lockfile_acquires_clean() {
        let dir = TempDirectory::new("aw1-r19-gone");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let lock_path = lockfile_path_for_project(Some(&project)).expect("a lockfile derives");
        assert!(!lock_path.exists(), "no lockfile precedes the acquire");
        let first =
            acquire_project_lock(&project, LockMode::Gui, "http://127.0.0.1:9/mcp", &recovery)
                .expect("the acquire lands");
        assert!(lock_path.exists(), "the acquire leaves a lockfile");
        first.handle.release().expect("the release lands");
        assert!(!lock_path.exists(), "the release removes the lockfile");
        let second = acquire_project_lock(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
        )
        .expect("the re-acquire lands");
        assert!(
            second.reclaimed.is_none() && second.handle.claim.reclaimed_from.is_none(),
            "a gone lockfile reclaims nothing"
        );
        second.handle.release().expect("the release lands");
    }

    /// R19: a flock-free lockfile means a dead owner — the next acquirer
    /// reclaims it with `reclaimed_from` set and the typed warning owed.
    #[test]
    fn r19_dead_owner_reclaims_with_warning() {
        let dir = TempDirectory::new("aw1-r19-reclaim");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let first =
            acquire_project_lock(&project, LockMode::Gui, "http://127.0.0.1:9/mcp", &recovery)
                .expect("the acquire lands");
        let lock_path = first.handle.path.clone();
        drop(first); // The crash: the fd closes, the file stays.
        assert!(lock_path.exists(), "a dead owner leaves its lockfile");
        let second = acquire_project_lock(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
        )
        .expect("the reclaim lands");
        let previous = second.reclaimed.as_ref().expect("the reclaim warns");
        assert_eq!(previous.pid, std::process::id());
        assert_eq!(previous.endpoint, "http://127.0.0.1:9/mcp");
        assert_eq!(
            second.handle.claim.reclaimed_from.as_ref(),
            Some(previous),
            "`reclaimed_from` is set"
        );
        assert_eq!(second.handle.claim.mode, LockMode::Headless);
        let warning: serde_json::Value =
            serde_json::from_str(&reclaim_warning_json(previous)).expect("the warning is JSON");
        assert_eq!(warning["code"], "lock_reclaimed");
        assert_eq!(warning["previous_endpoint"], "http://127.0.0.1:9/mcp");
        second.handle.release().expect("the release lands");
    }

    /// R19/S7: takeover first checks for a pending journal — a reclaim with
    /// crash data pending refuses naming GUI restore, and the dead owner's
    /// lockfile survives for the GUI to take.
    #[test]
    fn r19_reclaim_refuses_a_pending_journal() {
        let dir = TempDirectory::new("aw1-r19-pending");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let first =
            acquire_project_lock(&project, LockMode::Gui, "http://127.0.0.1:9/mcp", &recovery)
                .expect("the acquire lands");
        let lock_path = first.handle.path.clone();
        let before = fs::read(&lock_path).expect("the lockfile reads");
        drop(first); // The crash.
        let journal = recovery.join(journal_file_name(&project));
        fs::write(&journal, b"crash bytes").expect("the pending journal writes");
        let error = acquire_project_lock(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
        )
        .expect_err("a pending journal refuses the takeover");
        assert!(
            error.to_string().contains("in the GUI"),
            "the refusal names GUI restore: {error}"
        );
        let LockfileError::PendingRecovery { journal: found } = error else {
            panic!("pending_recovery, got {error}");
        };
        assert_eq!(found, journal);
        assert_eq!(
            fs::read(&lock_path).expect("the lockfile re-reads"),
            before,
            "the refused takeover writes nothing"
        );
    }

    /// R19: contention backs off (3 attempts, 250 ms apart) before refusing.
    #[test]
    fn r19_contention_backs_off_before_refusing() {
        let dir = TempDirectory::new("aw1-r19-backoff");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let _held =
            acquire_project_lock(&project, LockMode::Gui, "http://127.0.0.1:9/mcp", &recovery)
                .expect("the acquire lands");
        let began = std::time::Instant::now();
        let error = acquire_project_lock(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
        )
        .expect_err("a held flock refuses");
        assert!(
            matches!(error, LockfileError::Contention { .. }),
            "contention, got {error}"
        );
        // 3 attempts bracket 2 sleeps of 250 ms; the floor allows scheduling
        // slack, and only a floor is asserted — loaded machines sleep long.
        assert!(
            began.elapsed() >= Duration::from_millis(450),
            "the backoff ran before the refusal"
        );
    }

    /// The lockfile derives from the stem beside the project, like the
    /// sidecar — and nowhere for a pathless project.
    #[test]
    fn lockfile_path_derives_from_the_stem() {
        assert_eq!(lockfile_path_for_project(None), None);
        assert_eq!(
            lockfile_path_for_project(Some(Path::new("/tmp/x/edit.kinewright"))),
            Some(PathBuf::from("/tmp/x/edit.kinewright.lock"))
        );
        assert_eq!(
            lockfile_path_for_project(Some(Path::new("edit.kinewright"))),
            Some(PathBuf::from("edit.kinewright.lock"))
        );
        assert_eq!(lockfile_path_for_project(Some(Path::new("/"))), None);
    }
}
