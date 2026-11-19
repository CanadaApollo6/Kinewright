use super::{AttachmentRole, HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::{
    orchestration_records::{Role, RunProjection, TaskRecord},
    tasks::RuntimeTaskAuthority,
};
use kinewright_project::{
    editorial::{AuditSubmission, RulebookDecision, RulebookProposal},
    studio_records::MetadataChange,
};
use serde::Deserialize;
use serde_json::{Value, json};

fn refused(reason: &str) -> HostError {
    HostRefusal::new("editorial_scope", reason).into()
}

impl HostCommandOwner {
    #[cfg(target_os = "linux")]
    pub(super) fn task_rulebook(&self, task: &TaskRecord) -> Result<Value, HostError> {
        use kinewright_project::editorial::RulebookRevision;
        let Some(frozen) = &task.progress.frozen_rulebook else {
            return Ok(Value::Null);
        };
        let rules = frozen
            .revision
            .as_ref()
            .map(|reference| {
                self.owner()?
                    .store()?
                    .read_record::<RulebookRevision>(reference)
                    .map_err(HostError::from)
            })
            .transpose()?;
        Ok(
            json!({"revision":frozen.revision,"rules":rules.as_ref().map_or(&[][..], |r| r.rules.as_slice())}),
        )
    }

    #[cfg(target_os = "linux")]
    pub(super) fn task_editorial_versions(
        &self,
        task: &TaskRecord,
    ) -> Result<Vec<kinewright_project::studio_records::SnapshotKey>, HostError> {
        if !matches!(task.brief.role, Role::Editor | Role::Reviewer) {
            return Ok(vec![]);
        }
        Ok(self
            .owner()?
            .studio_versions_at(&task.brief.scope.base.checkpoint)?)
    }

    pub(super) fn editorial_task_tool(
        &mut self,
        state: &RunProjection,
        task: &TaskRecord,
        authority: &RuntimeTaskAuthority,
        name: &str,
        arguments: Value,
    ) -> Result<Value, HostError> {
        if self.hooks.pending.is_some() {
            return Err(kinewright_project::bundle::BundleError::Unsettled.into());
        }
        let attempt = state
            .attempts
            .iter()
            .find(|reservation| {
                reservation.identity.task == task.id
                    && reservation.identity.attempt == authority.attempt
            })
            .ok_or_else(|| refused("editorial task attempt missing"))?;
        let frozen = task
            .progress
            .frozen_rulebook
            .as_ref()
            .ok_or_else(|| refused("task rulebook was not prepared"))?;
        let change = match (name, task.brief.role) {
            ("propose_rulebook_revision", Role::Director) => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Proposal {
                    rule: String,
                }
                let proposal: Proposal = serde_json::from_value(arguments)?;
                MetadataChange::ProposeRulebook(RulebookProposal {
                    proposed_by: attempt.identity.clone(),
                    parent: frozen.revision.clone(),
                    rule: proposal.rule,
                })
            }
            ("record_editorial_audit", Role::Editor | Role::Reviewer) => {
                let submission: AuditSubmission = serde_json::from_value(arguments)?;
                let pinned = self.owner()?.read_snapshot(&submission.snapshot)?;
                let checkpoint = &pinned.descriptor.checkpoint_ref;
                if checkpoint != &task.brief.scope.base.checkpoint
                    && !(task.brief.role == Role::Editor
                        && self
                            .saved_task_history(state, &task.id)?
                            .is_some_and(|history| &history.source.checkpoint == checkpoint))
                {
                    return Err(refused("editorial audit snapshot is outside the task cut"));
                }
                let (target, part) = submission.attribute(attempt.identity.clone());
                MetadataChange::RetainEditorial {
                    target,
                    role: task.brief.role,
                    frozen: frozen.clone(),
                    part,
                }
            }
            _ => return Err(refused("tool hidden for durable task role")),
        };
        self.owner_mut()?.publish_editorial(change)?;
        Ok(json!({"retained":true}))
    }

    pub(super) fn decide_rulebook(
        &mut self,
        index: usize,
        decision: RulebookDecision,
    ) -> Result<(), HostError> {
        let attachment = &self.attachments[index];
        if attachment.role != AttachmentRole::Editor || !attachment.person_facing {
            return Err(refused("the person-facing Editor holder is required"));
        }
        if self.hooks.pending.is_some() {
            return Err(kinewright_project::bundle::BundleError::Unsettled.into());
        }
        self.owner_mut()?.decide_rulebook(decision)?;
        Ok(())
    }
}
