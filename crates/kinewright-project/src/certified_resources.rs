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
    context: Arc<StorePin>,
}
impl CertifiedResource {
    pub fn bytes(&self) -> Result<&[u8], BundleError> {
        self.context.read_store().check_read_authority()?;
        Ok(&self.shared.bytes)
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
            context: Arc::new(context),
        })
    }
}
