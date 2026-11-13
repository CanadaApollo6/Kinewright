//! Immutable prospective checkpoint bounds; no project mutex on task ingress.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError, bundle_records::SpecialistBaseRecord, storage_budget::StorageLimits,
};
use kinewright_core::{HistorySnapshot, Operation};

pub const LIMIT_REASON: &str =
    "Task history limit reached. Finish or freeze this task before making more edits.";

pub struct TaskHistoryAdmission {
    pub(crate) usage: std::sync::Arc<std::sync::Mutex<crate::storage_budget::StorageUsage>>,
    pub(crate) replay_gate: std::sync::Arc<crate::history_record::ReplayGate>,
    pub(crate) base: SpecialistBaseRecord,
    pub(crate) prefix: Vec<Operation>,
    pub(crate) floor_offset: u64,
    pub(crate) limits: StorageLimits,
    pub(crate) lineage_seed: crate::bundle_records::LineageRecord,
    pub(crate) proof: std::sync::Mutex<
        Option<(
            crate::history_record::HistoryRole,
            StorageLimits,
            kinewright_core::ResolvedHistory,
        )>,
    >,
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
        self.replay_gate
            .with(|| self.check_role_serial(history, role))?
    }
    fn check_role_serial(
        &self,
        history: &HistorySnapshot,
        role: crate::history_record::HistoryRole,
    ) -> Result<(), BundleError> {
        if history.floor_offset != self.floor_offset {
            return Err(BundleError::HistoryChanged);
        }
        let _replay = crate::storage_budget::StorageReservation::reserve_for_retained_read(
            &self.usage,
            &self.limits,
            0,
            role.replay_limit(&self.limits),
            if role == crate::history_record::HistoryRole::Ordinary {
                role.replay_limit(&self.limits)
            } else {
                0
            },
            false,
            u64::MAX,
            true,
        )?;
        let prior = self
            .proof
            .lock()
            .map_err(|_| BundleError::Capacity("history proof unavailable"))?
            .as_ref()
            .filter(|(prior_role, prior_limits, _)| {
                *prior_role == role && *prior_limits == self.limits
            })
            .map(|(_, _, proof)| proof.clone());
        if let Some(proof) = &prior
            && proof.snapshot() == history
        {
            let shared = proof
                .clone()
                .share_documents(history.clone())
                .map_err(crate::bundle::history_error)?;
            *self
                .proof
                .lock()
                .map_err(|_| BundleError::Capacity("history proof unavailable"))? =
                Some((role, self.limits.clone(), shared));
            return Ok(());
        }
        let proof = check_role_history(
            history,
            &self.limits,
            role,
            &self.lineage_seed,
            prior.as_ref(),
        )?;
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
        let bytes = serde_json::to_vec(&base).map_err(crate::bundle::encoding_error)?;
        if bytes.len() as u64
            > role
                .persisted_limit(&self.limits)
                .min(self.limits.branch_history_bytes)
            || crate::store::preflight_record(&bytes, &self.limits)?
                > role.persisted_limit(&self.limits)
        {
            return Err(BundleError::RunAdmission(LIMIT_REASON));
        }
        let proof = proof
            .share_documents(history.clone())
            .map_err(crate::bundle::history_error)?;
        *self
            .proof
            .lock()
            .map_err(|_| BundleError::Capacity("history proof unavailable"))? =
            Some((role, self.limits.clone(), proof));
        Ok(())
    }
    /// Restore a preview from the exact proven live cut, without replaying its
    /// closed prefix again. The caller's preview window remains serialized.
    pub fn restore_preview(
        &self,
        core: &kinewright_core::Core,
        history: &HistorySnapshot,
    ) -> Result<(), BundleError> {
        self.check(history)?;
        let proof = self
            .proof
            .lock()
            .map_err(|_| BundleError::Capacity("history proof unavailable"))?
            .as_ref()
            .map(|(_, _, proof)| proof.clone())
            .ok_or(BundleError::HistoryChanged)?;
        if proof.snapshot() != history {
            return Err(BundleError::HistoryChanged);
        }
        core.restore_resolved_history(proof)
            .map_err(crate::bundle::history_error)
    }
}
pub fn check_history(history: &HistorySnapshot, limits: &StorageLimits) -> Result<(), BundleError> {
    crate::history_size::verify_history_admission(
        history,
        crate::history_record::HistoryRole::Task,
        limits,
    )
    .map(|_| ())
    .map_err(task_capacity_error)
}

pub const DOCUMENT_BYTES: u64 = 5_645_312;
fn check_role_history(
    history: &HistorySnapshot,
    limits: &StorageLimits,
    role: crate::history_record::HistoryRole,
    seed: &crate::bundle_records::LineageRecord,
    prefix: Option<&kinewright_core::ResolvedHistory>,
) -> Result<kinewright_core::ResolvedHistory, BundleError> {
    let (retained, proof) =
        crate::history_size::verify_history_admission_with_prefix(history, role, limits, prefix)?;
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
                .map_err(|_| BundleError::HistoryCommitInvariant)
        })
        .collect::<Result<_, _>>()?;
    let lineage =
        crate::bundle_commit::lineage(&seed.branch_origin, history, &ids, Some(seed), Some(seed))?;
    let encoded = serde_json::to_vec(&lineage).map_err(crate::bundle::encoding_error)?;
    let live = retained
        .checked_add(crate::lineage_journal::live_bytes(&lineage)?)
        .ok_or(BundleError::RunAdmission(LIMIT_REASON))?;
    if live > role.live_limit(limits)
        || encoded.len() as u64 > crate::run_admission::RECORD_BYTES
        || crate::store::preflight_record(&encoded, limits)? > crate::run_admission::DECODED_BYTES
    {
        return Err(BundleError::RunAdmission(LIMIT_REASON));
    }
    Ok(proof)
}

#[cfg(any(test, feature = "test-util"))]
pub fn decoded_history_bytes_for_fixture(history: &HistorySnapshot) -> Result<u64, BundleError> {
    let record = crate::history_record::HistoryRecord::capture(
        history,
        crate::history_record::HistoryRole::Task,
    )?;
    let bytes = serde_json::to_vec(&record).map_err(crate::bundle::encoding_error)?;
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
