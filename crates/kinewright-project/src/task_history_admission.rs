//! Immutable prospective checkpoint bounds; no project mutex on task ingress.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError, bundle_records::SpecialistBaseRecord, storage_budget::StorageLimits,
};
use kinewright_core::{HistorySnapshot, Operation};

pub const LIMIT_REASON: &str =
    "Task history limit reached. Finish or freeze this task before making more edits.";

pub struct TaskHistoryAdmission {
    pub(crate) base: SpecialistBaseRecord,
    pub(crate) prefix: Vec<Operation>,
    pub(crate) limits: StorageLimits,
}
impl TaskHistoryAdmission {
    /// Check exactly the full history and specialist delta that persistence
    /// writes. The ordinary 1 MiB bound is deliberately conservative: cleanup
    /// has raw admission, while Finish/Freeze must remain available beforehand.
    pub fn check(&self, history: &HistorySnapshot) -> Result<(), BundleError> {
        check_history(history, &self.limits)?;
        let mut base = self.base.clone();
        base.full_applied_delta.clone_from(&self.prefix);
        base.full_applied_delta.extend(history.applied_operations());
        if base.full_applied_delta.len() > kinewright_core::HISTORY_COMMANDS {
            return Err(BundleError::RunAdmission(LIMIT_REASON));
        }
        let bytes = serde_json::to_vec(&base).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if bytes.len() as u64
            > crate::run_admission::HISTORY_BYTES.min(self.limits.branch_history_bytes)
            || crate::store::preflight_record(&bytes, &self.limits)?
                > crate::run_admission::HISTORY_BYTES
        {
            return Err(BundleError::RunAdmission(LIMIT_REASON));
        }
        Ok(())
    }
}
pub fn check_history(history: &HistorySnapshot, limits: &StorageLimits) -> Result<(), BundleError> {
    let bytes = serde_json::to_vec(history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    let bound = crate::run_admission::HISTORY_BYTES.min(limits.branch_history_bytes);
    if bytes.len() as u64 > bound || crate::store::preflight_record(&bytes, limits)? > bound {
        return Err(BundleError::RunAdmission(LIMIT_REASON));
    }
    Ok(())
}

#[cfg(any(test, feature = "test-util"))]
pub fn decoded_history_bytes_for_fixture(history: &HistorySnapshot) -> Result<u64, BundleError> {
    let bytes = serde_json::to_vec(history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    crate::store::preflight_record(&bytes, &StorageLimits::default())
}
