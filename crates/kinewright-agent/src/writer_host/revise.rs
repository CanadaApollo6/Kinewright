//! S4's `ReviseCut` port: one submitted batch becomes one Director run.
//!
//! The host chooses the Director provider (from the person's scheduler setting)
//! and its scope (the whole document at the forked candidate). The App only names
//! the batch's retained `ReviseRequest`. Batch consumption and the `RunIndex` entry
//! publish together in the run's first owner publication, before `Accepted`.
#![allow(clippy::missing_errors_doc)]
use super::{
    AttachmentRole, HostCommandOwner, HostError, HostRefusal,
    orchestration::{PendingPublication, ScopedOrchestrationRequest},
};
use crate::orchestration::{
    director_setting::configured_director,
    orchestration_records::{
        AuthorityBinding, OrchestrationCommand, OrchestrationEnvelope, ProviderSelectionRecord,
        Role, RunId, StartRun, TaskBrief,
    },
    policy,
};
use kinewright_project::{
    bundle::BundleError,
    bundle_records::{BranchCheckpoint, ObjectRef},
    studio_records::{self, ReviseRequest, ReviseResult},
};
use serde::{Deserialize, Serialize};

const RUN_SLOTS: usize = 64;

/// The Director's assignment names the exact request, command id included.
fn assignment_bytes(request: &ReviseRequest) -> Result<Vec<u8>, HostError> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "kind": "revise_cut",
        "request": request,
    }))?)
}
fn evidence_ref(bytes: &[u8]) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
    }
}
fn waiting(reason: &str) -> ReviseResult {
    ReviseResult::Waiting {
        reason: reason.into(),
    }
}

impl HostCommandOwner {
    pub(super) fn revise_cut(
        &mut self,
        index: usize,
        request: &ReviseRequest,
    ) -> Result<ReviseResult, HostError> {
        if self.attachments[index].role == AttachmentRole::Producer {
            return Err(HostRefusal::new(
                "role_refused",
                "a task Producer cannot start a Revise cut",
            )
            .into());
        }
        // An uncertain earlier publication of this batch's run retries its
        // original envelope; nothing new is built until it settles. Only the
        // exact request its Director assignment names may retry it.
        if let Some(PendingPublication::Wire(original, actor)) = &self.hooks.pending
            && original.consume_batch.as_ref() == Some(&request.batch_id)
            && *actor == self.attachments[index].id
        {
            let OrchestrationCommand::StartRun(start) = &original.envelope.command else {
                return Err(HostRefusal::new(
                    "orchestration_refused",
                    "original uncertain command must reconcile first",
                )
                .into());
            };
            if start.director.assignment != evidence_ref(&assignment_bytes(request)?) {
                return Err(HostRefusal::new(
                    "id_payload_conflict",
                    "Revise cut retry changed the submitted request",
                )
                .into());
            }
            let original = (**original).clone();
            let run: RunId = original.envelope.identity("run").map_err(|reason| {
                HostRefusal::new("orchestration_refused", &format!("{reason:?}"))
            })?;
            self.dispatch_orchestration(index, &original)?;
            return accepted(&run, request);
        }
        let index_now = self.owner()?.feedback_index()?;
        let named = index_now
            .batches
            .values()
            .find(|batch| batch.request.command_id == request.command_id);
        let submitted = match (named, index_now.batches.get(&request.batch_id)) {
            (Some(named), _) if named.request != *request => None,
            (_, Some(submitted)) => Some(submitted.clone()),
            (None | Some(_), None) => {
                return Err(
                    HostRefusal::new("unknown_batch", "this batch is not submitted").into(),
                );
            }
        };
        drop(index_now);
        let Some(submitted) = submitted.filter(|submitted| submitted.request == *request) else {
            return Err(HostRefusal::new(
                "id_payload_conflict",
                "Revise cut retry changed the submitted request",
            )
            .into());
        };
        if let Some(consumed) = &submitted.consumed_by {
            return self.replay_revise(request, consumed);
        }
        if self.attachments[index].role == AttachmentRole::Viewer {
            return Ok(waiting(
                "Your notes are saved. An editor of this project starts the revised cut.",
            ));
        }
        let (_, manual) = self.owner()?.manual_checkpoint()?;
        if manual.head_id != request.expected_candidate_head {
            return Ok(ReviseResult::Conflict);
        }
        let provider = match configured_director(self.config.scheduler_settings.as_deref()) {
            Ok(provider) => provider,
            Err(reason) => return Ok(waiting(&reason)),
        };
        if self.hooks.runs.len() >= RUN_SLOTS {
            return Ok(waiting(
                "Every run slot is in use. Your notes are saved. Revise cut again when a run finishes.",
            ));
        }
        let brief = self.revise_brief(request, &submitted.batch_ref, provider)?;
        let subject = request.command_id.issuer.clone();
        let issued = if self.require_metadata_issuer(index, &subject).is_ok() {
            self.issue_metadata(index, Some(subject))?
        } else {
            self.issue_metadata(index, None)?
        };
        let mut envelope = OrchestrationEnvelope {
            id: issued.envelope.id,
            digest: serde_json::from_value(serde_json::Value::String("0".repeat(64)))?,
            project: issued.envelope.project_id,
            expected_generation: issued.root_generation,
            authority: AuthorityBinding {
                grant_ref: issued.grant_ref,
                owner_epoch: issued.envelope.owner_epoch,
                endpoint: None,
                attempt: None,
            },
            command: OrchestrationCommand::StartRun(Box::new(StartRun { director: brief })),
        };
        envelope.digest = envelope
            .computed_digest()
            .map_err(|reason| HostRefusal::new("orchestration_refused", &format!("{reason:?}")))?;
        let run: RunId = envelope
            .identity("run")
            .map_err(|reason| HostRefusal::new("orchestration_refused", &format!("{reason:?}")))?;
        let scoped = ScopedOrchestrationRequest {
            run: None,
            envelope,
            consume_batch: Some(request.batch_id.clone()),
        };
        self.dispatch_orchestration(index, &scoped)?;
        accepted(&run, request)
    }

    /// A consumed batch answers with its run when that run's Director assignment
    /// names this exact request, command id included.
    fn replay_revise(
        &self,
        request: &ReviseRequest,
        consumed: &studio_records::RunId,
    ) -> Result<ReviseResult, HostError> {
        let run: RunId = same_identity(consumed)?;
        let expected = evidence_ref(&assignment_bytes(request)?);
        let named = self
            .owner()?
            .orchestration_projection(&run)?
            .and_then(|projection| projection.tasks.first().map(|task| task.brief.clone()))
            .is_some_and(|brief| brief.role == Role::Director && brief.assignment == expected);
        if !named {
            return Err(HostRefusal::new(
                "batch_consumed",
                "this batch already started a different run",
            )
            .into());
        }
        accepted(&run, request)
    }

    /// Fork the candidate from the manual head the batch was submitted at, then
    /// admit the plan, script and assignment that name the batch. The fork has a
    /// fresh head identity; the run's publication checks it is unchanged from
    /// the submitted head.
    fn revise_brief(
        &mut self,
        request: &ReviseRequest,
        batch_ref: &ObjectRef,
        provider: ProviderSelectionRecord,
    ) -> Result<TaskBrief, HostError> {
        let owner = self.owner_mut()?;
        let name = format!("revise-{}", request.batch_id.as_str());
        let mut candidate = match owner.fork_durable_branch(&name) {
            Ok(candidate) => candidate,
            Err(BundleError::HistoryChanged) => {
                return Err(HostRefusal::new(
                    "unsaved_edits",
                    "Save the project, then Revise cut again. Your notes are saved.",
                )
                .into());
            }
            Err(error) => return Err(error.into()),
        };
        owner.persist_durable_branch(&mut candidate)?;
        let base = owner.candidate_binding(candidate.branch_id())?;
        let document = (*owner.durable_branch_history(&candidate)?.document).clone();
        let inputs = owner
            .store()?
            .read_record::<BranchCheckpoint>(&base.checkpoint)?
            .input_manifest_ref
            .clone();
        let plan = owner.admit_host_grant(&serde_json::to_vec(&serde_json::json!({
            "kind": "revise_cut_plan",
            "batch_id": request.batch_id,
            "batch_ref": batch_ref,
        }))?)?;
        let assignment = owner.admit_host_grant(&assignment_bytes(request)?)?;
        let scope = policy::director_scope(&document, base)
            .map_err(|why| HostRefusal::new("scope_refused", &why))?;
        Ok(TaskBrief {
            plan: plan.clone(),
            script: plan,
            assignment,
            inputs,
            role: Role::Director,
            provider,
            scope,
        })
    }
}

fn accepted(run: &RunId, request: &ReviseRequest) -> Result<ReviseResult, HostError> {
    Ok(ReviseResult::Accepted {
        run_id: same_identity(run)?,
        batch_id: request.batch_id.clone(),
    })
}
/// The scheduler and Studio name one run with the same identity string.
fn same_identity<T: for<'de> Deserialize<'de>>(run: &impl Serialize) -> Result<T, HostError> {
    Ok(serde_json::from_value(serde_json::to_value(run)?)?)
}
