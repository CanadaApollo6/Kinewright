//! Read-only graph admission. Damaged roots/objects remain untouched for inspection.
use crate::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, SupportInventory},
    bundle_records::{BranchCheckpoint, BranchId, LineageRecord, ObjectRef, RecordKind},
    namespace::{PortablePath, ProjectNamespace},
    owned_inputs::OwnedInputManifest,
    storage_budget::{StorageLimits, StorageReservation, StorageUsage},
    store::{BundleRecord, ProjectStore, StoreManifest},
};
use kinewright_core::{Document, HistorySnapshot};
use serde::de::DeserializeOwned;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    sync::{Arc, Mutex},
};

pub(crate) fn inventory_path() -> Result<PortablePath, BundleError> {
    PortablePath::new(Path::new("studio/recovery/inventory"))
}
/// Extend the selected graph with the durable B3 inventory's content closure.
/// Archived automatic inventories are excluded by the same walk used on ordinary recovery.
#[allow(
    clippy::too_many_lines,
    reason = "one bounded inventory graph adoption before any residual deletion"
)]
pub(crate) fn retain_inventory_graph(
    namespace: &ProjectNamespace,
    retained: &mut StoreManifest,
    verified: &mut BTreeSet<String>,
    usage: &Arc<Mutex<StorageUsage>>,
) -> Result<(), BundleError> {
    let Some(bytes) = namespace.read(&inventory_path()?, StorageLimits::default().control_bytes)?
    else {
        return Ok(());
    };
    let inventory: StorageReservation<crate::store::RecoveryInventory> = decode(
        &bytes,
        usage,
        RecordKind::RootCheckpoint,
        &mut BTreeSet::new(),
    )?;
    inventory.validate()?;
    for reference in inventory.references() {
        if verified.contains(&reference.sha256) {
            continue;
        }
        let (bytes, _charge) = read_bytes(namespace, &reference, usage)?;
        let admission = BundleWriteAdmission::from_bytes(None, bytes)?;
        admission.require_supported()?;
        let Some(root) = admission.data.file.as_ref().and_then(|f| f.studio.as_ref()) else {
            // A pre-migration backup has no child graph.
            retained
                .dependencies
                .insert(reference.sha256.clone(), vec![]);
            retained.objects.push(reference.clone());
            verified.insert(reference.sha256);
            continue;
        };
        let manifest = read_record::<StoreManifest>(
            namespace,
            &root.store_manifest_ref,
            usage,
            &mut BTreeSet::new(),
        )?;
        let mut objects: BTreeMap<_, _> = manifest
            .objects
            .iter()
            .map(|r| (r.sha256.clone(), r.clone()))
            .collect();
        objects.insert(
            root.store_manifest_ref.sha256.clone(),
            root.store_manifest_ref.clone(),
        );
        let mut found = BTreeSet::new();
        for child in root.references() {
            walk(
                &child,
                &objects,
                &manifest,
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
                &mut found,
                false,
                0,
            )?;
        }
        if retained
            .objects
            .len()
            .checked_add(found.iter().filter(|h| !verified.contains(*h)).count() + 1)
            .is_none_or(|n| n as u64 > StorageLimits::default().objects)
        {
            return Err(BundleError::Capacity("recovery inventory object overlap"));
        }
        for hash in found {
            let object = &objects[&hash];
            if verified.contains(&hash) {
                continue;
            }
            ProjectStore::verify_in_namespace(namespace, object, usage, &StorageLimits::default())?;
            let dependencies = if *object == root.store_manifest_ref {
                manifest.durable_pins.clone()
            } else {
                manifest
                    .dependencies
                    .get(&hash)
                    .cloned()
                    .ok_or_else(|| BundleError::Corrupt("inventory content edge row".into()))?
            };
            retained.dependencies.insert(hash.clone(), dependencies);
            let inventory_edges = if *object == root.store_manifest_ref {
                manifest.recovery.references()
            } else {
                manifest
                    .inventory_dependencies
                    .get(&hash)
                    .cloned()
                    .unwrap_or_default()
            };
            if !inventory_edges.is_empty() {
                retained
                    .inventory_dependencies
                    .insert(hash.clone(), inventory_edges);
            }
            retained.objects.push(object.clone());
            verified.insert(hash);
        }
        retained
            .dependencies
            .insert(reference.sha256.clone(), root.references());
        retained.objects.push(reference.clone());
        verified.insert(reference.sha256);
    }
    Ok(())
}
pub(crate) fn supported_apis() -> BTreeMap<String, u32> {
    [("bundle", 1), ("core-history", 1), ("owned-inputs", 1)]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect()
}
pub(crate) fn decode<T: DeserializeOwned>(
    bytes: &[u8],
    usage: &Arc<Mutex<StorageUsage>>,
    kind: RecordKind,
    fields: &mut BTreeSet<String>,
) -> Result<StorageReservation<T>, BundleError> {
    #[cfg(any(test, feature = "test-util"))]
    if kind == RecordKind::History {
        crate::io_observer::note_history_decode();
    }
    let limits = StorageLimits::default();
    let scratch = StorageReservation::reserve(
        usage,
        &limits,
        0,
        limits.decoded_record_bytes,
        0,
        false,
        u64::MAX,
    )?;
    let charge = crate::store::preflight_record(bytes, &limits)?;
    drop(scratch);
    let shadow = StorageReservation::reserve(usage, &limits, 0, charge, 0, false, u64::MAX)?;
    let value = StorageReservation::reserve(
        usage,
        &limits,
        0,
        charge,
        if kind == RecordKind::History {
            charge
        } else {
            0
        },
        false,
        u64::MAX,
    )?;
    let raw: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    support_semantics(&raw)?;
    let observed = match kind {
        RecordKind::Document => kinewright_core::document_support_fields(&raw),
        RecordKind::History => kinewright_core::history_support_fields(&raw),
        RecordKind::SpecialistBase => {
            if let Some(operations) = raw
                .get("full_applied_delta")
                .and_then(serde_json::Value::as_array)
            {
                for operation in operations {
                    fields.extend(
                        kinewright_core::operation_support_fields(operation)
                            .map_err(BundleError::Unsupported)?,
                    );
                }
            }
            Ok(BTreeSet::new())
        }
        RecordKind::Lineage => {
            if let Some(ancestry) = raw
                .get("split_ancestry")
                .and_then(serde_json::Value::as_array)
            {
                for entry in ancestry {
                    let clip = entry
                        .get(2)
                        .ok_or_else(|| BundleError::Corrupt("split ancestry tuple".into()))?;
                    fields.extend(
                        kinewright_core::clip_support_fields(clip)
                            .map_err(BundleError::Unsupported)?,
                    );
                }
            }
            Ok(BTreeSet::new())
        }
        _ => Ok(BTreeSet::new()),
    }
    .map_err(BundleError::Unsupported)?;
    fields.extend(observed);
    // Raw support runs first; typed closed records then enforce shape and required fields.
    let typed = serde_json::from_value(raw).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    drop(shadow);
    Ok(value.hold(typed))
}
pub(crate) fn read_bytes(
    namespace: &ProjectNamespace,
    reference: &ObjectRef,
    usage: &Arc<Mutex<StorageUsage>>,
) -> Result<(Vec<u8>, StorageReservation), BundleError> {
    let limits = StorageLimits::default();
    let cap = if reference.record_kind == RecordKind::History {
        limits.branch_history_bytes
    } else {
        limits.control_bytes
    };
    if reference.byte_length > cap {
        return Err(BundleError::Capacity("encoded metadata record"));
    }
    let charge =
        StorageReservation::reserve(usage, &limits, 0, reference.byte_length, 0, false, u64::MAX)?;
    let mut file = ProjectStore::verify_in_namespace(namespace, reference, usage, &limits)?;
    let bytes = bounded_bytes(&mut file, reference.byte_length)?;
    if bytes.len() as u64 != reference.byte_length {
        return Err(BundleError::Corrupt("record changed while decoding".into()));
    }
    Ok((bytes, charge))
}
fn read_record<T: BundleRecord>(
    namespace: &ProjectNamespace,
    reference: &ObjectRef,
    usage: &Arc<Mutex<StorageUsage>>,
    fields: &mut BTreeSet<String>,
) -> Result<StorageReservation<T>, BundleError> {
    if reference.record_kind != T::KIND {
        return Err(BundleError::Corrupt("record reference kind".into()));
    }
    let (bytes, _encoded) = read_bytes(namespace, reference, usage)?;
    let record: StorageReservation<T> = decode(&bytes, usage, reference.record_kind, fields)?;
    record.validate(&StorageLimits::default())?;
    Ok(record)
}
fn compare_edges<T: BundleRecord>(
    reference: &ObjectRef,
    record: &T,
    manifest: &StoreManifest,
) -> Result<(), BundleError> {
    let mut expected = record.dependencies();
    expected.sort();
    expected.dedup();
    let mut actual = manifest
        .dependencies
        .get(&reference.sha256)
        .cloned()
        .ok_or_else(|| BundleError::Corrupt("omitted edge declaration".into()))?;
    actual.sort();
    actual.dedup();
    if actual != expected {
        return Err(BundleError::Corrupt(
            "raw record/dependency inventory mismatch".into(),
        ));
    }
    let mut inventory = record.inventory_dependencies();
    inventory.sort();
    inventory.dedup();
    let mut declared = manifest
        .inventory_dependencies
        .get(&reference.sha256)
        .cloned()
        .unwrap_or_default();
    declared.sort();
    declared.dedup();
    if inventory != declared {
        return Err(BundleError::Corrupt(
            "recovery edge inventory mismatch".into(),
        ));
    }
    Ok(())
}
#[allow(
    clippy::too_many_arguments,
    reason = "bounded graph traversal retains independent cycle and reachability sets"
)]
fn walk(
    reference: &ObjectRef,
    objects: &BTreeMap<String, ObjectRef>,
    manifest: &StoreManifest,
    visited: &mut BTreeSet<(String, bool)>,
    active: &mut BTreeSet<String>,
    found: &mut BTreeSet<String>,
    inventory: bool,
    depth: u64,
) -> Result<(), BundleError> {
    if depth > StorageLimits::default().graph_depth {
        return Err(BundleError::Capacity("graph depth"));
    }
    if objects.get(&reference.sha256) != Some(reference) {
        return Err(BundleError::Corrupt(
            "missing/conflicting object declaration".into(),
        ));
    }
    let inventory = inventory && reference.record_kind != RecordKind::RootCheckpoint;
    if active.contains(&reference.sha256) {
        return Err(BundleError::Corrupt("graph cycle".into()));
    }
    if !visited.insert((reference.sha256.clone(), inventory)) {
        return Ok(());
    }
    active.insert(reference.sha256.clone());
    let edges = if reference.record_kind == RecordKind::StoreManifest
        && !manifest.dependencies.contains_key(&reference.sha256)
    {
        manifest.durable_pins.clone()
    } else {
        manifest
            .dependencies
            .get(&reference.sha256)
            .cloned()
            .ok_or_else(|| BundleError::Corrupt("missing edge row".into()))?
    };
    for edge in edges {
        walk(
            &edge,
            objects,
            manifest,
            visited,
            active,
            found,
            inventory,
            depth + 1,
        )?;
    }
    if inventory {
        let edges = if reference.record_kind == RecordKind::StoreManifest
            && !manifest.dependencies.contains_key(&reference.sha256)
        {
            manifest.recovery.references()
        } else {
            manifest
                .inventory_dependencies
                .get(&reference.sha256)
                .cloned()
                .unwrap_or_default()
        };
        for edge in edges {
            walk(
                &edge,
                objects,
                manifest,
                visited,
                active,
                found,
                true,
                depth + 1,
            )?;
        }
    }
    active.remove(&reference.sha256);
    found.insert(reference.sha256.clone());
    Ok(())
}
#[allow(clippy::too_many_lines)]
pub(crate) fn read(path: &Path) -> Result<LoadedBundle, BundleError> {
    let namespace = ProjectNamespace::inspect(path)?;
    let bytes = namespace
        .read_root()?
        .ok_or_else(|| BundleError::Corrupt("missing root".into()))?;
    let admission =
        BundleWriteAdmission::from_bytes(Some(namespace.identity().resolved_root.clone()), bytes)?;
    admission.require_supported()?;
    let root = admission
        .data
        .file
        .as_ref()
        .and_then(|f| f.studio.as_ref())
        .ok_or_else(|| BundleError::Unsupported("not a Studio bundle".into()))?;
    let usage = &admission.data.usage;
    let mut fields = admission.data.inventory.required_fields.clone();
    let manifest: StorageReservation<StoreManifest> = read_record(
        &namespace,
        &root.store_manifest_ref,
        usage,
        &mut BTreeSet::new(),
    )?;
    if manifest.project_id.as_ref() != Some(&root.project_id)
        || manifest.session_id.is_some()
        || manifest.generation != root.generation
    {
        return Err(BundleError::Corrupt(
            "root/store identity or generation mismatch".into(),
        ));
    }
    if manifest.required_apis != supported_apis() {
        return Err(BundleError::Unsupported("required API inventory".into()));
    }
    let limits = StorageLimits::default();
    let mut objects = BTreeMap::new();
    let mut metadata = root.store_manifest_ref.byte_length;
    let mut owned = 0_u64;
    for reference in &manifest.objects {
        reference.validate()?;
        if objects
            .insert(reference.sha256.clone(), reference.clone())
            .is_some()
        {
            return Err(BundleError::Corrupt("duplicate object declaration".into()));
        }
        let count = if reference.record_kind.is_owned_bytes() {
            &mut owned
        } else {
            &mut metadata
        };
        *count = count
            .checked_add(reference.byte_length)
            .ok_or(BundleError::Capacity("graph bytes overflow"))?;
        metadata = metadata
            .checked_add(
                256 + 160
                    * manifest
                        .dependencies
                        .get(&reference.sha256)
                        .map_or(0, |d| d.len() as u64),
            )
            .ok_or(BundleError::Capacity("graph metadata overflow"))?;
        if owned > limits.owned_bytes || metadata > limits.metadata_bytes {
            return Err(BundleError::Capacity("graph quota"));
        }
    }
    if objects.len() as u64 >= limits.objects
        || objects
            .insert(
                root.store_manifest_ref.sha256.clone(),
                root.store_manifest_ref.clone(),
            )
            .is_some()
    {
        return Err(BundleError::Corrupt(
            "manifest self reference/object cap".into(),
        ));
    }
    if manifest.dependencies.len() != manifest.objects.len()
        || manifest
            .inventory_dependencies
            .keys()
            .any(|h| !objects.contains_key(h))
    {
        return Err(BundleError::Corrupt(
            "extraneous/missing inventory row".into(),
        ));
    }
    let kinds: BTreeSet<_> = objects.values().map(|r| r.record_kind).collect();
    if manifest.required_kinds != kinds {
        return Err(BundleError::Unsupported(
            "required kind inventory differs from retained graph".into(),
        ));
    }
    let mut found = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut active = BTreeSet::new();
    for reference in &root.references() {
        walk(
            reference,
            &objects,
            &manifest,
            &mut visited,
            &mut active,
            &mut found,
            true,
            0,
        )?;
    }
    if found.len() != objects.len() {
        return Err(BundleError::Corrupt(
            "unreachable manifest object declaration".into(),
        ));
    }
    let mut verified = BTreeSet::from([root.store_manifest_ref.sha256.clone()]);
    let mut checkpoints = BTreeMap::new();

    let mut indexes = BTreeMap::new();
    let mut input_manifests = BTreeMap::new();
    for reference in &manifest.objects {
        macro_rules! admit {
            ($ty:ty) => {{
                let value = read_record::<$ty>(&namespace, reference, usage, &mut fields)?;
                compare_edges(reference, &*value, &manifest)?;
                value
            }};
        }
        match reference.record_kind {
            RecordKind::Document => {
                let _value = admit!(Document);
            }
            RecordKind::History => {
                let _value = admit!(HistorySnapshot);
            }
            RecordKind::BranchCheckpoint => {
                checkpoints.insert(reference.sha256.clone(), admit!(BranchCheckpoint));
            }
            RecordKind::BranchIndex => {
                indexes.insert(
                    reference.sha256.clone(),
                    admit!(BTreeMap<BranchId, ObjectRef>),
                );
            }
            RecordKind::Lineage => {
                let _value = admit!(LineageRecord);
            }
            RecordKind::SpecialistBase => {
                let _value = admit!(crate::bundle_records::SpecialistBaseRecord);
            }
            RecordKind::HistoryFloor => {
                let _value = admit!(crate::bundle_records::HistoryFloorCheckpoint);
            }
            RecordKind::RecoveryClose => {
                let _value = admit!(crate::bundle_records::RecoveryCloseCheckpoint);
            }
            RecordKind::InputManifest => {
                input_manifests.insert(reference.sha256.clone(), admit!(OwnedInputManifest));
            }
            RecordKind::ReceiptIndex => {
                let _value = admit!(crate::receipts::ReceiptLedger);
            }
            RecordKind::IssuerState => {
                let _value =
                    admit!(BTreeMap<crate::receipts::IssuerId,crate::receipts::IssuerState>);
            }

            RecordKind::StoreManifest => {
                let value = admit!(StoreManifest);
                if value.required_apis != supported_apis() {
                    return Err(BundleError::Unsupported("historical API inventory".into()));
                }
            }
            RecordKind::RootCheckpoint => {
                let (bytes, _charge) = read_bytes(&namespace, reference, usage)?;
                let prior = BundleWriteAdmission::from_bytes(None, bytes)?;
                prior.require_supported()?;
                fields.extend(prior.data.inventory.required_fields.iter().cloned());
                let expected = prior
                    .data
                    .file
                    .as_ref()
                    .and_then(|f| f.studio.as_ref())
                    .map_or_else(Vec::new, super::bundle::StudioRoot::references);
                let actual = manifest
                    .dependencies
                    .get(&reference.sha256)
                    .ok_or_else(|| BundleError::Corrupt("root checkpoint edges".into()))?;
                let mut expected = expected;
                expected.sort();
                expected.dedup();
                let mut actual = actual.clone();
                actual.sort();
                actual.dedup();
                if expected != actual {
                    return Err(BundleError::Corrupt("root checkpoint content edges".into()));
                }
            }
            kind if kind.is_owned_bytes() => {
                ProjectStore::verify_in_namespace(&namespace, reference, usage, &limits)?;
            }
            _ => return Err(BundleError::Unsupported("inactive record producer".into())),
        }
        verified.insert(reference.sha256.clone());
    }
    if fields != manifest.required_fields {
        return Err(BundleError::Unsupported(
            "raw field inventory differs from declaration".into(),
        ));
    }
    let index = indexes
        .get(&root.branch_index_ref.sha256)
        .ok_or_else(|| BundleError::Corrupt("missing branch index".into()))?;
    // Verify every archived checkpoint, releasing its decode before moving to the next.
    // Only current branch histories remain resident, with shared hashes charged once.
    let mut histories: BTreeMap<String, Arc<StorageReservation<HistorySnapshot>>> = BTreeMap::new();
    let mut lineages: BTreeMap<String, Arc<StorageReservation<LineageRecord>>> = BTreeMap::new();
    let mut inspect_checkpoint = |id: &BranchId,
                                  checkpoint: StorageReservation<BranchCheckpoint>,
                                  active: bool|
     -> Result<crate::bundle::LoadedBranch, BundleError> {
        let history = if active && let Some(value) = histories.get(&checkpoint.history_ref.sha256) {
            Arc::clone(value)
        } else {
            Arc::new(read_record::<HistorySnapshot>(
                &namespace,
                &checkpoint.history_ref,
                usage,
                &mut fields,
            )?)
        };
        let lineage = if active && let Some(value) = lineages.get(&checkpoint.lineage_ref.sha256) {
            Arc::clone(value)
        } else {
            Arc::new(read_record::<LineageRecord>(
                &namespace,
                &checkpoint.lineage_ref,
                usage,
                &mut fields,
            )?)
        };
        let document =
            read_record::<Document>(&namespace, &checkpoint.document_ref, usage, &mut fields)?;
        let baseline =
            read_record::<Document>(&namespace, &checkpoint.baseline_ref, usage, &mut fields)?;
        if checkpoint.branch_id != *id
            || checkpoint.document_hash != checkpoint.document_ref.sha256
            || *document != *history.document
            || *baseline != *history.baseline
            || checkpoint.batch_ids.len() != history.batches.len()
            || checkpoint.history_input_manifest_refs.len() != history.batches.len() + 1
            || checkpoint.applied_operations_digest
                != kinewright_media::sha256_bytes(
                    &serde_json::to_vec(&history.applied_operations())
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                )
        {
            return Err(BundleError::Corrupt(
                "branch history/checkpoint mismatch".into(),
            ));
        }
        let mut retained_inputs = OwnedInputManifest {
            project_id: Some(root.project_id.clone()),
            session_id: None,
            owner_epoch: 0,
            entries: vec![],
        };
        for reference in std::iter::once(&checkpoint.input_manifest_ref)
            .chain(&checkpoint.history_input_manifest_refs)
        {
            let input = input_manifests.get(&reference.sha256).ok_or_else(|| {
                BundleError::Corrupt("active branch input manifest missing".into())
            })?;
            if (input.project_id.as_ref() != Some(&root.project_id)
                && (active
                    || !input
                        .project_id
                        .as_ref()
                        .is_some_and(|p| root.origin_project_ids.contains(p))))
                || input.session_id.is_some()
            {
                return Err(BundleError::Corrupt("active branch input owner".into()));
            }
            retained_inputs.entries.extend(input.entries.clone());
        }
        crate::bundle_commit::validate_document_inputs(
            &history.document,
            &input_manifests[&checkpoint.input_manifest_ref.sha256],
        )?;
        crate::bundle_commit::validate_inputs(&history, &retained_inputs)?;
        // Private candidate replay; no live Core is changed on a damaged branch.
        let core = kinewright_core::Core::spawn((*history.baseline).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        core.restore_history((**history).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if lineage.current.keys().copied().collect::<BTreeSet<_>>()
            != history
                .document
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(|c| c.id))
                .collect()
        {
            return Err(BundleError::Corrupt(
                "lineage head differs from history".into(),
            ));
        }

        if active {
            histories.insert(checkpoint.history_ref.sha256.clone(), Arc::clone(&history));
            lineages.insert(checkpoint.lineage_ref.sha256.clone(), Arc::clone(&lineage));
        }
        Ok((checkpoint, history, lineage))
    };
    let mut branches = BTreeMap::new();
    for (id, reference) in index.iter() {
        let checkpoint = checkpoints
            .remove(&reference.sha256)
            .ok_or_else(|| BundleError::Corrupt("missing branch checkpoint".into()))?;
        branches.insert(id.clone(), inspect_checkpoint(id, checkpoint, true)?);
    }
    for checkpoint in checkpoints.into_values() {
        let id = checkpoint.branch_id.clone();
        drop(inspect_checkpoint(&id, checkpoint, false)?);
    }
    let candidates: Vec<_> = branches
        .keys()
        .filter(|id| **id != root.manual_branch_id)
        .cloned()
        .collect();
    if root.candidate_branch_ids != candidates {
        return Err(BundleError::Corrupt(
            "candidate branch inventory mismatch".into(),
        ));
    }
    let manual = branches
        .get(&root.manual_branch_id)
        .ok_or_else(|| BundleError::Corrupt("manual branch omitted".into()))?;
    if admission.inspection_document() != Some(&manual.1.document)
        || manual.0.input_manifest_ref != root.input_manifest_ref
    {
        return Err(BundleError::Corrupt(
            "flattened manual checkpoint mismatch".into(),
        ));
    }
    let inventory = SupportInventory {
        required_fields: fields,
        required_kinds: kinds,
        required_apis: supported_apis(),
    };
    let receipts = root
        .receipt_index_ref
        .as_ref()
        .map(|r| {
            read_record::<crate::receipts::ReceiptLedger>(
                &namespace,
                r,
                usage,
                &mut BTreeSet::new(),
            )
        })
        .transpose()?;
    let issuers = root
        .issuer_state_ref
        .as_ref()
        .map(|r| {
            read_record::<BTreeMap<crate::receipts::IssuerId, crate::receipts::IssuerState>>(
                &namespace,
                r,
                usage,
                &mut BTreeSet::new(),
            )
        })
        .transpose()?;
    if let Some(ledger) = &receipts {
        for receipt in ledger.detailed.iter().chain(&ledger.retired) {
            if (receipt.project_id != root.project_id
                && !root.origin_project_ids.contains(&receipt.project_id))
                || receipt.generation > root.generation
                || !issuers
                    .as_ref()
                    .is_some_and(|i| i.contains_key(&receipt.id.issuer))
            {
                return Err(BundleError::Corrupt("receipt root/issuer binding".into()));
            }
        }
    }
    Ok(LoadedBundle {
        admission,
        namespace,
        manifest,
        branches,
        verified,
        inventory,
        receipts,
        issuers,
    })
}

pub(crate) fn bounded_bytes(mut reader: impl Read, limit: u64) -> Result<Vec<u8>, BundleError> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0_u8; 65_536];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if (bytes.len() as u64)
            .checked_add(count as u64)
            .is_none_or(|n| n > limit)
        {
            return Err(BundleError::Capacity("bounded read grew"));
        }
        bytes
            .try_reserve_exact(count)
            .map_err(|_| BundleError::Capacity("bounded read allocation"))?;
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

pub(crate) fn support_semantics(raw: &serde_json::Value) -> Result<(), BundleError> {
    match raw {
        serde_json::Value::Object(object) => {
            if let Some(pipeline) = object.get("pipeline_state") {
                if !matches!(pipeline.as_str(), Some("legacy" | "managed_sdr_v1")) {
                    return Err(BundleError::Unsupported(
                        "colour pipeline semantics are unavailable".into(),
                    ));
                }
                for name in ["working", "monitoring", "delivery"] {
                    if matches!(
                        object
                            .get(name)
                            .and_then(|d| d.get("transfer"))
                            .and_then(serde_json::Value::as_str),
                        Some("smpte2084" | "arib_std_b67")
                    ) {
                        return Err(BundleError::Unsupported(
                            "HDR rendering pipeline is unavailable".into(),
                        ));
                    }
                }
            }
            for value in object.values() {
                support_semantics(value)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                support_semantics(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Read-only outcome. A quarantined root never authorizes resumption or rewrites.
pub enum RecoveryAssessment {
    Verified(Box<LoadedBundle>),
    Quarantined {
        admission: BundleWriteAdmission,
        reason: String,
        branches: Vec<QuarantinedBranch>,
        verified_document_bytes: Vec<ObjectRef>,
    },
}
#[derive(Debug, Clone)]
pub struct QuarantinedBranch {
    pub branch_id: BranchId,
    pub history_ref: ObjectRef,
    pub reason: String,
}
/// Inspect damaged history independently of intact immutable document bytes.
/// # Errors
/// Refuses unreadable or unsupported envelopes and bounded inspection failures.
pub fn assess(path: &Path) -> Result<RecoveryAssessment, BundleError> {
    let error = match read(path) {
        Ok(loaded) => return Ok(RecoveryAssessment::Verified(Box::new(loaded))),
        Err(error) => error,
    };
    let namespace = ProjectNamespace::inspect(path)?;
    let bytes = namespace
        .read_root()?
        .ok_or_else(|| BundleError::Corrupt("missing root".into()))?;
    let admission =
        BundleWriteAdmission::from_bytes(Some(namespace.identity().resolved_root.clone()), bytes)?;
    admission.require_supported()?;
    let root = admission
        .data
        .file
        .as_ref()
        .and_then(|f| f.studio.as_ref())
        .ok_or_else(|| BundleError::Unsupported("not a Studio bundle".into()))?;
    let usage = &admission.data.usage;
    let index = read_record::<BTreeMap<BranchId, ObjectRef>>(
        &namespace,
        &root.branch_index_ref,
        usage,
        &mut BTreeSet::new(),
    )?;
    let mut branches = Vec::new();
    let mut verified_document_bytes = Vec::new();
    for (id, reference) in index.iter() {
        let checkpoint =
            read_record::<BranchCheckpoint>(&namespace, reference, usage, &mut BTreeSet::new())?;
        if checkpoint.branch_id != *id {
            return Err(BundleError::Corrupt("inspection branch identity".into()));
        }
        if read_record::<Document>(
            &namespace,
            &checkpoint.document_ref,
            usage,
            &mut BTreeSet::new(),
        )
        .is_ok()
        {
            verified_document_bytes.push(checkpoint.document_ref.clone());
        }
        let history = read_record::<HistorySnapshot>(
            &namespace,
            &checkpoint.history_ref,
            usage,
            &mut BTreeSet::new(),
        );
        let history_error = match history {
            Err(error) => Some(error.to_string()),
            Ok(history) => {
                let core = kinewright_core::Core::spawn((*history.baseline).clone())
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                core.restore_history((*history).clone())
                    .err()
                    .map(|e| e.to_string())
            }
        };
        if let Some(reason) = history_error {
            branches.push(QuarantinedBranch {
                branch_id: id.clone(),
                history_ref: checkpoint.history_ref.clone(),
                reason,
            });
        }
    }
    Ok(RecoveryAssessment::Quarantined {
        admission,
        reason: error.to_string(),
        branches,
        verified_document_bytes,
    })
}
