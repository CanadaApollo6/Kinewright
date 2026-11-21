//! JA-1 S5a commit 5: the frozen inputs a review check reads.
//!
//! Checks read only the verified artifact bytes and the records the job
//! froze at request time, never the working edit.
use kinewright_core::Document;

use super::{ReviewArtifact, ReviewRequest, ReviewToken, StoredReviewExpectations};
use crate::{
    bundle::{BundleError, ProjectBundle},
    storage_budget::StorageReservation,
};

/// Verified artifact bytes and the frozen records of the request that made them.
pub struct ReviewCheckInputs {
    pub artifact: StorageReservation<ReviewArtifact>,
    pub media: std::fs::File,
    pub request: StorageReservation<ReviewRequest>,
    pub expectations: StorageReservation<StoredReviewExpectations>,
    pub document: StorageReservation<Document>,
}

impl ProjectBundle {
    /// Open the published artifact of `token` with the request, expectations
    /// and document frozen with it.
    pub fn open_review_check_inputs(
        &self,
        token: &ReviewToken,
    ) -> Result<ReviewCheckInputs, BundleError> {
        let (artifact, media) = self.open_review_artifact(token)?;
        let job = self
            .review_job()?
            .filter(|job| &job.token == token)
            .ok_or_else(|| BundleError::Identity("review artifact authority changed".into()))?;
        if job.request_ref != artifact.request_ref {
            return Err(BundleError::Identity(
                "review artifact belongs to another request".into(),
            ));
        }
        let request = self
            .store
            .read_record::<ReviewRequest>(&artifact.request_ref)?;
        let expectations = self
            .store
            .read_record::<StoredReviewExpectations>(&request.expectations_ref)?;
        if expectations.0.identity != request.identity {
            return Err(BundleError::Identity(
                "review expectations belong to another cut".into(),
            ));
        }
        let document = self.store.read_record::<Document>(&request.document_ref)?;
        Ok(ReviewCheckInputs {
            artifact,
            media,
            request,
            expectations,
            document,
        })
    }
}
