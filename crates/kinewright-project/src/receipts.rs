//! Durable command results. Lookup precedes fresh authority and never executes a command.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{BatchId, BranchId, HeadId, ObjectRef, ProjectId, RecordKind, SessionId},
    storage_budget::StorageLimits,
    store::BundleRecord,
};
use kinewright_core::{Core, HistorySnapshot, JournalCommand, TimelineRevision};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IssuerId(pub(crate) String);
impl IssuerId {
    fn fresh() -> Result<Self, BundleError> {
        Ok(Self(SessionId::fresh()?.as_str().to_owned()))
    }
    fn validate(&self) -> Result<(), BundleError> {
        if !hex(&self.0, 32) {
            return Err(BundleError::Corrupt("invalid issuer identity".into()));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandId {
    pub issuer: IssuerId,
    pub sequence: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandDigest(pub(crate) String);
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A transition is admitted only at its original acknowledged checkpoint and cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryRequest {
    Floor {
        discard_batches: usize,
        preceding_checkpoint_generation: u64,
        preceding_floor_generation: u64,
        command_cut: CommandId,
    },
    RecoveryClose {
        preceding_checkpoint_generation: u64,
        reconciled_cut: CommandId,
        suffix_digest: String,
    },
}

/// Transport tokens are deliberately absent. A retry preserves every original field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEnvelope {
    pub version: u32,
    pub action: String,
    pub id: CommandId,
    pub project_id: ProjectId,
    pub target_branch: BranchId,
    pub owner_epoch: u64,
    pub lease: Option<String>,
    pub expected_head: Option<HeadId>,
    pub expected_revision: TimelineRevision,
    pub command: Option<JournalCommand>,
    pub comment_text: Option<ObjectRef>,
    pub comment_strokes: Option<ObjectRef>,
    pub evidence: Vec<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<HistoryRequest>,
}
impl CommandEnvelope {
    pub fn encoded_bytes(&self) -> Result<u64, BundleError> {
        self.id.issuer.validate()?;
        if self.version != 1
            || self.id.sequence == 0
            || self.lease.as_ref().is_some_and(|v| v.len() > 256)
        {
            return Err(BundleError::Unsupported(
                "command envelope version/identity".into(),
            ));
        }
        match self.action.as_str() {
            "edit"
                if self.transition.is_none()
                    && self.command.is_some()
                    && self.comment_text.is_none()
                    && self.comment_strokes.is_none()
                    && self.evidence.is_empty() => {}
            "comment"
                if self.transition.is_none()
                    && self.command.is_none()
                    && self.comment_text.is_some()
                    && self.comment_strokes.is_some() => {}
            "floor"
                if matches!(self.transition, Some(HistoryRequest::Floor { .. }))
                    && self.command.is_none()
                    && self.comment_text.is_none()
                    && self.comment_strokes.is_none()
                    && self.evidence.is_empty() => {}
            "recovery_close"
                if matches!(self.transition, Some(HistoryRequest::RecoveryClose { .. }))
                    && self.command.is_none()
                    && self.comment_text.is_none()
                    && self.comment_strokes.is_none()
                    && self.evidence.is_empty() => {}
            _ => {
                return Err(BundleError::Unsupported(
                    "command action/payload, including branch switch".into(),
                ));
            }
        }
        let mut counter = crate::store::BoundedCounter {
            bytes: 0,
            limit: StorageLimits::default().command_bytes,
        };
        serde_json::to_writer(&mut counter, self)
            .map_err(|_| BundleError::Capacity("command bytes"))?;
        Ok(counter.bytes)
    }
    pub fn digest(&self) -> Result<CommandDigest, BundleError> {
        self.encoded_bytes()?;
        let bytes = serde_json::to_vec(self).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        Ok(CommandDigest(kinewright_media::sha256_bytes(&bytes)))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptResult {
    Edit {
        checkpoint_ref: ObjectRef,
        batch_id: Option<BatchId>,
    },
    Comment {
        record_id: String,
        record_ref: ObjectRef,
        text: ObjectRef,
        strokes: ObjectRef,
        evidence: Vec<ObjectRef>,
    },
}
impl ReceiptResult {
    fn dependencies(&self) -> Vec<ObjectRef> {
        match self {
            Self::Edit { checkpoint_ref, .. } => vec![checkpoint_ref.clone()],
            Self::Comment {
                record_ref,
                text,
                strokes,
                evidence,
                ..
            } => {
                let mut refs = vec![record_ref.clone(), text.clone(), strokes.clone()];
                refs.extend(evidence.clone());
                refs
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    pub id: CommandId,
    pub digest: CommandDigest,
    pub project_id: ProjectId,
    pub target_branch: BranchId,
    pub original_epoch: u64,
    pub expected_head: Option<HeadId>,
    pub head_id: HeadId,
    pub generation: u64,
    pub result: ReceiptResult,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptLookup {
    Absent,
    Pending,
    Committed(CommandReceipt),
    Retired(CommandReceipt),
    RetiredUnresolved,
    IdPayloadConflict,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuerState {
    pub next_sequence: u64,
    pub reconciled_high_water: u64,
    pub retired: bool,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptLedger {
    pub detailed: Vec<CommandReceipt>,
    pub retired: Vec<CommandReceipt>,
    pub comments: u64,
}
impl ReceiptLedger {
    #[must_use]
    pub fn lookup(&self, id: &CommandId, digest: &CommandDigest) -> ReceiptLookup {
        for (records, retired) in [(&self.detailed, false), (&self.retired, true)] {
            if let Some(receipt) = records.iter().find(|r| r.id == *id) {
                return if receipt.digest != *digest {
                    ReceiptLookup::IdPayloadConflict
                } else if retired {
                    ReceiptLookup::Retired(receipt.clone())
                } else {
                    ReceiptLookup::Committed(receipt.clone())
                };
            }
        }
        ReceiptLookup::Absent
    }
    pub(crate) fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        if (self.detailed.len() as u64)
            .checked_add(self.retired.len() as u64)
            .is_none_or(|n| n > limits.receipts)
            || self.comments > limits.comments
        {
            return Err(BundleError::Capacity("receipt/comment entries"));
        }
        let mut ids = BTreeSet::new();
        for receipt in self.detailed.iter().chain(&self.retired) {
            receipt.id.issuer.validate()?;
            if receipt.id.sequence == 0
                || receipt.generation == 0
                || !hex(&receipt.digest.0, 64)
                || !ids.insert(&receipt.id)
            {
                return Err(BundleError::Corrupt("receipt identity/digest".into()));
            }
            for reference in receipt.result.dependencies() {
                reference.validate()?;
            }
            match &receipt.result {
                ReceiptResult::Edit { checkpoint_ref, .. }
                    if checkpoint_ref.record_kind != RecordKind::BranchCheckpoint =>
                {
                    return Err(BundleError::Corrupt("edit checkpoint kind".into()));
                }
                ReceiptResult::Comment {
                    record_id,
                    record_ref,
                    text,
                    strokes,
                    ..
                } if !record_id.starts_with("comment-")
                    || record_id.len() != 40
                    || [record_ref, text, strokes]
                        .iter()
                        .any(|r| r.record_kind != RecordKind::OwnedEvidence) =>
                {
                    return Err(BundleError::Corrupt("comment result identity/kind".into()));
                }
                _ => {}
            }
        }
        Ok(())
    }
}
impl crate::store::record_sealed::Sealed for ReceiptLedger {}
impl BundleRecord for ReceiptLedger {
    const KIND: RecordKind = RecordKind::ReceiptIndex;
    fn dependencies(&self) -> Vec<ObjectRef> {
        self.detailed
            .iter()
            .chain(&self.retired)
            .flat_map(|r| r.result.dependencies())
            .collect()
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        self.validate(limits)
    }
}
impl crate::store::record_sealed::Sealed for BTreeMap<IssuerId, IssuerState> {}
impl BundleRecord for BTreeMap<IssuerId, IssuerState> {
    const KIND: RecordKind = RecordKind::IssuerState;
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        if self
            .values()
            .filter(|state| {
                !state.retired
                    || state.reconciled_high_water.checked_add(1) != Some(state.next_sequence)
            })
            .count() as u64
            > limits.issuers
            || self.len() as u64
                > limits
                    .issuers
                    .checked_add(limits.receipts)
                    .ok_or(BundleError::Capacity("issuer index overflow"))?
        {
            return Err(BundleError::Capacity("issuer entries"));
        }
        for (id, state) in self {
            id.validate()?;
            if state.next_sequence == 0 || state.reconciled_high_water >= state.next_sequence {
                return Err(BundleError::Corrupt("issuer sequence/high water".into()));
            }
        }
        Ok(())
    }
}

impl ProjectBundle {
    /// Owner issuance is durable before the ID is returned. A fixture may start at its original action ID.
    pub fn issue_issuer(
        &mut self,
        core: &Core,
        inputs: &ObjectRef,
        first_sequence: u64,
    ) -> Result<IssuerId, BundleError> {
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        if first_sequence == 0
            || self
                .issuers
                .values()
                .filter(|state| {
                    !state.retired
                        || state.reconciled_high_water.checked_add(1) != Some(state.next_sequence)
                })
                .count() as u64
                >= self.store.limits().issuers
        {
            return Err(BundleError::Capacity("issuer entries"));
        }
        let id = IssuerId::fresh()?;
        self.issuers.insert(
            id.clone(),
            IssuerState {
                next_sequence: first_sequence,
                reconciled_high_water: first_sequence - 1,
                retired: false,
            },
        );
        let result = self.save(core, inputs);
        if let Err(error) = result {
            if !self.unsettled {
                self.issuers.remove(&id);
            }
            return Err(error);
        }
        Ok(id)
    }
    pub fn receipt_lookup(&self, envelope: &CommandEnvelope) -> Result<ReceiptLookup, BundleError> {
        self.store.with_namespace(|n| n.verify_read_authority())?;
        let digest = envelope.digest()?;
        if let Some(pending) = &self.pending_command
            && pending.id == envelope.id
        {
            return Ok(if pending.digest()? == digest {
                ReceiptLookup::Pending
            } else {
                ReceiptLookup::IdPayloadConflict
            });
        }
        let current = self.receipts.lookup(&envelope.id, &digest);
        if current != ReceiptLookup::Absent {
            return Ok(current);
        }
        for reference in &self.recovery.predecessors {
            let _encoded = self.store.reserve(0, reference.byte_length, 0, false)?;
            let mut file = self.store.open_verified(reference)?;
            let bytes = crate::bundle_recovery::bounded_bytes(&mut file, reference.byte_length)?;
            let prior = crate::bundle::BundleWriteAdmission::from_bytes_using(
                None,
                bytes,
                self.store.usage_arc(),
            )?;
            prior.require_supported()?;
            if let Some(root) = prior.data.file.as_ref().and_then(|f| f.studio.as_ref())
                && (root.project_id == self.project_id
                    || self.origin_project_ids.contains(&root.project_id))
                && let Some(index) = &root.receipt_index_ref
            {
                let ledger = self.store.read_record::<ReceiptLedger>(index)?;
                let result = ledger.lookup(&envelope.id, &digest);
                if result != ReceiptLookup::Absent {
                    return Ok(match result {
                        ReceiptLookup::Committed(receipt)
                            if self
                                .issuers
                                .get(&envelope.id.issuer)
                                .is_none_or(|state| state.retired) =>
                        {
                            ReceiptLookup::Retired(receipt)
                        }
                        other => other,
                    });
                }
            }
        }
        Ok(match self.issuers.get(&envelope.id.issuer) {
            Some(state) if !state.retired && envelope.id.sequence >= state.next_sequence => {
                ReceiptLookup::Absent
            }
            _ => ReceiptLookup::RetiredUnresolved,
        })
    }
    /// The supplied Core is quiescent, acknowledged state. S2 owns GUI producer fencing.
    pub fn command(
        &mut self,
        envelope: &CommandEnvelope,
        core: &Core,
        inputs: &ObjectRef,
        blobs: &[(&ObjectRef, &[u8])],
    ) -> Result<ReceiptLookup, BundleError> {
        crate::bundle_commit::BundleCommit::command(self, envelope, core, inputs, blobs)
    }
    pub fn acknowledge_issuer(
        &mut self,
        id: &IssuerId,
        through: u64,
        core: &Core,
        inputs: &ObjectRef,
    ) -> Result<(), BundleError> {
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let old = self
            .issuers
            .get(id)
            .cloned()
            .ok_or_else(|| BundleError::Unsupported("unknown issuer".into()))?;
        if through < old.reconciled_high_water || through >= old.next_sequence {
            return Err(BundleError::Unsupported(
                "invalid reconciled high water".into(),
            ));
        }
        self.issuers
            .get_mut(id)
            .ok_or_else(|| {
                BundleError::Corrupt("issuer disappeared during owner operation".into())
            })?
            .reconciled_high_water = through;
        if let Err(error) = self.save(core, inputs) {
            if !self.unsettled {
                self.issuers.insert(id.clone(), old);
            }
            return Err(error);
        }
        Ok(())
    }
    /// Retain exact results after detail retirement. Only acknowledged detail can be forgotten.
    pub fn retire_issuer(
        &mut self,
        id: &IssuerId,
        forget: bool,
        core: &Core,
        inputs: &ObjectRef,
    ) -> Result<(), BundleError> {
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let old = self
            .issuers
            .get(id)
            .cloned()
            .ok_or_else(|| BundleError::Unsupported("unknown issuer".into()))?;
        if old.reconciled_high_water.checked_add(1) != Some(old.next_sequence) {
            return Err(BundleError::Capacity("unresolved issuer evidence"));
        }
        let prior = self.receipts.clone();
        self.issuers
            .get_mut(id)
            .ok_or_else(|| {
                BundleError::Corrupt("issuer disappeared during owner operation".into())
            })?
            .retired = true;
        let mut kept = Vec::new();
        for receipt in self.receipts.detailed.drain(..) {
            if receipt.id.issuer == *id {
                self.receipts.retired.push(receipt);
            } else {
                kept.push(receipt);
            }
        }
        self.receipts.detailed = kept;
        if forget {
            self.receipts.retired.retain(|r| r.id.issuer != *id);
        }
        if !self
            .receipts
            .detailed
            .iter()
            .chain(&self.receipts.retired)
            .any(|r| r.id.issuer == *id)
        {
            self.issuers.remove(id);
        }
        if let Err(error) = self.save(core, inputs) {
            if !self.unsettled {
                self.receipts = prior;
                self.issuers.insert(id.clone(), old);
            }
            return Err(error);
        }
        Ok(())
    }
    pub fn reconcile_command(&mut self, core: &Core) -> Result<(), BundleError> {
        self.reconcile()?;
        let checkpoint = self
            .branches
            .get(&self.manual_branch_id)
            .ok_or_else(|| BundleError::Corrupt("missing acknowledged checkpoint".into()))?;
        let history = self
            .store
            .read_record::<HistorySnapshot>(&checkpoint.history_ref)?;
        core.restore_history((*history).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        self.pending_command = None;
        self.pending_result = None;
        Ok(())
    }
}

pub(crate) fn stage_comment(
    bundle: &mut ProjectBundle,
    envelope: &CommandEnvelope,
    blobs: &[(&ObjectRef, &[u8])],
) -> Result<ReceiptResult, BundleError> {
    let text = envelope
        .comment_text
        .as_ref()
        .ok_or_else(|| BundleError::Unsupported("missing comment text".into()))?;
    let strokes = envelope
        .comment_strokes
        .as_ref()
        .ok_or_else(|| BundleError::Unsupported("missing comment strokes".into()))?;
    if bundle.receipts.comments >= bundle.store.limits().comments
        || text.byte_length > bundle.store.limits().comment_text_bytes
        || strokes.byte_length > bundle.store.limits().command_bytes
    {
        return Err(BundleError::Capacity("comment bytes/count"));
    }
    let supplied_text = blobs
        .iter()
        .find(|(r, _)| *r == text)
        .map(|(_, bytes)| *bytes);
    let supplied_strokes = blobs
        .iter()
        .find(|(r, _)| *r == strokes)
        .map(|(_, bytes)| *bytes);
    let retained_text = if supplied_text.is_none() {
        Some(bundle.store.read_evidence(text)?)
    } else {
        None
    };
    let retained_strokes = if supplied_strokes.is_none() {
        Some(bundle.store.read_evidence(strokes)?)
    } else {
        None
    };
    let text_bytes = supplied_text.unwrap_or_else(|| retained_text.as_ref().unwrap().as_slice());
    let stroke_bytes =
        supplied_strokes.unwrap_or_else(|| retained_strokes.as_ref().unwrap().as_slice());
    for (reference, bytes) in [(text, text_bytes), (strokes, stroke_bytes)] {
        if bytes.len() as u64 != reference.byte_length
            || kinewright_media::sha256_bytes(bytes) != reference.sha256
        {
            return Err(BundleError::Corrupt("comment ingress identity".into()));
        }
    }
    std::str::from_utf8(text_bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    validate_strokes(stroke_bytes, bundle.store.limits().comment_points)?;
    let mut seals = Vec::new();
    for reference in std::iter::once(text)
        .chain(std::iter::once(strokes))
        .chain(&envelope.evidence)
    {
        if reference.record_kind != RecordKind::OwnedEvidence {
            return Err(BundleError::Unsupported("comment evidence kind".into()));
        }
        if let Some((_, bytes)) = blobs.iter().find(|(r, _)| *r == reference) {
            let mut input = *bytes;
            let sealed = bundle.store.admit_blob(&mut input, reference, &[])?;
            seals.push(sealed);
        } else {
            bundle.store.verify(reference)?;
        }
    }
    let record_id = format!("comment-{}", SessionId::fresh()?.as_str());
    let bytes=serde_json::to_vec(&serde_json::json!({"tag":"s1-observer-comment","record_id":record_id,"text":text,"strokes":strokes})).map_err(|e|BundleError::Corrupt(e.to_string()))?;
    let record_ref = ObjectRef {
        sha256: kinewright_media::sha256_bytes(&bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let mut dependencies = vec![text.clone(), strokes.clone()];
    dependencies.extend(envelope.evidence.clone());
    let record = bundle
        .store
        .admit_blob(&mut bytes.as_slice(), &record_ref, &dependencies)?;
    bundle.retain_evidence(&record_ref)?;
    drop(record);
    drop(seals);
    Ok(ReceiptResult::Comment {
        record_id,
        record_ref,
        text: text.clone(),
        strokes: strokes.clone(),
        evidence: envelope.evidence.clone(),
    })
}

fn validate_strokes(bytes: &[u8], limit: u64) -> Result<(), BundleError> {
    use serde::de::{Error, SeqAccess, Visitor};
    struct Points<'a> {
        limit: u64,
        overflow: &'a std::cell::Cell<bool>,
    }
    impl<'de> Visitor<'de> for Points<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded opaque point pairs")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
            let mut count = 0_u64;
            while sequence
                .next_element::<[serde_json::Number; 2]>()?
                .is_some()
            {
                count += 1;
                if count > self.limit {
                    self.overflow.set(true);
                    return Err(A::Error::custom("comment point capacity"));
                }
            }
            Ok(())
        }
    }
    let overflow = std::cell::Cell::new(false);
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    serde::Deserializer::deserialize_seq(
        &mut parser,
        Points {
            limit,
            overflow: &overflow,
        },
    )
    .map_err(|e| {
        if overflow.get() {
            BundleError::Capacity("comment points")
        } else {
            BundleError::Corrupt(e.to_string())
        }
    })?;
    parser
        .end()
        .map_err(|e| BundleError::Corrupt(e.to_string()))
}
