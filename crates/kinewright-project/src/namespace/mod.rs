//! Authority-bound Studio IO. Caller spellings stop at acquisition.
#![allow(clippy::missing_errors_doc)]
use crate::{LockfileHandle, bundle::BundleError};
use std::path::{Component, Path, PathBuf};
#[cfg(target_os = "linux")]
use std::{cell::RefCell, collections::BTreeMap, ffi::OsString, fs::File};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceIdentity {
    pub resolved_root: PathBuf,
    pub parent_device: u64,
    pub parent_inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityProfile {
    LinuxLocal,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortablePath(PathBuf);
impl PortablePath {
    pub fn new(path: &Path) -> Result<Self, BundleError> {
        let spelling = path
            .to_str()
            .ok_or_else(|| BundleError::Identity("non-UTF8 path".into()))?;
        if spelling.is_empty()
            || spelling.split(['/', '\\']).count() > 32
            || spelling
                .split(['/', '\\'])
                .any(|c| c.is_empty() || c == "." || c == ".." || c.len() > 255)
        {
            return Err(BundleError::Identity(
                "invalid portable path spelling".into(),
            ));
        }
        for component in path.components() {
            let Component::Normal(name) = component else {
                return Err(BundleError::Identity("non-relative path".into()));
            };
            let name = name
                .to_str()
                .ok_or_else(|| BundleError::Identity("non-UTF8 portable component".into()))?;
            let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
            if name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
                || name.ends_with(['.', ' '])
                || name.chars().any(char::is_control)
                || matches!(
                    base.as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "CONIN$"
                        | "CONOUT$"
                        | "COM¹"
                        | "COM²"
                        | "COM³"
                        | "LPT¹"
                        | "LPT²"
                        | "LPT³"
                )
                || (base.len() == 4
                    && (base.starts_with("COM") || base.starts_with("LPT"))
                    && base.as_bytes()[3].is_ascii_digit())
            {
                return Err(BundleError::Identity("nonportable component".into()));
            }
        }
        Ok(Self(path.to_path_buf()))
    }
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl serde::Serialize for PortablePath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0
            .components()
            .map(|c| c.as_os_str().to_str().expect("validated UTF-8 component"))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for PortablePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let components = Vec::<String>::deserialize(deserializer)?;
        if components.is_empty()
            || components.len() > 32
            || components
                .iter()
                .any(|c| c.is_empty() || c.len() > 255 || c.contains(['/', '\\']))
        {
            return Err(serde::de::Error::custom("invalid portable path components"));
        }
        let path: PathBuf = components.iter().collect();
        Self::new(&path).map_err(serde::de::Error::custom)
    }
}

pub struct ProjectNamespace {
    identity: NamespaceIdentity,
    #[cfg(target_os = "linux")]
    parent: File,
    #[cfg(target_os = "linux")]
    session: Option<(File, OsString)>,
    #[cfg(target_os = "linux")]
    root_name: OsString,
    #[cfg(target_os = "linux")]
    directories: RefCell<BTreeMap<PathBuf, File>>,
    lock: Option<LockfileHandle>,
    uncertain: bool,
}
impl ProjectNamespace {
    pub(crate) fn create_session(
        parent: &Path,
        session_id: &crate::bundle_records::SessionId,
    ) -> Result<Self, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::create_session(parent, session_id)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (parent, session_id);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn cleanup_session(&mut self) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::cleanup_session(self)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(())
        }
    }
    pub(crate) fn is_uncertain(&self) -> bool {
        self.uncertain
    }
    pub(crate) fn unlink_owned(&mut self, path: &PortablePath) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::unlink_relative(self, path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub fn acquire(path: &Path) -> Result<Self, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::acquire(path)
        }
        #[cfg(windows)]
        {
            windows::refuse(path)
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = path;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    #[must_use]
    pub fn identity(&self) -> &NamespaceIdentity {
        &self.identity
    }
    #[must_use]
    pub fn profile(&self) -> DurabilityProfile {
        if cfg!(target_os = "linux") {
            DurabilityProfile::LinuxLocal
        } else {
            DurabilityProfile::Unsupported
        }
    }
    pub fn verify(&self) -> Result<(), BundleError> {
        if self.uncertain {
            return Err(BundleError::Unsettled);
        }
        if let Some(lock) = self.lock.as_ref() {
            lock.verify()
                .map_err(|e| BundleError::Identity(e.to_string()))?;
        } else {
            #[cfg(target_os = "linux")]
            {
                linux::verify_session(self)?;
            }
            #[cfg(not(target_os = "linux"))]
            {
                return Err(BundleError::Identity("released namespace".into()));
            }
        }
        #[cfg(target_os = "linux")]
        linux::verify_directories(self)?;
        Ok(())
    }
    pub fn read_root(&self) -> Result<Option<Vec<u8>>, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::read_at(&self.parent, &self.root_name, 64 * 1024 * 1024)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub fn replace_root(&mut self, bytes: &[u8]) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::replace(self, bytes)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = bytes;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub fn publish_immutable(
        &mut self,
        name: &PortablePath,
        bytes: &[u8],
    ) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::publish(self, name, bytes)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (name, bytes);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub fn read(&self, name: &PortablePath, limit: u64) -> Result<Option<Vec<u8>>, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::read_relative(self, name, limit)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (name, limit);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn replace_root_probe(&mut self, bytes: &[u8]) -> Result<(), BundleError> {
        self.replace_root(bytes)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn publish_immutable_probe(
        &mut self,
        path: &PortablePath,
        bytes: &[u8],
    ) -> Result<(), BundleError> {
        self.publish_immutable(path, bytes)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn replace_root_refusing_rename(&mut self, bytes: &[u8]) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::replace_inner(self, bytes, true)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = bytes;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    #[cfg(all(windows, any(test, feature = "test-util")))]
    pub fn inspect_windows_parents(path: &Path) -> Result<Vec<std::fs::File>, BundleError> {
        windows::pin_parents(path)
    }
    pub fn publish_with(
        &mut self,
        path: &PortablePath,
        write: impl FnOnce(&mut std::fs::File) -> Result<(), BundleError>,
    ) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::publish_with(self, path, write)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, write);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub fn open_owned(&self, path: &PortablePath) -> Result<Option<std::fs::File>, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::open_relative(self, path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub fn available_bytes(&self) -> Result<u64, BundleError> {
        #[cfg(target_os = "linux")]
        {
            let stat = rustix::fs::fstatvfs(&self.parent)?;
            stat.f_bavail
                .checked_mul(stat.f_frsize)
                .ok_or(BundleError::Capacity("disk size overflow"))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub fn release(&mut self) -> Result<(), BundleError> {
        if self.uncertain {
            return Err(BundleError::Unsettled);
        }
        if let Some(lock) = self.lock.take() {
            lock.release()?;
        }
        Ok(())
    }
}

pub(super) fn note(operation: &'static str, path: &Path) {
    #[cfg(any(test, feature = "test-util"))]
    crate::io_observer::note(operation, path);
    #[cfg(not(any(test, feature = "test-util")))]
    let _ = (operation, path);
}

impl Drop for ProjectNamespace {
    fn drop(&mut self) {
        if self.cleanup_session().is_err() {
            eprintln!("kinewright: temporary namespace retained for reconciliation");
        }
    }
}
