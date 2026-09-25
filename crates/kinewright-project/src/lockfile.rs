//! The project lock: a lock object with flock liveness plus a readable
//! discovery file (AW1 §5 as amended by the F3 erratum, R19, R40).
//!
//! `<stem>.kinewright.lock` is the lock OBJECT: created if absent, never
//! unlinked by anyone, contents unused — liveness is the held OS advisory
//! lock (`fs2`, both OSes) alone. The claim (pid/host/endpoint, no secret)
//! lives in `<stem>.kinewright.lock.json`, written atomically by the owner
//! only while holding the lock and readable by anyone at any time —
//! including on Windows, where a `LockFileEx` range denies second-handle
//! reads of the locked file itself (B4). Release removes the discovery
//! while still holding the lock, then unlocks; a stale discovery with a
//! free lock means a dead owner and reclaims with a warning. Any flock
//! failure reads as *held* (fail-closed).

use std::{
    fs::{self, File},
    io,
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

/// Derive a project's lock discovery path: the lock object name plus
/// `.json`, beside the project. `None` yields `None`.
#[must_use]
pub fn discovery_path_for_project(project_path: Option<&Path>) -> Option<PathBuf> {
    let lock = lockfile_path_for_project(project_path)?;
    let mut name = lock.into_os_string();
    name.push(".json");
    Some(PathBuf::from(name))
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
    /// The lock is held elsewhere. `owner` is the holding triple when
    /// its discovery parses (the full claim overflows `result_large_err`;
    /// the refusal only names the triple).
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

/// A held project lock: the open lock object with its flock, plus the
/// discovery this handle published. Dropping frees the flock and leaves a
/// reclaimable stale discovery; [`Self::release`] removes the discovery
/// first, so the next acquire is clean. The lock object itself is never
/// unlinked (F3).
#[derive(Debug)]
pub struct LockfileHandle {
    /// The lock object path.
    pub path: PathBuf,
    /// The discovery path this handle published.
    pub discovery: PathBuf,
    /// This handle's claim.
    pub claim: LockfileClaim,
    file: File,
}

impl LockfileHandle {
    fn held(path: PathBuf, discovery: PathBuf, claim: LockfileClaim, file: File) -> Self {
        Self {
            path,
            discovery,
            claim,
            file,
        }
    }

    /// Release: remove the discovery while holding the flock, then close.
    /// A failed removal degrades to a reclaimable stale discovery, never
    /// a lost unlock.
    /// # Errors
    /// Returns the removal IO error, if any.
    pub fn release(self) -> io::Result<()> {
        let Self {
            discovery, file, ..
        } = self;
        let removed = fs::remove_file(&discovery);
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

/// The identity triple claimed at a discovery path: `None` when
/// missing/unparseable. The discovery file is never locked, so this
/// second-handle read is legal on both OSes — including while the lock
/// is held (B4).
fn read_owner(discovery_path: &Path) -> Option<ReclaimedOwner> {
    let bytes = fs::read(discovery_path).ok()?;
    let claim: LockfileClaim = serde_json::from_slice(&bytes).ok()?;
    Some(ReclaimedOwner {
        pid: claim.pid,
        hostname: claim.hostname,
        endpoint: claim.endpoint,
    })
}

/// Publish a claim to the discovery path, atomically (temp + rename,
/// synced through the one handle). Call only while holding the lock —
/// the flock serialises publishers.
fn write_discovery(discovery_path: &Path, claim: &LockfileClaim) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(claim)
        .map_err(|error| io::Error::other(format!("could not serialize the lockfile: {error}")))?;
    crate::project::write_file_atomic(discovery_path, &bytes)
}

fn build_claim(
    project: &Path,
    mode: LockMode,
    endpoint: &str,
    reclaimed_from: Option<ReclaimedOwner>,
) -> (PathBuf, PathBuf, LockfileClaim) {
    let canonical = crate::project::canonical_session_key(project);
    let canonical_text = canonical.to_string_lossy().into_owned();
    let lock_path = lockfile_path_for_project(Some(project))
        .unwrap_or_else(|| PathBuf::from(format!("{}.{LOCKFILE_SUFFIX}", project.display())));
    let discovery_path = {
        let mut name = lock_path.as_os_str().to_owned();
        name.push(".json");
        PathBuf::from(name)
    };
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
    (lock_path, discovery_path, claim)
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
    let (lock_path, discovery_path, mut claim) = build_claim(project_path, mode, endpoint, None);
    let attempts = attempts.max(1);
    for attempt in 1..=attempts {
        let last = attempt == attempts;
        // Open-or-create: the lock object is never unlinked, so a create
        // race is harmless — every opener flocks the same object (F3/L1).
        // The contents are unused and never truncated: truncating through
        // a contender's handle could fail against a live Windows
        // `LockFileEx` range, turning contention into an IO error.
        let file = match File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => file,
            // A missing parent never resolves by retrying.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(LockfileError::Io(format!(
                    "could not create {}: {error}",
                    lock_path.display()
                )));
            }
            // Permission and the like: retry, then fail.
            Err(error) => {
                if last {
                    return Err(LockfileError::Io(error.to_string()));
                }
                std::thread::sleep(retry_delay);
                continue;
            }
        };
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_lock_open_before_flock");
        if file.try_lock_exclusive().is_err() {
            // Held: contention, never a steal. The owner read is
            // best-effort; the verdict never is.
            drop(file);
            let owner = read_owner(&discovery_path);
            if last {
                return Err(LockfileError::Contention {
                    path: project_path.to_path_buf(),
                    owner,
                });
            }
            std::thread::sleep(retry_delay);
            continue;
        }
        // We hold the flock, so any discovery present is stale — a live
        // owner holds the lock. Pending recovery refuses first (S7), on
        // every ownership path.
        if let Some(journal) = pending_journal_for_project(recovery_dir, project_path) {
            drop(file);
            return Err(LockfileError::PendingRecovery { journal });
        }
        let previous = read_owner(&discovery_path);
        claim.reclaimed_from.clone_from(&previous);
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_flock_before_publish");
        if let Err(error) = write_discovery(&discovery_path, &claim) {
            drop(file);
            return Err(LockfileError::Io(error.to_string()));
        }
        return Ok(AcquiredLock {
            handle: LockfileHandle::held(lock_path, discovery_path, claim, file),
            reclaimed: previous,
        });
    }
    unreachable!("the loop above always returns");
}

#[cfg(test)]
mod tests {
    use std::{
        process::{Child, Command, Stdio},
        time::Instant,
    };

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
        let text = fs::read_to_string(&acquired.handle.discovery).expect("the discovery reads");
        let value: serde_json::Value = serde_json::from_str(&text).expect("the discovery is JSON");
        let keys: Vec<&str> = value
            .as_object()
            .expect("the discovery is an object")
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
        let before = fs::read(&first.handle.discovery).expect("the discovery reads");
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
            fs::read(&first.handle.discovery).expect("the discovery re-reads"),
            before,
            "the refused acquire writes nothing"
        );
        first.handle.release().expect("the release lands");
    }

    /// R19/F3: release frees the lock for a clean re-acquire — the
    /// discovery goes while the flock is held, the lock object stays
    /// (never unlinked by anyone), and the next acquire is fresh with no
    /// reclaim.
    #[test]
    fn r19_release_frees_the_lock_for_a_clean_reacquire() {
        let dir = TempDirectory::new("aw1-r19-gone");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let lock_path = lockfile_path_for_project(Some(&project)).expect("a lockfile derives");
        assert!(!lock_path.exists(), "no lock object precedes the acquire");
        let first =
            acquire_project_lock(&project, LockMode::Gui, "http://127.0.0.1:9/mcp", &recovery)
                .expect("the acquire lands");
        let discovery = first.handle.discovery.clone();
        assert!(lock_path.exists(), "the acquire leaves a lock object");
        assert!(discovery.exists(), "and publishes its discovery");
        first.handle.release().expect("the release lands");
        assert!(lock_path.exists(), "the lock object is never unlinked");
        assert!(!discovery.exists(), "the release removes the discovery");
        let second = acquire_project_lock(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
        )
        .expect("the re-acquire lands");
        assert!(
            second.reclaimed.is_none() && second.handle.claim.reclaimed_from.is_none(),
            "a released lock reclaims nothing"
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
        let discovery = first.handle.discovery.clone();
        let before = fs::read(&discovery).expect("the discovery reads");
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
            fs::read(&discovery).expect("the discovery re-reads"),
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

    /// Re-exec entry for the interleaving children: the parent spawns this
    /// binary with `--exact lockfile::tests::lock_child` plus `REV2_CHILD`.
    /// A plain test run returns immediately.
    #[test]
    fn lock_child() {
        if std::env::var("REV2_CHILD").as_deref() != Ok("owner") {
            return;
        }
        let project = PathBuf::from(std::env::var_os("REV2_PROJECT").unwrap());
        let recovery = PathBuf::from(std::env::var_os("REV2_RECOVERY").unwrap());
        let signals = PathBuf::from(std::env::var_os("REV2_SIGNALS").unwrap());
        match acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:40123/mcp",
            &recovery,
            1,
            Duration::ZERO,
        ) {
            Ok(owner) => {
                fs::write(signals.join("owned"), owner.handle.claim.pid.to_string())
                    .expect("the owned signal writes");
                wait_for_signal(&signals.join("done"));
                drop(owner);
            }
            Err(error) => {
                fs::write(signals.join("error"), format!("{error:?}")).expect("the error writes");
            }
        }
    }

    /// A child that is always reaped: killed on drop, then waited.
    struct Kid(Child);

    impl Drop for Kid {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Spawn this test binary as a lock claimant; `hook` arms one
    /// `test_hook` pause point in the child only (the parent never sets
    /// the hook variables, so parallel tests cannot interfere).
    fn spawn_claimant(project: &Path, recovery: &Path, signals: &Path, hook: &str) -> Kid {
        fs::create_dir_all(signals).expect("the signals dir creates");
        let mut command = Command::new(std::env::current_exe().expect("the test binary resolves"));
        command
            .args(["--exact", "lockfile::tests::lock_child", "--nocapture"])
            .env("REV2_CHILD", "owner")
            .env("REV2_PROJECT", project)
            .env("REV2_RECOVERY", recovery)
            .env("REV2_SIGNALS", signals)
            .env("REV2_HOOK", hook)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Kid(command.spawn().expect("the claimant spawns"))
    }

    /// Wait up to 20 s for a signal file (the hook deadline, mirrored).
    fn wait_for_signal(path: &Path) {
        let until = Instant::now() + Duration::from_secs(20);
        while !path.exists() {
            assert!(Instant::now() < until, "waiting for {}", path.display());
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// F3/L1 port: a contender that loses the flock must leave the live
    /// lock object alone — the live owner stays the sole owner.
    #[test]
    fn f3_failed_contender_leaves_the_live_lock_object() {
        let dir = TempDirectory::new("aw1-f3-l1");
        let project = dir.path("edit.kinewright");
        let recovery = recovery_dir(&dir);
        let signals = dir.path("a");
        let mut loser = spawn_claimant(
            &project,
            &recovery,
            &signals,
            "after_lock_open_before_flock",
        );
        wait_for_signal(&signals.join("paused"));
        let live = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the live claimant locks the merely opened file");
        fs::write(signals.join("resume"), "").expect("the resume writes");
        assert!(
            loser.0.wait().expect("the loser exits").success(),
            "the loser exits cleanly"
        );
        wait_for_signal(&signals.join("error"));
        assert!(
            matches!(
                acquire_project_lock_with_policy(
                    &project,
                    LockMode::Headless,
                    "http://127.0.0.1:10/mcp",
                    &recovery,
                    1,
                    Duration::ZERO
                ),
                Err(LockfileError::Contention { .. })
            ),
            "the live owner is the sole owner"
        );
        assert!(live.handle.path.exists(), "the lock object survives");
        drop(live);
    }

    /// F3/L2 port: a waiter that opened the lock object before a release
    /// must not own it after — the new published owner stands alone.
    #[test]
    fn f3_waiter_never_owns_after_a_release() {
        let dir = TempDirectory::new("aw1-f3-l2");
        let project = dir.path("edit.kinewright");
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
        let signals = dir.path("b");
        let mut waiter = spawn_claimant(
            &project,
            &recovery,
            &signals,
            "after_lock_open_before_flock",
        );
        wait_for_signal(&signals.join("paused"));
        first.handle.release().expect("the release lands");
        let live = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the new owner acquires");
        fs::write(signals.join("resume"), "").expect("the resume writes");
        // Either verdict ends the wait: `owned` (the bug) or `error`.
        let until = Instant::now() + Duration::from_secs(20);
        while !signals.join("owned").exists() && !signals.join("error").exists() {
            assert!(Instant::now() < until, "waiting for the waiter verdict");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            signals.join("error").exists(),
            "the waiter refuses: it never owns beside the published owner"
        );
        assert!(!signals.join("owned").exists(), "no second owner");
        // A buggy owner waits for `done`; a refused child already exited.
        fs::write(signals.join("done"), "").expect("the done writes");
        assert!(
            waiter.0.wait().expect("the waiter exits").success(),
            "the waiter exits cleanly"
        );
        drop(live);
    }

    /// The discovery derives from the lock object name plus `.json` —
    /// and nowhere for a pathless project.
    #[test]
    fn discovery_path_derives_from_the_lock_object() {
        assert_eq!(discovery_path_for_project(None), None);
        assert_eq!(
            discovery_path_for_project(Some(Path::new("/tmp/x/edit.kinewright"))),
            Some(PathBuf::from("/tmp/x/edit.kinewright.lock.json"))
        );
        assert_eq!(
            discovery_path_for_project(Some(Path::new("edit.kinewright"))),
            Some(PathBuf::from("edit.kinewright.lock.json"))
        );
        assert_eq!(discovery_path_for_project(Some(Path::new("/"))), None);
    }

    /// F3/B4: the discovery reads through a second handle while the lock
    /// is held — the `LockFileEx` range covers the lock object only, so
    /// the contender's refusal names the live owner. (The Windows CI lane
    /// is the real check: OS error 33 on a second-handle read of a
    /// locked file is what the old single-file protocol hit.)
    #[test]
    fn f3_discovery_reads_while_the_lock_is_held() {
        let dir = TempDirectory::new("aw1-f3-held-read");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let live = acquire_project_lock_with_policy(
            &project,
            LockMode::Gui,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the acquire lands");
        let bytes = fs::read(&live.handle.discovery).expect("the discovery reads while held");
        let claim: LockfileClaim = serde_json::from_slice(&bytes).expect("the discovery parses");
        assert_eq!(claim.pid, std::process::id());
        assert_eq!(claim.endpoint, "http://127.0.0.1:9/mcp");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("a held lock refuses");
        let LockfileError::Contention { owner, .. } = error else {
            panic!("contention, got {error}");
        };
        let owner = owner.expect("the refusal names its owner");
        assert_eq!(owner.pid, std::process::id());
        assert_eq!(owner.endpoint, "http://127.0.0.1:9/mcp");
        live.handle.release().expect("the release lands");
    }

    /// F3: a live owner is never reclaimed; once killed, its stale
    /// discovery reclaims with the typed warning.
    #[test]
    fn f3_killed_owner_reclaims_with_warning() {
        let dir = TempDirectory::new("aw1-f3-kill");
        let project = dir.path("edit.kinewright");
        let recovery = recovery_dir(&dir);
        let signals = dir.path("child");
        let mut child = spawn_claimant(&project, &recovery, &signals, "");
        wait_for_signal(&signals.join("owned"));
        let pid = child.0.id();
        assert!(
            matches!(
                acquire_project_lock_with_policy(
                    &project,
                    LockMode::Headless,
                    "http://127.0.0.1:10/mcp",
                    &recovery,
                    1,
                    Duration::ZERO
                ),
                Err(LockfileError::Contention { .. })
            ),
            "a live owner is never reclaimed"
        );
        child.0.kill().expect("the kill lands");
        child.0.wait().expect("the child reaps");
        let got = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:11/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the reclaim lands");
        let previous = got.reclaimed.expect("the reclaim warns");
        assert_eq!(previous.pid, pid);
        assert!(!previous.hostname.is_empty());
        assert_eq!(got.handle.claim.reclaimed_from, Some(previous.clone()));
        let warning: serde_json::Value =
            serde_json::from_str(&reclaim_warning_json(&previous)).expect("the warning is JSON");
        assert_eq!(warning["code"], "lock_reclaimed");
        assert_eq!(warning["previous_pid"], pid);
        got.handle.release().expect("the release lands");
    }

    /// F3: a holder paused between flock and publish still excludes — the
    /// contender refuses ownerless until the publish lands, named after.
    /// Two concurrent owners are impossible at this hook point too.
    #[test]
    fn f3_holder_excludes_before_its_publish() {
        let dir = TempDirectory::new("aw1-f3-publish");
        let project = dir.path("edit.kinewright");
        let recovery = recovery_dir(&dir);
        let signals = dir.path("holder");
        let holder = spawn_claimant(&project, &recovery, &signals, "after_flock_before_publish");
        wait_for_signal(&signals.join("paused"));
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("the unpublished holder still excludes");
        assert!(
            matches!(error, LockfileError::Contention { owner: None, .. }),
            "no discovery yet, still contention: {error}"
        );
        fs::write(signals.join("resume"), "").expect("the resume writes");
        wait_for_signal(&signals.join("owned"));
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:11/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("the published holder excludes");
        assert!(
            matches!(error, LockfileError::Contention { owner: Some(_), .. }),
            "the publish names the owner: {error}"
        );
        fs::write(signals.join("done"), "").expect("the done writes");
        drop(holder);
    }
}
