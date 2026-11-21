//! Lead-owned, one-use file grant for the on-demand live Reviewer vision witness (JA-1 S5a C6).
//! Mirrors the repository's `te1_eval::grant` security invariants: fixed local-account home
//! verified against `/etc/passwd` + `HOME`, no-symlink handle traversal, private `0700`/`0600`
//! ownership, bounded expiry (<= 24 h), guardian SHA-256 verification, and atomic `O_EXCL`
//! single-use consumption before any native provider process can launch.
#![cfg(target_os = "linux")]

use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::orchestration::orchestration_records::{ProviderOptionRecord, ProviderSelectionRecord};

pub const REVIEW_LIVE_WITNESS_NAME: &str =
    "s5_c6_live_reviewer_identifies_decoded_only_discrepancy";
pub const REVIEW_LIVE_GRANT_ID_ENV: &str = "KINEWRIGHT_S5_C6_LIVE_GRANT_ID";
const MAX_GRANT_BYTES: usize = 64 * 1024;
const MAX_EXPIRY_SECS: u64 = 24 * 60 * 60;
const MAX_WALL_MILLIS: u64 = 15 * 60 * 1000;
const MAX_TOKENS_PER_TASK: u64 = 200_000;

fn err(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewLiveGrant {
    pub version: u32,
    pub id: String,
    pub witness: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub options: Vec<ProviderOptionRecord>,
    pub wall_millis: u64,
    pub tokens_per_task: u64,
    pub run_identity: String,
    pub expires_at_unix_seconds: u64,
    pub guardian_sha256: String,
}

impl ReviewLiveGrant {
    #[must_use]
    pub fn selection(&self) -> ProviderSelectionRecord {
        ProviderSelectionRecord {
            connector: self.provider.clone(),
            model: self.model.clone(),
            options: self.options.clone(),
        }
    }
}

fn id_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn check_directory(file: &File, private: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.mode() & 0o022 != 0
        || (private && meta.mode() & 0o077 != 0)
    {
        return Err(err(
            "S5_C6_NO_LEAD_GRANT: grant directory must be owned by this account and private",
        ));
    }
    Ok(())
}

fn child_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    Ok(File::from(openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}

fn opened_home(home: &Path) -> io::Result<File> {
    if !home.is_absolute() {
        return Err(err("S5_C6_NO_LEAD_GRANT: grant home must be absolute"));
    }
    let mut directory = File::open("/")?;
    for component in home.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => directory = child_directory(&directory, name)?,
            _ => return Err(err("S5_C6_NO_LEAD_GRANT: invalid grant home component")),
        }
    }
    check_directory(&directory, false)?;
    Ok(directory)
}

fn grants_directory(home: &Path) -> io::Result<File> {
    let mut directory = opened_home(home)?;
    for name in [".local", "share", "kinewright-eval", "grants"] {
        directory = child_directory(&directory, OsStr::new(name)).map_err(|e| {
            err(format!(
                "S5_C6_NO_LEAD_GRANT: grants directory unavailable ({e})"
            ))
        })?;
        check_directory(&directory, name == "grants")?;
    }
    Ok(directory)
}

fn checked_file(file: File) -> io::Result<File> {
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.mode() & 0o077 != 0
    {
        return Err(err(
            "S5_C6_NO_LEAD_GRANT: grant must be a private owned regular file",
        ));
    }
    Ok(file)
}

fn open_relative(directory: &File, name: &str, create: bool) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    let access = if create {
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL
    } else {
        OFlags::RDONLY
    };
    checked_file(File::from(openat(
        directory,
        name,
        access | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?))
}

fn unix_seconds() -> io::Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| err(e.to_string()))?
        .as_secs())
}

fn check_expiry(expires: u64) -> io::Result<()> {
    let now = unix_seconds()?;
    if expires <= now {
        return Err(err("S5_C6_NO_LEAD_GRANT: lead grant expired"));
    }
    if expires.saturating_sub(now) > MAX_EXPIRY_SECS {
        return Err(err(
            "S5_C6_NO_LEAD_GRANT: lead grant expiry exceeds 24 hours",
        ));
    }
    Ok(())
}

fn account_home() -> io::Result<PathBuf> {
    let mut bytes = Vec::new();
    File::open("/etc/passwd")?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        return Err(err("S5_C6_NO_LEAD_GRANT: /etc/passwd exceeds cap"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|e| err(e.to_string()))?;
    let uid = rustix::process::getuid().as_raw();
    let mut homes = text.lines().filter_map(|line| {
        let fields: Vec<_> = line.split(':').collect();
        (fields.len() == 7 && fields[2].parse::<u32>() == Ok(uid)).then(|| PathBuf::from(fields[5]))
    });
    let home = homes
        .next()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| err("S5_C6_NO_LEAD_GRANT: local account home unavailable"))?;
    if homes.next().is_some() {
        return Err(err("S5_C6_NO_LEAD_GRANT: ambiguous local account home"));
    }
    Ok(home)
}

fn validate_grant(grant: &ReviewLiveGrant, id: &str, guardian_sha256: &str) -> io::Result<()> {
    if grant.version != 1
        || grant.id != id
        || grant.witness != REVIEW_LIVE_WITNESS_NAME
        || !matches!(grant.provider.as_str(), "codex" | "claude")
        || grant.model.trim().is_empty()
        || grant.wall_millis == 0
        || grant.wall_millis > MAX_WALL_MILLIS
        || grant.tokens_per_task == 0
        || grant.tokens_per_task > MAX_TOKENS_PER_TASK
        || grant.run_identity.len() != 32
        || !grant.run_identity.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(err("S5_C6_NO_LEAD_GRANT: grant fields failed validation"));
    }
    if grant.guardian_sha256 != guardian_sha256 {
        return Err(err(
            "S5_C6_NO_LEAD_GRANT: guardian SHA-256 differs from lead grant",
        ));
    }
    check_expiry(grant.expires_at_unix_seconds)?;
    Ok(())
}

/// Atomically validate and consume a one-use lead grant file inside an opened private `grants`
/// directory handle. Refuses missing, expired, replayed, symlinked, or modified grants.
pub(crate) fn consume_from_dir(
    folder: &File,
    id: &str,
    guardian_sha256: &str,
) -> io::Result<ReviewLiveGrant> {
    if !id_valid(id) {
        return Err(err("S5_C6_NO_LEAD_GRANT: missing or invalid lead grant id"));
    }
    check_directory(folder, true)?;
    let locking = match open_relative(folder, ".consume-lock", true) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            open_relative(folder, ".consume-lock", false)?
        }
        Err(e) => return Err(e),
    };
    locking.lock()?;

    let mut file = open_relative(folder, &format!("{id}.json"), false)
        .map_err(|e| err(format!("S5_C6_NO_LEAD_GRANT: grant file unavailable ({e})")))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_GRANT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_GRANT_BYTES {
        return Err(err("S5_C6_NO_LEAD_GRANT: grant exceeds byte cap"));
    }
    let grant: ReviewLiveGrant = serde_json::from_slice(&bytes)
        .map_err(|e| err(format!("S5_C6_NO_LEAD_GRANT: invalid grant JSON ({e})")))?;
    validate_grant(&grant, id, guardian_sha256)?;

    // Re-read and verify path/fd identity before durable consumption.
    file.seek(SeekFrom::Start(0))?;
    let mut again = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_GRANT_BYTES as u64 + 1)
        .read_to_end(&mut again)?;
    let path_meta = rustix::fs::statat(
        folder,
        format!("{id}.json"),
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )?;
    let fd_meta = file.metadata()?;
    if again != bytes
        || rustix::fs::FileType::from_raw_mode(path_meta.st_mode)
            != rustix::fs::FileType::RegularFile
        || path_meta.st_dev != fd_meta.dev()
        || path_meta.st_ino != fd_meta.ino()
        || fd_meta.nlink() != 1
    {
        return Err(err("S5_C6_NO_LEAD_GRANT: grant changed before consumption"));
    }
    check_expiry(grant.expires_at_unix_seconds)?;

    match rustix::fs::mkdirat(folder, "consumed", rustix::fs::Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(e) => return Err(e.into()),
    }
    let consumed = child_directory(folder, OsStr::new("consumed"))?;
    check_directory(&consumed, true)?;
    for name in [
        format!("id-{id}.json"),
        format!("run-{}.json", grant.run_identity),
    ] {
        let mut receipt = open_relative(&consumed, &name, true)
            .map_err(|e| err(format!("S5_C6_NO_LEAD_GRANT: grant already consumed ({e})")))?;
        receipt.write_all(&bytes)?;
        receipt.sync_all()?;
    }
    consumed.sync_all()?;
    folder.sync_all()?;
    Ok(grant)
}

/// Consume the per-run lead-owned grant file for the live Reviewer vision witness from the
/// local account's fixed `~/.local/share/kinewright-eval/grants/` directory. An environment
/// variable alone never authorizes execution.
pub(crate) fn consume_from_account_home(guardian: &Path) -> io::Result<ReviewLiveGrant> {
    let id = std::env::var(REVIEW_LIVE_GRANT_ID_ENV).map_err(|_| {
        err("S5_C6_NO_LEAD_GRANT: a lead-written private grant file is required; env vars alone never grant a paid run")
    })?;
    if !id_valid(&id) {
        return Err(err("S5_C6_NO_LEAD_GRANT: invalid grant id"));
    }
    let home = account_home()?;
    if std::env::var_os("HOME").as_deref().map(Path::new) != Some(home.as_path()) {
        return Err(err(
            "S5_C6_NO_LEAD_GRANT: HOME differs from local account home",
        ));
    }
    let guardian = guardian.canonicalize()?;
    let guardian_sha256 =
        kinewright_media::sha256_file(&guardian).map_err(|e| err(e.to_string()))?;
    let dir = grants_directory(&home)?;
    consume_from_dir(&dir, &id, &guardian_sha256)
}

/// Helper used by tests to open a private temporary directory with handle checks.
#[cfg(test)]
pub(crate) fn open_private_dir_for_test(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).custom_flags(
        (rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC)
            .bits()
            .cast_signed(),
    );
    opts.open(path)
}
