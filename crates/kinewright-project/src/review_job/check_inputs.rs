//! JA-1 S5a commit 5: the frozen inputs a review check reads.
//!
//! Checks read only the verified artifact bytes, the records the job froze at
//! request time and the pinned owned inputs of the reviewed version, never
//! the working edit.
//!
//! Binding happens in two steps (L13). [`ProjectBundle::prepare_review_check`]
//! runs under the writer lock: it proves the job, artifact and request all
//! belong to the caller's token and pins every object the check reads. It
//! hashes nothing. [`PendingReviewCheck::open`] runs with the lock released:
//! it verifies the artifact bytes and opens the pinned inputs.
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::ObjectRef,
    storage_budget::StorageReservation,
    store::StorePin,
    studio_store::{FrozenContext, FrozenContextRequest},
};

use super::{
    ReviewArtifact, ReviewJob, ReviewPhase, ReviewRequest, ReviewToken, StoredReviewExpectations,
};

/// Verified artifact bytes, the frozen records of the request that made them,
/// and the pinned inputs of the reviewed version.
pub struct ReviewCheckInputs {
    pub artifact: StorageReservation<ReviewArtifact>,
    pub media: std::fs::File,
    pub request: StorageReservation<ReviewRequest>,
    pub expectations: StorageReservation<StoredReviewExpectations>,
    /// The reviewed version's frozen document and certified inputs.
    pub context: FrozenContext,
}

/// A check bound to its token under the lock, not yet opened.
pub struct PendingReviewCheck {
    artifact_ref: ObjectRef,
    request_ref: ObjectRef,
    pin: StorePin,
    context: FrozenContextRequest,
    profile: kinewright_core::output_review::CoverageProfileId,
}

/// Every record a check reads must name the caller's token. A record that
/// names another job, generation or version is refused before any byte of the
/// artifact is read.
pub(crate) fn check_review_check_binding(
    token: &ReviewToken,
    job: &ReviewJob,
    artifact: &ReviewArtifact,
    request: &ReviewRequest,
) -> Result<(), BundleError> {
    let refuse = |what: &str| Err(BundleError::Identity(format!("review check {what}")));
    if &job.token != token {
        return refuse("token is not the current job");
    }
    if !matches!(job.phase, ReviewPhase::Checking(_)) {
        return refuse("job is not checking");
    }
    if artifact.job != token.job {
        return refuse("artifact belongs to another job");
    }
    if artifact.generation != token.generation {
        return refuse("artifact belongs to another generation");
    }
    if artifact.snapshot != token.snapshot {
        return refuse("artifact belongs to another version");
    }
    if artifact.request_ref != job.request_ref {
        return refuse("artifact belongs to another request");
    }
    if request.snapshot != token.snapshot {
        return refuse("request belongs to another version");
    }
    Ok(())
}

impl ProjectBundle {
    /// Bind a check to `token` and pin what it reads. Cheap: no hashing and
    /// no media is opened, so it may run under the writer lock.
    ///
    /// # Errors
    /// Refuses a token that is not the checking job's, any record that names
    /// another job, generation or version, and a version whose document or
    /// input manifest is not the request's.
    pub fn prepare_review_check(
        &self,
        token: &ReviewToken,
    ) -> Result<PendingReviewCheck, BundleError> {
        let job = self
            .review_job()?
            .filter(|job| &job.token == token)
            .ok_or_else(|| BundleError::Identity("review check token is not current".into()))?;
        let artifact_ref = job.artifact_ref.clone().ok_or_else(|| {
            BundleError::Identity("review artifact has not been published".into())
        })?;
        let artifact = self.store.read_record::<ReviewArtifact>(&artifact_ref)?;
        let request = self.store.read_record::<ReviewRequest>(&job.request_ref)?;
        check_review_check_binding(token, &job, &artifact, &request)?;
        let snapshot = self.read_snapshot(&token.snapshot)?;
        if snapshot.descriptor.document_ref != request.document_ref
            || snapshot.descriptor.input_manifest_ref != request.input_manifest_ref
        {
            return Err(BundleError::Identity(
                "review check version inputs are not the request's".into(),
            ));
        }
        let context = self.prepare_frozen_context_descriptor(&snapshot.descriptor)?;
        let pin = self.store.pin(&[
            artifact_ref.clone(),
            artifact.media_ref.clone(),
            job.request_ref.clone(),
            request.expectations_ref.clone(),
        ])?;
        Ok(PendingReviewCheck {
            artifact_ref,
            request_ref: job.request_ref.clone(),
            pin,
            context,
            profile: request.profile,
        })
    }

    /// Prepare and open in one call, for callers that hold no lock.
    ///
    /// # Errors
    /// As [`Self::prepare_review_check`] and [`PendingReviewCheck::open`].
    pub fn open_review_check_inputs(
        &self,
        token: &ReviewToken,
    ) -> Result<ReviewCheckInputs, BundleError> {
        self.prepare_review_check(token)?.open()
    }
}

impl PendingReviewCheck {
    /// The rows the request's coverage profile requires, so a caller can
    /// record each of them even when opening fails.
    #[must_use]
    pub fn required_rows(&self) -> &'static [kinewright_core::output_review::RequiredReviewRow] {
        self.profile.required_rows()
    }

    /// Verify the artifact bytes and open the pinned inputs. Runs with the
    /// writer lock released.
    ///
    /// # Errors
    /// Refuses changed bytes, expectations of another cut and inputs that no
    /// longer verify.
    pub fn open(self) -> Result<ReviewCheckInputs, BundleError> {
        let store = self.pin.read_store();
        let artifact = store.read_record::<ReviewArtifact>(&self.artifact_ref)?;
        let media = store.open_verified(&artifact.media_ref)?;
        let request = store.read_record::<ReviewRequest>(&self.request_ref)?;
        let expectations =
            store.read_record::<StoredReviewExpectations>(&request.expectations_ref)?;
        if expectations.0.identity != request.identity {
            return Err(BundleError::Identity(
                "review expectations belong to another cut".into(),
            ));
        }
        let context = self.context.open()?;
        Ok(ReviewCheckInputs {
            artifact,
            media,
            request,
            expectations,
            context,
        })
    }
}
