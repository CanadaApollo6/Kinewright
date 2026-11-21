use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::ProjectId,
};

impl super::Ja1EditingSession {
    /// A Save As retains the source writer while the new project becomes active.
    pub fn review_source_bundle(&self, project: &ProjectId) -> Result<&ProjectBundle, BundleError> {
        if let Ok(bundle) = self.active_bundle()
            && bundle.project_id() == project
        {
            return Ok(bundle);
        }
        self.retained_destinations
            .iter()
            .find_map(|(owner, _)| {
                owner
                    .bundle()
                    .ok()
                    .filter(|bundle| bundle.project_id() == project)
            })
            .ok_or_else(|| BundleError::Identity("review source writer retired".into()))
    }

    pub fn review_source_bundle_mut(
        &mut self,
        project: &ProjectId,
    ) -> Result<&mut ProjectBundle, BundleError> {
        if self
            .active_bundle()
            .is_ok_and(|bundle| bundle.project_id() == project)
        {
            return self.active_bundle_mut();
        }
        self.retained_destinations
            .iter_mut()
            .find(|(owner, _)| {
                owner
                    .bundle()
                    .is_ok_and(|bundle| bundle.project_id() == project)
            })
            .ok_or_else(|| BundleError::Identity("review source writer retired".into()))?
            .0
            .bundle_mut()
    }
}
