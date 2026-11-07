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
    /// C5 fix 7: witnesses see each raw provider event's type token only,
    /// never its content.
    #[cfg(any(test, feature = "test-util"))]
    raw_witness: Option<RawTypeWitness>,
}
/// Receives one raw provider event's bounded type token.
#[cfg(any(test, feature = "test-util"))]
pub(crate) type RawTypeWitness = Arc<dyn Fn(&str) + Send + Sync>;
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
            #[cfg(any(test, feature = "test-util"))]
            raw_witness: None,
        }
    }
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn with_raw_witness(mut self, witness: RawTypeWitness) -> Self {
        self.raw_witness = Some(witness);
        self
    }
}
/// The configuration shape of a Claude `system/init`, for the dry-launch
/// report: tool names outside the host MCP server, MCP server names, plugin
/// and skill counts, and the permission mode. Names only, bounded; never a
/// credential or conversation content.
#[cfg(any(test, feature = "test-util"))]
fn init_shape(raw: &serde_json::Value) -> String {
    let name = |value: &serde_json::Value| {
        value
            .as_str()
            .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
            .filter(|name| {
                name.len() <= 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
            })
            .unwrap_or("?")
            .to_owned()
    };
    let list = |key: &str, keep: &dyn Fn(&str) -> bool| {
        raw.get(key)
            .and_then(serde_json::Value::as_array)
            .map_or_else(
                || "absent".to_owned(),
                |values| {
                    values
                        .iter()
                        .map(name)
                        .filter(|name| keep(name))
                        .take(16)
                        .collect::<Vec<_>>()
                        .join(",")
                },
            )
    };
    let count = |key: &str| {
        raw.get(key)
            .and_then(serde_json::Value::as_array)
            .map_or_else(|| "absent".to_owned(), |values| values.len().to_string())
    };
    format!(
        "tools_outside=[{}] servers=[{}] plugins={} skills={} mode={} plugin_sources=[{}] keys=[{}]",
        list("tools", &|name| !name.starts_with("mcp__kinewright__")),
        list("mcp_servers", &|_| true),
        count("plugins"),
        count("skills"),
        raw.get("permissionMode")
            .map_or_else(|| "absent".to_owned(), name),
        plugins(raw),
        raw.as_object().map_or_else(String::new, |object| object
            .keys()
            .filter(|key| key.len() <= 64
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .take(48)
            .cloned()
            .collect::<Vec<_>>()
            .join(",")),
    )
}

/// Each loaded plugin's name and where it was loaded from, with the person's
/// home shown as `~`, so a plugin leaking in from their configuration shows.
#[cfg(any(test, feature = "test-util"))]
fn plugins(raw: &serde_json::Value) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let field = |plugin: &serde_json::Value, key: &str| {
        let value = plugin
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?");
        let value = match value.strip_prefix(home.as_str()) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => value.to_owned(),
        };
        value.chars().take(160).collect::<String>()
    };
    raw.get("plugins")
        .and_then(serde_json::Value::as_array)
        .map(|plugins| {
            plugins
                .iter()
                .take(8)
                .map(|plugin| format!("{}@{}", field(plugin, "name"), field(plugin, "path")))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}

/// A bounded type token from a raw provider event; anything else is `other`.
#[cfg(any(test, feature = "test-util"))]
fn raw_event_type(raw: &serde_json::Value, eof: bool) -> &str {
    if eof {
        return "native.eof";
    }
    raw.get("type")
        .and_then(serde_json::Value::as_str)
        .filter(|kind| {
            !kind.is_empty()
                && kind.len() <= 48
                && kind
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'.' || byte == b'_')
        })
        .unwrap_or("other")
}
impl InvocationOwner for HostInvocationOwner {
    fn launch_failed(&self, identity: &ExecutionIdentity, reason: &str) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        host.lock()
            .map_err(|_| transport("host owner poisoned"))?
            .record_runtime_refusal(&self.producer, &identity.run, reason)
            .map_err(transport)
    }
    fn stream_refused(&self, identity: &ExecutionIdentity, reason: &str) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        host.lock()
            .map_err(|_| transport("host owner poisoned"))?
            .record_stream_refusal(&self.producer, identity, reason)
            .map_err(transport)
    }
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
        self.raw_as(
            native_launcher::StreamDialect::Codex,
            identity,
            raw,
            ordinal,
            eof,
        )
    }
    fn raw_as(
        &self,
        dialect: native_launcher::StreamDialect,
        identity: &ExecutionIdentity,
        raw: &serde_json::Value,
        ordinal: u64,
        eof: bool,
    ) -> io::Result<()> {
        #[cfg(any(test, feature = "test-util"))]
        if let Some(witness) = &self.raw_witness {
            // A system event also names its subtype, a fixed classifier,
            // so a refused startup event can be identified without content.
            let kind = raw_event_type(raw, eof);
            match raw
                .get("subtype")
                .and_then(serde_json::Value::as_str)
                .filter(|subtype| {
                    kind == "system"
                        && !subtype.is_empty()
                        && subtype.len() <= 48
                        && subtype
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                }) {
                Some("init") => witness(&format!("system.init:{}", init_shape(raw))),
                Some(subtype) => witness(&format!("system.{subtype}")),
                None => witness(kind),
            }
        }
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| transport("host owner lost"))?;
        host.lock()
            .map_err(|_| transport("host owner poisoned"))?
            .ingest_native_raw_as(dialect, &self.producer, identity, raw, ordinal, eof)
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
            let record = self.owner()?.store()?.read_run_record(reference)?;
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
        if !self.run_recovery_blocks(&state)?.is_empty() {
            return Err(refused(
                "recovery_requires_person: unfinished task completion or merge",
            ));
        }
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
            self.context_result_wake(&state, &identity.task, &references)?
        } else {
            self.owner()?
                .store()?
                .read_evidence(&next.payload)?
                .to_vec()
        };
        let wake_bytes = if wake { payload.len() as u64 } else { 0 };
        let live = if let Some(endpoint) = self
            .hooks
            .runs
            .get(&identity.run)
            .and_then(|run| run.tasks.get(&identity.task))
            .map(crate::orchestration::tasks::EditorTask::endpoint)
        {
            let auth = self
                .attachments
                .iter()
                .find(|attachment| {
                    attachment.producer_scope.as_ref().is_some_and(|scope| {
                        scope.endpoint.as_ref() == Some(&endpoint.authority().endpoint)
                    })
                })
                .ok_or_else(|| refused("task prompt attachment missing"))?
                .auth
                .clone();
            let live = self.task_reply_context(&auth, &endpoint, &mut None);
            if live.get("controls").is_none() {
                return Err(refused("live prompt controls unavailable"));
            }
            live
        } else {
            #[cfg(any(test, feature = "test-util"))]
            if matches!(
                profile.specification().provider(),
                "scripted" | "scripted-claude"
            ) {
                serde_json::json!({"version":1})
            } else {
                return Err(refused("task prompt endpoint missing"));
            }
            #[cfg(not(any(test, feature = "test-util")))]
            return Err(refused("task prompt endpoint missing"));
        };
        let payload = self.codex_prompt(task, &payload, &live)?;
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
        self.owner()?.check_run_send_budget(&previous)?;
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
        // This launcher always constructs a new exec, without resume arguments.
        let stream = identity.attempt.0.clone();
        let measured = accounting.task_mut(&identity.task.0).map_err(refused)?;
        measured
            .bind_stream(kinewright_project::usage_stream::StreamBinding {
                attempt: identity.attempt.0.clone(),
                stream: stream.clone(),
                adapter: if native_launcher::StreamDialect::of(profile.specification().provider())
                    == native_launcher::StreamDialect::Claude
                {
                    kinewright_project::usage_stream::Adapter::Claude21316V1
                } else {
                    kinewright_project::usage_stream::Adapter::Codex01660V1
                },
                resumed: false,
            })
            .map_err(refused)?;
        let surface = crate::server::te1_surface(task.brief.role);
        let role = serde_json::to_value(task.brief.role)?
            .as_str()
            .ok_or_else(|| refused("task role"))?
            .to_owned();
        let prefix_identity = kinewright_media::sha256_bytes(
            &super::task_context::application_prefix(task.brief.role)?,
        );
        measured
            .measurements
            .get_or_insert_with(Default::default)
            .sends
            .push(kinewright_project::usage_measurement::Send {
                attempt: identity.attempt.0.clone(),
                role,
                stream,
                turn: 1,
                prompt_bytes: payload.len() as u64,
                wake_bytes,
                prefix_identity,
                surface,
            });
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
include!("native_wake.rs");
