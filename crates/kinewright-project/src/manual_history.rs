//! Actor-stamped manual commands use a distinct admitted-history publication route.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{BatchId, BranchId, HeadId, ObjectRef, ProjectId},
    receipts::{CommandEnvelope, CommandId, IssuerId},
};
use kinewright_core::{
    AcceptedHistory, HistoryBatch, HistoryError, HistorySnapshot, ManualHistoryCandidate,
    ManualHistoryEncoder, TimelineRevision,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualLease {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_identity: Option<(u64, u64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_name: Option<String>,
    pub client: String,
    pub token: String,
    pub project: ProjectId,
    pub branch: BranchId,
    pub switch_generation: u64,
    pub owner_epoch: u64,
    pub generation: u64,
    pub issuer: IssuerId,
    pub anchor_head: HeadId,
    pub predecessor: Option<BatchId>,
    pub history_sha256: String,
    pub inputs: ObjectRef,
    pub lineage: ObjectRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualHistoryIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_origin: Option<crate::receipts::OriginalCommand>,
    pub binding: ManualLease,
    pub batch_id: BatchId,
    pub predecessor: Option<BatchId>,
    pub before_sha256: String,
    pub after_sha256: String,
    pub cursor_sha256: String,
    pub batch: HistoryBatch,
}
pub fn history_sha256(history: &HistorySnapshot) -> Result<String, BundleError> {
    serde_json::to_vec(history)
        .map(|v| kinewright_media::sha256_bytes(&v))
        .map_err(|e| BundleError::Corrupt(e.to_string()))
}
struct AdmittedManualEncoder {
    lease: ManualLease,
    input_manifest: crate::owned_inputs::OwnedInputManifest,
}
impl std::ops::Deref for AdmittedManualEncoder {
    type Target = ManualLease;
    fn deref(&self) -> &Self::Target {
        &self.lease
    }
}
impl ManualLease {
    pub fn into_encoder(
        self,
        input_manifest: crate::owned_inputs::OwnedInputManifest,
    ) -> Result<Box<dyn ManualHistoryEncoder>, BundleError> {
        let bytes =
            serde_json::to_vec(&input_manifest).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if bytes.len() > kinewright_core::MANUAL_BYTES {
            return Err(BundleError::Capacity("manual input control overlap"));
        }
        if kinewright_media::sha256_bytes(&bytes) != self.inputs.sha256
            || bytes.len() as u64 != self.inputs.byte_length
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(Box::new(AdmittedManualEncoder {
            lease: self,
            input_manifest,
        }))
    }
}
impl ManualHistoryEncoder for AdmittedManualEncoder {
    fn encode(
        &self,
        sequence: u64,
        revision: TimelineRevision,
        predecessor: Option<&str>,
        candidate: ManualHistoryCandidate<'_>,
    ) -> Result<AcceptedHistory, HistoryError> {
        crate::bundle_commit::validate_document_inputs(candidate.document, &self.input_manifest)
            .map_err(|_| {
                HistoryError::MutationRefused(
                    kinewright_core::CoreMutationRefusal::ManualInputRequired,
                )
            })?;
        let encode = || -> Result<AcceptedHistory, BundleError> {
            let batch = candidate.batch.clone();
            let batch_id = BatchId::fresh()?;
            let previous = predecessor
                .map(|id| serde_json::from_value(serde_json::Value::String(id.into())))
                .transpose()
                .map_err(|e| BundleError::Corrupt(e.to_string()))?
                .or_else(|| self.predecessor.clone());
            let envelope = CommandEnvelope {
                switch: None,
                manual: Some(ManualHistoryIdentity {
                    recovery_origin: None,
                    binding: self.lease.clone(),
                    batch_id: batch_id.clone(),
                    predecessor: previous,
                    before_sha256: candidate.before_sha256.to_owned(),
                    after_sha256: candidate.after_sha256.to_owned(),
                    cursor_sha256: candidate.cursor_sha256.to_owned(),
                    batch: batch.clone(),
                }),
                version: 1,
                action: "manual".into(),
                id: CommandId {
                    issuer: self.issuer.clone(),
                    sequence,
                },
                project_id: self.project.clone(),
                target_branch: self.branch.clone(),
                owner_epoch: self.owner_epoch,
                lease: Some(self.token.clone()),
                expected_head: Some(self.anchor_head.clone()),
                expected_revision: revision,
                command: Some(batch.command),
                comment_text: None,
                comment_strokes: None,
                evidence: vec![],
                transition: None,
            };
            let digest = serde_json::to_value(envelope.digest()?)
                .map_err(|e| BundleError::Corrupt(e.to_string()))?
                .as_str()
                .ok_or(BundleError::HistoryBarrierInvariant)?
                .to_owned();
            let bytes =
                serde_json::to_vec(&envelope).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            Ok(AcceptedHistory {
                sequence,
                batch_id: batch_id.as_str().into(),
                digest,
                envelope: bytes,
            })
        };
        encode().map_err(|_| HistoryError::Capacity)
    }
}
impl AcceptedManualCommand {
    pub fn decode(accepted: &AcceptedHistory) -> Result<Self, BundleError> {
        let envelope: CommandEnvelope = serde_json::from_slice(&accepted.envelope)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        envelope.encoded_bytes()?;
        let identity = envelope
            .manual
            .as_ref()
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let digest = serde_json::to_value(envelope.digest()?)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if accepted.sequence != envelope.id.sequence
            || accepted.batch_id != identity.batch_id.as_str()
            || digest.as_str() != Some(accepted.digest.as_str())
        {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        Ok(Self { envelope })
    }
}
pub struct AcceptedManualCommand {
    pub envelope: CommandEnvelope,
}
