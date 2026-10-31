//! A Stop intent is kept before dispatch and retried with its original revision.
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

const STOP_LIMIT: usize = 64;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingStop {
    pub(crate) request: ScopedOrchestrationRequest,
}
impl PendingStop {
    pub(crate) fn run(&self) -> Option<RunId> {
        self.request
            .run
            .as_ref()
            .and_then(|run| same_identity(run).ok())
    }
    pub(crate) fn valid(&self, project: &ProjectId) -> bool {
        let OrchestrationCommand::ControlRun(intent) = &self.request.envelope.command else {
            return false;
        };
        self.request.envelope.project == *project
            && self.request.consume_batch.is_none()
            && self.request.run.as_ref() == Some(&intent.run_id)
            && intent.action == RunAction::Stop
            && intent.command_id == self.request.envelope.id
            && self.request.envelope.computed_digest().ok().as_ref()
                == Some(&self.request.envelope.digest)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StopReply {
    Accepted,
    Refused(String),
    Unknown(String),
}
pub(super) fn prepare(
    port: &HostFeedback<'_>,
    run: &RunId,
    revision: &OpaqueRevision,
    issuer: &mut Option<IssuerId>,
) -> Result<ScopedOrchestrationRequest, String> {
    let page = port.run_view(run)?;
    if !page.allowed_actions.contains(&RunActionKind::Stop) {
        return Err("Stop is not available from this attachment.".into());
    }
    if &page.projection.control_revision != revision {
        return Err("The run changed. Check its status and press Stop again.".into());
    }
    let metadata = port
        .request(HostCommand::IssueMetadataIssuer {
            issuer: issuer.clone(),
        })?
        .metadata_issuer
        .ok_or("The host returned no Stop envelope.")?;
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
            action: RunAction::Stop,
        }),
    };
    envelope.digest = envelope.computed_digest().map_err(|e| format!("{e:?}"))?;
    Ok(ScopedOrchestrationRequest {
        run: Some(same_identity(run)?),
        envelope,
        consume_batch: None,
    })
}
pub(super) fn send(port: &HostFeedback<'_>, request: &ScopedOrchestrationRequest) -> StopReply {
    let reply = match port.client.request(HostCommand::DispatchOrchestration {
        request: Box::new(request.clone()),
    }) {
        Ok(reply) => reply,
        Err(error) => return StopReply::Unknown(error.to_string()),
    };
    if let Some(error) = reply.error {
        return if matches!(
            error.code.as_str(),
            "durability_unknown" | "unsettled" | "recovery_frozen"
        ) {
            StopReply::Unknown(error.message)
        } else {
            StopReply::Refused(error.message)
        };
    }
    let Some(receipt) = reply.orchestration else {
        return StopReply::Unknown("The host returned no Stop receipt.".into());
    };
    if receipt.envelope != request.envelope {
        return StopReply::Unknown("The host returned a different Stop receipt.".into());
    }
    match receipt.decision {
        CommandDecision::Applied => StopReply::Accepted,
        CommandDecision::Refused(_) => {
            StopReply::Refused("The run changed. Check its status and press Stop again.".into())
        }
    }
}
impl Feedback {
    pub(crate) fn stop_run(
        &mut self,
        port: &mut dyn FeedbackPort,
        run: &RunId,
    ) -> Result<StopReply, String> {
        let request = if let Some(pending) = self
            .stops
            .iter()
            .find(|pending| pending.run().as_ref() == Some(run))
        {
            pending.request.clone()
        } else {
            if self.stops.len() >= STOP_LIMIT {
                return Err("Too many Stop requests are still unconfirmed.".into());
            }
            let state = self
                .rows
                .iter()
                .find(|row| row.run.as_ref() == Some(run))
                .and_then(|row| row.state.as_ref())
                .and_then(|state| state.as_ref().ok())
                .ok_or("Check the run's status before stopping it.")?;
            if !state.can_stop {
                return Err("Stop is not available from this attachment.".into());
            }
            let request = port.prepare_stop(run, &state.control_revision, &mut self.issuer)?;
            let pending = PendingStop {
                request: request.clone(),
            };
            let project = self.keeping.project().ok_or("Save this project first.")?;
            if !pending.valid(project) {
                return Err(super::HANDOFF_REFUSAL.into());
            }
            self.stops.push(pending);
            self.changed = true;
            if let Err(why) = self.retain_for_dispatch() {
                self.stops
                    .retain(|pending| pending.run().as_ref() != Some(run));
                return Err(format!(
                    "Stop was not sent: its request could not be kept ({why})."
                ));
            }
            request
        };
        let Keeping::Kept { project, .. } = &self.keeping else {
            return Err("The Stop request cannot be kept for this project.".into());
        };
        if !(PendingStop {
            request: request.clone(),
        })
        .valid(project)
        {
            return Err(super::HANDOFF_REFUSAL.into());
        }
        let reply = port.send_stop(&request);
        if !matches!(reply, StopReply::Unknown(_)) {
            self.stops.retain(|pending| pending.request != request);
            self.changed = true;
        }
        self.observe_runs(&*port);
        self.message = Some(match &reply {
            // The run row follows durable cleanup status on each poll.
            StopReply::Accepted => "Stop accepted.".into(),
            StopReply::Refused(why) => why.clone(),
            StopReply::Unknown(why) => {
                format!("Stop is unconfirmed ({why}). Retry Stop sends the same request.")
            }
        });
        if self.changed {
            let _ = self.persist();
        }
        Ok(reply)
    }
    pub(crate) fn restore_stops(&mut self, project: &ProjectId, pending: Vec<PendingStop>) {
        for pending in pending.into_iter().take(STOP_LIMIT) {
            if pending.valid(project)
                && !self
                    .stops
                    .iter()
                    .any(|existing| existing.request == pending.request)
                && self.stops.len() < STOP_LIMIT
            {
                self.stops.push(pending);
            }
        }
    }
    pub(crate) fn retry_stops(&mut self, port: &mut dyn FeedbackPort) {
        let runs = self
            .stops
            .iter()
            .filter_map(PendingStop::run)
            .collect::<Vec<_>>();
        for run in runs {
            if let Err(why) = self.stop_run(port, &run) {
                self.message = Some(why);
            }
        }
    }
}
