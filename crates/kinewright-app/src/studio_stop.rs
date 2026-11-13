//! Run controls are kept before dispatch and retried with their original revision.
use super::{Feedback, FeedbackPort, HostFeedback, Keeping, RunId, same_identity};
use kinewright_agent::writer_host::{HostCommand, ScopedOrchestrationRequest};
use kinewright_project::{
    bundle_records::ProjectId,
    orchestration_commit::CommandDecision,
    orchestration_records::{
        AuthorityBinding, OpaqueRevision, OrchestrationCommand, OrchestrationEnvelope, RunAction,
        RunActionKind, RunIntent,
    },
    receipts::IssuerId,
};
use serde::{Deserialize, Serialize};

const CONTROL_LIMIT: usize = 64;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingControl {
    pub(crate) request: ScopedOrchestrationRequest,
    #[serde(skip)]
    pub(super) recovery: Option<super::HostClient>,
}
impl PendingControl {
    pub(crate) fn run(&self) -> Option<RunId> {
        self.request
            .run
            .as_ref()
            .and_then(|run| same_identity(run).ok())
    }
    pub(crate) fn action(&self) -> Option<RunActionKind> {
        match &self.request.envelope.command {
            OrchestrationCommand::ControlRun(intent) => match intent.action {
                RunAction::Stop => Some(RunActionKind::Stop),
                RunAction::Continue => Some(RunActionKind::Continue),
                _ => None,
            },
            _ => None,
        }
    }
    pub(crate) fn valid(&self, project: &ProjectId) -> bool {
        let OrchestrationCommand::ControlRun(intent) = &self.request.envelope.command else {
            return false;
        };
        self.request.envelope.project == *project
            && self.request.consume_batch.is_none()
            && self.request.run.as_ref() == Some(&intent.run_id)
            && self.action().is_some()
            && intent.command_id == self.request.envelope.id
            && self.request.envelope.computed_digest().ok().as_ref()
                == Some(&self.request.envelope.digest)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ControlReply {
    Accepted,
    Refused(String),
    Unknown(String),
    RecoveryUnknown(String),
    Busy(String),
}
pub(super) fn prepare(
    port: &HostFeedback<'_>,
    run: &RunId,
    action: RunActionKind,
    revision: &OpaqueRevision,
    issuer: &mut Option<IssuerId>,
) -> Result<ScopedOrchestrationRequest, String> {
    let (verb, action_value) = match action {
        RunActionKind::Stop => ("Stop", RunAction::Stop),
        RunActionKind::Continue => ("Continue", RunAction::Continue),
        _ => return Err("This run control is unavailable.".into()),
    };
    let page = port.run_view(run)?;
    if !page.allowed_actions.contains(&action) {
        return Err(format!("{verb} is not available from this attachment."));
    }
    if &page.projection.control_revision != revision {
        return Err(format!(
            "The run changed. Check its status and press {verb} again."
        ));
    }
    let metadata = port
        .request(HostCommand::IssueMetadataIssuer {
            issuer: issuer.clone(),
        })?
        .metadata_issuer
        .ok_or_else(|| format!("The host returned no {verb} envelope."))?;
    *issuer = Some(metadata.envelope.id.issuer.clone());
    let mut envelope = OrchestrationEnvelope {
        id: metadata.envelope.id.clone(),
        digest: serde_json::from_value(serde_json::json!("0".repeat(64)))
            .map_err(|e| e.to_string())?,
        project: metadata.envelope.project_id,
        expected_generation: metadata.root_generation,
        authority: AuthorityBinding {
            grant_ref: metadata.grant_ref,
            owner_epoch: metadata.envelope.owner_epoch,
            endpoint: None,
            attempt: None,
        },
        command: OrchestrationCommand::ControlRun(RunIntent {
            command_id: metadata.envelope.id,
            run_id: same_identity(run)?,
            expected_control_revision: revision.clone(),
            action: action_value,
        }),
    };
    envelope.digest = envelope.computed_digest().map_err(|e| format!("{e:?}"))?;
    Ok(ScopedOrchestrationRequest {
        run: Some(same_identity(run)?),
        envelope,
        consume_batch: None,
    })
}
pub(super) fn send(port: &HostFeedback<'_>, request: &ScopedOrchestrationRequest) -> ControlReply {
    let verb = if matches!(&request.envelope.command, OrchestrationCommand::ControlRun(intent) if intent.action == RunAction::Continue)
    {
        "Continue"
    } else {
        "Stop"
    };
    let reply = match port.client.request(HostCommand::DispatchOrchestration {
        request: Box::new(request.clone()),
    }) {
        Ok(reply) => reply,
        Err(super::HostError::Refused(error)) if error.is_busy() => {
            return ControlReply::Busy(error.message);
        }
        Err(super::HostError::Refused(error)) if error.retry_after_recovery() => {
            return ControlReply::RecoveryUnknown(error.message);
        }
        Err(error) => return ControlReply::Unknown(error.to_string()),
    };
    if let Some(error) = reply.error {
        if error.is_busy() {
            return ControlReply::Busy(error.message);
        }
        if error.retry_after_recovery() {
            return ControlReply::RecoveryUnknown(error.message);
        }
        return if matches!(
            error.code.as_str(),
            "durability_unknown" | "unsettled" | "recovery_frozen"
        ) {
            ControlReply::Unknown(error.message)
        } else {
            ControlReply::Refused(error.message)
        };
    }
    let Some(receipt) = reply.orchestration else {
        return ControlReply::Unknown(format!("The host returned no {verb} receipt."));
    };
    if receipt.envelope != request.envelope {
        return ControlReply::Unknown(format!("The host returned a different {verb} receipt."));
    }
    match receipt.decision {
        CommandDecision::Applied => ControlReply::Accepted,
        CommandDecision::Refused(_) => ControlReply::Refused(format!(
            "The run changed. Check its status and press {verb} again."
        )),
    }
}
impl Feedback {
    pub(crate) fn stop_run(
        &mut self,
        port: &mut dyn FeedbackPort,
        run: &RunId,
    ) -> Result<ControlReply, String> {
        self.control_run(port, run, RunActionKind::Stop)
    }
    pub(crate) fn continue_run(
        &mut self,
        port: &mut dyn FeedbackPort,
        run: &RunId,
    ) -> Result<ControlReply, String> {
        self.control_run(port, run, RunActionKind::Continue)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Keep request persistence, recovery arming and receipt handling in one control lifecycle"
    )]
    fn control_run(
        &mut self,
        port: &mut dyn FeedbackPort,
        run: &RunId,
        action: RunActionKind,
    ) -> Result<ControlReply, String> {
        let verb = if action == RunActionKind::Continue {
            "Continue"
        } else {
            "Stop"
        };
        let request =
            if let Some(pending) = self.controls.iter().find(|pending| {
                pending.run().as_ref() == Some(run) && pending.action() == Some(action)
            }) {
                pending.request.clone()
            } else {
                if self.controls.len() >= CONTROL_LIMIT {
                    return Err(format!("Too many {verb} requests are still unconfirmed."));
                }
                let state = self
                    .rows
                    .iter()
                    .find(|row| row.run.as_ref() == Some(run))
                    .and_then(|row| row.state.as_ref())
                    .and_then(|state| state.as_ref().ok())
                    .ok_or_else(|| {
                        format!(
                            "Check the run's status before {} it.",
                            if action == RunActionKind::Continue {
                                "continuing"
                            } else {
                                "stopping"
                            }
                        )
                    })?;
                if !(match action {
                    RunActionKind::Continue => state.can_continue,
                    RunActionKind::Stop => state.can_stop,
                    _ => false,
                }) {
                    return Err(format!("{verb} is not available from this attachment."));
                }
                let request =
                    port.prepare_control(run, action, &state.control_revision, &mut self.issuer)?;
                let pending = PendingControl {
                    request: request.clone(),
                    recovery: None,
                };
                let project = self.keeping.project().ok_or("Save this project first.")?;
                if !pending.valid(project) {
                    return Err(super::HANDOFF_REFUSAL.into());
                }
                self.controls.push(pending);
                self.changed = true;
                if let Err(why) = self.retain_for_dispatch() {
                    self.controls.retain(|pending| pending.request != request);
                    return Err(format!(
                        "{verb} was not sent: its request could not be kept ({why})."
                    ));
                }
                request
            };
        let Keeping::Kept { project, .. } = &self.keeping else {
            return Err(format!(
                "The {verb} request cannot be kept for this project."
            ));
        };
        if !(PendingControl {
            request: request.clone(),
            recovery: None,
        })
        .valid(project)
        {
            return Err(super::HANDOFF_REFUSAL.into());
        }
        let reply = port.send_control(&request);
        if let Some(pending) = self
            .controls
            .iter_mut()
            .find(|pending| pending.request == request)
        {
            pending.recovery = if matches!(reply, ControlReply::RecoveryUnknown(_)) {
                port.host_client().cloned()
            } else {
                None
            };
        }
        if !matches!(
            reply,
            ControlReply::Unknown(_) | ControlReply::RecoveryUnknown(_) | ControlReply::Busy(_)
        ) {
            self.controls.retain(|pending| pending.request != request);
            self.changed = true;
        }
        self.observe_runs(&*port);
        self.message = Some(match &reply {
            // The run row follows durable cleanup status on each poll.
            ControlReply::Accepted => format!("{verb} accepted."),
            ControlReply::Refused(why) => why.clone(),
            ControlReply::Busy(why) if action == RunActionKind::Continue => {
                format!("Continue is kept and waiting ({why}). Checking the same request again.")
            }
            ControlReply::Busy(why) => {
                format!("{verb} is kept and waiting ({why}). Retry {verb} sends the same request.")
            }
            ControlReply::Unknown(why) if action == RunActionKind::Continue => {
                format!("Continue is unconfirmed ({why}). Checking the same request again.")
            }
            ControlReply::RecoveryUnknown(why) => {
                format!(
                    "{verb} is waiting for recovery ({why}). The same request retries when this host is ready."
                )
            }
            ControlReply::Unknown(why) => {
                format!("{verb} is unconfirmed ({why}). Retry {verb} sends the same request.")
            }
        });
        if self.changed {
            let _ = self.persist();
        }
        Ok(reply)
    }
    pub(crate) fn restore_controls(&mut self, project: &ProjectId, pending: Vec<PendingControl>) {
        for pending in pending.into_iter().take(CONTROL_LIMIT) {
            if pending.valid(project)
                && !self
                    .controls
                    .iter()
                    .any(|existing| existing.request == pending.request)
                && self.controls.len() < CONTROL_LIMIT
            {
                self.controls.push(pending);
            }
        }
    }
    pub(crate) fn retry_ready_controls(&mut self, port: &mut dyn FeedbackPort) {
        let runs = self
            .controls
            .iter()
            .filter(|pending| {
                pending.action() == Some(RunActionKind::Continue) || pending.recovery.is_some()
            })
            .filter_map(|pending| Some((pending.run()?, pending.action()?)))
            .collect::<Vec<_>>();
        for (run, action) in runs {
            let arm = self
                .controls
                .iter_mut()
                .find(|pending| {
                    pending.run().as_ref() == Some(&run) && pending.action() == Some(action)
                })
                .and_then(|pending| pending.recovery.take());
            match self.control_run(port, &run, action) {
                Ok(ControlReply::Busy(_)) if arm.is_some() => {
                    if let Some(pending) = self.controls.iter_mut().find(|pending| {
                        pending.run().as_ref() == Some(&run) && pending.action() == Some(action)
                    }) {
                        pending.recovery = arm;
                    }
                    self.message = Some("The host is busy. Your control request is kept and waiting; checking again on the next poll.".into());
                }
                Err(why) => self.message = Some(why),
                _ => {}
            }
        }
    }

    pub(crate) fn poll_runs(&mut self, port: &mut dyn FeedbackPort) {
        self.poll_readiness = Some(match port.poll_status() {
            Ok((readiness, runtime)) => {
                self.runtime_status = runtime;
                readiness
            }
            Err(_) => super::RecoveryReadiness::Blocked,
        });
        self.retry_after_recovery(port);
        self.observe_runs(&*port);
        self.poll_readiness = None;
    }

    pub(crate) fn retry_after_recovery(&mut self, port: &mut dyn FeedbackPort) {
        let same_host = |saved: &super::HostClient| {
            port.host_client()
                .is_some_and(|client| saved.same_attachment(client))
        };
        if self
            .revise_recovery
            .as_ref()
            .is_some_and(|saved| !same_host(saved))
        {
            self.revise_recovery = None;
        }
        for pending in &mut self.controls {
            if pending
                .recovery
                .as_ref()
                .is_some_and(|saved| !same_host(saved))
            {
                pending.recovery = None;
            }
        }
        let readiness = if let Some(readiness) = self.poll_readiness {
            readiness
        } else if let Ok(readiness) = port.recovery_readiness() {
            readiness
        } else {
            return;
        };
        if readiness == super::RecoveryReadiness::Stopping {
            self.revise_recovery = None;
            for pending in &mut self.controls {
                pending.recovery = None;
            }
        }
        if readiness != super::RecoveryReadiness::Ready {
            return;
        }
        #[cfg(all(test, target_os = "linux"))]
        if let Some(witness) = self.recovery_ready_witness.take() {
            witness();
        }
        if let Some(arm) = self.revise_recovery.take()
            && matches!(self.start_run(port), Some(super::RunStart::Busy(_)))
        {
            self.revise_recovery = Some(arm);
            self.message = Some("The host is busy. Your Revise cut request is kept and waiting; checking again on the next poll.".into());
        }
        self.retry_ready_controls(port);
    }
    pub(crate) fn retry_controls(&mut self, port: &mut dyn FeedbackPort) {
        let runs = self
            .controls
            .iter()
            .filter_map(|pending| Some((pending.run()?, pending.action()?)))
            .collect::<Vec<_>>();
        for (run, action) in runs {
            if let Err(why) = self.control_run(port, &run, action) {
                self.message = Some(why);
            }
        }
    }
}
