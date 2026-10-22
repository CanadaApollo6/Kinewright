//! Stable std subset only. These handles never grant S1 Studio write authority.
use super::ProjectNamespace;
use crate::bundle::BundleError;
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf, Prefix},
};
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub(super) fn pin_parents(path: &Path) -> Result<Vec<File>, BundleError> {
    if !path.is_absolute() {
        return Err(BundleError::Identity("absolute local path required".into()));
    }
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
        _ => return Err(BundleError::Identity("device/network path refused".into())),
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(BundleError::Identity("traversal refused".into()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| BundleError::Identity("missing parent".into()))?;
    let mut spelling = PathBuf::new();
    let mut handles = Vec::new();
    for component in parent.components() {
        spelling.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let handle = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&spelling)?;
        let metadata = handle.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(BundleError::Identity(
                "reparse or non-directory ancestor".into(),
            ));
        }
        handles.push(handle);
    }
    Ok(handles)
}

pub(super) fn refuse(path: &Path) -> Result<ProjectNamespace, BundleError> {
    let _parents = pin_parents(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            // Stable std cannot prove root file identity or its hard-link count.
            Err(BundleError::Identity(
                "existing root by-handle identity is unprovable with stable std".into(),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(BundleError::DurabilityUnsupported)
        }
        Err(e) => Err(e.into()),
    }
}
