//! Authority-bound Studio IO. Caller spellings stop at acquisition.
#![allow(clippy::missing_errors_doc)]
use crate::{LockfileHandle, bundle::BundleError};
use std::path::{Component, Path, PathBuf};
mod file_identity;
pub(crate) use file_identity::FileIdentity;
pub(crate) use file_identity::{file_matches_path, paths_match};
#[cfg(target_os = "linux")]
use std::{cell::RefCell, collections::BTreeMap, ffi::OsString, fs::File};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamespaceIdentity {
    pub resolved_root: PathBuf,
    pub parent_device: u64,
    pub parent_inode: u64,
}

/// Read-only, pinned admission. Creating parents or claiming a writer is a later operation.
pub struct DestinationInspection {
    path: PathBuf,
    #[cfg(target_os = "linux")]
    ancestor: PathBuf,
    #[cfg(target_os = "linux")]
    parent: File,
}
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "uncertified profiles retain the Linux read-only refusal API"
    )
)]
impl DestinationInspection {
    pub(crate) fn inspect_borrowed(
        path: &Path,
        sources: &[&ProjectNamespace],
    ) -> Result<Self, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::inspect_copy_destination(path, sources, false)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, sources);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    /// Wire-safe identity of this pinned target admission; contains no source locator.
    pub fn identity(&self) -> Result<NamespaceIdentity, BundleError> {
        #[cfg(target_os = "linux")]
        {
            let stat = rustix::fs::fstat(&self.parent)?;
            Ok(NamespaceIdentity {
                resolved_root: self.path.clone(),
                parent_device: stat.st_dev,
                parent_inode: stat.st_ino,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn inspect(path: &Path, sources: &[&ProjectNamespace]) -> Result<Self, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::inspect_rescue_destination(path, sources)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, sources);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn revalidate(&self, sources: &[&ProjectNamespace]) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::revalidate_rescue_destination(self, sources)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = sources;
            Err(BundleError::DurabilityUnsupported)
        }
    }
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

/// The root boundary is independent of temporary-file cleanup uncertainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootReplaceBoundary {
    NeverAttempted,
    #[cfg_attr(
        not(target_os = "linux"),
        allow(
            dead_code,
            reason = "only the Linux namespace can observe a definite rename refusal"
        )
    )]
    DefinitelyRefused,
    Unknown,
}
#[derive(Debug)]
pub(crate) struct RootReplaceFailure {
    pub(crate) boundary: RootReplaceBoundary,
    pub(crate) error: BundleError,
}
impl From<BundleError> for RootReplaceFailure {
    fn from(error: BundleError) -> Self {
        Self {
            boundary: RootReplaceBoundary::NeverAttempted,
            error,
        }
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "writer/read policy, object batching and once-only session retirement are independent namespace facts"
)]
pub struct ProjectNamespace {
    identity: NamespaceIdentity,
    #[cfg(target_os = "linux")]
    parent: File,
    #[cfg(target_os = "linux")]
    probe_failure: RefCell<Option<ProbeFailure>>,
    #[cfg(target_os = "linux")]
    session: Option<(File, OsString)>,
    #[cfg(target_os = "linux")]
    session_unlinked: bool,
    #[cfg(target_os = "linux")]
    root_name: OsString,
    #[cfg(target_os = "linux")]
    pending_root_cleanup: Option<OsString>,
    #[cfg(target_os = "linux")]
    directories: RefCell<BTreeMap<PathBuf, File>>,
    #[cfg(target_os = "linux")]
    pending_directory_syncs: RefCell<BTreeMap<PathBuf, (File, bool)>>,
    #[cfg(target_os = "linux")]
    dirty_object_directories: BTreeMap<PathBuf, File>,
    #[cfg(target_os = "linux")]
    batch_objects: bool,
    lock: Option<LockfileHandle>,
    uncertain: bool,
    read_only: bool,
    read_authority: Option<kinewright_media::owned_io::ReadAuthority>,
}
#[cfg(target_os = "linux")]
type ProbeFailure = (
    crate::bundle_rescue::ReconcilePhase,
    &'static str,
    Option<BundleError>,
);

#[derive(Debug, Clone)]
pub(crate) struct PhysicalFile {
    pub(crate) relative: PathBuf,
    pub(crate) root_parent: bool,
    pub(crate) length: u64,
}

/// Pins the destination parent, including an ancestor claim when creating parents.
pub(crate) struct PreparedDestination {
    pub(crate) path: PathBuf,
    #[cfg(target_os = "linux")]
    parent: File,
    claim: Option<ProjectNamespace>,
}
impl Drop for PreparedDestination {
    fn drop(&mut self) {
        if let Some(claim) = &mut self.claim {
            let _ = claim.release();
        }
    }
}
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "uncertified platform stubs retain the Linux authority API"
    )
)]
impl ProjectNamespace {
    pub(crate) fn lend_read_authority(
        &mut self,
        authority: kinewright_media::owned_io::ReadAuthority,
    ) -> Result<(), BundleError> {
        if !self.read_only || self.lock.is_some() {
            return Err(BundleError::Identity(
                "snapshot reader must be unclaimed".into(),
            ));
        }
        self.read_authority = Some(authority);
        self.verify_read_authority()
    }
    pub(crate) fn read_authority(&self) -> Option<kinewright_media::owned_io::ReadAuthority> {
        self.read_authority.clone()
    }

    pub(crate) fn probe_retained_namespace(&self) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::verify_parent(self)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn clear_probe_failure(&self) {
        #[cfg(target_os = "linux")]
        self.probe_failure.replace(None);
    }
    #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
    pub(crate) fn defer_parent_barrier_probe(&self) -> Result<(), BundleError> {
        self.pending_directory_syncs.borrow_mut().insert(
            self.identity
                .resolved_root
                .parent()
                .ok_or(BundleError::HistoryBarrierInvariant)?
                .to_path_buf(),
            (self.parent.try_clone()?, true),
        );
        Ok(())
    }
    #[cfg(target_os = "linux")]
    fn probe_operation(
        &self,
        phase: crate::bundle_rescue::ReconcilePhase,
        operation: &'static str,
    ) {
        self.probe_failure.replace(Some((phase, operation, None)));
    }
    #[cfg(target_os = "linux")]
    fn probe_uncertainty(&self, error: BundleError) -> BundleError {
        if let Some((_, _, original)) = &mut *self.probe_failure.borrow_mut() {
            *original = Some(error);
        }
        BundleError::DurabilityUnknown
    }
    pub(crate) fn take_probe_failure(
        &self,
        error: BundleError,
    ) -> (
        Option<(crate::bundle_rescue::ReconcilePhase, &'static str)>,
        BundleError,
    ) {
        #[cfg(target_os = "linux")]
        if let Some((phase, operation, original)) = self.probe_failure.take() {
            return (Some((phase, operation)), original.unwrap_or(error));
        }
        (None, error)
    }
    pub(crate) fn physical_files(&self, limit: u64) -> Result<Vec<PhysicalFile>, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::physical_files(self, limit)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = limit;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn retire_temporary(&mut self, file: &PhysicalFile) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::retire_temporary(self, file)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = file;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn prepare_destination(path: &Path) -> Result<PreparedDestination, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::prepare_destination(path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn prepare_rescue_destination(
        inspection: &DestinationInspection,
        on_mutation: &mut dyn FnMut() -> Result<(), BundleError>,
    ) -> Result<PreparedDestination, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::prepare_destination_admitted(inspection.path(), Some(inspection), on_mutation)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (inspection, on_mutation);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn seal_destination_ancestors(&self) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::seal_destination_ancestors(self)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn matches_destination(
        &self,
        prepared: &PreparedDestination,
    ) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::matches_destination(self, prepared)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = prepared;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn has_asset_store(&self) -> Result<bool, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::has_asset_store(self)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn probe_copy_contention(&self) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::probe_copy_contention(self)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn cleanup_empty_copy_store(&mut self) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::cleanup_empty_copy_store(self)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BundleError::DurabilityUnsupported)
        }
    }
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
        self.uncertain || {
            #[cfg(target_os = "linux")]
            {
                !self.pending_directory_syncs.borrow().is_empty()
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn unsettled_probe(&self) -> bool {
        self.is_uncertain()
    }
    pub(crate) fn object_batch_active(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.batch_objects
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
    pub(crate) fn begin_object_batch(&mut self) -> Result<(), BundleError> {
        self.verify()?;
        #[cfg(target_os = "linux")]
        {
            self.batch_objects = true;
        }
        Ok(())
    }
    pub(crate) fn finish_object_batch(&mut self) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::finish_object_batch(self)?;
        }
        Ok(())
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
    pub(crate) fn inspect(path: &Path) -> Result<Self, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::inspect(path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn claim(&mut self) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::claim(self)
        }
        #[cfg(not(target_os = "linux"))]
        {
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
        if self.read_only {
            return Err(BundleError::Identity("read-only namespace".into()));
        }
        if self.uncertain {
            return Err(BundleError::Unsettled);
        }
        self.verify_read_authority()?;
        #[cfg(target_os = "linux")]
        linux::finish_directory_barriers(self)?;
        Ok(())
    }
    pub(crate) fn verify_read_authority(&self) -> Result<(), BundleError> {
        if self.read_only {
            self.read_authority
                .as_ref()
                .ok_or_else(|| BundleError::Identity("unleased read-only namespace".into()))?
                .check()
                .map_err(|e| BundleError::Identity(e.to_string()))?;
            #[cfg(target_os = "linux")]
            {
                linux::verify_parent(self)?;
                linux::verify_directories(self)?;
            }
            return Ok(());
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
        {
            linux::verify_parent(self)?;
            linux::verify_directories(self)?;
        }
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
    pub(crate) fn seal_observed_root(&self, expected: Option<&[u8]>) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::seal_observed_root(self, expected)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = expected;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn verify_admitted_root(&self, expected: &[u8]) -> Result<(), BundleError> {
        self.verify()?;
        #[cfg(target_os = "linux")]
        {
            if linux::read_at(&self.parent, &self.root_name, expected.len() as u64)?.as_deref()
                != Some(expected)
            {
                return Err(BundleError::Identity(
                    "source root changed after admission".into(),
                ));
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = expected;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn seal_existing(&mut self, path: &PortablePath) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::seal_existing(self, path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn replace_relative(
        &mut self,
        path: &PortablePath,
        bytes: &[u8],
    ) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::replace_relative(self, path, bytes)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, bytes);
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn reconcile_copy_root(
        &mut self,
        expected: Option<&[u8]>,
    ) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::reconcile_copy_root(self, expected)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = expected;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn reconcile(&mut self, expected: &[u8]) -> Result<(), BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::reconcile(self, expected)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = expected;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn replace_root(&mut self, bytes: &[u8]) -> Result<(), RootReplaceFailure> {
        #[cfg(target_os = "linux")]
        {
            linux::replace(self, bytes)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = bytes;
            Err(BundleError::DurabilityUnsupported.into())
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn publish_immutable(
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
    pub(crate) fn read(
        &self,
        name: &PortablePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, BundleError> {
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
        self.replace_root(bytes).map_err(|failure| failure.error)
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
            linux::replace_inner(self, bytes, true).map_err(|failure| failure.error)
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
    pub(crate) fn publish_hashed(
        &mut self,
        write: impl FnOnce(&mut std::fs::File) -> Result<crate::bundle_records::ObjectRef, BundleError>,
    ) -> Result<crate::bundle_records::ObjectRef, BundleError> {
        #[cfg(target_os = "linux")]
        {
            linux::publish_hashed(self, write)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = write;
            Err(BundleError::DurabilityUnsupported)
        }
    }
    pub(crate) fn publish_with(
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
    pub(crate) fn open_owned(
        &self,
        path: &PortablePath,
    ) -> Result<Option<std::fs::File>, BundleError> {
        if self.read_authority.is_some() {
            self.verify_read_authority()?;
        }
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
    pub(crate) fn available_bytes(&self) -> Result<u64, BundleError> {
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
        if self.is_uncertain() {
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
    {
        crate::io_observer::note(operation, path);
        crate::test_hook(operation);
    }
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
