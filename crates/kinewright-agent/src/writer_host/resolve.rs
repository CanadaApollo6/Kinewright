//! `ResolveFeedback`: map one indexed note onto a version or the saved current
//! edit (S3 C10). Read-only and recomputed on every ask; the mapping itself is
//! `ProjectBundle::resolve_feedback`, which reads only immutable checkpoints.
use super::{FeedbackMapping, HostCommandOwner, HostError, MapTarget, auth::ReadPin};
use kinewright_project::{bundle_records::ObjectRef, studio_records::ResolveFeedback};

impl HostCommandOwner {
    /// The note and the target checkpoint the host chose for it.
    pub(super) fn feedback_target_roots(
        &self,
        revision: &ObjectRef,
        onto: &MapTarget,
    ) -> Result<Vec<ObjectRef>, HostError> {
        let owner = self.owner()?;
        let candidate = match onto {
            MapTarget::Version { key } => owner.read_snapshot(key)?.descriptor.checkpoint_ref,
            MapTarget::CurrentEdit => owner.manual_checkpoint()?.0,
        };
        Ok(vec![revision.clone(), candidate])
    }

    pub(super) fn resolve_feedback(
        &self,
        revision: &ObjectRef,
        onto: &MapTarget,
    ) -> Result<(FeedbackMapping, ReadPin), HostError> {
        let owner = self.owner()?;
        // Only an indexed note is mapped.
        drop(owner.read_feedback(revision)?);
        let roots = self.feedback_target_roots(revision, onto)?;
        let mapped = owner.resolve_feedback(
            &ResolveFeedback {
                comment_revision_ref: revision.clone(),
                candidate_checkpoint_ref: roots[1].clone(),
                beat_map_ref: None,
            },
            None,
        )?;
        let pin = owner.store()?.pin(&roots)?;
        Ok((
            FeedbackMapping {
                candidates: mapped
                    .candidates
                    .iter()
                    .map(|candidate| (candidate.clip, candidate.frame))
                    .collect(),
                original_frame: mapped.original_frame,
                resolution: mapped.resolution,
            },
            ReadPin { roots, pin },
        ))
    }
}
