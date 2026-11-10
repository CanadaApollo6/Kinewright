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
use kinewright_core::{
    Command, Core, Document, Event, HistorySnapshot, Operation, Query, QueryResult,
    TimelineRevision,
};
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
    pub removed_clips: usize,
    pub removed_tracks: usize,
    pub applied: bool,
    pub issue: Option<String>,
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
    #[serde(default)]
    removed_clips: usize,
    #[serde(default)]
    removed_tracks: usize,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppliedRecord {
    kind: String,
    approval: kinewright_project::bundle_records::ObjectRef,
    revision: TimelineRevision,
}
fn problem(code: &str, reason: impl Into<String>) -> HostError {
    HostRefusal::new(code, &reason.into()).into()
}
fn missing() -> HostError {
    problem(
        "consent_missing",
        "This run started before edit permissions were recorded. Stop it and start a new run.",
    )
}
fn authority(reason: impl Into<String>) -> HostError {
    problem("consent_authority", reason)
}
fn scope(reason: impl std::fmt::Display) -> HostError {
    problem("consent_scope", format!("{reason}. Tell the Director."))
}
fn policy(reason: impl Into<String>) -> HostError {
    problem("consent_policy", reason)
}
fn lock_host<'a>(
    owner: &'a std::sync::Mutex<HostCommandOwner>,
    endpoint: &crate::orchestration::tasks::TaskEndpoint,
) -> Result<std::sync::MutexGuard<'a, HostCommandOwner>, HostError> {
    let started = std::time::Instant::now();
    loop {
        endpoint
            .check_live()
            .map_err(|why| super::task_authority_error(&why))?;
        match owner.try_lock() {
            Ok(host) => {
                endpoint
                    .check_live()
                    .map_err(|why| super::task_authority_error(&why))?;
                return Ok(host);
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(authority("The host is unavailable. Tell the Director."));
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if started.elapsed() >= std::time::Duration::from_millis(350) {
                    return Err(unavailable());
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}
#[cfg(all(test, target_os = "linux"))]
pub(crate) enum ConsentFixture {
    Missing,
    Record(kinewright_project::bundle_records::ObjectRef),
}
fn consent_reference<'a>(
    host: &'a HostCommandOwner,
    state: &'a RunProjection,
) -> Result<&'a kinewright_project::bundle_records::ObjectRef, HostError> {
    #[cfg(all(test, target_os = "linux"))]
    let value = match &host.hooks.consent_fixture {
        Some(ConsentFixture::Missing) => None,
        Some(ConsentFixture::Record(reference)) => Some(reference),
        None => state.edit_consent.as_ref(),
    };
    #[cfg(not(all(test, target_os = "linux")))]
    let value = {
        let _ = host;
        state.edit_consent.as_ref()
    };
    value.ok_or_else(missing)
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
    class_name(operation_name(operation))
}
fn class_name(name: &str) -> Option<&'static str> {
    match name {
        "delete_clip" => Some("clip_removal"),
        "ripple_delete_clip" => Some("ripple_delete"),
        "remove_track" => Some("track_removal"),
        "trim_clip" => Some("trim"),
        "add_track"
        | "add_clip"
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
fn inverse_class(op: &Operation) -> Option<&'static str> {
    let forward = class(op)?;
    Some(match operation_name(op) {
        "add_track" => "track_removal",
        "add_clip"
        | "add_title"
        | "add_solid_clip"
        | "add_adjustment_clip"
        | "add_freeze_frame" => "clip_removal",
        "delete_clip" | "ripple_delete_clip" | "remove_track" => "timeline_edit",
        _ => forward,
    })
}
impl HostCommandOwner {
    pub(crate) fn seed_edit_consent(&mut self, state: &mut RunProjection) -> Result<(), HostError> {
        let root = state
            .tasks
            .first()
            .ok_or_else(|| authority("Task authority is unavailable. Tell the Director."))?;
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
        #[cfg(all(test, target_os = "linux"))]
        if let Some(value) = &self.hooks.consent_fixture {
            state.edit_consent = match value {
                ConsentFixture::Missing => None,
                ConsentFixture::Record(r) => Some(r.clone()),
            };
            return Ok(());
        }
        state.edit_consent = Some(
            self.owner_mut()?
                .admit_host_grant(&serde_json::to_vec(&consent)?)?,
        );
        Ok(())
    }
    pub(crate) fn edit_approval_details(&self, state: &RunProjection) -> Vec<EditApproval> {
        let mut applied = BTreeSet::new();
        let mut integrity = false;
        for reference in &state.edit_applications {
            let read = (|| -> Result<AppliedRecord, HostError> {
                Ok(serde_json::from_slice(
                    &self.owner()?.store()?.read_evidence(reference)?,
                )?)
            })();
            match read {
                Ok(r)
                    if r.kind == "studio_edit_applied_v1"
                        && state.edit_approvals.contains(&r.approval) =>
                {
                    applied.insert(r.approval);
                }
                _ => integrity = true,
            }
        }
        let mut rows = Vec::new();
        for reference in &state.edit_approvals {
            let read = (|| -> Result<ApprovalRecord, HostError> {
                let record: ApprovalRecord =
                    serde_json::from_slice(&self.owner()?.store()?.read_evidence(reference)?)?;
                if record.kind != "studio_edit_approval_v1"
                    || record.binding.run != state.run
                    || record.binding.project != self.owner()?.active_manual_project()?
                    || record.digest != digest(&record.binding)?
                {
                    return Err(scope("Edit record integrity check failed"));
                }
                Ok(record)
            })();
            match read {
                Ok(r) => rows.push(EditApproval {
                    task: r.binding.task,
                    removed_clips: r.removed_clips,
                    removed_tracks: r.removed_tracks,
                    digest: r.digest,
                    applied: applied.contains(reference),
                    issue: None,
                }),
                Err(_) => integrity = true,
            }
        }
        if integrity {
            rows.push(EditApproval {
                task: state.tasks[0].id.clone(),
                removed_clips: 0,
                removed_tracks: 0,
                digest: String::new(),
                applied: false,
                issue: Some("Some edit records could not be verified.".into()),
            });
        }
        rows
    }
}

#[derive(Serialize)]
enum ConsentEdit<'a> {
    Batch(&'a [Operation]),
    History {
        before: Box<HistorySnapshot>,
        after: Box<HistorySnapshot>,
        operations: Vec<Operation>,
        redo: bool,
    },
}
impl ConsentEdit<'_> {
    fn operations(&self) -> &[Operation] {
        match self {
            Self::Batch(ops) => ops,
            Self::History { operations, .. } => operations,
        }
    }
    fn after(&self, before: &Document) -> Result<Document, HostError> {
        match self {
            Self::Batch(ops) => {
                let mut doc = before.clone();
                kinewright_core::apply_batch(&mut doc, ops).map_err(|e| policy(e.to_string()))?;
                Ok(doc)
            }
            Self::History { after, .. } => Ok((*after.document).clone()),
        }
    }
    fn classes(&self, before: &Document) -> Result<BTreeSet<&'static str>, HostError> {
        let mut classes = BTreeSet::new();
        for op in self.operations() {
            let forward = class(op).ok_or_else(refused)?;
            let inverse = matches!(self, Self::History { redo: false, .. });
            let c = if inverse {
                inverse_class(op).ok_or_else(refused)?
            } else {
                forward
            };
            classes.insert(c);
        }
        let (clips, tracks) = removed(before, &self.after(before)?);
        if clips > 0 {
            classes.insert("clip_removal");
        }
        if tracks > 0 {
            classes.insert("track_removal");
        }
        Ok(classes)
    }
    fn apply(
        &self,
        endpoint: &crate::orchestration::tasks::TaskEndpoint,
        core: &Core,
        revision: TimelineRevision,
    ) -> Result<EditOutcome, HostError> {
        match self {
            Self::Batch(ops) => Ok(EditOutcome::Batch(Box::new(core.request(
                Command::DoBatchIfRevision {
                    expected: revision,
                    operations: ops.to_vec(),
                },
            )))),
            Self::History {
                before,
                after,
                redo,
                ..
            } => {
                if core.export_history().map_err(|e| policy(e.to_string()))? != **before {
                    return Err(stale());
                }
                let actual = endpoint
                    .history_step(core, revision, *redo)
                    .map_err(policy)?;
                if actual != **after {
                    return Err(policy("History differs from the approved transition"));
                }
                Ok(EditOutcome::History(actual))
            }
        }
    }
}
enum EditOutcome {
    Batch(Box<Result<Event, kinewright_core::CoreDisconnected>>),
    History(HistorySnapshot),
}
impl EditOutcome {
    fn applied(&self) -> bool {
        match self {
            Self::Batch(event) => matches!(event.as_ref(), Ok(Event::DocumentChanged { .. })),
            Self::History(_) => true,
        }
    }
}

fn removed(before: &Document, after: &Document) -> (usize, usize) {
    (
        before
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| after.clip(c.id).is_none())
            .count(),
        before
            .tracks
            .iter()
            .filter(|t| !after.tracks.iter().any(|a| a.id == t.id))
            .count(),
    )
}

impl TaskMcp {
    fn batch_binding(
        &self,
        host: &HostCommandOwner,
        revision: TimelineRevision,
        edit: &ConsentEdit<'_>,
        description: &str,
    ) -> Result<(BatchBinding, RunProjection), HostError> {
        let (_, state, task) = host.resolve_task_mcp(&self.auth, &self.endpoint)?;
        consent_reference(host, &state)?;
        let record = state
            .tasks
            .iter()
            .find(|t| t.id == task)
            .ok_or_else(|| authority("Task authority is unavailable. Tell the Director."))?;
        let turn = record
            .progress
            .turn
            .clone()
            .filter(|t| crate::orchestration::requests::live_turn(&state, t).is_ok())
            .ok_or_else(|| {
                problem(
                    "consent_turn",
                    "The task has no live turn. Tell the Director.",
                )
            })?;
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
                batch_sha256: digest(edit)?,
                description: description.into(),
            },
            state,
        ))
    }
    /// The ingress guard already serializes this endpoint. Never wait for the
    /// host mutex without a deadline: a freeze may be draining that same guard.
    fn consent_host(
        &self,
    ) -> Result<std::sync::Arc<std::sync::Mutex<HostCommandOwner>>, HostError> {
        self.host
            .upgrade()
            .ok_or_else(|| authority("The host closed. Tell the Director."))
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
        let host = lock_host(&owner, &self.endpoint)?;
        let (binding, state) = self.batch_binding(
            &host,
            revision,
            &ConsentEdit::Batch(operations),
            description,
        )?;
        self.validate_consent(
            &host,
            &state,
            &binding,
            core,
            before,
            &ConsentEdit::Batch(operations),
        )
    }
    fn validate_consent(
        &self,
        host: &HostCommandOwner,
        state: &RunProjection,
        binding: &BatchBinding,
        core: &Core,
        before: &Document,
        edit: &ConsentEdit<'_>,
    ) -> Result<(), HostError> {
        let reference = consent_reference(host, state)?;
        let consent: RunEditConsent =
            serde_json::from_slice(&host.owner()?.store()?.read_evidence(reference)?)?;
        let task = state
            .tasks
            .iter()
            .find(|t| t.id == binding.task)
            .ok_or_else(|| authority("Task authority is unavailable. Tell the Director."))?;
        let manual = host.owner()?.manual_checkpoint()?.1;
        let runtime = host
            .hooks
            .runs
            .get(&state.run)
            .and_then(|r| r.tasks.get(&task.id))
            .ok_or_else(|| authority("Task authority is unavailable. Tell the Director."))?;
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
        {
            return Err(scope(
                "The edit scope, candidate or actor no longer matches the run",
            ));
        }
        if !edit
            .classes(before)?
            .iter()
            .all(|c| consent.classes.contains(*c))
        {
            return Err(refused());
        }
        let task_policy = TaskPolicy {
            role: task.brief.role,
            scope: task.brief.scope.clone(),
        };
        match edit {
            ConsentEdit::Batch(ops) => task_policy.validate_batch(before, ops).map_err(policy)?,
            ConsentEdit::History {
                before,
                after,
                operations,
                redo,
            } => {
                task_policy
                    .validate_batch(
                        if *redo {
                            &before.document
                        } else {
                            &after.document
                        },
                        operations,
                    )
                    .map_err(policy)?;
                task_policy
                    .validate_history_change(&before.document, &after.document)
                    .map_err(policy)?;
            }
        }
        let (actual, document) = host.context_snapshot(&state.run, &task.id)?;
        if actual != binding.revision || document.as_ref() != before {
            return Err(scope("The task snapshot changed"));
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
    ) -> Result<Result<Event, kinewright_core::CoreDisconnected>, HostError> {
        match self.authorize_edit(
            broker,
            core,
            revision,
            before,
            &ConsentEdit::Batch(operations),
            description,
        )? {
            EditOutcome::Batch(event) => Ok(*event),
            EditOutcome::History(_) => unreachable!(),
        }
    }
    pub(crate) fn history_with_edit_consent(
        &self,
        core: &Core,
        revision: TimelineRevision,
        redo: bool,
    ) -> Result<HistorySnapshot, HostError> {
        {
            let owner = self.consent_host()?;
            let host = lock_host(&owner, &self.endpoint)?;
            self.batch_binding(&host, revision, &ConsentEdit::Batch(&[]), "History step")?;
        }
        let (before, after, operations) = self
            .endpoint
            .preview_history_step(core, revision, redo)
            .map_err(policy)?;
        let (clips, tracks) = removed(&before.document, &after.document);
        let description = (clips > 0 || tracks > 0)
            .then(|| format!("History removes {clips} clips and {tracks} tracks"));
        let document = before.document.clone();
        let edit = ConsentEdit::History {
            before: Box::new(before),
            after: Box::new(after),
            operations,
            redo,
        };
        let broker = self
            .confirmations
            .as_ref()
            .ok_or_else(|| authority("Task broker unavailable. Tell the Director."))?;
        match self.authorize_edit(
            broker,
            core,
            revision,
            &document,
            &edit,
            description.as_deref(),
        )? {
            EditOutcome::History(history) => Ok(history),
            EditOutcome::Batch(_) => unreachable!(),
        }
    }
    fn authorize_edit(
        &self,
        broker: &ConfirmationBroker,
        core: &Core,
        revision: TimelineRevision,
        before: &Document,
        edit: &ConsentEdit<'_>,
        description: Option<&str>,
    ) -> Result<EditOutcome, HostError> {
        let counts = removed(before, &edit.after(before)?);
        let actual_description = (counts.0 > 0 || counts.1 > 0)
            .then(|| format!("Remove {} clips and {} tracks", counts.0, counts.1));
        let description = actual_description.as_deref().or(description);
        let owner = self.consent_host()?;
        if description.is_none() {
            let host = lock_host(&owner, &self.endpoint)?;
            let (binding, state) =
                self.batch_binding(&host, revision, edit, "Reversible private-branch edit")?;
            self.validate_consent(&host, &state, &binding, core, before, edit)?;
            return edit.apply(&self.endpoint, core, revision);
        }
        let description = description.ok_or_else(refused)?;
        let binding = {
            let host = lock_host(&owner, &self.endpoint)?;
            self.batch_binding(&host, revision, edit, description)?.0
        };
        let request_digest = digest(&binding)?;
        let request = broker
            .request_task(&serde_json::to_value(&binding)?, description.into())
            .map_err(|e| problem(e.code(), e.to_string()))?;
        let mut host = lock_host(&owner, &self.endpoint)?;
        let (current, mut state) =
            self.batch_binding(&host, revision, edit, &request.description)?;
        if request.digest != digest(&current)? {
            return Err(stale());
        }
        self.validate_consent(&host, &state, &current, core, before, edit)?;
        let answer = broker
            .claim_task(&request)
            .map_err(|e| problem("consent_broker", e))?;
        if state.edit_approvals.len() >= ITEM_LIMIT {
            return Err(problem(
                "consent_capacity",
                "The run's edit-approval history is full. Tell the Director; this edit was not applied.",
            ));
        }
        let (removed_clips, removed_tracks) = removed(before, &edit.after(before)?);
        let record = ApprovalRecord {
            kind: "studio_edit_approval_v1".into(),
            binding: current,
            consent: state.edit_consent.clone().ok_or_else(missing)?,
            digest: request_digest,
            removed_clips,
            removed_tracks,
        };
        let reference = host
            .owner_mut()?
            .admit_host_grant(&serde_json::to_vec(&record)?)?;
        state.edit_approvals.push(reference.clone());
        let producer = host.run_producer(&state.run)?;
        let execution = state
            .attempts
            .iter()
            .find(|a| a.identity.task == binding.task && a.identity.attempt == binding.attempt)
            .ok_or_else(|| authority("The task attempt ended"))?
            .identity
            .clone();
        host.publish_native(&producer, state.clone(), &execution, vec![])?;
        self.endpoint.check_live().map_err(authority)?;
        answer.approve().map_err(|e| problem("consent_broker", e))?;
        #[cfg(test)]
        if let Some(hook) = host.hooks.consent_apply_hook.take() {
            hook(core);
        }
        let outcome = edit.apply(&self.endpoint, core, revision)?;
        if outcome.applied() {
            // The original successful Core outcome survives an audit-publication failure.
            let _ = (|| -> Result<(), HostError> {
                let Event::QueryResult(QueryResult::Snapshot { revision, .. }) = core
                    .request(Command::Query(Query::Snapshot))
                    .map_err(|e| authority(e.to_string()))?
                else {
                    return Err(stale());
                };
                let marker = AppliedRecord {
                    kind: "studio_edit_applied_v1".into(),
                    approval: reference,
                    revision,
                };
                state = host
                    .owner()?
                    .orchestration_projection(&state.run)?
                    .ok_or_else(|| authority("Run unavailable"))?;
                state.edit_applications.push(
                    host.owner_mut()?
                        .admit_host_grant(&serde_json::to_vec(&marker)?)?,
                );
                host.publish_native(&producer, state, &execution, vec![])?;
                Ok(())
            })();
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn te15_b21f_every_operation_is_decided() {
        const REFUSED: &[&str] = &[
            "add_asset",
            "relink_asset",
            "set_asset_color_description",
            "set_color_context",
            "upsert_bin",
            "remove_bin",
            "set_asset_bin",
            "upsert_string_out",
            "remove_string_out",
            "upsert_sync_group",
            "remove_sync_group",
            "upsert_audio_bus",
            "remove_audio_bus",
            "set_audio_master",
            "set_pan_law",
            "convert_legacy_look",
            "add_lut_asset",
            "remove_lut_asset",
        ];
        for name in crate::orchestration::policy::OPERATION_NAMES {
            assert_ne!(
                class_name(name).is_some(),
                REFUSED.contains(name),
                "FOL_OPERATION_DECISION {name}"
            );
        }
        assert!(
            REFUSED
                .iter()
                .all(|name| crate::orchestration::policy::OPERATION_NAMES.contains(name))
        );
    }
    #[test]
    fn te15_b21f_add_clip_inverse_is_removal() {
        let op = Operation::AddClip {
            track: kinewright_core::TrackId(1),
            asset: kinewright_core::AssetId(1),
            at: kinewright_core::TimeCode(0),
            source: kinewright_core::TimeCode(0)..kinewright_core::TimeCode(1),
        };
        assert_eq!(
            inverse_class(&op),
            Some("clip_removal"),
            "FOL_ADD_CLIP_INVERSE"
        );
    }
}
