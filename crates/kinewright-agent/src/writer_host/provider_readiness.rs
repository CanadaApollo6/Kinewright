//! One unpaid readiness plan for every provider a run can use. Probes run outside H3.
#![cfg(target_os = "linux")]
use super::{
    AttachmentRole, HostCommand, HostCommandOwner, HostError, HostRefusal, HostReply, HostRequest,
};
use crate::orchestration::{
    adapters::{CertificationRefusal, ConnectorRuntimeChecks, available_connectors},
    orchestration_records::{
        OrchestrationCommand, ProviderSelectionRecord, RunAction, RunId, RunPhase, RunWait,
        TaskState,
    },
};
use std::sync::{Arc, Mutex};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ReadinessPlan {
    selections: Vec<ProviderSelectionRecord>,
}
pub(crate) fn provider_words(provider: &str, why: &impl std::fmt::Display) -> String {
    let provider = match provider {
        "codex" => "Codex",
        "claude" => "Claude",
        _ => "The selected provider",
    };
    format!(
        "{provider} is not ready: {why}. Check its installation and subscription sign-in, then review Director and Specialists in Settings and try again. Your choices have not been substituted."
    )
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ProviderSetupRefusal(String);
pub(crate) fn provider_error(provider: &str, why: &impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(ProviderSetupRefusal(provider_words(provider, why)))
}
impl ReadinessPlan {
    pub(super) fn check(&self, checks: &dyn ConnectorRuntimeChecks) -> std::io::Result<()> {
        self.check_except(checks, None)
    }
    fn check_except(
        &self,
        checks: &dyn ConnectorRuntimeChecks,
        bound: Option<&str>,
    ) -> std::io::Result<()> {
        let mut providers = std::collections::BTreeSet::new();
        for selection in &self.selections {
            #[cfg(any(test, feature = "test-util"))]
            if matches!(selection.connector.as_str(), "scripted" | "scripted-claude") {
                continue;
            }
            if bound == Some(selection.connector.as_str())
                || !providers.insert(&selection.connector)
            {
                continue;
            }
            let spec = available_connectors()
                .iter()
                .copied()
                .find(|s| s.provider() == selection.connector);
            #[cfg(any(test, feature = "test-util"))]
            let spec = spec.or_else(|| {
                crate::orchestration::adapters::fixture_connectors()
                    .iter()
                    .copied()
                    .find(|s| s.provider() == selection.connector)
            });
            let spec = spec.ok_or_else(|| {
                certification_error(&selection.connector, CertificationRefusal::UnknownProvider)
            })?;
            checks
                .preflight(spec)
                .map_err(|why| certification_error(&selection.connector, why))?;
        }
        Ok(())
    }
}
impl HostCommandOwner {
    pub(super) fn readiness_for_director(
        &self,
        director: &ProviderSelectionRecord,
    ) -> Result<ReadinessPlan, String> {
        let specialist =
            super::configured_specialist(self.config.scheduler_settings.as_deref(), director)?;
        Ok(ReadinessPlan {
            selections: vec![director.clone(), specialist],
        })
    }
    pub(super) fn readiness_for_run(&self, run: &RunId) -> Result<ReadinessPlan, String> {
        let state = self
            .owner()
            .map_err(|e| e.to_string())?
            .orchestration_projection(run)
            .map_err(|e| e.to_string())?
            .ok_or("The run is unavailable.")?;
        let mut plan = self.readiness_for_director(&state.tasks[0].brief.provider)?;
        for task in &state.tasks {
            if !matches!(task.state, TaskState::Terminal(_) | TaskState::Cancelled)
                && !plan.selections.contains(&task.brief.provider)
            {
                plan.selections.push(task.brief.provider.clone());
            }
        }
        Ok(plan)
    }
    pub(crate) fn wait_for_provider(&mut self, run: &RunId, reason: &str) -> Result<(), HostError> {
        let state = self
            .owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| HostRefusal::new("provider_wait", "The run is unavailable."))?;
        if crate::orchestration::machine::has_unknown_send(&state) {
            if let Some(runtime) = self.hooks.runs.get_mut(run) {
                runtime.pending_provider_wait = None;
            }
            return Ok(());
        }
        self.hooks
            .runs
            .get_mut(run)
            .ok_or_else(|| HostRefusal::new("provider_wait", "The run is unavailable."))?
            .pending_provider_wait = Some(reason.to_owned());
        let reason = if reason == CHECK_RETRY {
            reason.to_owned()
        } else {
            format!("{reason} After fixing Settings, press Continue.")
        };
        let same = state.phase == RunPhase::Interrupted
            && matches!(&state.wait, RunWait::ProviderSettings(detail) if detail.reason.sha256 == kinewright_media::sha256_bytes(reason.as_bytes()));
        if !same {
            let producer = self.run_producer(run)?;
            self.record_runtime_wait(&producer, run, &reason, false, RunWait::ProviderSettings)?;
        }
        if let Some(runtime) = self.hooks.runs.get_mut(run) {
            runtime.pending_provider_wait = None;
        }
        Ok(())
    }
    pub(super) fn retry_pending_provider_wait(&mut self, run: &RunId) -> Result<(), HostError> {
        if let Some(reason) = self
            .hooks
            .runs
            .get(run)
            .and_then(|run| run.pending_provider_wait.clone())
        {
            self.wait_for_provider(run, &reason)?;
        }
        Ok(())
    }
    fn command_readiness(
        &self,
        bearer: &str,
        request: &HostRequest,
    ) -> Result<Option<ReadinessPlan>, String> {
        if !matches!(
            request.command,
            HostCommand::ReviseCut { .. } | HostCommand::DispatchOrchestration { .. }
        ) {
            return Ok(None);
        }
        // An uncertain original publication must settle before fresh settings are read.
        if self.hooks.pending.is_some() {
            return Ok(None);
        }
        let Ok((index, _, _)) = self.authorize_request(Some(bearer), request) else {
            return Ok(None);
        };
        match &request.command {
            HostCommand::ReviseCut { request }
                if self.attachments[index].role == AttachmentRole::Editor =>
            {
                if self
                    .owner()
                    .map_err(|e| e.to_string())?
                    .feedback_index()
                    .map_err(|e| e.to_string())?
                    .batches
                    .get(&request.batch_id)
                    .is_some_and(|batch| batch.consumed_by.is_some())
                {
                    return Ok(None);
                }
                self.readiness_for_director(&self.revise_director()?)
                    .map(Some)
            }
            HostCommand::DispatchOrchestration { request }
                if self.authorize_orchestration(index, request).is_ok() =>
            {
                if let Some(run) = &request.run
                    && self
                        .owner()
                        .map_err(|e| e.to_string())?
                        .orchestration_receipt(run, &request.envelope.id)
                        .map_err(|e| e.to_string())?
                        .is_some()
                {
                    return Ok(None);
                }
                match &request.envelope.command {
                    OrchestrationCommand::StartRun(start) => self
                        .readiness_for_director(&start.director.provider)
                        .map(Some),
                    OrchestrationCommand::AssignTask(_) => self
                        .readiness_for_run(
                            request
                                .run
                                .as_ref()
                                .ok_or("The assignment run is unavailable.")?,
                        )
                        .map(Some),
                    OrchestrationCommand::ControlRun(intent)
                        if matches!(
                            intent.action,
                            RunAction::Continue | RunAction::SendMessage { .. }
                        ) =>
                    {
                        self.readiness_for_run(&intent.run_id).map(Some)
                    }
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }
    fn readiness_reply(&mut self, command: &HostCommand, failure: &std::io::Error) -> HostReply {
        let reason = if setup_failure(failure) {
            failure.to_string()
        } else {
            match command {
                HostCommand::ReviseCut { .. } => CHECK_REVISE,
                HostCommand::DispatchOrchestration { request } if request.run.is_none() => {
                    CHECK_START
                }
                _ => CHECK_RETRY,
            }
            .into()
        };
        if matches!(command, HostCommand::ReviseCut { .. }) {
            return HostReply {
                status: self.status(),
                revise_result: Some(kinewright_project::studio_records::ReviseResult::Waiting {
                    reason: reason.clone(),
                }),
                ..HostReply::default()
            };
        }
        if let HostCommand::DispatchOrchestration { request } = command
            && let Some(run) = &request.run
            && let Err(error) = wait_for_failure(self, run, failure)
        {
            return HostReply {
                status: self.status(),
                error: Some(error.refusal()),
                ..HostReply::default()
            };
        }
        HostReply {
            status: self.status(),
            error: Some(HostRefusal::new("provider_wait", &reason)),
            ..HostReply::default()
        }
    }
}
/// The HTTP and host-MCP routes share this boundary. The plan is checked again
/// after probes, so a Settings edit during a contained probe cannot pass unnoticed.
pub(super) fn execute(
    host: &Arc<Mutex<HostCommandOwner>>,
    bearer: &str,
    request: HostRequest,
) -> HostReply {
    let (plan, checks) = match host.lock() {
        Ok(owner) => (
            owner.command_readiness(bearer, &request),
            owner.native_readiness_checks.clone(),
        ),
        Err(_) => {
            return HostReply {
                error: Some(HostRefusal::new("retention_lost", super::RETENTION_LOST)),
                ..HostReply::default()
            };
        }
    };
    let checked = match &plan {
        Ok(Some(plan)) => checks
            .as_ref()
            .ok_or_else(|| {
                std::io::Error::other(
                    "Provider checks are unavailable. Review Director and Specialists in Settings.",
                )
            })
            .and_then(|checks| plan.check(checks.as_ref())),
        Ok(None) => Ok(()),
        Err(why) => Err(choice_error(why)),
    };
    let Ok(mut owner) = host.lock() else {
        return HostReply {
            error: Some(HostRefusal::new("retention_lost", super::RETENTION_LOST)),
            ..HostReply::default()
        };
    };
    if owner.authorize_request(Some(bearer), &request).is_err() {
        return owner.execute_bearer(Some(bearer), request);
    }
    if owner.command_readiness(bearer, &request) != plan {
        return owner.readiness_reply(&request.command, &choice_error("Your provider choices changed while they were checked. Review Director and Specialists in Settings, then try again."));
    }
    if let Err(why) = checked {
        return owner.readiness_reply(&request.command, &why);
    }
    owner.execute_bearer(Some(bearer), request)
}
/// Every effect, including a restored or queued one, rechecks before reservation.
pub(crate) fn check_run(
    host: &Arc<Mutex<HostCommandOwner>>,
    run: &RunId,
) -> std::io::Result<ReadinessPlan> {
    let (plan, checks) = {
        let owner = host
            .lock()
            .map_err(|_| std::io::Error::other("host closed"))?;
        (
            owner.readiness_for_run(run),
            owner.native_readiness_checks.clone(),
        )
    };
    let result = plan.as_ref().map_err(|why| choice_error(why)).and_then(|plan| {
        checks
            .ok_or_else(|| {
                std::io::Error::other("Provider checks are unavailable. Review Director and Specialists in Settings.")
            })
            .and_then(|checks| plan.check(checks.as_ref()))
    });
    let mut owner = host
        .lock()
        .map_err(|_| std::io::Error::other("host closed"))?;
    let result = if owner.readiness_for_run(run) == plan {
        result
    } else {
        Err(choice_error(
            "Your provider choices changed while they were checked. Review Director and Specialists in Settings, then try again.",
        ))
    };
    if let Err(why) = result {
        wait_for_failure(&mut owner, run, &why).map_err(std::io::Error::other)?;
        return Err(why);
    }
    plan.map_err(std::io::Error::other)
}

pub(crate) fn wait_on_launch_error(
    host: &Arc<Mutex<HostCommandOwner>>,
    run: &RunId,
    error: &std::io::Error,
    resumable: bool,
) -> std::io::Result<()> {
    let mut owner = host
        .lock()
        .map_err(|_| std::io::Error::other("host owner poisoned"))?;
    if resumable {
        wait_for_failure(&mut owner, run, error).map_err(std::io::Error::other)?;
    } else {
        let producer = owner.run_producer(run).map_err(std::io::Error::other)?;
        owner
            .record_runtime_wait(&producer, run, &format!("{error} The send outcome is unknown. Press Stop, then Reopen to review your notes before starting another run. Continue cannot retry this send."), false, RunWait::Provider)
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

/// Bind already probed the launching provider. Recheck every other provider,
/// then compare this plan under the SAME owner lock that charges the invocation.
pub(crate) fn check_bound(
    host: &Arc<Mutex<HostCommandOwner>>,
    run: &RunId,
    plan: &ReadinessPlan,
    provider: &str,
) -> std::io::Result<()> {
    let checks = {
        let mut owner = host
            .lock()
            .map_err(|_| std::io::Error::other("host closed"))?;
        plan.current(&mut owner, run)?;
        owner.native_readiness_checks.clone()
    };
    let result = checks
        .ok_or_else(|| {
            std::io::Error::other(
                "Provider checks are unavailable. Review Director and Specialists in Settings.",
            )
        })
        .and_then(|checks| plan.check_except(checks.as_ref(), Some(provider)));
    if let Err(why) = result {
        let mut owner = host
            .lock()
            .map_err(|_| std::io::Error::other("host closed"))?;
        wait_for_failure(&mut owner, run, &why).map_err(std::io::Error::other)?;
        return Err(why);
    }
    Ok(())
}
impl ReadinessPlan {
    pub(crate) fn current(&self, owner: &mut HostCommandOwner, run: &RunId) -> std::io::Result<()> {
        let current = owner.readiness_for_run(run);
        if current.as_ref() == Ok(self) {
            return Ok(());
        }
        let why = current.err().unwrap_or_else(|| "Your provider choices changed while they were checked. Review Director and Specialists in Settings, then try again.".into());
        owner
            .wait_for_provider(run, &why)
            .map_err(std::io::Error::other)?;
        Err(std::io::Error::other(ProviderSetupRefusal(why)))
    }
}
pub(crate) const CHECK_RETRY: &str =
    "The provider check failed before anything was sent. Continue to try again.";
pub(crate) const CHECK_REVISE: &str =
    "The provider check failed before anything was sent. Press Start run to try again.";
pub(crate) const CHECK_START: &str =
    "The provider check failed before anything was sent. Start the run again to try again.";
pub(crate) const MODEL_CHECK_WAIT: &str = "The run is waiting for the person to retry the provider check. End your turn now. Do not retry this call.";

pub(crate) fn setup_failure(error: &std::io::Error) -> bool {
    matches!(error.get_ref(), Some(cause) if cause.is::<ProviderSetupRefusal>())
}
pub(crate) fn certification_error(provider: &str, error: CertificationRefusal) -> std::io::Error {
    if matches!(
        error,
        CertificationRefusal::UnknownProvider
            | CertificationRefusal::UnqualifiedVersion
            | CertificationRefusal::ApiAuthentication
            | CertificationRefusal::UnknownAuthentication
            | CertificationRefusal::EffectiveAuthentication
            | CertificationRefusal::ExecutableIdentity
            | CertificationRefusal::ExecutableResolution(_)
            | CertificationRefusal::ModelUnavailable
    ) {
        provider_error(provider, &error)
    } else {
        std::io::Error::other(error)
    }
}
pub(super) fn model_wait() -> &'static str {
    "The run is waiting for the person to fix Settings. End your turn now. Do not retry this call."
}

fn choice_error(reason: &str) -> std::io::Error {
    std::io::Error::other(ProviderSetupRefusal(reason.into()))
}
fn wait_for_failure(
    owner: &mut HostCommandOwner,
    run: &RunId,
    failure: &std::io::Error,
) -> Result<(), HostError> {
    if setup_failure(failure) {
        owner.wait_for_provider(run, &failure.to_string())
    } else {
        owner.wait_for_provider(run, CHECK_RETRY)
    }
}
