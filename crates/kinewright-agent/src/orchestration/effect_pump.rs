//! The only persisted-effect executor. H1 decisions finish before native work.
//! C4 supplies task endpoints; C5 supplies qualified profiles. Neither changes
//! the charge/recovery path. The production connector registry remains empty.
#![allow(dead_code, reason = "C4/C5 supply qualified endpoint/profile ports")]
#![allow(clippy::missing_errors_doc)]
use crate::orchestration::{
    AdmittedAttempt,
    adapters::{CertifiedConnectorProfile, GuardianLauncher},
    orchestration_records::{ExecutionIdentity, RunId, RunOutcome},
};
use crate::{
    execution_claim::MachineClaim,
    native_launcher::NativeGuardianLauncher,
    writer_host::{HostAuth, HostCommandOwner},
};
use kinewright_core::{AgentError, AgentEvent, AgentSession};
use std::{
    collections::BTreeMap,
    io,
    num::NonZeroU64,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

pub(crate) trait ProfileFactory: Send + Sync {
    fn bind(&self, attempt: &AdmittedAttempt) -> io::Result<CertifiedConnectorProfile>;
}

/// Driven by the existing `session::pump_session` through nonblocking Studio
/// polling. It adds no event wait loop or serialized-host process launch.
pub(crate) struct EffectPump {
    host: Weak<Mutex<HostCommandOwner>>,
    producer: HostAuth,
    run: RunId,
    launcher: NativeGuardianLauncher,
    profiles: Arc<dyn ProfileFactory>,
    ceiling: Option<NonZeroU64>,
    active: BTreeMap<crate::orchestration::orchestration_records::AttemptId, Box<dyn AgentSession>>,
    sender: crate::protocol::limits::EventSender,
    events: crossbeam_channel::Receiver<AgentEvent>,
    observed: Instant,
    opened: bool,
}
impl EffectPump {
    pub(crate) fn new(
        host: &Arc<Mutex<HostCommandOwner>>,
        producer: HostAuth,
        run: RunId,
        launcher: NativeGuardianLauncher,
        profiles: Arc<dyn ProfileFactory>,
        ceiling: Option<NonZeroU64>,
    ) -> Self {
        let (sender, events) = crate::protocol::limits::event_channel();
        Self {
            host: Arc::downgrade(host),
            producer,
            run,
            launcher,
            profiles,
            ceiling,
            active: BTreeMap::new(),
            sender,
            events,
            observed: Instant::now(),
            opened: false,
        }
    }
    fn drain_events(&mut self) -> io::Result<()> {
        // Drain bounded child channels on the SAME owner pump. Raw usage is
        // already H1-published before these legacy display events are parsed.
        let mut done = Vec::new();
        for (attempt, session) in &self.active {
            for event in session.events().try_iter().take(32) {
                if event == AgentEvent::Done {
                    done.push(attempt.clone());
                    break;
                }
                if !matches!(event, AgentEvent::Cost { .. }) && self.sender.send(event).is_err() {
                    return Err(io::Error::other("effect event capacity"));
                }
            }
        }
        for attempt in done {
            self.active.remove(&attempt);
        }
        Ok(())
    }
    fn observe_clock(
        &mut self,
        host: &mut HostCommandOwner,
        state: &crate::orchestration::orchestration_records::RunProjection,
        identity: &ExecutionIdentity,
        ready: bool,
    ) -> io::Result<u64> {
        let accounting = host
            .native_accounting(state)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("run clock missing"))?;
        let wall = crate::writer_host::spending::unix_ms();
        let trusted = wall.is_ok();
        let now = wall.unwrap_or(accounting.last_unix_ms);
        let delta = u64::try_from(self.observed.elapsed().as_millis()).map_err(io::Error::other)?;
        let clock_due = !trusted
            || now < accounting.last_unix_ms
            || now >= accounting.deadline_unix_ms
            || accounting.elapsed_ms.saturating_add(delta)
                >= kinewright_project::orchestration_accounting::RUN_MILLIS;
        if delta >= 30_000 || ready || !self.opened || clock_due {
            host.change_spending(
                &self.producer,
                identity,
                crate::writer_host::native_invocation::SpendingChange::Clock,
                crate::protocol::limits::usage::ClockObservation {
                    unix_ms: now,
                    monotonic_delta_ms: delta,
                    trusted,
                },
            )
            .map_err(io::Error::other)?;
            self.observed = Instant::now();
        }
        self.opened = true;
        Ok(now)
    }
    fn launch_reserved(
        &self,
        owner: &Arc<Mutex<HostCommandOwner>>,
        attempt: AdmittedAttempt,
        effect: &crate::orchestration::orchestration_records::EffectId,
        kind: crate::orchestration::adapters::SendKind,
        now: u64,
    ) -> io::Result<Box<dyn AgentSession>> {
        let identity = attempt.identity();
        if let Some(limit) = self.ceiling {
            owner
                .lock()
                .map_err(|_| io::Error::other("host owner poisoned"))?
                .change_spending(
                    &self.producer,
                    identity,
                    crate::writer_host::native_invocation::SpendingChange::Ceiling(limit.get()),
                    crate::protocol::limits::usage::ClockObservation {
                        unix_ms: now,
                        monotonic_delta_ms: 0,
                        trusted: true,
                    },
                )
                .map_err(io::Error::other)?;
        }
        let profile = self.profiles.bind(&attempt)?;
        let invocation = owner
            .lock()
            .map_err(|_| io::Error::other("host owner poisoned"))?
            .charge_native_invocation(
                &self.producer,
                &attempt,
                profile,
                effect,
                kind,
                crate::writer_host::spending::unix_ms().map_err(io::Error::other)?,
            )
            .map_err(io::Error::other)?;
        self.launcher
            .launch(attempt, invocation)
            .map_err(|e| io::Error::other(e.0))
            .map(crate::orchestration::adapters::ContainedSession::into_session)
    }
    pub(crate) fn tick(&mut self) -> io::Result<()> {
        self.drain_events()?;
        let owner = self
            .host
            .upgrade()
            .ok_or_else(|| io::Error::other("host owner lost"))?;
        let mut host = owner
            .lock()
            .map_err(|_| io::Error::other("host owner poisoned"))?;
        let state = host
            .spending_scope(&self.producer, &self.run)
            .map_err(io::Error::other)?;
        let project = host.spending_project().map_err(io::Error::other)?;
        let identity = state.attempts.first().map_or_else(
            || ExecutionIdentity {
                version: 1,
                project,
                run: self.run.clone(),
                task: state.tasks[0].id.clone(),
                attempt: crate::orchestration::orchestration_records::AttemptId(self.run.0.clone()),
                nonce: self.run.0.clone(),
                owner_instance: self.run.0.clone(),
            },
            |a| a.identity.clone(),
        );
        let accounting = host.native_accounting(&state).map_err(io::Error::other)?;
        let available = self.active.len() < 3
            && state.effects.iter().any(|e| {
                crate::orchestration::outbox::next_effect(&state, &e.lane)
                    .is_some_and(|n| n.id == e.id)
            });
        let ready = available
            && state.effects.iter().any(|e| {
                crate::orchestration::outbox::next_effect(&state, &e.lane)
                    .is_some_and(|n| n.id == e.id)
                    && !accounting.as_ref().is_some_and(|a| {
                        a.tasks
                            .iter()
                            .any(|t| t.task == e.task.0 && t.wait_reason.is_some())
                    })
            });
        let now = self.observe_clock(&mut host, &state, &identity, ready)?;
        host.clear_due_native_waits(&self.producer, &self.run, now)
            .map_err(io::Error::other)?;
        if state.outcome != RunOutcome::Pending && self.active.is_empty() {
            let _ = self.sender.send(AgentEvent::Done);
            return Ok(());
        }
        if !available {
            return Ok(());
        }
        let Some((attempt, effect, kind)) = host
            .reserve_native_effect(&self.producer, &self.run)
            .map_err(io::Error::other)?
        else {
            return Ok(());
        };
        let identity = attempt.identity().clone();
        drop(host);
        let launched = self.launch_reserved(&owner, attempt, &effect, kind, now);
        let session = match launched {
            Ok(session) => session,
            Err(error) => {
                let mut claim = self
                    .launcher
                    .claim
                    .lock()
                    .map_err(|_| io::Error::other("machine claim poisoned"))?;
                if let Err(cleanup) =
                    reconcile_attempt(&owner, &mut claim, &self.producer, &identity)
                {
                    return Err(io::Error::other(format!(
                        "{error}; cleanup_blocked: {cleanup}"
                    )));
                }
                return Err(error);
            }
        };
        let id = identity.attempt;
        self.active.insert(id, session);
        Ok(())
    }
}
impl AgentSession for EffectPump {
    fn send_user_message(&mut self, text: String) -> Result<(), AgentError> {
        // The host's durable command carries the payload. This message only
        // starts its pump; subsequent steering must be persisted by H3 first.
        if !text.is_empty() {
            return Err(AgentError::Protocol(
                "persist steering through the host".into(),
            ));
        }
        self.tick().map_err(|e| AgentError::Protocol(e.to_string()))
    }
    fn poll_runtime_request(
        &mut self,
    ) -> Result<Option<kinewright_core::RuntimeRequest>, AgentError> {
        self.tick()
            .map_err(|e| AgentError::Protocol(e.to_string()))?;
        Ok(None)
    }
    fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
        self.events.clone()
    }
    fn interrupt(&mut self) {
        for session in self.active.values_mut() {
            session.interrupt();
        }
    }
}
impl Drop for EffectPump {
    fn drop(&mut self) {
        self.interrupt();
    }
}

/// Called after reopening the host and acquiring `MachineClaim::recover()`. No
/// process is launched. Unknown charged outcomes remain unknown and charged.
pub(crate) fn recover_run(
    host: &Arc<Mutex<HostCommandOwner>>,
    mut claim: MachineClaim,
    run: &RunId,
) -> io::Result<(MachineClaim, HostAuth)> {
    claim.check_run(
        host.lock()
            .map_err(|_| io::Error::other("host owner poisoned"))?
            .spending_project()
            .map_err(io::Error::other)?
            .as_str(),
        run.0.as_str(),
    )?;
    let (producer, state) = host
        .lock()
        .map_err(|_| io::Error::other("host owner poisoned"))?
        .restore_native_run(run)
        .map_err(io::Error::other)?;
    let retained = claim.recovery_identities().to_vec();
    for native in retained {
        let attempt = host
            .lock()
            .map_err(|_| io::Error::other("host owner poisoned"))?
            .recovery_attempt(&state, &native)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("private attempt missing from H1"))?;
        reconcile_attempt(host, &mut claim, &producer, &attempt.identity)?;
    }
    for attempt in &state.attempts {
        let current = host
            .lock()
            .map_err(|_| io::Error::other("host owner poisoned"))?
            .spending_scope(&producer, run)
            .map_err(io::Error::other)?;
        if current.attempts.iter().any(|a| {
            a.identity == attempt.identity
                && !matches!(
                    a.state,
                    crate::orchestration::orchestration_records::AttemptState::Retired(_)
                )
        }) {
            reconcile_attempt(host, &mut claim, &producer, &attempt.identity)?;
        }
    }
    if let Some(attempt) = state.attempts.first() {
        let mut owner = host
            .lock()
            .map_err(|_| io::Error::other("host owner poisoned"))?;
        let current = owner
            .spending_scope(&producer, run)
            .map_err(io::Error::other)?;
        let accounting = owner
            .native_accounting(&current)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("recovery clock missing"))?;
        let wall = crate::writer_host::spending::unix_ms();
        let trusted = wall.is_ok();
        let result = owner.change_spending(
            &producer,
            &attempt.identity,
            crate::writer_host::native_invocation::SpendingChange::Clock,
            crate::protocol::limits::usage::ClockObservation {
                unix_ms: wall.unwrap_or(accounting.last_unix_ms),
                monotonic_delta_ms: 0,
                trusted,
            },
        );
        if let Err(error) = result {
            let persisted = owner
                .spending_scope(&producer, run)
                .map_err(io::Error::other)?;
            if owner
                .native_accounting(&persisted)
                .map_err(io::Error::other)?
                .is_none_or(|r| r.check_clock().is_ok())
                || (persisted.outcome == RunOutcome::Pending
                    && persisted.phase
                        != crate::orchestration::orchestration_records::RunPhase::Paused)
            {
                return Err(io::Error::other(error));
            }
        }
    }
    Ok((
        claim.resume(
            host.lock()
                .map_err(|_| io::Error::other("host owner poisoned"))?
                .spending_project()
                .map_err(io::Error::other)?
                .as_str(),
            run.0.as_str(),
        )?,
        producer,
    ))
}

fn reconcile_attempt(
    host: &Arc<Mutex<HostCommandOwner>>,
    claim: &mut MachineClaim,
    producer: &HostAuth,
    identity: &ExecutionIdentity,
) -> io::Result<()> {
    use crate::orchestration::orchestration_records::AttemptState;
    claim.check_run(identity.project.as_str(), identity.run.0.as_str())?;
    host.lock()
        .map_err(|_| io::Error::other("host owner poisoned"))?
        .reconcile_native_pending(producer)
        .map_err(io::Error::other)?;
    let native = claim
        .recovery_identities()
        .iter()
        .find(|a| a.attempt == identity.attempt.0.as_str())
        .cloned();
    if let Some(native) = native {
        if native.project != identity.project.as_str()
            || native.run != identity.run.0.as_str()
            || native.task != identity.task.0.as_str()
            || native.nonce != identity.nonce.as_str()
            || native.owner_instance != identity.owner_instance.as_str()
        {
            return Err(io::Error::other("private attempt scope changed"));
        }
        match claim.recovery(&native) {
            Ok(recovery) => {
                let proof = crate::guardian::RecoveryObserver::recover_cleanup(&recovery)?;
                host.lock()
                    .map_err(|_| io::Error::other("host owner poisoned"))?
                    .native_completed(producer, identity, false, &proof)
                    .map_err(io::Error::other)?;
                claim.retire(&proof)?;
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let state = host
                    .lock()
                    .map_err(|_| io::Error::other("host owner poisoned"))?
                    .spending_scope(producer, &identity.run)
                    .map_err(io::Error::other)?;
                if !state
                    .attempts
                    .iter()
                    .any(|a| a.identity == *identity && a.state == AttemptState::Reserved)
                {
                    return Err(io::Error::other(
                        "unbound native recovery requires Reserved H1 attempt",
                    ));
                }
                let proof = crate::guardian::RecoveryObserver::unbound_cleanup(claim, &native)?;
                // Delete only under proof. A crash here leaves H1 Reserved with no
                // private record, the already-cleaned case covered by the same lock.
                claim.retire_unbound(&proof)?;
            }
            Err(error) => return Err(error),
        }
    }
    host.lock()
        .map_err(|_| io::Error::other("host owner poisoned"))?
        .reconcile_native_effect(producer, identity)
        .map_err(io::Error::other)
}
