//! Local Studio entry point before the process host is introduced.
use kinewright_project::{
    bundle::{BundleError, BundleSaveReport},
    editing_session::Ja1EditingSession,
};
use std::path::Path;

/// One local agent's New/Open through Close owner. Resume repeats full raw/graph admission.
pub struct LocalEditingSession {
    owner: Ja1EditingSession,
}
impl LocalEditingSession {
    /// Open or resume after validating support, before creating an editable session.
    /// # Errors
    /// Unsupported/raw-invalid graphs and unsupported durable storage refuse without mutation.
    pub fn open(path: &Path) -> Result<Self, BundleError> {
        Ok(Self {
            owner: Ja1EditingSession::open(path)?,
        })
    }
    /// The local caller uses the same recovery/rescue lifecycle as the App.
    pub fn owner(&mut self) -> &mut Ja1EditingSession {
        &mut self.owner
    }
    /// # Errors
    /// Returns the ordinary save admission/publication error without retrying an unknown result.
    pub fn save(&mut self) -> Result<BundleSaveReport, BundleError> {
        self.owner.save_active()
    }
}
