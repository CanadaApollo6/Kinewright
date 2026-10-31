//! Host lifecycle for the one durable effect session. No provider work holds H3.
#![cfg(target_os = "linux")]
use super::{HostAuth, HostCommandOwner};
use crate::{
    effect_pump::{EffectPump, ProfileFactory},
    execution_claim::MachineClaim,
    native_launcher::NativeGuardianLauncher,
    orchestration::{
        AdmittedAttempt,
        adapters::{CertifiedConnectorProfile, ConnectorRuntimeChecks},
        orchestration_records::RunId,
    },
};
use kinewright_core::{AgentEvent, AgentSession};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

#[cfg(any(test, feature = "test-util"))]
pub(crate) mod fixture;
#[cfg(any(test, feature = "test-util"))]
pub(super) struct RuntimeWitness {
    pub on_event: Arc<dyn Fn(&AgentEvent) + Send + Sync>,
    pub on_native_events: Arc<dyn Fn(crossbeam_channel::Receiver<AgentEvent>) + Send + Sync>,
}
pub(super) struct RuntimePorts {
    pub profiles: Option<Arc<dyn ProfileFactory>>,
    pub checks: Arc<dyn ConnectorRuntimeChecks + Send + Sync>,
    pub guardian: PathBuf,
    pub ceiling: Option<std::num::NonZeroU64>,
    #[cfg(any(test, feature = "test-util"))]
    pub fixture_claim: Option<PathBuf>,
    #[cfg(any(test, feature = "test-util"))]
    pub fixture_autostart: bool,
    #[cfg(any(test, feature = "test-util"))]
    pub witness: Option<Arc<RuntimeWitness>>,
}
struct Unqualified;
impl ConnectorRuntimeChecks for Unqualified {
    fn probe(
        &self,
        _: &crate::orchestration::adapters::ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<
        crate::orchestration::adapters::RuntimeQualification,
        crate::orchestration::adapters::CertificationRefusal,
    > {
        Err(crate::orchestration::adapters::CertificationRefusal::UnqualifiedSpecification)
    }
}
impl Default for RuntimePorts {
    fn default() -> Self {
        Self {
            profiles: None,
            checks: Arc::new(Unqualified),
            guardian: std::env::current_exe()
                .unwrap_or_default()
                .with_file_name("kinewright-job-guardian"),
            ceiling: None,
            #[cfg(any(test, feature = "test-util"))]
            fixture_claim: None,
            #[cfg(any(test, feature = "test-util"))]
            fixture_autostart: false,
            #[cfg(any(test, feature = "test-util"))]
            witness: None,
        }
    }
}
struct Worker {
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}
#[derive(Default)]
struct State {
    recovered: bool,
    blocked: Option<String>,
    claim: Option<(RunId, Arc<Mutex<MachineClaim>>)>,
    worker: Option<Worker>,
}
pub(super) struct StudioRuntime {
    host: Weak<Mutex<HostCommandOwner>>,
    ports: RuntimePorts,
    state: Mutex<State>,
}
impl StudioRuntime {
    pub(super) fn new(host: &Arc<Mutex<HostCommandOwner>>, ports: RuntimePorts) -> Self {
        let mut ports = ports;
        if ports.profiles.is_none() {
            ports.profiles = Some(Arc::new(RegisteredProfiles {
                host: Arc::downgrade(host),
                checks: ports.checks.clone(),
            }));
        }
        Self {
            host: Arc::downgrade(host),
            ports,
            state: Mutex::default(),
        }
    }
    /// Every host open inspects the private claim. Recovery never launches work.
    pub(super) fn recover_on_open(&self) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| io::Error::other("host closed"))?;
        {
            let mut owner = host
                .lock()
                .map_err(|error| io::Error::other(error.to_string()))?;
            if let Ok(session) = owner.owner() {
                let runs = session.orchestration_runs().map_err(io::Error::other)?;
                for run in runs {
                    let state = owner
                        .owner()
                        .map_err(io::Error::other)?
                        .orchestration_projection(&run)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("run missing"))?;
                    if state.outcome
                        == crate::orchestration::orchestration_records::RunOutcome::Pending
                        || state.cleanup
                            != crate::orchestration::orchestration_records::RunCleanup::Settled
                    {
                        owner.restore_native_run(&run).map_err(io::Error::other)?;
                    }
                }
            }
        }
        let claim = {
            #[cfg(any(test, feature = "test-util"))]
            if let Some(root) = &self.ports.fixture_claim {
                MachineClaim::recover_fixture(root)
            } else {
                MachineClaim::recover()
            }
            #[cfg(not(any(test, feature = "test-util")))]
            MachineClaim::recover()
        };
        let claim = match claim {
            Ok(claim) => claim,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.state
                    .lock()
                    .map_err(|error| io::Error::other(error.to_string()))?
                    .recovered = true;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let (project, run) = claim.claimed_run();
        if host
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?
            .spending_project()
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_str()
            != project
        {
            return Err(io::Error::other("another project's run requires recovery"));
        }
        let run: RunId = serde_json::from_value(serde_json::json!(run))?;
        let (claim, _) = crate::effect_pump::recover_run(&host, claim, &run)?;
        let mut state = self
            .state
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let terminal = host
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?
            .owner()
            .map_err(|error| io::Error::other(error.to_string()))?
            .orchestration_projection(&run)
            .map_err(|error| io::Error::other(error.to_string()))?
            .is_some_and(|run| {
                run.outcome != crate::orchestration::orchestration_records::RunOutcome::Pending
                    && run.cleanup
                        == crate::orchestration::orchestration_records::RunCleanup::Settled
            });
        if terminal {
            claim.finish()?;
        } else {
            state.claim = Some((run, Arc::new(Mutex::new(claim))));
        }
        state.recovered = true;
        Ok(())
    }
    pub(super) fn drive(&self) -> io::Result<()> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| io::Error::other("host closed"))?;
        let mut runtime = self
            .state
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?;
        if !runtime.recovered {
            return Err(io::Error::other("native recovery is not settled"));
        }
        if runtime
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.thread.is_finished())
        {
            return Ok(());
        }
        if let Some(worker) = runtime.worker.take() {
            let _ = worker.thread.join();
        }
        if let Some((run, claim)) = runtime.claim.take() {
            let terminal = host
                .lock()
                .map_err(|error| io::Error::other(error.to_string()))?
                .owner()
                .map_err(io::Error::other)?
                .orchestration_projection(&run)
                .map_err(io::Error::other)?
                .is_some_and(|state| {
                    state.outcome
                        != crate::orchestration::orchestration_records::RunOutcome::Pending
                        && state.cleanup
                            == crate::orchestration::orchestration_records::RunCleanup::Settled
                });
            if terminal {
                match Arc::try_unwrap(claim) {
                    Ok(claim) => claim
                        .into_inner()
                        .map_err(|error| io::Error::other(error.to_string()))?
                        .finish()?,
                    Err(claim) => runtime.claim = Some((run, claim)),
                }
            } else {
                runtime.claim = Some((run, claim));
            }
        }
        let (run, producer) = {
            let owner = host
                .lock()
                .map_err(|error| io::Error::other(error.to_string()))?;
            let mut candidate = None;
            for id in owner.hooks.runs.keys() {
                let Some(state) = owner
                    .owner()
                    .map_err(|error| io::Error::other(error.to_string()))?
                    .orchestration_projection(id)
                    .map_err(|error| io::Error::other(error.to_string()))?
                else {
                    continue;
                };
                if state.outcome != crate::orchestration::orchestration_records::RunOutcome::Pending
                {
                    continue;
                }
                let selection = &state.tasks[0].brief.provider;
                let qualified = crate::orchestration::adapters::certified_connectors()
                    .iter()
                    .any(|spec| spec.selection() == *selection && spec.provider() != "scripted");
                #[cfg(any(test, feature = "test-util"))]
                let qualified = qualified
                    || (self.ports.fixture_claim.is_some() && self.ports.fixture_autostart);
                if qualified {
                    candidate = Some((
                        id.clone(),
                        owner
                            .run_producer(id)
                            .map_err(|error| io::Error::other(error.to_string()))?,
                    ));
                    break;
                }
            }
            let Some(candidate) = candidate else {
                return Ok(());
            };
            candidate
        };
        #[cfg(not(any(test, feature = "test-util")))]
        let fixture = false;
        #[cfg(any(test, feature = "test-util"))]
        let fixture = self.ports.fixture_autostart;
        if !fixture && self.ports.ceiling.is_none() {
            return Err(io::Error::other(
                "qualification requires a positive token ceiling",
            ));
        }
        let claim = if let Some((claimed, claim)) = &runtime.claim {
            if claimed != &run {
                return Err(io::Error::other("another run owns native capacity"));
            }
            claim.clone()
        } else {
            let project = host
                .lock()
                .map_err(|error| io::Error::other(error.to_string()))?
                .spending_project()
                .map_err(|error| io::Error::other(error.to_string()))?;
            let claim = {
                #[cfg(any(test, feature = "test-util"))]
                if let Some(root) = &self.ports.fixture_claim {
                    MachineClaim::acquire_fixture(root, project.as_str(), run.0.as_str())
                } else {
                    MachineClaim::acquire(project.as_str(), run.0.as_str())
                }
                #[cfg(not(any(test, feature = "test-util")))]
                MachineClaim::acquire(project.as_str(), run.0.as_str())
            }?;
            let claim = Arc::new(Mutex::new(claim));
            runtime.claim = Some((run.clone(), claim.clone()));
            claim
        };
        let launcher = NativeGuardianLauncher {
            claim,
            owner: Arc::new(super::native_invocation::HostInvocationOwner::new(
                &host,
                producer.clone(),
            )),
            guardian: self.ports.guardian.clone(),
            checks: self.ports.checks.clone(),
            #[cfg(test)]
            lose_commit_ack: false,
            #[cfg(test)]
            after_release: None,
            #[cfg(test)]
            startup_failure: None,
        };
        let profiles = Arc::new(TaskProfiles {
            host: Arc::downgrade(&host),
            producer: producer.clone(),
            inner: self
                .ports
                .profiles
                .as_ref()
                .expect("runtime profile factory")
                .clone(),
            servers: Mutex::default(),
        });
        let refusal_owner = Arc::downgrade(&host);
        let refusal_producer = producer.clone();
        let refusal_run = run.clone();
        let mut session =
            EffectPump::new(&host, producer, run, launcher, profiles, self.ports.ceiling);
        #[cfg(any(test, feature = "test-util"))]
        if let Some(witness) = &self.ports.witness {
            session.set_native_event_observer(witness.on_native_events.clone());
        }
        let observer = Observer {
            #[cfg(any(test, feature = "test-util"))]
            witness: self.ports.witness.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("kinewright-studio-effects".into())
            .spawn(move || {
                let events = session.events();
                let (stop, _) = crate::pump_session(
                    &mut session,
                    &events,
                    &mut std::iter::once(String::new()),
                    None,
                    crate::ConfirmationPolicy::Studio,
                    &crate::SessionLimits {
                        max_turns: 1,
                        max_wall_time: Duration::from_hours(6),
                        max_tokens: None,
                    },
                    &Arc::new(crate::SharedCounters::default()),
                    &cancelled,
                    &mut { observer },
                );
                #[cfg(test)]
                eprintln!("C4_RUNTIME_STOP {stop:?}");
                if let crate::SessionStop::Cancelled(crate::StopReason::Harness(reason)) = stop
                    && let Some(host) = refusal_owner.upgrade()
                    && let Ok(mut owner) = host.lock()
                    && let Err(error) =
                        owner.record_runtime_refusal(&refusal_producer, &refusal_run, &reason)
                {
                    owner.message =
                        format!("native runtime refusal: {reason}; publication: {error}");
                    #[cfg(test)]
                    eprintln!("C4_RUNTIME_REFUSAL {}", owner.message);
                }
            })?;
        runtime.worker = Some(Worker { cancel, thread });
        Ok(())
    }
    pub(super) fn update(&self) {
        if let Err(error) = self.drive()
            && let Ok(mut state) = self.state.lock()
        {
            state.blocked = Some(error.to_string());
        }
    }
    pub(super) fn stop(&self) {
        let worker = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.worker.take());
        if let Some(worker) = worker {
            worker.cancel.store(true, Ordering::Release);
            let _ = worker.thread.join();
        }
    }
}
impl Drop for StudioRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Observer {
    #[cfg(any(test, feature = "test-util"))]
    witness: Option<Arc<RuntimeWitness>>,
}
impl crate::SessionObserver for Observer {
    fn prepare_studio_stop(
        &mut self,
        _: &crate::SessionStop,
        binding: Option<&kinewright_core::RuntimeRequestBinding>,
    ) -> Result<(), String> {
        // Native release/Stop are H1 transitions. EffectPump never owns a live
        // permission callback; its Drop closes every retained native lifetime.
        if binding.is_some() {
            return Err("native runtime response binding unsupported".into());
        }
        Ok(())
    }

    fn on_tick(&mut self, _: &crate::SessionCounters) -> crate::SessionFlow {
        crate::SessionFlow::Continue
    }

    fn on_event(&mut self, event: &AgentEvent) -> crate::SessionFlow {
        #[cfg(any(test, feature = "test-util"))]
        if let Some(witness) = &self.witness {
            (witness.on_event)(event);
        }
        #[cfg(not(any(test, feature = "test-util")))]
        let _ = event;
        crate::SessionFlow::Continue
    }
}
struct TaskProfiles {
    host: Weak<Mutex<HostCommandOwner>>,
    producer: HostAuth,
    inner: Arc<dyn ProfileFactory>,
    servers: Mutex<
        Vec<(
            crate::orchestration::orchestration_records::AttemptId,
            crate::McpServer,
        )>,
    >,
}
impl ProfileFactory for TaskProfiles {
    fn bind(&self, attempt: &AdmittedAttempt) -> io::Result<CertifiedConnectorProfile> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| io::Error::other("host closed"))?;
        let identity = attempt.identity();
        let server = {
            let mut owner = host
                .lock()
                .map_err(|error| io::Error::other(error.to_string()))?;
            owner
                .prepare_task_endpoint(
                    &self.producer,
                    &identity.run,
                    &identity.task,
                    &identity.attempt,
                    Duration::from_secs(3600),
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
            owner
                .start_task_server(&self.producer, &identity.run, &identity.task)
                .map_err(|error| io::Error::other(error.to_string()))?
        };
        let profile = self.inner.bind_for_task(attempt, server.endpoint())?;
        let mut servers = self
            .servers
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let state = host
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?
            .owner()
            .map_err(|error| io::Error::other(error.to_string()))?
            .orchestration_projection(&identity.run)
            .map_err(|error| io::Error::other(error.to_string()))?
            .ok_or_else(|| io::Error::other("run missing"))?;
        servers.retain(|(id, _)| {
            state.attempts.iter().any(|attempt| {
                &attempt.identity.attempt == id
                    && !matches!(
                        attempt.state,
                        crate::orchestration::orchestration_records::AttemptState::Retired(_)
                    )
            })
        });
        if servers.len() >= 3 {
            return Err(io::Error::other("task endpoint capacity"));
        }
        servers.push((identity.attempt.clone(), server));
        Ok(profile)
    }
}

struct RegisteredProfiles {
    host: Weak<Mutex<HostCommandOwner>>,
    checks: Arc<dyn ConnectorRuntimeChecks + Send + Sync>,
}
impl ProfileFactory for RegisteredProfiles {
    fn bind(&self, _: &AdmittedAttempt) -> io::Result<CertifiedConnectorProfile> {
        Err(io::Error::other("host task MCP binding is required"))
    }
    fn bind_for_task(
        &self,
        attempt: &AdmittedAttempt,
        endpoint: &str,
    ) -> io::Result<CertifiedConnectorProfile> {
        use crate::orchestration::adapters::{AttemptBinding, certify_connector};
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| io::Error::other("host closed"))?;
        let (selection, roots) = {
            let owner = host
                .lock()
                .map_err(|_| io::Error::other("host owner unavailable"))?;
            let state = owner
                .owner()
                .map_err(io::Error::other)?
                .orchestration_projection(&attempt.identity().run)
                .map_err(io::Error::other)?
                .ok_or_else(|| io::Error::other("run missing"))?;
            let task = state
                .tasks
                .iter()
                .find(|task| task.id == attempt.identity().task)
                .ok_or_else(|| io::Error::other("task missing"))?;
            (
                task.brief.provider.clone(),
                owner
                    .config
                    .roots
                    .read
                    .iter()
                    .chain(&owner.config.roots.write)
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            )
        };
        if !crate::orchestration::adapters::certified_connectors()
            .iter()
            .any(|spec| spec.selection() == selection)
        {
            return Err(io::Error::other("saved connector is not qualified"));
        }
        let scratch = std::env::temp_dir().join(format!(
            "kinewright-studio-{}",
            attempt.identity().nonce.as_str()
        ));
        std::fs::create_dir(&scratch)?;
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
        let metadata = std::fs::symlink_metadata(&scratch)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != rustix::process::getuid().as_raw()
        {
            return Err(io::Error::other("private scratch ownership"));
        }
        let binding = AttemptBinding::issue(attempt, endpoint.into(), roots, scratch)
            .map_err(io::Error::other)?;
        let scratch = binding.working_directory().to_path_buf();
        let profile = match certify_connector(&selection.connector, binding, self.checks.as_ref()) {
            Ok(profile) => profile,
            Err(error) => {
                let _ = std::fs::remove_dir(&scratch);
                return Err(io::Error::other(error));
            }
        };
        if profile.selection() != selection {
            return Err(io::Error::other("saved connector policy changed"));
        }
        Ok(profile)
    }
}

impl HostCommandOwner {
    fn record_runtime_refusal(
        &mut self,
        producer: &HostAuth,
        run: &RunId,
        reason: &str,
    ) -> Result<(), super::HostError> {
        use crate::orchestration::orchestration_records::{
            RunOutcome, RunPhase, RunWait, WaitDetail,
        };
        let mut state = self.spending_scope(producer, run)?;
        if state.outcome != RunOutcome::Pending {
            return Ok(());
        }
        let reason = super::bounded_text(reason, 1024);
        let reference = self.owner_mut()?.admit_host_grant(reason.as_bytes())?;
        state.wait = RunWait::Provider(WaitDetail {
            reason: reference,
            missing: vec![],
            attempt: None,
        });
        if state.phase != RunPhase::Paused {
            state.phase = RunPhase::Interrupted;
        }
        let identity = state
            .attempts
            .last()
            .map(|attempt| attempt.identity.clone())
            .unwrap_or(crate::orchestration::ExecutionIdentity {
                version: 1,
                project: self.spending_project()?,
                run: run.clone(),
                task: state.tasks[0].id.clone(),
                attempt: crate::orchestration::orchestration_records::AttemptId(run.0.clone()),
                nonce: run.0.clone(),
                owner_instance: run.0.clone(),
            });
        self.publish_native(producer, state, &identity, vec![])
    }
}
