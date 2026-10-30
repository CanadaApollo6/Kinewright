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
