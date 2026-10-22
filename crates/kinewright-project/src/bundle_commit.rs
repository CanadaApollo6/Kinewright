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
    receipts::{CommandEnvelope, CommandReceipt, ReceiptLookup, ReceiptResult},
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
    /// Stage through Core, then publish the result and issuer sequence with the same root.
    #[allow(clippy::too_many_lines)]
    pub fn command(
        bundle: &mut ProjectBundle,
        envelope: &CommandEnvelope,
        core: &Core,
        inputs: &ObjectRef,
        blobs: &[(&ObjectRef, &[u8])],
    ) -> Result<ReceiptLookup, BundleError> {
        use kinewright_core::{Command, Event, Query, QueryResult};
        let lookup = bundle.receipt_lookup(envelope)?;
        if lookup != ReceiptLookup::Absent {
            return Ok(lookup);
        }
        if bundle.unsettled || bundle.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        if envelope.project_id != bundle.project_id
            || envelope.target_branch != bundle.manual_branch_id
            || envelope.owner_epoch != bundle.store.owner_epoch()
            || envelope.lease.is_some()
            || envelope.expected_head.as_ref() != bundle.manual_head()
        {
            return Err(BundleError::Unsupported(
                "stale command authority/head".into(),
            ));
        }
        let Event::QueryResult(QueryResult::Snapshot { revision, .. }) = core
            .request(Command::Query(Query::Snapshot))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
        else {
            return Err(BundleError::Corrupt("Core snapshot response".into()));
        };
        if revision != envelope.expected_revision {
            return Err(BundleError::Unsupported("stale runtime binding".into()));
        }
        let issuer = bundle
            .issuers
            .get(&envelope.id.issuer)
            .cloned()
            .ok_or_else(|| BundleError::Unsupported("unknown issuer".into()))?;
        if issuer.retired || issuer.next_sequence != envelope.id.sequence {
            return Err(BundleError::Unsupported(
                "issuer sequence/retirement".into(),
            ));
        }
        let next_sequence = envelope
            .id
            .sequence
            .checked_add(1)
            .ok_or(BundleError::Capacity("issuer sequence overflow"))?;
        if bundle.store.limits().pending_commands == 0
            || (bundle.receipts.detailed.len() as u64)
                .checked_add(bundle.receipts.retired.len() as u64)
                .is_none_or(|n| n >= bundle.store.limits().receipts)
        {
            return Err(BundleError::Capacity("pending command/receipt entries"));
        }
        let encoded = envelope.encoded_bytes()?;
        if encoded > bundle.store.limits().command_bytes {
            return Err(BundleError::Capacity("command bytes"));
        }
        let reservation = bundle.store.reserve(0, encoded, 0, false)?;
        let mut before = core
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let original = before.clone();
        if let Some(checkpoint) = bundle.branches.get(&bundle.manual_branch_id) {
            let acknowledged =
                if let Some(cached) = bundle.history_cache.get(&bundle.manual_branch_id) {
                    std::sync::Arc::clone(cached)
                } else {
                    std::sync::Arc::new(
                        bundle
                            .store
                            .read_record::<HistorySnapshot>(&checkpoint.history_ref)?,
                    )
                };
            if before.floor_offset < acknowledged.floor_offset {
                let cut = usize::try_from(acknowledged.floor_offset - before.floor_offset)
                    .map_err(|_| BundleError::Capacity("history floor offset"))?;
                before = kinewright_core::HistoryTransition::floor(before, cut)
                    .map_err(|_| {
                        BundleError::Unsupported(
                            "Core floor differs from acknowledged checkpoint".into(),
                        )
                    })?
                    .after()
                    .clone();
            }
            if **acknowledged != before {
                return Err(BundleError::Unsupported(
                    "Core is not the acknowledged checkpoint".into(),
                ));
            }
        }
        let _shadow = bundle.store.reserve_history_shadow(
            before
                .encoded_bytes()
                .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        )?;
        let _transition_overlap = if envelope.transition.is_some() {
            Some((
                bundle.store.reserve_history_shadow(
                    before
                        .encoded_bytes()
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                )?,
                bundle.store.reserve_history_shadow(
                    before
                        .encoded_bytes()
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                )?,
            ))
        } else {
            None
        };
        let after = if envelope.command.is_none() && envelope.transition.is_none() {
            before.clone()
        } else {
            let candidate = Core::spawn((*before.baseline).clone())
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            candidate
                .restore_history(before.clone())
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if let Some(transition) = &envelope.transition {
                let checkpoint =
                    bundle
                        .branches
                        .get(&bundle.manual_branch_id)
                        .ok_or_else(|| {
                            BundleError::Corrupt(
                                "transition without acknowledged checkpoint".into(),
                            )
                        })?;
                let install =
                    crate::bundle_branch::transition(transition, checkpoint, before.clone())?;
                candidate
                    .install_history_transition(install)
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            } else if let Some(command) = &envelope.command {
                let command = if let JournalCommand::DoBatch(operations) = command {
                    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) = candidate
                        .request(Command::Query(Query::Snapshot))
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?
                    else {
                        return Err(BundleError::Corrupt("candidate revision".into()));
                    };
                    Command::DoBatchIfRevision {
                        expected: revision,
                        operations: operations.clone(),
                    }
                } else {
                    Command::from(command.clone())
                };
                if !matches!(
                    candidate
                        .request(command)
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                    Event::DocumentChanged { .. }
                ) {
                    return Err(BundleError::Unsupported("Core rejected the command".into()));
                }
            }
            let after = candidate
                .export_history()
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            drop(candidate);
            after
        };
        if envelope.action == "comment" {
            match crate::receipts::stage_comment(bundle, envelope, blobs) {
                Ok(result) => bundle.pending_result = Some(result),
                Err(error) => {
                    return Err(error);
                }
            }
        }
        bundle.pending_command = Some(reservation.hold(envelope.clone()));
        bundle
            .issuers
            .get_mut(&envelope.id.issuer)
            .ok_or_else(|| BundleError::Corrupt("admitted issuer disappeared".into()))?
            .next_sequence = next_sequence;
        if let Err(error) = Self::publish(bundle, &after, inputs) {
            if !matches!(
                error,
                BundleError::DurabilityUnknown | BundleError::Unsettled
            ) {
                bundle.issuers.insert(envelope.id.issuer.clone(), issuer);
            }
            bundle.unsettled = true;
            return Err(error);
        }
        bundle.pending_command = None;
        bundle.pending_result = None;
        let published = std::sync::Arc::clone(
            bundle
                .history_cache
                .get(&bundle.manual_branch_id)
                .ok_or_else(|| {
                    BundleError::Corrupt("published history missing from owner".into())
                })?,
        );
        if **published != original
            && core
                .install_checkpoint(original, (**published).clone())
                .is_err()
        {
            bundle.unsettled = true;
            return Err(BundleError::Unsettled);
        }
        bundle.receipt_lookup(envelope)
    }
    pub fn run(bundle: &mut ProjectBundle, core: &Core, inputs: &ObjectRef) -> CommitOutcome {
        if bundle.pending_command.is_some() {
            return CommitOutcome::DefinitelyUnpublished(BundleError::Unsettled);
        }
        match bundle.save(core, inputs) {
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
        let branch_id = bundle.manual_branch_id.clone();
        Self::publish_branch(bundle, history, inputs, &branch_id, None)
    }
    #[allow(clippy::too_many_lines)]
    pub(crate) fn publish_branch(
        bundle: &mut ProjectBundle,
        history: &HistorySnapshot,
        inputs: &ObjectRef,
        branch_id: &crate::bundle_records::BranchId,
        seed: Option<&LineageRecord>,
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
        if !bundle
            .store
            .with_namespace(|n| Ok(n.object_batch_active()))?
            && let Err(error) = bundle.collect_after_ack()
        {
            // Namespace uncertainty still fences publication; ordinary unlink failures retry.
            bundle.store.with_namespace(|n| n.verify())?;
            if matches!(
                error,
                BundleError::Unsettled | BundleError::DurabilityUnknown
            ) {
                return Err(error);
            }
            crate::namespace::note("gc_retry_pending", &bundle.store.resolved_root()?);
        }
        let prior = bundle.branches.get(branch_id).cloned();
        let old_history = if let Some(prior) = &prior {
            Some(if let Some(cached) = bundle.history_cache.get(branch_id) {
                std::sync::Arc::clone(cached)
            } else {
                std::sync::Arc::new(
                    bundle
                        .store
                        .read_record::<HistorySnapshot>(&prior.history_ref)?,
                )
            })
        } else {
            None
        };
        let explicit_floor = bundle.pending_command.as_ref().is_some_and(|e| {
            matches!(
                e.transition,
                Some(crate::receipts::HistoryRequest::Floor { .. })
            )
        });
        let aligned = if let Some(old) = &old_history
            && history.floor_offset < old.floor_offset
        {
            kinewright_core::HistoryTransition::floor(
                history.clone(),
                usize::try_from(old.floor_offset - history.floor_offset)
                    .map_err(|_| BundleError::Capacity("history floor offset"))?,
            )
            .map_err(|_| BundleError::Corrupt("history before durable floor".into()))?
            .after()
            .clone()
        } else {
            history.clone()
        };
        if !explicit_floor
            && old_history.as_ref().is_some_and(|old| {
                aligned.floor_offset == old.floor_offset
                    && (aligned.baseline != old.baseline
                        || aligned.batches.len() < old.batches.len()
                        || aligned.batches[..old.batches.len()] != old.batches)
            })
        {
            return Err(BundleError::Corrupt(
                "history is not a continuation of the admitted head".into(),
            ));
        }
        if !explicit_floor
            && let Some(old) = &old_history
            && aligned.floor_offset > old.floor_offset
        {
            let cut = usize::try_from(aligned.floor_offset - old.floor_offset)
                .map_err(|_| BundleError::Capacity("history floor offset"))?;
            let expected = kinewright_core::HistoryTransition::floor((***old).clone(), cut)
                .map_err(|_| BundleError::Corrupt("unproved history floor".into()))?;
            let expected = expected.after();
            if aligned.baseline != expected.baseline
                || aligned.batches.len() < expected.batches.len()
                || aligned.batches[..expected.batches.len()] != expected.batches
            {
                return Err(BundleError::Corrupt(
                    "history floor differs from admitted cut".into(),
                ));
            }
        }
        let bounded = if old_history.as_ref().is_some_and(|old| ***old == aligned) {
            aligned.clone()
        } else {
            bound_history(&aligned, bundle.store.limits())?
        };
        let checked = if old_history.as_ref().is_some_and(|old| ***old == bounded) {
            std::sync::Arc::clone(old_history.as_ref().unwrap())
        } else {
            let serialized =
                serde_json::to_vec(&bounded).map_err(|e| BundleError::Corrupt(e.to_string()))?;
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
            std::sync::Arc::new(checked)
        };
        let manifest = bundle.store.read_input_manifest(inputs)?;
        if manifest.project_id.as_ref() != Some(&bundle.project_id) || manifest.session_id.is_some()
        {
            return Err(BundleError::Identity("current input manifest owner".into()));
        }
        validate_document_inputs(&checked.document, &manifest)?;
        let mut retained_manifest = (*manifest).clone();
        if let Some(prior) = &prior {
            for reference in &prior.history_input_manifest_refs {
                let old = bundle.store.read_input_manifest(reference)?;
                retained_manifest.entries.extend(old.entries.clone());
            }
        }
        let floor_discard = old_history
            .as_ref()
            .map(|old| {
                usize::try_from(
                    checked
                        .floor_offset
                        .checked_sub(old.floor_offset)
                        .ok_or_else(|| {
                            BundleError::Corrupt("history floor moved backward".into())
                        })?,
                )
                .map_err(|_| BundleError::Capacity("history floor offset"))
            })
            .transpose()?
            .filter(|n| *n > 0);
        let mut contexts = prior
            .as_ref()
            .map_or_else(Vec::new, |p| p.history_input_manifest_refs.clone());
        if contexts.is_empty() {
            contexts.push(inputs.clone());
        }
        while contexts.len() < checked.batches.len() + floor_discard.unwrap_or(0) + 1 {
            contexts.push(inputs.clone());
        }
        let floor_inputs = if let Some(discard) = floor_discard {
            contexts = contexts
                .get(discard..)
                .ok_or_else(|| BundleError::Corrupt("floor input-context cut".into()))?
                .to_vec();
            // The boundary document can precede the input context of a multi-edit save.
            // Give the retained checkpoint its own exact owned graph before validating it.
            retain_history_inputs(&checked, &mut retained_manifest);
            validate_inputs(&checked, &retained_manifest)?;
            crate::store::BundleRecord::validate(&retained_manifest, bundle.store.limits())?;
            Some(retained_manifest)
        } else {
            validate_inputs(&checked, &retained_manifest)?;
            None
        };
        let mut batch_ids = prior
            .as_ref()
            .map_or_else(Vec::new, |p| p.batch_ids.clone());
        let total_batches = checked.batches.len() + floor_discard.unwrap_or(0);
        while batch_ids.len() < total_batches {
            batch_ids.push(BatchId::fresh()?);
        }
        let all_batch_ids = batch_ids.clone();
        if let Some(discard) = floor_discard {
            batch_ids = batch_ids
                .get(discard..)
                .ok_or_else(|| BundleError::Corrupt("floor batch cut".into()))?
                .to_vec();
        }
        let old_lineage = prior
            .as_ref()
            .map(|p| bundle.store.read_record::<LineageRecord>(&p.lineage_ref))
            .transpose()?;
        let floor_seed = if let Some(discard) = floor_discard {
            let old = if aligned.floor_offset < checked.floor_offset {
                &aligned
            } else {
                &***old_history
                    .as_ref()
                    .ok_or_else(|| BundleError::Corrupt("floor without previous history".into()))?
            };
            let prefix = crate::bundle_branch::replay_prefix(old, discard)?;
            let prefix_lineage = lineage(
                branch_id,
                &prefix,
                &all_batch_ids[..discard],
                old_lineage.as_deref(),
            )?;
            Some(LineageRecord {
                branch_origin: branch_id.clone(),
                baseline: prefix_lineage.current.clone(),
                current: prefix_lineage.current,
                undo: vec![],
                redo: vec![],
                split_ancestry: prefix_lineage.split_ancestry,
            })
        } else {
            None
        };
        let lineage = lineage(
            branch_id,
            &checked,
            &batch_ids,
            floor_seed.as_ref().or(seed).or(old_lineage.as_deref()),
        )?;
        let generation = bundle
            .generation
            .checked_add(1)
            .ok_or(BundleError::Capacity("root generation"))?;
        let unchanged = old_history.as_ref().is_some_and(|h| ***h == **checked)
            && prior
                .as_ref()
                .is_some_and(|p| p.input_manifest_ref == *inputs);
        let head_id = if unchanged
            && bundle
                .pending_command
                .as_ref()
                .is_none_or(|e| e.transition.is_none())
        {
            prior.as_ref().unwrap().head_id.clone()
        } else {
            HeadId::fresh()?
        };
        barrier(bundle, CommitBarrier::B0)?;
        bundle.store.begin_publication()?;
        let mut seals: Vec<SealedObject> = vec![];
        macro_rules! seal {
            ($value:expr) => {{
                let sealed = bundle.store.admit_record($value)?;
                let reference = sealed.reference.clone();
                seals.push(sealed);
                reference
            }};
        }
        let (document_ref, baseline_ref, history_ref, lineage_ref) = if unchanged {
            let prior = prior.as_ref().unwrap();
            (
                prior.document_ref.clone(),
                prior.baseline_ref.clone(),
                prior.history_ref.clone(),
                prior.lineage_ref.clone(),
            )
        } else {
            (
                seal!(&*checked.document),
                seal!(&*checked.baseline),
                seal!(&**checked),
                seal!(&lineage),
            )
        };
        if let Some(floor_inputs) = floor_inputs {
            contexts[0] = seal!(&floor_inputs);
        }
        let checkpoint = BranchCheckpoint {
            branch_id: branch_id.clone(),
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
            floor_generation: if floor_discard.is_some() {
                prior
                    .as_ref()
                    .map_or(0, |p| p.floor_generation)
                    .checked_add(1)
                    .ok_or(BundleError::Capacity("floor generation"))?
            } else {
                prior.as_ref().map_or(0, |p| p.floor_generation)
            },
            command_cut: bundle
                .pending_command
                .as_ref()
                .map(|e| e.id.clone())
                .or_else(|| prior.as_ref().and_then(|p| p.command_cut.clone())),
            predecessor: prior.as_ref().map(|p| p.head_id.clone()),
            take_map_revision_ref: None,
        };
        let checkpoint_ref = seal!(&checkpoint);
        let mut branches = bundle.branches.clone();
        branches.insert(branch_id.clone(), checkpoint);
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
        let preceding_checkpoint_ref = branch_index.get(branch_id).cloned();
        branch_index.insert(branch_id.clone(), checkpoint_ref.clone());
        let mut evidence = bundle.evidence.clone();
        if let Some(envelope) = bundle.pending_command.as_ref()
            && let Some(transition) = &envelope.transition
        {
            evidence.retain(|r| {
                !matches!(
                    r.record_kind,
                    RecordKind::HistoryFloor | RecordKind::RecoveryClose
                )
            });
            let previous = prior
                .as_ref()
                .ok_or_else(|| BundleError::Corrupt("transition without checkpoint".into()))?;
            let reference = match transition {
                crate::receipts::HistoryRequest::Floor { .. } => {
                    seal!(&crate::bundle_records::HistoryFloorCheckpoint {
                        preceding_checkpoint_generation: previous.checkpoint_generation,
                        preceding_floor_generation: previous.floor_generation,
                        floor_generation: branches[branch_id].floor_generation,
                        command_cut: Some(envelope.id.clone()),
                        old_baseline_ref: previous.baseline_ref.clone(),
                        new_baseline_ref: branches[branch_id].baseline_ref.clone(),
                        checkpoint_ref: checkpoint_ref.clone(),
                        equivalence_digest: branches[branch_id].history_ref.sha256.clone(),
                    })
                }
                crate::receipts::HistoryRequest::RecoveryClose {
                    suffix_digest,
                    reconciled_cut,
                    ..
                } => seal!(&crate::bundle_records::RecoveryCloseCheckpoint {
                    preceding_checkpoint_ref: preceding_checkpoint_ref
                        .ok_or_else(|| BundleError::Corrupt("close predecessor".into()))?,
                    reconciled_cut: reconciled_cut.clone(),
                    suffix_digest: suffix_digest.clone(),
                    closed_checkpoint_ref: checkpoint_ref.clone(),
                }),
            };
            evidence.push(reference);
        }
        if floor_discard.is_some()
            && !explicit_floor
            && let Some(previous) = &prior
        {
            evidence.retain(|r| r.record_kind != RecordKind::HistoryFloor);
            evidence.push(seal!(&crate::bundle_records::HistoryFloorCheckpoint {
                preceding_checkpoint_generation: previous.checkpoint_generation,
                preceding_floor_generation: previous.floor_generation,
                floor_generation: branches[branch_id].floor_generation,
                command_cut: branches[branch_id].command_cut.clone(),
                old_baseline_ref: previous.baseline_ref.clone(),
                new_baseline_ref: branches[branch_id].baseline_ref.clone(),
                checkpoint_ref: checkpoint_ref.clone(),
                equivalence_digest: branches[branch_id].history_ref.sha256.clone(),
            }));
        }
        let branch_index_ref = seal!(&branch_index);
        barrier(bundle, CommitBarrier::B1)?;
        let mut staged_inventory = None;
        let mut recovery = bundle.recovery.clone();
        if !bundle.admission.raw_bytes().is_empty() {
            let deps = bundle
                .admission
                .data
                .file
                .as_ref()
                .and_then(|f| f.studio.as_ref())
                .map_or_else(Vec::new, super::bundle::StudioRoot::references);
            let previous = bundle
                .admit_root_checkpoint(bundle.admission.raw_bytes().to_vec().as_slice(), &deps)?;
            let reference = previous.reference.clone();
            seals.push(previous);
            let mut staged = recovery.clone();
            staged.staging = Some(reference.clone());
            staged_inventory = Some(staged);
            recovery.predecessors.insert(0, reference);
            recovery.predecessors.truncate(2);
        }
        recovery.generation = generation;
        recovery.staging = None;
        let mut receipts = bundle.receipts.clone();
        if let Some(envelope) = &bundle.pending_command {
            let result = bundle
                .pending_result
                .clone()
                .unwrap_or_else(|| ReceiptResult::Edit {
                    checkpoint_ref: checkpoint_ref.clone(),
                    batch_id: branches[&bundle.manual_branch_id].batch_ids.last().cloned(),
                });
            if matches!(result, ReceiptResult::Comment { .. }) {
                receipts.comments = receipts
                    .comments
                    .checked_add(1)
                    .ok_or(BundleError::Capacity("comment count"))?;
            }
            receipts.detailed.push(CommandReceipt {
                id: envelope.id.clone(),
                digest: envelope.digest()?,
                project_id: bundle.project_id.clone(),
                target_branch: envelope.target_branch.clone(),
                original_epoch: envelope.owner_epoch,
                expected_head: envelope.expected_head.clone(),
                head_id: head_id.clone(),
                generation,
                result,
            });
        }
        receipts.validate(bundle.store.limits())?;
        let receipt_index_ref = if receipts.detailed.is_empty()
            && receipts.retired.is_empty()
            && receipts.comments == 0
        {
            None
        } else {
            Some(seal!(&receipts))
        };
        let issuer_state_ref = if bundle.issuers.is_empty() && receipt_index_ref.is_none() {
            None
        } else {
            Some(seal!(&bundle.issuers))
        };
        let mut roots = vec![
            branch_index_ref.clone(),
            branches[&bundle.manual_branch_id]
                .input_manifest_ref
                .clone(),
        ];
        roots.extend(receipt_index_ref.clone());
        roots.extend(issuer_state_ref.clone());
        let mut inventory = SupportInventory {
            required_fields: BTreeSet::new(),
            required_kinds: BTreeSet::new(),
            required_apis: crate::bundle_recovery::supported_apis(),
        };
        let mut retained = roots.clone();
        retained.extend(evidence.clone());
        retained.extend(recovery.references());
        inventory.required_fields = bundle.store.support_fields(&retained)?;
        let store_manifest = bundle.store.selected_manifest(
            &roots,
            &evidence,
            recovery.clone(),
            &inventory,
            generation,
        )?;
        let store_manifest_ref = seal!(&store_manifest);
        let root = StudioRoot {
            project_id: bundle.project_id.clone(),
            origin_project_ids: bundle.origin_project_ids.clone(),
            origin_session_ids: bundle.origin_session_ids.clone(),
            generation,
            creation_brief_ref: None,
            manual_branch_id: bundle.manual_branch_id.clone(),
            manual_switch_generation: 0,
            candidate_branch_ids: branches
                .keys()
                .filter(|id| **id != bundle.manual_branch_id)
                .cloned()
                .collect(),
            branch_index_ref,
            version_index_ref: None,
            feedback_index_ref: None,
            run_index_ref: None,
            claim_index_ref: None,
            input_manifest_ref: branches[&bundle.manual_branch_id]
                .input_manifest_ref
                .clone(),
            receipt_index_ref,
            issuer_state_ref,
            store_manifest_ref,
        };
        root.admit()?;
        bundle
            .store
            .with_namespace(crate::namespace::ProjectNamespace::finish_object_batch)?;
        barrier(bundle, CommitBarrier::B2)?;
        if let Some(staged) = staged_inventory {
            bundle.store.publish_inventory(&staged)?;
        }
        barrier(bundle, CommitBarrier::B3)?;
        let manual_history =
            if *branch_id == bundle.manual_branch_id {
                None
            } else {
                Some(bundle.store.read_record::<HistorySnapshot>(
                    &branches[&bundle.manual_branch_id].history_ref,
                )?)
            };
        let manual_document = manual_history
            .as_ref()
            .map_or(&checked.document, |h| &h.document);
        let (bytes, _encoded_root) = serialize_root(&bundle.store, manual_document, &root)?;
        let next_admission = BundleWriteAdmission::from_bytes_using(
            Some(bundle.store.resolved_root()?),
            bytes,
            bundle.store.usage_arc(),
        )?;
        next_admission.require_supported()?;
        bundle.store.closure(&root.references())?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("root_before_publish", &bundle.store.resolved_root()?)?;
        let saved_path = bundle.store.resolved_root()?;
        let collection = bundle.store.collection_status()?;
        if let Some(pending) = &mut bundle.pending_save {
            pending.root_sha256 = Some(kinewright_media::sha256_bytes(next_admission.raw_bytes()));
        }
        if let Err(error) = bundle.store.publish_root(next_admission.raw_bytes()) {
            if bundle.store.with_namespace(|n| Ok(n.is_uncertain()))? {
                bundle.unsettled = true;
                return Err(BundleError::DurabilityUnknown);
            }
            return Err(error);
        }
        // Publication is monotonic: any fallible tail is unsettled, never a definite rollback.
        bundle.unsettled = true;
        let tail = (|| -> Result<(), BundleError> {
            bundle.evidence = evidence;
            bundle.receipts = receipts;
            bundle.root = Some(root);
            bundle.branches = branches;
            bundle
                .history_cache
                .insert(branch_id.clone(), std::sync::Arc::clone(&checked));
            bundle.generation = generation;
            bundle.inventory = inventory;
            bundle.recovery = recovery;
            bundle.admission = next_admission;
            bundle.durable = Some(bundle.store.mark_committed(&bundle.root_references()?)?);
            barrier(bundle, CommitBarrier::B5)?;
            bundle.pending_evidence.clear();
            Ok(())
        })();
        if tail.is_err() {
            return Err(BundleError::DurabilityUnknown);
        }
        bundle.unsettled = false;
        drop(seals);
        Ok(BundleSaveReport {
            collection,
            path: saved_path,
            project_id: bundle.project_id.clone(),
            generation,
            head_id,
            root_sha256: kinewright_media::sha256_bytes(bundle.admission.raw_bytes()),
            byte_length: bundle.admission.raw_bytes().len() as u64,
        })
    }
}
pub(crate) fn bound_history(
    history: &HistorySnapshot,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<HistorySnapshot, BundleError> {
    #[cfg(any(test, feature = "test-util"))]
    let started = std::time::Instant::now();
    let result = bound_history_inner(history, limits);
    #[cfg(any(test, feature = "test-util"))]
    crate::io_observer::note_history_bound(started.elapsed());
    result
}

fn bound_history_inner(
    history: &HistorySnapshot,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<HistorySnapshot, BundleError> {
    let fits = |h: &HistorySnapshot| -> bool {
        let Ok(encoded) = h.encoded_bytes() else {
            return false;
        };
        if encoded > limits.branch_history_bytes {
            return false;
        }
        let Ok(bytes) = serde_json::to_vec(h) else {
            return false;
        };
        crate::store::preflight_record(&bytes, limits)
            .is_ok_and(|n| n <= limits.branch_history_bytes)
    };
    // Keep the common small checkpoint path direct. For a long session, component
    // costs can reject the original stream without first serializing tens of MiB.
    if history.batches.len() <= 64 && fits(history) {
        return Ok(history.clone());
    }
    // Independent JSON subtrees give a lower bound without reserializing every suffix.
    // Zeroing counters only shortens their encoding; the final fit/replay remains authoritative.
    let undo = suffix_costs(history.cursor.undo.iter(), limits);
    let redo = suffix_costs(history.cursor.redo.iter(), limits);
    let mut batches = history.batches.clone();
    for batch in &mut batches {
        batch.order = 0;
        batch.undo_entries = 0;
    }
    let batches = suffix_costs(batches.iter(), limits);
    if history.batches.len() > 64
        && undo[0].saturating_add(redo[0]).saturating_add(batches[0]) <= limits.branch_history_bytes
        && fits(history)
    {
        return Ok(history.clone());
    }
    let document_cost = record_cost(&*history.document, limits)?;
    let skeleton = serde_json::json!({"floor_offset":1,"baseline":null,"document":null,"batches":[],"cursor":{"undo":[],"redo":[]}});
    // Remove the two null value slots; their actual Document subtrees are added below.
    let overhead = record_cost(&skeleton, limits)?
        .checked_sub(128)
        .ok_or(BundleError::Capacity("history cost"))?;
    let mut baseline_cost: Option<(std::sync::Arc<kinewright_core::Document>, u64)> = None;
    kinewright_core::HistoryTransition::find_floor(
        history.clone(),
        |cut, dropped, baseline| {
            let count = history.batches.len() - cut;
            let Some(undo_cost) = undo.get(dropped) else {
                return false;
            };
            let lower = undo_cost
                .saturating_add(redo[0])
                .saturating_add(batches[cut]);
            if count > kinewright_core::HISTORY_COMMANDS || lower > limits.branch_history_bytes {
                return false;
            }
            if baseline_cost
                .as_ref()
                .is_none_or(|(prior, _)| !std::sync::Arc::ptr_eq(prior, baseline))
            {
                baseline_cost = Some((
                    std::sync::Arc::clone(baseline),
                    record_cost(&**baseline, limits).unwrap_or(u64::MAX),
                ));
            }
            let Some(offset) = history.floor_offset.checked_add(cut as u64) else {
                return false;
            };
            let commas = count.saturating_sub(1)
                + (history.cursor.undo.len() - dropped).saturating_sub(1)
                + history.cursor.redo.len().saturating_sub(1);
            let mut cost = lower
                .saturating_add(document_cost)
                .saturating_add(baseline_cost.as_ref().unwrap().1)
                .saturating_add(overhead)
                .saturating_add(2 * commas as u64)
                .saturating_add(2 * u64::from(offset.ilog10()));
            for (order, batch) in history.batches[cut..].iter().enumerate() {
                let Some(entries) = batch.undo_entries.checked_sub(dropped) else {
                    return false;
                };
                // Only normalized order and undo count change in an equivalent retained batch.
                cost = cost.saturating_add(
                    2 * u64::from(
                        order.checked_ilog10().unwrap_or(0) + entries.checked_ilog10().unwrap_or(0),
                    ),
                );
            }
            cost <= limits.branch_history_bytes
        },
        fits,
    )
    .map(|transition| transition.after().clone())
    .map_err(|_| BundleError::Capacity("history baseline exceeds branch allowance"))
}

fn record_cost(
    value: &impl serde::Serialize,
    limits: &crate::storage_budget::StorageLimits,
) -> Result<u64, BundleError> {
    let bytes = serde_json::to_vec(value).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    crate::store::preflight_record(&bytes, limits)
}

fn suffix_costs<'a, T: serde::Serialize + PartialEq + Clone + 'a>(
    values: impl DoubleEndedIterator<Item = &'a T>,
    limits: &crate::storage_budget::StorageLimits,
) -> Vec<u64> {
    let mut costs = vec![0_u64];
    let mut cache: Vec<(T, u64)> = vec![];
    for value in values.rev() {
        let cost = if let Some((_, cost)) = cache.iter().find(|(prior, _)| prior == value) {
            *cost
        } else {
            let cost = record_cost(value, limits).unwrap_or(u64::MAX);
            if cache.len() == 16 {
                cache.remove(0);
            }
            cache.push((value.clone(), cost));
            cost
        };
        costs.push(costs.last().unwrap().saturating_add(cost));
    }
    costs.reverse();
    costs
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
pub(crate) fn validate_inputs(
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
        validate_document_inputs(document, manifest)?;
    }
    Ok(())
}
fn retain_history_inputs(history: &HistorySnapshot, manifest: &mut OwnedInputManifest) {
    let documents: Vec<_> = std::iter::once(&history.baseline)
        .chain(std::iter::once(&history.document))
        .chain(
            history
                .cursor
                .undo
                .iter()
                .chain(&history.cursor.redo)
                .map(|e| &e.document),
        )
        .collect();
    manifest.entries.retain(|entry| {
        documents.iter().any(|document| {
            document
                .media_pool
                .iter()
                .any(|a| entry.entry_id == format!("media:{}", a.id.0))
                || document.lut_assets.iter().any(|a| {
                    lut_entry_id_matches(&entry.entry_id, a)
                        && entry.object.sha256 == a.sha256
                        && entry.object.byte_length == a.byte_len
                })
                || (entry.entry_id == "font:Inter"
                    && document
                        .tracks
                        .iter()
                        .flat_map(|t| &t.clips)
                        .any(|c| matches!(c.content, kinewright_core::ClipContent::Title(_))))
        })
    });
    for entry in &mut manifest.entries {
        if entry.object.record_kind == RecordKind::OwnedLut
            && let Some(asset) = documents.iter().flat_map(|d| &d.lut_assets).find(|a| {
                lut_entry_id_matches(&entry.entry_id, a)
                    && entry.object.sha256 == a.sha256
                    && entry.object.byte_length == a.byte_len
            })
        {
            entry.entry_id = format!("lut:{}:{}:{}", asset.id.0, asset.sha256, asset.byte_len);
        }
    }
    // Merge dependency metadata for identical entries; conflicting non-LUT identities refuse.
    let mut unique: BTreeMap<String, crate::owned_inputs::OwnedInputEntry> = BTreeMap::new();
    for entry in std::mem::take(&mut manifest.entries) {
        if let Some(prior) = unique.get_mut(&entry.entry_id) {
            if prior.object == entry.object {
                prior.dependencies.extend(entry.dependencies);
                prior.dependencies.sort();
                prior.dependencies.dedup();
            } else {
                // Retain the conflict so manifest validation refuses before any root publication.
                manifest.entries.push(entry);
            }
        } else {
            unique.insert(entry.entry_id.clone(), entry);
        }
    }
    manifest.entries.extend(unique.into_values());
}
fn lut_entry_id_matches(id: &str, asset: &kinewright_core::LutAsset) -> bool {
    id == format!("lut:{}", asset.id.0)
        || id == format!("lut:{}:{}", asset.id.0, asset.sha256)
        || id == format!("lut:{}:{}:{}", asset.id.0, asset.sha256, asset.byte_len)
}
pub(crate) fn validate_document_inputs(
    document: &kinewright_core::Document,
    manifest: &OwnedInputManifest,
) -> Result<(), BundleError> {
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
            lut_entry_id_matches(&e.entry_id, asset)
                && e.object.record_kind == RecordKind::OwnedLut
                && e.object.sha256 == asset.sha256
                && e.object.byte_length == asset.byte_len
        }) {
            return Err(BundleError::Unsupported(
                "retained LUT lacks owned bytes".into(),
            ));
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "one ordered history replay of occurrence maps"
)]
pub(crate) fn lineage(
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
    let mut split_ancestry =
        old.map_or_else(Vec::new, |old| ancestry(&baseline, &old.split_ancestry));
    for (batch, id) in history.batches.iter().zip(batches) {
        match &batch.command {
            JournalCommand::Undo => {
                if let Some(previous) = undo.pop() {
                    redo.push(current);
                    current = previous;
                }
            }
            JournalCommand::Redo => {
                if let Some(next) = redo.pop() {
                    undo.push(current);
                    current = next;
                }
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
    split_ancestry = ancestry(&current, &split_ancestry);
    Ok(LineageRecord {
        branch_origin: branch.clone(),
        baseline,
        current,
        undo,
        redo,
        split_ancestry,
    })
}

fn ancestry(
    current: &BTreeMap<ClipId, OccurrenceId>,
    all: &[(OccurrenceId, OccurrenceId, kinewright_core::Clip)],
) -> Vec<(OccurrenceId, OccurrenceId, kinewright_core::Clip)> {
    let mut retained: BTreeSet<_> = current.values().cloned().collect();
    loop {
        let before = retained.len();
        for (child, parent, _) in all {
            if retained.contains(child) {
                retained.insert(parent.clone());
            }
        }
        if before == retained.len() {
            break;
        }
    }
    all.iter()
        .filter(|(child, _, _)| retained.contains(child))
        .cloned()
        .collect()
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
