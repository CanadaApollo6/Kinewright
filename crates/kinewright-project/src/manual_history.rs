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
    pub history_limits: OrdinaryHistoryLimits,
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

/// Only the effective ordinary predicate crosses the GUI lease boundary; the
/// writer's raw/shared reservation ledgers remain with their actual owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrdinaryHistoryLimits {
    branch_bytes: u64,
    live_bytes: u64,
    decoded_bytes: u64,
    graph_depth: u64,
    command_bytes: u64,
}
impl Default for OrdinaryHistoryLimits {
    fn default() -> Self {
        Self::from_limits(&crate::storage_budget::StorageLimits::default())
            .expect("default history limits")
    }
}
impl OrdinaryHistoryLimits {
    pub fn check_batch(
        &self,
        core: &kinewright_core::Core,
        operations: &[kinewright_core::Operation],
    ) -> Result<(), BundleError> {
        OrdinaryHistoryAdmission::new(self.clone()).check_batch(core, operations)
    }
    pub fn from_limits(limits: &crate::storage_budget::StorageLimits) -> Result<Self, BundleError> {
        limits.validate()?;
        Ok(Self {
            branch_bytes: limits.branch_history_bytes,
            live_bytes: crate::history_record::HistoryRole::Ordinary.live_limit(limits),
            decoded_bytes: limits.decoded_record_bytes,
            graph_depth: limits.graph_depth,
            command_bytes: limits.command_bytes,
        })
    }
    fn limits(&self) -> Result<crate::storage_budget::StorageLimits, BundleError> {
        let limits = crate::storage_budget::StorageLimits {
            branch_history_bytes: self.branch_bytes,
            live_history_bytes: self.live_bytes,
            decoded_record_bytes: self.decoded_bytes,
            graph_depth: self.graph_depth,
            command_bytes: self.command_bytes,
            ..crate::storage_budget::StorageLimits::default()
        };
        limits.validate()?;
        Ok(limits)
    }
    pub fn check(&self, history: &HistorySnapshot) -> Result<(), BundleError> {
        crate::history_size::verify_history_admission(
            history,
            crate::history_record::HistoryRole::Ordinary,
            &self.limits()?,
        )
        .map(|_| ())
    }
}
/// Runtime proof cache owned by one ordinary editing endpoint. Only a complete
/// successful admission can replace its immutable prefix proof.
pub struct OrdinaryHistoryAdmission {
    limits: OrdinaryHistoryLimits,
    proof: std::sync::Mutex<Option<kinewright_core::ResolvedHistory>>,
}
impl OrdinaryHistoryAdmission {
    #[must_use]
    pub fn new(limits: OrdinaryHistoryLimits) -> Self {
        Self {
            limits,
            proof: std::sync::Mutex::new(None),
        }
    }
    fn prove(
        &self,
        history: &HistorySnapshot,
    ) -> Result<kinewright_core::ResolvedHistory, BundleError> {
        let prior = self
            .proof
            .lock()
            .map_err(|_| BundleError::Capacity("history proof unavailable"))?
            .clone();
        if let Some(proof) = &prior
            && proof.snapshot() == history
        {
            return proof
                .clone()
                .share_documents(history.clone())
                .map_err(crate::bundle::history_error);
        }
        let (_, proof) = crate::history_size::verify_history_admission_with_prefix(
            history,
            crate::history_record::HistoryRole::Ordinary,
            &self.limits.limits()?,
            prior.as_ref(),
        )?;
        let proof = proof
            .share_documents(history.clone())
            .map_err(crate::bundle::history_error)?;
        Ok(proof)
    }
    fn remember(&self, proof: kinewright_core::ResolvedHistory) -> Result<(), BundleError> {
        *self
            .proof
            .lock()
            .map_err(|_| BundleError::Capacity("history proof unavailable"))? = Some(proof);
        Ok(())
    }
    fn verify(
        &self,
        history: &HistorySnapshot,
    ) -> Result<kinewright_core::ResolvedHistory, BundleError> {
        let proof = self.prove(history)?;
        self.remember(proof.clone())?;
        Ok(proof)
    }
    pub fn check(&self, history: &HistorySnapshot) -> Result<(), BundleError> {
        self.verify(history).map(|_| ())
    }
    pub fn check_batch(
        &self,
        core: &kinewright_core::Core,
        operations: &[kinewright_core::Operation],
    ) -> Result<(), BundleError> {
        let history = core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let proof = self.verify(&history)?;
        let private = kinewright_core::Core::spawn(std::sync::Arc::clone(&history.baseline))
            .map_err(|error| BundleError::Corrupt(error.to_string()))?;
        private
            .restore_resolved_history(proof)
            .map_err(crate::bundle::history_error)?;
        if matches!(
            private
                .request(kinewright_core::Command::DoBatch(operations.to_vec()))
                .map_err(|error| BundleError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    error
                )))?,
            kinewright_core::Event::DocumentChanged { .. }
        ) {
            self.check(
                &private
                    .export_history()
                    .map_err(crate::bundle::history_error)?,
            )?;
        }
        Ok(())
    }
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
    admission: OrdinaryHistoryAdmission,
    staged: Option<kinewright_core::ResolvedHistory>,
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
        let admission = OrdinaryHistoryAdmission::new(self.history_limits.clone());
        Ok(Box::new(AdmittedManualEncoder {
            lease: self,
            input_manifest,
            admission,
            staged: None,
        }))
    }
}
impl ManualHistoryEncoder for AdmittedManualEncoder {
    fn check_history(&mut self, history: &HistorySnapshot) -> Result<(), HistoryError> {
        self.admission
            .prove(history)
            .map(|proof| {
                self.staged = Some(proof);
            })
            .map_err(|error| match error {
                BundleError::Capacity(_) => HistoryError::MutationRefused(
                    kinewright_core::CoreMutationRefusal::ManualHistoryCapacity,
                ),
                BundleError::Corrupt(_) => HistoryError::Corrupt,
                BundleError::Io(_) => HistoryError::Encoding,
                _ => HistoryError::MutationRefused(
                    kinewright_core::CoreMutationRefusal::ManualBackpressure,
                ),
            })
    }
    fn finish_history_check(&mut self, accepted: bool) {
        if let Some(proof) = self.staged.take()
            && accepted
        {
            // This endpoint is actor-owned; its private proof mutex has no
            // external holder that can poison it during admission.
            self.admission
                .remember(proof)
                .expect("actor-owned history proof");
        }
    }
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
            if bytes.len() as u64 > self.history_limits.command_bytes {
                return Err(BundleError::Capacity("manual command bytes"));
            }
            Ok(AcceptedHistory {
                sequence,
                batch_id: batch_id.as_str().into(),
                digest,
                envelope: bytes,
            })
        };
        encode().map_err(|error| match error {
            BundleError::Capacity(_) => HistoryError::MutationRefused(
                kinewright_core::CoreMutationRefusal::ManualHistoryCapacity,
            ),
            _ => HistoryError::Capacity,
        })
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

#[cfg(test)]
mod admission_tests {
    use super::*;
    use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
    #[test]
    fn history_admission_cached_previews_rebind_to_actual_actor_documents() {
        let core = Core::spawn(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let admission = OrdinaryHistoryAdmission::new(OrdinaryHistoryLimits::default());
        for index in 0_usize..32 {
            let operations = vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index.is_multiple_of(2),
            }];
            admission.check_batch(&core, &operations).unwrap();
            assert!(matches!(
                core.request(Command::DoBatch(operations)).unwrap(),
                Event::DocumentChanged { .. }
            ));
            let live = core.export_history().unwrap();
            admission.check(&live).unwrap();
            let held = admission.proof.lock().unwrap();
            let proof = held.as_ref().unwrap().snapshot();
            assert!(std::sync::Arc::ptr_eq(&proof.document, &live.document));
            for (cached, actual) in proof.cursor.undo.iter().zip(&live.cursor.undo) {
                assert!(
                    std::sync::Arc::ptr_eq(&cached.document, &actual.document),
                    "CACHED_PROOF_DOES_NOT_KEEP_A_SECOND_DOCUMENT_HISTORY"
                );
            }
        }
    }
}
