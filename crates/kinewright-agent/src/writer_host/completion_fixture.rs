//! Unpaid Studio witnesses use real host dispatch and real publication faults.
#![allow(clippy::missing_errors_doc)]
use super::{
    CompletionStatus, Document, Duration, HostAuth, HostCommandOwner, HostError, HostRefusal,
    ScopedOrchestrationRequest, WriterHost, orchestration,
};
use crate::orchestration::orchestration_records::{
    AuthorityBinding, CompletionStep, ExecutionIdentity, FinishTask, OrchestrationCommand,
    OrchestrationEnvelope, ProviderSelectionRecord, Role, RunId, StartRun, TaskBrief, TaskTurn,
    TurnPurpose,
};
use kinewright_project::orchestration_commit::CommandDecision;

pub struct CompletionFixture {
    pub run: RunId,
    pub receipt: kinewright_project::orchestration_commit::OrchestrationReceipt,
    request: ScopedOrchestrationRequest,
    actor: HostAuth,
    producer: HostAuth,
    identity: ExecutionIdentity,
    host: std::sync::Weak<std::sync::Mutex<HostCommandOwner>>,
}
impl CompletionFixture {
    pub fn reconcile_storage(&self) -> Result<(), HostError> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| HostRefusal::new("fixture", "host closed"))?;
        let mut owner = host
            .lock()
            .map_err(|_| HostRefusal::new("fixture", "host poisoned"))?;
        owner.owner_mut()?.reconcile_metadata().map_err(Into::into)
    }
    pub fn retire(&self) -> Result<(), HostError> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| HostRefusal::new("fixture", "host closed"))?;
        let mut owner = host
            .lock()
            .map_err(|_| HostRefusal::new("fixture", "host poisoned"))?;
        owner.reconcile_native_effect(
            &self.producer,
            &self.identity,
            &crate::guardian::AttemptRetirementProof::NotDispatched(
                crate::native_launcher::native_identity(&self.identity),
            ),
        )
    }
    pub fn retry(&self) -> Result<CompletionStatus, HostError> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| HostRefusal::new("fixture", "host closed"))?;
        let mut owner = host
            .lock()
            .map_err(|_| HostRefusal::new("fixture", "host poisoned"))?;
        let index = owner.resolve_attachment(Some(&self.actor.bearer()))?;
        let receipt = owner.dispatch_orchestration(index, &self.request)?;
        if receipt != self.receipt {
            return Err(HostRefusal::new("fixture", "original finish receipt changed").into());
        }
        let OrchestrationCommand::FinishTask(finish) = &self.request.envelope.command else {
            unreachable!()
        };
        Ok(owner.completion_reply_status(&self.run, finish, &receipt.envelope.id))
    }
}

fn request(
    owner: &mut HostCommandOwner,
    index: usize,
    run: Option<RunId>,
    command: OrchestrationCommand,
) -> Result<ScopedOrchestrationRequest, HostError> {
    let issued = owner.issue_metadata(index, None)?;
    let scope = owner.attachments[index].producer_scope.as_ref();
    let mut envelope = OrchestrationEnvelope {
        id: issued.envelope.id,
        digest: serde_json::from_value(serde_json::json!("0".repeat(64)))?,
        project: issued.envelope.project_id,
        expected_generation: issued.root_generation,
        authority: AuthorityBinding {
            grant_ref: issued.grant_ref,
            owner_epoch: issued.envelope.owner_epoch,
            endpoint: scope.and_then(|scope| scope.endpoint.clone()),
            attempt: scope.and_then(|scope| scope.attempt.clone()),
        },
        command,
    };
    envelope.digest = envelope
        .computed_digest()
        .map_err(|_| HostRefusal::new("fixture", "invalid digest"))?;
    Ok(ScopedOrchestrationRequest {
        run,
        envelope,
        consume_batch: None,
    })
}

impl WriterHost {
    /// Reserve an unpaid, unstarted task, then lose a real completion substep.
    /// The caller supplies a dormant `ProtocolFixture`; no provider is launched.
    pub fn fixture_pending_completion(
        &self,
        step: CompletionStep,
    ) -> Result<CompletionFixture, HostError> {
        let host = self
            .owner
            .upgrade()
            .ok_or_else(|| HostRefusal::new("fixture", "host closed"))?;
        let mut owner = host
            .lock()
            .map_err(|_| HostRefusal::new("fixture", "host poisoned"))?;
        if !owner.fixture_director || owner.native_recovery_pending {
            return Err(
                HostRefusal::new("fixture", "dormant fixture must finish recovery first").into(),
            );
        }
        let index = owner.resolve_attachment(Some(&self.auth.bearer()))?;
        let project = owner.owner_mut()?;
        let mut candidate = project.fork_durable_branch("completion fixture")?;
        project.persist_durable_branch(&mut candidate)?;
        let base = project.candidate_binding(candidate.branch_id())?;
        let text = project.admit_host_grant(b"unpaid completion witness")?;
        let brief = TaskBrief {
            plan: text.clone(),
            script: text.clone(),
            assignment: text,
            inputs: project.current_inputs_ref().clone(),
            role: Role::Director,
            provider: ProviderSelectionRecord {
                connector: "scripted".into(),
                model: "fixture".into(),
                options: vec![],
            },
            scope: crate::orchestration::policy::director_scope(&Document::default(), base)
                .map_err(|_| HostRefusal::new("fixture", "scope"))?,
        };
        let start = request(
            &mut owner,
            index,
            None,
            OrchestrationCommand::StartRun(Box::new(StartRun {
                director: brief.clone(),
            })),
        )?;
        let inputs = owner.prepare_run_inputs(&brief.scope.base.checkpoint)?;
        drop(owner);
        let inputs = inputs.open()?;
        let mut owner = host
            .lock()
            .map_err(|_| HostRefusal::new("fixture", "host poisoned"))?;
        let index = owner.resolve_attachment(Some(&self.auth.bearer()))?;
        owner.hooks.prepared_run_inputs = Some(inputs);
        let receipt = owner.dispatch_orchestration(index, &start);
        owner.hooks.prepared_run_inputs = None;
        let receipt = receipt?;
        let run: RunId = receipt
            .envelope
            .identity("run")
            .map_err(|_| HostRefusal::new("fixture", "run"))?;
        let producer = owner.run_producer(&run)?;
        let (admitted, _, _) = owner
            .reserve_native_effect(&producer, &run)?
            .ok_or_else(|| HostRefusal::new("fixture", "no task reservation"))?;
        let identity = admitted.identity();
        let (actor, endpoint) = owner.prepare_task_endpoint(
            &producer,
            &run,
            &identity.task,
            &identity.attempt,
            Duration::from_secs(300),
        )?;
        let index = owner.resolve_attachment(Some(&actor.bearer()))?;
        let turn = request(
            &mut owner,
            index,
            Some(run.clone()),
            OrchestrationCommand::BeginTurn(TaskTurn {
                task: identity.task.clone(),
                attempt: identity.attempt.clone(),
                session: identity.nonce.clone(),
                ordinal: 1,
                owner_epoch: endpoint.authority().owner_epoch,
                expected_head: brief.scope.base.head,
                purpose: TurnPurpose::Work,
            }),
        )?;
        owner.dispatch_orchestration(index, &turn)?;
        let core = owner.hooks.runs[&run].tasks[&identity.task].core();
        let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
            revision,
            ..
        }) = core
            .request(kinewright_core::Command::Query(
                kinewright_core::Query::Snapshot,
            ))
            .map_err(|error| HostError::Protocol(error.to_string()))?
        else {
            unreachable!()
        };
        let finish = request(
            &mut owner,
            index,
            Some(run.clone()),
            OrchestrationCommand::FinishTask(FinishTask {
                task: identity.task.clone(),
                expected_revision: revision,
            }),
        )?;
        let nth = if step == CompletionStep::Freeze { 2 } else { 3 };
        let receipt = kinewright_project::io_observer::fail_nth("root_rename_io", nth, || {
            owner.dispatch_orchestration(index, &finish)
        })?;
        if receipt.decision != CommandDecision::Applied
            || !matches!(
                owner.hooks.pending,
                Some(orchestration::PendingPublication::Completion(_))
            )
        {
            return Err(HostRefusal::new("fixture", "completion fault was not reached").into());
        }
        Ok(CompletionFixture {
            run,
            receipt,
            request: finish,
            actor,
            producer,
            identity: identity.clone(),
            host: self.owner.clone(),
        })
    }
}
