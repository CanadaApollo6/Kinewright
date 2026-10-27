//! Studio's isolated C1 kernel. No provider execution or project publication.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {Orchestrator,EventSink,CommandReceiptStore,EffectOutbox,EffectWorker}.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright uses the existing Studio owner,
//! explicit project/task/epoch binding, and acknowledgment-gated effects.
//!
//! H1 belongs to S3. Until project lib.rs registers these project-owned modules,
//! compile their exact sources here for the contract handoff. H1 replaces these
//! path declarations with reexports from `kinewright_project`. No opaque-blob or
//! alternative persistence adapter is provided.

use kinewright_project::{bundle_records, editing_session, receipts};

#[path = "../../../kinewright-project/src/orchestration_commit.rs"]
pub mod orchestration_commit;
#[path = "../../../kinewright-project/src/orchestration_records.rs"]
pub mod orchestration_records;

pub mod machine;
pub mod outbox;

/// The complete upstream MIT notice is retained in the shipping agent library.
pub const T3CODE_MIT_NOTICE: &str = include_str!("../../../../LICENSES/t3code-MIT.txt");

pub use orchestration_records::ExecutionIdentity;

/// Opaque proof of the exact attempt reservation acknowledged by the Studio owner.
/// It is runtime-only and has no deserializer or public constructor.
#[derive(Debug)]
pub struct AdmittedAttempt {
    identity: ExecutionIdentity,
    reservation: orchestration_commit::OrchestrationReceipt,
}

impl AdmittedAttempt {
    #[must_use]
    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    #[must_use]
    pub fn reservation_receipt(&self) -> &orchestration_commit::OrchestrationReceipt {
        &self.reservation
    }
}

/// H1 calls this only after B5, while it still owns the exact committed cut.
/// There is no production caller until S3 integrates H1. Unknown publication
/// must reconcile the original envelope and never reaches this function.
#[allow(
    dead_code,
    reason = "receiving H1 owner port; production publication is not integrated yet"
)]
pub(crate) fn acknowledge_attempt_reservation(
    update: &orchestration_commit::ValidatedRunUpdate,
    returned: &orchestration_commit::OrchestrationReceipt,
    identity: &ExecutionIdentity,
) -> Result<AdmittedAttempt, orchestration_records::Refusal> {
    use orchestration_commit::CommandDecision;
    use orchestration_records::{AttemptState, Refusal};
    update.validate_returned_receipt(returned)?;
    identity.validate()?;
    if returned.decision != CommandDecision::Applied
        || identity.project != update.envelope().project
        || identity.run != update.projection().run
        || update
            .projection()
            .attempts
            .iter()
            .filter(|reservation| {
                reservation.identity == *identity
                    && reservation.state == AttemptState::Reserved
                    && reservation.owner_epoch == update.envelope().authority.owner_epoch
            })
            .count()
            != 1
    {
        return Err(Refusal::ScopeConflict);
    }
    Ok(AdmittedAttempt {
        identity: identity.clone(),
        reservation: returned.clone(),
    })
}
