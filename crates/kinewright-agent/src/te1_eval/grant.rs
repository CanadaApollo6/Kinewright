//! Lead-owned, one-use approval. No native work precedes the durable receipt.
use super::error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub version: u32,
    pub id: String,
    pub pin_sha256: String,
    pub provider: String,
    pub case: u8,
    pub arm: String,
    pub models_and_options: Value,
    pub limits: Value,
    pub run_identity: String,
    pub expires_at_unix_seconds: u64,
    pub dry_pass: Value,
}
pub(crate) struct ConsumedGrant {
    grant: Grant,
    authority: LaunchAuthority,
}
enum LaunchAuthority {
    Unpaid,
    Native { guardian: PathBuf, home: PathBuf },
}
impl ConsumedGrant {
    pub(crate) fn guardian(&self) -> io::Result<&Path> {
        match &self.authority {
            LaunchAuthority::Native { guardian, .. } => Ok(guardian),
            LaunchAuthority::Unpaid => Err(error(
                "temporary grant receipts cannot authorise native execution",
            )),
        }
    }
    pub(crate) fn verify_guardian(&self, guardian: &Path) -> io::Result<()> {
        verify_guardian_hash(&self.grant, guardian)
    }
    pub(crate) fn run_root(&self) -> io::Result<PathBuf> {
        match &self.authority {
            LaunchAuthority::Native { home, .. } => {
                Ok(home.join(".local/share/kinewright-eval/runs"))
            }
            LaunchAuthority::Unpaid => Err(error("temporary receipts have no paid output root")),
        }
    }
    pub(crate) fn wall_duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(
            self.grant.limits["wall_millis"]
                .as_u64()
                .expect("validated grant wall"),
        )
    }
    pub(crate) fn grant(&self) -> &Grant {
        &self.grant
    }
    pub(crate) fn selection(
        &self,
        role: &str,
    ) -> io::Result<crate::orchestration::orchestration_records::ProviderSelectionRecord> {
        let value = &self.grant.models_and_options[role];
        Ok(
            crate::orchestration::orchestration_records::ProviderSelectionRecord {
                connector: self.grant.provider.clone(),
                model: value["model"]
                    .as_str()
                    .ok_or_else(|| error("grant model missing"))?
                    .into(),
                options: serde_json::from_value(value["options"].clone()).map_err(error)?,
            },
        )
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
        return Err(error(
            "grant directory must be owned by this account and private where required",
        ));
    }
    Ok(())
}
fn child_directory(parent: &File, name: &std::ffi::OsStr) -> io::Result<File> {
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
        return Err(error("grant home must be absolute"));
    }
    let mut directory = File::open("/")?;
    for component in home.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => directory = child_directory(&directory, name)?,
            _ => return Err(error("invalid grant home component")),
        }
    }
    check_directory(&directory, false)?;
    Ok(directory)
}
fn grants_directory(home: &Path) -> io::Result<File> {
    let mut directory = opened_home(home)?;
    for name in [".local", "share", "kinewright-eval", "grants"] {
        directory = child_directory(&directory, std::ffi::OsStr::new(name))?;
        check_directory(&directory, name == "grants")?;
    }
    Ok(directory)
}
fn unix_seconds() -> io::Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(error)?
        .as_secs())
}
fn check_expiry(expires: u64) -> io::Result<()> {
    let now = unix_seconds()?;
    if expires <= now {
        return Err(error("grant expired"));
    }
    if expires.saturating_sub(now) > 24 * 60 * 60 {
        return Err(error("grant expiry exceeds 24 hours"));
    }
    Ok(())
}
fn limits_match(granted: &Value, pinned: &Value) -> bool {
    let Some(wall) = granted["wall_millis"].as_u64() else {
        return false;
    };
    if wall == 0 || pinned["wall_millis"].as_u64().is_none_or(|max| wall > max) {
        return false;
    }
    let mut normalized = granted.clone();
    normalized["wall_millis"] = pinned["wall_millis"].clone();
    normalized == *pinned
}
#[cfg(test)]
std::thread_local! { static FAIL_SYNC:std::cell::Cell<bool>=const {std::cell::Cell::new(false)}; }
fn durable(file: &File) -> io::Result<()> {
    #[cfg(test)]
    if FAIL_SYNC.with(std::cell::Cell::get) {
        return Err(error("fixture sync failure"));
    }
    file.sync_all()
}
fn checked_file(file: File) -> io::Result<File> {
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.mode() & 0o077 != 0
    {
        return Err(error("grant must be a private owned regular file"));
    }
    Ok(file)
}
fn open_owned(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed());
    checked_file(options.open(path)?)
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
fn validate_grant(
    grant: Grant,
    id: &str,
    pin: &Value,
    pin_bytes: &[u8],
    expected: &Grant,
) -> io::Result<ConsumedGrant> {
    if &grant != expected {
        return Err(error("lead grant differs from requested run"));
    }
    if grant.version != 1
        || grant.id != id
        || grant.pin_sha256 != kinewright_hash::sha256_bytes(pin_bytes)
        || ![1, 5, 6].contains(&grant.case)
        || !["codex", "claude"].contains(&grant.provider.as_str())
        || grant.arm != "candidate"
        || grant.models_and_options != pin["models_and_options"][&grant.provider]
        || !limits_match(&grant.limits, &pin["limits"])
        || grant.run_identity.len() != 32
        || !grant.run_identity.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(error("lead grant differs from the pinned run"));
    }
    check_expiry(grant.expires_at_unix_seconds)?;
    for role in [
        "director", "editor", "reviewer", "motion", "audio", "colour",
    ] {
        let value = &grant.models_and_options[role];
        if value["model"].as_str().is_none_or(str::is_empty)
            || !value["options"].is_array()
            || value["status"]
                .as_str()
                .is_none_or(|s| s.starts_with("OPEN"))
        {
            return Err(error("models and options remain OPEN"));
        }
    }
    if grant.dry_pass["pin_sha256"] != grant.pin_sha256
        || grant.dry_pass["provider"] != grant.provider
        || grant.dry_pass["launcher_sha256"] != pin["launcher"]["source_sha256"]
        || [
            "egress_denied",
            "stdin_eof",
            "first_progress",
            "proc_startup",
        ]
        .iter()
        .any(|key| grant.dry_pass[*key] != true)
    {
        return Err(error("a matching full-product unpaid dry pass is required"));
    }
    let candidate = ConsumedGrant {
        grant,
        authority: LaunchAuthority::Unpaid,
    };
    let editor = candidate.selection("editor")?;
    for role in ["reviewer", "motion", "audio", "colour"] {
        if candidate.selection(role)? != editor {
            return Err(error(
                "this Studio requires one pinned specialist selection",
            ));
        }
    }
    Ok(candidate)
}
#[cfg(test)]
fn consume(
    folder: &Path,
    id: &str,
    pin: &Value,
    pin_bytes: &[u8],
    expected: &Grant,
) -> io::Result<ConsumedGrant> {
    consume_at(&opened_home(folder)?, id, pin, pin_bytes, expected)
}
#[cfg(test)]
fn consume_at(
    folder: &File,
    id: &str,
    pin: &Value,
    pin_bytes: &[u8],
    expected: &Grant,
) -> io::Result<ConsumedGrant> {
    consume_guarded_at(folder, id, pin, pin_bytes, expected, None)
}
fn consume_guarded_at(
    folder: &File,
    id: &str,
    pin: &Value,
    pin_bytes: &[u8],
    expected: &Grant,
    guardian: Option<&Path>,
) -> io::Result<ConsumedGrant> {
    if !id_valid(id) {
        return Err(error("missing or invalid lead grant id"));
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
    if let Some(guardian) = guardian {
        verify_guardian_hash(expected, guardian)?;
    }
    let mut file = open_relative(folder, &format!("{id}.json"), false)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(error("grant capacity"));
    }
    let candidate = validate_grant(
        serde_json::from_slice(&bytes)?,
        id,
        pin,
        pin_bytes,
        expected,
    )?;
    file.seek(SeekFrom::Start(0))?;
    let mut again = Vec::new();
    Read::by_ref(&mut file)
        .take(64 * 1024 + 1)
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
        return Err(error("grant changed before consumption"));
    }
    // An id and a run identity are both irrevocable, including failed starts.
    check_expiry(candidate.grant.expires_at_unix_seconds)?;
    match rustix::fs::mkdirat(folder, "consumed", rustix::fs::Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(e) => return Err(e.into()),
    }
    let consumed = child_directory(folder, std::ffi::OsStr::new("consumed"))?;
    check_directory(&consumed, true)?;
    for name in [
        format!("id-{id}.json"),
        format!("run-{}.json", candidate.grant.run_identity),
    ] {
        let mut receipt = open_relative(&consumed, &name, true)?;
        receipt.write_all(&bytes)?;
        durable(&receipt)?;
    }
    durable(&consumed)?;
    durable(folder)?;
    Ok(candidate)
}
fn consume_for_guardian(
    folder: &File,
    id: &str,
    pin: &Value,
    pin_bytes: &[u8],
    expected: &Grant,
    guardian: &Path,
) -> io::Result<ConsumedGrant> {
    consume_guarded_at(folder, id, pin, pin_bytes, expected, Some(guardian))
}
fn verify_guardian_hash(grant: &Grant, guardian: &Path) -> io::Result<()> {
    if grant.dry_pass["guardian_sha256"]
        != kinewright_media::sha256_file(guardian).map_err(error)?
    {
        return Err(error("guardian changed since the dry pass"));
    }
    Ok(())
}

fn home_in_accounts(bytes: &[u8], uid: u32) -> io::Result<PathBuf> {
    let text = std::str::from_utf8(bytes).map_err(error)?;
    let mut homes = text.lines().filter_map(|line| {
        let fields = line.split(':').collect::<Vec<_>>();
        (fields.len() == 7 && fields[2].parse::<u32>() == Ok(uid)).then(|| PathBuf::from(fields[5]))
    });
    let home = homes
        .next()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| error("local account home unavailable"))?;
    if homes.next().is_some() {
        return Err(error("ambiguous local account home"));
    }
    Ok(home)
}
fn check_home(home: &Path, provided: Option<&std::ffi::OsStr>) -> io::Result<()> {
    if provided.map(Path::new) != Some(home) {
        return Err(error(
            "HOME differs from the local account home; grant path cannot be redirected",
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
        return Err(error("local account metadata capacity"));
    }
    home_in_accounts(&bytes, rustix::process::getuid().as_raw())
}
pub(super) fn from_env(guardian: &Path) -> io::Result<ConsumedGrant> {
    let id = std::env::var("KINEWRIGHT_TE1_RUN")
        .map_err(|_| error("a per-run lead grant is required"))?;
    if !id_valid(&id) {
        return Err(error("invalid grant id"));
    }
    let plan = std::env::var_os("KINEWRIGHT_TE1_PLAN")
        .ok_or_else(|| error("a run plan is required; an env id alone is not approval"))?;
    let mut plan_file = open_owned(&PathBuf::from(plan))?;
    let mut plan_bytes = Vec::new();
    Read::by_ref(&mut plan_file)
        .take(64 * 1024 + 1)
        .read_to_end(&mut plan_bytes)?;
    if plan_bytes.len() > 64 * 1024 {
        return Err(error("run plan capacity"));
    }
    let expected: Grant = serde_json::from_slice(&plan_bytes)?;
    let pin_bytes = std::fs::read(super::fixtures().with_file_name("te15-measurement-pin.json"))?;
    let pin: Value = serde_json::from_slice(&pin_bytes)?;
    super::pin::verify(&pin)?;
    // A HOME override must not redirect lead approval to a worker-made folder.
    // Linux local-account metadata is public; no credential file is consulted.
    let home = account_home()?;
    check_home(&home, std::env::var_os("HOME").as_deref())?;
    let guardian = guardian.canonicalize()?;
    let mut permit = consume_for_guardian(
        &grants_directory(&home)?,
        &id,
        &pin,
        &pin_bytes,
        &expected,
        &guardian,
    )?;
    permit.authority = LaunchAuthority::Native { guardian, home };
    Ok(permit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn data() -> (Value, Vec<u8>, Grant) {
        let mut pin = super::super::pin::record().unwrap();
        for provider in ["codex", "claude"] {
            for role in [
                "director", "editor", "reviewer", "motion", "audio", "colour",
            ] {
                pin["models_and_options"][provider][role] = json!({"status":"chosen for this temporary witness","model":"fixture","options":[]});
            }
        }
        let bytes = serde_json::to_vec(&pin).unwrap();
        let grant = Grant {
            version: 1,
            expires_at_unix_seconds: unix_seconds().unwrap() + 3600,
            id: "temporary-run".into(),
            pin_sha256: kinewright_hash::sha256_bytes(&bytes),
            provider: "codex".into(),
            case: 5,
            arm: crate::writer_host::te1_eval_surface::arm().into(),
            models_and_options: pin["models_and_options"]["codex"].clone(),
            limits: pin["limits"].clone(),
            run_identity: "1234567890abcdef1234567890abcdef".into(),
            dry_pass: json!({"pin_sha256":kinewright_hash::sha256_bytes(&bytes),"provider":"codex","launcher_sha256":pin["launcher"]["source_sha256"],"egress_denied":true,"stdin_eof":true,"first_progress":true,"proc_startup":true}),
        };
        (pin, bytes, grant)
    }
    fn folder() -> kinewright_media::test_support::TempDirectory {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "b2b-grants",
        );
        std::fs::set_permissions(temp.root(), std::fs::Permissions::from_mode(0o700)).unwrap();
        temp
    }
    fn write(folder: &Path, grant: &Grant) {
        let path = folder.join(format!("{}.json", grant.id));
        std::fs::write(&path, serde_json::to_vec(grant).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn te15_b2b_fu2_guardian_replaced_after_hash() {
        let (pin, bytes, mut grant) = data();
        let temp = folder();
        let guardian = temp.path("guardian");
        std::fs::write(&guardian, b"synthetic image A").unwrap();
        grant.dry_pass["guardian_sha256"] =
            json!(kinewright_media::sha256_file(&guardian).unwrap());
        write(temp.root(), &grant);
        let permit = consume_for_guardian(
            &opened_home(temp.root()).unwrap(),
            &grant.id,
            &pin,
            &bytes,
            &grant,
            &guardian,
        )
        .unwrap();
        assert!(temp.path("consumed").exists());
        std::fs::write(temp.path("replacement"), b"synthetic image B").unwrap();
        std::fs::rename(temp.path("replacement"), &guardian).unwrap();
        let mut launches = 0;
        let result = permit.verify_guardian(&guardian).map(|()| launches += 1);
        assert!(result.is_err(), "FU2_REPLACED_GUARDIAN_REFUSED");
        assert_eq!(launches, 0, "FU2_ZERO_PROVIDER_STARTS");
        assert!(
            consume_for_guardian(
                &opened_home(temp.root()).unwrap(),
                &grant.id,
                &pin,
                &bytes,
                &grant,
                &guardian
            )
            .is_err(),
            "spent grant stays spent"
        );
    }
    #[test]
    fn te15_b2b_fu2_unchanged_guardian() {
        let (pin, bytes, mut grant) = data();
        let temp = folder();
        let guardian = temp.path("guardian");
        std::fs::write(&guardian, b"unchanged synthetic image").unwrap();
        grant.dry_pass["guardian_sha256"] =
            json!(kinewright_media::sha256_file(&guardian).unwrap());
        write(temp.root(), &grant);
        let permit = consume_for_guardian(
            &opened_home(temp.root()).unwrap(),
            &grant.id,
            &pin,
            &bytes,
            &grant,
            &guardian,
        )
        .unwrap();
        permit.verify_guardian(&guardian).unwrap();
    }
    #[test]
    fn te15_b2b_fu2_wall_includes_setup() {
        let (pin, bytes, mut grant) = data();
        let temp = folder();
        grant.limits["wall_millis"] = json!(1000);
        write(temp.root(), &grant);
        let permit = consume(temp.root(), &grant.id, &pin, &bytes, &grant).unwrap();
        let mut started = None;
        let ((), deadline) = super::super::start_with_deadline(Some(&permit), || {
            started = Some(std::time::Instant::now());
            std::thread::sleep(std::time::Duration::from_millis(5));
            Ok(())
        })
        .unwrap();
        assert!(
            deadline <= started.unwrap() + permit.wall_duration(),
            "FU2_SETUP_COUNTS_AGAINST_GRANTED_WALL"
        );
    }
    #[test]
    fn te15_b2b_fu2_writable_chain_refused() {
        let (_, _, grant) = data();
        let home = folder();
        let grants = home.path(".local/share/kinewright-eval/grants");
        std::fs::create_dir_all(&grants).unwrap();
        std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o700)).unwrap();
        write(&grants, &grant);
        for name in ["", ".local", ".local/share", ".local/share/kinewright-eval"] {
            let path = home.path(name);
            let original = path.metadata().unwrap().permissions();
            for mode in [0o720, 0o702] {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
                assert!(
                    grants_directory(home.root()).is_err(),
                    "FU2_WRITABLE_PARENT_REFUSED {name} {mode:o}"
                );
                assert!(!grants.join("consumed").exists());
            }
            std::fs::set_permissions(path, original).unwrap();
        }
        assert!(grants_directory(home.root()).is_ok());
    }
    #[test]
    fn te15_b2b_fu1_expired() {
        let (pin, bytes, mut grant) = data();
        let temp = folder();
        grant.expires_at_unix_seconds = unix_seconds().unwrap() - 1;
        write(temp.root(), &grant);
        let result = consume(temp.root(), &grant.id, &pin, &bytes, &grant);
        assert!(result.is_err(), "FU1_EXPIRED_REFUSED");
        assert!(!temp.path("consumed").exists(), "FU1_EXPIRED_NOT_CONSUMED");
    }
    #[test]
    fn te15_b2b_fu1_expiry_too_far() {
        let (pin, bytes, mut grant) = data();
        let temp = folder();
        grant.expires_at_unix_seconds = unix_seconds().unwrap() + 25 * 3600;
        write(temp.root(), &grant);
        assert!(consume(temp.root(), &grant.id, &pin, &bytes, &grant).is_err());
        assert!(!temp.path("consumed").exists());
    }
    #[test]
    fn te15_b2b_fu1_symlink_parent() {
        let (pin, bytes, grant) = data();
        let home = folder();
        let real = home.path("real-local");
        std::fs::create_dir_all(real.join("share/kinewright-eval/grants")).unwrap();
        let grants = real.join("share/kinewright-eval/grants");
        std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o700)).unwrap();
        write(&grants, &grant);
        symlink(&real, home.path(".local")).unwrap();
        let result = grants_directory(home.root())
            .and_then(|dir| consume_at(&dir, &grant.id, &pin, &bytes, &grant));
        assert!(result.is_err(), "FU1_PARENT_SYMLINK_REFUSED");
        assert!(!grants.join("consumed").exists(), "FU1_PARENT_NOT_CONSUMED");
        std::fs::remove_file(home.path(".local")).unwrap();
        std::fs::rename(real, home.path(".local")).unwrap();
        let dir = grants_directory(home.root()).unwrap();
        consume_at(&dir, &grant.id, &pin, &bytes, &grant).unwrap();
        assert_eq!(
            home.path(".local/share/kinewright-eval/grants/consumed")
                .metadata()
                .unwrap()
                .mode()
                & 0o777,
            0o700
        );
    }
    #[test]
    fn te15_b2b_fu1_guardian_before_consumption() {
        let (pin, bytes, mut grant) = data();
        let temp = folder();
        let guardian = temp.path("guardian");
        std::fs::write(&guardian, b"rebuilt synthetic guardian").unwrap();
        grant.dry_pass["guardian_sha256"] = serde_json::json!("old image");
        write(temp.root(), &grant);
        let dir = opened_home(temp.root()).unwrap();
        assert!(consume_for_guardian(&dir, &grant.id, &pin, &bytes, &grant, &guardian).is_err());
        assert!(!temp.path("consumed").exists());
        grant.dry_pass["guardian_sha256"] =
            serde_json::json!(kinewright_media::sha256_file(&guardian).unwrap());
        write(temp.root(), &grant);
        consume_for_guardian(&dir, &grant.id, &pin, &bytes, &grant, &guardian).unwrap();
    }
    #[test]
    fn te15_b2b_fu1_lower_wall_used() {
        let (pin, bytes, mut grant) = data();
        let temp = folder();
        grant.limits["wall_millis"] = serde_json::json!(1234);
        write(temp.root(), &grant);
        let permit = consume(temp.root(), &grant.id, &pin, &bytes, &grant).unwrap();
        assert_eq!(
            super::super::run_timeout(Some(&permit)),
            std::time::Duration::from_millis(1234)
        );
    }
    #[test]
    fn te15_b2b_fu1_changed_limits_refused() {
        let (pin, bytes, grant) = data();
        for key in pin["limits"]
            .as_object()
            .unwrap()
            .keys()
            .chain(std::iter::once(&"extra".to_owned()))
        {
            let temp = folder();
            let mut changed = grant.clone();
            changed.limits[key] = serde_json::json!(pin["limits"][key].as_u64().unwrap_or(0) + 1);
            write(temp.root(), &changed);
            assert!(
                consume(temp.root(), &changed.id, &pin, &bytes, &changed).is_err(),
                "changed {key}"
            );
            assert!(!temp.path("consumed").exists());
        }
    }
    #[test]
    fn te15_b2b_candidate_grant_only() {
        let (pin, bytes, grant) = data();
        let mut constructions = 0;
        for arm in ["baseline", "other", "Candidate", ""] {
            let temp = folder();
            let mut refused = grant.clone();
            refused.arm = arm.into();
            write(temp.root(), &refused);
            let result = consume(temp.root(), &refused.id, &pin, &bytes, &refused)
                .inspect(|_| constructions += 1);
            assert!(result.is_err(), "B2B_ONLY_CANDIDATE_GRANT {arm}");
            assert_eq!(constructions, 0, "B2B_NONCANDIDATE_ZERO_CONSTRUCTION");
            assert!(!temp.path("consumed").exists());
        }
        let temp = folder();
        write(temp.root(), &grant);
        let token = consume(temp.root(), &grant.id, &pin, &bytes, &grant).unwrap();
        constructions += 1;
        assert_eq!(constructions, 1, "candidate needs no earlier run");
        assert!(
            token.guardian().is_err(),
            "temporary grants never launch providers"
        );
    }
    #[test]
    fn te15_b2b_grant_boundary() {
        let (pin, bytes, grant) = data();
        let folder = folder();
        let mut constructions = 0;
        let mut attempt = |expected: &Grant| {
            let token = consume(folder.root(), &expected.id, &pin, &bytes, expected)?;
            constructions += 1;
            Ok::<_, io::Error>(token)
        };
        assert!(attempt(&grant).is_err(), "B2B_MISSING_GRANT");
        write(folder.root(), &grant);
        let mut changed = grant.clone();
        changed.case = 6;
        assert!(attempt(&changed).is_err(), "B2B_CHANGED_GRANT");
        let token = attempt(&grant).unwrap();
        assert!(
            token.guardian().is_err(),
            "B2B_TEMP_RECEIPT_IS_NOT_NATIVE_AUTHORITY"
        );
        assert!(
            super::super::run_case(
                &folder.path("native-denied"),
                5,
                "codex",
                None,
                Some(&std::sync::Arc::new(token))
            )
            .is_err(),
            "B2B_DIRECT_NATIVE_ROUTE_REFUSED"
        );
        assert!(!folder.path("native-denied").exists());
        assert!(attempt(&grant).is_err(), "B2B_REUSED_GRANT");
        assert_eq!(constructions, 1, "B2B_NO_CONSTRUCTION_ON_REFUSAL");
        assert!(folder.path("consumed/id-temporary-run.json").exists());
        let mut another = grant.clone();
        another.id = "changed-id-same-run".into();
        write(folder.root(), &another);
        assert!(
            consume(folder.root(), &another.id, &pin, &bytes, &another).is_err(),
            "B2B_RUN_ID_REUSED"
        );
    }
    #[test]
    fn te15_b2b_grant_open_and_identity_fields() {
        let (pin, bytes, grant) = data();
        for field in [
            "pin_sha256",
            "provider",
            "case",
            "arm",
            "models_and_options",
            "limits",
            "run_identity",
            "expires_at_unix_seconds",
        ] {
            let temp = folder();
            write(temp.root(), &grant);
            let mut value = serde_json::to_value(&grant).unwrap();
            value[field] = match field {
                "case" | "expires_at_unix_seconds" => json!(1),
                "models_and_options" | "limits" => json!({}),
                _ => json!("changed"),
            };
            let changed: Grant = serde_json::from_value(value).unwrap();
            assert!(
                consume(temp.root(), &grant.id, &pin, &bytes, &changed).is_err(),
                "B2B_BOUND_FIELD {field}"
            );
            assert!(!temp.path("consumed").exists());
        }
        let temp = folder();
        let mut open = grant.clone();
        open.models_and_options["editor"]["status"] = json!("OPEN");
        let mut openpin = pin.clone();
        openpin["models_and_options"]["codex"] = open.models_and_options.clone();
        let openbytes = serde_json::to_vec(&openpin).unwrap();
        open.pin_sha256 = kinewright_hash::sha256_bytes(&openbytes);
        open.dry_pass["pin_sha256"] = json!(open.pin_sha256);
        write(temp.root(), &open);
        assert!(
            consume(temp.root(), &open.id, &openpin, &openbytes, &open).is_err(),
            "B2B_OPEN_CHOICES_REFUSED"
        );
    }
    #[test]
    fn te15_b2b_grant_symlinks_and_concurrency() {
        let (pin, bytes, grant) = data();
        let temp = folder();
        let target = temp.path("outside");
        std::fs::write(&target, serde_json::to_vec(&grant).unwrap()).unwrap();
        symlink(&target, temp.path("temporary-run.json")).unwrap();
        assert!(
            consume(temp.root(), &grant.id, &pin, &bytes, &grant).is_err(),
            "B2B_SYMLINK_REFUSED"
        );
        std::fs::remove_file(temp.path("temporary-run.json")).unwrap();
        write(temp.root(), &grant);
        std::thread::scope(|scope| {
            let a = scope.spawn(|| consume(temp.root(), &grant.id, &pin, &bytes, &grant));
            let b = scope.spawn(|| consume(temp.root(), &grant.id, &pin, &bytes, &grant));
            assert_eq!(
                usize::from(a.join().unwrap().is_ok()) + usize::from(b.join().unwrap().is_ok()),
                1,
                "B2B_ONE_CONSUMER"
            );
        });
    }
    #[test]
    fn te15_b2b_grant_durability() {
        let (pin, bytes, grant) = data();
        let folder = folder();
        write(folder.root(), &grant);
        let mut constructions = 0;
        FAIL_SYNC.with(|f| f.set(true));
        let result = consume(folder.root(), &grant.id, &pin, &bytes, &grant);
        FAIL_SYNC.with(|f| f.set(false));
        if result.is_ok() {
            constructions += 1;
        }
        assert_eq!(constructions, 0, "B2B_SYNC_BEFORE_CONSTRUCTION");
        assert!(result.is_err());
        assert!(
            consume(folder.root(), &grant.id, &pin, &bytes, &grant).is_err(),
            "B2B_FAILED_CONSUMPTION_IRREVOCABLE"
        );
    }
    #[test]
    fn te15_b2b_grant_interrupted() {
        const KEY: &str = "KINEWRIGHT_B2B_TEMP_RECEIPT_CHILD";
        if let Some(root) = std::env::var_os(KEY) {
            let (pin, bytes, _) = data();
            let grant: Grant = serde_json::from_slice(
                &std::fs::read(Path::new(&root).join("temporary-run.json")).unwrap(),
            )
            .unwrap();
            let _token = consume(Path::new(&root), &grant.id, &pin, &bytes, &grant).unwrap();
            // Terminate without Drop, just after the durable boundary, before construction.
            std::process::exit(0);
        }
        let (pin, bytes, grant) = data();
        let folder = folder();
        write(folder.root(), &grant);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "te1_eval::grant::tests::te15_b2b_grant_interrupted",
            ])
            .env(KEY, folder.root())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "interrupted-run child failed: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            consume(folder.root(), &grant.id, &pin, &bytes, &grant).is_err(),
            "B2B_INTERRUPTION_NO_RETRY"
        );
    }
    #[test]
    fn te15_b2b_grant_dry_pass() {
        let (pin, bytes, grant) = data();
        for field in [
            "egress_denied",
            "stdin_eof",
            "first_progress",
            "proc_startup",
            "pin_sha256",
            "provider",
            "launcher_sha256",
        ] {
            let temp = folder();
            let mut changed = grant.clone();
            changed.dry_pass[field] = Value::Null;
            write(temp.root(), &changed);
            assert!(
                consume(temp.root(), &changed.id, &pin, &bytes, &changed).is_err(),
                "B2B_DRY_PASS_REQUIRED {field}"
            );
            assert!(!temp.path("consumed").exists());
        }
        // Tests never resolve the production HOME/grants path. An id alone is
        // rejected before even reading a plan or computing the real grant root.
        let temp = folder();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "te1_eval::grant::tests::te15_b2b_env_only_child"])
            .env("HOME", temp.root())
            .env("KINEWRIGHT_TE1_RUN", "env-only")
            .env_remove("KINEWRIGHT_TE1_PLAN")
            .env("KINEWRIGHT_B2B_ENV_ONLY_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "env-only child failed: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!temp.path(".local").exists());
    }
    #[test]
    fn te15_b2b_env_only_child() {
        if std::env::var_os("KINEWRIGHT_B2B_ENV_ONLY_CHILD").is_none() {
            return;
        }
        let refusal = from_env(Path::new("not-opened"))
            .err()
            .expect("B2B_ENV_ID_NOT_APPROVAL");
        assert!(refusal.to_string().contains("env id alone"));
    }

    #[test]
    fn te15_b2b_fixed_account_home() {
        assert!(
            check_home(
                Path::new("/home/fixture"),
                Some(std::ffi::OsStr::new("/temporary/worker"))
            )
            .is_err(),
            "B2B_HOME_OVERRIDE_REFUSED"
        );
        assert!(
            check_home(
                Path::new("/home/fixture"),
                Some(std::ffi::OsStr::new("/home/fixture"))
            )
            .is_ok()
        );
        let accounts =
            b"fixture:x:123:123::/home/fixture:/bin/sh\nother:x:124:124::/home/other:/bin/sh\n";
        assert_eq!(
            home_in_accounts(accounts, 123).unwrap(),
            PathBuf::from("/home/fixture"),
            "B2B_FIXED_ACCOUNT_HOME"
        );
        assert!(home_in_accounts(accounts, 999).is_err());
        assert!(home_in_accounts(b"a:x:123:123::relative:/bin/sh", 123).is_err());
        assert!(
            home_in_accounts(b"a:x:123:123::/a:/bin/sh\nb:x:123:123::/b:/bin/sh", 123).is_err()
        );
    }
}
