//! Synchronous B0–B5 publication through the one store/namespace owner.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{
        BundleError, BundleSaveReport, BundleWriteAdmission, ProjectBundle, StudioRoot,
        SupportInventory,
    },
    bundle_records::{
        BatchId, BranchCheckpoint, HeadId, LineageRecord, ObjectRef, OccurrenceId, RecordKind,
    },
    owned_inputs::OwnedInputManifest,
    receipts::CommandId,
    store::SealedObject,
};
use kinewright_core::{ClipId, Core, HistorySnapshot, JournalCommand};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitBarrier {
    B0,
    B1,
    B2,
    B3,
    B4,
    B5,
}
#[derive(Debug)]
pub enum CommitOutcome {
    Committed(BundleSaveReport),
    DefinitelyUnpublished(BundleError),
    DurabilityUnknown,
}
pub struct BundleCommit;
impl BundleCommit {
    /// C4 publishes graph/checkpoint state only. Receipt-bearing admission lands with its ledger.
    pub fn command(
        _bundle: &mut ProjectBundle,
        _id: &CommandId,
    ) -> Result<CommitOutcome, BundleError> {
        Err(BundleError::Unsupported(
            "durable command receipts are not implemented".into(),
        ))
    }
    pub fn run(
        bundle: &mut ProjectBundle,
        history: &HistorySnapshot,
        inputs: &ObjectRef,
    ) -> CommitOutcome {
        match Self::publish(bundle, history, inputs) {
            Ok(report) => CommitOutcome::Committed(report),
            Err(BundleError::DurabilityUnknown | BundleError::Unsettled) => {
                CommitOutcome::DurabilityUnknown
            }
            Err(error) => CommitOutcome::DefinitelyUnpublished(error),
        }
    }
    #[allow(clippy::too_many_lines)]
    pub(crate) fn publish(
        bundle: &mut ProjectBundle,
        history: &HistorySnapshot,
        inputs: &ObjectRef,
    ) -> Result<BundleSaveReport, BundleError> {
        if bundle.unsettled {
            return Err(BundleError::Unsettled);
        }
        bundle.store.with_namespace(|n| n.verify())?;
        if bundle.store.with_namespace(|n| n.read_root())?.as_deref()
            != bundle
                .admission
                .source()
                .map(|_| bundle.admission.raw_bytes())
        {
            return Err(BundleError::Identity(
                "root changed since bundle admission".into(),
            ));
        }
        history.encoded_bytes().map_err(|e| {
            BundleError::Capacity(match e {
                kinewright_core::HistoryError::Capacity => "history",
                _ => "history actor",
            })
        })?;
        let serialized =
            serde_json::to_vec(history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let mut support = BTreeSet::new();
        let checked = crate::bundle_recovery::decode::<HistorySnapshot>(
            &serialized,
            &bundle.store.usage_arc(),
            RecordKind::History,
            &mut support,
        )?;
        let _replay = bundle
            .store
            .reserve_history_shadow(crate::store::preflight_record(
                &serialized,
                bundle.store.limits(),
            )?)?;
        let core = Core::spawn((*checked.baseline).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        core.restore_history((*checked).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let manifest = bundle.store.read_input_manifest(inputs)?;
        validate_inputs(&checked, &manifest)?;
        let prior = bundle.branches.get(&bundle.manual_branch_id).cloned();
        let old_history = prior
            .as_ref()
            .map(|p| bundle.store.read_record::<HistorySnapshot>(&p.history_ref))
            .transpose()?;
        if old_history.as_ref().is_some_and(|old| {
            checked.batches.len() < old.batches.len()
                || checked.batches[..old.batches.len()] != old.batches
        }) {
            return Err(BundleError::Corrupt(
                "history is not a continuation of the admitted head".into(),
            ));
        }
        let mut batch_ids = prior
            .as_ref()
            .map_or_else(Vec::new, |p| p.batch_ids.clone());
        while batch_ids.len() < checked.batches.len() {
            batch_ids.push(BatchId::fresh()?);
        }
        let old_lineage = prior
            .as_ref()
            .map(|p| bundle.store.read_record::<LineageRecord>(&p.lineage_ref))
            .transpose()?;
        let lineage = lineage(
            &bundle.manual_branch_id,
            &checked,
            &batch_ids,
            old_lineage.as_deref(),
        )?;
        let generation = bundle
            .generation
            .checked_add(1)
            .ok_or(BundleError::Capacity("root generation"))?;
        let unchanged = old_history.as_ref().is_some_and(|h| **h == *checked)
            && prior
                .as_ref()
                .is_some_and(|p| p.input_manifest_ref == *inputs);
        let head_id = if unchanged {
            prior.as_ref().unwrap().head_id.clone()
        } else {
            HeadId::fresh()?
        };
        barrier(bundle, CommitBarrier::B0)?;
        let mut seals: Vec<SealedObject> = vec![];
        macro_rules! seal {
            ($value:expr) => {{
                let sealed = bundle.store.admit_record($value)?;
                let reference = sealed.reference.clone();
                seals.push(sealed);
                reference
            }};
        }
        let document_ref = seal!(&*checked.document);
        let baseline_ref = seal!(&*checked.baseline);
        let history_ref = seal!(&*checked);
        let lineage_ref = seal!(&lineage);
        let mut contexts = prior
            .as_ref()
            .map_or_else(Vec::new, |p| p.history_input_manifest_refs.clone());
        if contexts.is_empty() {
            contexts.push(inputs.clone());
        }
        while contexts.len() < checked.batches.len() + 1 {
            contexts.push(inputs.clone());
        }
        let checkpoint = BranchCheckpoint {
            branch_id: bundle.manual_branch_id.clone(),
            head_id: head_id.clone(),
            document_hash: document_ref.sha256.clone(),
            document_ref,
            baseline_ref,
            history_ref,
            lineage_ref,
            input_manifest_ref: inputs.clone(),
            history_input_manifest_refs: contexts,
            batch_ids,
            applied_operations_digest: kinewright_media::sha256_bytes(
                &serde_json::to_vec(&checked.applied_operations())
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?,
            ),
            checkpoint_generation: generation,
            floor_generation: prior.as_ref().map_or(0, |p| p.floor_generation),
            command_cut: None,
            predecessor: prior.as_ref().map(|p| p.head_id.clone()),
            take_map_revision_ref: None,
        };
        let checkpoint_ref = seal!(&checkpoint);
        let mut branches = bundle.branches.clone();
        branches.insert(bundle.manual_branch_id.clone(), checkpoint);
        let mut branch_index: BTreeMap<_, _> = if let Some(root) = bundle.root.as_ref() {
            (*bundle
                .store
                .read_record::<BTreeMap<crate::bundle_records::BranchId, ObjectRef>>(
                    &root.branch_index_ref,
                )?)
            .clone()
        } else {
            BTreeMap::new()
        };
        branch_index.insert(bundle.manual_branch_id.clone(), checkpoint_ref);
        let branch_index_ref = seal!(&branch_index);
        barrier(bundle, CommitBarrier::B1)?;
        barrier(bundle, CommitBarrier::B2)?;
        let mut recovery = bundle.recovery.clone();
        if !bundle.admission.raw_bytes().is_empty() {
            let deps = if bundle.root.is_some() {
                bundle.root_references()?
            } else {
                vec![]
            };
            let previous = bundle
                .admit_root_checkpoint(bundle.admission.raw_bytes().to_vec().as_slice(), &deps)?;
            let reference = previous.reference.clone();
            seals.push(previous);
            let mut staged = recovery.clone();
            staged.staging = Some(reference.clone());
            bundle.store.publish_inventory(&staged)?;
            recovery.predecessors.insert(0, reference);
            recovery.predecessors.truncate(2);
        }
        recovery.generation = generation;
        recovery.staging = None;
        barrier(bundle, CommitBarrier::B3)?;
        let roots = vec![branch_index_ref.clone(), inputs.clone()];
        let mut inventory = SupportInventory {
            required_fields: BTreeSet::new(),
            required_kinds: BTreeSet::new(),
            required_apis: crate::bundle_recovery::supported_apis(),
        };
        let mut retained = roots.clone();
        retained.extend(bundle.evidence.clone());
        retained.extend(recovery.references());
        inventory.required_fields = bundle.store.support_fields(&retained)?;
        let store_manifest = bundle.store.selected_manifest(
            &roots,
            &bundle.evidence,
            recovery.clone(),
            &inventory,
            generation,
        )?;
        let store_manifest_ref = seal!(&store_manifest);
        let root = StudioRoot {
            project_id: bundle.project_id.clone(),
            generation,
            creation_brief_ref: None,
            manual_branch_id: bundle.manual_branch_id.clone(),
            manual_switch_generation: 0,
            candidate_branch_ids: vec![],
            branch_index_ref,
            version_index_ref: None,
            feedback_index_ref: None,
            run_index_ref: None,
            claim_index_ref: None,
            input_manifest_ref: inputs.clone(),
            receipt_index_ref: None,
            issuer_state_ref: None,
            store_manifest_ref,
        };
        root.admit()?;
        let (bytes, _encoded_root) = serialize_root(&bundle.store, &checked.document, &root)?;
        let next_admission = BundleWriteAdmission::from_bytes_using(
            Some(bundle.store.resolved_root()?),
            bytes,
            bundle.store.usage_arc(),
        )?;
        next_admission.require_supported()?;
        bundle.store.closure(&[
            root.branch_index_ref.clone(),
            root.input_manifest_ref.clone(),
            root.store_manifest_ref.clone(),
        ])?;
        if let Err(error) = bundle.store.publish_root(next_admission.raw_bytes()) {
            if bundle.store.with_namespace(|n| Ok(n.is_uncertain()))? {
                bundle.unsettled = true;
                return Err(BundleError::DurabilityUnknown);
            }
            return Err(error);
        }
        bundle.root = Some(root);
        bundle.branches = branches;
        bundle.generation = generation;
        bundle.inventory = inventory;
        bundle.recovery = recovery;
        bundle.admission = next_admission;
        bundle.durable = Some(bundle.store.mark_committed(&bundle.root_references()?)?);
        // A root has already landed. An unsettled subordinate inventory cannot be called a rollback.
        if bundle.store.publish_inventory(&bundle.recovery).is_err() {
            bundle.unsettled = true;
            return Err(BundleError::DurabilityUnknown);
        }
        barrier(bundle, CommitBarrier::B5)?;
        bundle.store.allow_collection()?;
        bundle.pending_evidence.clear();
        drop(seals);
        bundle.store.collect_unpublished()?;
        Ok(BundleSaveReport {
            path: bundle.store.resolved_root()?,
            project_id: bundle.project_id.clone(),
            generation,
            head_id,
            root_sha256: kinewright_media::sha256_bytes(bundle.admission.raw_bytes()),
            byte_length: bundle.admission.raw_bytes().len() as u64,
        })
    }
}
fn barrier(bundle: &ProjectBundle, barrier: CommitBarrier) -> Result<(), BundleError> {
    let path = bundle.store.resolved_root()?;
    let label = match barrier {
        CommitBarrier::B0 => "bundle_B0",
        CommitBarrier::B1 => "bundle_B1",
        CommitBarrier::B2 => "bundle_B2",
        CommitBarrier::B3 => "bundle_B3",
        CommitBarrier::B4 => "bundle_B4",
        CommitBarrier::B5 => "bundle_B5",
    };
    crate::namespace::note(label, &path);
    #[cfg(any(test, feature = "test-util"))]
    crate::test_hook(label);
    Ok(())
}
fn validate_inputs(
    history: &HistorySnapshot,
    manifest: &OwnedInputManifest,
) -> Result<(), BundleError> {
    // All retained source identities, including disabled and undone assets, require owned bytes.
    let documents = std::iter::once(&history.baseline)
        .chain(std::iter::once(&history.document))
        .chain(
            history
                .cursor
                .undo
                .iter()
                .chain(&history.cursor.redo)
                .map(|e| &e.document),
        );
    for document in documents {
        for asset in &document.media_pool {
            let id = format!("media:{}", asset.id.0);
            if !manifest
                .entries
                .iter()
                .any(|e| e.entry_id == id && e.object.record_kind == RecordKind::OwnedMedia)
            {
                return Err(BundleError::Unsupported(format!(
                    "retained asset {id} lacks owned input"
                )));
            }
        }
        if document
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .any(|c| matches!(c.content, kinewright_core::ClipContent::Title(_)))
        {
            let digest = kinewright_media::sha256_bytes(kinewright_core::title_font_bytes());
            if !manifest.entries.iter().any(|e| {
                e.entry_id == "font:Inter"
                    && e.object.record_kind == RecordKind::OwnedFont
                    && e.object.sha256 == digest
            }) {
                return Err(BundleError::Unsupported(
                    "retained title requires its owned embedded font".into(),
                ));
            }
        }
        for asset in &document.lut_assets {
            if !manifest.entries.iter().any(|e| {
                e.entry_id == format!("lut:{}", asset.id.0)
                    && e.object.record_kind == RecordKind::OwnedLut
            }) {
                return Err(BundleError::Unsupported(
                    "retained LUT lacks owned bytes".into(),
                ));
            }
        }
    }
    Ok(())
}
#[allow(
    clippy::too_many_lines,
    reason = "one ordered history replay of occurrence maps"
)]
fn lineage(
    branch: &crate::bundle_records::BranchId,
    history: &HistorySnapshot,
    batches: &[BatchId],
    old: Option<&LineageRecord>,
) -> Result<LineageRecord, BundleError> {
    let seed = BatchId::fresh()?;
    let baseline: BTreeMap<ClipId, OccurrenceId> = if let Some(old) = old {
        old.baseline.clone()
    } else {
        history
            .baseline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .enumerate()
            .map(|(i, c)| {
                (
                    c.id,
                    OccurrenceId {
                        branch_origin: branch.clone(),
                        creation_batch_id: seed.clone(),
                        result_ordinal: i as u64,
                    },
                )
            })
            .collect()
    };
    let mut current = baseline.clone();
    let mut undo = vec![];
    let mut redo = vec![];
    let mut split_ancestry = vec![];
    for (batch, id) in history.batches.iter().zip(batches) {
        match &batch.command {
            JournalCommand::Undo => {
                let previous = undo
                    .pop()
                    .ok_or_else(|| BundleError::Corrupt("lineage undo underflow".into()))?;
                redo.push(current);
                current = previous;
            }
            JournalCommand::Redo => {
                let next = redo
                    .pop()
                    .ok_or_else(|| BundleError::Corrupt("lineage redo underflow".into()))?;
                undo.push(current);
                current = next;
            }
            _ => {
                let before = current.clone();
                let mut ordinal = 0_u64;
                for effect in &batch.identity_effects {
                    let parent = effect
                        .split_parent
                        .as_ref()
                        .and_then(|c| current.get(&c.id))
                        .cloned();
                    for deleted in &effect.deleted {
                        current.remove(deleted);
                    }
                    let children: BTreeSet<_> = effect.split_results.iter().map(|c| c.id).collect();
                    for created in effect.created.iter().filter(|c| !children.contains(c)) {
                        current.insert(
                            *created,
                            OccurrenceId {
                                branch_origin: branch.clone(),
                                creation_batch_id: id.clone(),
                                result_ordinal: ordinal,
                            },
                        );
                        ordinal += 1;
                    }
                    for child in &effect.split_results {
                        let occurrence = OccurrenceId {
                            branch_origin: branch.clone(),
                            creation_batch_id: id.clone(),
                            result_ordinal: ordinal,
                        };
                        ordinal += 1;
                        split_ancestry.push((
                            occurrence.clone(),
                            parent.clone().ok_or_else(|| {
                                BundleError::Corrupt("unknown split parent occurrence".into())
                            })?,
                            child.clone(),
                        ));
                        current.insert(child.id, occurrence);
                    }
                }
                if batch.undo_entries == undo.len() + 1 {
                    undo.push(before);
                } else if batch.undo_entries != undo.len() {
                    return Err(BundleError::Corrupt("lineage gesture boundary".into()));
                }
                redo.clear();
            }
        }
        if undo.len() != batch.undo_entries || redo.len() != batch.redo_entries {
            return Err(BundleError::Corrupt("lineage cursor counts".into()));
        }
    }
    Ok(LineageRecord {
        branch_origin: branch.clone(),
        baseline,
        current,
        undo,
        redo,
        split_ancestry,
    })
}

pub(crate) fn serialize_root(
    store: &crate::store::ProjectStore,
    document: &kinewright_core::Document,
    root: &StudioRoot,
) -> Result<(Vec<u8>, crate::storage_budget::StorageReservation), BundleError> {
    #[derive(serde::Serialize)]
    struct BorrowedRoot<'a> {
        format_version: u32,
        studio: &'a StudioRoot,
        #[serde(flatten)]
        document: &'a kinewright_core::Document,
    }
    let file = BorrowedRoot {
        format_version: crate::project::required_format_version(document, Some(root)),
        studio: root,
        document,
    };
    let mut counter = crate::store::BoundedCounter {
        bytes: 0,
        limit: store.limits().control_bytes,
    };
    serde_json::to_writer_pretty(&mut counter, &file)
        .map_err(|_| BundleError::Capacity("root encoded bytes"))?;
    let encoded = store.reserve(0, counter.bytes, 0, false)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(
            usize::try_from(counter.bytes)
                .map_err(|_| BundleError::Capacity("root address space"))?,
        )
        .map_err(|_| BundleError::Capacity("root allocation"))?;
    serde_json::to_writer_pretty(&mut bytes, &file)
        .map_err(|e| BundleError::Corrupt(e.to_string()))?;
    if bytes.len() as u64 != counter.bytes {
        return Err(BundleError::Corrupt("unstable root serialization".into()));
    }
    Ok((bytes, encoded))
}
