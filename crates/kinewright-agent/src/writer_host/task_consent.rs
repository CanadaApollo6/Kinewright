//! Recorded consent authorizes exact reversible batches on private task branches.
use super::TaskMcp;
use crate::writer_host::{HostCommandOwner, HostError, HostRefusal};
use crate::{
    ConfirmationBroker,
    orchestration::{
        orchestration_records::{ITEM_LIMIT, Role, RunId, RunProjection},
        policy::{TaskPolicy, operation_name},
    },
};
use kinewright_core::{
    Command, Core, Document, Event, HistorySnapshot, Operation, TimelineRevision,
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
pub use kinewright_project::edit_audit::EditApproval;
use kinewright_project::edit_audit::{AppliedRecord, ApprovalRecord, BatchBinding, RemovedObject};
pub(in crate::writer_host) struct PendingEditApplication {
    run: RunId,
    prepared: kinewright_project::orchestration_commit::PreparedOrchestrationPair,
}
fn capacity(error: HostError) -> HostError {
    match &error {
        HostError::Refused(refusal) if refusal.code == "run_limit" => problem(
            "consent_capacity",
            format!(
                "This run cannot safely keep both records for another approved edit. This edit was not applied. Tell the Director to finish or Stop the run. If this run reached its own limit, start a new run after cleanup. If project storage is full, review retained project data or free storage before trying again. Cause: {error}"
            ),
        ),
        HostError::Refused(refusal) if refusal.code == "storage_backpressure" => problem(
            "consent_capacity",
            format!(
                "The project's storage capacity cannot keep both edit records. This edit was not applied. Tell the Director to finish or Stop the run, then review retained project data or free storage before trying again. Cause: {error}"
            ),
        ),
        _ => error,
    }
}
pub(in crate::writer_host) fn pending_retry_error(error: HostError) -> HostError {
    let message = format!(
        "The previous edit's audit could not be saved. This new edit was not applied. Tell the Director to finish or Stop this run and retry once the storage problem is resolved. Cause: {error}"
    );
    if let HostError::Refused(mut refusal) = error {
        refusal.message = message;
        HostError::Refused(refusal)
    } else {
        problem("consent_pending", message)
    }
}
fn evidence_reference(bytes: &[u8]) -> kinewright_project::bundle_records::ObjectRef {
    kinewright_project::bundle_records::ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
    }
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
    let reason = reason.into();
    let code = if crate::orchestration::tasks::is_liveness_refusal(&reason) {
        "consent_liveness"
    } else {
        "consent_policy"
    };
    problem(code, reason)
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
    pub(crate) fn edit_approval_summary(
        &self,
        state: &RunProjection,
    ) -> kinewright_project::edit_audit::EditAuditSummary {
        let mut live = Vec::new();
        if let Some(run) = self.hooks.runs.get(&state.run) {
            for (task, runtime) in &run.tasks {
                if !runtime.had_approved_edit() {
                    continue;
                }
                let Ok(identity) = runtime.core().history_change_id() else {
                    continue;
                };
                let Some(position) = identity.position() else {
                    continue;
                };
                // Core already shares this immutable document. Keep its exact
                // snapshot with a stable history identity, rather than copying it.
                let Ok(Event::QueryResult(kinewright_core::QueryResult::Snapshot {
                    revision,
                    document,
                })) = runtime
                    .core()
                    .request(Command::Query(kinewright_core::Query::Snapshot))
                else {
                    continue;
                };
                if revision != identity.revision()
                    || runtime.core().history_change_id().ok() != Some(identity)
                {
                    continue;
                }
                live.push(kinewright_project::edit_audit::LiveEditCut {
                    task: task.clone(),
                    cut: kinewright_project::edit_audit::KeptHistoryCut {
                        epoch: runtime.history_epoch().clone(),
                        revision: identity.revision(),
                        position,
                    },
                    document,
                });
            }
        }
        self.owner()
            .and_then(|owner| {
                owner
                    .edit_audit_summary(state, &live)
                    .map_err(HostError::from)
            })
            .unwrap_or_else(|_| {
                let mut summary = kinewright_project::edit_audit::EditAuditSummary::default();
                if let Some(root) = state.tasks.first() {
                    summary.approvals.push(EditApproval {
                        task: root.id.clone(),
                        removed_clips: 0,
                        removed_tracks: 0,
                        applied: false,
                        issue: Some("Some edit records could not be verified.".into()),
                        digest: String::new(),
                    });
                }
                summary
            })
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
            Self::Batch(ops) => {
                endpoint.check_batch_history(core, ops).map_err(policy)?;
                Ok(EditOutcome::Batch(Box::new(core.request(
                    Command::DoBatchIfRevision {
                        expected: revision,
                        operations: ops.to_vec(),
                    },
                ))))
            }
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
        core: &Core,
        revision: TimelineRevision,
        edit: &ConsentEdit<'_>,
        description: &str,
    ) -> Result<(BatchBinding, RunProjection), HostError> {
        self.endpoint
            .check_live()
            .map_err(|reason| problem("consent_liveness", reason))?;
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
        if !self.endpoint.matches_core(core) {
            return Err(scope("The task actor changed"));
        }
        let history = core
            .history_change_id()
            .map_err(|e| problem("consent_authority", e.to_string()))?;
        if history.revision() != revision {
            return Err(stale());
        }
        let history_position = history.position().ok_or_else(stale)?;
        let history_epoch = host
            .hooks
            .runs
            .get(&state.run)
            .and_then(|run| run.tasks.get(&task))
            .ok_or_else(|| scope("Task authority is unavailable"))?
            .history_epoch()
            .clone();
        Ok((
            BatchBinding {
                project: authority.project.clone(),
                run: authority.run.clone(),
                task: authority.task.clone(),
                attempt: authority.attempt.clone(),
                endpoint: authority.endpoint.clone(),
                owner_epoch: authority.owner_epoch,
                history_epoch,
                history_position,
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
        self.endpoint
            .check_batch_history(core, operations)
            .map_err(policy)?;
        let owner = self.consent_host()?;
        let host = lock_host(&owner, &self.endpoint)?;
        let (binding, state) = self.batch_binding(
            &host,
            core,
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
            self.batch_binding(
                &host,
                core,
                revision,
                &ConsentEdit::Batch(&[]),
                "History step",
            )?;
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
        _description: Option<&str>,
    ) -> Result<EditOutcome, HostError> {
        match edit {
            ConsentEdit::Batch(operations) => self
                .endpoint
                .check_batch_history(core, operations)
                .map_err(policy)?,
            ConsentEdit::History { after, .. } => {
                self.endpoint.check_history(after).map_err(policy)?;
            }
        }
        let counts = removed(before, &edit.after(before)?);
        let actual_description = (counts.0 > 0 || counts.1 > 0)
            .then(|| format!("Remove {} clips and {} tracks", counts.0, counts.1));
        let description = actual_description.as_deref();
        let owner = self.consent_host()?;
        if description.is_none() {
            let host = lock_host(&owner, &self.endpoint)?;
            let (binding, state) = self.batch_binding(
                &host,
                core,
                revision,
                edit,
                "Reversible private-branch edit",
            )?;
            self.validate_consent(&host, &state, &binding, core, before, edit)?;
            return edit.apply(&self.endpoint, core, revision);
        }
        let description = description.ok_or_else(refused)?;
        let binding = {
            let host = lock_host(&owner, &self.endpoint)?;
            self.batch_binding(&host, core, revision, edit, description)?
                .0
        };
        let request_digest = digest(&binding)?;
        let request = broker
            .request_task(&serde_json::to_value(&binding)?, description.into())
            .map_err(|e| problem(e.code(), e.to_string()))?;
        let mut host = lock_host(&owner, &self.endpoint)?;
        let (current, mut state) =
            self.batch_binding(&host, core, revision, edit, &request.description)?;
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
        let after = edit.after(before)?;
        let kept_clips = after
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .map(|clip| clip.id)
            .collect::<BTreeSet<_>>();
        let kept_tracks = after
            .tracks
            .iter()
            .map(|track| track.id)
            .collect::<BTreeSet<_>>();
        let mut targets = Vec::new();
        for track in &before.tracks {
            for clip in &track.clips {
                if !kept_clips.contains(&clip.id) {
                    targets.push(RemovedObject::Clip {
                        clip: clip.id,
                        track: track.id,
                    });
                }
            }
            if !kept_tracks.contains(&track.id) {
                targets.push(RemovedObject::Track { track: track.id });
            }
        }
        let removed_clips = targets
            .iter()
            .filter(|target| matches!(target, RemovedObject::Clip { .. }))
            .count();
        let removed_tracks = targets
            .iter()
            .filter(|target| matches!(target, RemovedObject::Track { .. }))
            .count();
        let record = ApprovalRecord {
            kind: "studio_edit_approval_v1".into(),
            binding: current,
            consent: state.edit_consent.clone().ok_or_else(missing)?,
            digest: request_digest,
            removed_clips,
            removed_tracks,
            targets,
        };
        if host.hooks.pending.is_none() {
            host.acknowledge_current_edit_applications()?;
        }
        let producer = host.run_producer(&state.run)?;
        let producer_index = host.resolve_attachment(Some(&producer.bearer()))?;
        let issued = host
            .prepare_edit_metadata(producer_index)
            .map_err(capacity)?;
        // Authority reconciliation may finish earlier work. Fence the approval to
        // the resulting current projection and the unchanged live Core revision.
        let (rechecked, fresh) = self.batch_binding(&host, core, revision, edit, description)?;
        if rechecked != record.binding {
            return Err(stale());
        }
        self.validate_consent(&host, &fresh, &rechecked, core, before, edit)?;
        state = fresh;
        if state.edit_approvals.len() >= ITEM_LIMIT {
            return Err(problem(
                "consent_capacity",
                "The run's edit-approval history is full. Tell the Director to finish or Stop this run; this edit was not applied.",
            ));
        }
        if host.has_pending_edit_in_run(&state.run) {
            return Err(problem(
                "consent_capacity",
                "This run still has an edit audit waiting to be saved. This edit was not applied. Tell the Director to finish or Stop this run before requesting another approval.",
            ));
        }
        let approval_bytes = serde_json::to_vec(&record)?;
        let reference = evidence_reference(&approval_bytes);
        state.edit_approvals.push(reference.clone());
        let execution = state
            .attempts
            .iter()
            .find(|a| a.identity.task == binding.task && a.identity.attempt == binding.attempt)
            .ok_or_else(|| authority("The task attempt ended"))?
            .identity
            .clone();
        let marker = serde_json::to_vec(&AppliedRecord {
            kind: "studio_edit_applied_v1".into(),
            approval: reference,
            revision: TimelineRevision(revision.0.checked_add(1).ok_or_else(stale)?),
        })?;
        let marker_reference = evidence_reference(&marker);
        let mut next_issuer = issued.clone();
        next_issuer.envelope.id.sequence = next_issuer
            .envelope
            .id
            .sequence
            .checked_add(1)
            .ok_or_else(stale)?;
        let first = crate::writer_host::context_publication::native_update(
            state,
            &execution,
            vec![],
            issued,
            host.owner()?.metadata_generation()?,
            host.owner()?
                .orchestration_cut(&execution.run)?
                .map(|cut| cut.segment),
        )?;
        // Each durable cut owns its full immutable snapshot. Keep the approved
        // snapshot intact while deriving its application successor.
        let mut application_state = first.projection().clone();
        application_state.edit_applications.push(marker_reference);
        let mut prepared = Some(
            kinewright_project::orchestration_commit::prepare_orchestration_pair(
                host.owner_mut()?,
                first,
                &approval_bytes,
                |generation, previous| {
                    let update = crate::writer_host::context_publication::native_update(
                        application_state,
                        &execution,
                        vec![],
                        next_issuer,
                        generation,
                        Some(previous),
                    )
                    .map_err(|error| {
                        kinewright_project::bundle::BundleError::Identity(error.to_string())
                    })?;
                    Ok((update, marker))
                },
            )
            .map_err(HostError::from)
            .map_err(capacity)?,
        );
        let result = (|| {
            #[cfg(test)]
            if let Some(hook) = host.hooks.consent_prepared_hook.take() {
                hook()?;
            }
            let pair = prepared.as_mut().ok_or_else(stale)?;
            let (approval_update, approval_hold) = pair.approval_mut()?;
            host.publish_prepared_edit(&producer, approval_update, approval_hold)?;
            pair.release_approval();
            self.endpoint
                .check_live()
                .map_err(|reason| problem("consent_liveness", reason))?;
            answer.approve().map_err(|e| problem("consent_broker", e))?;
            #[cfg(test)]
            if let Some(hook) = host.hooks.consent_apply_hook.take() {
                hook(core);
            }
            let outcome = edit.apply(&self.endpoint, core, revision)?;
            if outcome.applied() {
                if let Some(runtime) = host
                    .hooks
                    .runs
                    .get_mut(&execution.run)
                    .and_then(|run| run.tasks.get_mut(&execution.task))
                {
                    runtime.record_approved_edit();
                }
                // Both exact records were sealed before approval. The successful Core
                // outcome survives a non-capacity application-publication failure.
                let (update, held) = pair.application_mut();
                if let Err(error) = host.publish_prepared_edit(&producer, update, held) {
                    host.hooks
                        .pending_edit_applications
                        .push(PendingEditApplication {
                            run: execution.run.clone(),
                            prepared: prepared.take().ok_or_else(stale)?,
                        });
                    host.message =
                        format!("Edit applied; its audit is pending publication: {error}");
                    eprintln!("{}", host.message);
                }
            }
            Ok(outcome)
        })();
        if let Some(prepared) = prepared {
            host.discard_prepared_edit(prepared);
        }
        result
    }
}

impl HostCommandOwner {
    pub(in crate::writer_host) fn acknowledge_edit_applications(&mut self, state: &RunProjection) {
        use crate::orchestration::orchestration_records::{RunCleanup, RunOutcome};
        let pending = std::mem::take(&mut self.hooks.pending_edit_applications);
        for entry in pending {
            let recorded = state
                .edit_applications
                .contains(&evidence_reference(entry.prepared.marker()));
            if entry.run == state.run
                && (recorded
                    || state.edit_applications.len() >= ITEM_LIMIT
                    || (state.outcome != RunOutcome::Pending
                        && state.cleanup == RunCleanup::Settled))
            {
                self.discard_prepared_edit(entry.prepared);
                if !recorded {
                    self.message = "This edit was approved, but its application record was not kept. Review the kept branch, then re-run the edit or continue the task.".into();
                }
            } else {
                self.hooks.pending_edit_applications.push(entry);
            }
        }
    }
    fn discard_prepared_edit(
        &mut self,
        prepared: kinewright_project::orchestration_commit::PreparedOrchestrationPair,
    ) {
        let result = self.owner_mut().and_then(|owner| {
            kinewright_project::orchestration_commit::discard_prepared_orchestration(
                owner, prepared,
            )
            .map_err(HostError::from)
        });
        if let Err(error) = result {
            self.message = format!("Unused edit preparation awaits storage cleanup: {error}");
            eprintln!("{}", self.message);
        }
    }
    pub(in crate::writer_host) fn has_pending_edit_in_run(&self, run: &RunId) -> bool {
        self.hooks
            .pending_edit_applications
            .iter()
            .any(|entry| &entry.run == run)
    }
    pub(in crate::writer_host) fn has_pending_edit_publication(
        &self,
        update: &crate::orchestration::orchestration_commit::ValidatedRunUpdate,
    ) -> bool {
        self.hooks
            .pending_edit_applications
            .iter()
            .any(|entry| entry.prepared.application_update() == update)
    }
    fn acknowledge_current_edit_applications(&mut self) -> Result<(), HostError> {
        let runs = self
            .hooks
            .pending_edit_applications
            .iter()
            .map(|entry| entry.run.clone())
            .collect::<BTreeSet<_>>();
        for run in runs {
            if let Some(state) = self.owner()?.orchestration_projection(&run)? {
                self.acknowledge_edit_applications(&state);
            } else {
                let pending = std::mem::take(&mut self.hooks.pending_edit_applications);
                for entry in pending {
                    if entry.run == run {
                        self.discard_prepared_edit(entry.prepared);
                    } else {
                        self.hooks.pending_edit_applications.push(entry);
                    }
                }
            }
        }
        Ok(())
    }
    pub(in crate::writer_host) fn retry_pending_edit_publications(
        &mut self,
    ) -> Result<(), HostError> {
        // A fallback may have committed this marker under a newer root/ID
        // payload. Observe that fact before replaying its superseded plan.
        if self.hooks.pending.is_none() {
            self.acknowledge_current_edit_applications()?;
        }
        let mut pending = std::mem::take(&mut self.hooks.pending_edit_applications);
        let result = (|| {
            for entry in &mut pending {
                let update = entry.prepared.application_update();
                if self.hooks.pending.is_none()
                    && self
                        .owner()?
                        .orchestration_projection(&entry.run)?
                        .is_some_and(|current| current.sequence >= update.projection().sequence)
                {
                    // A different durable H1 cut consumed this successor slot.
                    // Only a fresh cut carrying the marker can acknowledge the
                    // debt; the old command identity must never be replayed.
                    continue;
                }
                let (update, held) = entry.prepared.application_mut();
                let producer = self.run_producer(&entry.run)?;
                match self.publish_prepared_edit(&producer, update, held) {
                    Ok(()) => {}
                    // A later person-owned save can supersede the prepared root.
                    // Keep its hold until a fresh ordinary/terminal cut includes
                    // the same marker; never install an old root over that save.
                    Err(HostError::Refused(refusal)) if refusal.code == "history_changed" => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })();
        self.hooks.pending_edit_applications = pending;
        if result.is_ok() {
            self.acknowledge_current_edit_applications()?;
        }
        result
    }
    pub(crate) fn append_pending_edit_applications(
        &mut self,
        state: &mut RunProjection,
    ) -> Result<(), HostError> {
        if let Some(current) = self.owner()?.orchestration_projection(&state.run)? {
            self.acknowledge_edit_applications(&current);
            // Retrying a held application cut can advance this one audit field
            // while an already-built native successor waits for its issuer.
            state.sequence = state.sequence.max(current.sequence);
            for reference in current.edit_applications {
                if !state.edit_applications.contains(&reference) {
                    state.edit_applications.push(reference);
                }
            }
        }
        let pending = std::mem::take(&mut self.hooks.pending_edit_applications);
        let result = (|| {
            for entry in pending.iter().filter(|entry| entry.run == state.run) {
                #[cfg(test)]
                if let Some(hook) = &mut self.hooks.consent_marker_hook {
                    hook()?;
                }
                let reference = self
                    .owner_mut()?
                    .admit_host_grant(entry.prepared.marker())?;
                if !state.edit_applications.contains(&reference) {
                    state.edit_applications.push(reference);
                }
            }
            Ok(())
        })();
        self.hooks.pending_edit_applications = pending;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c4_followups_refusal_words_name_the_action_and_keep_the_cause() {
        for code in ["run_limit", "storage_backpressure"] {
            let HostError::Refused(refusal) = capacity(problem(code, "fixture capacity cause"))
            else {
                panic!("typed refusal")
            };
            assert_eq!(refusal.code, "consent_capacity");
            assert!(
                refusal.message.contains("not applied")
                    && refusal.message.contains("Director")
                    && refusal.message.contains("fixture capacity cause")
            );
            if code == "storage_backpressure" {
                assert!(refusal.message.contains("project's storage capacity"));
                assert!(!refusal.message.contains("start a new run"));
            } else {
                assert!(refusal.message.contains("If project storage is full"));
            }
        }
        let HostError::Refused(refusal) = pending_retry_error(problem("io", "fixture disk cause"))
        else {
            panic!("typed refusal")
        };
        assert_eq!(refusal.code, "io");
        assert!(
            refusal.message.contains("This new edit was not applied")
                && refusal.message.contains("finish or Stop")
                && refusal.message.contains("fixture disk cause")
        );
    }
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
