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
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};

use crate::recovery::{fnv1a_64, pending_journal_for_project};

/// The lockfile suffix: `<stem>.kinewright.lock` beside the project.
pub const LOCKFILE_SUFFIX: &str = "kinewright.lock";

/// The lockfile format version this build writes.
pub const LOCKFILE_FORMAT_VERSION: u32 = 1;

/// Acquire attempts before contention (AW1 §5: 3 × 250 ms). Retries only
/// re-probe after a racing local contender: flock release is immediate,
/// so a dying owner never needs a wait — spurious contention comes only
/// from two contenders racing one try (one verdict each). Never a steal.
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
    /// The owner's claim second: `release` only removes a discovery still
    /// naming this handle (G9). Defaulted so pre-G9 claims still parse.
    #[serde(default)]
    pub started_at_unix: u64,
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

/// Hostname equality (G11/AF5): case-insensitive after trimming a
/// trailing dot — `HOST.` and `host` name one machine; anything else
/// (notably an FQDN) stays distinct and fail-closed.
fn hostnames_equal(first: &str, second: &str) -> bool {
    fn normalise(host: &str) -> String {
        host.trim_end_matches('.').to_ascii_lowercase()
    }
    normalise(first) == normalise(second)
}

/// This machine's hostname: OS name, env, [`UNKNOWN_HOSTNAME`].
fn current_hostname() -> String {
    let os = gethostname::gethostname();
    current_hostname_with(
        Some(os.as_os_str()),
        std::env::var_os("HOSTNAME").as_deref(),
        std::env::var_os("COMPUTERNAME").as_deref(),
    )
}

/// Hostname resolution with its inputs injected (G11/R2-S1): the OS name,
/// then `HOSTNAME`, then `COMPUTERNAME`, then [`UNKNOWN_HOSTNAME`]. Blank
/// and non-UTF-8 inputs fall through. Pure, so the fallback-order test
/// needs no interposition.
fn current_hostname_with(
    os: Option<&std::ffi::OsStr>,
    hostname: Option<&std::ffi::OsStr>,
    computername: Option<&std::ffi::OsStr>,
) -> String {
    fn clean(value: &std::ffi::OsStr) -> Option<&str> {
        value
            .to_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
    [os, hostname, computername]
        .into_iter()
        .flatten()
        .find_map(clean)
        .unwrap_or(UNKNOWN_HOSTNAME)
        .to_owned()
}

/// Why a project lock acquire failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockfileError {
    /// The lockfile exists and its flock is held. `owner` is the holding
    /// triple when its claim parses (the full claim overflows
    /// `result_large_err`; the refusal only names the triple).
    /// Best-effort: an unreadable discovery contends ownerless, and a
    /// parsed triple is still only an unauthenticated advertisement —
    /// S3's proxy must keep §5's authenticated `initialize` liveness
    /// check before trusting an endpoint.
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
        /// The discovery to delete if this machine was renamed (G11).
        discovery: PathBuf,
    },
    /// The lock object was deleted under this live handle (G9): the fd
    /// still holds its flock, but the path names another (or no) object —
    /// not writing. Only the `.lock.json` discovery may be hand-deleted.
    LockLost {
        path: PathBuf,
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
                }?;
                write!(
                    formatter,
                    "; only the .lock.json discovery may be hand-deleted, never the .lock object"
                )
            }
            Self::PendingRecovery { journal } => write!(
                formatter,
                "not taking over: a recovery journal is pending ({}) — \
                 restore or discard it in the GUI first",
                journal.display()
            ),
            Self::RecoveryLookup { reason } => write!(formatter, "recovery scan failed: {reason}"),
            Self::ForeignHost {
                host,
                endpoint: _,
                discovery,
            } => write!(
                formatter,
                "foreign host {host}; if this is this machine renamed, delete {} and retry",
                discovery.display()
            ),
            Self::LockLost { path } => write!(
                formatter,
                "the lock {} was deleted under this live owner; not writing",
                path.display()
            ),
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

    /// Confirm the handle's fd still names the lock object (G9): Unix
    /// compares the fd's `(dev, ino)` with the path's; a mismatch — or a
    /// vanished path — is `LockLost`: the object was deleted under this
    /// live owner, so no write may follow. Call before every headless save
    /// and every discovery re-publish.
    /// # Errors
    /// Returns `LockLost` when the object no longer names this handle.
    #[cfg(unix)]
    pub fn verify(&self) -> Result<(), LockfileError> {
        use std::os::unix::fs::MetadataExt as _;
        let live = match (self.file.metadata(), fs::metadata(&self.path)) {
            (Ok(opened), Ok(current)) => {
                opened.dev() == current.dev() && opened.ino() == current.ino()
            }
            _ => false,
        };
        if live {
            Ok(())
        } else {
            Err(LockfileError::LockLost {
                path: self.path.clone(),
            })
        }
    }

    /// Non-Unix `verify` always passes: `std` opens with
    /// `FILE_SHARE_DELETE`, so a delete goes delete-pending and blocks
    /// re-create while the handle lives — the object cannot be swapped
    /// under a live owner.
    /// # Errors
    /// Returns nothing: the non-Unix object cannot be swapped.
    #[cfg(not(unix))]
    pub fn verify(&self) -> Result<(), LockfileError> {
        Ok(())
    }

    /// Release: remove the discovery while holding the flock, then drop —
    /// the drop unlocks explicitly, so the release lands even when a
    /// forked child still holds a duplicate of the open description.
    /// The discovery is removed only if it still names this handle (pid,
    /// claim second, endpoint — G9); otherwise it is left alone and the
    /// skip is logged — another owner may have published after an
    /// external unlink. G10: release ends this handle's writer right — a
    /// journal for the identity may be created or renamed only under its
    /// lock, so the removal lands under the held flock and any later
    /// journal write must re-acquire first.
    /// # Errors
    /// Returns the removal IO error, if any.
    pub fn release(self) -> io::Result<()> {
        let mine = match read_owner(&self.discovery, &self.claim.hostname) {
            DiscoveryRead::Owner(owner) => {
                owner.pid == self.claim.pid
                    && owner.started_at_unix == self.claim.started_at_unix
                    && owner.endpoint == self.claim.endpoint
            }
            DiscoveryRead::Absent => true,
            _ => false,
        };
        let removed = if mine {
            fs::remove_file(&self.discovery)
        } else {
            eprintln!(
                "kinewright: not removing {}: it no longer names this owner",
                self.discovery.display()
            );
            Ok(())
        };
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
    /// An unreadable previous discovery reclaims a pid-0/`unknown` sentinel
    /// with [`AcquiredLock::reclaimed_unreadable`] set — warn via
    /// [`reclaim_warning_json_unreadable`] instead.
    pub reclaimed: Option<ReclaimedOwner>,
    /// The reclaimed discovery existed but was unreadable (G7).
    pub reclaimed_unreadable: bool,
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

/// The typed JSON warning an unreadable-discovery reclaim owes (G7).
#[must_use]
pub fn reclaim_warning_json_unreadable() -> String {
    serde_json::json!({
        "code": "lock_reclaimed",
        "previous_unreadable": true,
    })
    .to_string()
}

/// The most a discovery read takes: claims are small JSON; anything
/// larger (or a link to `/dev/zero`) reads as unreadable, never unbounded.
const DISCOVERY_READ_LIMIT: u64 = 64 * 1024;

/// What the discovery read found (G7): absent splits from present-but-
/// unreadable, and a lenient pass still catches a known foreign hostname
/// inside a claim this build cannot parse (AF5 during version skew).
#[derive(Debug)]
enum DiscoveryRead {
    /// No discovery file: a fresh acquire, no warning owed.
    Absent,
    /// A parsed claim's triple.
    Owner(ReclaimedOwner),
    /// Unparseable, but the lenient pass found a foreign hostname string.
    Foreign { host: String, endpoint: String },
    /// Present but unreadable (a special file, a read error, over-limit, or
    /// unparseable with no usable hostname): reclaims WITH a warning.
    Unreadable,
}

/// Triple at a discovery path; legal while held (B4). Only a regular file
/// reads, bounded (G1): a FIFO, device, directory, or symlink is
/// unreadable, and the read never blocks on a FIFO's open nor overruns on
/// a huge file.
fn read_owner(discovery_path: &Path, own_hostname: &str) -> DiscoveryRead {
    use std::io::Read as _;
    match fs::symlink_metadata(discovery_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return DiscoveryRead::Absent,
        Err(_) => return DiscoveryRead::Unreadable,
        Ok(meta) if !meta.file_type().is_file() => return DiscoveryRead::Unreadable,
        Ok(_) => {}
    }
    let file = match fs::File::open(discovery_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return DiscoveryRead::Absent,
        Err(_) => return DiscoveryRead::Unreadable,
    };
    let mut bytes = Vec::new();
    if file
        .take(DISCOVERY_READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > DISCOVERY_READ_LIMIT
    {
        return DiscoveryRead::Unreadable;
    }
    if let Ok(claim) = serde_json::from_slice::<LockfileClaim>(&bytes) {
        return DiscoveryRead::Owner(ReclaimedOwner {
            pid: claim.pid,
            hostname: claim.hostname,
            endpoint: claim.endpoint,
            started_at_unix: claim.started_at_unix,
        });
    }
    // Lenient (G7/AF5): a hostname string that is neither this host nor
    // `unknown` still refuses, however unparseable the rest of the claim.
    if let Some(value) = serde_json::from_slice::<serde_json::Value>(&bytes).ok()
        && let Some(host) = value.get("hostname").and_then(serde_json::Value::as_str)
        && !hostnames_equal(host, UNKNOWN_HOSTNAME)
        && !hostnames_equal(host, own_hostname)
    {
        let endpoint = value
            .get("endpoint")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return DiscoveryRead::Foreign {
            host: host.to_owned(),
            endpoint: endpoint.to_owned(),
        };
    }
    DiscoveryRead::Unreadable
}

/// Release an acquire attempt's flock before its close: explicit
/// unlock, then the caller drops. A bare close releases only at the
/// last close of the open description — see [`LockfileHandle`]'s [`Drop`].
fn unlock_attempt(file: &File) {
    let _ = file.unlock();
}

/// A held flock that unlocks explicitly on every exit (G2): taken
/// immediately after a successful `try_lock_exclusive`, its drop runs
/// `unlock_attempt` before the close — so every post-flock refusal unlocks
/// past forked duplicates by construction, with no per-arm calls to forget.
/// The success path hands the live flock to [`LockfileHandle`] via
/// `release`, which skips the unlock (the handle owns the live lock; its
/// own drop unlocks).
struct FlockGuard {
    file: Option<File>,
}

impl FlockGuard {
    fn held(file: File) -> Self {
        Self { file: Some(file) }
    }

    /// Hand the live flock to its handle without unlocking.
    fn release(mut self) -> File {
        self.file.take().expect("a held flock releases once")
    }

    /// The held file, for pre-publish liveness checks (G9; Unix only —
    /// the sole caller is the Unix fd-vs-path check).
    #[cfg(unix)]
    fn file(&self) -> &File {
        self.file.as_ref().expect("a held flock")
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            unlock_attempt(&file);
        }
    }
}

/// The discovery temp nonce: `.<file>.<pid>.<nonce>.tmp` is unique per
/// process, and `create_new` retries past stale siblings anyway.
static DISCOVERY_TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Publish a claim through the dedicated symlink-safe writer (G1) — never
/// `write_file_atomic`, which canonicalises (writing through a planted link)
/// and falls back to symlink-following `fs::write`. Temp beside the
/// UNRESOLVED path with `create_new` (a planted temp link is never
/// followed); `write_all` + `sync_all` through the one handle, closed (the
/// rename cannot replace an open file on Windows); `rename` over the
/// discovery, replacing a planted link itself, never its target. No fallback
/// of any kind: any failure removes the temp and reports `Io`. Call only
/// while holding the lock — the flock serialises publishers.
fn write_discovery(discovery_path: &Path, claim: &LockfileClaim) -> io::Result<()> {
    write_discovery_with_rename(discovery_path, claim, None)
}

/// Decide a free lock's claim from its discovery (G7): a known foreign host
/// refuses (strict or lenient); an unreadable stale discovery reclaims a
/// pid-0/`unknown` sentinel flagged unreadable; absence is fresh. F7:
/// unknown-host claims predate real hostnames and still reclaim.
fn decide_free_claim(
    discovery_path: &Path,
    own_hostname: &str,
) -> Result<(Option<ReclaimedOwner>, bool), LockfileError> {
    match read_owner(discovery_path, own_hostname) {
        DiscoveryRead::Absent => Ok((None, false)),
        DiscoveryRead::Owner(owner)
            if !hostnames_equal(&owner.hostname, UNKNOWN_HOSTNAME)
                && !hostnames_equal(&owner.hostname, own_hostname) =>
        {
            Err(LockfileError::ForeignHost {
                host: owner.hostname,
                endpoint: owner.endpoint,
                discovery: discovery_path.to_path_buf(),
            })
        }
        DiscoveryRead::Owner(owner) => Ok((Some(owner), false)),
        DiscoveryRead::Foreign { host, endpoint } => Err(LockfileError::ForeignHost {
            host,
            endpoint,
            discovery: discovery_path.to_path_buf(),
        }),
        DiscoveryRead::Unreadable => Ok((
            Some(ReclaimedOwner {
                pid: 0,
                hostname: UNKNOWN_HOSTNAME.to_owned(),
                endpoint: String::new(),
                started_at_unix: 0,
            }),
            true,
        )),
    }
}

/// [`write_discovery`] with the rename injected: `None` renames for real.
/// The seam exists so the refused-rename pin has a portable test — no
/// portable fixture fails a real rename.
fn write_discovery_with_rename(
    discovery_path: &Path,
    claim: &LockfileClaim,
    rename: Option<fn(&Path, &Path) -> io::Result<()>>,
) -> io::Result<()> {
    use std::io::Write as _;
    let bytes = serde_json::to_vec_pretty(claim)
        .map_err(|error| io::Error::other(format!("could not serialize the lockfile: {error}")))?;
    let Some(name) = discovery_path.file_name() else {
        return Err(io::Error::other(format!(
            "{} names no file",
            discovery_path.display()
        )));
    };
    let parent = discovery_path.parent().unwrap_or_else(|| Path::new(""));
    for _ in 0..100 {
        let temp = parent.join(format!(
            ".{}.{}.{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            DISCOVERY_TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let mut file = file;
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        drop(file);
        let renamed = match rename {
            Some(hook) => hook(&temp, discovery_path),
            None => fs::rename(&temp, discovery_path),
        };
        if let Err(error) = renamed {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        return Ok(());
    }
    Err(io::Error::other(format!(
        "could not pick a temp beside {}",
        discovery_path.display()
    )))
}

/// Sweep this writer's own stale temps (`write_discovery`'s `.<file>.*.tmp`
/// siblings, left by a kill between temp-write and rename). Best-effort —
/// never fails the acquire — and strictly patterned, so nothing else is
/// touched. Call while holding the flock.
fn sweep_discovery_temps(discovery_path: &Path) {
    let Some(name) = discovery_path.file_name() else {
        return;
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let parent = discovery_path.parent().unwrap_or_else(|| Path::new("."));
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.starts_with(&prefix) && stem.as_bytes().ends_with(b".tmp"))
        {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Whether the opened lock fd still names the lock path (Unix): the fd's
/// `(dev, ino)` must equal the path's, and the path must not have become a
/// symlink — closing the swap window between open and flock.
#[cfg(unix)]
fn fd_matches_path(file: &File, lock_path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let (Ok(opened), Ok(current)) = (file.metadata(), fs::symlink_metadata(lock_path)) else {
        return false;
    };
    !current.file_type().is_symlink()
        && opened.dev() == current.dev()
        && opened.ino() == current.ino()
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
        // G1: never open through a symlink — a dangling lock link must
        // create nothing, and a planted link must not divert the lock.
        if fs::symlink_metadata(&lock_path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(LockfileError::Io(format!(
                "refusing to lock through a symlink: {}",
                lock_path.display()
            )));
        }
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
        // G1 (Unix): the fd must still name the lock path — a swap between
        // open and flock retries onto the current object.
        #[cfg(unix)]
        if !fd_matches_path(&file, &lock_path) {
            drop(file);
            if last {
                return Err(LockfileError::Io(format!(
                    "the lock object {} changed under its open",
                    lock_path.display()
                )));
            }
            std::thread::sleep(retry_delay);
            continue;
        }
        if file.try_lock_exclusive().is_err() {
            // Held: contention, never a steal. Only a parsed claim names
            // its owner; anything unreadable contends ownerless.
            drop(file);
            let owner = match read_owner(&discovery_path, &claim.hostname) {
                DiscoveryRead::Owner(owner) => Some(owner),
                _ => None,
            };
            if last {
                return Err(LockfileError::Contention {
                    path: project_path.to_path_buf(),
                    owner,
                });
            }
            std::thread::sleep(retry_delay);
            continue;
        }
        // Holding the flock: every exit below goes through the guard.
        let guard = FlockGuard::held(file);
        // Holding the flock: sweep our own stale publish temps first.
        sweep_discovery_temps(&discovery_path);
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_flock_before_scan"); // RACE-REVIEW
        // Holding the flock: pending recovery (or lookup failure) refuses first.
        match pending_journal_for_project(recovery_dir, project_path) {
            Ok(Some(journal)) => {
                return Err(LockfileError::PendingRecovery { journal });
            }
            Ok(None) => {}
            Err(error) => {
                return Err(LockfileError::RecoveryLookup {
                    reason: error.to_string(),
                });
            }
        }
        let (previous, unreadable) = decide_free_claim(&discovery_path, &claim.hostname)?;
        claim.reclaimed_from.clone_from(&previous);
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_flock_before_publish");
        // G9: the object may have been deleted under this pending owner
        // between open and publish — never publish for a stale inode.
        #[cfg(unix)]
        if !fd_matches_path(guard.file(), &lock_path) {
            return Err(LockfileError::LockLost { path: lock_path });
        }
        if let Err(error) = write_discovery(&discovery_path, &claim) {
            return Err(LockfileError::Io(error.to_string()));
        }
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("after_publish_before_return"); // RACE-REVIEW
        return Ok(AcquiredLock {
            handle: LockfileHandle::held(lock_path, discovery_path, claim, guard.release()),
            reclaimed: previous,
            reclaimed_unreadable: unreadable,
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

    /// G1: a dangling symlink at the lock object refuses with typed `Io` —
    /// the open creates nothing through the link, and the link is untouched.
    #[cfg(unix)]
    #[test]
    fn g1_dangling_lock_symlink_creates_nothing() {
        let dir = TempDirectory::new("aw1-g1-dangling-lock");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let lock = lockfile_path_for_project(Some(&project)).expect("a lockfile derives");
        let target = dir.path("nowhere.txt");
        std::os::unix::fs::symlink(&target, &lock).expect("the dangling link plants");
        let error = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect_err("a lock symlink refuses");
        assert!(
            matches!(error, LockfileError::Io(_)),
            "typed Io, got {error:?}"
        );
        assert!(!target.exists(), "nothing is created through the link");
        assert!(
            fs::symlink_metadata(&lock)
                .expect("the link reads")
                .file_type()
                .is_symlink(),
            "the link itself is untouched"
        );
    }

    /// G1/RS1: a refused publish renames nothing and falls back to nothing —
    /// the prior discovery keeps its bytes and no temp litters the dir. The
    /// refusal is injected: no portable fixture fails a real rename.
    #[test]
    fn g1_refused_publish_preserves_the_prior_discovery() {
        let dir = TempDirectory::new("aw1-g1-refused-publish");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let (_, discovery, claim) =
            build_claim(&project, LockMode::Gui, "http://127.0.0.1:9/mcp", None);
        let prior = b"prior discovery bytes";
        fs::write(&discovery, prior).expect("the prior discovery writes");
        let refused = |_: &Path, _: &Path| -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected sharing violation",
            ))
        };
        let error = write_discovery_with_rename(&discovery, &claim, Some(refused))
            .expect_err("the refused rename reports");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            fs::read(&discovery).expect("the discovery re-reads"),
            prior,
            "the prior discovery keeps its bytes"
        );
        let torn = |_: &Path, _: &Path| -> io::Result<()> {
            Err(io::Error::other("injected unsplittable failure"))
        };
        assert!(
            write_discovery_with_rename(&discovery, &claim, Some(torn)).is_err(),
            "a non-permission refusal still reports"
        );
        assert_eq!(
            fs::read(&discovery).expect("the discovery re-reads"),
            prior,
            "and still preserves"
        );
        for entry in fs::read_dir(discovery.parent().expect("a parent")).expect("the dir reads") {
            let entry = entry.expect("a readable entry");
            assert!(
                !entry.file_name().to_string_lossy().starts_with(&format!(
                    ".{}",
                    discovery.file_name().expect("a name").to_string_lossy()
                )),
                "no temp litter: {}",
                entry.file_name().to_string_lossy()
            );
        }
    }

    /// G1/N7: the acquire sweeps this writer's own stale publish temps —
    /// and nothing else.
    #[test]
    fn g1_acquire_sweeps_only_its_own_stale_temps() {
        let dir = TempDirectory::new("aw1-g1-sweep");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let discovery = discovery_path_for_project(Some(&project)).expect("a discovery derives");
        let parent = discovery.parent().expect("a parent");
        let name = discovery
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        let stale = parent.join(format!(".{name}.12345.7.tmp"));
        fs::write(&stale, b"stale temp").expect("the stale temp plants");
        let stranger = parent.join(".something-else.tmp");
        fs::write(&stranger, b"not ours").expect("the stranger plants");
        let acquired = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the acquire lands");
        assert!(!stale.exists(), "our stale temp is swept");
        assert!(stranger.exists(), "nothing else is touched");
        acquired.handle.release().expect("the release lands");
    }

    /// G11/R2-S1: hostname fallback order — OS, then `HOSTNAME`, then
    /// `COMPUTERNAME`, then `unknown`; blank and non-UTF-8 fall through.
    /// Kills R2's M18 (the env order swap).
    #[test]
    fn g11_hostname_fallback_order() {
        use std::ffi::OsStr;
        fn name(value: &str) -> &OsStr {
            OsStr::new(value)
        }
        assert_eq!(
            current_hostname_with(Some(name("os")), Some(name("h")), Some(name("c"))),
            "os"
        );
        assert_eq!(
            current_hostname_with(None, Some(name("h")), Some(name("c"))),
            "h"
        );
        assert_eq!(current_hostname_with(None, None, Some(name("c"))), "c");
        assert_eq!(current_hostname_with(None, None, None), "unknown");
        assert_eq!(
            current_hostname_with(Some(name("  ")), Some(name("h")), None),
            "h"
        );
        assert_eq!(
            current_hostname_with(None, Some(name("")), Some(name("c"))),
            "c"
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            assert_eq!(
                current_hostname_with(None, Some(OsStr::from_bytes(b"\xff")), Some(name("c"))),
                "c"
            );
        }
    }

    /// G9: the fd-vs-path check detects a swapped or vanished object —
    /// the helper behind the open-time, pre-publish, and `verify` gates.
    #[cfg(unix)]
    #[test]
    fn g9_fd_check_detects_a_swapped_object() {
        let dir = TempDirectory::new("aw1-g9-fd-check");
        let lock = dir.path("edit.kinewright.lock");
        let file = fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)
            .expect("the lock opens");
        assert!(fd_matches_path(&file, &lock), "a live fd names its path");
        let stash = dir.path("edit.kinewright.lock.stashed");
        fs::rename(&lock, &stash).expect("the object moves aside");
        fs::write(&lock, b"").expect("a new object takes the path");
        assert!(
            !fd_matches_path(&file, &lock),
            "a swapped object no longer matches"
        );
        fs::remove_file(&lock).expect("the path vanishes");
        assert!(!fd_matches_path(&file, &lock), "a vanished path mismatches");
    }

    /// G7: an unreadable stale discovery with a free lock reclaims WITH the
    /// typed warning — the sentinel triple plus the `previous_unreadable`
    /// flag — while a missing discovery reclaims silently.
    #[test]
    fn g7_unreadable_reclaim_warns_typed() {
        let dir = TempDirectory::new("aw1-g7-unreadable");
        let project = dir.path("edit.kinewright");
        fs::write(&project, b"{}").expect("the project writes");
        let recovery = recovery_dir(&dir);
        let fresh = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:9/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the fresh acquire lands");
        assert!(
            fresh.reclaimed.is_none() && !fresh.reclaimed_unreadable,
            "absence reclaims silently"
        );
        let discovery = fresh.handle.discovery.clone();
        drop(fresh); // The crash: a stale discovery, a free lock.
        fs::write(&discovery, b"{ torn").expect("the discovery tears");
        let got = acquire_project_lock_with_policy(
            &project,
            LockMode::Headless,
            "http://127.0.0.1:10/mcp",
            &recovery,
            1,
            Duration::ZERO,
        )
        .expect("the free lock reclaims");
        let previous = got.reclaimed.as_ref().expect("the reclaim warns");
        assert_eq!(previous.pid, 0, "the sentinel triple");
        assert!(got.reclaimed_unreadable, "the unreadable flag");
        let warning: serde_json::Value =
            serde_json::from_str(&reclaim_warning_json_unreadable()).expect("the warning is JSON");
        assert_eq!(warning["code"], "lock_reclaimed");
        assert_eq!(warning["previous_unreadable"], true);
        got.handle.release().expect("the release lands");
    }
}
