//! The run clock starts in the same H1 cut as `StartRun`, before any wait/send.
use super::{HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::{
    orchestration_commit::ValidatedRunUpdate,
    orchestration_records::{OrchestrationCommand, OrchestrationPayload, OrchestrationRecord},
};
use kinewright_project::orchestration_accounting::AccountingRecord;

pub(crate) fn unix_ms() -> Result<u64, HostError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|time| u64::try_from(time.as_millis()).ok())
        .ok_or_else(|| HostRefusal::new("clock_uncertain", "the run clock is unavailable").into())
}
impl HostCommandOwner {
    pub(super) fn seed_run_spending(
        &mut self,
        update: ValidatedRunUpdate,
    ) -> Result<ValidatedRunUpdate, HostError> {
        if !matches!(update.envelope().command, OrchestrationCommand::StartRun(_)) {
            return Ok(update);
        }
        let mut state = update.projection().clone();
        // The first native attempt can follow a host restart. Retain the
        // candidate's original merge base before any task actor exists.
        for task in &mut state.tasks {
            if let Some((_, guard)) = self
                .owner()?
                .durable_sessions()
                .into_iter()
                .find(|(_, guard)| guard.branch_id() == &task.brief.scope.base.branch)
            {
                let bytes = serde_json::to_vec(guard.base())?;
                let reference = kinewright_project::bundle_records::ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&bytes),
                    byte_length: bytes.len() as u64,
                    record_kind: kinewright_project::bundle_records::RecordKind::SpecialistBase,
                };
                self.owner()?.store()?.verify(&reference)?;
                task.progress.decisions.push(reference);
            }
        }
        let mut accounting = AccountingRecord::new(
            update.envelope().project.clone(),
            state.run.0.clone(),
            unix_ms()?,
        )
        .map_err(|reason| HostRefusal::new("accounting", &format!("{reason:?}")))?;
        for task in &state.tasks {
            accounting
                .add_task(task.id.0.clone(), None)
                .map_err(|reason| HostRefusal::new("accounting", &format!("{reason:?}")))?;
        }
        let record = self
            .owner_mut()?
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(accounting)),
            })?;
        state.usage.push(record.reference);
        let mut segment = update.segment().clone();
        segment
            .events
            .last_mut()
            .ok_or_else(|| HostRefusal::new("accounting", "missing start event"))?
            .projection = state.clone();
        ValidatedRunUpdate::new(
            &update.envelope().project,
            update.envelope().expected_generation,
            update.envelope(),
            state,
            segment,
            update.receipt().decision.clone(),
        )
        .map_err(|reason| HostRefusal::new("accounting", &format!("{reason:?}")).into())
    }
}

impl HostCommandOwner {
    pub(super) fn durable_run_accounting(
        &self,
        state: &crate::orchestration::orchestration_records::RunProjection,
    ) -> Result<Option<super::orchestration::DurableRunAccounting>, HostError> {
        for reference in state.usage.iter().rev() {
            let record = self
                .owner()?
                .store()?
                .read_record::<OrchestrationRecord>(reference)?;
            if let OrchestrationPayload::Accounting(accounting) = &record.payload {
                if accounting.project != self.owner()?.active_manual_project()?
                    || accounting.run != state.run.0
                {
                    return Err(
                        HostRefusal::new("accounting", "run accounting scope changed").into(),
                    );
                }
                let stop_reason = match &state.outcome {
                    crate::orchestration::orchestration_records::RunOutcome::Cancelled(_) => Some("Stopped".to_owned()),
                    crate::orchestration::orchestration_records::RunOutcome::Failed(reason) => Some(reason.code.clone()),
                    _ if accounting.clock_uncertain => Some("Clock could not be verified".to_owned()),
                    _ => match &state.wait {
                        crate::orchestration::orchestration_records::RunWait::Provider(wait) if wait.reason.record_kind == kinewright_project::bundle_records::RecordKind::OwnedEvidence => {
                            let bytes = self.owner()?.store()?.read_evidence(&wait.reason)?;
                            Some(super::bounded_text(&String::from_utf8_lossy(&bytes), 1024))
                        }
                        _ => accounting.tasks.iter().find_map(|task| task.pause.map(|reason| format!("{reason:?}"))),
                    },
                };
                return Ok(Some(super::orchestration::DurableRunAccounting {
                    stop_reason,
                    start_unix_ms: accounting.start_unix_ms,
                    deadline_unix_ms: accounting.deadline_unix_ms,
                    last_unix_ms: accounting.last_unix_ms,
                    elapsed_ms: accounting.elapsed_ms,
                    clock_uncertain: accounting.clock_uncertain,
                    tasks: accounting
                        .tasks
                        .iter()
                        .map(|task| super::orchestration::DurableTaskAccounting {
                            task: crate::orchestration::orchestration_records::TaskId(
                                task.task.clone(),
                            ),
                            sends: task.sends.len(),
                            provider: task.providers.last().cloned(),
                            totals: task.totals.clone(),
                            wait_reason: task.wait_reason.clone(),
                            pause: task.pause,
                        })
                        .collect(),
                }));
            }
        }
        Ok(None)
    }
}
