//! Prepared branch objects enter the root only with their matching H1 history.
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchCheckpoint, BranchId, ObjectRef},
    editing_session::BranchBinding,
    orchestration_commit::ValidatedRunUpdate,
    orchestration_records::{OrchestrationPayload, OrchestrationRecord},
    store::{SealedObject, StorePin},
};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) struct StagedTaskCheckpoint {
    pub before: ObjectRef,
    pub reference: ObjectRef,
    pub checkpoint: BranchCheckpoint,
    pub base_ref: ObjectRef,
    pub history: Arc<crate::storage_budget::StorageReservation<kinewright_core::HistorySnapshot>>,
    pub _pin: StorePin,
    pub _seals: Vec<SealedObject>,
}
impl StagedTaskCheckpoint {
    pub(crate) fn binding(&self) -> BranchBinding {
        BranchBinding {
            branch: self.checkpoint.branch_id.clone(),
            head: self.checkpoint.head_id.clone(),
            checkpoint: self.reference.clone(),
            document_hash: self.checkpoint.document_hash.clone(),
        }
    }
}
impl ProjectBundle {
    pub(crate) fn task_checkpoint_updates(
        &self,
        transition: &ValidatedRunUpdate,
    ) -> Result<Vec<BranchId>, BundleError> {
        let mut matched = BTreeSet::new();
        for task in &transition.projection().tasks {
            for reference in &task.progress.decisions {
                if reference.record_kind != crate::bundle_records::RecordKind::RunIndex {
                    continue;
                }
                let record = self.store.read_record::<OrchestrationRecord>(reference)?;
                let OrchestrationPayload::TaskHistory(history) = &record.payload else {
                    continue;
                };
                if let Some(staged) = self.task_checkpoints.get(&history.source.branch) {
                    if history.project != self.project_id
                        || history.run != transition.projection().run
                        || history.task != task.id
                        || history.source != staged.binding()
                        || history.base_ref != staged.base_ref
                        || self.checkpoint_reference(&history.source.branch)? != staged.before
                    {
                        return Err(BundleError::HistoryChanged);
                    }
                    matched.insert(history.source.branch.clone());
                }
                if let Some(staged) = self.task_checkpoints.get(&history.base.branch)
                    && history.candidate_base_ref == staged.base_ref
                {
                    if self.checkpoint_reference(&history.base.branch)? != staged.before {
                        return Err(BundleError::HistoryChanged);
                    }
                    matched.insert(history.base.branch.clone());
                }
            }
        }
        Ok(matched.into_iter().collect())
    }
}
