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
        let ready = self.active.len() < 3
            && state.effects.iter().any(|e| {
                crate::orchestration::outbox::next_effect(&state, &e.lane)
                    .is_some_and(|n| n.id == e.id)
            });
        let now = self.observe_clock(&mut host, &state, &identity, ready)?;
        if state.outcome != RunOutcome::Pending && self.active.is_empty() {
            let _ = self.sender.send(AgentEvent::Done);
            return Ok(());
        }
        if !ready {
            return Ok(());
        }
        let Some((attempt, effect, kind)) = host
            .reserve_native_effect(&self.producer, &self.run)
            .map_err(io::Error::other)?
        else {
            return Ok(());
        };
        if let Some(limit) = self.ceiling {
            host.change_spending(
                &self.producer,
                attempt.identity(),
                crate::writer_host::native_invocation::SpendingChange::Ceiling(limit.get()),
                crate::protocol::limits::usage::ClockObservation {
                    unix_ms: now,
                    monotonic_delta_ms: 0,
                    trusted: true,
                },
            )
            .map_err(io::Error::other)?;
        }
        drop(host);
        // Endpoint/runtime checks may perform IO. Never hold the H1 owner here.
        let profile = self.profiles.bind(&attempt)?;
        let invocation = owner
            .lock()
            .map_err(|_| io::Error::other("host owner poisoned"))?
            .charge_native_invocation(
                &self.producer,
                &attempt,
                profile,
                &effect,
                kind,
                crate::writer_host::spending::unix_ms().map_err(io::Error::other)?,
            )
            .map_err(io::Error::other)?;
        let id = attempt.identity().attempt.clone();
        let session = self
            .launcher
            .launch(attempt, invocation)
            .map_err(|e| io::Error::other(e.0))?
            .into_session();
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
        let recovery = claim.recovery(&native)?;
        let proof = crate::guardian::RecoveryObserver::recover_cleanup(&recovery)?;
        if let Some(attempt) = state.attempts.iter().find(|a| {
            a.identity.attempt.0.as_str() == native.attempt
                && a.identity.nonce.as_str() == native.nonce
        }) {
            host.lock()
                .map_err(|_| io::Error::other("host owner poisoned"))?
                .native_completed(&producer, &attempt.identity, false, &proof)
                .map_err(io::Error::other)?;
        } else {
            return Err(io::Error::other("private attempt missing from H1"));
        }
        claim.retire(&proof)?;
    }
    for attempt in &state.attempts {
        if !matches!(
            attempt.state,
            crate::orchestration::orchestration_records::AttemptState::Retired(_)
        ) {
            // While this same lock is held, no private reservation means launch
            // never began OR an earlier cleanup proof already retired its file.
            host.lock()
                .map_err(|_| io::Error::other("host owner poisoned"))?
                .reconcile_native_effect(&producer, &attempt.identity)
                .map_err(io::Error::other)?;
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
