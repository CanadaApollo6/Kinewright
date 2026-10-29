//! Revocable GUI access to a host-owned immutable snapshot graph.
use kinewright_agent::writer_host::{
    HostClient, HostCommand, SnapshotLeaseError, SnapshotLeaseRemote,
};
use kinewright_core::MediaSourceOrigin;
use kinewright_media::owned_io::ReadAuthority;
use kinewright_project::{
    bundle::BundleError,
    bundle_records::ObjectRef,
    storage_budget::StorageLimits,
    store::snapshot_reader::{CurrentEditReader, HostSnapshotReader, SnapshotReadSession},
    studio_records::SnapshotKey,
};
use std::{
    path::Path,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct SnapshotReaders {
    session: SnapshotReadSession,
    live: Mutex<Vec<Weak<LeaseLifetime>>>,
}
struct LeaseLifetime {
    origin: MediaSourceOrigin,
    remote: SnapshotLeaseRemote,
    context: Weak<AtomicBool>,
    released: AtomicBool,
}
impl LeaseLifetime {
    fn check(&self) -> Result<(), kinewright_core::MediaError> {
        let result = if self
            .context
            .upgrade()
            .is_some_and(|alive| alive.load(Ordering::Acquire))
        {
            self.remote.check_cached()
        } else {
            Err(SnapshotLeaseError::Ended)
        };
        lease_read_result(&self.origin, result)
    }
    fn revoke(&self) {
        self.origin.revoke_legacy_media();
        if !self.released.swap(true, Ordering::AcqRel) {
            let _ = self.remote.release();
        }
    }
}
fn lease_read_result(
    origin: &MediaSourceOrigin,
    result: Result<(), SnapshotLeaseError>,
) -> Result<(), kinewright_core::MediaError> {
    match result {
        Ok(()) => Ok(()),
        Err(SnapshotLeaseError::TemporarilyUnavailable) => {
            Err(ReadAuthority::temporarily_unavailable())
        }
        Err(SnapshotLeaseError::Ended) => {
            origin.revoke_legacy_media();
            Err(kinewright_core::MediaError::MediaAdmissionRequired)
        }
    }
}
impl Drop for LeaseLifetime {
    fn drop(&mut self) {
        self.revoke();
    }
}
impl SnapshotReaders {
    pub(super) fn new() -> Result<Self, BundleError> {
        Ok(Self {
            session: SnapshotReadSession::new(StorageLimits::default())?,
            live: Mutex::new(vec![]),
        })
    }
    pub(super) fn open(
        &self,
        client: &HostClient,
        path: &Path,
        context: Weak<AtomicBool>,
        key: &SnapshotKey,
    ) -> Result<HostSnapshotReader, BundleError> {
        let mut live = self.admit()?;
        let reply = client
            .request(HostCommand::LeaseSnapshot { key: key.clone() })
            .map_err(cause)?;
        if let Some(error) = reply.error {
            return Err(BundleError::Identity(error.message));
        }
        let lease = reply.snapshot_lease.ok_or(BundleError::HistoryChanged)?;
        let (lifetime, authority) =
            Self::lifetime(client, &lease.id, &lease.payload_sha256, context)?;
        if lease.descriptor.key != *key {
            return Err(BundleError::HistoryChanged);
        }
        let reader = self.session.open(path, &lease, authority)?;
        live.push(Arc::downgrade(&lifetime));
        Ok(reader)
    }
    /// Leases the host's current edit inputs named by `scope`, the owner epoch
    /// and input manifest from the host's status (S3 C11).
    pub(super) fn open_current(
        &self,
        client: &HostClient,
        path: &Path,
        context: Weak<AtomicBool>,
        scope: &(u64, ObjectRef),
    ) -> Result<CurrentEditReader, BundleError> {
        let mut live = self.admit()?;
        // A lost reply, a moved scope or a busy host is temporary: the Studio
        // asks again on a newer scope or after a bounded wait (S3 C11 fu1).
        let reply = client
            .request(HostCommand::LeaseCurrentEdit {
                inputs: scope.1.clone(),
            })
            .map_err(|_| BundleError::Unsettled)?;
        if let Some(error) = reply.error {
            return Err(match error.code.as_str() {
                "history_changed" => BundleError::HistoryChanged,
                "unsettled" | "save_in_progress" => BundleError::Unsettled,
                _ => BundleError::Identity(error.message),
            });
        }
        let lease = reply
            .current_edit_lease
            .ok_or(BundleError::HistoryChanged)?;
        let (lifetime, authority) =
            Self::lifetime(client, &lease.id, &lease.payload_sha256, context)?;
        if (lease.owner_epoch, &lease.inputs) != (scope.0, &scope.1) {
            return Err(BundleError::HistoryChanged);
        }
        let reader = self.session.open_current(path, &lease, authority)?;
        live.push(Arc::downgrade(&lifetime));
        Ok(reader)
    }
    fn admit(&self) -> Result<std::sync::MutexGuard<'_, Vec<Weak<LeaseLifetime>>>, BundleError> {
        let mut live = self
            .live
            .lock()
            .map_err(|_| BundleError::Capacity("snapshot readers poisoned"))?;
        live.retain(|reader| reader.strong_count() != 0);
        if live.len() >= 64 {
            return Err(BundleError::Capacity("64 open snapshot readers"));
        }
        Ok(live)
    }
    /// The granted lease's lifetime: dropping it, or any failure before a
    /// reader holds it, releases the lease.
    fn lifetime(
        client: &HostClient,
        id: &str,
        payload_sha256: &str,
        context: Weak<AtomicBool>,
    ) -> Result<(Arc<LeaseLifetime>, ReadAuthority), BundleError> {
        let remote = match client.snapshot_lease_remote(id, payload_sha256) {
            Ok(remote) => remote,
            Err(error) => {
                let _ = client.request(HostCommand::ReleaseSnapshot {
                    id: id.into(),
                    payload_sha256: payload_sha256.into(),
                });
                return Err(cause(error));
            }
        };
        let lifetime = Arc::new(LeaseLifetime {
            origin: MediaSourceOrigin::new_project(),
            remote,
            context,
            released: AtomicBool::new(false),
        });
        let grant = lifetime
            .origin
            .legacy_media()
            .ok_or(BundleError::HistoryChanged)?;
        let checker = Arc::clone(&lifetime);
        let authority = ReadAuthority::new(
            grant,
            payload_sha256.into(),
            Arc::new(move || checker.check()),
        );
        Ok((lifetime, authority))
    }
    pub(super) fn revoke_all(&self) {
        if let Ok(mut live) = self.live.lock() {
            for reader in live.drain(..).filter_map(|reader| reader.upgrade()) {
                reader.revoke();
            }
        }
    }
}
impl Drop for SnapshotReaders {
    fn drop(&mut self) {
        self.revoke_all();
    }
}
fn cause(error: impl std::fmt::Display) -> BundleError {
    BundleError::Corrupt(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fix2_temporary_lease_failure_preserves_grant_and_ended_revokes() {
        let origin = MediaSourceOrigin::new_project();
        let grant = origin.legacy_media().unwrap();
        assert!(lease_read_result(&origin, Ok(())).is_ok());
        let error = lease_read_result(&origin, Err(SnapshotLeaseError::TemporarilyUnavailable))
            .unwrap_err();
        assert!(ReadAuthority::is_temporarily_unavailable(&error));
        assert!(grant.check().is_ok());
        assert!(lease_read_result(&origin, Ok(())).is_ok());
        assert!(grant.check().is_ok());
        assert!(lease_read_result(&origin, Err(SnapshotLeaseError::Ended)).is_err());
        assert!(grant.check().is_err());
    }
}
