//! Revocable GUI access to a host-owned immutable snapshot graph.
use kinewright_agent::writer_host::{HostClient, HostCommand, SnapshotLeaseRemote};
use kinewright_core::MediaSourceOrigin;
use kinewright_media::owned_io::ReadAuthority;
use kinewright_project::{
    bundle::BundleError,
    storage_budget::StorageLimits,
    store::snapshot_reader::{HostSnapshotReader, SnapshotReadSession},
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
    fn check(&self) -> Result<(), String> {
        let result = if self
            .context
            .upgrade()
            .is_some_and(|alive| alive.load(Ordering::Acquire))
        {
            self.remote.check()
        } else {
            Err("attachment ended".into())
        };
        if result.is_err() {
            self.origin.revoke_legacy_media();
        }
        result
    }
    fn revoke(&self) {
        self.origin.revoke_legacy_media();
        if !self.released.swap(true, Ordering::AcqRel) {
            let _ = self.remote.release();
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
        let mut live = self
            .live
            .lock()
            .map_err(|_| BundleError::Capacity("snapshot readers poisoned"))?;
        live.retain(|reader| reader.strong_count() != 0);
        if live.len() >= 64 {
            return Err(BundleError::Capacity("64 open snapshot readers"));
        }
        let reply = client
            .request(HostCommand::LeaseSnapshot { key: key.clone() })
            .map_err(cause)?;
        if let Some(error) = reply.error {
            return Err(BundleError::Identity(error.message));
        }
        let lease = reply.snapshot_lease.ok_or(BundleError::HistoryChanged)?;
        if lease.descriptor.key != *key {
            return Err(BundleError::HistoryChanged);
        }
        let remote = client
            .snapshot_lease_remote(&lease.id, &lease.payload_sha256)
            .map_err(cause)?;
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
            lease.payload_sha256.clone(),
            Arc::new(move || checker.check()),
        );
        let reader = self.session.open(path, &lease, authority)?;
        live.push(Arc::downgrade(&lifetime));
        Ok(reader)
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
