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
        orchestration_records::{
            AttemptState, DeliveryState, EffectId, EffectRecord, EffectSafety, EffectState,
            ITEM_LIMIT, OrchestrationPayload, OrchestrationRecord, RunOutcome, RunPhase,
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

/// Host-only H1 launch intent. Kept across reserve, bind, charge and execution.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeLaunchIntent {
    pub identity: ExecutionIdentity,
    pub boot_id: String,
    pub dispatched: bool,
    pub native: Option<RecoveryIdentity>,
}
impl NativeLaunchIntent {
    pub(super) fn new(identity: &ExecutionIdentity) -> io::Result<Self> {
        Ok(Self {
            identity: identity.clone(),
            boot_id: crate::guardian::boot_id()?,
            dispatched: false,
            native: None,
        })
    }
}
pub(crate) struct HostInvocationOwner {
    host: Weak<Mutex<HostCommandOwner>>,
    producer: HostAuth,
}
pub(super) struct RegisteredNativeAttempt {
    pub identity: ExecutionIdentity,
    pub control: crate::guardian::StartupControl,
    recovery: RecoveryIdentity,
    pub owner_epoch: u64,
    commit: NativeCommit,
}
enum NativeCommit {
    Authorized,
    Sent,
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
    fn started(&self, identity: &ExecutionIdentity) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        host.lock()
            .map_err(|_| transport("host owner poisoned"))?
            .native_started(&self.producer, identity)
            .map_err(transport)
    }
    fn raw(
        &self,
        identity: &ExecutionIdentity,
        raw: &serde_json::Value,
        ordinal: u64,
        eof: bool,
    ) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        host.lock()
            .map_err(|_| transport("host owner poisoned"))?
            .ingest_native_raw(&self.producer, identity, raw, ordinal, eof)
            .map_err(transport)
    }
    fn completed(
        &self,
        identity: &ExecutionIdentity,
        completed: bool,
        proof: &crate::guardian::CleanupProof,
    ) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        host.lock()
            .map_err(|_| transport("host owner poisoned"))?
            .native_completed(&self.producer, identity, completed, proof)
            .map_err(transport)
    }
    fn unbound_completed(
        &self,
        identity: &ExecutionIdentity,
        proof: &crate::guardian::UnboundCleanupProof,
    ) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        host.lock()
            .map_err(|_| transport("host owner poisoned"))?
            .reconcile_native_effect(
                &self.producer,
                identity,
                &crate::guardian::AttemptRetirementProof::Unbound(proof),
            )
            .map_err(transport)
    }
    fn reserve(&self, invocation: &AuthorizedInvocation) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        let mut host = host.lock().map_err(|_| transport("host owner poisoned"))?;
        let mut projection = host
            .validate_native_invocation(&self.producer, invocation)
            .map_err(transport)?;
        let identity = invocation.invocation().binding().identity();
        let mut intent = host
            .native_launch_intent(&projection, identity)
            .map_err(transport)?
            .unwrap_or(NativeLaunchIntent::new(identity)?);
        if intent.boot_id != crate::guardian::boot_id()? {
            return Err(transport("launch boot changed"));
        }
        intent.dispatched = true;
        let reference = host
            .owner_mut()
            .map_err(transport)?
            .admit_host_grant(&serde_json::to_vec(&intent)?)
            .map_err(transport)?;
        projection
            .attempts
            .iter_mut()
            .find(|a| a.identity == *identity)
            .ok_or_else(|| transport("launch reservation missing"))?
            .recovery_ref = reference.clone();
        crate::orchestration::tasks::refresh_cleanup(&mut projection);
        host.publish_native(&self.producer, projection, identity, vec![reference])
            .map_err(transport)
    }
    fn release(
        &self,
        invocation: &AuthorizedInvocation,
        recovery: &RecoveryIdentity,
        control: &crate::guardian::StartupControl,
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
        host.hooks
            .native
            .retain(|_, attempt| !attempt.control.closed());
        let key = (identity.run.clone(), identity.attempt.clone());
        if host.hooks.native.len() >= ITEM_LIMIT || host.hooks.native.contains_key(&key) {
            return Err(transport("native startup capacity or duplicate"));
        }
        let owner_epoch = host
            .owner()
            .map_err(transport)?
            .store()
            .map_err(transport)?
            .owner_epoch();
        host.hooks.native.insert(
            key,
            RegisteredNativeAttempt {
                identity: identity.clone(),
                control: control.clone(),
                recovery: recovery.clone(),
                owner_epoch,
                commit: NativeCommit::Authorized,
            },
        );
        let mut intent = host
            .native_launch_intent(&projection, identity)
            .map_err(transport)?
            .unwrap_or(NativeLaunchIntent::new(identity)?);
        if intent.boot_id != crate::guardian::boot_id()? {
            return Err(transport("launch boot changed"));
        }
        intent.native = Some(recovery.clone());
        let recovery_ref = host
            .owner_mut()
            .map_err(transport)?
            .admit_host_grant(&serde_json::to_vec(&intent)?)
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
    fn commit(
        &self,
        invocation: &AuthorizedInvocation,
        recovery: &RecoveryIdentity,
        send: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        let mut host = host.lock().map_err(|_| transport("host owner poisoned"))?;
        let identity = invocation.invocation().binding().identity();
        let projection = host
            .native_projection(&self.producer, identity)
            .map_err(transport)?;
        if !projection.attempts.iter().any(|attempt| {
            attempt.identity == *identity && attempt.state == AttemptState::Contained
        }) {
            return Err(transport("native release is no longer contained"));
        }
        let epoch = host
            .owner()
            .map_err(transport)?
            .store()
            .map_err(transport)?
            .owner_epoch();
        let registered = host
            .hooks
            .native
            .get(&(identity.run.clone(), identity.attempt.clone()))
            .ok_or_else(|| transport("native startup revoked"))?;
        if registered.identity != *identity
            || registered.recovery != *recovery
            || registered.owner_epoch != epoch
            || !matches!(registered.commit, NativeCommit::Authorized)
            || registered.control.closed()
        {
            return Err(transport("native startup changed or revoked"));
        }
        let control = registered.control.clone();
        if let Err(error) = host.check_native_commit_budget(&self.producer, invocation) {
            control.close();
            return Err(transport(error));
        }
        let registered = host
            .hooks
            .native
            .get_mut(&(identity.run.clone(), identity.attempt.clone()))
            .ok_or_else(|| transport("native startup revoked"))?;
        // Stop uses this same host mutex and the registered sole lifetime writer.
        // Do not wait for the guardian acknowledgment while holding the host.
        registered.commit = NativeCommit::Sent;
        send()
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
    pub(super) fn revoke_native_owner(
        &mut self,
        owner: Option<&(kinewright_project::bundle_records::ProjectId, u64)>,
    ) {
        self.hooks.native.retain(|_, attempt| {
            let current = owner.is_some_and(|(project, epoch)| {
                attempt.identity.project == *project && attempt.owner_epoch == *epoch
            });
            if !current {
                attempt.control.close();
            }
            // Closing authority never retires a machine claim. The native
            // reader/startup guard still requires an independent CleanupProof.
            current && !attempt.control.closed()
        });
    }

    fn native_projection(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
    ) -> Result<RunProjection, HostError> {
        if self.recovery_only {
            return Err(refused("recovery-only owner cannot authorize sends"));
        }
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
    pub(crate) fn native_accounting(
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
        if !accounting.tasks.iter().any(|task| {
            task.task == identity.task.0 && task.sends.iter().any(|send| send == ack.consumption())
        }) || !state.tasks.iter().any(|task| {
            task.id == identity.task
                && task
                    .progress
                    .decisions
                    .contains(invocation.invocation().manifest())
        }) || !state.effects.iter().any(|effect| {
            effect.task == identity.task
                && effect.state == EffectState::Claimed(identity.attempt.clone())
                && effect.safety == EffectSafety::ProcessBound
        }) {
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
        let observed = std::time::Instant::now();
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
            let mut expanded = Vec::new();
            for reference in references {
                let record = self
                    .owner()?
                    .store()?
                    .read_record::<OrchestrationRecord>(reference)?;
                let OrchestrationPayload::Result(result) = &record.payload else {
                    return Err(refused("wake reference is not a task result"));
                };
                let contents = |reference: &ObjectRef| -> Result<serde_json::Value, HostError> {
                    if reference.byte_length > (crate::protocol::limits::FRAME_BYTES / 2) as u64 {
                        return Err(refused("expanded result capacity"));
                    }
                    if reference.record_kind
                        == kinewright_project::bundle_records::RecordKind::OwnedEvidence
                    {
                        let bytes = self.owner()?.store()?.read_evidence(reference)?;
                        Ok(match std::str::from_utf8(&bytes) {
                            Ok(text) => serde_json::Value::String(text.into()),
                            Err(_) => {
                                serde_json::json!({"encoding":"bytes", "contents":bytes.to_vec()})
                            }
                        })
                    } else {
                        use kinewright_project::bundle_records::{
                            RecordKind, SpecialistBaseRecord,
                        };
                        Ok(match reference.record_kind {
                            RecordKind::RunIndex => serde_json::to_value(
                                &*self
                                    .owner()?
                                    .store()?
                                    .read_record::<OrchestrationRecord>(reference)?,
                            )?,
                            RecordKind::SpecialistBase => serde_json::to_value(
                                &*self
                                    .owner()?
                                    .store()?
                                    .read_record::<SpecialistBaseRecord>(reference)?,
                            )?,
                            _ => return Err(refused("unsupported expanded result record")),
                        })
                    }
                };
                expanded.push(serde_json::json!({
                    "result": result,
                    "explanation": contents(&result.explanation)?,
                    "delta": result.delta_ref.as_ref().map(contents).transpose()?,
                    "artifacts": result.artifacts.iter().map(contents).collect::<Result<Vec<_>,_>>()?,
                    "evidence": result.evidence.iter().map(contents).collect::<Result<Vec<_>,_>>()?,
                    "usage": contents(&result.usage_ref)?,
                }));
            }
            serde_json::to_vec(&expanded)?
        } else {
            self.owner()?
                .store()?
                .read_evidence(&next.payload)?
                .to_vec()
        };
        let payload = if profile.specification().provider() == "codex" {
            self.codex_prompt(task, &payload)?
        } else {
            payload
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
        let previous = self
            .native_accounting(&state)?
            .ok_or_else(|| refused("run has no durable clock"))?;
        self.owner_mut()?
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::AccountingArchive(Box::new(
                    previous.archive().map_err(refused)?,
                )),
            })?;
        let mut accounting = previous.successor().map_err(refused)?;
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
        accounting.validate_successor(&previous).map_err(refused)?;
        let lifetime = crate::orchestration::adapters::InvocationLifetime {
            observed,
            elapsed_ms: accounting.elapsed_ms,
            expires: observed
                .checked_add(std::time::Duration::from_millis(
                    kinewright_project::orchestration_accounting::RUN_MILLIS
                        .saturating_sub(accounting.elapsed_ms)
                        .min(
                            accounting
                                .deadline_unix_ms
                                .saturating_sub(accounting.last_unix_ms),
                        ),
                ))
                .ok_or_else(|| refused("native monotonic deadline unavailable"))?,
        };
        let sealed = self
            .owner_mut()?
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(accounting)),
            })?;
        self.replace_accounting_reference(&mut state, sealed.reference.clone())?;
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
                lifetime,
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

include!("native_spending.rs");

include!("effect_execution.rs");

include!("codex_prompt.rs");
