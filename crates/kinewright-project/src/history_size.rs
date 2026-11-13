//! The store uses the same typed bound as portable Core history decoding.
use crate::bundle::BundleError;
pub(crate) fn payload(value: &impl serde::Serialize) -> Result<u64, BundleError> {
    let bytes = serde_json::to_vec(value).map_err(crate::bundle::encoding_error)?;
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
/// Shared by the pre-edit guard, merge preview, publication and cold resolver.
/// A proof includes bounded ordinary execution, not just support-field decoding.
pub(crate) fn verify_history_admission(
    history: &kinewright_core::HistorySnapshot,
    role: crate::history_record::HistoryRole,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<u64, BundleError> {
    verify_history_admission_with_prefix(history, role, limits, None).map(|(retained, _)| retained)
}
pub(crate) fn verify_history_admission_with_prefix(
    history: &kinewright_core::HistorySnapshot,
    role: crate::history_record::HistoryRole,
    limits: &crate::storage_budget::StorageLimits,
    prefix: Option<&kinewright_core::ResolvedHistory>,
) -> Result<(u64, kinewright_core::ResolvedHistory), BundleError> {
    let record = crate::history_record::HistoryRecord::capture(history, role)?;
    crate::store::BundleRecord::validate(&record, limits)?;
    let resolved = record
        .journal
        .resolve_verified_with_budget(
            std::sync::Arc::clone(&history.baseline),
            &history.document,
            role.replay_limit(limits),
            prefix,
        )
        .map_err(crate::bundle::history_error)?;
    if resolved.snapshot() != history {
        return Err(BundleError::Corrupt("history replay agreement".into()));
    }
    let retained = check_retained_history(resolved.snapshot(), role, limits)?;
    Ok((retained, resolved))
}

pub(crate) fn check_retained_history(
    history: &kinewright_core::HistorySnapshot,
    role: crate::history_record::HistoryRole,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<u64, BundleError> {
    if role == crate::history_record::HistoryRole::Ordinary {
        history
            .ordinary_admission_bytes()
            .map_err(crate::bundle::history_error)?;
        if crate::bundle_commit::ordinary_history_admission_cost(history, limits)?
            > limits.branch_history_bytes
        {
            return Err(BundleError::Capacity("ordinary history admission"));
        }
    }
    let retained = native_history_live_bytes(history)?;
    if retained > role.live_limit(limits) {
        return Err(BundleError::Capacity("role history memory"));
    }
    Ok(retained)
}

pub(crate) fn verify_history_lineage(
    history: &kinewright_core::HistorySnapshot,
    lineage: &crate::bundle_records::LineageRecord,
    role: crate::history_record::HistoryRole,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<(), BundleError> {
    if role != crate::history_record::HistoryRole::Ordinary {
        let live = native_history_live_bytes(history)?
            .checked_add(crate::lineage_journal::live_bytes(lineage)?)
            .ok_or(BundleError::Capacity("role history memory"))?;
        if live > role.live_limit(limits) {
            return Err(BundleError::Capacity("role history memory"));
        }
    }
    crate::lineage_journal::verify_history_boundaries(lineage, history)
}

/// Native admission includes capacities retained by a fresh decode and replay.
pub(crate) fn native_history_live_bytes(
    history: &kinewright_core::HistorySnapshot,
) -> Result<u64, BundleError> {
    kinewright_core::history_size::restored_history_live_bytes(history)
        .map_err(crate::bundle::history_error)
}
