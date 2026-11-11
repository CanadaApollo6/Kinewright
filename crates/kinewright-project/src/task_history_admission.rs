//! Immutable prospective checkpoint bounds; no project mutex on task ingress.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError, bundle_records::SpecialistBaseRecord, storage_budget::StorageLimits,
};
use kinewright_core::{HistorySnapshot, Operation};

pub const LIMIT_REASON: &str =
    "Task history limit reached. Finish or freeze this task before making more edits.";

pub struct TaskHistoryAdmission {
    pub(crate) replay_gate: std::sync::Arc<crate::history_record::ReplayGate>,
    pub(crate) base: SpecialistBaseRecord,
    pub(crate) prefix: Vec<Operation>,
    pub(crate) floor_offset: u64,
    pub(crate) limits: StorageLimits,
    pub(crate) lineage_seed: crate::bundle_records::LineageRecord,
}
impl TaskHistoryAdmission {
    /// All task previews share the store publication replay overlap.
    pub fn with_preview<T>(&self, work: impl FnOnce() -> T) -> Result<T, BundleError> {
        self.replay_gate.with(work)
    }
    /// Check exactly the full history and specialist delta that persistence
    /// writes, with independent persisted and live budgets for its role.
    /// Finish and Freeze remain available after a pre-mutation refusal.
    pub fn check(&self, history: &HistorySnapshot) -> Result<(), BundleError> {
        self.check_role(history, crate::history_record::HistoryRole::Task)
            .map_err(task_capacity_error)
    }
    pub(crate) fn check_role(
        &self,
        history: &HistorySnapshot,
        role: crate::history_record::HistoryRole,
    ) -> Result<(), BundleError> {
        if history.floor_offset != self.floor_offset {
            return Err(BundleError::HistoryChanged);
        }
        check_role_history(history, &self.limits, role, &self.lineage_seed)?;
        let mut base = SpecialistBaseRecord {
            history_role: role,
            target_branch: self.base.target_branch.clone(),
            base_head: self.base.base_head.clone(),
            base_document_hash: self.base.base_document_hash.clone(),
            original_baseline_ref: self.base.original_baseline_ref.clone(),
            frozen_checkpoint_ref: self.base.frozen_checkpoint_ref.clone(),
            full_applied_delta: self.prefix.clone(),
        };
        base.full_applied_delta.extend(history.applied_operations());
        if base.full_applied_delta.len() > kinewright_core::HISTORY_COMMANDS {
            return Err(BundleError::RunAdmission(LIMIT_REASON));
        }
        let bytes = serde_json::to_vec(&base).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if bytes.len() as u64
            > role
                .persisted_limit(&self.limits)
                .min(self.limits.branch_history_bytes)
            || crate::store::preflight_record(&bytes, &self.limits)?
                > role.persisted_limit(&self.limits)
        {
            return Err(BundleError::RunAdmission(LIMIT_REASON));
        }
        Ok(())
    }
}
pub fn check_history(history: &HistorySnapshot, limits: &StorageLimits) -> Result<(), BundleError> {
    let bytes = serde_json::to_vec(&crate::history_record::HistoryRecord::capture(
        history,
        crate::history_record::HistoryRole::Task,
    )?)
    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
    let bound = crate::history_record::TASK_BYTES.min(limits.branch_history_bytes);
    if bytes.len() as u64 > bound
        || crate::store::preflight_record(&bytes, limits).map_err(task_capacity_error)? > bound
    {
        return Err(BundleError::RunAdmission(LIMIT_REASON));
    }
    Ok(())
}

pub const DOCUMENT_BYTES: u64 = 5_645_312;
pub(crate) fn check_role_history(
    history: &HistorySnapshot,
    limits: &StorageLimits,
    role: crate::history_record::HistoryRole,
    seed: &crate::bundle_records::LineageRecord,
) -> Result<(), BundleError> {
    let record = crate::history_record::HistoryRecord::capture(history, role)?;
    let bytes =
        serde_json::to_vec(&record).map_err(|error| BundleError::Corrupt(error.to_string()))?;
    let bound = role
        .persisted_limit(limits)
        .min(limits.branch_history_bytes);
    if bytes.len() as u64 > bound || crate::store::preflight_record(&bytes, limits)? > bound {
        return Err(BundleError::RunAdmission(LIMIT_REASON));
    }
    for document in [&history.baseline, &history.document] {
        if crate::history_size::payload(document)? > DOCUMENT_BYTES {
            return Err(BundleError::RunAdmission(LIMIT_REASON));
        }
    }
    // Admission-only IDs have the same fixed width and distinct batch identities;
    // they are never published or substituted for the checkpoint's real origins.
    let ids: Vec<crate::bundle_records::BatchId> = (0..history.batches.len())
        .map(|index| {
            serde_json::from_value(serde_json::Value::String(format!("{:032x}", index + 1)))
                .map_err(|error| BundleError::Corrupt(error.to_string()))
        })
        .collect::<Result<_, _>>()?;
    let lineage =
        crate::bundle_commit::lineage(&seed.branch_origin, history, &ids, Some(seed), Some(seed))?;
    let encoded =
        serde_json::to_vec(&lineage).map_err(|error| BundleError::Corrupt(error.to_string()))?;
    let live = crate::history_size::native_history_live_bytes(history)?
        .checked_add(crate::lineage_journal::live_bytes(&lineage)?)
        .ok_or(BundleError::RunAdmission(LIMIT_REASON))?;
    let replay = crate::history_size::replay_live_bound(
        &record.journal,
        &history.baseline,
        &history.document,
    )?;
    if live > role.live_limit(limits)
        || replay > role.replay_limit(limits)
        || encoded.len() as u64 > crate::run_admission::RECORD_BYTES
        || crate::store::preflight_record(&encoded, limits)? > crate::run_admission::DECODED_BYTES
    {
        return Err(BundleError::RunAdmission(LIMIT_REASON));
    }
    Ok(())
}

#[cfg(any(test, feature = "test-util"))]
pub fn decoded_history_bytes_for_fixture(history: &HistorySnapshot) -> Result<u64, BundleError> {
    let record = crate::history_record::HistoryRecord::capture(
        history,
        crate::history_record::HistoryRole::Task,
    )?;
    let bytes = serde_json::to_vec(&record).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    crate::store::preflight_record(&bytes, &StorageLimits::default())
}

#[cfg(feature = "test-util")]
pub fn lineage_for_fixture(
    history: &HistorySnapshot,
) -> Result<crate::bundle_records::LineageRecord, BundleError> {
    let branch = crate::bundle_records::BranchId::fresh()?;
    let ids = history
        .batches
        .iter()
        .map(|_| crate::bundle_records::BatchId::fresh())
        .collect::<Result<Vec<_>, _>>()?;
    crate::bundle_commit::lineage(&branch, history, &ids, None, None)
}

#[cfg(feature = "test-util")]
pub fn live_history_bytes_for_fixture(
    history: &HistorySnapshot,
    lineage: &crate::bundle_records::LineageRecord,
) -> Result<(u64, u64), BundleError> {
    Ok((
        crate::history_size::native_history_live_bytes(history)?,
        crate::lineage_journal::live_bytes(lineage)?,
    ))
}

fn task_capacity_error(error: BundleError) -> BundleError {
    match error {
        BundleError::Capacity(_) => BundleError::RunAdmission(LIMIT_REASON),
        other => other,
    }
}
