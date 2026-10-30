//! One H1 publication boundary for native effects and task checkpoints.
use super::{HostAuth, HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::{
    ExecutionIdentity,
    orchestration_commit::{CommandDecision, ValidatedRunUpdate},
    orchestration_records::{
        AuthorityBinding, EventSegment, OrchestrationCommand, OrchestrationEnvelope,
        OrchestrationEvent, RunProjection,
    },
};
use kinewright_project::bundle_records::ObjectRef;
fn refused(reason: impl std::fmt::Debug) -> HostError {
    HostRefusal::new("native_invocation", &format!("{reason:?}")).into()
}
impl HostCommandOwner {
    pub(super) fn publish_native(
        &mut self,
        producer: &HostAuth,
        mut projection: RunProjection,
        identity: &ExecutionIdentity,
        decisions: Vec<ObjectRef>,
    ) -> Result<(), HostError> {
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let issued = self.issue_metadata(index, None)?;
        projection
            .tasks
            .iter_mut()
            .find(|task| task.id == identity.task)
            .ok_or_else(|| refused("native task disappeared"))?
            .progress
            .decisions
            .extend(decisions.clone());
        self.publish_native_issued(producer, projection, identity, decisions, issued)?;
        Ok(())
    }
    pub(super) fn publish_native_issued(
        &mut self,
        producer: &HostAuth,
        mut projection: RunProjection,
        identity: &ExecutionIdentity,
        decisions: Vec<ObjectRef>,
        issued: super::orchestration::MetadataIssuer,
    ) -> Result<
        (
            ValidatedRunUpdate,
            crate::orchestration::orchestration_commit::OrchestrationReceipt,
        ),
        HostError,
    > {
        let mut envelope = OrchestrationEnvelope {
            id: issued.envelope.id,
            digest: serde_json::from_value(serde_json::json!("0".repeat(64)))?,
            project: issued.envelope.project_id,
            expected_generation: self.owner()?.metadata_generation()?,
            authority: AuthorityBinding {
                grant_ref: issued.grant_ref,
                owner_epoch: issued.envelope.owner_epoch,
                endpoint: None,
                attempt: None,
            },
            command: OrchestrationCommand::RecordContext {
                task: identity.task.clone(),
                feedback: vec![],
                decisions,
            },
        };
        envelope.digest = envelope.computed_digest().map_err(refused)?;
        projection.sequence = projection
            .sequence
            .checked_add(1)
            .ok_or_else(|| refused("native sequence capacity"))?;
        projection.control_revision = envelope.identity("control-revision").map_err(refused)?;
        let segment = EventSegment {
            run: identity.run.clone(),
            first_sequence: projection.sequence,
            previous: self
                .owner()?
                .orchestration_cut(&identity.run)?
                .map(|cut| cut.segment),
            events: vec![OrchestrationEvent {
                command: envelope.id.clone(),
                sequence: projection.sequence,
                projection: projection.clone(),
            }],
        };
        let update = ValidatedRunUpdate::new(
            &envelope.project,
            envelope.expected_generation,
            &envelope,
            projection,
            segment,
            CommandDecision::Applied,
        )
        .map_err(refused)?;
        let (receipt, _) = self.publish_orchestration_transition(producer, &update, None)?;
        Ok((update, receipt))
    }
}
