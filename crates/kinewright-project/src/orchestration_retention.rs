//! Flat ownership for full run snapshots. Predecessors are identities, never edges.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::ObjectRef,
    orchestration_records::{OrchestrationPayload, OrchestrationRecord, Refusal, RunId},
    store::ProjectStore,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ownership", rename_all = "snake_case", deny_unknown_fields)]
pub enum Retention {
    Complete,
    Introduced {
        base: Option<ObjectRef>,
        references: Vec<ObjectRef>,
    },
}
fn invalid() -> BundleError {
    BundleError::Corrupt("run reference introductions disagree with retained predecessor".into())
}
impl OrchestrationRecord {
    pub(crate) fn retained(
        payload: OrchestrationPayload,
        base: Option<ObjectRef>,
        store: &ProjectStore,
    ) -> Result<Self, BundleError> {
        let mut record = Self {
            version: 1,
            payload,
            retention: Retention::Complete,
        };
        record.derive_retention(base, store)?;
        Ok(record)
    }
    fn derive_retention(
        &mut self,
        base: Option<ObjectRef>,
        store: &ProjectStore,
    ) -> Result<(), BundleError> {
        let before = base
            .as_ref()
            .map(|r| store.read_record::<Self>(r))
            .transpose()?;
        let references = self.introductions(before.as_deref())?;
        self.retention = Retention::Introduced { base, references };
        Ok(())
    }
    fn introductions(&self, before: Option<&Self>) -> Result<Vec<ObjectRef>, BundleError> {
        let mut references = self.full_dependencies().map_err(|_| invalid())?;
        if let Some(before) = before {
            match (&self.payload, &before.payload) {
                (OrchestrationPayload::Snapshot(after), OrchestrationPayload::Snapshot(before))
                    if after.run == before.run && after.sequence >= before.sequence => {}
                (
                    OrchestrationPayload::Accounting(after),
                    OrchestrationPayload::Accounting(before),
                ) if after.project == before.project
                    && after.run == before.run
                    && after.revision >= before.revision => {}
                (
                    OrchestrationPayload::AttemptArchive(after),
                    OrchestrationPayload::AttemptArchive(before),
                ) if after.run == before.run && after.attempts.starts_with(&before.attempts) => {}
                _ => return Err(invalid()),
            }
            let inherited = before.full_dependencies().map_err(|_| invalid())?;
            references.retain(|reference| inherited.binary_search(reference).is_err());
        }
        Ok(references)
    }
    /// The base must also be owned by the flat index; this check takes its loaded
    /// predecessor so recovery never walks a soft predecessor chain.
    pub(crate) fn validate_introductions(
        &self,
        expected_base: Option<&ObjectRef>,
        before: Option<&Self>,
    ) -> Result<(), BundleError> {
        let Retention::Introduced { base, references } = &self.retention else {
            return Err(invalid());
        };
        if base.as_ref() != expected_base || *references != self.introductions(before)? {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn validate_retained_base(
        &self,
        expected: Option<&ObjectRef>,
    ) -> Result<(), BundleError> {
        match &self.retention {
            Retention::Introduced { base, .. } if base.as_ref() == expected => Ok(()),
            _ => Err(invalid()),
        }
    }
    pub(crate) fn validate_stored_retention_kind(&self) -> Result<(), BundleError> {
        if matches!(self.retention, Retention::Complete)
            && matches!(
                self.payload,
                OrchestrationPayload::Snapshot(_)
                    | OrchestrationPayload::Accounting(_)
                    | OrchestrationPayload::AttemptArchive(_)
            )
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn validate_retention_admission(
        &self,
        store: &ProjectStore,
    ) -> Result<(), BundleError> {
        self.validate_stored_retention_kind()?;
        if let Retention::Introduced { base, .. } = &self.retention {
            let before = base
                .as_ref()
                .map(|r| store.read_record::<Self>(r))
                .transpose()?;
            self.validate_introductions(base.as_ref(), before.as_deref())?;
        }
        Ok(())
    }
    pub(crate) fn retained_dependencies(
        &self,
        full: Vec<ObjectRef>,
    ) -> Result<Vec<ObjectRef>, Refusal> {
        match &self.retention {
            Retention::Complete => Ok(full),
            Retention::Introduced { base, references } => {
                if !matches!(
                    self.payload,
                    OrchestrationPayload::Snapshot(_)
                        | OrchestrationPayload::Accounting(_)
                        | OrchestrationPayload::AttemptArchive(_)
                ) || base
                    .as_ref()
                    .is_some_and(|r| r.record_kind != crate::bundle_records::RecordKind::RunIndex)
                    || references.windows(2).any(|w| w[0] >= w[1])
                    || references.iter().any(|r| full.binary_search(r).is_err())
                {
                    return Err(Refusal::InvalidState);
                }
                Ok(references.clone())
            }
        }
    }
}
impl ProjectBundle {
    pub(crate) fn semantic_snapshot(&self, run: &RunId) -> Result<Option<ObjectRef>, BundleError> {
        Ok(self
            .run_index()?
            .orchestration
            .get(run)
            .and_then(|cuts| cuts.last())
            .map(|cut| cut.snapshot.clone()))
    }
    pub(crate) fn admit_owned_scheduler_record(
        &mut self,
        mut record: OrchestrationRecord,
    ) -> Result<crate::store::SealedObject, BundleError> {
        let run = match &record.payload {
            OrchestrationPayload::Accounting(next) => RunId(next.run.clone()),
            OrchestrationPayload::AttemptArchive(next) => next.run.clone(),
            _ => return self.store.admit_record(&record),
        };
        let snapshot = self.semantic_snapshot(&run)?;
        let base = if let Some(reference) = snapshot {
            let anchor = self.store.read_record::<OrchestrationRecord>(&reference)?;
            let OrchestrationPayload::Snapshot(state) = &anchor.payload else {
                return Err(invalid());
            };
            let mut found = None;
            for reference in &state.usage {
                let candidate = self.store.read_run_record(reference)?;
                if matches!(
                    (&record.payload, &candidate.payload),
                    (
                        OrchestrationPayload::Accounting(_),
                        OrchestrationPayload::Accounting(_)
                    ) | (
                        OrchestrationPayload::AttemptArchive(_),
                        OrchestrationPayload::AttemptArchive(_)
                    )
                ) {
                    if found.is_some() {
                        return Err(invalid());
                    }
                    found = Some(reference.clone());
                }
            }
            found
        } else {
            None
        };
        record.derive_retention(base, &self.store)?;
        self.store.admit_record(&record)
    }
}

/// Unpaid retained-graph witness; this accessor is absent from production builds.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Serialize)]
pub struct PendingTaskHistoryProfile {
    pub history_encoded: usize,
    pub history_decoded: u64,
    pub history_batches: usize,
    pub specialist_base_encoded: usize,
    pub specialist_base_decoded: u64,
    pub full_delta_encoded: usize,
    pub full_delta_decoded: u64,
    pub full_delta_operations: usize,
}
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn measure_pending_task_history(
    history: &kinewright_core::HistorySnapshot,
    base: &crate::bundle_records::SpecialistBaseRecord,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<PendingTaskHistoryProfile, BundleError> {
    fn size(
        value: &impl Serialize,
        limits: &crate::storage_budget::StorageLimits,
    ) -> Result<(usize, u64), BundleError> {
        let bytes = serde_json::to_vec(value).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        Ok((bytes.len(), crate::store::preflight_record(&bytes, limits)?))
    }
    let (history_encoded, history_decoded) = size(
        &crate::history_record::HistoryRecord::capture(history, base.history_role)?,
        limits,
    )?;
    let (specialist_base_encoded, specialist_base_decoded) = size(base, limits)?;
    let (full_delta_encoded, full_delta_decoded) = size(&base.full_applied_delta, limits)?;
    Ok(PendingTaskHistoryProfile {
        history_encoded,
        history_decoded,
        history_batches: history.batches.len(),
        specialist_base_encoded,
        specialist_base_decoded,
        full_delta_encoded,
        full_delta_decoded,
        full_delta_operations: base.full_applied_delta.len(),
    })
}

#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Serialize)]
pub struct RetainedRunProfile {
    pub objects: u64,
    pub declared_objects: usize,
    pub edges: usize,
    pub physical_edges: u64,
    pub manifest_encoded: usize,
    pub manifest_decoded: u64,
    pub max_record_encoded: u64,
    pub max_record_decoded: u64,
    pub max_history_encoded: u64,
    pub max_history_decoded: u64,
    pub max_accounting_encoded: usize,
    pub snapshot_count: usize,
    pub snapshot_encoded_total: u64,
    pub snapshot_decoded_total: u64,
    pub snapshot_encoded_max: u64,
    pub snapshot_decoded_max: u64,
    pub run_closure_disk_bytes: u64,
    pub project_objects_disk_bytes: u64,
    pub physical_bytes: u64,
    pub peak_physical_bytes: u64,
    pub peak_reserved_bytes: u64,
    pub peak_decoded_bytes: u64,
    pub peak_live_history_bytes: u64,
    pub metadata_bytes: u64,
    pub owned_bytes: u64,
    pub live_history_bytes: u64,
    pub graph_depth: u64,
    pub receipts: usize,
    pub issuers: usize,
    pub ordinary_cuts: usize,
    pub terminal_cuts: usize,
    pub transition_encoded: usize,
    pub ordinary_transition_encoded: usize,
}
#[cfg(any(test, feature = "test-util"))]
impl crate::editing_session::Ja1EditingSession {
    #[allow(
        clippy::too_many_lines,
        reason = "one unpaid snapshot reports every retained-record and graph quota"
    )]
    pub fn retained_run_profile(&self, run: &RunId) -> Result<RetainedRunProfile, BundleError> {
        let bundle = self.active_bundle()?;
        let store = self.store()?;
        let manifest_ref = &bundle.root.as_ref().ok_or_else(invalid)?.store_manifest_ref;
        let manifest = store.read_record::<crate::store::StoreManifest>(manifest_ref)?;
        let bytes =
            serde_json::to_vec(&*manifest).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let usage = store.usage()?;
        let mut max_record_decoded = 0;
        let mut max_history_encoded = 0;
        let mut max_history_decoded = 0;
        let mut max_accounting_encoded = 0;
        let mut snapshot_count = 0;
        let mut snapshot_encoded_total = 0;
        let mut snapshot_decoded_total = 0;
        let mut snapshot_encoded_max = 0;
        let mut snapshot_decoded_max = 0;
        for reference in &manifest.objects {
            if !reference.record_kind.is_owned_bytes() {
                store.verify(reference)?;
                let path = crate::namespace::PortablePath::new(
                    &std::path::Path::new("studio/objects").join(&reference.sha256),
                )?;
                let file = store
                    .with_namespace(|n| n.open_owned(&path))?
                    .ok_or_else(invalid)?;
                let bytes = crate::bundle_recovery::bounded_bytes(file, reference.byte_length)?;
                let decoded = crate::store::preflight_record(&bytes, store.limits())?;
                max_record_decoded = max_record_decoded.max(decoded);
                if reference.record_kind == crate::bundle_records::RecordKind::History {
                    max_history_encoded = max_history_encoded.max(reference.byte_length);
                    max_history_decoded = max_history_decoded.max(decoded);
                }
                if reference.record_kind == crate::bundle_records::RecordKind::RunIndex {
                    let record: crate::orchestration_commit::RegisteredRunIndex =
                        serde_json::from_slice(&bytes)
                            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                    if let crate::orchestration_commit::RegisteredRunIndex::Record(record) = record
                    {
                        match record.payload {
                            OrchestrationPayload::Accounting(record) => {
                                max_accounting_encoded = max_accounting_encoded.max(
                                    crate::orchestration_accounting::encode(
                                        &record,
                                        crate::run_admission::ACCOUNTING_BYTES,
                                    )
                                    .map_err(|_| invalid())?
                                    .len(),
                                );
                            }
                            OrchestrationPayload::Snapshot(record) if &record.run == run => {
                                snapshot_count += 1;
                                snapshot_encoded_total += reference.byte_length;
                                snapshot_decoded_total += decoded;
                                snapshot_encoded_max =
                                    snapshot_encoded_max.max(reference.byte_length);
                                snapshot_decoded_max = snapshot_decoded_max.max(decoded);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        let index = bundle.run_index()?;
        let cuts = index.orchestration.get(run).ok_or_else(invalid)?;
        let roots = cuts
            .iter()
            .flat_map(crate::orchestration_commit::PublishedRunCut::references)
            .chain(index.cumulative.get(run).cloned())
            .collect::<Vec<_>>();
        let run_closure_disk_bytes = store.closure(&roots)?.iter().map(|r| r.byte_length).sum();
        let mut transition_encoded = 0;
        let mut ordinary_transition_encoded = 0;
        let mut before_pending = true;
        let mut previous = None;
        for cut in cuts {
            let snapshot = store.read_record::<OrchestrationRecord>(&cut.snapshot)?;
            let segment = store.read_record::<OrchestrationRecord>(&cut.segment)?;
            let transition = cut.validate_cut(run, &snapshot, &segment, previous)?;
            let bytes = serde_json::to_vec(&transition)
                .map_err(|e| BundleError::Corrupt(e.to_string()))?
                .len();
            transition_encoded = transition_encoded.max(bytes);
            if before_pending {
                ordinary_transition_encoded = ordinary_transition_encoded.max(bytes);
            }
            before_pending = transition.projection().outcome
                == crate::orchestration_records::RunOutcome::Pending;
            previous = Some(cut);
        }
        let budget = index.budgets.get(run).ok_or_else(invalid)?;
        Ok(RetainedRunProfile {
            objects: usage.objects,
            declared_objects: manifest.objects.len(),
            physical_edges: usage.edges,
            edges: manifest.dependencies.values().map(Vec::len).sum::<usize>()
                + manifest
                    .inventory_dependencies
                    .values()
                    .map(Vec::len)
                    .sum::<usize>(),
            manifest_encoded: bytes.len(),
            manifest_decoded: crate::store::preflight_record(&bytes, store.limits())?,
            max_record_encoded: manifest
                .objects
                .iter()
                .filter(|r| !r.record_kind.is_owned_bytes())
                .map(|r| r.byte_length)
                .max()
                .unwrap_or(0)
                .max(bytes.len() as u64),
            max_record_decoded: max_record_decoded
                .max(crate::store::preflight_record(&bytes, store.limits())?),
            max_history_encoded,
            max_history_decoded,
            max_accounting_encoded,
            snapshot_count,
            snapshot_encoded_total,
            snapshot_decoded_total,
            snapshot_encoded_max,
            snapshot_decoded_max,
            run_closure_disk_bytes,
            project_objects_disk_bytes: manifest.objects.iter().map(|r| r.byte_length).sum::<u64>()
                + bytes.len() as u64,
            physical_bytes: usage.physical_bytes,
            peak_physical_bytes: usage.peak_physical_bytes,
            peak_reserved_bytes: usage.peak_reserved_bytes,
            peak_decoded_bytes: usage.peak_decoded_bytes,
            peak_live_history_bytes: usage.peak_live_history_bytes,
            metadata_bytes: usage.metadata_bytes,
            owned_bytes: usage.owned_bytes,
            live_history_bytes: usage.live_history_bytes,
            graph_depth: store.maximum_committed_depth()?,
            receipts: bundle.receipts.detailed.len() + bundle.receipts.retired.len(),
            issuers: bundle.issuers.len(),
            ordinary_cuts: budget.ordinary,
            terminal_cuts: budget.terminal,
            transition_encoded,
            ordinary_transition_encoded,
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "orchestration_retention_tests.rs"]
mod tests;

#[cfg(any(test, feature = "test-util"))]
impl crate::editing_session::Ja1EditingSession {
    /// Collect, then verify all retained cuts and their complete reference sets.
    pub fn verify_retained_run_after_collection(
        &mut self,
        run: &RunId,
    ) -> Result<usize, BundleError> {
        fn addresses(
            value: &serde_json::Value,
            refs: &mut std::collections::BTreeSet<ObjectRef>,
        ) -> Result<(), BundleError> {
            match value {
                serde_json::Value::Object(fields)
                    if fields.len() == 3
                        && fields.contains_key("sha256")
                        && fields.contains_key("byte_length")
                        && fields.contains_key("record_kind") =>
                {
                    refs.insert(
                        ObjectRef::deserialize(value)
                            .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                    );
                }
                serde_json::Value::Object(fields) => {
                    for value in fields.values() {
                        addresses(value, refs)?;
                    }
                }
                serde_json::Value::Array(values) => {
                    for value in values {
                        addresses(value, refs)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        let bundle = self.active_bundle_mut()?;
        bundle.collect_after_ack()?;
        let manifest = bundle.store.read_record::<crate::store::StoreManifest>(
            &bundle.root.as_ref().ok_or_else(invalid)?.store_manifest_ref,
        )?;
        let declared = manifest
            .objects
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        let mut required = std::collections::BTreeSet::new();
        for reference in &manifest.objects {
            bundle.store.verify(reference)?;
            if reference.record_kind == crate::bundle_records::RecordKind::RunIndex
                && let crate::orchestration_commit::RegisteredRunIndex::Record(record) = &*bundle
                    .store
                    .read_record::<crate::orchestration_commit::RegisteredRunIndex>(reference)?
            {
                // Independent oracle: inspect every typed address in the raw
                // stored contents, including inherited and soft references.
                let value = serde_json::to_value(record)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                addresses(&value, &mut required)?;
            }
        }
        for reference in &required {
            if !declared.contains(reference) {
                return Err(invalid());
            }
            bundle.store.verify(reference)?;
        }
        let index = bundle.run_index()?;
        let cuts = index.orchestration.get(run).ok_or_else(invalid)?;
        let mut previous = None;
        for cut in cuts {
            let snapshot = bundle
                .store
                .read_record::<OrchestrationRecord>(&cut.snapshot)?;
            let segment = bundle
                .store
                .read_record::<OrchestrationRecord>(&cut.segment)?;
            cut.validate_cut(run, &snapshot, &segment, previous)?;
            previous = Some(cut);
        }
        Ok(cuts.len())
    }
}
