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
pub(crate) fn resolve_history_journal(
    journal: &kinewright_core::HistoryJournal,
    baseline: std::sync::Arc<kinewright_core::Document>,
    current: &kinewright_core::Document,
    role: crate::history_record::HistoryRole,
    window: u64,
) -> Result<kinewright_core::HistorySnapshot, BundleError> {
    if role == crate::history_record::HistoryRole::Ordinary {
        journal
            .resolve_canonical_with_budget(&baseline, current, window, None)
            .map(kinewright_core::ResolvedHistory::into_snapshot)
    } else {
        journal
            .resolve_with_budget(baseline, current, window)
            .map(|(history, _)| history)
    }
    .map_err(crate::bundle::history_error)
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
    let resolved = if role == crate::history_record::HistoryRole::Ordinary {
        record.journal.resolve_canonical_with_budget(
            &history.baseline,
            &history.document,
            role.replay_limit(limits),
            prefix,
        )
    } else {
        record.journal.resolve_verified_with_budget(
            std::sync::Arc::clone(&history.baseline),
            &history.document,
            role.replay_limit(limits),
            prefix,
        )
    }
    .map_err(crate::bundle::history_error)?;
    if resolved.snapshot() != history {
        return Err(BundleError::Corrupt("history replay agreement".into()));
    }
    let retained = check_retained_history_inner(
        resolved.snapshot(),
        role,
        limits,
        Some(&resolved),
        Some(&record),
        Some(resolved.snapshot()),
    )?;
    Ok((retained, resolved))
}

pub(crate) fn check_retained_history(
    history: &kinewright_core::HistorySnapshot,
    role: crate::history_record::HistoryRole,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<u64, BundleError> {
    check_retained_history_inner(history, role, limits, None, None, None)
}
fn check_retained_history_inner(
    history: &kinewright_core::HistorySnapshot,
    role: crate::history_record::HistoryRole,
    limits: &crate::storage_budget::StorageLimits,
    resolved: Option<&kinewright_core::ResolvedHistory>,
    record: Option<&crate::history_record::HistoryRecord>,
    input: Option<&kinewright_core::HistorySnapshot>,
) -> Result<u64, BundleError> {
    if role == crate::history_record::HistoryRole::Ordinary {
        history
            .ordinary_admission_bytes()
            .map_err(crate::bundle::history_error)?;
        let admission = crate::bundle_commit::ordinary_history_admission_cost(history, limits)?;
        if admission > limits.branch_history_bytes {
            return Err(BundleError::Capacity("ordinary history admission"));
        }
        if !history.cursor.redo.is_empty() {
            let captured;
            let record = if let Some(record) = record {
                record
            } else {
                captured = crate::history_record::HistoryRecord::capture(history, role)?;
                &captured
            };
            let bound = match resolved {
                Some(proof) => proof.redo_capacity_bound(&record.journal, input.unwrap_or(history)),
                None => history.redo_capacity_bound(role.replay_limit(limits), &record.journal),
            }
            .map_err(crate::bundle::history_error)?;
            check_redo_reservation(history, admission, bound, limits, record)?;
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

// Ordinary cost moves a document from the cursor to the header and back; the
// document/operation sum stays fixed. Only the new payload-free batches grow.
fn check_redo_reservation(
    history: &kinewright_core::HistorySnapshot,
    admission: u64,
    bound: kinewright_core::history_size::RedoCapacityBound,
    limits: &crate::storage_budget::StorageLimits,
    record: &crate::history_record::HistoryRecord,
) -> Result<(), BundleError> {
    let capacity = || BundleError::Capacity("ordinary redo reservation");
    let count = history.cursor.redo.len() as u64;
    let zero = kinewright_core::JournalBatch {
        order: 0,
        command: kinewright_core::JournalCommand::Redo,
        identity_effects: vec![],
        undo_entries: 0,
        redo_entries: 0,
    };
    let bytes = serde_json::to_vec(&zero).map_err(crate::bundle::encoding_error)?;
    let structural = crate::store::preflight_record(&bytes, limits)?
        .checked_sub(2 * bytes.len() as u64)
        .ok_or_else(capacity)?;
    let added = bound
        .encoded_batch_bytes
        .checked_mul(2)
        .and_then(|n| {
            structural
                .checked_mul(count)
                .and_then(|nodes| n.checked_add(nodes))
        })
        .ok_or_else(capacity)?;
    let ordinary = admission.checked_add(added).ok_or_else(capacity)?;
    let role = crate::history_record::HistoryRole::Ordinary;
    if ordinary > limits.branch_history_bytes
        || bound.retained_bytes > role.live_limit(limits)
        || bound.replay_bytes > role.replay_limit(limits)
    {
        return Err(capacity());
    }
    let commas = u64::from(!history.cursor.undo.is_empty());
    let transport = history
        .ordinary_admission_bytes()
        .map_err(crate::bundle::history_error)?
        .checked_add(bound.encoded_batch_bytes)
        .and_then(|n| n.checked_add(commas))
        .ok_or_else(capacity)?;
    if transport > kinewright_core::HISTORY_BYTES {
        return Err(capacity());
    }
    let encoded = serde_json::to_vec(&record).map_err(crate::bundle::encoding_error)?;
    let digits = |value: u64| u64::from(value.checked_ilog10().unwrap_or(0)) + 1;
    let reference_growth = 2 * digits(bound.max_current_document_bytes)
        .saturating_sub(digits(record.current_ref.byte_length));
    let record_bound = crate::store::preflight_record(&encoded, limits)?
        .checked_add(added)
        .and_then(|n| n.checked_add(2 * commas))
        .and_then(|n| n.checked_add(reference_growth))
        .ok_or_else(capacity)?;
    if record_bound > limits.branch_history_bytes {
        return Err(capacity());
    }
    Ok(())
}
