// Included in native_invocation: H1 reservations and recovery never run processes.
impl HostCommandOwner {
    pub(crate) fn spending_project(
        &self,
    ) -> Result<kinewright_project::bundle_records::ProjectId, HostError> {
        self.owner()?.project_id().map_err(Into::into)
    }

    pub(crate) fn reconcile_native_pending(
        &mut self,
        producer: &HostAuth,
    ) -> Result<(), HostError> {
        match self.hooks.pending.clone() {
            None => Ok(()),
            Some(super::orchestration::PendingPublication::Owner(original)) => self
                .publish_orchestration_transition(producer, &original, None)
                .map(|_| ()),
            Some(_) => Err(refused("original wire publication must reconcile first")),
        }
    }
    pub(crate) fn reserve_native_effect(
        &mut self,
        producer: &HostAuth,
        run: &crate::orchestration::orchestration_records::RunId,
    ) -> Result<Option<(AdmittedAttempt, EffectId, SendKind)>, HostError> {
        use crate::orchestration::orchestration_records::{
            AttemptReservation, DependencyReservation, TaskState,
        };
        let mut state = self.spending_scope(producer, run)?;
        self.archive_retired_attempts(&mut state)?;
        if state
            .attempts
            .iter()
            .filter(|a| !matches!(a.state, AttemptState::Retired(_)))
            .count()
            >= 3
        {
            return Ok(None);
        }
        let accounting = self.native_accounting(&state)?;
        let effect = state
            .effects
            .iter()
            .filter(|e| e.safety == EffectSafety::ProcessBound)
            .find(|e| {
                !accounting.as_ref().is_some_and(|a| {
                    a.tasks
                        .iter()
                        .any(|t| t.task == e.task.0 && t.wait_reason.is_some())
                }) && crate::orchestration::outbox::next_effect(&state, &e.lane)
                    .is_some_and(|next| next.id == e.id)
                    && !state.attempts.iter().any(|a| {
                        a.identity.task == e.task && !matches!(a.state, AttemptState::Retired(_))
                    })
            })
            .cloned();
        let Some(effect) = effect else {
            return Ok(None);
        };
        let task = state
            .tasks
            .iter()
            .find(|t| t.id == effect.task)
            .ok_or_else(|| refused("effect task missing"))?;
        if task.progress.expired
            || matches!(
                task.state,
                TaskState::Terminal(_)
                    | TaskState::Cancelled
                    | TaskState::Cancelling
                    | TaskState::CleanupBlocked(_)
            )
        {
            return Err(refused("effect task retired"));
        }
        let kind = if state.deliveries.iter().any(|d| d.wake == effect.id) {
            SendKind::AutomaticResultWake
        } else if effect.payload == task.brief.assignment
            && !state
                .effects
                .iter()
                .any(|e| e.task == task.id && e.order < effect.order)
        {
            if task.parent.is_some() {
                SendKind::SpecialistStart
            } else {
                SendKind::OpeningAssignment
            }
        } else {
            SendKind::QueuedSteering
        };
        let identity = ExecutionIdentity {
            version: 1,
            project: self.owner()?.project_id()?,
            run: run.clone(),
            task: effect.task.clone(),
            attempt: crate::orchestration::orchestration_records::AttemptId(
                kinewright_project::bundle_records::SessionId::fresh()?,
            ),
            nonce: kinewright_project::bundle_records::SessionId::fresh()?,
            owner_instance: kinewright_project::bundle_records::SessionId::fresh()?,
        };
        let recovery_ref = self
            .owner_mut()?
            .admit_host_grant(&serde_json::to_vec(&identity)?)?;
        state.attempts.push(AttemptReservation {
            identity: identity.clone(),
            owner_epoch: self.owner()?.store()?.owner_epoch(),
            state: AttemptState::Reserved,
            recovery_ref,
        });
        let task = state
            .tasks
            .iter_mut()
            .find(|t| t.id == identity.task)
            .ok_or_else(|| refused("effect task missing"))?;
        task.state = TaskState::Preparing;
        task.progress.capacity = DependencyReservation::Backed(identity.attempt.clone());
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let issued = self.issue_metadata(index, None)?;
        let (update, receipt) =
            self.publish_native_issued(producer, state, &identity, vec![], issued)?;
        let admitted =
            crate::orchestration::acknowledge_attempt_reservation(&update, &receipt, &identity)
                .map_err(refused)?;
        Ok(Some((admitted, effect.id, kind)))
    }
    /// Reconciliation never makes a process-bound outcome Pending again.
    pub(crate) fn reconcile_native_effect(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
    ) -> Result<(), HostError> {
        use crate::orchestration::orchestration_records::{
            AttemptOutcome, DependencyReservation, TaskState,
        };
        let mut state = self.spending_scope(producer, &identity.run)?;
        self.persist_runtime_task(&mut state, identity, true)?;
        let attempt = state
            .attempts
            .iter_mut()
            .find(|a| a.identity == *identity)
            .ok_or_else(|| refused("recovery attempt missing"))?;
        attempt.state = AttemptState::Retired(AttemptOutcome::Interrupted);
        let task = state
            .tasks
            .iter_mut()
            .find(|t| t.id == identity.task)
            .ok_or_else(|| refused("recovery task missing"))?;
        task.progress.capacity = DependencyReservation::Free;
        task.progress.turn = None;
        task.progress.turn_ended = !task.progress.expired;
        if task.state == TaskState::Cancelling {
            task.state = TaskState::Cancelled;
        } else if matches!(task.state, TaskState::Preparing | TaskState::Running(_)) {
            task.state = TaskState::Queued;
        }
        for effect in state.effects.iter_mut().filter(|e| {
            e.state == EffectState::Claimed(identity.attempt.clone())
                || (e.task == identity.task && e.state == EffectState::OutcomeUnknown)
        }) {
            effect.state = EffectState::OutcomeUnknown;
        }
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        self.publish_native(producer, state, identity, vec![])
    }
    pub(crate) fn restore_native_run(
        &mut self,
        run: &crate::orchestration::orchestration_records::RunId,
    ) -> Result<(HostAuth, RunProjection), HostError> {
        let state = self
            .owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| refused("missing recovery run"))?;
        let brief = state
            .tasks
            .iter()
            .find(|t| t.parent.is_none())
            .ok_or_else(|| refused("missing Director"))?
            .brief
            .clone();
        self.retain_run(run, &brief)?;
        Ok((self.run_producer(run)?, state))
    }
}
