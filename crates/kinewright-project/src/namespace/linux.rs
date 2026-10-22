use super::{NamespaceIdentity, PortablePath, ProjectNamespace, note};
use crate::{LockMode, acquire_project_lock_with_policy, bundle::BundleError};
use rustix::fs::{self, AtFlags, Mode, OFlags, RenameFlags};
use std::{
    fmt::Write as _,
    fs::File,
    io::Write,
    path::{Component, Path},
    time::Duration,
};

const DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn asset_name(namespace: &ProjectNamespace) -> Result<std::ffi::OsString, BundleError> {
    let mut name = namespace
        .identity
        .resolved_root
        .file_stem()
        .ok_or_else(|| BundleError::Identity("missing root stem".into()))?
        .to_os_string();
    name.push(".kinewright-assets");
    Ok(name)
}
pub(super) fn has_asset_store(namespace: &ProjectNamespace) -> Result<bool, BundleError> {
    verify_parent(namespace)?;
    match fs::statat(
        &namespace.parent,
        asset_name(namespace)?,
        AtFlags::SYMLINK_NOFOLLOW,
    ) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn probe_copy_contention(namespace: &ProjectNamespace) -> Result<(), BundleError> {
    verify_parent(namespace)?;
    let path = crate::lockfile_path_for_project(Some(&namespace.identity.resolved_root))
        .ok_or_else(|| BundleError::Identity("destination lock path".into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| BundleError::Identity("destination lock filename".into()))?;
    let file = match fs::openat(
        &namespace.parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let stat = fs::fstat(&file)?;
    if stat.st_nlink != 1 || fs::FileType::from_raw_mode(stat.st_mode) != fs::FileType::RegularFile
    {
        return Err(BundleError::Identity(
            "ambiguous destination lock object".into(),
        ));
    }
    fs2::FileExt::try_lock_exclusive(&file).map_err(|e| BundleError::Contention(e.to_string()))?;
    fs2::FileExt::unlock(&file)?;
    verify_parent(namespace)
}
fn reject_case_collision(directory: &File, name: &std::ffi::OsStr) -> Result<(), BundleError> {
    use std::os::fd::AsRawFd;
    let expected = name
        .to_str()
        .ok_or_else(|| BundleError::Identity("non-UTF8 destination name".into()))?;
    let folded = expected.to_lowercase();
    for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
        let actual = entry?.file_name();
        if actual != name && actual.to_str().is_some_and(|n| n.to_lowercase() == folded) {
            return Err(BundleError::Identity("destination case collision".into()));
        }
    }
    Ok(())
}
#[allow(
    clippy::too_many_lines,
    reason = "one no-follow ancestor claim through pinned destination-parent creation"
)]
pub(super) fn prepare_destination(path: &Path) -> Result<super::PreparedDestination, BundleError> {
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                PortablePath::new(Path::new(name))?;
            }
            Component::RootDir => {}
            _ => {
                return Err(BundleError::Identity(
                    "destination traversal or prefix".into(),
                ));
            }
        }
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let resolved = crate::canonical_project_identity(&absolute)
        .map_err(|e| BundleError::Identity(e.to_string()))?;
    let name = resolved
        .file_name()
        .ok_or_else(|| BundleError::Identity("destination filename".into()))?
        .to_os_string();
    PortablePath::new(Path::new(&name))?;
    let mut ancestor = resolved
        .parent()
        .ok_or_else(|| BundleError::Identity("destination parent".into()))?
        .to_path_buf();
    let mut missing = vec![];
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| BundleError::Identity("missing destination ancestor".into()))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| BundleError::Identity("missing destination ancestor".into()))?
            .to_path_buf();
    }
    let mut actual = ancestor.canonicalize()?;
    let mut parent = pin_directory(&actual)?;
    check_profile(&parent)?;
    let claim = if missing.is_empty() {
        None
    } else {
        let digest = kinewright_media::sha256_bytes(resolved.as_os_str().as_encoded_bytes());
        let claim = acquire(&actual.join(format!(".ja-s1-copy-parent-{digest}.kinewright")))?;
        let held = fs::fstat(&parent)?;
        if held.st_dev != claim.identity.parent_device || held.st_ino != claim.identity.parent_inode
        {
            return Err(BundleError::Identity("ancestor claim moved".into()));
        }
        Some(claim)
    };
    let mut prepared = super::PreparedDestination {
        path: resolved,
        parent: parent.try_clone()?,
        claim,
    };
    for component in missing.into_iter().rev() {
        let owner = prepared
            .claim
            .as_ref()
            .ok_or_else(|| BundleError::Identity("missing ancestor claim".into()))?;
        owner.verify()?;
        reject_case_collision(&parent, &component)?;
        actual.push(&component);
        match fs::mkdirat(&parent, &component, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            Ok(()) => note("mkdir", &actual),
            Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
        let next = File::from(fs::openat(&parent, &component, DIR, Mode::empty())?);
        check_profile(&next)?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("destination_child_sync", &actual)
            .map_err(|_| BundleError::DurabilityUnknown)?;
        sync_directory(&next, &actual).map_err(|_| BundleError::DurabilityUnknown)?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("destination_parent_sync", actual.parent().unwrap())
            .map_err(|_| BundleError::DurabilityUnknown)?;
        sync_directory(
            &parent,
            actual
                .parent()
                .ok_or_else(|| BundleError::Identity("ancestor path".into()))?,
        )?;
        parent = next;
    }
    reject_case_collision(&parent, &name)?;
    let path = actual.join(&name);
    let mut assets = path
        .file_stem()
        .ok_or_else(|| BundleError::Identity("destination stem".into()))?
        .to_os_string();
    assets.push(".kinewright-assets");
    reject_case_collision(&parent, &assets)?;
    let lock = crate::lockfile_path_for_project(Some(&path))
        .ok_or_else(|| BundleError::Identity("destination lock filename".into()))?;
    reject_case_collision(
        &parent,
        lock.file_name()
            .ok_or_else(|| BundleError::Identity("destination lock filename".into()))?,
    )?;
    prepared.path = path;
    prepared.parent = parent;
    Ok(prepared)
}
pub(super) fn seal_destination_ancestors(namespace: &ProjectNamespace) -> Result<(), BundleError> {
    namespace.verify()?;
    let mut path = namespace.identity.resolved_root.parent();
    while let Some(ancestor) = path {
        let handle = pin_directory(ancestor)?;
        if fs::fstat(&handle)?.st_dev != namespace.identity.parent_device {
            break;
        }
        namespace
            .pending_directory_syncs
            .borrow_mut()
            .insert(ancestor.to_path_buf(), (handle, true));
        path = ancestor.parent();
    }
    finish_directory_barriers(namespace)
}
pub(super) fn matches_destination(
    namespace: &ProjectNamespace,
    prepared: &super::PreparedDestination,
) -> Result<(), BundleError> {
    let expected = fs::fstat(&prepared.parent)?;
    if namespace.identity.resolved_root != prepared.path
        || namespace.identity.parent_device != expected.st_dev
        || namespace.identity.parent_inode != expected.st_ino
    {
        return Err(BundleError::Identity(
            "prepared destination was retargeted".into(),
        ));
    }
    let lock = crate::lockfile_path_for_project(Some(&namespace.identity.resolved_root))
        .ok_or_else(|| BundleError::Identity("destination lock filename".into()))?;
    reject_case_collision(
        &namespace.parent,
        lock.file_name()
            .ok_or_else(|| BundleError::Identity("destination lock filename".into()))?,
    )?;
    verify_parent(namespace)
}
pub(super) fn cleanup_empty_copy_store(
    namespace: &mut ProjectNamespace,
) -> Result<(), BundleError> {
    namespace.verify()?;
    verify_directories(namespace)?;
    let directories = namespace.directories.get_mut();
    let mut paths: Vec<_> = directories.keys().cloned().collect();
    paths.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for path in paths {
        let parent = if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            directories
                .get(parent)
                .ok_or_else(|| BundleError::Identity("copy cleanup parent".into()))?
                .try_clone()?
        } else {
            namespace.parent.try_clone()?
        };
        fs::unlinkat(
            &parent,
            path.file_name()
                .ok_or_else(|| BundleError::Identity("copy cleanup name".into()))?,
            AtFlags::REMOVEDIR,
        )?;
        sync_directory(
            &parent,
            namespace
                .identity
                .resolved_root
                .parent()
                .ok_or_else(|| BundleError::Identity("copy cleanup root parent".into()))?,
        )?;
        directories.remove(&path);
    }
    Ok(())
}

pub(super) fn inspect(path: &Path) -> Result<ProjectNamespace, BundleError> {
    let resolved = crate::canonical_project_identity(path)
        .map_err(|e| BundleError::Identity(e.to_string()))?;
    let parent_path = resolved
        .parent()
        .ok_or_else(|| BundleError::Identity("root has no parent".into()))?;
    let parent = pin_directory(parent_path)?;
    let root_name = resolved
        .file_name()
        .ok_or_else(|| BundleError::Identity("missing root name".into()))?
        .to_os_string();
    match fs::statat(&parent, &root_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat)
            if stat.st_nlink != 1
                || fs::FileType::from_raw_mode(stat.st_mode) != fs::FileType::RegularFile =>
        {
            return Err(BundleError::Identity(
                "ambiguous or non-regular root".into(),
            ));
        }
        Ok(_) | Err(rustix::io::Errno::NOENT) => {}
        Err(e) => return Err(e.into()),
    }
    check_profile(&parent)?;
    let stat = fs::fstat(&parent)?;
    let namespace = ProjectNamespace {
        identity: NamespaceIdentity {
            resolved_root: resolved,
            parent_device: stat.st_dev,
            parent_inode: stat.st_ino,
        },
        parent,
        session: None,
        root_name,
        lock: None,
        directories: std::cell::RefCell::new(std::collections::BTreeMap::new()),
        pending_directory_syncs: std::cell::RefCell::default(),
        dirty_object_directories: std::collections::BTreeMap::new(),
        batch_objects: false,
        uncertain: false,
        read_only: true,
    };
    verify_parent(&namespace)?;
    Ok(namespace)
}

pub(super) fn acquire(path: &Path) -> Result<ProjectNamespace, BundleError> {
    let mut namespace = inspect(path)?;
    claim(&mut namespace)?;
    Ok(namespace)
}
pub(super) fn claim(namespace: &mut ProjectNamespace) -> Result<(), BundleError> {
    if namespace.lock.is_some() {
        return namespace.verify();
    }
    verify_parent(namespace)?;
    let acquired = acquire_project_lock_with_policy(
        &namespace.identity.resolved_root,
        LockMode::Headless,
        "http://127.0.0.1:9/studio",
        &namespace
            .identity
            .resolved_root
            .parent()
            .unwrap()
            .join(".studio-recovery-scan"),
        1,
        Duration::ZERO,
    )
    .map_err(|e| BundleError::Contention(e.to_string()))?;
    if acquired.handle.resolved_identity() != namespace.identity.resolved_root {
        let _ = acquired.handle.release();
        return Err(BundleError::Identity(
            "claim identity changed during admission".into(),
        ));
    }
    namespace.lock = Some(acquired.handle);
    namespace.read_only = false;
    {
        let directories = namespace.directories.borrow();
        let base = namespace.identity.resolved_root.parent().unwrap();
        for (path, child) in directories.iter() {
            namespace
                .pending_directory_syncs
                .borrow_mut()
                .insert(base.join(path), (child.try_clone()?, false));
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
            let handle = parent.map_or(&namespace.parent, |p| &directories[p]);
            namespace.pending_directory_syncs.borrow_mut().insert(
                base.join(path.parent().unwrap()),
                (handle.try_clone()?, true),
            );
        }
    }

    if let Err(error) = namespace.verify() {
        if let Some(lock) = namespace.lock.take() {
            let _ = lock.release();
        }
        namespace.read_only = true;
        return Err(error);
    }
    Ok(())
}
pub(super) fn verify_parent(namespace: &ProjectNamespace) -> Result<(), BundleError> {
    let stat = fs::stat(namespace.identity.resolved_root.parent().unwrap())?;
    if stat.st_dev != namespace.identity.parent_device
        || stat.st_ino != namespace.identity.parent_inode
    {
        return Err(BundleError::Identity(
            "pinned parent no longer matches the admitted namespace".into(),
        ));
    }
    Ok(())
}

pub(super) fn read_at(
    dir: &File,
    name: &std::ffi::OsStr,
    limit: u64,
) -> Result<Option<Vec<u8>>, BundleError> {
    let fd = match fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(BundleError::Capacity("bounded regular file"));
    }
    crate::bundle_recovery::bounded_bytes(file, limit).map(Some)
}

fn temp_prefix(namespace: &ProjectNamespace) -> Result<String, BundleError> {
    let name = namespace
        .root_name
        .to_str()
        .ok_or_else(|| BundleError::Identity("temporary owner filename".into()))?;
    Ok(format!(
        ".studio-{}-",
        kinewright_media::sha256_bytes(name.as_bytes())
    ))
}
fn temp(
    namespace: &ProjectNamespace,
    dir: &File,
) -> Result<(std::ffi::OsString, File), BundleError> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|e| BundleError::Identity(e.to_string()))?;
    let mut encoded = temp_prefix(namespace)?;
    for byte in random {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded.push_str(".tmp");
    let name = std::ffi::OsString::from(encoded);
    let file = File::from(fs::openat(
        dir,
        &name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?);
    Ok((name, file))
}

pub(super) fn replace(namespace: &mut ProjectNamespace, bytes: &[u8]) -> Result<(), BundleError> {
    replace_inner(namespace, bytes, false)
}

pub(super) fn replace_inner(
    namespace: &mut ProjectNamespace,
    bytes: &[u8],
    refuse: bool,
) -> Result<(), BundleError> {
    namespace.verify()?;
    let (name, mut file) = temp(namespace, &namespace.parent)?;
    let temp_path = namespace
        .identity
        .resolved_root
        .parent()
        .unwrap()
        .join(&name);
    let staged = (|| {
        file.write_all(bytes)?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::note_bytes("root_stage", &temp_path, Some(bytes));
        sync_file(&file, "root_file_sync", &temp_path)?;
        note("bundle_B4", &namespace.identity.resolved_root);
        namespace.verify()?;
        #[cfg(any(test, feature = "test-util"))]
        let inject_io_error =
            crate::io_observer::check_failure("root_rename_io", &namespace.identity.resolved_root)
                .is_err();
        #[cfg(not(any(test, feature = "test-util")))]
        let inject_io_error = false;
        let renamed = if refuse {
            #[cfg(any(test, feature = "test-util"))]
            {
                let attempted = read_at(&namespace.parent, &name, bytes.len() as u64)?
                    .ok_or_else(|| BundleError::Corrupt("missing staged root".into()))?;
                crate::io_observer::note_bytes(
                    "rename_refusal_hook",
                    &namespace.identity.resolved_root,
                    Some(&attempted),
                );
                note("rename_refusal_injected", &namespace.identity.resolved_root);
            }
            Err(rustix::io::Errno::ACCESS)
        } else if inject_io_error {
            Err(rustix::io::Errno::IO)
        } else {
            fs::renameat(
                &namespace.parent,
                &name,
                &namespace.parent,
                &namespace.root_name,
            )
        };
        if let Err(error) = renamed {
            note("root_rename_refused", &namespace.identity.resolved_root);
            // Permission refusal is definite; other errors can leave publication ambiguous.
            if error != rustix::io::Errno::ACCESS {
                namespace.uncertain = true;
                return Err(BundleError::DurabilityUnknown);
            }
            return Err(error.into());
        }
        note("root_rename", &namespace.identity.resolved_root);
        namespace.uncertain = true;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("root_parent_sync", &namespace.identity.resolved_root)
            .map_err(|_| BundleError::DurabilityUnknown)?;
        sync_directory(
            &namespace.parent,
            namespace.identity.resolved_root.parent().unwrap(),
        )
        .map_err(|_| BundleError::DurabilityUnknown)?;
        namespace.uncertain = false;
        Ok(())
    })();
    if staged.is_err()
        && !namespace.uncertain
        && (fs::unlinkat(&namespace.parent, &name, AtFlags::empty()).is_err()
            || sync_directory(
                &namespace.parent,
                namespace.identity.resolved_root.parent().unwrap(),
            )
            .is_err())
    {
        namespace.uncertain = true;
        return Err(BundleError::DurabilityUnknown);
    }
    staged
}

fn directory(
    namespace: &ProjectNamespace,
    relative: &Path,
    create: bool,
) -> Result<File, BundleError> {
    let root = &namespace.identity.resolved_root;
    let mut assets = root
        .file_stem()
        .ok_or_else(|| BundleError::Identity("missing stem".into()))?
        .to_os_string();
    assets.push(".kinewright-assets");
    let path = Path::new(&assets).join(relative);
    let mut dir = namespace.parent.try_clone()?;
    let mut key = std::path::PathBuf::new();
    let mut observed = root.parent().unwrap().to_path_buf();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(BundleError::Identity("invalid owned path".into()));
        };
        key.push(name);
        observed.push(name);
        if let Some(cached) = namespace.directories.borrow().get(&key) {
            let current = fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
            let held = fs::fstat(cached)?;
            if current.st_dev != held.st_dev || current.st_ino != held.st_ino {
                return Err(BundleError::Identity(
                    "owned directory binding changed".into(),
                ));
            }
            dir = cached.try_clone()?;
            continue;
        }
        if namespace.directories.borrow().len() >= 128 {
            return Err(BundleError::Capacity("owned directory handles"));
        }
        let created = if create {
            match fs::mkdirat(&dir, name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
                Ok(()) => true,
                Err(rustix::io::Errno::EXIST) => false,
                Err(e) => return Err(e.into()),
            }
        } else {
            false
        };
        let child = File::from(fs::openat(&dir, name, DIR, Mode::empty())?);
        if fs::fstat(&child)?.st_dev != namespace.identity.parent_device {
            return Err(BundleError::Identity(
                "owned directory crosses the admitted filesystem".into(),
            ));
        }
        if created {
            note("directory_create", &observed);
        }
        if create {
            // Keep both barriers even if the first sync fails. EXIST may be an interrupted mkdir.
            namespace
                .pending_directory_syncs
                .borrow_mut()
                .insert(observed.clone(), (child.try_clone()?, false));
            namespace.pending_directory_syncs.borrow_mut().insert(
                observed.parent().unwrap().to_path_buf(),
                (dir.try_clone()?, true),
            );
            finish_directory_barriers(namespace)?;
        }
        namespace
            .directories
            .borrow_mut()
            .insert(key.clone(), child.try_clone()?);
        dir = child;
    }
    Ok(dir)
}

pub(super) fn publish(
    namespace: &mut ProjectNamespace,
    path: &PortablePath,
    bytes: &[u8],
) -> Result<(), BundleError> {
    namespace.verify()?;
    let path = path.as_path();
    let dir = directory(
        namespace,
        path.parent().unwrap_or_else(|| Path::new("")),
        true,
    )?;
    let name = path
        .file_name()
        .ok_or_else(|| BundleError::Identity("missing immutable name".into()))?;
    if let Some(existing) = read_at(&dir, name, bytes.len() as u64)? {
        if existing == bytes {
            return Ok(());
        }
        return Err(BundleError::Corrupt(
            "immutable name has different bytes".into(),
        ));
    }
    let (temp_name, mut file) = temp(namespace, &dir)?;
    let result = (|| {
        file.write_all(bytes)?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::note_bytes("object_stage", path, Some(bytes));
        sync_file(&file, "object_file_sync", path)?;
        fs::renameat_with(&dir, &temp_name, &dir, name, RenameFlags::NOREPLACE)?;
        note("object_rename", path);
        sync_object_directory(namespace, dir.try_clone()?, path.parent().unwrap())?;
        Ok(())
    })();
    if result.is_err()
        && !namespace.uncertain
        && (fs::unlinkat(&dir, &temp_name, AtFlags::empty()).is_err()
            || sync_directory(&dir, path.parent().unwrap()).is_err())
    {
        namespace.uncertain = true;
        return Err(BundleError::DurabilityUnknown);
    }
    result
}

pub(super) fn read_relative(
    namespace: &ProjectNamespace,
    path: &PortablePath,
    limit: u64,
) -> Result<Option<Vec<u8>>, BundleError> {
    let path = path.as_path();
    let dir = match directory(
        namespace,
        path.parent().unwrap_or_else(|| Path::new("")),
        false,
    ) {
        Ok(dir) => dir,
        Err(BundleError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    read_at(
        &dir,
        path.file_name()
            .ok_or_else(|| BundleError::Identity("missing name".into()))?,
        limit,
    )
}

pub(super) fn verify_directories(namespace: &ProjectNamespace) -> Result<(), BundleError> {
    let directories = namespace.directories.borrow();
    for (path, held) in directories.iter() {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
        let parent = if let Some(parent) = parent {
            directories
                .get(parent)
                .ok_or_else(|| BundleError::Identity("unpinned ancestor".into()))?
        } else {
            &namespace.parent
        };
        let current = fs::statat(parent, path.file_name().unwrap(), AtFlags::SYMLINK_NOFOLLOW)?;
        let held = fs::fstat(held)?;
        if current.st_dev != held.st_dev || current.st_ino != held.st_ino {
            return Err(BundleError::Identity("owned namespace moved".into()));
        }
    }
    Ok(())
}

pub(super) fn publish_with(
    namespace: &mut ProjectNamespace,
    path: &PortablePath,
    write: impl FnOnce(&mut File) -> Result<(), BundleError>,
) -> Result<(), BundleError> {
    namespace.verify()?;
    let path = path.as_path();
    let dir = directory(
        namespace,
        path.parent().unwrap_or_else(|| Path::new("")),
        true,
    )?;
    let name = path
        .file_name()
        .ok_or_else(|| BundleError::Identity("missing object name".into()))?;
    match fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err(BundleError::Corrupt(
                "unregistered immutable object already exists".into(),
            ));
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(e) => return Err(e.into()),
    }
    let (temp_name, mut file) = temp(namespace, &dir)?;
    let result = (|| {
        write(&mut file)?;
        sync_file(&file, "object_file_sync", path)?;
        namespace.verify()?;
        fs::renameat_with(&dir, &temp_name, &dir, name, RenameFlags::NOREPLACE)?;
        note("object_rename", path);
        sync_object_directory(namespace, dir.try_clone()?, path.parent().unwrap())?;
        Ok(())
    })();
    if result.is_err()
        && !namespace.uncertain
        && (fs::unlinkat(&dir, &temp_name, AtFlags::empty()).is_err()
            || sync_directory(&dir, path.parent().unwrap()).is_err())
    {
        namespace.uncertain = true;
        return Err(BundleError::DurabilityUnknown);
    }
    result
}

pub(super) fn open_relative(
    namespace: &ProjectNamespace,
    path: &PortablePath,
) -> Result<Option<File>, BundleError> {
    let path = path.as_path();
    let dir = match directory(
        namespace,
        path.parent().unwrap_or_else(|| Path::new("")),
        false,
    ) {
        Ok(dir) => dir,
        Err(BundleError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match fs::openat(
        &dir,
        path.file_name()
            .ok_or_else(|| BundleError::Identity("missing object name".into()))?,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(fd) => {
            let file = File::from(fd);
            if !file.metadata()?.is_file() {
                return Err(BundleError::Corrupt(
                    "owned object is not a regular file".into(),
                ));
            }
            Ok(Some(file))
        }
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn finish_directory_barriers(namespace: &ProjectNamespace) -> Result<(), BundleError> {
    let mut pending = namespace.pending_directory_syncs.borrow_mut();
    let mut paths: Vec<_> = pending.keys().cloned().collect();
    paths.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for path in paths {
        let (handle, parent) = &pending[&path];
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure(
            if *parent {
                "directory_parent_sync"
            } else {
                "directory_child_sync"
            },
            &path,
        )
        .map_err(|_| BundleError::DurabilityUnknown)?;
        #[cfg(not(any(test, feature = "test-util")))]
        let _ = parent;
        sync_directory(handle, &path).map_err(|_| BundleError::DurabilityUnknown)?;
        pending.remove(&path);
    }
    Ok(())
}
fn sync_object_directory(
    namespace: &mut ProjectNamespace,
    directory: File,
    path: &Path,
) -> Result<(), BundleError> {
    namespace
        .dirty_object_directories
        .insert(path.to_path_buf(), directory);
    if !namespace.batch_objects {
        finish_object_batch(namespace)?;
    }
    Ok(())
}
pub(super) fn finish_object_batch(namespace: &mut ProjectNamespace) -> Result<(), BundleError> {
    let paths: Vec<_> = namespace.dirty_object_directories.keys().cloned().collect();
    for path in paths {
        namespace.uncertain = true;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("object_directory_sync", &path)
            .map_err(|_| BundleError::DurabilityUnknown)?;
        sync_directory(&namespace.dirty_object_directories[&path], &path)
            .map_err(|_| BundleError::DurabilityUnknown)?;
        namespace.dirty_object_directories.remove(&path);
        namespace.uncertain = false;
    }
    namespace.batch_objects = false;
    Ok(())
}

fn sync_directory(directory: &File, observed: &Path) -> Result<(), BundleError> {
    #[cfg(any(test, feature = "test-util"))]
    crate::io_observer::measure_sync(|| fs::fsync(directory))?;
    #[cfg(not(any(test, feature = "test-util")))]
    fs::fsync(directory)?;
    note("directory_sync", observed);
    Ok(())
}

fn sync_file(file: &File, operation: &'static str, observed: &Path) -> Result<(), BundleError> {
    #[cfg(any(test, feature = "test-util"))]
    {
        crate::io_observer::check_failure(operation, observed)?;
        crate::io_observer::measure_sync(|| file.sync_all())?;
    }
    #[cfg(not(any(test, feature = "test-util")))]
    file.sync_all()?;
    note(operation, observed);
    Ok(())
}

pub(super) fn pin_directory(path: &Path) -> Result<File, BundleError> {
    if !path.is_absolute() {
        return Err(BundleError::Identity(
            "absolute resolved directory required".into(),
        ));
    }
    let mut directory = File::from(fs::open("/", DIR, Mode::empty())?);
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = File::from(fs::openat(&directory, name, DIR, Mode::empty())?);
            }
            _ => {
                return Err(BundleError::Identity(
                    "unresolved directory component".into(),
                ));
            }
        }
    }
    Ok(directory)
}
pub(super) fn check_profile(directory: &File) -> Result<(), BundleError> {
    if ![0x9123_683e, 0xef53, 0x5846_5342].contains(&fs::fstatfs(directory)?.f_type) {
        return Err(BundleError::DurabilityUnsupported);
    }
    Ok(())
}
pub(super) fn unlink_relative(
    namespace: &mut ProjectNamespace,
    path: &PortablePath,
) -> Result<(), BundleError> {
    namespace.verify()?;
    let path = path.as_path();
    let dir = directory(
        namespace,
        path.parent().unwrap_or_else(|| Path::new("")),
        false,
    )?;
    let name = path
        .file_name()
        .ok_or_else(|| BundleError::Identity("missing object name".into()))?;
    let stat = match fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return sync_directory(&dir, path.parent().unwrap()),
        Err(error) => return Err(error.into()),
    };
    if fs::FileType::from_raw_mode(stat.st_mode) != fs::FileType::RegularFile {
        return Err(BundleError::Identity(
            "retirement target is not an owned regular file".into(),
        ));
    }
    fs::unlinkat(&dir, name, AtFlags::empty())?;
    note("object_unlink", path);
    sync_object_directory(namespace, dir.try_clone()?, path.parent().unwrap())?;
    Ok(())
}

pub(super) fn verify_session(namespace: &ProjectNamespace) -> Result<(), BundleError> {
    let (base, name) = namespace
        .session
        .as_ref()
        .ok_or_else(|| BundleError::Identity("missing temporary authority".into()))?;
    let current = fs::statat(base, name, AtFlags::SYMLINK_NOFOLLOW)?;
    let opened = fs::fstat(&namespace.parent)?;
    if current.st_dev != opened.st_dev || current.st_ino != opened.st_ino {
        return Err(BundleError::Identity(
            "temporary root binding changed".into(),
        ));
    }
    Ok(())
}
pub(super) fn create_session(
    base: &Path,
    session_id: &crate::bundle_records::SessionId,
) -> Result<ProjectNamespace, BundleError> {
    let base_path = base.canonicalize()?;
    let base = pin_directory(&base_path)?;
    check_profile(&base)?;
    let name: std::ffi::OsString = format!(".kinewright-session-{}", session_id.as_str()).into();
    fs::mkdirat(&base, &name, Mode::RUSR | Mode::WUSR | Mode::XUSR)?;
    let parent = File::from(fs::openat(
        &base,
        &name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    fs::fsync(&parent)?;
    fs::fsync(&base)?;
    let stat = fs::fstat(&parent)?;
    Ok(ProjectNamespace {
        identity: NamespaceIdentity {
            resolved_root: base_path.join(&name).join("session.kinewright"),
            parent_device: stat.st_dev,
            parent_inode: stat.st_ino,
        },
        parent,
        root_name: "session.kinewright".into(),
        directories: std::cell::RefCell::default(),
        lock: None,
        pending_directory_syncs: std::cell::RefCell::default(),
        dirty_object_directories: std::collections::BTreeMap::new(),
        batch_objects: false,
        uncertain: false,
        read_only: false,
        session: Some((base, name)),
    })
}
pub(super) fn cleanup_session(namespace: &mut ProjectNamespace) -> Result<(), BundleError> {
    let Some((base, name)) = namespace.session.as_ref() else {
        return Ok(());
    };
    if namespace.uncertain {
        return Err(BundleError::Unsettled);
    }
    verify_session(namespace)?;
    verify_directories(namespace)?;
    match fs::unlinkat(&namespace.parent, &namespace.root_name, AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => {}
        Err(e) => return Err(e.into()),
    }
    let directories = namespace.directories.get_mut();
    let mut names = directories.keys().cloned().collect::<Vec<_>>();
    names.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for path in names {
        let held_parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| {
                directories
                    .get(p)
                    .ok_or_else(|| BundleError::Identity("missing temporary parent".into()))
            })
            .transpose()?
            .unwrap_or(&namespace.parent);
        fs::unlinkat(held_parent, path.file_name().unwrap(), AtFlags::REMOVEDIR)?;
        directories.remove(&path);
    }
    fs::fsync(&namespace.parent)?;
    fs::unlinkat(base, name, AtFlags::REMOVEDIR)?;
    fs::fsync(base)?;
    namespace.session = None;
    Ok(())
}

pub(super) fn replace_relative(
    namespace: &mut ProjectNamespace,
    path: &PortablePath,
    bytes: &[u8],
) -> Result<(), BundleError> {
    namespace.verify()?;
    let dir = directory(
        namespace,
        path.as_path().parent().unwrap_or_else(|| Path::new("")),
        true,
    )?;
    let name = path
        .as_path()
        .file_name()
        .ok_or_else(|| BundleError::Identity("missing journal name".into()))?;
    let (temporary, mut file) = temp(namespace, &dir)?;
    let result = (|| {
        file.write_all(bytes)?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::note_bytes("inventory_stage", path.as_path(), Some(bytes));
        sync_file(&file, "inventory_file_sync", path.as_path())?;
        namespace.verify()?;
        namespace.uncertain = true;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("inventory_before_rename", path.as_path())?;
        fs::renameat(&dir, &temporary, &dir, name).map_err(|_| BundleError::DurabilityUnknown)?;
        note("inventory_rename", path.as_path());
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("inventory_after_rename", path.as_path())
            .map_err(|_| BundleError::DurabilityUnknown)?;
        namespace.uncertain = true;
        sync_directory(&dir, path.as_path().parent().unwrap())
            .map_err(|_| BundleError::DurabilityUnknown)?;
        namespace.uncertain = false;
        Ok(())
    })();
    if result.is_err()
        && !namespace.uncertain
        && (fs::unlinkat(&dir, &temporary, AtFlags::empty()).is_err()
            || sync_directory(&dir, path.as_path().parent().unwrap()).is_err())
    {
        namespace.uncertain = true;
        return Err(BundleError::DurabilityUnknown);
    }
    result
}
pub(super) fn reconcile(
    namespace: &mut ProjectNamespace,
    expected: &[u8],
) -> Result<(), BundleError> {
    reconcile_copy_root(namespace, Some(expected))
}
pub(super) fn seal_observed_root(
    namespace: &ProjectNamespace,
    expected: Option<&[u8]>,
) -> Result<(), BundleError> {
    verify_parent(namespace)?;
    if namespace.read_root()?.as_deref() != expected {
        return Err(BundleError::DurabilityUnknown);
    }
    sync_directory(
        &namespace.parent,
        namespace.identity.resolved_root.parent().unwrap(),
    )?;
    verify_parent(namespace)?;
    if namespace.read_root()?.as_deref() != expected {
        return Err(BundleError::DurabilityUnknown);
    }
    Ok(())
}
pub(super) fn reconcile_copy_root(
    namespace: &mut ProjectNamespace,
    expected: Option<&[u8]>,
) -> Result<(), BundleError> {
    namespace.uncertain = false;
    let result = (|| {
        namespace.verify()?;
        if namespace.read_root()?.as_deref() != expected {
            return Err(BundleError::Corrupt("reconciliation root changed".into()));
        }
        sync_directory(
            &namespace.parent,
            namespace.identity.resolved_root.parent().unwrap(),
        )?;
        Ok(())
    })();
    namespace.uncertain = result.is_err();
    result
}

/// Enumerate only the pinned managed directories and this root's temporary prefix.
#[allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "exact generated names define temporary ownership"
)]
pub(super) fn physical_files(
    namespace: &ProjectNamespace,
    limit: u64,
) -> Result<Vec<super::PhysicalFile>, BundleError> {
    use std::os::fd::AsRawFd;
    namespace.verify_read_authority()?;
    let prefix = temp_prefix(namespace)?;
    let mut files = vec![];
    for relative in [
        None,
        Some(Path::new("studio/objects")),
        Some(Path::new("studio/recovery")),
    ] {
        let dir = if let Some(relative) = relative {
            match directory(namespace, relative, false) {
                Ok(dir) => dir,
                Err(BundleError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            }
        } else {
            namespace.parent.try_clone()?
        };
        for entry in std::fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))? {
            let name = entry?.file_name();
            let text = name
                .to_str()
                .ok_or_else(|| BundleError::Identity("managed non-UTF8 filename".into()))?;
            if relative.is_none() && !(text.starts_with(&prefix) && text.ends_with(".tmp")) {
                continue;
            }
            if relative == Some(Path::new("studio/recovery")) && text == "inventory" {
                continue;
            }
            if files.len() as u64 >= limit {
                return Err(BundleError::Capacity("physical file count"));
            }
            let file = File::from(fs::openat(
                &dir,
                &name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?);
            let stat = fs::fstat(&file)?;
            if !file.metadata()?.is_file()
                || stat.st_nlink != 1
                || stat.st_dev != namespace.identity.parent_device
            {
                return Err(BundleError::Identity("managed file identity/type".into()));
            }
            files.push(super::PhysicalFile {
                relative: relative
                    .map_or_else(|| Path::new(&name).to_path_buf(), |p| p.join(&name)),
                root_parent: relative.is_none(),
                length: file.metadata()?.len(),
            });
        }
    }
    Ok(files)
}
#[allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "case folding would widen deletion authority"
)]
pub(super) fn retire_temporary(
    namespace: &mut ProjectNamespace,
    file: &super::PhysicalFile,
) -> Result<(), BundleError> {
    namespace.verify()?;
    let name = file
        .relative
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| BundleError::Identity("temporary filename".into()))?;
    if !name.starts_with(&temp_prefix(namespace)?) || !name.ends_with(".tmp") {
        return Err(BundleError::Identity(
            "unclassified immutable bytes cannot be retired as temporary".into(),
        ));
    }
    if file.root_parent {
        fs::unlinkat(&namespace.parent, name, AtFlags::empty())?;
        sync_directory(
            &namespace.parent,
            namespace.identity.resolved_root.parent().unwrap(),
        )
    } else {
        unlink_relative(namespace, &PortablePath::new(&file.relative)?)
    }
}

pub(super) fn seal_existing(
    namespace: &mut ProjectNamespace,
    path: &PortablePath,
) -> Result<(), BundleError> {
    namespace.verify()?;
    let file = open_relative(namespace, path)?
        .ok_or_else(|| BundleError::Corrupt("missing recovered immutable file".into()))?;
    sync_file(&file, "object_file_sync", path.as_path())?;
    let mut relative = path.as_path().parent();
    while let Some(directory_path) = relative {
        let handle = directory(namespace, directory_path, false)?;
        let observed = if directory_path.as_os_str().is_empty() {
            namespace
                .identity
                .resolved_root
                .with_extension("kinewright-assets")
        } else {
            directory_path.to_path_buf()
        };
        sync_directory(&handle, &observed)?;
        if directory_path.as_os_str().is_empty() {
            break;
        }
        relative = directory_path.parent();
    }
    sync_directory(
        &namespace.parent,
        namespace.identity.resolved_root.parent().unwrap(),
    )?;
    Ok(())
}
