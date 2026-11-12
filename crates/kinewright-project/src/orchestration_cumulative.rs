//! Cumulative accounting replaces the current snapshot; semantic cuts have a flat index.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::ObjectRef,
    orchestration_accounting::AccountingRecord,
    orchestration_records::{
        OrchestrationPayload, OrchestrationRecord, RunId, RunOutcome, RunPhase, RunProjection,
    },
    store::SealedObject,
};
fn invalid() -> BundleError {
    BundleError::Identity("cumulative run state changed authority or retained history".into())
}
fn accounting(
    state: &RunProjection,
    load: &mut impl FnMut(&ObjectRef) -> Result<OrchestrationRecord, BundleError>,
) -> Result<(ObjectRef, AccountingRecord), BundleError> {
    let mut found = None;
    for reference in &state.usage {
        if let OrchestrationPayload::Accounting(record) = load(reference)?.payload {
            if found.is_some() || record.run != state.run.0 {
                return Err(invalid());
            }
            found = Some((reference.clone(), *record));
        }
    }
    found.ok_or_else(invalid)
}
fn cumulative_progress(
    before: &AccountingRecord,
    after: &AccountingRecord,
) -> Result<(), BundleError> {
    if after == before {
        return Ok(());
    }
    if after.revision <= before.revision {
        return Err(invalid());
    }
    // Validate a retained prefix against the latest replacement without retaining
    // clock-only predecessors. Revision counts all intermediate publications.
    let mut previous = before.clone();
    previous.revision = after.revision.checked_sub(1).ok_or_else(invalid)?;
    after.validate_successor(&previous).map_err(|_| invalid())
}
fn replace_accounting(
    mut state: RunProjection,
    previous: &ObjectRef,
    reference: &ObjectRef,
    record: &AccountingRecord,
) -> RunProjection {
    for old in &mut state.usage {
        if old == previous {
            old.clone_from(reference);
        }
    }
    if state.outcome == RunOutcome::Pending && record.check_clock().is_err() {
        state.phase = RunPhase::Paused;
    }
    state
}
pub(crate) fn validate_snapshot(
    before: &RunProjection,
    after: &RunProjection,
    mut load: impl FnMut(&ObjectRef) -> Result<OrchestrationRecord, BundleError>,
) -> Result<(), BundleError> {
    let (previous_ref, previous) = accounting(before, &mut load)?;
    let (current_ref, current) = accounting(after, &mut load)?;
    cumulative_progress(&previous, &current)?;
    if replace_accounting(before.clone(), &previous_ref, &current_ref, &current) != *after {
        return Err(invalid());
    }
    Ok(())
}
impl ProjectBundle {
    fn stage_orchestration_accounting(
        &mut self,
        run: &RunId,
        next: &AccountingRecord,
    ) -> Result<(Vec<SealedObject>, ObjectRef), BundleError> {
        let state = self.orchestration_projection(run)?.ok_or_else(invalid)?;
        let (previous_ref, previous) = accounting(&state, &mut |r| {
            Ok((*self.store.read_record::<OrchestrationRecord>(r)?).clone())
        })?;
        next.validate_successor(&previous).map_err(|_| invalid())?;
        crate::orchestration_accounting::encode(next, crate::run_admission::ACCOUNTING_BYTES)
            .map_err(|_| BundleError::RunLimit)?;
        cumulative_progress(&previous, next)?;
        // The caller retains its live state; the cut owns an immutable snapshot.
        let record = self.admit_owned_scheduler_record(OrchestrationRecord {
            retention: crate::orchestration_retention::Retention::Complete,
            version: 1,
            payload: OrchestrationPayload::Accounting(Box::new(next.clone())),
        })?;
        let projection = replace_accounting(state, &previous_ref, &record.reference, next);
        let base = self.semantic_snapshot(run)?;
        let snapshot_record = OrchestrationRecord::retained(
            OrchestrationPayload::Snapshot(Box::new(projection)),
            base,
            &self.store,
        )?;
        let snapshot = self.store.admit_record(&snapshot_record)?;
        let mut index = (*self.run_index()?).clone();
        index
            .cumulative
            .insert(run.clone(), snapshot.reference.clone());
        let index = self.store.admit_record(&index)?;
        let reference = index.reference.clone();
        Ok((vec![record, snapshot, index], reference))
    }
}

impl ProjectBundle {
    fn publish_run_accounting(
        &mut self,
        run: &RunId,
        next: &AccountingRecord,
    ) -> Result<(), BundleError> {
        if self.unsettled {
            return Err(BundleError::Unsettled);
        }
        let (state, current) = {
            let _reading = crate::run_admission::reading(&self.store);
            let state = self.orchestration_projection(run)?.ok_or_else(invalid)?;
            let (_, current) = accounting(&state, &mut |r| {
                Ok((*self.store.read_run_record(r)?).clone())
            })?;
            (state, current)
        };
        if current == *next {
            return Ok(());
        }
        let admission = self.accounting_recovery_admission(&state, &current, next)?;
        if admission
            .as_ref()
            .is_some_and(|token| !token.matches_accounting(run, next))
        {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        let terminal = admission.is_some();
        self.pending_terminal_publication.clone_from(&admission);
        let result = self.with_publication_admission(admission, |bundle| {
            if let Err(error) = bundle.collect_after_ack() {
                bundle
                    .store
                    .with_namespace(|namespace| namespace.verify())?;
                if matches!(
                    error,
                    BundleError::Unsettled
                        | BundleError::DurabilityUnknown
                        | BundleError::DurabilityUnknownAt(_)
                ) {
                    return Err(error);
                }
            }
            let before = bundle.root.clone();
            let result = (|| {
                let (_seals, reference) = bundle.stage_orchestration_accounting(run, next)?;
                if let Some(admission) = &mut bundle.pending_terminal_publication {
                    admission.accounting_root(reference.clone());
                }
                bundle.root.as_mut().ok_or_else(invalid)?.run_index_ref = Some(reference);
                crate::bundle_commit::BundleCommit::publish_maintenance(bundle).map(|_| ())
            })();
            if result.is_err() && !bundle.unsettled {
                bundle.root = before;
                if terminal && bundle.collect_after_ack().is_err() {
                    bundle.unsettled = true;
                    return Err(BundleError::Unsettled);
                }
            }
            result
        });
        if !self.unsettled {
            self.pending_terminal_publication = None;
        }
        result
    }
}
impl crate::editing_session::Ja1EditingSession {
    pub fn publish_run_accounting(
        &mut self,
        run: &RunId,
        next: &AccountingRecord,
    ) -> Result<(), BundleError> {
        self.active_bundle_mut()?.publish_run_accounting(run, next)
    }
}
