//! Session-local verified identities supply costs, never current-branch membership.
use super::{
    account_graph_storage, check_encoded_record, compare_declared_edges, compare_edges,
    decoded_reservations, inspect_checkpoints, read_record, validate_archived_root,
    validate_evidence, validate_manifest_apis, walk,
};
use crate::{
    bundle::{BundleError, BundleWriteAdmission},
    bundle_records::{BranchCheckpoint, BranchId, LineageRecord, ObjectRef, RecordKind},
    namespace::ProjectNamespace,
    owned_inputs::OwnedInputManifest,
    storage_budget::{StorageLimits, StorageReservation, StorageUsage},
    store::{BundleRecord, ProjectStore, StoreManifest},
};
use kinewright_core::{Document, HistorySnapshot};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct VerifiedIdentityMemo {
    profile: Option<StorageLimits>,
    records: BTreeMap<ObjectRef, RecordCost>,
}
struct RecordCost {
    decoded: u64,
    fields: Option<BTreeSet<String>>,
    dependencies: Vec<ObjectRef>,
    inventory_dependencies: Vec<ObjectRef>,
}
impl VerifiedIdentityMemo {
    pub(super) fn verify_identity(
        &mut self,
        namespace: &ProjectNamespace,
        reference: &ObjectRef,
        usage: &Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
    ) -> Result<(), BundleError> {
        self.bind(limits);
        if self.records.contains_key(reference) {
            drop(StorageReservation::reserve(
                usage,
                limits,
                0,
                1 << 20,
                0,
                false,
                u64::MAX,
            )?);
        } else {
            ProjectStore::verify_in_namespace(namespace, reference, usage, limits)?;
            if reference.record_kind.is_owned_bytes()
                && (self.records.len() as u64) < limits.objects
            {
                self.records.insert(
                    reference.clone(),
                    RecordCost {
                        decoded: 0,
                        fields: Some(BTreeSet::new()),
                        dependencies: vec![],
                        inventory_dependencies: vec![],
                    },
                );
            }
        }
        Ok(())
    }
    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.profile = None;
    }
    fn bind(&mut self, limits: &StorageLimits) {
        if self.profile.as_ref() != Some(limits) {
            self.clear();
            self.profile = Some(limits.clone());
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn misbind_probe(&mut self, reference: &ObjectRef, wrong: ObjectRef) {
        if let Some(cost) = self.records.remove(reference) {
            self.records.insert(wrong, cost);
        }
    }
    pub(crate) fn forget(&mut self, reference: &ObjectRef) {
        self.records.remove(reference);
    }
    pub(crate) fn retain(&mut self, references: &BTreeSet<ObjectRef>) {
        self.records
            .retain(|reference, _| references.contains(reference));
    }
    pub(crate) fn remember<T: BundleRecord>(
        &mut self,
        reference: &ObjectRef,
        record: &T,
        decoded: u64,
        fields: Option<BTreeSet<String>>,
        limits: &StorageLimits,
    ) {
        // Traversal controls are interpreted on each admission. Only payload costs are memoized.
        if matches!(
            reference.record_kind,
            RecordKind::BranchIndex
                | RecordKind::BranchCheckpoint
                | RecordKind::InputManifest
                | RecordKind::InputChunkTable
                | RecordKind::StoreManifest
                | RecordKind::RootCheckpoint
        ) {
            return;
        }
        self.bind(limits);
        if !self.records.contains_key(reference) && self.records.len() as u64 >= limits.objects {
            return; // A bounded cache miss always takes the exact reader path.
        }
        let needs_fields = matches!(
            reference.record_kind,
            RecordKind::Document
                | RecordKind::History
                | RecordKind::Lineage
                | RecordKind::SpecialistBase
        );
        self.records.insert(
            reference.clone(),
            RecordCost {
                decoded,
                fields: fields
                    .or_else(|| {
                        self.records
                            .get(reference)
                            .and_then(|prior| prior.fields.clone())
                    })
                    .or_else(|| (!needs_fields).then(BTreeSet::new)),
                dependencies: record.dependencies(),
                inventory_dependencies: record.inventory_dependencies(),
            },
        );
    }
    pub(crate) fn remember_raw(
        &mut self,
        reference: &ObjectRef,
        decoded: u64,
        fields: BTreeSet<String>,
        dependencies: Vec<ObjectRef>,
        inventory_dependencies: Vec<ObjectRef>,
        limits: &StorageLimits,
    ) {
        self.bind(limits);
        if self.records.contains_key(reference) || (self.records.len() as u64) < limits.objects {
            self.records.insert(
                reference.clone(),
                RecordCost {
                    decoded,
                    fields: Some(fields),
                    dependencies,
                    inventory_dependencies,
                },
            );
        }
    }
    pub(crate) fn cached_fields(
        &mut self,
        reference: &ObjectRef,
        limits: &StorageLimits,
    ) -> Option<BTreeSet<String>> {
        self.bind(limits);
        self.records
            .get(reference)
            .and_then(|cost| cost.fields.clone())
    }
    pub(crate) fn remember_fields(
        &mut self,
        reference: &ObjectRef,
        fields: &BTreeSet<String>,
        limits: &StorageLimits,
    ) {
        self.bind(limits);
        if let Some(cost) = self.records.get_mut(reference) {
            cost.fields = Some(fields.clone());
        }
    }
    fn read<T: BundleRecord>(
        &mut self,
        namespace: &ProjectNamespace,
        reference: &ObjectRef,
        usage: &Arc<Mutex<StorageUsage>>,
        fields: &mut BTreeSet<String>,
        manifest: &StoreManifest,
        limits: &StorageLimits,
    ) -> Result<StorageReservation, BundleError> {
        self.bind(limits);
        if let Some(cost) = self.records.get(reference)
            && let Some(observed) = &cost.fields
        {
            if reference.record_kind != T::KIND {
                return Err(BundleError::Corrupt("memo record kind".into()));
            }
            compare_declared_edges(
                reference,
                &cost.dependencies,
                &cost.inventory_dependencies,
                manifest,
            )?;
            fields.extend(observed.iter().cloned());
            #[cfg(any(test, feature = "test-util"))]
            crate::namespace::note("publication_memo_hit", &namespace.identity().resolved_root);
            let decoded = cost.decoded;
            return model_record_read(reference, decoded, usage, limits);
        }
        #[cfg(any(test, feature = "test-util"))]
        crate::namespace::note("publication_memo_miss", &namespace.identity().resolved_root);
        let mut observed = BTreeSet::new();
        let value = read_record::<T>(namespace, reference, usage, &mut observed, limits)?;
        compare_edges(reference, &*value, manifest)?;
        self.remember(
            reference,
            &*value,
            value.decoded_charge(),
            Some(observed.clone()),
            limits,
        );
        fields.extend(observed);
        Ok(value.map(|_| ()))
    }
}

/// Reproduce the reader's encoded, hash-working, preflight, raw/typed and retained charges.
/// No decoded payload is retained in the memo or in these operation-local model reservations.
fn model_record_read(
    reference: &ObjectRef,
    decoded: u64,
    usage: &Arc<Mutex<StorageUsage>>,
    limits: &StorageLimits,
) -> Result<StorageReservation, BundleError> {
    check_encoded_record(reference, limits)?;
    let _encoded =
        StorageReservation::reserve(usage, limits, 0, reference.byte_length, 0, false, u64::MAX)?;
    drop(StorageReservation::reserve(
        usage,
        limits,
        0,
        1 << 20,
        0,
        false,
        u64::MAX,
    )?);
    drop(StorageReservation::reserve(
        usage,
        limits,
        0,
        limits.decoded_record_bytes,
        0,
        false,
        u64::MAX,
    )?);
    let (value, shadow) = decoded_reservations(decoded, reference.record_kind, usage, limits)?;
    drop(shadow);
    Ok(value)
}

#[allow(
    clippy::too_many_lines,
    reason = "the reader's ordered current and archived checkpoint accounting schedule"
)]
pub(super) fn validate(
    namespace: &ProjectNamespace,
    bytes: &[u8],
    limits: &StorageLimits,
    memo: &mut VerifiedIdentityMemo,
) -> Result<(), BundleError> {
    limits.validate()?;
    let admission = BundleWriteAdmission::from_bytes_with_limits(
        Some(namespace.identity().resolved_root.clone()),
        bytes.to_vec(),
        Arc::new(Mutex::new(StorageUsage::default())),
        limits,
    )?;
    admission.require_supported()?;
    let root = admission
        .data
        .file
        .as_ref()
        .and_then(|f| f.studio.as_ref())
        .ok_or_else(|| BundleError::Unsupported("not a Studio bundle".into()))?;
    let usage = &admission.data.usage;
    let mut fields = admission.data.inventory.required_fields.clone();
    let manifest = read_record::<StoreManifest>(
        namespace,
        &root.store_manifest_ref,
        usage,
        &mut BTreeSet::new(),
        limits,
    )?;
    if manifest.project_id.as_ref() != Some(&root.project_id)
        || manifest.session_id.is_some()
        || manifest.generation != root.generation
    {
        return Err(BundleError::Corrupt("publication store binding".into()));
    }
    validate_manifest_apis(&manifest, root.manual_switch_generation)?;
    account_graph_storage(namespace, &admission, &manifest, limits, Some(memo))?;
    let mut objects: BTreeMap<_, _> = manifest
        .objects
        .iter()
        .map(|r| (r.sha256.clone(), r.clone()))
        .collect();
    if objects.len() != manifest.objects.len()
        || objects
            .insert(
                root.store_manifest_ref.sha256.clone(),
                root.store_manifest_ref.clone(),
            )
            .is_some()
        || manifest.dependencies.len() != manifest.objects.len()
        || manifest
            .inventory_dependencies
            .keys()
            .any(|h| !objects.contains_key(h))
    {
        return Err(BundleError::Corrupt(
            "publication object declarations".into(),
        ));
    }
    let kinds: BTreeSet<_> = objects.values().map(|r| r.record_kind).collect();
    if kinds != manifest.required_kinds {
        return Err(BundleError::Unsupported(
            "publication kind inventory".into(),
        ));
    }
    let mut found = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for reference in root.references() {
        walk(
            &reference,
            &objects,
            &manifest,
            &mut visited,
            &mut BTreeSet::new(),
            &mut found,
            true,
            0,
            limits,
        )?;
    }
    if found.len() != objects.len() {
        return Err(BundleError::Corrupt(
            "unreachable publication object".into(),
        ));
    }
    // Cache pruning is bounded by the exact candidate plus its independently retained inventory.
    let retained: BTreeSet<_> = objects.into_values().collect();
    memo.retain(&retained);
    #[cfg(any(test, feature = "test-util"))]
    let initially_verified: BTreeSet<_> = memo.records.keys().cloned().collect();
    let mut checkpoints = BTreeMap::new();
    let mut indexes = BTreeMap::new();
    let mut inputs = Vec::new();
    let mut tables = BTreeMap::new();
    for reference in &manifest.objects {
        macro_rules! payload {
            ($ty:ty) => {
                drop(memo.read::<$ty>(
                    namespace,
                    reference,
                    usage,
                    &mut fields,
                    &manifest,
                    limits,
                )?);
            };
        }
        macro_rules! control {
            ($ty:ty) => {{
                let value = read_record::<$ty>(namespace, reference, usage, &mut fields, limits)?;
                compare_edges(reference, &*value, &manifest)?;
                value
            }};
        }
        match reference.record_kind {
            RecordKind::VersionIndex => {
                control!(crate::studio_records::VersionIndex);
            }
            RecordKind::FeedbackIndex => {
                control!(crate::studio_records::FeedbackIndex);
            }
            RecordKind::FeedbackRevision => {
                control!(crate::studio_records::FeedbackRevision);
            }
            RecordKind::FeedbackBatch => {
                control!(crate::studio_records::FeedbackBatch);
            }
            RecordKind::RunIndex => {
                control!(crate::studio_records::RunIndex);
            }
            RecordKind::RunSnapshot => {
                control!(crate::studio_records::RunSnapshotRecord);
            }
            RecordKind::RunEventSegment => {
                control!(crate::studio_records::RunEventSegment);
            }
            RecordKind::RunEffect => {
                control!(crate::studio_records::RunEffectRecord);
            }
            RecordKind::RunRequest => {
                control!(crate::studio_records::RunRequestRecord);
            }
            RecordKind::RunDelivery => {
                control!(crate::studio_records::RunDeliveryRecord);
            }
            RecordKind::RunResult => {
                control!(crate::studio_records::RunResultRecord);
            }
            RecordKind::RunUsage => {
                control!(crate::studio_records::RunUsageRecord);
            }
            RecordKind::Document => {
                payload!(Document);
            }
            RecordKind::History => {
                payload!(HistorySnapshot);
            }
            RecordKind::Lineage => {
                payload!(LineageRecord);
            }
            RecordKind::SpecialistBase => {
                payload!(crate::bundle_records::SpecialistBaseRecord);
            }
            RecordKind::HistoryFloor => {
                payload!(crate::bundle_records::HistoryFloorCheckpoint);
            }
            RecordKind::RecoveryClose => {
                payload!(crate::bundle_records::RecoveryCloseCheckpoint);
            }
            RecordKind::ReceiptIndex => {
                let value = control!(crate::receipts::ReceiptLedger);
                super::validate_switch_receipts(
                    namespace,
                    &value,
                    &manifest,
                    usage,
                    &mut fields,
                    limits,
                )?;
            }
            RecordKind::IssuerState => {
                payload!(BTreeMap<crate::receipts::IssuerId, crate::receipts::IssuerState>);
            }
            RecordKind::BranchCheckpoint => {
                checkpoints.insert(reference.sha256.clone(), control!(BranchCheckpoint));
            }
            RecordKind::BranchIndex => {
                indexes.insert(
                    reference.sha256.clone(),
                    control!(BTreeMap<BranchId, ObjectRef>),
                );
            }
            RecordKind::InputManifest => {
                inputs.push(control!(OwnedInputManifest));
            }
            RecordKind::InputChunkTable => {
                let value = control!(crate::owned_inputs::InputChunkTable);
                let _encoded = StorageReservation::reserve(
                    usage,
                    limits,
                    0,
                    reference.byte_length,
                    0,
                    false,
                    u64::MAX,
                )?;
                let canonical =
                    serde_json::to_vec(&*value).map_err(|e| BundleError::Corrupt(e.to_string()))?;
                if canonical.len() as u64 != reference.byte_length
                    || kinewright_media::sha256_bytes(&canonical) != reference.sha256
                {
                    return Err(BundleError::Corrupt("noncanonical chunk table".into()));
                }
                tables.insert(reference.clone(), value);
            }
            RecordKind::StoreManifest => {
                let value = control!(StoreManifest);
                validate_manifest_apis(&value, 0)?;
            }
            RecordKind::RootCheckpoint => {
                validate_archived_root(
                    namespace,
                    reference,
                    usage,
                    &mut fields,
                    &manifest,
                    limits,
                )?;
            }
            RecordKind::OwnedEvidence => {
                validate_evidence(namespace, reference, usage, &manifest, limits)?;
            }
            kind if kind.is_owned_bytes() => {
                memo.verify_identity(namespace, reference, usage, limits)?;
            }
            _ => return Err(BundleError::Unsupported("inactive record producer".into())),
        }
    }
    for input in &inputs {
        for entry in &input.entries {
            if entry.object.record_kind == RecordKind::OwnedMedia {
                tables
                    .get(entry.chunk_table_ref()?)
                    .ok_or_else(|| BundleError::Corrupt("missing bound chunk table".into()))?
                    .validate_binding(&entry.object, limits)?;
            }
        }
    }
    super::validate_studio_records(namespace, root, usage, limits)?;
    if fields != manifest.required_fields {
        return Err(BundleError::Unsupported(
            "raw field inventory differs from declaration".into(),
        ));
    }
    let index = indexes
        .get(&root.branch_index_ref.sha256)
        .ok_or_else(|| BundleError::Corrupt("missing branch index".into()))?;
    let candidates: Vec<_> = index
        .keys()
        .filter(|id| **id != root.manual_branch_id)
        .cloned()
        .collect();
    if candidates != root.candidate_branch_ids || !index.contains_key(&root.manual_branch_id) {
        return Err(BundleError::Corrupt(
            "candidate branch inventory mismatch".into(),
        ));
    }
    let mut histories: BTreeMap<String, Arc<StorageReservation>> = BTreeMap::new();
    let mut lineages: BTreeMap<String, Arc<StorageReservation>> = BTreeMap::new();
    let inspect = |id: &BranchId,
                   checkpoint: StorageReservation<BranchCheckpoint>,
                   active: bool| {
        let _ = id;
        #[cfg(any(test, feature = "test-util"))]
        let _ = &initially_verified;
        let history = if active && let Some(value) = histories.get(&checkpoint.history_ref.sha256) {
            Arc::clone(value)
        } else {
            Arc::new(memo.read::<HistorySnapshot>(
                namespace,
                &checkpoint.history_ref,
                usage,
                &mut fields,
                &manifest,
                limits,
            )?)
        };
        let lineage = if active && let Some(value) = lineages.get(&checkpoint.lineage_ref.sha256) {
            Arc::clone(value)
        } else {
            Arc::new(memo.read::<LineageRecord>(
                namespace,
                &checkpoint.lineage_ref,
                usage,
                &mut fields,
                &manifest,
                limits,
            )?)
        };
        let _document = memo.read::<Document>(
            namespace,
            &checkpoint.document_ref,
            usage,
            &mut fields,
            &manifest,
            limits,
        )?;
        let _baseline = memo.read::<Document>(
            namespace,
            &checkpoint.baseline_ref,
            usage,
            &mut fields,
            &manifest,
            limits,
        )?;
        let charge = history.decoded_charge();
        let _replay =
            StorageReservation::reserve(usage, limits, 0, charge, charge, false, u64::MAX)?;
        if active {
            histories.insert(checkpoint.history_ref.sha256.clone(), Arc::clone(&history));
            lineages.insert(checkpoint.lineage_ref.sha256.clone(), Arc::clone(&lineage));
        }
        Ok((
            checkpoint,
            active.then_some(history),
            active.then_some(lineage),
        ))
    };
    let _resident_checkpoints = inspect_checkpoints(index, checkpoints, inspect)?;
    let _receipts = root
        .receipt_index_ref
        .as_ref()
        .map(|r| {
            memo.read::<crate::receipts::ReceiptLedger>(
                namespace,
                r,
                usage,
                &mut fields,
                &manifest,
                limits,
            )
        })
        .transpose()?;
    let _issuers = root
        .issuer_state_ref
        .as_ref()
        .map(|r| {
            memo.read::<BTreeMap<crate::receipts::IssuerId, crate::receipts::IssuerState>>(
                namespace,
                r,
                usage,
                &mut fields,
                &manifest,
                limits,
            )
        })
        .transpose()?;
    #[cfg(any(test, feature = "test-util"))]
    crate::io_observer::note_publication_usage(
        &*usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?,
    );
    Ok(())
}
