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
        if !decisions.is_empty() {
            projection
                .check_task_context_capacity(&identity.task, 0, decisions.len())
                .map_err(super::orchestration::scheduler_error)?;
        }
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
    /// Launch admission in `reserve_native_effect` covers both distinct intents.
    /// Replaying the same intent cannot consume another slot.
    #[cfg(target_os = "linux")]
    pub(super) fn publish_native_attempt_decision(
        &mut self,
        producer: &HostAuth,
        mut projection: RunProjection,
        identity: &ExecutionIdentity,
        decision: ObjectRef,
    ) -> Result<(), HostError> {
        let task = projection
            .tasks
            .iter_mut()
            .find(|task| task.id == identity.task)
            .ok_or_else(|| refused("native task disappeared"))?;
        if !task.progress.decisions.contains(&decision) {
            task.progress.decisions.push(decision.clone());
        }
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let issued = self.issue_metadata(index, None)?;
        self.publish_native_issued(producer, projection, identity, vec![decision], issued)?;
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
        if let Err(error) = self.append_pending_edit_applications(&mut projection) {
            self.message = format!("Edit audit publication is still pending: {error}");
            eprintln!("{}", self.message);
        }
        let update = native_update(
            projection,
            identity,
            decisions,
            issued,
            self.owner()?.metadata_generation()?,
            self.owner()?
                .orchestration_cut(&identity.run)?
                .map(|cut| cut.segment),
        )?;
        let (receipt, _) = self.publish_orchestration_transition(producer, &update, None)?;
        Ok((update, receipt))
    }
}

/// Construct the exact native successor against an explicit publication fence.
pub(super) fn native_update(
    mut projection: RunProjection,
    identity: &ExecutionIdentity,
    decisions: Vec<ObjectRef>,
    issued: super::orchestration::MetadataIssuer,
    generation: u64,
    previous: Option<ObjectRef>,
) -> Result<ValidatedRunUpdate, HostError> {
    if matches!(
        projection.wait,
        crate::orchestration::orchestration_records::RunWait::Replan(_)
    ) {
        crate::orchestration::tasks::refresh_wait(&mut projection);
    }
    let mut envelope = OrchestrationEnvelope {
        id: issued.envelope.id,
        digest: serde_json::from_value(serde_json::json!("0".repeat(64)))?,
        project: issued.envelope.project_id,
        expected_generation: generation,
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
        previous,
        events: vec![OrchestrationEvent {
            command: envelope.id.clone(),
            sequence: projection.sequence,
            projection: projection.clone(),
        }],
    };
    ValidatedRunUpdate::new(
        &envelope.project,
        envelope.expected_generation,
        &envelope,
        projection,
        segment,
        CommandDecision::Applied,
    )
    .map_err(refused)
}
