//! Recorded consent authorizes exact reversible batches on private task branches.
use super::TaskMcp;
use crate::writer_host::{HostCommandOwner, HostError, HostRefusal};
use crate::{
    ConfirmationBroker,
    orchestration::{
        orchestration_records::{
            AttemptId, EndpointId, ITEM_LIMIT, Role, RunId, RunProjection, TaskId, TaskTurn,
        },
        policy::{TaskPolicy, operation_name},
    },
};
use kinewright_core::{Core, Document, Operation, TimelineRevision};
use kinewright_project::bundle_records::{BranchId, ProjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(crate) const REFUSAL: &str = "This edit needs the person's approval, which this run does not have. Plan the edit without it, or tell the Director.";
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunEditConsent {
    kind: String,
    project: ProjectId,
    run: RunId,
    candidate: BranchId,
    tracks: BTreeSet<String>,
    classes: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditApproval {
    pub task: TaskId,
    pub description: String,
    pub digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchBinding {
    project: ProjectId,
    run: RunId,
    task: TaskId,
    attempt: AttemptId,
    endpoint: EndpointId,
    owner_epoch: u64,
    turn: TaskTurn,
    revision: TimelineRevision,
    batch_sha256: String,
    description: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalRecord {
    kind: String,
    binding: BatchBinding,
    consent: kinewright_project::bundle_records::ObjectRef,
    digest: String,
}
fn refused() -> HostError {
    HostRefusal::new("consent_required", REFUSAL).into()
}
fn stale() -> HostError {
    HostRefusal::new("consent_stale", "This edit's approval no longer matches the live task and exact plan. Prepare it again, or tell the Director.").into()
}
fn unavailable() -> HostError {
    HostRefusal::new(
        "task_busy",
        "The host cannot check this edit yet. Retry this plan without changing it.",
    )
    .into()
}
fn digest(value: &impl Serialize) -> Result<String, HostError> {
    Ok(kinewright_media::sha256_bytes(&serde_json::to_vec(
        &serde_json::to_value(value)?,
    )?))
}
fn class(operation: &Operation) -> Option<&'static str> {
    match operation_name(operation) {
        "delete_clip" => Some("clip_removal"),
        "ripple_delete_clip" => Some("ripple_delete"),
        "remove_track" => Some("track_removal"),
        "trim_clip" => Some("trim"),
        "add_clip"
        | "add_title"
        | "split_clip"
        | "move_clip"
        | "three_point_edit"
        | "patched_three_point_edit"
        | "slip_clip"
        | "roll_edit"
        | "slide_clip"
        | "replace_clip"
        | "fit_to_fill"
        | "ripple_insert_gap"
        | "link_clips"
        | "unlink_clips"
        | "add_marker"
        | "remove_marker"
        | "move_marker"
        | "add_effect"
        | "insert_effect"
        | "remove_effect"
        | "set_effect_param"
        | "set_effect_keyframes"
        | "clear_effect_keyframes"
        | "upsert_effect_keyframe"
        | "remove_effect_keyframe"
        | "set_effect_enabled"
        | "set_clip_enabled"
        | "set_clip_enabled_curve"
        | "copy_clip_attributes"
        | "set_title_param"
        | "set_clip_audio"
        | "set_clip_gain_envelope"
        | "add_transition"
        | "remove_transition"
        | "set_marker_param"
        | "add_freeze_frame"
        | "set_clip_speed"
        | "set_clip_blend_mode"
        | "add_adjustment_clip"
        | "add_solid_clip"
        | "set_solid_color"
        | "set_track_sync_lock"
        | "set_track_mix"
        | "set_track_automation" => Some("timeline_edit"),
        _ => None,
    }
}
impl HostCommandOwner {
    pub(crate) fn seed_edit_consent(&mut self, state: &mut RunProjection) -> Result<(), HostError> {
        let root = state.tasks.first().ok_or_else(refused)?;
        let consent = RunEditConsent {
            kind: "studio_run_edit_consent_v1".into(),
            project: self.owner()?.active_manual_project()?,
            run: state.run.clone(),
            candidate: root.brief.scope.base.branch.clone(),
            tracks: root.brief.scope.tracks.clone(),
            classes: [
                "clip_removal",
                "track_removal",
                "ripple_delete",
                "trim",
                "timeline_edit",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        };
        state.edit_consent = Some(
            self.owner_mut()?
                .admit_host_grant(&serde_json::to_vec(&consent)?)?,
        );
        Ok(())
    }
    pub(crate) fn edit_approval_details(
        &self,
        state: &RunProjection,
    ) -> Result<Vec<EditApproval>, HostError> {
        state
            .edit_approvals
            .iter()
            .map(|reference| {
                let bytes = self.owner()?.store()?.read_evidence(reference)?;
                let record: ApprovalRecord = serde_json::from_slice(&bytes)?;
                if record.kind != "studio_edit_approval_v1"
                    || record.binding.run != state.run
                    || record.binding.project != self.owner()?.active_manual_project()?
                    || record.digest != digest(&record.binding)?
                {
                    return Err(refused());
                }
                Ok(EditApproval {
                    task: record.binding.task,
                    description: record.binding.description,
                    digest: record.digest,
                })
            })
            .collect()
    }
}
impl TaskMcp {
    fn batch_binding(
        &self,
        host: &HostCommandOwner,
        revision: TimelineRevision,
        operations: &[Operation],
        description: &str,
    ) -> Result<(BatchBinding, RunProjection), HostError> {
        let (_, state, task) = host.resolve_task_mcp(&self.auth, &self.endpoint)?;
        let record = state
            .tasks
            .iter()
            .find(|t| t.id == task)
            .ok_or_else(refused)?;
        let turn = record
            .progress
            .turn
            .clone()
            .filter(|t| crate::orchestration::requests::live_turn(&state, t).is_ok())
            .ok_or_else(refused)?;
        let authority = self.endpoint.authority();
        Ok((
            BatchBinding {
                project: authority.project.clone(),
                run: authority.run.clone(),
                task: authority.task.clone(),
                attempt: authority.attempt.clone(),
                endpoint: authority.endpoint.clone(),
                owner_epoch: authority.owner_epoch,
                turn,
                revision,
                batch_sha256: digest(&operations)?,
                description: description.into(),
            },
            state,
        ))
    }
    /// The ingress guard already serializes this endpoint. Never wait for the
    /// host mutex here: a concurrent freeze can be draining that same guard.
    fn consent_host(
        &self,
    ) -> Result<std::sync::Arc<std::sync::Mutex<HostCommandOwner>>, HostError> {
        self.host.upgrade().ok_or_else(refused)
    }
    pub(crate) fn validate_edit_consent(
        &self,
        core: &Core,
        revision: TimelineRevision,
        before: &Document,
        operations: &[Operation],
        description: &str,
    ) -> Result<(), HostError> {
        let owner = self.consent_host()?;
        let host = owner.try_lock().map_err(|_| unavailable())?;
        let (binding, state) = self.batch_binding(&host, revision, operations, description)?;
        self.validate_consent(&host, &state, &binding, core, before, operations)
    }
    fn validate_consent(
        &self,
        host: &HostCommandOwner,
        state: &RunProjection,
        binding: &BatchBinding,
        core: &Core,
        before: &Document,
        operations: &[Operation],
    ) -> Result<(), HostError> {
        let reference = state.edit_consent.as_ref().ok_or_else(refused)?;
        let consent: RunEditConsent =
            serde_json::from_slice(&host.owner()?.store()?.read_evidence(reference)?)?;
        let task = state
            .tasks
            .iter()
            .find(|t| t.id == binding.task)
            .ok_or_else(refused)?;
        let manual = host.owner()?.manual_checkpoint()?.1;
        let runtime = host
            .hooks
            .runs
            .get(&state.run)
            .and_then(|r| r.tasks.get(&task.id))
            .ok_or_else(refused)?;
        if consent.kind != "studio_run_edit_consent_v1"
            || consent.project != binding.project
            || consent.run != state.run
            || consent.candidate != state.tasks[0].brief.scope.base.branch
            || consent.candidate != task.brief.scope.base.branch
            || consent.candidate == manual.branch_id
            || task.parent.is_none()
            || task.brief.role == Role::Reviewer
            || !self.endpoint.matches_core(core)
            || runtime.core().actor_identity() != core.actor_identity()
            || host.owner()?.core().actor_identity() == core.actor_identity()
            || !task.brief.scope.tracks.is_subset(&consent.tracks)
            || !operations
                .iter()
                .all(|op| class(op).is_some_and(|c| consent.classes.contains(c)))
        {
            return Err(refused());
        }
        TaskPolicy {
            role: task.brief.role,
            scope: task.brief.scope.clone(),
        }
        .validate_batch(before, operations)
        .map_err(|_| refused())?;
        let (actual, document) = host.context_snapshot(&state.run, &task.id)?;
        if actual != binding.revision || document.as_ref() != before {
            return Err(refused());
        }
        Ok(())
    }
    pub(crate) fn apply_with_edit_consent(
        &self,
        broker: &ConfirmationBroker,
        core: &Core,
        revision: TimelineRevision,
        before: &Document,
        operations: &[Operation],
        description: Option<&str>,
    ) -> Result<Result<kinewright_core::Event, kinewright_core::CoreDisconnected>, HostError> {
        let apply = || {
            core.request(kinewright_core::Command::DoBatchIfRevision {
                expected: revision,
                operations: operations.to_vec(),
            })
        };
        let owner = self.consent_host()?;
        if description.is_none() {
            let host = owner.try_lock().map_err(|_| unavailable())?;
            let (binding, state) = self.batch_binding(
                &host,
                revision,
                operations,
                "Reversible private-branch edit",
            )?;
            self.validate_consent(&host, &state, &binding, core, before, operations)?;
            return Ok(apply());
        }
        let description = description.ok_or_else(refused)?;
        let binding = {
            let host = owner.try_lock().map_err(|_| unavailable())?;
            self.batch_binding(&host, revision, operations, description)?
                .0
        };
        let request_digest = digest(&binding)?;
        let request = broker
            .request_task(&serde_json::to_value(&binding)?, description.into())
            .map_err(|_| refused())?;
        let mut host = owner.try_lock().map_err(|_| unavailable())?;
        let (current, mut state) =
            self.batch_binding(&host, revision, operations, &request.description)?;
        if request.digest != digest(&current)? {
            return Err(stale());
        }
        self.validate_consent(&host, &state, &current, core, before, operations)?;
        let answer = broker.claim_task(&request).map_err(|_| stale())?;
        if state.edit_approvals.len() >= ITEM_LIMIT {
            return Err(HostRefusal::new("consent_capacity", "The run's edit-approval history is full. Tell the Director; this edit was not applied.").into());
        }
        let record = ApprovalRecord {
            kind: "studio_edit_approval_v1".into(),
            binding: current,
            consent: state.edit_consent.clone().ok_or_else(refused)?,
            digest: request_digest,
        };
        let reference = host
            .owner_mut()?
            .admit_host_grant(&serde_json::to_vec(&record)?)?;
        state.edit_approvals.push(reference);
        let producer = host.run_producer(&state.run)?;
        let execution = state
            .attempts
            .iter()
            .find(|a| a.identity.task == binding.task && a.identity.attempt == binding.attempt)
            .ok_or_else(refused)?
            .identity
            .clone();
        host.publish_native(&producer, state, &execution, vec![])?;
        self.endpoint.check_live().map_err(|_| refused())?;
        answer.approve().map_err(|_| stale())?;
        // Host lifecycle commands cannot cross the consent/commit cut.
        Ok(apply())
    }
}
