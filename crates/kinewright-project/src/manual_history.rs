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

/// The writer's ordinary cut and pool bounds cross the GUI lease boundary;
/// raw/shared reservation ledgers remain with their actual owners.
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
    pub fn request(
        &self,
        core: &kinewright_core::Core,
        command: kinewright_core::Command,
    ) -> Result<kinewright_core::Event, kinewright_core::CoreDisconnected> {
        std::sync::Arc::new(OrdinaryHistoryAdmission::new(self.clone())).request(core, command)
    }
    pub fn select(&self, history: &HistorySnapshot) -> Result<HistorySnapshot, BundleError> {
        crate::bundle_commit::select_history(history, &self.limits()?)
            .map(|selected| selected.history)
    }
    pub fn from_limits(limits: &crate::storage_budget::StorageLimits) -> Result<Self, BundleError> {
        limits.validate()?;
        Ok(Self {
            branch_bytes: limits.branch_history_bytes,
            live_bytes: limits.live_history_bytes,
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
    /// Check an already selected cut without discarding any Undo steps.
    pub fn check(&self, history: &HistorySnapshot) -> Result<(), BundleError> {
        crate::history_size::verify_history_admission(
            history,
            crate::history_record::HistoryRole::Ordinary,
            &self.limits()?,
        )
        .map(|_| ())
    }
}
impl crate::bundle::ProjectBundle {
    pub(crate) fn ordinary_manual_admission(
        &mut self,
    ) -> Result<std::sync::Arc<OrdinaryHistoryAdmission>, BundleError> {
        let limits = OrdinaryHistoryLimits::from_limits(self.store.limits())?;
        if self
            .ordinary_manual_admission
            .as_ref()
            .is_none_or(|admission| admission.limits != limits)
        {
            self.ordinary_manual_admission =
                Some(std::sync::Arc::new(OrdinaryHistoryAdmission::new(limits)));
        }
        Ok(std::sync::Arc::clone(
            self.ordinary_manual_admission
                .as_ref()
                .expect("initialized ordinary manual admission"),
        ))
    }
}

/// Runtime proof cache owned by one ordinary editing endpoint. Only a complete
/// successful admission can replace its immutable prefix proof.
pub struct OrdinaryHistoryAdmission {
    limits: OrdinaryHistoryLimits,
    proof: std::sync::Mutex<Option<crate::history_size::OrdinaryHistoryProof>>,
}
impl OrdinaryHistoryAdmission {
    #[must_use]
    pub fn new(limits: OrdinaryHistoryLimits) -> Self {
        Self {
            limits,
            proof: std::sync::Mutex::new(None),
        }
    }
    pub(crate) fn prior(&self) -> Result<Option<crate::history_size::OrdinaryHistoryProof>, BundleError> {
        self.proof
            .lock()
            .map(|proof| proof.clone())
            .map_err(|_| BundleError::Io(std::io::Error::other("history proof unavailable")))
    }
    fn prove(
        &self,
        history: &HistorySnapshot,
    ) -> Result<crate::history_size::OrdinaryHistoryProof, BundleError> {
        let prior = self.prior()?;
        if let Some(proof) = &prior
            && proof.snapshot() == history
        {
            return Ok(proof.clone());
        }
        let (_, proof) = crate::bundle_commit::select_admitted_history(
            history,
            &self.limits.limits()?,
            prior.as_ref(),
        )?;
        Ok(proof)
    }
    pub(crate) fn remember(
        &self,
        proof: crate::history_size::OrdinaryHistoryProof,
    ) -> Result<(), BundleError> {
        *self
            .proof
            .lock()
            .map_err(|_| BundleError::Io(std::io::Error::other("history proof unavailable")))? =
            Some(proof);
        Ok(())
    }
    fn verify(
        &self,
        history: &HistorySnapshot,
    ) -> Result<crate::history_size::OrdinaryHistoryProof, BundleError> {
        let proof = self.prove(history)?;
        self.remember(proof.clone())?;
        Ok(proof)
    }
    pub fn check(&self, history: &HistorySnapshot) -> Result<(), BundleError> {
        self.verify(history).map(|_| ())
    }
    pub(crate) fn request_at_floor(
        self: &std::sync::Arc<Self>,
        core: &kinewright_core::Core,
        command: kinewright_core::Command,
        floor_offset: u64,
    ) -> Result<kinewright_core::Event, kinewright_core::CoreDisconnected> {
        core.request_admitted(
            command,
            Box::new(OrdinaryHistoryGuard {
                admission: std::sync::Arc::clone(self),
                staged: None,
                exact_floor: Some(floor_offset),
            }),
            self.limits.live_bytes.saturating_sub(
                self.limits
                    .branch_bytes
                    .min(self.limits.live_bytes)
                    .min(self.limits.decoded_bytes),
            ),
        )
    }
    pub fn request(
        self: &std::sync::Arc<Self>,
        core: &kinewright_core::Core,
        command: kinewright_core::Command,
    ) -> Result<kinewright_core::Event, kinewright_core::CoreDisconnected> {
        // This endpoint's proof crosses to the Core actor and returns to the
        // same endpoint only after its entire mutation transaction succeeds.
        core.request_admitted(
            command,
            Box::new(OrdinaryHistoryGuard {
                admission: std::sync::Arc::clone(self),
                staged: None,
                exact_floor: None,
            }),
            self.limits.live_bytes.saturating_sub(
                self.limits
                    .branch_bytes
                    .min(self.limits.live_bytes)
                    .min(self.limits.decoded_bytes),
            ),
        )
    }
}
fn manual_preparation_error(error: &BundleError, undo: bool) -> HistoryError {
    HistoryError::MutationRefused(match error {
        BundleError::Capacity(_) => {
            if undo {
                kinewright_core::CoreMutationRefusal::ManualUndoCapacity
            } else {
                kinewright_core::CoreMutationRefusal::ManualHistoryCapacity
            }
        }
        _ => kinewright_core::CoreMutationRefusal::ManualHistoryUnavailable,
    })
}

struct OrdinaryHistoryGuard {
    admission: std::sync::Arc<OrdinaryHistoryAdmission>,
    staged: Option<crate::history_size::OrdinaryHistoryProof>,
    exact_floor: Option<u64>,
}
impl kinewright_core::HistoryAdmission for OrdinaryHistoryGuard {
    fn check_history(
        &mut self,
        history: &HistorySnapshot,
    ) -> Result<Option<kinewright_core::ResolvedHistory>, HistoryError> {
        let proof = if let Some(offset) = self.exact_floor {
            (|| {
                let discard = offset
                    .checked_sub(history.floor_offset)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or(BundleError::HistoryBarrierInvariant)?;
                let selected = if discard == 0 {
                    history.clone()
                } else {
                    kinewright_core::HistoryTransition::floor_after(history, discard)
                        .map_err(crate::bundle::history_error)?
                };
                let prior = self.admission.prior()?;
                let proof = crate::history_size::verify_ordinary_history(
                    &selected, &self.admission.limits.limits()?, prior.as_ref(),
                )?;
                Ok(proof)
            })()
        } else {
            self.admission.prove(history)
        };
        proof
            .and_then(|proof| {
                proof.share_canonical_documents(history)
            })
            .map(|proof| {
                let replay = proof.replay().clone();
                self.staged = Some(proof);
                Some(replay)
            })
            .map_err(|error| {
                manual_preparation_error(
                    &error,
                    history.batches.last().is_some_and(|batch| {
                        matches!(batch.command, kinewright_core::JournalCommand::Undo)
                    }),
                )
            })
    }
    fn finish_history_check(&mut self, accepted: bool) {
        if let Some(proof) = self.staged.take()
            && accepted
        {
            // A poisoned memo loses only a performance hint, never the accepted edit.
            let _ = self.admission.remember(proof);
        }
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
    guard: OrdinaryHistoryGuard,
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
        let admission =
            std::sync::Arc::new(OrdinaryHistoryAdmission::new(self.history_limits.clone()));
        Ok(Box::new(AdmittedManualEncoder {
            lease: self,
            input_manifest,
            guard: OrdinaryHistoryGuard {
                admission,
                staged: None,
                exact_floor: None,
            },
        }))
    }
}
impl kinewright_core::HistoryAdmission for AdmittedManualEncoder {
    fn check_history(
        &mut self,
        history: &HistorySnapshot,
    ) -> Result<Option<kinewright_core::ResolvedHistory>, HistoryError> {
        self.guard.check_history(history)
    }
    fn finish_history_check(&mut self, accepted: bool) {
        self.guard.finish_history_check(accepted);
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
        encode().map_err(|error| manual_preparation_error(&error, false))
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
    fn ordinary_equal_cut_allocation_replacement_agrees_with_fresh_selection() {
        use kinewright_core::{HistoryError, HistoryJournal, SolidColor, TimeCode};
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
        for index in 0..4 {
            core.request(Command::Do(Operation::AddSolidClip {
                track: TrackId(1),
                timeline_start: TimeCode(index * 90),
                duration: TimeCode(90),
                color: SolidColor { r: 1, g: 2, b: 3 },
            }))
            .unwrap();
        }
        let original = core.export_history().unwrap();
        let mut changed = original.clone();
        let mut baseline = (*changed.baseline).clone();
        baseline.tracks.reserve(100);
        let enlarged = std::sync::Arc::new(baseline);
        for boundary in changed
            .cursor
            .undo
            .iter_mut()
            .chain(&mut changed.cursor.redo)
        {
            if std::sync::Arc::ptr_eq(&boundary.document, &changed.baseline) {
                boundary.document = std::sync::Arc::clone(&enlarged);
            }
        }
        changed.baseline = enlarged;
        let journal = HistoryJournal::capture(&changed).unwrap();
        let cold = journal
            .resolve_verified_with_budget(
                std::sync::Arc::clone(&changed.baseline),
                &changed.document,
                128 << 20,
                None,
            )
            .unwrap();
        let window = cold.replay_peak() - 1;
        assert_eq!(
            journal
                .resolve_verified_with_budget(
                    std::sync::Arc::clone(&changed.baseline),
                    &changed.document,
                    window,
                    None
                )
                .unwrap_err(),
            HistoryError::Capacity,
            "LARGER_ALLOCATION_IS_NOT_FREE"
        );
        let limits = crate::storage_budget::StorageLimits {
            decoded_record_bytes: window,
            ..crate::storage_budget::StorageLimits::default()
        };
        let policy = OrdinaryHistoryLimits::from_limits(&limits).unwrap();
        let cached = OrdinaryHistoryAdmission::new(policy.clone());
        cached.check(&original).unwrap();
        let from_cache = cached.prove(&changed);
        let fresh = OrdinaryHistoryAdmission::new(policy).prove(&changed);
        match (from_cache, fresh) {
            (Ok(cached), Ok(fresh)) => {
                assert_eq!(
                    cached.snapshot(),
                    fresh.snapshot(),
                    "EQUAL_VALUE_REPLACEMENT_USES_THE_SAME_FLOOR"
                );
                assert_eq!(cached.replay().replay_peak(), fresh.replay().replay_peak());
            }
            (Err(BundleError::Capacity(_)), Err(BundleError::Capacity(_))) => {}
            (cached, fresh) => panic!("cached={cached:?} fresh={fresh:?}"),
        }
    }

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
        let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
            OrdinaryHistoryLimits::default(),
        ));
        for index in 0_usize..32 {
            let operations = vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index.is_multiple_of(2),
            }];
            let Event::DocumentChanged { doc, .. } = admission
                .request(&core, Command::DoBatch(operations))
                .unwrap()
            else {
                panic!("accepted edit");
            };
            let live = core.export_history().unwrap();
            assert!(
                std::sync::Arc::ptr_eq(&doc, &live.document),
                "ORDINARY_EDIT_KEEPS_ITS_ALREADY_CHECKED_DOCUMENT"
            );
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

#[cfg(test)]
mod manual_failure_contract_tests {
    use super::*;
    #[test]
    fn poisoned_manual_proof_is_a_final_preparation_refusal() {
        use kinewright_core::{
            Command, Core, CoreMutationRefusal, Document, Event, Operation, Track, TrackId,
            TrackKind,
        };
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
        let admission = std::sync::Arc::new(OrdinaryHistoryAdmission::new(
            OrdinaryHistoryLimits::default(),
        ));
        let before = core.export_history().unwrap();
        let revision = match core
            .request(Command::Query(kinewright_core::Query::Snapshot))
            .unwrap()
        {
            Event::QueryResult(kinewright_core::QueryResult::Snapshot { revision, .. }) => revision,
            other => panic!("unexpected revision: {other:?}"),
        };
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _held = admission.proof.lock().unwrap();
                panic!("inject unavailable proof");
            }))
            .is_err()
        );
        for _ in 0..2 {
            assert!(matches!(
                admission
                    .request(
                        &core,
                        Command::Do(Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked: false
                        })
                    )
                    .unwrap(),
                Event::MutationRefused {
                    reason: CoreMutationRefusal::ManualHistoryUnavailable,
                    ..
                }
            ));
            assert_eq!(core.export_history().unwrap(), before);
            assert!(
                matches!(core.request(Command::Query(kinewright_core::Query::Snapshot)).unwrap(), Event::QueryResult(kinewright_core::QueryResult::Snapshot { revision: current, .. }) if current == revision)
            );
        }
    }
    #[test]
    fn manual_backend_failures_never_promise_acknowledgement_room() {
        for undo in [false, true] {
            for error in [
                BundleError::HistoryChanged,
                BundleError::Unsettled,
                BundleError::Corrupt("probe".into()),
                BundleError::Io(std::io::Error::other("probe")),
            ] {
                assert_eq!(
                    manual_preparation_error(&error, undo),
                    HistoryError::MutationRefused(
                        kinewright_core::CoreMutationRefusal::ManualHistoryUnavailable
                    )
                );
            }
        }
        assert_eq!(
            kinewright_core::CoreMutationRefusal::ManualHistoryUnavailable.to_string(),
            "This edit could not be prepared. Your timeline is unchanged."
        );
    }
}
