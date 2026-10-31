//! Copies retain task edits as plain branches, never as destination run authority.
use crate::{
    bundle::{BundleError, ProjectBundle, StudioRoot},
    bundle_records::{BranchCheckpoint, BranchId, ObjectRef, RecordKind},
    orchestration_records::{OrchestrationPayload, OrchestrationRecord},
    store::{SealedObject, StorePin},
};
use std::collections::BTreeMap;

pub(crate) enum CopyPublication {
    Pending,
    Captured(BTreeMap<BranchId, ObjectRef>),
    Plain(Box<PlainCopy>),
}
pub(crate) struct PlainCopy {
    checkpoints: BTreeMap<BranchId, ObjectRef>,
    redirected: Option<(BranchId, BranchId)>,
    _pin: StorePin,
    _index: Option<SealedObject>,
    _feedback: Option<SealedObject>,
}
impl ProjectBundle {
    fn copy_task_checkpoint_refs(&self) -> Result<BTreeMap<BranchId, ObjectRef>, BundleError> {
        let mut checkpoints = BTreeMap::new();
        let Some(root) = &self.root else {
            return Ok(checkpoints);
        };
        let Some(reference) = &root.run_index_ref else {
            return Ok(checkpoints);
        };
        let runs = self
            .store
            .read_copy_record::<crate::studio_records::RunIndex>(reference, true)?;
        let index = self
            .store
            .read_copy_record::<BTreeMap<BranchId, ObjectRef>>(&root.branch_index_ref, true)?;
        for cuts in runs.orchestration.values() {
            let Some(cut) = cuts.last() else { continue };
            let record = self
                .store
                .read_copy_record::<OrchestrationRecord>(&cut.snapshot, true)?;
            let OrchestrationPayload::Snapshot(state) = &record.payload else {
                return Err(BundleError::HistoryChanged);
            };
            for task in &state.tasks {
                for reference in &task.progress.decisions {
                    if reference.record_kind != RecordKind::RunIndex {
                        continue;
                    }
                    let record = self
                        .store
                        .read_copy_record::<OrchestrationRecord>(reference, true)?;
                    if let OrchestrationPayload::TaskHistory(history) = &record.payload
                        && let Some(reference) = index.get(&history.source.branch)
                    {
                        checkpoints.insert(history.source.branch.clone(), reference.clone());
                    }
                }
            }
        }
        Ok(checkpoints)
    }
    pub(crate) fn capture_copy_tasks(&mut self, source: &Self) -> Result<(), BundleError> {
        self.copy_publication = Some(CopyPublication::Captured(
            source.copy_task_checkpoint_refs()?,
        ));
        Ok(())
    }
    pub(crate) fn prepare_plain_copy(
        &mut self,
        branch: &BranchId,
    ) -> Result<Option<BranchId>, BundleError> {
        let checkpoints = match &self.copy_publication {
            None => return Ok(None),
            Some(CopyPublication::Plain(copy)) => {
                return Ok(copy
                    .redirected
                    .as_ref()
                    .filter(|(old, _)| old == branch)
                    .map(|(_, new)| new.clone()));
            }
            Some(CopyPublication::Pending) => self.copy_task_checkpoint_refs()?,
            Some(CopyPublication::Captured(checkpoints)) => checkpoints.clone(),
        };
        if self.unsettled {
            return Err(BundleError::Unsettled);
        }
        let references: Vec<_> = checkpoints.values().cloned().collect();
        let pin = self.store.pin(&references)?;
        let mut index = self
            .root
            .as_ref()
            .map(|root| {
                self.store
                    .read_record::<BTreeMap<BranchId, ObjectRef>>(&root.branch_index_ref)
            })
            .transpose()?
            .map_or_else(BTreeMap::new, |index| (*index).clone());
        for (id, reference) in &checkpoints {
            let checkpoint = self.store.read_record::<BranchCheckpoint>(reference)?;
            self.branches.insert(id.clone(), (*checkpoint).clone());
            index.insert(id.clone(), reference.clone());
        }
        // If a task branch was selected as manual, retain it verbatim and put the
        // accepted copy cut on a new plain manual branch instead of replacing it.
        let redirected = if checkpoints.contains_key(&self.manual_branch_id) {
            let old = self.manual_branch_id.clone();
            let new = BranchId::fresh()?;
            self.manual_branch_id = new.clone();
            Some((old, new))
        } else {
            None
        };
        let mut feedback_seal = None;
        if let Some(reference) = self
            .root
            .as_ref()
            .and_then(|root| root.feedback_index_ref.as_ref())
        {
            let original = self
                .store
                .read_record::<crate::studio_records::FeedbackIndex>(reference)?;
            if original
                .batches
                .values()
                .any(|batch| batch.consumed_by.is_some())
            {
                let mut feedback = (*original).clone();
                for batch in feedback.batches.values_mut() {
                    batch.consumed_by = None;
                }
                feedback_seal = Some(self.store.admit_record(&feedback)?);
            }
        }
        let sealed = if let Some(root) = &mut self.root {
            let sealed = self.store.admit_record(&index)?;
            root.branch_index_ref = sealed.reference.clone();
            root.run_index_ref = None;
            if let Some(feedback) = &feedback_seal {
                root.feedback_index_ref = Some(feedback.reference.clone());
            }
            Some(sealed)
        } else {
            None
        };
        let result = redirected
            .as_ref()
            .filter(|(old, _)| old == branch)
            .map(|(_, new)| new.clone());
        self.copy_publication = Some(CopyPublication::Plain(Box::new(PlainCopy {
            checkpoints,
            redirected,
            _pin: pin,
            _index: sealed,
            _feedback: feedback_seal,
        })));
        Ok(result)
    }
    /// Check the entire proposed map, including entries prepared before the publisher.
    pub(crate) fn validate_plain_copy(
        &self,
        root: &StudioRoot,
        index: &BTreeMap<BranchId, ObjectRef>,
    ) -> Result<(), BundleError> {
        let Some(copy) = &self.copy_publication else {
            return Ok(());
        };
        let CopyPublication::Plain(copy) = copy else {
            return Err(BundleError::HistoryBarrierInvariant);
        };
        if root.run_index_ref.is_some()
            || copy
                .checkpoints
                .iter()
                .any(|(branch, checkpoint)| index.get(branch) != Some(checkpoint))
        {
            return Err(BundleError::Identity(
                "copy must retain plain task checkpoints without an inherited run".into(),
            ));
        }
        if let Some(reference) = &root.feedback_index_ref {
            let feedback = self
                .store
                .read_record::<crate::studio_records::FeedbackIndex>(reference)?;
            if feedback
                .batches
                .values()
                .any(|batch| batch.consumed_by.is_some())
            {
                return Err(BundleError::Identity(
                    "copy cannot bind feedback to an inherited run".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bundle::BundleWriteAdmission, studio_records::RunIndex};
    use kinewright_core::{Core, Document};

    #[test]
    fn plain_copy_invariant_checks_the_whole_map_and_run_index() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "plain-copy-invariant",
        );
        let path = temp.path("project.kinewright");
        let mut bundle =
            ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
        let core = Core::spawn(Document::default()).unwrap();
        let inputs = bundle.empty_inputs().unwrap().reference.clone();
        bundle.save(&core, &inputs).unwrap();
        let branch = bundle
            .fork_branch(&bundle.manual_branch_id.clone())
            .unwrap();
        let checkpoint = bundle.checkpoint_reference(&branch).unwrap();
        bundle.copy_publication = Some(CopyPublication::Captured(BTreeMap::from([(
            branch.clone(),
            checkpoint.clone(),
        )])));
        bundle
            .prepare_plain_copy(&bundle.manual_branch_id.clone())
            .unwrap();
        let root = bundle.root.clone().unwrap();
        let index = (*bundle
            .store
            .read_record::<BTreeMap<BranchId, ObjectRef>>(&root.branch_index_ref)
            .unwrap())
        .clone();
        bundle.validate_plain_copy(&root, &index).unwrap();
        let mut changed = index.clone();
        changed.insert(
            branch,
            bundle
                .checkpoint_reference(&bundle.manual_branch_id)
                .unwrap(),
        );
        assert!(
            bundle.validate_plain_copy(&root, &changed).is_err(),
            "C3_COPY_WHOLE_MAP_INVARIANT"
        );
        let inherited = bundle.store.admit_record(&RunIndex::default()).unwrap();
        let mut changed_root = root.clone();
        changed_root.run_index_ref = Some(inherited.reference.clone());
        assert!(
            bundle.validate_plain_copy(&changed_root, &index).is_err(),
            "C3_COPY_RUN_INDEX_INVARIANT"
        );
        assert_eq!(
            bundle
                .checkpoint_reference(&bundle.manual_branch_id)
                .unwrap(),
            index[&bundle.manual_branch_id]
        );
    }
}
