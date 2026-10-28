//! Typed metadata admission on the existing bundle publisher and receipt ledger.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, ProjectBundle, StudioRoot},
    bundle_commit::BundleCommit,
    bundle_records::{BranchCheckpoint, ObjectRef, RecordKind},
    receipts::{CommandEnvelope, ReceiptLookup, ReceiptResult},
    storage_budget::{StorageLimits, StorageReservation},
    store::{BundleRecord, SealedObject, StorePin, record_sealed},
    studio_records::{
        FeedbackBatch, FeedbackBatchId, FeedbackIndex, FeedbackRevision, MetadataChange,
        MetadataEnvelope, MetadataResult, MetadataScope, MetadataUpdate, ReviseRequest, RunAction,
        RunActionKind, RunCleanup, RunDeliveryRecord, RunEffectRecord, RunEvent, RunEventSegment,
        RunId, RunIndex, RunIntent, RunRecord, RunRequestRecord, RunResultRecord,
        RunSnapshotRecord, RunUsageRecord, RunView, RuntimeRequest, SnapshotDescriptor,
        SnapshotKey, SubmittedBatch, VersionIndex, VoiceRef, bounded, kind, schema,
    },
};
use std::collections::BTreeSet;

macro_rules! record {
    ($ty:ty, $kind:ident, $deps:expr, $check:expr) => {
        impl record_sealed::Sealed for $ty {}
        impl BundleRecord for $ty {
            const KIND: RecordKind = RecordKind::$kind;
            fn dependencies(&self) -> Vec<ObjectRef> {
                ($deps)(self)
            }
            fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
                ($check)(self, limits)?;
                for reference in self.dependencies() {
                    reference.validate()?;
                }
                bounded(self, limits.decoded_record_bytes)
            }
        }
    };
}
record!(
    VersionIndex,
    VersionIndex,
    |v: &VersionIndex| v
        .snapshots
        .values()
        .flat_map(SnapshotDescriptor::references)
        .collect(),
    |v: &VersionIndex, l: &StorageLimits| {
        schema(v.version)?;
        if v.snapshots.len() as u64 > l.versions {
            return Err(BundleError::Capacity("Studio versions"));
        }
        for (id, d) in &v.snapshots {
            if id != &d.key.version
                || d.key.document_hash != d.document_ref.sha256
                || d.input_manifest_ref != d.key.input_manifest_ref
                || d.parent_version
                    .as_ref()
                    .is_some_and(|parent| parent == id || !v.snapshots.contains_key(parent))
            {
                return Err(BundleError::Corrupt("snapshot descriptor binding".into()));
            }
            kind(&d.checkpoint_ref, RecordKind::BranchCheckpoint)?;
            kind(&d.document_ref, RecordKind::Document)?;
            kind(&d.lineage_ref, RecordKind::Lineage)?;
            kind(&d.input_manifest_ref, RecordKind::InputManifest)?;
            if d.revision_refs.len() as u64 > l.comments {
                return Err(BundleError::Capacity("snapshot revision refs"));
            }
            for r in &d.revision_refs {
                kind(r, RecordKind::FeedbackRevision)?;
            }
        }
        Ok(())
    }
);
record!(
    FeedbackRevision,
    FeedbackRevision,
    FeedbackRevision::references,
    |v: &FeedbackRevision, l: &StorageLimits| {
        let a = &v.anchor;
        if v.revision == 0
            || a.frame < 0
            || a.fps.numerator == 0
            || a.fps.denominator == 0
            || a.raster_size.contains(&0)
            || a.script_beat_id.as_ref().is_some_and(|s| s.len() > 256)
            || (v.revision == 1) != v.predecessor.is_none()
        {
            return Err(BundleError::Corrupt("feedback anchor/revision".into()));
        }
        kind(&a.snapshot.input_manifest_ref, RecordKind::InputManifest)?;
        for r in [&v.original_text_ref, &v.strokes_ref, &v.frame_proof_ref] {
            kind(r, RecordKind::OwnedEvidence)?;
        }
        if v.original_text_ref.byte_length > l.comment_text_bytes
            || v.strokes_ref.byte_length > l.command_bytes
        {
            return Err(BundleError::Capacity("feedback text/strokes"));
        }
        if let Some(r) = &v.predecessor {
            kind(r, RecordKind::FeedbackRevision)?;
        }
        if let Some(voice) = &v.voice_ref {
            validate_voice(voice, l)?;
        }
        Ok(())
    }
);
record!(
    FeedbackBatch,
    FeedbackBatch,
    |v: &FeedbackBatch| v.revision_refs.clone(),
    |v: &FeedbackBatch, l: &StorageLimits| {
        if v.revision_refs.is_empty()
            || v.revision_refs.len() as u64 > l.comments
            || v.revision_refs.iter().collect::<BTreeSet<_>>().len() != v.revision_refs.len()
        {
            return Err(BundleError::Capacity("feedback batch revisions"));
        }
        for r in &v.revision_refs {
            kind(r, RecordKind::FeedbackRevision)?;
        }
        Ok(())
    }
);
record!(
    FeedbackIndex,
    FeedbackIndex,
    |v: &FeedbackIndex| {
        let mut refs: Vec<_> = v.revisions.values().flatten().cloned().collect();
        refs.extend(v.batches.values().map(|b| b.batch_ref.clone()));
        for voice in v.notes.values() {
            refs.extend(voice_references(voice));
        }
        refs
    },
    |v: &FeedbackIndex, l: &StorageLimits| {
        schema(v.version)?;
        if v.revisions.len() as u64 > l.comments
            || v.batches.len() as u64 > l.comments
            || v.notes.len() as u64 > l.comments
            || v.revisions.values().map(Vec::len).sum::<usize>() as u64 > l.receipts
        {
            return Err(BundleError::Capacity("feedback index entries"));
        }
        for revisions in v.revisions.values() {
            if revisions.is_empty() {
                return Err(BundleError::Corrupt(
                    "empty feedback revision history".into(),
                ));
            }
            for r in revisions {
                kind(r, RecordKind::FeedbackRevision)?;
            }
        }
        for (id, b) in &v.batches {
            if id != &b.request.batch_id {
                return Err(BundleError::Corrupt("batch request binding".into()));
            }
            kind(&b.batch_ref, RecordKind::FeedbackBatch)?;
        }
        for (id, voice) in &v.notes {
            if id != &voice.note_id {
                return Err(BundleError::Corrupt("note identity".into()));
            }
            validate_voice(voice, l)?;
        }
        Ok(())
    }
);
record!(
    RunIndex,
    RunIndex,
    |v: &RunIndex| {
        let mut refs: Vec<_> = v.runs.values().flat_map(RunRecord::references).collect();
        refs.extend(
            v.orchestration
                .values()
                .flatten()
                .flat_map(crate::orchestration_commit::PublishedRunCut::references),
        );
        refs
    },
    |v: &RunIndex, l: &StorageLimits| {
        schema(v.version)?;
        if (v.runs.len() + v.orchestration.len()) as u64 > l.versions {
            return Err(BundleError::Capacity("run index entries"));
        }
        for cuts in v.orchestration.values() {
            if cuts.is_empty() || cuts.len() as u64 > l.receipts {
                return Err(BundleError::Capacity("orchestration cuts"));
            }
        }
        for (id, run) in &v.runs {
            if id != &run.run_id || id != &run.projection.run_id {
                return Err(BundleError::Corrupt("run projection identity".into()));
            }
            run.projection.validate()?;
            for (refs, k) in [
                (&run.snapshot_refs, RecordKind::RunSnapshot),
                (&run.event_segment_refs, RecordKind::RunEventSegment),
                (&run.effect_refs, RecordKind::RunEffect),
                (&run.request_refs, RecordKind::RunRequest),
                (&run.delivery_refs, RecordKind::RunDelivery),
                (&run.result_refs, RecordKind::RunResult),
                (&run.usage_refs, RecordKind::RunUsage),
            ] {
                if refs.len() as u64 > l.objects {
                    return Err(BundleError::Capacity("run record refs"));
                }
                for r in refs {
                    kind(r, k)?;
                }
            }
        }
        Ok(())
    }
);
record!(
    RunSnapshotRecord,
    RunSnapshot,
    |v: &RunSnapshotRecord| v.view.references(),
    |v: &RunSnapshotRecord, _: &StorageLimits| {
        schema(v.version)?;
        v.view.validate()
    }
);
record!(
    RunEventSegment,
    RunEventSegment,
    |v: &RunEventSegment| v
        .events
        .iter()
        .flat_map(|e| e.delta.view.references())
        .collect(),
    |v: &RunEventSegment, _: &StorageLimits| {
        schema(v.version)?;
        if v.events.len() > 200 {
            return Err(BundleError::Capacity("run event segment"));
        }
        let mut previous = None;
        for e in &v.events {
            if e.run_id != v.run_id
                || e.delta.view.run_id != v.run_id
                || e.sequence != e.delta.view.sequence
                || previous.is_some_and(|p: u64| p.checked_add(1) != Some(e.sequence))
            {
                return Err(BundleError::Corrupt("run event sequence".into()));
            }
            e.delta.view.validate()?;
            previous = Some(e.sequence);
        }
        Ok(())
    }
);
record!(
    RunEffectRecord,
    RunEffect,
    |v: &RunEffectRecord| {
        let mut r = v.payload_refs.clone();
        r.push(v.effect.record_ref.clone());
        r
    },
    |v: &RunEffectRecord, _: &StorageLimits| schema(v.version)
);
record!(
    RunRequestRecord,
    RunRequest,
    |v: &RunRequestRecord| match &v.request {
        RuntimeRequest::Question(r) | RuntimeRequest::Approval(r) | RuntimeRequest::Secret(r) =>
            vec![r.clone()],
    },
    |v: &RunRequestRecord, _: &StorageLimits| schema(v.version)
);
record!(
    RunDeliveryRecord,
    RunDelivery,
    |v: &RunDeliveryRecord| vec![v.delivery.result_ref.clone()],
    |v: &RunDeliveryRecord, _: &StorageLimits| schema(v.version)
);
record!(
    RunResultRecord,
    RunResult,
    |v: &RunResultRecord| {
        let mut r = v.artifacts.clone();
        r.extend(v.outcome.evidence_refs.clone());
        r.extend(v.base.clone());
        r.extend(v.head.clone());
        r.push(v.usage_ref.clone());
        r
    },
    |v: &RunResultRecord, _: &StorageLimits| {
        schema(v.version)?;
        kind(&v.usage_ref, RecordKind::RunUsage)
    }
);
record!(
    RunUsageRecord,
    RunUsage,
    |v: &RunUsageRecord| v.evidence_refs.clone(),
    |v: &RunUsageRecord, _: &StorageLimits| {
        schema(v.version)?;
        if v.event_id.is_empty() || v.event_id.len() > 256 || v.coverage.len() > 1024 {
            return Err(BundleError::Capacity("usage identity/coverage"));
        }
        Ok(())
    }
);

fn voice_references(v: &VoiceRef) -> Vec<ObjectRef> {
    let mut refs = vec![v.audio_ref.clone()];
    refs.extend(v.playhead_log_ref.clone());
    refs.extend(v.original_asr_ref.clone());
    refs.extend(v.chosen_transcript_revision.clone());
    refs
}
fn validate_voice(v: &VoiceRef, l: &StorageLimits) -> Result<(), BundleError> {
    kind(&v.audio_ref, RecordKind::OwnedEvidence)?;
    if v.duration_micros == 0
        || v.duration_micros > 120_000_000
        || v.audio_ref.byte_length > l.voice_bytes
        || v.range
            .as_ref()
            .is_some_and(|r| r.start_micros >= r.end_micros || r.end_micros > v.duration_micros)
    {
        return Err(BundleError::Capacity("voice duration/bytes/range"));
    }
    for r in voice_references(v) {
        kind(&r, RecordKind::OwnedEvidence)?;
    }
    Ok(())
}

pub(crate) struct ValidatedMetadataUpdate {
    pub(crate) versions: Option<ObjectRef>,
    pub(crate) feedback: Option<ObjectRef>,
    pub(crate) runs: Option<ObjectRef>,
    pub(crate) seals: Vec<SealedObject>,
}
impl ValidatedMetadataUpdate {
    pub(crate) fn apply(&self, root: &mut StudioRoot) {
        if let Some(r) = &self.versions {
            root.version_index_ref = Some(r.clone());
        }
        if let Some(r) = &self.feedback {
            root.feedback_index_ref = Some(r.clone());
        }
        if let Some(r) = &self.runs {
            root.run_index_ref = Some(r.clone());
        }
    }
}

pub struct PinnedSnapshot {
    pub descriptor: SnapshotDescriptor,
    pub pin: StorePin,
    pub roots: Vec<ObjectRef>,
    _charges: Vec<StorageReservation>,
}
pub struct PinnedFeedback {
    pub revision: FeedbackRevision,
    pub pin: StorePin,
    pub roots: Vec<ObjectRef>,
    charges: Vec<StorageReservation>,
}
pub struct PinnedSubmittedBatch {
    pub batch: FeedbackBatch,
    pub revisions: Vec<FeedbackRevision>,
    pub request: ReviseRequest,
    pub consumed_by: Option<RunId>,
    pub pin: StorePin,
    pub roots: Vec<ObjectRef>,
    _charges: Vec<StorageReservation>,
}

impl MetadataEnvelope {
    pub fn receipt_envelope(
        &self,
        update: &MetadataUpdate,
    ) -> Result<CommandEnvelope, BundleError> {
        bounded(update, StorageLimits::default().command_bytes)?;
        let bytes = serde_json::to_vec(update).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        Ok(CommandEnvelope {
            switch: None,
            manual: None,
            version: 1,
            action: "metadata".into(),
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            target_branch: self.target_branch.clone(),
            owner_epoch: self.owner_epoch,
            lease: None,
            expected_head: self.expected_head.clone(),
            expected_revision: kinewright_core::TimelineRevision(0),
            command: None,
            comment_text: None,
            comment_strokes: None,
            evidence: vec![ObjectRef {
                sha256: kinewright_media::sha256_bytes(&bytes),
                byte_length: bytes.len() as u64,
                record_kind: RecordKind::OwnedEvidence,
            }],
            transition: None,
        })
    }
}
impl ProjectBundle {
    fn read_index<T: BundleRecord + Default>(
        &self,
        reference: Option<&ObjectRef>,
    ) -> Result<StorageReservation<T>, BundleError> {
        reference.map_or_else(
            || Ok(self.store.reserve(0, 256, 0, false)?.hold(T::default())),
            |r| self.store.read_record(r),
        )
    }
    pub fn version_index(&self) -> Result<StorageReservation<VersionIndex>, BundleError> {
        self.read_index(
            self.root
                .as_ref()
                .and_then(|r| r.version_index_ref.as_ref()),
        )
    }
    pub fn feedback_index(&self) -> Result<StorageReservation<FeedbackIndex>, BundleError> {
        self.read_index(
            self.root
                .as_ref()
                .and_then(|r| r.feedback_index_ref.as_ref()),
        )
    }
    pub fn run_index(&self) -> Result<StorageReservation<RunIndex>, BundleError> {
        self.read_index(self.root.as_ref().and_then(|r| r.run_index_ref.as_ref()))
    }
    pub fn read_snapshot(&self, key: &SnapshotKey) -> Result<PinnedSnapshot, BundleError> {
        let index = self.version_index()?;
        let charges = vec![charge_value(&self.store, &*index)?];
        let descriptor = index
            .snapshots
            .get(&key.version)
            .filter(|v| v.key == *key)
            .cloned()
            .ok_or_else(|| {
                BundleError::Identity("snapshot is not indexed with this identity".into())
            })?;
        self.validate_snapshot(&descriptor)?;
        let roots = descriptor.references();
        let pin = self.store.pin(&roots)?;
        Ok(PinnedSnapshot {
            descriptor,
            pin,
            roots,
            _charges: charges,
        })
    }
    pub(crate) fn validate_snapshot(&self, d: &SnapshotDescriptor) -> Result<(), BundleError> {
        if d.key.project != self.project_id && !self.origin_project_ids.contains(&d.key.project) {
            return Err(BundleError::Identity("snapshot project".into()));
        }
        let c = self
            .store
            .read_record::<BranchCheckpoint>(&d.checkpoint_ref)?;
        if c.head_id != d.key.head
            || c.document_hash != d.key.document_hash
            || c.document_ref != d.document_ref
            || c.lineage_ref != d.lineage_ref
            || c.input_manifest_ref != d.input_manifest_ref
        {
            return Err(BundleError::Corrupt("snapshot checkpoint binding".into()));
        }
        Ok(())
    }
    pub fn read_feedback(&self, reference: &ObjectRef) -> Result<PinnedFeedback, BundleError> {
        if !self
            .feedback_index()?
            .revisions
            .values()
            .flatten()
            .any(|r| r == reference)
        {
            return Err(BundleError::Identity("feedback is not indexed".into()));
        }
        let original = self.store.read_record::<FeedbackRevision>(reference)?;
        let charges = vec![charge_value(&self.store, &*original)?];
        let revision = (*original).clone();
        let pin = self.store.pin(std::slice::from_ref(reference))?;
        Ok(PinnedFeedback {
            revision,
            pin,
            roots: vec![reference.clone()],
            charges,
        })
    }
    pub fn read_submitted_batch(
        &self,
        id: &FeedbackBatchId,
    ) -> Result<PinnedSubmittedBatch, BundleError> {
        let index = self.feedback_index()?;
        let b = index
            .batches
            .get(id)
            .ok_or_else(|| BundleError::Identity("batch is not submitted".into()))?;
        let batch = (*self.store.read_record::<FeedbackBatch>(&b.batch_ref)?).clone();
        if &batch.batch_id != id {
            return Err(BundleError::Corrupt("batch identity binding".into()));
        }
        let pin = self.store.pin(std::slice::from_ref(&b.batch_ref))?;
        let mut charges = vec![
            charge_value(&self.store, &batch)?,
            charge_value(&self.store, &*index)?,
        ];
        let mut revisions = Vec::new();
        for reference in &batch.revision_refs {
            let revision = self.read_feedback(reference)?;
            revisions.push(revision.revision);
            charges.extend(revision.charges);
        }
        Ok(PinnedSubmittedBatch {
            batch,
            revisions,
            request: b.request.clone(),
            consumed_by: b.consumed_by.clone(),
            pin,
            roots: vec![b.batch_ref.clone()],
            _charges: charges,
        })
    }
    pub fn admit_orchestration_record<T: BundleRecord>(
        &mut self,
        record: &T,
    ) -> Result<SealedObject, BundleError> {
        if !matches!(
            T::KIND,
            RecordKind::RunSnapshot
                | RecordKind::RunEventSegment
                | RecordKind::RunEffect
                | RecordKind::RunRequest
                | RecordKind::RunDelivery
                | RecordKind::RunResult
                | RecordKind::RunUsage
        ) {
            return Err(BundleError::Unsupported(
                "not an orchestration record".into(),
            ));
        }
        self.store_mut()?.admit_record(record)
    }
    pub fn metadata_command(
        &mut self,
        envelope: &MetadataEnvelope,
        scope: MetadataScope,
        update: &MetadataUpdate,
    ) -> Result<ReceiptLookup, BundleError> {
        if matches!(update.change, MetadataChange::SaveVoice(_)) {
            return crate::voice_store::metadata_command(self, envelope, scope, update);
        }
        self.metadata_command_inner(envelope, scope, update)
    }
    /// Only the locked note slot can authorize a voice publication, including `WriterHost` writes.
    pub(crate) fn voice_metadata_command(
        &mut self,
        envelope: &MetadataEnvelope,
        scope: MetadataScope,
        update: &MetadataUpdate,
        publication: crate::voice_store::VoicePublication<'_>,
    ) -> Result<ReceiptLookup, BundleError> {
        let MetadataChange::SaveVoice(voice) = &update.change else {
            return Err(BundleError::HistoryBarrierInvariant);
        };
        if publication.note() != &voice.note_id {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        self.metadata_command_inner(envelope, scope, update)
    }
    fn metadata_command_inner(
        &mut self,
        envelope: &MetadataEnvelope,
        scope: MetadataScope,
        update: &MetadataUpdate,
    ) -> Result<ReceiptLookup, BundleError> {
        let allowed = matches!(
            (scope, &update.change),
            (
                MetadataScope::Feedback,
                MetadataChange::SaveFeedback(_)
                    | MetadataChange::SaveVoice(_)
                    | MetadataChange::SubmitBatch { .. }
            ) | (MetadataScope::Snapshot, MetadataChange::PublishSnapshot(_))
                | (
                    MetadataScope::Orchestration,
                    MetadataChange::Orchestration { .. }
                        | MetadataChange::OrchestrationUpdate { .. }
                )
        );
        if !allowed || envelope.project_id != self.project_id {
            return Err(BundleError::Identity("metadata scope/project".into()));
        }
        bounded(update, self.store.limits().command_bytes)?;
        let command = envelope.receipt_envelope(update)?;
        let lookup = self.receipt_lookup(&command)?;
        if lookup != ReceiptLookup::Absent {
            return Ok(lookup);
        }
        if self.unsettled || self.pending_command.is_some() || self.pending_metadata.is_some() {
            return Err(BundleError::Unsettled);
        }
        if update.expected_root_generation != self.generation
            || envelope.target_branch != self.manual_branch_id
            || envelope.expected_head.as_ref() != self.manual_head()
            || envelope.owner_epoch != self.store.owner_epoch()
        {
            return Err(BundleError::HistoryChanged);
        }
        let issuer = self
            .issuers
            .get(&envelope.id.issuer)
            .filter(|i| !i.retired && i.next_sequence == envelope.id.sequence)
            .cloned()
            .ok_or_else(|| BundleError::Identity("metadata issuer sequence".into()))?;
        if self.store.limits().pending_commands == 0
            || self.receipts.detailed.len() as u64 + self.receipts.retired.len() as u64
                >= self.store.limits().receipts
        {
            return Err(BundleError::Capacity("metadata receipts/pending"));
        }
        let reservation = self.store.reserve(0, command.encoded_bytes()?, 0, false)?;
        let (validated, result) = self.validate_metadata(envelope, update)?;
        self.pending_metadata = Some(validated);
        self.pending_command = Some(reservation.hold(command.clone()));
        self.pending_result = Some(ReceiptResult::Metadata { result });
        self.issuers
            .get_mut(&envelope.id.issuer)
            .ok_or_else(|| BundleError::Identity("metadata issuer disappeared".into()))?
            .next_sequence = envelope
            .id
            .sequence
            .checked_add(1)
            .ok_or(BundleError::Capacity("metadata sequence overflow"))?;
        let publication = BundleCommit::publish_maintenance(self);
        if let Err(error) = publication {
            if !matches!(
                error,
                BundleError::DurabilityUnknown | BundleError::Unsettled
            ) && !self.unsettled
            {
                self.issuers.insert(envelope.id.issuer.clone(), issuer);
                self.pending_command = None;
                self.pending_result = None;
                self.pending_metadata = None;
            }
            return Err(error);
        }
        self.pending_command = None;
        self.pending_result = None;
        self.pending_metadata = None;
        self.receipt_lookup(&command)
    }
    #[allow(clippy::too_many_lines)]
    fn validate_metadata(
        &mut self,
        envelope: &MetadataEnvelope,
        update: &MetadataUpdate,
    ) -> Result<(ValidatedMetadataUpdate, MetadataResult), BundleError> {
        let mut pending = ValidatedMetadataUpdate {
            versions: None,
            feedback: None,
            runs: None,
            seals: vec![],
        };
        let original_feedback = self.feedback_index()?;
        let _feedback_charge = charge_value(&self.store, &*original_feedback)?;
        let _update_charge = charge_value(&self.store, update)?;
        let mut feedback = (*original_feedback).clone();
        let result = match &update.change {
            MetadataChange::OrchestrationUpdate {
                transition,
                consume_batch,
            } => {
                let (sealed, index_ref) = self.stage_orchestration_update(
                    envelope,
                    transition,
                    consume_batch.as_ref(),
                    &mut feedback,
                )?;
                pending.seals.extend(sealed);
                pending.runs = Some(index_ref);
                MetadataResult::Orchestration(vec![RunId(transition.projection().run.0.clone())])
            }
            MetadataChange::PublishSnapshot(p) => {
                let original_versions = self.version_index()?;
                let _version_charge = charge_value(&self.store, &*original_versions)?;
                let mut versions = (*original_versions).clone();
                if versions.snapshots.contains_key(&p.version)
                    || versions.snapshots.len() as u64 >= self.store.limits().versions
                {
                    return Err(BundleError::Capacity("Studio versions"));
                }
                let checkpoint = self
                    .store
                    .read_record::<BranchCheckpoint>(&p.checkpoint_ref)?;
                if checkpoint.head_id != p.expected_head
                    || self.checkpoint_reference(&checkpoint.branch_id)? != p.checkpoint_ref
                {
                    return Err(BundleError::HistoryChanged);
                }
                let key = SnapshotKey {
                    project: self.project_id.clone(),
                    version: p.version.clone(),
                    head: checkpoint.head_id.clone(),
                    document_hash: checkpoint.document_hash.clone(),
                    input_manifest_ref: checkpoint.input_manifest_ref.clone(),
                };
                let descriptor = SnapshotDescriptor {
                    key: key.clone(),
                    checkpoint_ref: p.checkpoint_ref.clone(),
                    document_ref: checkpoint.document_ref.clone(),
                    lineage_ref: checkpoint.lineage_ref.clone(),
                    input_manifest_ref: checkpoint.input_manifest_ref.clone(),
                    revision_refs: p.revision_refs.clone(),
                    parent_version: p.parent_version.clone(),
                    run_provenance: p.run_provenance.clone(),
                };
                self.validate_snapshot(&descriptor)?;
                for r in &p.revision_refs {
                    self.read_feedback(r)?;
                }
                versions.snapshots.insert(p.version.clone(), descriptor);
                let sealed = self.store.admit_record(&versions)?;
                pending.versions = Some(sealed.reference.clone());
                pending.seals.push(sealed);
                MetadataResult::Snapshot(key)
            }
            MetadataChange::SaveFeedback(revision) => {
                revision.validate(self.store.limits())?;
                self.read_snapshot(&revision.anchor.snapshot)?;
                let previous = feedback.revisions.get(&revision.comment_id);
                if previous.is_none()
                    && feedback.revisions.len() as u64 >= self.store.limits().comments
                {
                    return Err(BundleError::Capacity("Studio comments"));
                }
                if revision.revision != previous.map_or(1, |p| p.len() as u64 + 1)
                    || revision.predecessor.as_ref() != previous.and_then(|p| p.last())
                {
                    return Err(BundleError::HistoryChanged);
                }
                let text = self.store.read_evidence(&revision.original_text_ref)?;
                std::str::from_utf8(&text).map_err(|e| BundleError::Corrupt(e.to_string()))?;
                let strokes = self.store.read_evidence(&revision.strokes_ref)?;
                crate::receipts::validate_strokes(&strokes, self.store.limits().comment_points)?;
                if let Some(voice) = &revision.voice_ref
                    && feedback
                        .notes
                        .get(&voice.note_id)
                        .is_none_or(|saved| !voice.matches_note(saved))
                {
                    return Err(BundleError::Identity(
                        "voice must be saved before feedback".into(),
                    ));
                }
                let sealed = self.store.admit_record(revision)?;
                feedback
                    .revisions
                    .entry(revision.comment_id.clone())
                    .or_default()
                    .push(sealed.reference.clone());
                let result = MetadataResult::Feedback(sealed.reference.clone());
                pending.seals.push(sealed);
                result
            }
            MetadataChange::SaveVoice(voice) => {
                validate_voice(voice, self.store.limits())?;
                if let Some(previous) = feedback.notes.get(&voice.note_id)
                    && (previous.audio_ref != voice.audio_ref
                        || previous.duration_micros != voice.duration_micros
                        || previous.playhead_log_ref != voice.playhead_log_ref
                        || previous
                            .original_asr_ref
                            .as_ref()
                            .is_some_and(|r| Some(r) != voice.original_asr_ref.as_ref()))
                {
                    return Err(BundleError::Identity(
                        "original voice evidence changed".into(),
                    ));
                }
                feedback.notes.insert(voice.note_id.clone(), voice.clone());
                MetadataResult::Voice(voice.note_id.clone())
            }
            MetadataChange::SubmitBatch { batch, request } => {
                batch.validate(self.store.limits())?;
                if batch.batch_id != request.batch_id
                    || envelope.id != request.command_id
                    || feedback.batches.contains_key(&batch.batch_id)
                {
                    return Err(BundleError::Identity(
                        "immutable batch/request identity".into(),
                    ));
                }
                for r in &batch.revision_refs {
                    self.read_feedback(r)?;
                }
                let sealed = self.store.admit_record(batch)?;
                feedback.batches.insert(
                    batch.batch_id.clone(),
                    SubmittedBatch {
                        batch_ref: sealed.reference.clone(),
                        request: request.clone(),
                        consumed_by: None,
                    },
                );
                pending.seals.push(sealed);
                MetadataResult::Batch(batch.batch_id.clone())
            }
            MetadataChange::Orchestration {
                runs,
                consume_batch,
            } => {
                let original_runs = self.run_index()?;
                let _run_charge = charge_value(&self.store, &*original_runs)?;
                let mut index = (*original_runs).clone();
                if runs.is_empty()
                    || runs.len() as u64 > self.store.limits().versions
                    || runs
                        .iter()
                        .map(|r| &r.run_id)
                        .collect::<BTreeSet<_>>()
                        .len()
                        != runs.len()
                {
                    return Err(BundleError::Capacity("run update entries"));
                }
                for run in runs {
                    if let Some(previous) = index.runs.get(&run.run_id) {
                        if run.projection.sequence <= previous.projection.sequence {
                            return Err(BundleError::HistoryChanged);
                        }
                        let old: BTreeSet<_> = previous.references().into_iter().collect();
                        let new: BTreeSet<_> = run.references().into_iter().collect();
                        if !old.is_subset(&new) {
                            return Err(BundleError::Identity(
                                "orchestration evidence removal".into(),
                            ));
                        }
                    }
                    self.validate_run_bindings(run)?;
                    index.runs.insert(run.run_id.clone(), run.clone());
                }
                if let Some((batch, run)) = consume_batch {
                    let submitted = feedback
                        .batches
                        .get_mut(batch)
                        .ok_or_else(|| BundleError::Identity("unsubmitted run batch".into()))?;
                    if !runs.iter().any(|r| &r.run_id == run)
                        || submitted.consumed_by.as_ref().is_some_and(|old| old != run)
                    {
                        return Err(BundleError::Identity(
                            "batch already consumed/unknown run".into(),
                        ));
                    }
                    submitted.consumed_by = Some(run.clone());
                }
                let sealed = self.store.admit_record(&index)?;
                pending.runs = Some(sealed.reference.clone());
                pending.seals.push(sealed);
                MetadataResult::Orchestration(runs.iter().map(|r| r.run_id.clone()).collect())
            }
        };
        if !matches!(update.change, MetadataChange::PublishSnapshot(_)) {
            let sealed = self.store.admit_record(&feedback)?;
            pending.feedback = Some(sealed.reference.clone());
            pending.seals.push(sealed);
        }
        Ok((pending, result))
    }
    fn validate_run_bindings(&self, run: &RunRecord) -> Result<(), BundleError> {
        macro_rules! check {
            ($refs:expr, $ty:ty, $id:expr) => {
                for r in $refs {
                    let record = self.store.read_record::<$ty>(r)?;
                    if $id(&*record) != run.run_id {
                        return Err(BundleError::Identity(
                            "orchestration record run binding".into(),
                        ));
                    }
                }
            };
        }
        check!(
            &run.snapshot_refs,
            RunSnapshotRecord,
            |v: &RunSnapshotRecord| v.view.run_id.clone()
        );
        check!(
            &run.event_segment_refs,
            RunEventSegment,
            |v: &RunEventSegment| v.run_id.clone()
        );
        check!(&run.effect_refs, RunEffectRecord, |v: &RunEffectRecord| v
            .run_id
            .clone());
        check!(
            &run.request_refs,
            RunRequestRecord,
            |v: &RunRequestRecord| v.run_id.clone()
        );
        check!(
            &run.delivery_refs,
            RunDeliveryRecord,
            |v: &RunDeliveryRecord| v.run_id.clone()
        );
        check!(&run.result_refs, RunResultRecord, |v: &RunResultRecord| v
            .run_id
            .clone());
        check!(&run.usage_refs, RunUsageRecord, |v: &RunUsageRecord| v
            .run_id
            .clone());
        Ok(())
    }
}

pub(crate) fn add_required_apis(
    apis: &mut std::collections::BTreeMap<String, u32>,
    kinds: impl Iterator<Item = RecordKind>,
) {
    for kind in kinds {
        if matches!(
            kind,
            RecordKind::VersionIndex
                | RecordKind::FeedbackIndex
                | RecordKind::FeedbackRevision
                | RecordKind::FeedbackBatch
        ) {
            apis.insert("studio".into(), 1);
        }
        if matches!(
            kind,
            RecordKind::RunIndex
                | RecordKind::RunSnapshot
                | RecordKind::RunEventSegment
                | RecordKind::RunEffect
                | RecordKind::RunRequest
                | RecordKind::RunDelivery
                | RecordKind::RunResult
                | RecordKind::RunUsage
        ) {
            apis.insert("orchestration".into(), 1);
        }
    }
}

impl ProjectBundle {
    /// Issuer publication uses the durable checkpoint and cannot save pending manual edits.
    pub fn issue_metadata_issuer(&mut self) -> Result<crate::receipts::IssuerId, BundleError> {
        self.issue_metadata_issuer_bound(None)
    }
    pub(crate) fn issue_metadata_issuer_bound(
        &mut self,
        subject: Option<String>,
    ) -> Result<crate::receipts::IssuerId, BundleError> {
        if self.unsettled || self.pending_command.is_some() || self.pending_metadata.is_some() {
            return Err(BundleError::Unsettled);
        }
        if self.issuers.len() as u64 >= self.store.limits().issuers {
            return Err(BundleError::Capacity("metadata issuers"));
        }
        let id = crate::receipts::IssuerId::fresh()?;
        self.issuers.insert(
            id.clone(),
            crate::receipts::IssuerState {
                next_sequence: 1,
                reconciled_high_water: 0,
                retired: false,
                metadata_subject: subject,
            },
        );
        if let Err(error) = BundleCommit::publish_maintenance(self) {
            if !self.unsettled {
                self.issuers.remove(&id);
            }
            return Err(error);
        }
        Ok(id)
    }
    pub fn reconcile_metadata(&mut self) -> Result<(), BundleError> {
        self.reconcile_destination(None)?;
        self.pending_metadata = None;
        self.pending_command = None;
        self.pending_result = None;
        Ok(())
    }
}

impl crate::editing_session::Ja1EditingSession {
    pub fn metadata_command(
        &mut self,
        envelope: &MetadataEnvelope,
        scope: MetadataScope,
        update: &MetadataUpdate,
    ) -> Result<ReceiptLookup, BundleError> {
        self.active_bundle_mut()?
            .metadata_command(envelope, scope, update)
    }
    pub fn issue_metadata_issuer(&mut self) -> Result<crate::receipts::IssuerId, BundleError> {
        self.active_bundle_mut()?.issue_metadata_issuer()
    }
    pub fn issue_metadata_issuer_for(
        &mut self,
        subject: &str,
    ) -> Result<crate::receipts::IssuerId, BundleError> {
        if subject.is_empty() || subject.len() > 512 {
            return Err(BundleError::Capacity("metadata subject"));
        }
        self.active_bundle_mut()?
            .issue_metadata_issuer_bound(Some(subject.to_owned()))
    }
    pub fn metadata_issuer_subject(
        &self,
        issuer: &crate::receipts::IssuerId,
    ) -> Result<Option<String>, BundleError> {
        Ok(self
            .active_bundle()?
            .issuers
            .get(issuer)
            .and_then(|state| state.metadata_subject.clone()))
    }
    pub fn metadata_issuer_sequence(
        &self,
        issuer: &crate::receipts::IssuerId,
    ) -> Result<u64, BundleError> {
        self.active_bundle()?
            .issuers
            .get(issuer)
            .filter(|state| !state.retired)
            .map(|state| state.next_sequence)
            .ok_or_else(|| BundleError::Identity("metadata issuer retired or absent".into()))
    }
    pub fn metadata_envelope(
        &self,
        id: crate::receipts::CommandId,
    ) -> Result<MetadataEnvelope, BundleError> {
        let bundle = self.active_bundle()?;
        Ok(MetadataEnvelope {
            id,
            project_id: bundle.project_id.clone(),
            target_branch: bundle.manual_branch_id.clone(),
            owner_epoch: bundle.store.owner_epoch(),
            expected_head: bundle.manual_head().cloned(),
        })
    }
    pub fn metadata_generation(&self) -> Result<u64, BundleError> {
        Ok(self.active_bundle()?.generation)
    }
    pub fn reconcile_metadata(&mut self) -> Result<(), BundleError> {
        self.active_bundle_mut()?.reconcile_metadata()
    }
    pub fn read_snapshot(&self, key: &SnapshotKey) -> Result<PinnedSnapshot, BundleError> {
        self.active_bundle()?.read_snapshot(key)
    }
    pub fn read_feedback(&self, reference: &ObjectRef) -> Result<PinnedFeedback, BundleError> {
        self.active_bundle()?.read_feedback(reference)
    }
    pub fn read_submitted_batch(
        &self,
        batch: &FeedbackBatchId,
    ) -> Result<PinnedSubmittedBatch, BundleError> {
        self.active_bundle()?.read_submitted_batch(batch)
    }
}

pub struct FrozenContext {
    pub snapshot: PinnedSnapshot,
    pub document: kinewright_core::Document,
    pub certified_input_leases: Vec<crate::owned_inputs::OwnedInputLease>,
    pub resource_context: Vec<crate::owned_inputs::CertifiedResource>,
    _document_charge: StorageReservation,
    reserve_frame: std::sync::Arc<kinewright_media::owned_io::CursorReservation>,
}
impl ProjectBundle {
    pub fn frozen_context(&self, key: &SnapshotKey) -> Result<FrozenContext, BundleError> {
        let snapshot = self.read_snapshot(key)?;
        self.build_frozen_context(snapshot)
    }
    pub fn frozen_context_descriptor(
        &self,
        descriptor: &SnapshotDescriptor,
    ) -> Result<FrozenContext, BundleError> {
        self.validate_snapshot(descriptor)?;
        let roots = descriptor.references();
        let pin = self.store.pin(&roots)?;
        let charges = vec![charge_value(&self.store, descriptor)?];
        self.build_frozen_context(PinnedSnapshot {
            descriptor: descriptor.clone(),
            pin,
            roots,
            _charges: charges,
        })
    }
    fn build_frozen_context(&self, snapshot: PinnedSnapshot) -> Result<FrozenContext, BundleError> {
        let original_document = self
            .store
            .read_record::<kinewright_core::Document>(&snapshot.descriptor.document_ref)?;
        let document_charge = charge_value(&self.store, &*original_document)?;
        let document = (*original_document).clone();
        let manifest = self
            .store
            .read_input_manifest(&snapshot.descriptor.input_manifest_ref)?;
        crate::bundle_commit::validate_document_inputs(&document, &manifest)?;
        let mut certified_input_leases = Vec::new();
        let mut resource_context = Vec::new();
        for entry in &manifest.entries {
            let identity = crate::owned_inputs::OwnedInputRef {
                manifest_ref: snapshot.descriptor.input_manifest_ref.clone(),
                entry: entry.clone(),
            };
            if matches!(
                entry.object.record_kind,
                RecordKind::OwnedFont | RecordKind::OwnedLut
            ) {
                resource_context.push(self.store.certify_resource_input(&identity)?);
            } else {
                certified_input_leases.push(self.store.lease_input_for_owner(
                    &identity,
                    &(manifest.project_id.clone(), manifest.session_id.clone()),
                )?);
            }
        }
        Ok(FrozenContext {
            snapshot,
            document,
            certified_input_leases,
            resource_context,
            _document_charge: document_charge,
            reserve_frame: self.store.decoder_reserver(),
        })
    }
}

/// A receiver loses control submission on a gap until a full snapshot is installed.
/// This is presentation state. S4 still authorizes every command at the owner.
#[derive(Default)]
pub struct RunProjectionCursor {
    pub view: Option<RunView>,
    pub needs_snapshot: bool,
}
impl RunProjectionCursor {
    pub fn refresh(&mut self, view: RunView) -> Result<(), BundleError> {
        view.validate()?;
        self.view = Some(view);
        self.needs_snapshot = false;
        Ok(())
    }
    pub fn receive(&mut self, event: RunEvent) -> Result<(), BundleError> {
        if self.needs_snapshot
            || self.view.as_ref().is_none_or(|v| {
                v.run_id != event.run_id || v.sequence.checked_add(1) != Some(event.sequence)
            })
        {
            self.needs_snapshot = true;
            return Err(BundleError::HistoryChanged);
        }
        if event.delta.view.sequence != event.sequence || event.delta.view.run_id != event.run_id {
            self.needs_snapshot = true;
            return Err(BundleError::Corrupt("run event projection binding".into()));
        }
        if let Err(error) = event.delta.view.validate() {
            self.needs_snapshot = true;
            return Err(error);
        }
        self.view = Some(event.delta.view);
        Ok(())
    }
    #[must_use]
    pub fn can_submit(&self, intent: &RunIntent) -> bool {
        !self.needs_snapshot
            && self.view.as_ref().is_some_and(|v| {
                let kind = match intent.action {
                    RunAction::Pause => RunActionKind::Pause,
                    RunAction::Stop => RunActionKind::Stop,
                    RunAction::Continue => RunActionKind::Continue,
                    RunAction::SendMessage { .. } => RunActionKind::SendMessage,
                };
                v.run_id == intent.run_id
                    && v.control_revision == intent.expected_control_revision
                    && !(kind == RunActionKind::Continue
                        && matches!(v.cleanup, RunCleanup::Blocked { .. }))
                    && v.allowed_actions.contains(&kind)
            })
    }
}

/// Only owner validation can construct this. A later root change still conflicts at commit.
pub struct ValidatedRunUpdate {
    envelope: MetadataEnvelope,
    update: MetadataUpdate,
    _admitted: Option<ValidatedMetadataUpdate>,
    _request_charge: StorageReservation,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationReceipt {
    pub receipt: crate::receipts::CommandReceipt,
    pub run_ids: Vec<RunId>,
    pub consumed_batch: Option<(FeedbackBatchId, RunId)>,
}
impl ProjectBundle {
    pub fn validate_run_update(
        &mut self,
        envelope: &MetadataEnvelope,
        update: &MetadataUpdate,
    ) -> Result<ValidatedRunUpdate, BundleError> {
        if envelope.project_id != self.project_id
            || !matches!(update.change, MetadataChange::Orchestration { .. })
        {
            return Err(BundleError::Identity(
                "orchestration validation scope".into(),
            ));
        }
        let command = envelope.receipt_envelope(update)?;
        let lookup = self.receipt_lookup(&command)?;
        let admitted = if lookup == ReceiptLookup::Absent {
            if self.unsettled || self.pending_command.is_some() || self.pending_metadata.is_some() {
                return Err(BundleError::Unsettled);
            }
            if update.expected_root_generation != self.generation
                || envelope.target_branch != self.manual_branch_id
                || envelope.expected_head.as_ref() != self.manual_head()
                || envelope.owner_epoch != self.store.owner_epoch()
            {
                return Err(BundleError::HistoryChanged);
            }
            Some(self.validate_metadata(envelope, update)?.0)
        } else if matches!(
            lookup,
            ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_)
        ) {
            None
        } else {
            return Err(BundleError::Identity(
                "orchestration receipt is not committed".into(),
            ));
        };
        Ok(ValidatedRunUpdate {
            envelope: envelope.clone(),
            update: update.clone(),
            _admitted: admitted,
            _request_charge: charge_value(&self.store, update)?,
        })
    }
    pub fn commit_orchestration(
        &mut self,
        envelope: &MetadataEnvelope,
        transition: ValidatedRunUpdate,
    ) -> Result<OrchestrationReceipt, BundleError> {
        if envelope != &transition.envelope {
            return Err(BundleError::Identity(
                "validated orchestration envelope changed".into(),
            ));
        }
        let MetadataChange::Orchestration {
            runs,
            consume_batch,
        } = &transition.update.change
        else {
            return Err(BundleError::Identity(
                "validated orchestration domain changed".into(),
            ));
        };
        let lookup =
            self.metadata_command(envelope, MetadataScope::Orchestration, &transition.update)?;
        let (ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) = lookup else {
            return Err(BundleError::Unsettled);
        };
        if receipt.id != envelope.id
            || receipt.digest != envelope.receipt_envelope(&transition.update)?.digest()?
            || receipt.project_id != envelope.project_id
            || receipt.target_branch != envelope.target_branch
        {
            return Err(BundleError::Identity(
                "orchestration returned receipt scope".into(),
            ));
        }
        let result = OrchestrationReceipt {
            receipt,
            run_ids: runs.iter().map(|r| r.run_id.clone()).collect(),
            consumed_batch: consume_batch.clone(),
        };
        drop(transition);
        Ok(result)
    }
}
impl crate::editing_session::Ja1EditingSession {
    pub fn validate_run_update(
        &mut self,
        envelope: &MetadataEnvelope,
        update: &MetadataUpdate,
    ) -> Result<ValidatedRunUpdate, BundleError> {
        self.active_bundle_mut()?
            .validate_run_update(envelope, update)
    }
    pub fn commit_orchestration(
        &mut self,
        envelope: &MetadataEnvelope,
        transition: ValidatedRunUpdate,
    ) -> Result<OrchestrationReceipt, BundleError> {
        self.active_bundle_mut()?
            .commit_orchestration(envelope, transition)
    }
}
pub fn commit_orchestration(
    owner: &mut crate::editing_session::Ja1EditingSession,
    envelope: &MetadataEnvelope,
    transition: ValidatedRunUpdate,
) -> Result<OrchestrationReceipt, BundleError> {
    owner.commit_orchestration(envelope, transition)
}

fn charge_value<T: serde::Serialize>(
    store: &crate::store::ProjectStore,
    value: &T,
) -> Result<StorageReservation, BundleError> {
    bounded(value, store.limits().decoded_record_bytes)?;
    let _scratch = store.reserve(0, store.limits().decoded_record_bytes, 0, false)?;
    let bytes = serde_json::to_vec(value).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    let decoded = crate::store::preflight_record(&bytes, store.limits())?;
    store.reserve(0, decoded, 0, false)
}

impl crate::editing_session::Ja1EditingSession {
    pub fn frozen_context(&self, key: &SnapshotKey) -> Result<FrozenContext, BundleError> {
        self.active_bundle()?.frozen_context(key)
    }
}

impl FrozenContext {
    /// Check retained native descriptors, failure latches and one verified chunk
    /// before releasing an originating attachment. Later chunks remain verified
    /// by the existing cursor on demand; this is not a new full-media scan.
    pub fn verify_retained_inputs(&self) -> Result<(), BundleError> {
        for lease in &self.certified_input_leases {
            let source = lease.certified_source()?.playback_source();
            let mut cursor = source
                .cursor(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                    false,
                )))
                .map_err(|error| BundleError::Corrupt(error.to_string()))?;
            if cursor
                .read(&mut [0])
                .map_err(|error| BundleError::Corrupt(error.to_string()))?
                != 1
            {
                return Err(BundleError::Corrupt(
                    "retained certified input is empty".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn reserve_frame(
        &self,
        bytes: u64,
    ) -> Result<kinewright_media::owned_io::LifetimeGuard, BundleError> {
        (self.reserve_frame)(bytes).map_err(BundleError::Corrupt)
    }
    /// Build the renderer scene without consulting a media or LUT locator.
    /// The scene itself retains this context through decoder and paint consumers.
    pub fn playback_inputs(
        self: &std::sync::Arc<Self>,
    ) -> Result<std::sync::Arc<kinewright_media::owned_io::PlaybackInputs>, BundleError> {
        use std::{
            collections::{BTreeMap, HashMap},
            sync::Arc,
        };
        let encoded = serde_json::to_vec(&self.document)
            .map_err(|error| BundleError::Corrupt(error.to_string()))?;
        if kinewright_media::sha256_bytes(&encoded) != self.snapshot.descriptor.key.document_hash
            || self.snapshot.descriptor.document_ref.sha256
                != self.snapshot.descriptor.key.document_hash
            || self.certified_input_leases.iter().any(|lease| {
                lease.identity().manifest_ref != self.snapshot.descriptor.input_manifest_ref
            })
            || self.resource_context.iter().any(|resource| {
                resource.identity().manifest_ref != self.snapshot.descriptor.input_manifest_ref
            })
        {
            return Err(BundleError::Identity(
                "frozen document/context binding".into(),
            ));
        }
        let mut sources = BTreeMap::new();
        for asset in &self.document.media_pool {
            let lease = self
                .certified_input_leases
                .iter()
                .find(|lease| lease.identity().entry.matches_media_asset(asset))
                .ok_or_else(|| BundleError::Identity("frozen media interpretation".into()))?;
            sources.insert(asset.id, lease.certified_source()?.playback_source());
        }
        let mut luts = HashMap::new();
        for resource in &self.resource_context {
            if resource.identity().entry.object.record_kind == RecordKind::OwnedLut {
                let lut = kinewright_media::parse_cube_lut_bytes(resource.bytes())
                    .map_err(|error| BundleError::Corrupt(error.to_string()))?;
                luts.insert(
                    resource.identity().entry.object.sha256.clone(),
                    Arc::new(lut),
                );
            }
        }
        let (library, missing) =
            kinewright_media::LutLibrary::from_document_assets(&self.document.lut_assets, &luts);
        if !missing.is_empty() {
            return Err(BundleError::Identity("frozen LUT resources missing".into()));
        }
        kinewright_media::owned_io::PlaybackInputs::admit(
            Arc::new(self.document.clone()),
            sources,
            Arc::new(library),
            Arc::clone(self) as kinewright_media::owned_io::LifetimeGuard,
        )
        .map(Arc::new)
        .map_err(|error| BundleError::Corrupt(error.to_string()))
    }
}
impl crate::editing_session::Ja1EditingSession {
    pub fn frozen_context_descriptor(
        &self,
        descriptor: &SnapshotDescriptor,
    ) -> Result<FrozenContext, BundleError> {
        self.active_bundle()?.frozen_context_descriptor(descriptor)
    }
    pub fn studio_versions(&self) -> Result<Vec<SnapshotKey>, BundleError> {
        Ok(self
            .active_bundle()?
            .version_index()?
            .snapshots
            .values()
            .map(|descriptor| descriptor.key.clone())
            .collect())
    }
}

impl crate::store::ProjectStore {
    /// Read-only playback admission for an accepted manual view. This lends
    /// this owner's existing namespace and ledger; it publishes no snapshot.
    pub fn manual_playback_inputs(
        &self,
        document: std::sync::Arc<kinewright_core::Document>,
        inputs: &ObjectRef,
    ) -> Result<std::sync::Arc<kinewright_media::owned_io::PlaybackInputs>, BundleError> {
        use std::{
            collections::{BTreeMap, HashMap},
            sync::Arc,
        };
        let manifest = self.read_input_manifest(inputs)?;
        crate::bundle_commit::validate_document_inputs(&document, &manifest)?;
        let pin = self.pin(std::slice::from_ref(inputs))?;
        let document_charge = charge_value(self, &*document)?;
        let mut leases = Vec::new();
        let mut resources = Vec::new();
        let mut sources = BTreeMap::new();
        let mut luts = HashMap::new();
        for entry in &manifest.entries {
            let identity = crate::owned_inputs::OwnedInputRef {
                manifest_ref: inputs.clone(),
                entry: entry.clone(),
            };
            if matches!(
                entry.object.record_kind,
                RecordKind::OwnedLut | RecordKind::OwnedFont
            ) {
                let resource = self.certify_resource_input(&identity)?;
                if entry.object.record_kind == RecordKind::OwnedLut {
                    let lut = kinewright_media::parse_cube_lut_bytes(resource.bytes())
                        .map_err(|error| BundleError::Corrupt(error.to_string()))?;
                    luts.insert(entry.object.sha256.clone(), Arc::new(lut));
                }
                resources.push(resource);
            } else {
                leases.push(self.lease_input_for_owner(
                    &identity,
                    &(manifest.project_id.clone(), manifest.session_id.clone()),
                )?);
            }
        }
        for asset in &document.media_pool {
            let lease = leases
                .iter()
                .find(|lease| lease.identity().entry.matches_media_asset(asset))
                .ok_or_else(|| BundleError::Identity("manual playback interpretation".into()))?;
            sources.insert(asset.id, lease.certified_source()?.playback_source());
        }
        let (library, missing) =
            kinewright_media::LutLibrary::from_document_assets(&document.lut_assets, &luts);
        if !missing.is_empty() {
            return Err(BundleError::Identity(
                "manual playback LUT resources".into(),
            ));
        }
        let lifetime = Arc::new((pin, document_charge, manifest, leases, resources));
        kinewright_media::owned_io::PlaybackInputs::admit(
            document,
            sources,
            Arc::new(library),
            lifetime,
        )
        .map(Arc::new)
        .map_err(|error| BundleError::Corrupt(error.to_string()))
    }
}
