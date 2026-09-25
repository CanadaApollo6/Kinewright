//! Never-unlinked lock object (flock liveness) + discovery (AW1 §5/F3).
//! The claim (pid/host/endpoint, no secret) publishes atomically while
//! holding the lock and reads at any time — even on Windows, where
//! `LockFileEx` denies second-handle reads of the locked file (B4).
//! Release removes the discovery first, then unlocks explicitly — never
//! a last-close race against a forked duplicate (F3). Stale discovery
//! reclaims unless foreign (F7 — host-local liveness; no multi-host
//! exclusion). Flock failure reads as *held* (fail-closed).

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

/// Sibling lock beside the real file, via the identity (F4).
#[must_use]
pub fn lockfile_path_for_project(project_path: Option<&Path>) -> Option<PathBuf> {
    let path = project_path?;
    let identity = crate::project::canonical_project_identity(path);
    let stem = identity.file_stem()?;
    let mut name = stem.to_os_string();
    name.push(".");
    name.push(LOCKFILE_SUFFIX);
    let parent = identity.parent().unwrap_or_else(|| Path::new(""));
    Some(parent.join(name))
}

/// The lock object name plus `.json`, beside the project.
#[must_use]
pub fn discovery_path_for_project(project_path: Option<&Path>) -> Option<PathBuf> {
    let lock = lockfile_path_for_project(project_path)?;
    let mut name = lock.into_os_string();
    name.push(".json");
    Some(PathBuf::from(name))
}

/// Absent host info; stale claims carrying it still reclaim (F7).
const UNKNOWN_HOSTNAME: &str = "unknown";

/// This machine's hostname: OS name, env, [`UNKNOWN_HOSTNAME`].
fn current_hostname() -> String {
    if let Some(name) = gethostname::gethostname()
        .to_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Some(value) = std::env::var_os(key)
            && let Some(name) = value
                .to_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
        {
            return name.to_owned();
        }
    }
    UNKNOWN_HOSTNAME.to_owned()
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
    /// Takeover refused: the journal lookup failed — fail closed (F5).
    RecoveryLookup {
        reason: String,
    },
    /// Takeover refused: stale lock names a known foreign host (F7).
    ForeignHost {
        host: String,
        endpoint: String,
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
            Self::RecoveryLookup { reason } => write!(formatter, "recovery scan failed: {reason}"),
            Self::ForeignHost { host, endpoint: _ } => write!(formatter, "foreign host {host}"),
            Self::Io(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for LockfileError {}

/// A held lock: open object plus published discovery. Never unlinked (F3).
#[derive(Debug)]
pub struct LockfileHandle {
    /// The lockfile path.
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

    /// Release: remove the discovery while holding the flock, then drop —
    /// the drop unlocks explicitly, so the release lands even when a
    /// forked child still holds a duplicate of the open description.
    /// # Errors
    /// Returns the removal IO error, if any.
    pub fn release(self) -> io::Result<()> {
        let removed = fs::remove_file(&self.discovery);
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("release_after_remove_before_unlock"); // RACE-REVIEW
        drop(self);
        match removed {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for LockfileHandle {
    /// Explicit unlock before close: a bare close releases the flock only
    /// at the last close of the open description, so a forked child that
    /// inherited the fd (a spawned claimant, an ffmpeg helper) would hold
    /// the release hostage until its exec. `LOCK_UN` releases regardless
    /// of duplicates — the release is deterministic.
    fn drop(&mut self) {
        let _ = self.file.unlock();
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

/// Triple at a discovery path (`None` if missing/torn); legal while held (B4).
fn read_owner(discovery_path: &Path) -> Option<ReclaimedOwner> {
    let bytes = fs::read(discovery_path).ok()?;
    let claim: LockfileClaim = serde_json::from_slice(&bytes).ok()?;
    Some(ReclaimedOwner {
        pid: claim.pid,
        hostname: claim.hostname,
        endpoint: claim.endpoint,
    })
}

/// Release an acquire attempt's flock before its close: explicit
/// unlock, then the caller drops. A bare close releases only at the
/// last close of the open description — see [`LockfileHandle`]'s [`Drop`].
fn unlock_attempt(file: &File) {
    let _ = file.unlock();
}

/// Publish a claim atomically. Call only while holding the lock — the
/// flock serialises publishers.
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
    let canonical = crate::project::canonical_project_identity(project);
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
/// `Contention`, `PendingRecovery`, `RecoveryLookup`, `ForeignHost`, or `Io`.
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
        // Open-or-create on a never-unlinked object: create races share
        // one flock (F3/L1). Never truncate: it could fail against a live
        // Windows `LockFileEx` range, turning contention into IO errors.
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
            // Held: contention, never a steal.
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
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_flock_before_scan"); // RACE-REVIEW
        // Holding the flock: pending recovery (or lookup failure) refuses first.
        match pending_journal_for_project(recovery_dir, project_path) {
            Ok(Some(journal)) => {
                unlock_attempt(&file);
                return Err(LockfileError::PendingRecovery { journal });
            }
            Ok(None) => {}
            Err(error) => {
                unlock_attempt(&file);
                return Err(LockfileError::RecoveryLookup {
                    reason: error.to_string(),
                });
            }
        }
        let previous = read_owner(&discovery_path);
        // F7: a KNOWN foreign host refuses; unknown-host claims predate
        // real hostnames and still reclaim.
        if let Some(refused) = previous
            .as_ref()
            .filter(|owner| owner.hostname != UNKNOWN_HOSTNAME && owner.hostname != claim.hostname)
        {
            drop(file);
            return Err(LockfileError::ForeignHost {
                host: refused.hostname.clone(),
                endpoint: refused.endpoint.clone(),
            });
        }
        claim.reclaimed_from.clone_from(&previous);
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_flock_before_publish");
        if let Err(error) = write_discovery(&discovery_path, &claim) {
            unlock_attempt(&file);
            return Err(LockfileError::Io(error.to_string()));
        }
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_publish_before_return"); // RACE-REVIEW
        return Ok(AcquiredLock {
            handle: LockfileHandle::held(lock_path, discovery_path, claim, file),
            reclaimed: previous,
        });
    }
    unreachable!("the loop above always returns");
}

// RACE-REVIEW: adversarial scenario module (Opus race review, 2026-09-25).
#[cfg(test)]
#[path = "aw1_race_tests.rs"]
mod race_tests;

#[cfg(test)]
mod tests {
    use std::{
        process::{Child, Command, Stdio},
        time::Instant,
    };

    use kinewright_media::test_support::TempDirectory;

    use crate::recovery::{allocate_journal_path, journal_file_name};

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
        let canonical = crate::project::canonical_project_identity(&project)
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
    /// sidecar — and nowhere for a pathless project. A relative spelling
    /// resolves against the working dir; an unresolvable parent falls back
    /// to the raw spelling (F4).
    #[test]
    fn lockfile_path_derives_from_the_stem() {
        assert_eq!(lockfile_path_for_project(None), None);
        assert_eq!(
            lockfile_path_for_project(Some(Path::new("/no/such/kinewright-dir/edit.kinewright"))),
            Some(PathBuf::from(
                "/no/such/kinewright-dir/edit.kinewright.lock"
            ))
        );
        assert_eq!(
            lockfile_path_for_project(Some(Path::new("edit.kinewright"))),
            Some(
                fs::canonicalize(".")
                    .expect("the working dir resolves")
                    .join("edit.kinewright.lock")
            )
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
                fs::write(signals.join("hostname"), &owner.handle.claim.hostname)
                    .expect("the hostname signal writes");
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
    /// `strip_host_env` removes the hostname variables from the child's
    /// environment, hermetically (F7).
    fn spawn_claimant(
        project: &Path,
        recovery: &Path,
        signals: &Path,
        hook: &str,
        strip_host_env: bool,
    ) -> Kid {
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
        if strip_host_env {
            command.env_remove("HOSTNAME").env_remove("COMPUTERNAME");
        }
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

    /// F3: release is an explicit unlock, never a last-close race — a
    /// forked child inherits the open description (spawned claimants do),
    /// and the clone models that duplicate without the fork's timing.
    #[test]
    fn f3_release_unlocks_past_a_forked_duplicate() {
        let dir = TempDirectory::new("aw1-f3-dup");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let first = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the first acquire lands");
        let inherited = first
            .handle
            .file
            .try_clone()
            .expect("the description duplicates");
        drop(first);
        acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the release frees the lock past a duplicate");
        drop(inherited);
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
            false,
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
            false,
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
            discovery_path_for_project(Some(Path::new("/no/such/kinewright-dir/edit.kinewright"))),
            Some(PathBuf::from(
                "/no/such/kinewright-dir/edit.kinewright.lock.json"
            ))
        );
        assert_eq!(
            discovery_path_for_project(Some(Path::new("edit.kinewright"))),
            Some(
                fs::canonicalize(".")
                    .expect("the working dir resolves")
                    .join("edit.kinewright.lock.json")
            )
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
        let mut child = spawn_claimant(&project, &recovery, &signals, "", false);
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
        let holder = spawn_claimant(
            &project,
            &recovery,
            &signals,
            "after_flock_before_publish",
            false,
        );
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

    /// F4/B5: a file symlink to a project in another directory shares the
    /// one lock — the alias contends, never double-owns.
    #[cfg(unix)]
    #[test]
    fn f4_symlink_alias_shares_one_lock() {
        let dir = TempDirectory::new("aw1-f4-alias");
        let real_dir = dir.path("real");
        let link_dir = dir.path("link");
        fs::create_dir(&real_dir).expect("the real dir creates");
        fs::create_dir(&link_dir).expect("the link dir creates");
        let real = real_dir.join("edit.kinewright");
        fs::write(&real, b"{}").expect("the project writes");
        let alias = link_dir.join("alias.kinewright");
        std::os::unix::fs::symlink(&real, &alias).expect("the alias links");
        assert_eq!(
            lockfile_path_for_project(Some(&alias)),
            lockfile_path_for_project(Some(&real)),
            "one lock for one file"
        );
        let recovery = recovery_dir(&dir);
        let _held = acquire_project_lock_with_policy(
            &real,
            LockMode::Gui,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the real path acquires");
        assert!(
            matches!(
                acquire_project_lock_with_policy(
                    &alias,
                    LockMode::Headless,
                    "http://127.0.0.1:10/mcp",
                    &recovery,
                    1,
                    Duration::ZERO
                ),
                Err(LockfileError::Contention { .. })
            ),
            "the alias contends"
        );
    }

    /// F4: two spellings of a not-yet-existing target share one lock, one
    /// claim identity, and one journal name.
    #[test]
    fn f4_first_save_spellings_share_one_identity() {
        let dir = TempDirectory::new("aw1-f4-first-save");
        let sub = dir.path("sub");
        fs::create_dir(&sub).expect("the sub dir creates");
        let direct = dir.path("new.kinewright");
        let crooked = sub.join("..").join("new.kinewright");
        assert!(!direct.exists(), "the target is not yet saved");
        assert_eq!(
            lockfile_path_for_project(Some(&crooked)),
            lockfile_path_for_project(Some(&direct)),
            "one lock for one target"
        );
        assert_eq!(
            journal_file_name(&crooked),
            journal_file_name(&direct),
            "one journal name for one target"
        );
        let recovery = recovery_dir(&dir);
        let first = acquire_project_lock_with_policy(
            &direct,
            LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the direct spelling acquires");
        let identity = first.handle.claim.project.clone();
        first.handle.release().expect("the release lands");
        let second = acquire_project_lock_with_policy(
            &crooked,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the crooked spelling acquires");
        assert_eq!(
            second.handle.claim.project, identity,
            "one claim identity for one target"
        );
        let _held = second;
        assert!(
            matches!(
                acquire_project_lock_with_policy(
                    &direct,
                    LockMode::Headless,
                    "http://127.0.0.1:11/mcp",
                    &recovery,
                    1,
                    Duration::ZERO
                ),
                Err(LockfileError::Contention { .. })
            ),
            "the spellings exclude each other"
        );
    }

    /// F5/L3 port: a pending journal refuses even with no lockfile at all —
    /// the check runs after the lock on every ownership path.
    #[test]
    fn f5_pending_recovery_without_lockfile_refuses() {
        let dir = TempDirectory::new("aw1-f5-l3");
        let project = dir.path("edit.kinewright");
        let recovery = recovery_dir(&dir);
        let journal = recovery.join(journal_file_name(&project));
        fs::write(&journal, b"pending crash data").expect("the pending journal writes");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:40123/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("a pending journal refuses without a lockfile");
        assert!(
            matches!(&error, LockfileError::PendingRecovery { journal: found } if found == &journal),
            "pending_recovery naming the journal, got {error}"
        );
    }

    /// F5/L4 port: a suffixed pending journal refuses too — the lookup
    /// covers every `allocate_journal_path` name for the identity.
    #[test]
    fn f5_pending_allocator_suffix_refuses() {
        let dir = TempDirectory::new("aw1-f5-l4");
        let project = dir.path("edit.kinewright");
        let recovery = recovery_dir(&dir);
        drop(
            acquire_project_lock_with_policy(
                &project,
                LockMode::Gui,
                "http://127.0.0.1:9/mcp",
                &recovery,
                1,
                Duration::ZERO,
            )
            .expect("the first acquire lands"),
        );
        let base = recovery.join(journal_file_name(&project));
        fs::write(&base, b"reserved").expect("the base journal writes");
        let suffixed = allocate_journal_path(&recovery, Some(&project), Path::new(""), &[&base]);
        assert_ne!(suffixed, base);
        fs::write(&suffixed, b"second pending crash").expect("the suffixed journal writes");
        fs::remove_file(&base).expect("only the suffix pends");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:40123/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("a suffixed pending journal refuses");
        assert!(
            matches!(&error, LockfileError::PendingRecovery { journal: found } if found == &suffixed),
            "pending_recovery naming the suffix, got {error}"
        );
    }

    /// F5/B6: a journal named by an alias spelling still refuses — the
    /// lookup reads the owning project from the journal header.
    #[test]
    fn f5_alias_named_journal_refuses() {
        let dir = TempDirectory::new("aw1-f5-alias");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        // Named as a pre-F4 alias spelling would name it: a different stem
        // and hash, so no filename pattern matches — only the header does.
        let alias = dir.path("elsewhere.kinewright");
        let journal = recovery.join(journal_file_name(&alias));
        assert_ne!(
            journal_file_name(&alias),
            journal_file_name(&project),
            "the alias name matches no pattern for the project"
        );
        let header = serde_json::json!({
            "format_version": 1,
            "project_path": project,
            "writer_format_version": 1,
            "initial_document": kinewright_core::Document::default(),
        });
        let mut bytes = crate::recovery::JOURNAL_MAGIC.to_vec();
        bytes.extend_from_slice(&serde_json::to_vec(&header).expect("the header serialises"));
        bytes.push(b'\n');
        fs::write(&journal, &bytes).expect("the alias-named journal writes");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:40123/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("an alias-named pending journal refuses");
        assert!(
            matches!(&error, LockfileError::PendingRecovery { journal: found } if found == &journal),
            "pending_recovery naming the journal, got {error}"
        );
    }

    /// F5: a failed pending-journal lookup refuses takeover closed — an
    /// unscannable recovery dir never reads as "nothing pends".
    #[test]
    fn f5_lookup_error_refuses_takeover_closed() {
        let dir = TempDirectory::new("aw1-f5-lookup-error");
        let project = dir.path("edit.kinewright");
        let recovery = dir.path("recovery");
        fs::write(&recovery, b"not a dir").expect("the recovery path is a file");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:40123/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("an unscannable recovery dir refuses");
        assert!(
            matches!(error, LockfileError::RecoveryLookup { .. }),
            "a typed fail-closed refusal, got {error}"
        );
    }

    /// F7/S1: the claim names the OS hostname even with no hostname
    /// variables in the environment (the child strips them hermetically,
    /// so parallel tests cannot interfere).
    #[cfg(unix)]
    #[test]
    fn f7_claim_names_the_os_hostname_without_env() {
        let dir = TempDirectory::new("aw1-f7-host");
        let project = dir.path("edit.kinewright");
        let recovery = recovery_dir(&dir);
        let signals = dir.path("child");
        let child = spawn_claimant(&project, &recovery, &signals, "", true);
        wait_for_signal(&signals.join("hostname"));
        let hostname = fs::read_to_string(signals.join("hostname")).expect("the hostname reads");
        fs::write(signals.join("done"), "").expect("the done writes");
        drop(child);
        assert_ne!(
            hostname, "unknown",
            "the OS names the host without env help"
        );
        assert!(!hostname.is_empty());
    }

    /// F7/S1: a stale claim from a known foreign host refuses takeover —
    /// a locally free lock proves nothing about a foreign owner.
    #[test]
    fn f7_foreign_host_claim_refuses_takeover() {
        let dir = TempDirectory::new("aw1-f7-foreign");
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
        .expect("the acquire lands");
        let discovery = first.handle.discovery.clone();
        drop(first); // The crash: a stale discovery, a free lock.
        // The stale owner was foreign: patch the hostname it published.
        let mut claim: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&discovery).expect("the discovery reads"))
                .expect("the discovery parses");
        claim["hostname"] = serde_json::Value::String("definitely-foreign-host.invalid".to_owned());
        fs::write(
            &discovery,
            serde_json::to_vec_pretty(&claim).expect("the claim serialises"),
        )
        .expect("the foreign discovery writes");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("a foreign-host stale lock refuses");
        assert!(
            matches!(&error, LockfileError::ForeignHost { host, .. } if host == "definitely-foreign-host.invalid"),
            "the refusal names the host, got {error}"
        );
    }

    /// F7: a stale claim with no host information (`unknown`, predating
    /// real hostnames) still reclaims — only a KNOWN foreign host refuses.
    #[test]
    fn f7_unknown_host_stale_claim_still_reclaims() {
        let dir = TempDirectory::new("aw1-f7-legacy");
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
        .expect("the acquire lands");
        let discovery = first.handle.discovery.clone();
        drop(first); // The crash: a stale discovery, a free lock.
        let mut claim: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&discovery).expect("the discovery reads"))
                .expect("the discovery parses");
        claim["hostname"] = serde_json::Value::String("unknown".to_owned());
        fs::write(
            &discovery,
            serde_json::to_vec_pretty(&claim).expect("the claim serialises"),
        )
        .expect("the legacy discovery writes");
        let got = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("an unknown-host stale lock reclaims");
        assert_eq!(
            got.reclaimed.as_ref().expect("the reclaim warns").hostname,
            "unknown"
        );
        got.handle.release().expect("the release lands");
    }
}
