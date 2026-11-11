//! H1 records use the existing `RunIndex` edge and the single metadata publisher.
use super::{OrchestrationReceipt, ValidatedRunUpdate};
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{ObjectRef, ProjectId, RecordKind},
    orchestration_records::{
        OrchestrationEnvelope, OrchestrationPayload, OrchestrationRecord, RunId, RunProjection,
    },
    receipts::ReceiptLookup,
    storage_budget::StorageLimits,
    store::{BundleRecord, SealedObject, record_sealed},
    studio_records::{
        FeedbackBatchId, FeedbackIndex, MetadataChange, MetadataEnvelope, MetadataScope,
        MetadataUpdate, RunIndex,
    },
};
use serde::{Deserialize, Serialize};

fn invalid(value: impl std::fmt::Debug) -> BundleError {
    BundleError::Identity(format!("orchestration: {value:?}"))
}

impl record_sealed::Sealed for OrchestrationRecord {}
impl BundleRecord for OrchestrationRecord {
    const KIND: RecordKind = RecordKind::RunIndex;
    fn dependencies(&self) -> Vec<ObjectRef> {
        OrchestrationRecord::dependencies(self).unwrap_or_default()
    }
    fn validate_admission(&self, store: &crate::store::ProjectStore) -> Result<(), BundleError> {
        self.validate_retention_admission(store)
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        OrchestrationRecord::dependencies(self).map_err(invalid)?;
        crate::studio_records::bounded(self, limits.control_bytes)
    }
}

/// `RunIndex` is the registered tagged S4 record kind as well as the S3 root index.
/// Root admission still requires the S3 index, so a scheduler cannot supply a root pointer.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum RegisteredRunIndex {
    Index(Box<RunIndex>),
    Record(Box<OrchestrationRecord>),
}
impl record_sealed::Sealed for RegisteredRunIndex {}
impl BundleRecord for RegisteredRunIndex {
    const KIND: RecordKind = RecordKind::RunIndex;
    fn dependencies(&self) -> Vec<ObjectRef> {
        match self {
            Self::Index(index) => BundleRecord::dependencies(index.as_ref()),
            Self::Record(record) => BundleRecord::dependencies(record.as_ref()),
        }
    }
    fn validate_admission(&self, store: &crate::store::ProjectStore) -> Result<(), BundleError> {
        match self {
            Self::Index(index) => index.validate_admission(store),
            Self::Record(record) => record.validate_admission(store),
        }
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        match self {
            Self::Index(index) => index.validate(limits),
            Self::Record(record) => record.validate(limits),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedRunCut {
    pub metadata_envelope: MetadataEnvelope,
    pub receipt: OrchestrationReceipt,
    pub snapshot: ObjectRef,
    pub segment: ObjectRef,
    pub consumed_batch: Option<FeedbackBatchId>,
}
impl PublishedRunCut {
    pub(crate) fn references(&self) -> Vec<ObjectRef> {
        vec![
            self.snapshot.clone(),
            self.segment.clone(),
            self.receipt.envelope.authority.grant_ref.clone(),
        ]
    }
    pub(crate) fn validate_cut(
        &self,
        run: &RunId,
        snapshot: &OrchestrationRecord,
        events: &OrchestrationRecord,
        previous: Option<&Self>,
    ) -> Result<ValidatedRunUpdate, BundleError> {
        let (
            OrchestrationPayload::Snapshot(projection),
            OrchestrationPayload::EventSegment(segment),
        ) = (&snapshot.payload, &events.payload)
        else {
            return Err(invalid("cut record kind"));
        };
        let update = ValidatedRunUpdate::new(
            &self.receipt.envelope.project,
            self.receipt.envelope.expected_generation,
            &self.receipt.envelope,
            (**projection).clone(),
            segment.restore(projection).map_err(invalid)?,
            self.receipt.decision.clone(),
        )
        .map_err(invalid)?;
        update
            .validate_returned_receipt(&self.receipt)
            .map_err(invalid)?;
        if &projection.run != run
            || self.metadata_envelope.id != self.receipt.envelope.id
            || self.metadata_envelope.project_id != self.receipt.envelope.project
            || self.metadata_envelope.owner_epoch != self.receipt.envelope.authority.owner_epoch
            || segment.binding.previous.as_ref() != previous.map(|cut| &cut.segment)
            || segment.snapshot != self.snapshot
            || projection.sequence
                != previous.map_or(1, |cut| cut.receipt.sequence.saturating_add(1))
        {
            return Err(invalid("cut identity or predecessor"));
        }
        Ok(update)
    }
}

impl ProjectBundle {
    pub(crate) fn validate_run_publication_binding(
        &self,
        metadata: &MetadataEnvelope,
        transition: &ValidatedRunUpdate,
    ) -> Result<(), BundleError> {
        let _reading = crate::run_admission::reading(&self.store);
        transition
            .validate_returned_receipt(transition.receipt())
            .map_err(invalid)?;
        let envelope = transition.envelope();
        if envelope.project != self.project_id
            || envelope.id != metadata.id
            || envelope.authority.owner_epoch != self.store.owner_epoch()
            || envelope.expected_generation != self.generation
        {
            return Err(invalid("publication envelope"));
        }
        let index = self.run_index()?;
        let previous = index
            .orchestration
            .get(&transition.projection().run)
            .and_then(|cuts| cuts.last());
        if transition.projection().sequence
            != previous.map_or(1, |cut| cut.receipt.sequence.saturating_add(1))
            || transition.segment().previous.as_ref() != previous.map(|cut| &cut.segment)
        {
            return Err(invalid("successor isolation"));
        }
        if let Some(previous) = previous {
            let reference = index
                .cumulative
                .get(&transition.projection().run)
                .unwrap_or(&previous.snapshot);
            let record = self.store.read_record::<OrchestrationRecord>(reference)?;
            let OrchestrationPayload::Snapshot(before) = &record.payload else {
                return Err(invalid("previous snapshot kind"));
            };
            super::validate_projection_successor(before, transition.projection())
                .map_err(invalid)?;
        }
        Ok(())
    }
    pub(crate) fn stage_orchestration_update(
        &mut self,
        metadata_envelope: &MetadataEnvelope,
        transition: &ValidatedRunUpdate,
        consume_batch: Option<&FeedbackBatchId>,
        feedback: &mut FeedbackIndex,
    ) -> Result<(Vec<SealedObject>, ObjectRef), BundleError> {
        self.validate_run_publication_binding(metadata_envelope, transition)?;
        let original = self.run_index()?;
        let mut index = (*original).clone();
        let run = &transition.projection().run;
        if let Some(batch) = consume_batch {
            if transition.receipt().decision != super::CommandDecision::Applied {
                return Err(invalid("refused feedback consumption"));
            }
            let submitted = feedback
                .batches
                .get_mut(batch)
                .ok_or_else(|| invalid("unsubmitted batch"))?;
            let legacy_run = crate::studio_records::RunId(run.0.clone());
            let director = transition
                .projection()
                .tasks
                .first()
                .ok_or_else(|| invalid("feedback run has no Director"))?;
            if !self.revises_head(
                &director.brief.scope.base,
                &submitted.request.expected_candidate_head,
            )? {
                return Err(invalid("feedback candidate head changed"));
            }
            if submitted
                .consumed_by
                .as_ref()
                .is_some_and(|old| old != &legacy_run)
            {
                return Err(invalid("batch already consumed"));
            }
            submitted.consumed_by = Some(legacy_run);
        }
        let base = self.semantic_snapshot(run)?;
        let snapshot_record = OrchestrationRecord::retained(
            OrchestrationPayload::Snapshot(Box::new(transition.projection().clone())),
            base,
            &self.store,
        )?;
        let snapshot = self.store.admit_record(&snapshot_record)?;
        let segment = self.store.admit_record(&OrchestrationRecord {
            retention: crate::orchestration_retention::Retention::Complete,
            version: 1,
            payload: OrchestrationPayload::EventSegment(
                crate::orchestration_records::StoredEventSegment {
                    binding: transition.segment().binding().map_err(invalid)?,
                    snapshot: snapshot.reference.clone(),
                },
            ),
        })?;
        self.advance_run_budget(&mut index, transition)?;
        index.cumulative.remove(run);
        index
            .orchestration
            .entry(run.clone())
            .or_default()
            .push(PublishedRunCut {
                metadata_envelope: metadata_envelope.clone(),
                receipt: transition.receipt().clone(),
                snapshot: snapshot.reference.clone(),
                segment: segment.reference.clone(),
                consumed_batch: consume_batch.cloned(),
            });
        let sealed_index = self.store.admit_record(&index)?;
        let reference = sealed_index.reference.clone();
        Ok((vec![snapshot, segment, sealed_index], reference))
    }

    /// Whether a run based on `base` revises the cut at `expected`: the base is that
    /// head itself, or a candidate forked from it with nothing applied yet (the same
    /// document, no accepted batches) while the manual branch still stands there. A
    /// fork publishes a fresh head identity, so the head alone cannot link it.
    fn revises_head(
        &self,
        base: &crate::orchestration_records::BranchBinding,
        expected: &crate::bundle_records::HeadId,
    ) -> Result<bool, BundleError> {
        if &base.head == expected {
            return Ok(true);
        }
        let candidate = self
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&base.checkpoint)?;
        let manual = self.checkpoint(&self.manual_branch_id)?;
        Ok(base.branch != self.manual_branch_id
            && self.checkpoint_reference(&base.branch)? == base.checkpoint
            && candidate.branch_id == base.branch
            && candidate.head_id == base.head
            && candidate.batch_ids.is_empty()
            && candidate.document_hash == base.document_hash
            && &manual.head_id == expected
            && manual.document_hash == candidate.document_hash)
    }
    pub fn orchestration_cut(&self, run: &RunId) -> Result<Option<PublishedRunCut>, BundleError> {
        let _reading = crate::run_admission::reading(&self.store);
        Ok(self
            .run_index()?
            .orchestration
            .get(run)
            .and_then(|cuts| cuts.last())
            .cloned())
    }
    pub fn orchestration_projection(
        &self,
        run: &RunId,
    ) -> Result<Option<RunProjection>, BundleError> {
        let _reading = crate::run_admission::reading(&self.store);
        let index = self.run_index()?;
        let Some(cut) = index.orchestration.get(run).and_then(|cuts| cuts.last()) else {
            return Ok(None);
        };
        let reference = index.cumulative.get(run).unwrap_or(&cut.snapshot);
        let record = self.store.read_record::<OrchestrationRecord>(reference)?;
        let OrchestrationPayload::Snapshot(projection) = &record.payload else {
            return Err(invalid("snapshot kind"));
        };
        Ok(Some((**projection).clone()))
    }
    /// Read-only lookup. A known command cannot be relabelled as another run.
    pub fn orchestration_receipt(
        &self,
        run: &RunId,
        id: &crate::receipts::CommandId,
    ) -> Result<Option<OrchestrationReceipt>, BundleError> {
        let _reading = crate::run_admission::reading(&self.store);
        for (original_run, cuts) in &self.run_index()?.orchestration {
            for cut in cuts {
                if &cut.receipt.envelope.id == id {
                    if original_run != run {
                        return Err(invalid("receipt identity changed"));
                    }
                    return Ok(Some(cut.receipt.clone()));
                }
            }
        }
        Ok(None)
    }
    pub fn orchestration_receipt_projection(
        &self,
        receipt: &OrchestrationReceipt,
    ) -> Result<RunProjection, BundleError> {
        let _reading = crate::run_admission::reading(&self.store);
        for cut in self.run_index()?.orchestration.values().flatten() {
            if cut.receipt == *receipt {
                let record = self
                    .store
                    .read_record::<OrchestrationRecord>(&cut.snapshot)?;
                if let OrchestrationPayload::Snapshot(state) = &record.payload {
                    return Ok((**state).clone());
                }
            }
        }
        Err(invalid("original receipt projection missing"))
    }
    pub fn orchestration_replay(
        &self,
        envelope: &OrchestrationEnvelope,
        consume_batch: Option<&FeedbackBatchId>,
    ) -> Result<Option<OrchestrationReceipt>, BundleError> {
        let _reading = crate::run_admission::reading(&self.store);
        for cut in self.run_index()?.orchestration.values().flatten() {
            if cut.receipt.envelope.id == envelope.id {
                if &cut.receipt.envelope != envelope || cut.consumed_batch.as_ref() != consume_batch
                {
                    return Err(invalid("original command changed"));
                }
                return Ok(Some(cut.receipt.clone()));
            }
        }
        Ok(None)
    }
}

/// The caller has resolved live S3 authority before invoking this trusted port.
pub fn commit_orchestration(
    owner: &mut crate::editing_session::Ja1EditingSession,
    transition: ValidatedRunUpdate,
    consume_batch: Option<FeedbackBatchId>,
) -> Result<OrchestrationReceipt, BundleError> {
    let envelope = transition.envelope().clone();
    if let Some(receipt) = owner.orchestration_replay(&envelope, consume_batch.as_ref())? {
        return Ok(receipt);
    }
    let metadata = owner.metadata_envelope(envelope.id.clone())?;
    let expected = transition.receipt().clone();
    let update = MetadataUpdate {
        expected_root_generation: envelope.expected_generation,
        change: MetadataChange::OrchestrationUpdate {
            transition: Box::new(transition),
            consume_batch,
        },
    };
    let result = owner.metadata_command(&metadata, MetadataScope::Orchestration, &update);
    let result = result?;
    let (ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) = result else {
        return Err(BundleError::Unsettled);
    };
    if receipt.generation != expected.generation
        || receipt.id != envelope.id
        || receipt.digest != metadata.receipt_envelope(&update)?.digest()?
    {
        return Err(invalid("durable returned receipt"));
    }
    Ok(expected)
}

impl crate::editing_session::Ja1EditingSession {
    pub fn orchestration_cut(&self, run: &RunId) -> Result<Option<PublishedRunCut>, BundleError> {
        self.active_bundle()?.orchestration_cut(run)
    }
    pub fn orchestration_projection(
        &self,
        run: &RunId,
    ) -> Result<Option<RunProjection>, BundleError> {
        self.active_bundle()?.orchestration_projection(run)
    }
    pub fn orchestration_receipt(
        &self,
        run: &RunId,
        id: &crate::receipts::CommandId,
    ) -> Result<Option<OrchestrationReceipt>, BundleError> {
        self.active_bundle()?.orchestration_receipt(run, id)
    }
    pub fn orchestration_receipt_projection(
        &self,
        receipt: &OrchestrationReceipt,
    ) -> Result<RunProjection, BundleError> {
        self.active_bundle()?
            .orchestration_receipt_projection(receipt)
    }
    pub fn orchestration_replay(
        &self,
        envelope: &OrchestrationEnvelope,
        batch: Option<&FeedbackBatchId>,
    ) -> Result<Option<OrchestrationReceipt>, BundleError> {
        self.active_bundle()?.orchestration_replay(envelope, batch)
    }
    pub fn orchestration_runs(&self) -> Result<Vec<RunId>, BundleError> {
        let _reading = crate::run_admission::reading(self.store()?);
        Ok(self
            .active_bundle()?
            .run_index()?
            .orchestration
            .keys()
            .cloned()
            .collect())
    }
    pub fn admit_scheduler_record(
        &mut self,
        record: OrchestrationRecord,
    ) -> Result<SealedObject, BundleError> {
        self.active_bundle_mut()?
            .admit_owned_scheduler_record(record)
    }
    pub fn admit_host_grant(&mut self, bytes: &[u8]) -> Result<ObjectRef, BundleError> {
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        let bundle = self.active_bundle_mut()?;
        let seal = bundle
            .store_mut()?
            .admit_blob(&mut &bytes[..], &reference, &[])?;
        bundle.retain_evidence(&reference)?;
        drop(seal);
        Ok(reference)
    }
    pub fn validate_orchestration_scope(
        &self,
        project: &ProjectId,
        brief: &crate::orchestration_records::TaskBrief,
    ) -> Result<(), BundleError> {
        let _reading = crate::run_admission::reading(self.store()?);
        let store = self.store()?;
        let checkpoint = store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&brief.scope.base.checkpoint)?;
        if project != &self.active_manual_project()?
            || checkpoint.branch_id != brief.scope.base.branch
            || checkpoint.head_id != brief.scope.base.head
            || checkpoint.document_hash != brief.scope.base.document_hash
            || checkpoint.input_manifest_ref != brief.inputs
        {
            return Err(invalid("task base or inputs"));
        }
        for reference in [&brief.plan, &brief.script, &brief.assignment]
            .into_iter()
            .chain(&brief.scope.asset_roots)
            .chain(&brief.scope.consent_refs)
        {
            store.verify(reference)?;
        }
        Ok(())
    }
}
