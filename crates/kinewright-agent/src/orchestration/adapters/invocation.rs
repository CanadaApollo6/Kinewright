//! First-send authorization is consumed per invocation, separately from reusable certification.
#![allow(clippy::missing_errors_doc)]
use super::super::{
    AdmittedAttempt, ExecutionIdentity,
    orchestration_commit::{CommandDecision, OrchestrationReceipt},
};
use super::{AttemptBinding, CertificationRefusal, CertifiedConnectorProfile};
use kinewright_core::SessionConfig;
use kinewright_project::{bundle_records::ObjectRef, orchestration_accounting::SendConsumption};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SendKind {
    OpeningAssignment,
    SpecialistStart,
    QueuedSteering,
    AutomaticResultWake,
}

/// The manifest identifies the immutable executable, argv (including opening prompts),
/// effective environment/settings identities, binding, send kind, turn and payload.
/// It must not serialize MCP secrets. Payload is the exact charged prompt bytes.
pub struct ImmutableInvocation {
    binding: AttemptBinding,
    kind: SendKind,
    manifest: ObjectRef,
    payload: ObjectRef,
    #[cfg(target_os = "linux")]
    manifest_bytes: Vec<u8>,
    #[cfg(target_os = "linux")]
    bytes: Vec<u8>,
}
impl ImmutableInvocation {
    #[cfg(target_os = "linux")]
    pub(crate) fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn binding(&self) -> &AttemptBinding {
        &self.binding
    }
    #[must_use]
    pub fn kind(&self) -> SendKind {
        self.kind
    }
    #[must_use]
    pub fn manifest(&self) -> &ObjectRef {
        &self.manifest
    }
    #[must_use]
    pub fn payload(&self) -> &ObjectRef {
        &self.payload
    }
}

/// C2 must construct this only from an exact, durably acknowledged consumption cut.
/// Absent/unknown ACK retains the charge and cannot authorize replacement execution.
/// Opening/specialist/wake sends need that path too; legacy `SendMessage` permits do not suffice.
pub struct SendConsumptionAcknowledgment {
    identity: ExecutionIdentity,
    consumption: SendConsumption,
    accounting: ObjectRef,
    invocation: ObjectRef,
    kind: SendKind,
    receipt: OrchestrationReceipt,
}
impl SendConsumptionAcknowledgment {
    #[must_use]
    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }
    #[must_use]
    pub fn consumption(&self) -> &SendConsumption {
        &self.consumption
    }
    #[must_use]
    pub fn accounting(&self) -> &ObjectRef {
        &self.accounting
    }
    #[must_use]
    pub fn receipt(&self) -> &OrchestrationReceipt {
        &self.receipt
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvocationRefusal {
    #[error(transparent)]
    Certification(#[from] CertificationRefusal),
    #[error("invocation differs from its acknowledged send consumption")]
    SendAcknowledgment,
    #[error("invocation manifest or charged payload changed")]
    PayloadChanged,
}

/// No Clone, deserializer or production constructor. C1 cannot release a provider.
/// C2's minimal durable first-send path must issue this before guardian COMMIT.
/// The launcher consumes it once and verifies the actual manifest/payload before release.
///
/// ```compile_fail
/// use kinewright_agent::orchestration::adapters::AuthorizedInvocation;
/// let authorization = AuthorizedInvocation {};
/// ```
pub struct AuthorizedInvocation {
    profile: CertifiedConnectorProfile,
    invocation: ImmutableInvocation,
    acknowledgment: SendConsumptionAcknowledgment,
}
#[cfg(target_os = "linux")]
pub(crate) struct PreparedInvocation {
    pub profile: CertifiedConnectorProfile,
    pub kind: SendKind,
    pub manifest_bytes: Vec<u8>,
    pub bytes: Vec<u8>,
    pub manifest: ObjectRef,
    pub accounting: ObjectRef,
    pub consumption: SendConsumption,
}
impl AuthorizedInvocation {
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn replace_payload_for_test(&mut self, bytes: Vec<u8>) {
        self.invocation.bytes = bytes;
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn acknowledged(
        prepared: PreparedInvocation,
        update: &super::super::orchestration_commit::ValidatedRunUpdate,
        receipt: &OrchestrationReceipt,
    ) -> Result<Self, InvocationRefusal> {
        let PreparedInvocation {
            profile,
            kind,
            manifest_bytes,
            bytes,
            manifest,
            accounting,
            consumption,
        } = prepared;
        update
            .validate_returned_receipt(receipt)
            .map_err(|_| InvocationRefusal::SendAcknowledgment)?;
        let identity = profile.binding().identity().clone();
        if receipt.decision != CommandDecision::Applied
            || !update.projection().usage.contains(&accounting)
            || !update
                .projection()
                .tasks
                .iter()
                .any(|task| task.id == identity.task && task.progress.decisions.contains(&manifest))
            || !update.projection().attempts.iter().any(|attempt| {
                attempt.identity == identity
                    && attempt.state == super::super::orchestration_records::AttemptState::Reserved
            })
        {
            return Err(InvocationRefusal::SendAcknowledgment);
        }
        Ok(Self {
            invocation: ImmutableInvocation {
                binding: profile.binding().clone(),
                kind,
                manifest: manifest.clone(),
                payload: consumption.payload.clone(),
                manifest_bytes,
                bytes,
            },
            acknowledgment: SendConsumptionAcknowledgment {
                identity,
                consumption,
                accounting,
                invocation: manifest,
                kind,
                receipt: receipt.clone(),
            },
            profile,
        })
    }

    #[must_use]
    pub fn profile(&self) -> &CertifiedConnectorProfile {
        &self.profile
    }
    #[must_use]
    pub fn invocation(&self) -> &ImmutableInvocation {
        &self.invocation
    }
    #[must_use]
    pub fn acknowledgment(&self) -> &SendConsumptionAcknowledgment {
        &self.acknowledgment
    }

    pub fn validate(
        &self,
        cfg: &SessionConfig,
        attempt: &AdmittedAttempt,
    ) -> Result<(), InvocationRefusal> {
        self.profile.validate_session(cfg, attempt.identity())?;
        self.profile.validate_binding(&self.invocation.binding)?;
        let ack = &self.acknowledgment;
        if ack.identity != *attempt.identity()
            || ack.consumption.attempt != attempt.identity().attempt.0
            || ack.consumption.turn == 0
            || ack.consumption.payload != self.invocation.payload
            || ack.invocation != self.invocation.manifest
            || ack.kind != self.invocation.kind
            || ack.receipt.envelope.project != attempt.identity().project
            || ack.receipt.envelope.id != ack.consumption.command
            || ack.receipt.decision != CommandDecision::Applied
        {
            return Err(InvocationRefusal::SendAcknowledgment);
        }
        Ok(())
    }

    pub fn verify_payload(
        &self,
        manifest: &ObjectRef,
        payload: &[u8],
    ) -> Result<(), InvocationRefusal> {
        if manifest != &self.invocation.manifest
            || self.invocation.payload.byte_length != payload.len() as u64
            || self.invocation.payload.sha256 != kinewright_media::sha256_bytes(payload)
        {
            return Err(InvocationRefusal::PayloadChanged);
        }
        Ok(())
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "isolated kernel integration fixture exercises the launch contract"
    )]
    pub(crate) fn fixture(
        profile: CertifiedConnectorProfile,
        attempt: &AdmittedAttempt,
        kind: SendKind,
    ) -> Self {
        use kinewright_project::bundle_records::RecordKind;
        let payload = ObjectRef {
            sha256: kinewright_media::sha256_bytes(b"fixture prompt"),
            byte_length: 14,
            record_kind: RecordKind::OwnedData,
        };
        let manifest = ObjectRef {
            sha256: kinewright_media::sha256_bytes(b"fixture invocation"),
            byte_length: 18,
            record_kind: RecordKind::OwnedData,
        };
        let invocation = ImmutableInvocation {
            binding: profile.binding().clone(),
            kind,
            manifest: manifest.clone(),
            payload: payload.clone(),
            #[cfg(target_os = "linux")]
            manifest_bytes: b"fixture invocation".to_vec(),
            #[cfg(target_os = "linux")]
            bytes: b"fixture prompt".to_vec(),
        };
        let acknowledgment = SendConsumptionAcknowledgment {
            identity: attempt.identity().clone(),
            consumption: SendConsumption {
                command: attempt.reservation_receipt().envelope.id.clone(),
                attempt: attempt.identity().attempt.0.clone(),
                turn: 1,
                payload,
            },
            accounting: manifest.clone(),
            invocation: manifest,
            kind,
            receipt: attempt.reservation_receipt().clone(),
        };
        Self {
            profile,
            invocation,
            acknowledgment,
        }
    }
}
