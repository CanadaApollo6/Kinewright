//! First-send authorization is consumed per invocation, separately from reusable certification.
#![allow(clippy::missing_errors_doc)]
use super::super::{
    AdmittedAttempt, ExecutionIdentity,
    orchestration_commit::{CommandDecision, OrchestrationReceipt},
};
use super::{AttemptBinding, CertificationRefusal, CertifiedConnectorProfile};
use kinewright_core::SessionConfig;
use kinewright_project::{bundle_records::ObjectRef, orchestration_accounting::SendConsumption};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}
impl ImmutableInvocation {
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
impl AuthorizedInvocation {
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
