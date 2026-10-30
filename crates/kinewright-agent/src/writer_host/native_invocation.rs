//! Native release and queued sends share the serialized project metadata owner.
#![allow(dead_code, reason = "C5 enables the qualified connector registry")]
#![allow(clippy::missing_errors_doc)]
use super::{HostAuth, HostCommandOwner, HostError, HostRefusal};
use crate::{
    job_guardian::protocol::RecoveryIdentity,
    native_launcher::{self, InvocationOwner, Manifest},
    orchestration::{
        AdmittedAttempt, ExecutionIdentity,
        adapters::{AuthorizedInvocation, CertifiedConnectorProfile, SendKind},
        orchestration_commit::{CommandDecision, ValidatedRunUpdate},
        orchestration_records::{
            AttemptState, AuthorityBinding, DeliveryState, EffectId, EffectRecord, EffectSafety,
            EffectState, EventSegment, ITEM_LIMIT, OrchestrationCommand, OrchestrationEnvelope,
            OrchestrationEvent, OrchestrationPayload, OrchestrationRecord, RunOutcome, RunPhase,
            RunProjection,
        },
    },
};
use kinewright_project::{
    bundle_records::ObjectRef,
    orchestration_accounting::{
        AccountingRecord, ProviderMetadata, ProviderSelection, SendConsumption,
    },
};
use std::{
    io,
    sync::{Arc, Mutex, Weak},
};

fn refused(reason: impl std::fmt::Debug) -> HostError {
    HostRefusal::new("native_invocation", &format!("{reason:?}")).into()
}
fn transport(reason: impl std::fmt::Display) -> io::Error {
    io::Error::other(reason.to_string())
}

pub(crate) struct HostInvocationOwner {
    host: Weak<Mutex<HostCommandOwner>>,
    producer: HostAuth,
}
impl HostInvocationOwner {
    pub(crate) fn new(host: &Arc<Mutex<HostCommandOwner>>, producer: HostAuth) -> Self {
        Self {
            host: Arc::downgrade(host),
            producer,
        }
    }
}
impl InvocationOwner for HostInvocationOwner {
    fn reserve(&self, invocation: &AuthorizedInvocation) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        let mut host = host.lock().map_err(|_| transport("host owner poisoned"))?;
        host.validate_native_invocation(&self.producer, invocation)
            .map(|_| ())
            .map_err(transport)
    }
    fn release(
        &self,
        invocation: &AuthorizedInvocation,
        recovery: &RecoveryIdentity,
    ) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        let mut host = host.lock().map_err(|_| transport("host owner poisoned"))?;
        let mut projection = host
            .validate_native_invocation(&self.producer, invocation)
            .map_err(transport)?;
        let identity = invocation.invocation().binding().identity();
        let matches = recovery.attempt.project == identity.project.as_str()
            && recovery.attempt.run == identity.run.0.as_str()
            && recovery.attempt.task == identity.task.0.as_str()
            && recovery.attempt.attempt == identity.attempt.0.as_str()
            && recovery.attempt.nonce == identity.nonce.as_str()
            && recovery.attempt.owner_instance == identity.owner_instance.as_str();
        if !matches {
            return Err(transport("native recovery identity changed"));
        }
        let recovery_ref = host
            .owner_mut()
            .map_err(transport)?
            .admit_host_grant(&serde_json::to_vec(recovery)?)
            .map_err(transport)?;
        let reservation = projection
            .attempts
            .iter_mut()
            .find(|attempt| attempt.identity == *identity)
            .ok_or_else(|| transport("attempt disappeared"))?;
        if reservation.state != AttemptState::Reserved {
            return Err(transport("native release already consumed"));
        }
        reservation.state = AttemptState::Contained;
        reservation.recovery_ref = recovery_ref.clone();
        crate::orchestration::tasks::refresh_cleanup(&mut projection);
        host.publish_native(&self.producer, projection, identity, vec![recovery_ref])
            .map_err(transport)?;
        Ok(())
    }
    fn steering(&self, identity: &ExecutionIdentity, text: &str) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        let mut host = host.lock().map_err(|_| transport("host owner poisoned"))?;
        host.queue_native_steering(&self.producer, identity, text)
            .map_err(transport)
    }
}
impl HostCommandOwner {
    fn native_projection(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
    ) -> Result<RunProjection, HostError> {
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("missing native Producer"))?;
        if self.attachments[index].role != super::AttachmentRole::Producer
            || scope.run != identity.run
            || scope.task.is_some()
            || scope.owner_epoch != self.owner()?.store()?.owner_epoch()
            || scope.expired()
            || self.recovering
            || self.hooks.pending.is_some()
        {
            return Err(refused("native Producer authority changed"));
        }
        let state = self
            .owner()?
            .orchestration_projection(&identity.run)?
            .ok_or_else(|| refused("native run missing"))?;
        let binding = super::orchestration::RuntimeGrantBinding {
            run: identity.run.clone(),
            task: identity.task.clone(),
            attempt: identity.attempt.clone(),
            owner_epoch: scope.owner_epoch,
        };
        if !binding.live_in(&state, Some(scope.owner_epoch)) {
            return Err(refused("native task authority retired or expired"));
        }
        if state.outcome != RunOutcome::Pending
            || matches!(
                state.phase,
                RunPhase::Paused | RunPhase::Interrupted | RunPhase::Finished
            )
            || identity.project != self.owner()?.project_id()?
        {
            return Err(refused("native attempt authority changed"));
        }
        Ok(state)
    }
    fn native_accounting(
        &self,
        state: &RunProjection,
    ) -> Result<Option<AccountingRecord>, HostError> {
        for reference in state.usage.iter().rev() {
            let record = self
                .owner()?
                .store()?
                .read_record::<OrchestrationRecord>(reference)?;
            if let OrchestrationPayload::Accounting(accounting) = &record.payload {
                return Ok(Some((**accounting).clone()));
            }
        }
        Ok(None)
    }
    fn validate_native_invocation(
        &mut self,
        producer: &HostAuth,
        invocation: &AuthorizedInvocation,
    ) -> Result<RunProjection, HostError> {
        let identity = invocation.invocation().binding().identity();
        let state = self.native_projection(producer, identity)?;
        if !state
            .attempts
            .iter()
            .any(|attempt| attempt.identity == *identity && attempt.state == AttemptState::Reserved)
        {
            return Err(refused("native startup already released"));
        }
        let ack = invocation.acknowledgment();
        let accounting = self
            .native_accounting(&state)?
            .ok_or_else(|| refused("native send has no durable charge"))?;
        if !state.usage.contains(ack.accounting())
            || !accounting.tasks.iter().any(|task| {
                task.task == identity.task.0
                    && task.sends.iter().any(|send| send == ack.consumption())
            })
            || !state.tasks.iter().any(|task| {
                task.id == identity.task
                    && task
                        .progress
                        .decisions
                        .contains(invocation.invocation().manifest())
            })
            || !state.effects.iter().any(|effect| {
                effect.task == identity.task
                    && effect.state == EffectState::Claimed(identity.attempt.clone())
                    && effect.safety == EffectSafety::ProcessBound
            })
        {
            return Err(refused("native send differs from acknowledged charge"));
        }
        Ok(state)
    }
    pub(crate) fn charge_native_invocation(
        &mut self,
        producer: &HostAuth,
        admitted: &AdmittedAttempt,
        profile: CertifiedConnectorProfile,
        effect: &EffectId,
        kind: SendKind,
        now_ms: u64,
    ) -> Result<AuthorizedInvocation, HostError> {
        profile
            .validate_session(profile.session_config(), admitted.identity())
            .map_err(refused)?;
        let identity = admitted.identity();
        let mut state = self.native_projection(producer, identity)?;
        let next = state
            .effects
            .iter()
            .find(|candidate| &candidate.id == effect)
            .ok_or_else(|| refused("native effect missing"))?
            .clone();
        if next.task != identity.task
            || crate::orchestration::outbox::next_effect(&state, &next.lane)
                .map(|candidate| &candidate.id)
                != Some(effect)
            || !state.attempts.iter().any(|attempt| {
                attempt.identity == *identity && attempt.state == AttemptState::Reserved
            })
            || state.attempts.iter().any(|attempt| {
                attempt.identity.task == identity.task
                    && attempt.identity != *identity
                    && !matches!(attempt.state, AttemptState::Retired(_))
            })
        {
            return Err(refused("native effect not ready or already claimed"));
        }
        let task = state
            .tasks
            .iter()
            .find(|task| task.id == identity.task)
            .ok_or_else(|| refused("native task missing"))?;
        if task.brief.provider != profile.selection() {
            return Err(refused("native provider differs from saved task choice"));
        }
        let wake = state
            .deliveries
            .iter()
            .any(|delivery| &delivery.wake == effect);
        let opening = next.payload == task.brief.assignment
            && !state
                .effects
                .iter()
                .any(|effect| effect.task == task.id && effect.order < next.order);
        if wake
            && state
                .deliveries
                .iter()
                .filter(|delivery| &delivery.wake == effect)
                .any(|delivery| {
                    delivery.state != DeliveryState::Pending
                        || !state.tasks.iter().any(|child| {
                            child.id == delivery.task
                                && child.parent.as_ref() == Some(&identity.task)
                        })
                })
        {
            return Err(refused("native result wake belongs to another task"));
        }
        let actual_kind = if wake {
            SendKind::AutomaticResultWake
        } else if opening {
            if task.parent.is_some() {
                SendKind::SpecialistStart
            } else {
                SendKind::OpeningAssignment
            }
        } else {
            SendKind::QueuedSteering
        };
        if kind != actual_kind {
            return Err(refused("native send route changed"));
        }
        let payload = if wake {
            let references: Vec<_> = state
                .deliveries
                .iter()
                .filter(|delivery| &delivery.wake == effect)
                .map(|delivery| &delivery.result)
                .collect();
            serde_json::to_vec(&references)?
        } else {
            self.owner()?
                .store()?
                .read_evidence(&next.payload)?
                .to_vec()
        };
        if payload.len() > crate::protocol::limits::FRAME_BYTES / 2 {
            return Err(refused("native prompt capacity"));
        }
        let prompt = std::str::from_utf8(&payload).map_err(refused)?;
        let program = native_launcher::program(&profile, prompt).map_err(refused)?;
        native_launcher::context_matches(&profile, &program).map_err(refused)?;
        for root in self
            .config
            .roots
            .read
            .iter()
            .chain(&self.config.roots.write)
        {
            for grant in native_launcher::runtime_paths(&program) {
                if native_launcher::overlaps(grant, root).map_err(refused)? {
                    return Err(refused("native allowlist overlaps host roots"));
                }
            }
        }

        let payload_ref = self.owner_mut()?.admit_host_grant(&payload)?;
        let manifest = Manifest {
            identity: identity.clone(),
            endpoint: profile.binding().endpoint().into(),
            roots: profile.binding().roots().to_vec(),
            kind,
            program,
            payload: payload_ref.clone(),
            turn: 1,
        };
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        let manifest_ref = self.owner_mut()?.admit_host_grant(&manifest_bytes)?;
        let previous = self.native_accounting(&state)?;
        let mut accounting = if let Some(current) = &previous {
            let archive = current.archive().map_err(refused)?;
            self.owner_mut()?
                .admit_scheduler_record(&OrchestrationRecord {
                    version: 1,
                    payload: OrchestrationPayload::AccountingArchive(Box::new(archive)),
                })?;
            current.successor().map_err(refused)?
        } else {
            AccountingRecord::new(identity.project.clone(), identity.run.0.clone(), now_ms)
                .map_err(refused)?
        };
        accounting.observe_clock(now_ms, 0, true).map_err(refused)?;
        if !accounting
            .tasks
            .iter()
            .any(|task| task.task == identity.task.0)
        {
            accounting
                .add_task(identity.task.0.clone(), None)
                .map_err(refused)?;
        }
        if accounting
            .tasks
            .iter()
            .flat_map(|task| &task.sends)
            .any(|send| send.attempt == identity.attempt.0)
        {
            return Err(refused("native attempt already has a consumed send"));
        }
        let selection = profile.selection();
        let provider = ProviderSelection {
            connector: selection.connector,
            model: selection.model,
            options: selection
                .options
                .into_iter()
                .map(|option| (option.name, option.value))
                .collect(),
        };
        let version = profile.runtime().version;
        accounting
            .task_mut(&identity.task.0)
            .map_err(refused)?
            .select_provider(ProviderMetadata {
                attempt: identity.attempt.0.clone(),
                requested: provider.clone(),
                resolved: provider,
                cli_version: format!("{}.{}.{}", version.0, version.1, version.2),
            })
            .map_err(refused)?;
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let issued = self.issue_metadata(index, None)?;
        let send = SendConsumption {
            command: issued.envelope.id.clone(),
            attempt: identity.attempt.0.clone(),
            turn: 1,
            payload: payload_ref,
        };
        accounting
            .task_mut(&identity.task.0)
            .map_err(refused)?
            .consume_send(send.clone())
            .map_err(refused)?;
        if let Some(previous) = previous {
            accounting.validate_successor(&previous).map_err(refused)?;
        }
        let sealed = self
            .owner_mut()?
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(accounting)),
            })?;
        state.usage.push(sealed.reference.clone());
        let claimed = state
            .effects
            .iter_mut()
            .find(|candidate| &candidate.id == effect)
            .ok_or_else(|| refused("native effect disappeared"))?;
        claimed.safety = EffectSafety::ProcessBound;
        claimed.state = EffectState::Claimed(identity.attempt.clone());
        state
            .tasks
            .iter_mut()
            .find(|task| task.id == identity.task)
            .ok_or_else(|| refused("native task disappeared"))?
            .progress
            .decisions
            .push(manifest_ref.clone());
        let (update, receipt) = self.publish_native_issued(
            producer,
            state,
            identity,
            vec![manifest_ref.clone()],
            issued,
        )?;
        AuthorizedInvocation::acknowledged(
            crate::orchestration::adapters::PreparedInvocation {
                profile,
                kind,
                manifest_bytes,
                bytes: payload,
                manifest: manifest_ref,
                accounting: sealed.reference,
                consumption: send,
            },
            &update,
            &receipt,
        )
        .map_err(refused)
    }

    fn publish_native(
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
    fn publish_native_issued(
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
    fn queue_native_steering(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
        text: &str,
    ) -> Result<(), HostError> {
        if text.is_empty() || text.len() > crate::protocol::limits::FRAME_BYTES / 2 {
            return Err(refused("native steering capacity"));
        }
        let mut state = self.native_projection(producer, identity)?;
        if state.effects.len() >= ITEM_LIMIT {
            return Err(refused("native steering queue capacity"));
        }
        let payload = self.owner_mut()?.admit_host_grant(text.as_bytes())?;
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let issued = self.issue_metadata(index, None)?;
        let task = state
            .tasks
            .iter_mut()
            .find(|task| task.id == identity.task)
            .ok_or_else(|| refused("native steering task missing"))?;
        let lane = task.brief.scope.base.branch.clone();
        task.progress.decisions.push(payload.clone());
        let order = state
            .effects
            .iter()
            .filter(|effect| effect.lane == lane)
            .map(|effect| effect.order)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| refused("native effect sequence capacity"))?;
        let digest = kinewright_media::sha256_bytes(&serde_json::to_vec(&(
            "native-steering",
            &issued.envelope.id,
        ))?);
        let id = serde_json::from_value(serde_json::json!(&digest[..32]))?;
        state.effects.push(EffectRecord {
            id,
            task: identity.task.clone(),
            lane,
            order,
            safety: EffectSafety::ProcessBound,
            state: EffectState::Pending,
            payload: payload.clone(),
        });
        self.publish_native_issued(producer, state, identity, vec![payload], issued)?;
        Ok(())
    }
}
