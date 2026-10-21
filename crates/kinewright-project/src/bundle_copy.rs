//! Unsaved edits use exactly the named store's object, budget and pin owner.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError, bundle_records::SessionId, namespace::ProjectNamespace,
    storage_budget::StorageLimits, store::ProjectStore,
};
use std::path::Path;
pub struct SessionStore {
    store: ProjectStore,
    session_id: SessionId,
}
impl SessionStore {
    pub fn create(parent: &Path, limits: StorageLimits) -> Result<Self, BundleError> {
        limits.validate()?;
        let session_id = SessionId::fresh()?;
        let namespace = ProjectNamespace::create_session(parent, &session_id)?;
        let store = ProjectStore::new(namespace, None, Some(session_id.clone()), limits)?;
        Ok(Self { store, session_id })
    }
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    #[must_use]
    pub fn store(&self) -> &ProjectStore {
        &self.store
    }
    pub fn store_mut(&mut self) -> &mut ProjectStore {
        &mut self.store
    }
    pub fn first_save(&mut self, _destination: &Path) -> Result<(), BundleError> {
        Err(BundleError::Unsupported(
            "named handoff requires complete bundle-copy publication".into(),
        ))
    }
}
