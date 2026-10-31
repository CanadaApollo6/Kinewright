//! One checkpoint-bound recovery base per registered branch, published with its cut.
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BranchId, ObjectRef, SpecialistBaseRecord},
    editing_session::specialist_floor_delta_from_baseline,
};
use kinewright_core::{HistorySnapshot, HistoryTransition};
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
                        let base = self
                            .store
                            .read_record::<SpecialistBaseRecord>(&history.base_ref)?;
                        return Ok(envelope.command.as_ref()
                            == Some(&kinewright_core::JournalCommand::DoBatch(
                                base.full_applied_delta.clone(),
                            )));
                    }
                }
            }
        }
        Ok(false)
    }
    pub(crate) fn current_candidate_base(
        &self,
        branch: &BranchId,
        legacy: &ObjectRef,
    ) -> Result<ObjectRef, BundleError> {
        let Some(binding) = self
            .root
            .as_ref()
            .and_then(|r| r.candidate_recovery.get(branch))
        else {
            // Old roots have no shared binding. Resolve all retained hints by
            // the same full replay/checkpoint comparison, never restore order.
            return self.legacy_candidate_base(branch, legacy);
        };
        if self.checkpoint_reference(branch)? != binding.checkpoint {
            return Err(BundleError::HistoryChanged);
        }
        Ok(binding.base_ref.clone())
    }
    fn legacy_candidate_base(
        &self,
        branch: &BranchId,
        legacy: &ObjectRef,
    ) -> Result<ObjectRef, BundleError> {
        use crate::orchestration_records::{OrchestrationPayload, OrchestrationRecord};
        let mut references = std::collections::BTreeSet::from([legacy.clone()]);
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
                    if let OrchestrationPayload::TaskHistory(history) = &record.payload {
                        if &history.base.branch == branch {
                            references.insert(history.candidate_base_ref.clone());
                        }
                        if &history.source.branch == branch {
                            references.insert(history.base_ref.clone());
                        }
                    }
                }
            }
        }
        let history = self.branch_history(branch)?;
        let mut selected = None;
        for reference in references {
            let base = self.store.read_record::<SpecialistBaseRecord>(&reference)?;
            let baseline = self
                .store
                .read_record::<kinewright_core::Document>(&base.original_baseline_ref)?;
            if specialist_floor_delta_from_baseline(&base, &history, &baseline).is_ok() {
                if selected
                    .as_ref()
                    .is_some_and(|current| current != &reference)
                {
                    return Err(BundleError::Identity(
                        "ambiguous checkpoint-bound candidate base".into(),
                    ));
                }
                selected = Some(reference);
            }
        }
        selected.ok_or_else(|| {
            BundleError::Corrupt("frozen specialist delta differs from checkpoint".into())
        })
    }
    pub(crate) fn advanced_candidate_base(
        &self,
        branch: &BranchId,
        after: &HistorySnapshot,
    ) -> Result<Option<SpecialistBaseRecord>, BundleError> {
        if let Some((saved_branch, reference)) = self
            .pending_save
            .as_ref()
            .and_then(|save| save.candidate_base.as_ref())
            && saved_branch == branch
        {
            let base = self.store.read_record::<SpecialistBaseRecord>(reference)?;
            let baseline = self
                .store
                .read_record::<kinewright_core::Document>(&base.original_baseline_ref)?;
            specialist_floor_delta_from_baseline(&base, after, &baseline)?;
            return Ok(Some((*base).clone()));
        }
        let Some(binding) = self
            .root
            .as_ref()
            .and_then(|r| r.candidate_recovery.get(branch))
        else {
            return Ok(None);
        };
        if self.checkpoint_reference(branch)? != binding.checkpoint {
            return Err(BundleError::HistoryChanged);
        }
        let mut base = (*self
            .store
            .read_record::<SpecialistBaseRecord>(&binding.base_ref)?)
        .clone();
        let before = self.branch_history(branch)?;
        let discarded = usize::try_from(
            after
                .floor_offset
                .checked_sub(before.floor_offset)
                .ok_or(BundleError::HistoryChanged)?,
        )
        .map_err(|_| BundleError::Capacity("candidate floor offset"))?;
        let floor = if discarded == 0 {
            (*before).clone()
        } else {
            HistoryTransition::floor((*before).clone(), discarded)
                .map_err(crate::bundle::history_error)?
                .after()
                .clone()
        };
        if floor.baseline != after.baseline {
            return Err(BundleError::HistoryChanged);
        }
        let baseline = self
            .store
            .read_record::<kinewright_core::Document>(&base.original_baseline_ref)?;
        let mut full = specialist_floor_delta_from_baseline(&base, &floor, &baseline)?;
        full.extend(after.applied_operations());
        base.full_applied_delta = full;
        // Preserve the full replay/checkpoint comparison, including floored edits.
        specialist_floor_delta_from_baseline(&base, after, &baseline)?;
        Ok(Some(base))
    }
}
