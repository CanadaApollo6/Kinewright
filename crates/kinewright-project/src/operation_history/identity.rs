//! Canonical semantic identity of the selected Core and its owner records.
use super::{Error, snapshot::SnapshotData};
use serde::Serialize;

/// Protocol identity: neither digest alone names the entire selected state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotIdentity {
    pub core: [u8; 32],
    pub owner: [u8; 32],
}
impl SnapshotIdentity {
    #[must_use]
    pub fn digest(self) -> [u8; 32] {
        let mut hash = kinewright_hash::Sha256::new();
        hash.update(b"kinewright/owner-snapshot/v1\0");
        hash.update(&self.core);
        hash.update(&self.owner);
        hash.finalize()
    }
}
impl SnapshotData {
    pub(super) fn identity(&self) -> Result<SnapshotIdentity, Error> {
        Ok(SnapshotIdentity {
            core: self.history.snapshot_identity()?,
            owner: self.owner_digest()?,
        })
    }
    pub(super) fn owner_digest(&self) -> Result<[u8; 32], Error> {
        super::borrowed_identity::captured(self)
    }
}
pub(super) fn digest(value: &impl Serialize) -> Result<[u8; 32], Error> {
    struct Writer(kinewright_hash::Sha256);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            super::owner::identity_serialized(bytes.len() as u64);
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    super::owner::identity_started();
    let mut writer = Writer(kinewright_hash::Sha256::new());
    writer.0.update(b"kinewright/history-owner/v1\0");
    serde_json::to_writer(&mut writer, value).map_err(|_| Error::InvalidRecord)?;
    Ok(writer.0.finalize())
}
