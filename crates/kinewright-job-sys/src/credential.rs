//! Single-file credentials. The directory containing the source is never granted.
use std::{
    fs::File,
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
};

/// Open without following a final symlink and verify the kernel file identity.
///
/// # Errors
/// Refuses a nonregular, replaced, foreign-owned, or overly accessible file.
pub fn pin_credential_file(path: &Path, expected: Option<[u64; 2]>) -> io::Result<File> {
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !credential_metadata_valid(&metadata, rustix::process::getuid().as_raw())
        || expected.is_some_and(|identity| identity != [metadata.dev(), metadata.ino()])
    {
        return Err(io::Error::other(
            "credential file identity or permissions refused",
        ));
    }
    Ok(file)
}

fn credential_metadata_valid(metadata: &std::fs::Metadata, uid: u32) -> bool {
    metadata.is_file() && metadata.uid() == uid && metadata.mode() & 0o7777 == 0o600
}

/// Bind one pinned file into the private mount namespace before provider execution.
///
/// # Errors
/// Refuses an invalid source/target or a failed single-file mount.
pub fn mount_credential_file(source: &Path, target: &Path, identity: [u64; 2]) -> io::Result<()> {
    let source = pin_credential_file(source, Some(identity))?;
    let target_file = pin_credential_file(target, None)?;
    let target_path = format!("/proc/self/fd/{}", target_file.as_raw_fd());
    rustix::mount::mount_bind(format!("/proc/self/fd/{}", source.as_raw_fd()), target_path)?;
    // Check by pathname after mounting, not the descriptor to the old placeholder.
    pin_credential_file(target, Some(identity))?;
    Ok(())
}

/// C5 fix 8, the unpaid dry policy only: bind the pinned file over its
/// placeholder, then remount that bind read-only and check it with
/// `statvfs`. Writes and truncation meet the read-only mount; rename over it
/// and removal meet a mount point. The private per-attempt home around it
/// stays writable: Codex keeps its own state there, and the home is
/// discarded with the attempt.
///
/// # Errors
/// Refuses an invalid source or target, a failed bind or remount, or a mount
/// that does not report read-only.
pub fn mount_credential_file_read_only(
    source: &Path,
    target: &Path,
    identity: [u64; 2],
) -> io::Result<()> {
    mount_credential_file(source, target, identity)?;
    remount_read_only(target)?;
    pin_credential_file(target, Some(identity))?;
    if !read_only(target)? {
        return Err(io::Error::other("credential_mount_not_read_only"));
    }
    Ok(())
}

/// Most entries the unpaid policy inspects in a private home.
const HOME_ENTRIES: usize = 256;

/// C5 fix 8: the unpaid policy's private home holds nothing of the person's
/// but one read-only `auth.json` bind. The home is a private, writable
/// directory that is no mount point; every entry beneath it is a directory
/// or a single-link regular file owned by this user, on the home's own
/// mount, except `auth.json`, which must be the pinned sign-in on a separate,
/// read-only mount. No symbolic link, hard link, device, socket or other
/// mount is accepted. Returns one line per entry, sorted.
///
/// # Errors
/// Refuses any other shape.
pub fn verify_unpaid_home(home: &Path, credential: [u64; 2]) -> io::Result<Vec<String>> {
    use rustix::fs::{AtFlags, FileType, StatxAttributes, StatxFlags};
    let refuse = |why: &str| io::Error::other(format!("unpaid_home_refused: {why}"));
    let stat = |path: &Path| {
        rustix::fs::statx(
            rustix::fs::CWD,
            path,
            AtFlags::SYMLINK_NOFOLLOW,
            StatxFlags::BASIC_STATS | StatxFlags::MNT_ID,
        )
        .map_err(io::Error::from)
    };
    let uid = rustix::process::getuid().as_raw();
    let root = stat(home)?;
    if FileType::from_raw_mode(root.stx_mode.into()) != FileType::Directory
        || root.stx_uid != uid
        || root.stx_mode & 0o077 != 0
        || !root
            .stx_attributes_mask
            .contains(StatxAttributes::MOUNT_ROOT)
        || root.stx_attributes.contains(StatxAttributes::MOUNT_ROOT)
        || read_only(home)?
    {
        return Err(refuse("home is not a private writable directory"));
    }
    let target = home.join("auth.json");
    let mut lines = Vec::new();
    let mut credential_seen = false;
    let mut pending = vec![(home.to_path_buf(), 0)];
    while let Some((directory, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if lines.len() >= HOME_ENTRIES {
                return Err(refuse("too many entries"));
            }
            let meta = stat(&path)?;
            let kind = FileType::from_raw_mode(meta.stx_mode.into());
            let relative = path
                .strip_prefix(home)
                .map_err(|_| refuse("entry outside home"))?
                .display()
                .to_string();
            let identity = [
                rustix::fs::makedev(meta.stx_dev_major, meta.stx_dev_minor),
                meta.stx_ino,
            ];
            if meta.stx_uid != uid {
                return Err(refuse("foreign owner"));
            }
            if path == target {
                if kind != FileType::RegularFile
                    || identity != credential
                    || meta.stx_mnt_id == root.stx_mnt_id
                    || !read_only(&path)?
                {
                    return Err(refuse("auth.json is not the read-only sign-in bind"));
                }
                credential_seen = true;
                lines.push(format!("file {relative} (read-only bind of the sign-in)"));
                continue;
            }
            if meta.stx_mnt_id != root.stx_mnt_id {
                return Err(refuse("another mount"));
            }
            // The identity is pinned from the live sign-in, so no other file
            // can carry it: a match is a second name for the sign-in.
            if identity == credential {
                return Err(refuse("the sign-in elsewhere"));
            }
            match kind {
                FileType::Directory if depth < 8 => {
                    lines.push(format!("dir {relative}"));
                    pending.push((path, depth + 1));
                }
                FileType::RegularFile if meta.stx_nlink == 1 => {
                    lines.push(format!("file {relative}"));
                }
                FileType::Symlink => return Err(refuse("symbolic link")),
                FileType::RegularFile => return Err(refuse("hard link")),
                _ => return Err(refuse("other entry")),
            }
        }
    }
    if !credential_seen {
        return Err(refuse("auth.json missing"));
    }
    lines.sort();
    Ok(lines)
}

/// C5 fix 8: in init's mount table, the only mount at or beneath the
/// private home is the read-only `auth.json` bind.
///
/// # Errors
/// Refuses any other mount there, or a writable sign-in mount.
pub fn verify_unpaid_mounts(mountinfo: &str, home: &Path, target: &Path) -> io::Result<()> {
    fn escaped(path: &Path) -> String {
        let mut out = String::new();
        for c in path.to_string_lossy().chars() {
            match c {
                ' ' => out.push_str("\\040"),
                '\t' => out.push_str("\\011"),
                '\n' => out.push_str("\\012"),
                '\\' => out.push_str("\\134"),
                c => out.push(c),
            }
        }
        out
    }
    let (home, target) = (escaped(home), escaped(target));
    let beneath = format!("{home}/");
    let mounts = mountinfo
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            Some((*fields.get(4)?, *fields.get(5)?))
        })
        .filter(|(point, _)| *point == home || point.starts_with(&beneath))
        .collect::<Vec<_>>();
    match mounts.as_slice() {
        [(point, options)] if *point == target && options.split(',').any(|o| o == "ro") => Ok(()),
        _ => Err(io::Error::other(
            "unpaid_home_refused: mounts beneath the home",
        )),
    }
}

/// Whether the mount holding `path` refuses writes.
///
/// # Errors
/// Refuses an unavailable `statvfs`.
pub fn read_only(path: &Path) -> io::Result<bool> {
    Ok(rustix::fs::statvfs(path)?
        .f_flag
        .contains(rustix::fs::StatVfsMountFlags::RDONLY))
}

fn remount_read_only(target: &Path) -> io::Result<()> {
    use rustix::{fs::StatVfsMountFlags as S, mount::MountFlags as M};
    // A user namespace may add read-only but must keep the flags its parent
    // locked, so carry the current ones over.
    let current = rustix::fs::statvfs(target)?.f_flag;
    let mut flags = M::BIND | M::RDONLY;
    for (have, keep) in [
        (S::NOSUID, M::NOSUID),
        (S::NODEV, M::NODEV),
        (S::NOEXEC, M::NOEXEC),
        (S::NOATIME, M::NOATIME),
        (S::NODIRATIME, M::NODIRATIME),
        (S::RELATIME, M::RELATIME),
    ] {
        if current.contains(have) {
            flags |= keep;
        }
    }
    rustix::mount::mount_remount(target, flags, c"")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c5_credential_refuses_symlink_owner_and_replacement() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("c5-credential-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("auth.json");
        std::fs::write(&path, b"fixture").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let file = pin_credential_file(&path, None).unwrap();
        let metadata = file.metadata().unwrap();
        assert!(!credential_metadata_valid(&metadata, metadata.uid() + 1));
        assert!(pin_credential_file(&path, Some([metadata.dev(), metadata.ino() + 1])).is_err());
        let link = root.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(pin_credential_file(&link, None).is_err());
        for mode in [0o400, 0o200, 0o000, 0o4600, 0o2600, 0o1600] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                pin_credential_file(&path, None).is_err(),
                "C5_FIX1_EXACT_MODE_BEFORE {mode:o}"
            );
            assert!(
                pin_credential_file(&path, Some([metadata.dev(), metadata.ino()])).is_err(),
                "C5_FIX1_EXACT_MODE_AFTER {mode:o}"
            );
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(pin_credential_file(&path, None).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn s4b_c5_fix8_unpaid_mounts_allow_only_the_read_only_sign_in() {
        let home = Path::new("/w/s/codex-home");
        let target = home.join("auth.json");
        let line = |point: &str, options: &str| {
            format!("36 35 0:40 /x {point} {options},nosuid shared:1 - ext4 /dev/x rw\n")
        };
        let base = line("/", "rw");
        let good = format!("{base}{}", line("/w/s/codex-home/auth.json", "ro"));
        verify_unpaid_mounts(&good, home, &target).unwrap();
        for bad in [
            base.clone(),
            format!("{base}{}", line("/w/s/codex-home/auth.json", "rw")),
            format!("{good}{}", line("/w/s/codex-home/skills", "rw")),
            format!("{good}{}", line("/w/s/codex-home", "rw")),
        ] {
            assert!(verify_unpaid_mounts(&bad, home, &target).is_err(), "{bad}");
        }
        let spaced = Path::new("/w/a b");
        let spaced_good = format!("{base}{}", line("/w/a\\040b/auth.json", "ro"));
        verify_unpaid_mounts(&spaced_good, spaced, &spaced.join("auth.json")).unwrap();
    }

    #[test]
    fn s4b_c5_fix8_unpaid_home_refuses_without_the_bind_and_links() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("c5f8-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(home.join("skills")).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        // The identity comes from a sign-in that stays alive for the whole
        // test, as the pinned source does for a launch. A deleted file's
        // identity can be handed to the next new file: ext4 reuses freed
        // inode numbers at once, which made this fixture fail on CI.
        let sign_in = root.join("sign-in");
        std::fs::write(&sign_in, b"fixture").unwrap();
        let meta = std::fs::metadata(&sign_in).unwrap();
        let identity = [meta.dev(), meta.ino()];
        let auth = home.join("auth.json");
        std::fs::write(&auth, b"fixture").unwrap();
        // A plain file is not the bind: same mount as the home.
        let reason = verify_unpaid_home(&home, identity).unwrap_err().to_string();
        assert!(reason.contains("read-only sign-in bind"), "{reason}");
        std::fs::remove_file(&auth).unwrap();
        std::fs::write(home.join("other"), b"x").unwrap();
        let reason = verify_unpaid_home(&home, identity).unwrap_err().to_string();
        assert!(reason.contains("auth.json missing"), "{reason}");
        // A second name for the live sign-in anywhere in the home is refused.
        std::fs::hard_link(&sign_in, home.join("skills/sign-in")).unwrap();
        let reason = verify_unpaid_home(&home, identity).unwrap_err().to_string();
        assert!(reason.contains("the sign-in elsewhere"), "{reason}");
        std::fs::remove_file(home.join("skills/sign-in")).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", home.join("skills/link")).unwrap();
        let reason = verify_unpaid_home(&home, identity).unwrap_err().to_string();
        assert!(reason.contains("symbolic link"), "{reason}");
        std::fs::remove_file(home.join("skills/link")).unwrap();
        std::fs::hard_link(home.join("other"), root.join("outside")).unwrap();
        let reason = verify_unpaid_home(&home, identity).unwrap_err().to_string();
        assert!(reason.contains("hard link"), "{reason}");
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reason = verify_unpaid_home(&home, identity).unwrap_err().to_string();
        assert!(reason.contains("private writable"), "{reason}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
