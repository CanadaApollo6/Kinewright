//! The store uses the same typed bound as portable Core history decoding.
use crate::bundle::BundleError;
pub(crate) fn payload(value: &impl serde::Serialize) -> Result<u64, BundleError> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| BundleError::Corrupt(error.to_string()))?;
    crate::store::preflight_record(&bytes, &crate::storage_budget::StorageLimits::default())
}
pub(crate) fn document_live_bytes(
    document: &kinewright_core::Document,
) -> Result<u64, BundleError> {
    kinewright_core::history_size::document_live_bytes(document)
        .map_err(crate::bundle::history_error)
}
pub(crate) fn history_live_bytes(
    history: &kinewright_core::HistorySnapshot,
) -> Result<u64, BundleError> {
    kinewright_core::history_size::history_live_bytes(history).map_err(crate::bundle::history_error)
}
pub(crate) fn replay_live_bound(
    journal: &kinewright_core::HistoryJournal,
    baseline: &kinewright_core::Document,
    current: &kinewright_core::Document,
) -> Result<u64, BundleError> {
    kinewright_core::history_size::replay_live_bound(journal, baseline, current)
        .map_err(crate::bundle::history_error)
}
