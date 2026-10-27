//! Whole verified immutable copies for bounded LUT and font resources.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    owned_inputs::OwnedInputRef,
    storage_budget::StorageReservation,
    store::{ProjectStore, StorePin},
};
use std::sync::Arc;
pub(crate) struct ResourceBytes {
    pub(crate) bytes: StorageReservation<Vec<u8>>,
    pub(crate) _pin: StorePin,
}
#[derive(Clone)]
pub struct CertifiedResource {
    shared: Arc<ResourceBytes>,
    identity: OwnedInputRef,
    _context: Arc<StorePin>,
}
impl CertifiedResource {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.shared.bytes
    }
    #[must_use]
    pub fn identity(&self) -> &OwnedInputRef {
        &self.identity
    }
}
impl ProjectStore {
    pub fn certify_resource_input(
        &self,
        reference: &OwnedInputRef,
    ) -> Result<CertifiedResource, BundleError> {
        let manifest = self.read_input_manifest(&reference.manifest_ref)?;
        if !manifest.entries.contains(&reference.entry) {
            return Err(BundleError::Identity("resource manifest binding".into()));
        }
        let shared = self.shared_resource(&reference.entry.object)?;
        let context = self.pin(std::slice::from_ref(&reference.manifest_ref))?;
        Ok(CertifiedResource {
            shared,
            identity: reference.clone(),
            _context: Arc::new(context),
        })
    }
}
