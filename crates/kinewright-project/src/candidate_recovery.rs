//! One checkpoint-bound recovery base per registered branch, published with its cut.
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchId, ObjectRef, SpecialistBaseRecord},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRecovery {
    pub checkpoint: ObjectRef,
    pub base_ref: ObjectRef,
}
impl ProjectBundle {
    /// Only H1's exact, frozen merge envelope can outlive its runtime binding.
    /// Current target head, source checkpoint, issuer sequence and full delta
    /// remain checked by the one candidate command publisher.
    pub(crate) fn retained_candidate_merge(
        &self,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<bool, BundleError> {
        use crate::orchestration_records::{OrchestrationPayload, OrchestrationRecord};
        for run in self.run_index()?.orchestration.keys() {
            let Some(state) = self.orchestration_projection(run)? else {
                continue;
            };
            for task in state.tasks {
                for reference in task.progress.decisions {
                    if reference.record_kind != crate::bundle_records::RecordKind::RunIndex {
                        continue;
                    }
                    let record = self.store.read_record::<OrchestrationRecord>(&reference)?;
                    let OrchestrationPayload::TaskHistory(history) = &record.payload else {
                        continue;
                    };
                    if history.frozen
                        && history.project == self.project_id
                        && history.merge_envelope.as_ref() == Some(envelope)
                        && history.base.branch == envelope.target_branch
                        && envelope.expected_head.as_ref() == Some(&history.base.head)
                        && self.checkpoint_reference(&history.source.branch)?
                            == history.source.checkpoint
                    {
                        let checkpoint = self
                            .store
                            .read_record::<crate::bundle_records::BranchCheckpoint>(
                                &history.source.checkpoint,
                            )?;
                        let delta = checkpoint
                            .open(&self.store, self.history_pool())?
                            .into_reader()?
                            .into_operations()?;
                        return Ok(
                            matches!(&envelope.command, Some(kinewright_core::JournalCommand::DoBatch(operations)) if operations.as_slice() == &*delta),
                        );
                    }
                }
            }
        }
        Ok(false)
    }
    pub(crate) fn current_candidate_base(
        &self,
        branch: &BranchId,
        initial: &ObjectRef,
    ) -> Result<ObjectRef, BundleError> {
        let reference = if let Some(binding) = self
            .root
            .as_ref()
            .and_then(|root| root.candidate_recovery.get(branch))
        {
            if self.checkpoint_reference(branch)? != binding.checkpoint {
                return Err(BundleError::HistoryChanged);
            }
            &binding.base_ref
        } else {
            initial
        };
        let base = self.store.read_record::<SpecialistBaseRecord>(reference)?;
        let target = self
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&base.frozen_checkpoint_ref)?;
        let current = self.checkpoint(branch)?;
        let record = self
            .store
            .read_record::<crate::operation_history::StoredOwnerRecord>(&current.owner_ref)?;
        if base.history_role != current.role
            || record.baseline != base.original_baseline_ref
            || target.head_id != base.base_head
            || target.branch_id != base.target_branch
            || target.document_ref.sha256 != base.base_document_hash
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(reference.clone())
    }
}
