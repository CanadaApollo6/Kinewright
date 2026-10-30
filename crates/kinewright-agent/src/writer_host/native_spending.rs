// Included in the native owner module so every spending publication uses H1.
use crate::protocol::limits::usage::{ClockObservation, UsageAdapter, decode_usage, ingest_usage};
use kinewright_project::orchestration_accounting::{
    AccountingRefusal, UsageMode, UsageSample, UsageValues,
};

pub(crate) const RATE_LIMIT_BACKOFF_MS: u64 = 5_000;

pub(crate) enum SpendingChange {
    Clock,
    Ceiling(u64),
    Wait(Option<String>),
    Repair {
        delivery: kinewright_project::bundle_records::SessionId,
        failure: String,
    },
    Usage(UsageSample),
}
impl HostCommandOwner {
    pub(crate) fn spending_scope(
        &mut self,
        producer: &HostAuth,
        run: &crate::orchestration::orchestration_records::RunId,
    ) -> Result<RunProjection, HostError> {
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("missing run Producer"))?;
        if self.attachments[index].role != super::AttachmentRole::Producer
            || scope.run != *run
            || scope.task.is_some()
            || scope.expired()
            || scope.owner_epoch != self.owner()?.store()?.owner_epoch()
            || self.hooks.pending.is_some()
        {
            return Err(refused("spending authority changed"));
        }
        self.owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| refused("spending run missing"))
    }
    pub(crate) fn replace_accounting_reference(
        &self,
        state: &mut RunProjection,
        reference: ObjectRef,
    ) -> Result<(), HostError> {
        let mut keep = Vec::new();
        for previous in &state.usage {
            let record = self
                .owner()?
                .store()?
                .read_record::<OrchestrationRecord>(previous)?;
            if !matches!(record.payload, OrchestrationPayload::Accounting(_)) {
                keep.push(previous.clone());
            }
        }
        keep.push(reference);
        state.usage = keep;
        Ok(())
    }
    fn accounting_successor(
        &mut self,
        current: &AccountingRecord,
    ) -> Result<AccountingRecord, HostError> {
        self.owner_mut()?
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::AccountingArchive(Box::new(
                    current.archive().map_err(refused)?,
                )),
            })?;
        current.successor().map_err(refused)
    }
    pub(crate) fn change_spending(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
        change: SpendingChange,
        clock: ClockObservation,
    ) -> Result<(), HostError> {
        let mut state = self.spending_scope(producer, &identity.run)?;
        if identity.project != self.owner()?.project_id()?
            || !state.tasks.iter().any(|t| t.id == identity.task)
        {
            return Err(refused("spending identity changed"));
        }
        let previous = self
            .native_accounting(&state)?
            .ok_or_else(|| refused("run has no durable clock"))?;
        let mut next = self.accounting_successor(&previous)?;
        let clock_result =
            next.observe_clock(clock.unix_ms, clock.monotonic_delta_ms, clock.trusted);
        if !next.tasks.iter().any(|t| t.task == identity.task.0) {
            next.add_task(identity.task.0.clone(), None)
                .map_err(refused)?;
        }
        let deadline_unix_ms = next.deadline_unix_ms;
        let task = next.task_mut(&identity.task.0).map_err(refused)?;
        let outcome = match change {
            SpendingChange::Clock => Ok(()),
            SpendingChange::Ceiling(limit) => task.configure_token_limit(limit),
            SpendingChange::Wait(reason) => {
                if reason.as_ref().is_some_and(|s| s.len() > 1024) {
                    return Err(refused("wait capacity"));
                }
                task.wait_until_unix_ms = reason.as_ref().map(|_| {
                    clock
                        .unix_ms
                        .saturating_add(RATE_LIMIT_BACKOFF_MS)
                        .min(deadline_unix_ms)
                });
                task.wait_reason = reason;
                Ok(())
            }
            SpendingChange::Repair { delivery, failure } => {
                task.repair(&delivery, &identity.attempt.0, failure)
            }
            SpendingChange::Usage(sample) => ingest_usage(task, sample).map(|_| ()),
        };
        if let Err(reason) = outcome
            && reason != AccountingRefusal::Replan
        {
            return Err(refused(reason));
        }
        if clock_result.is_err() {
            if state.outcome == RunOutcome::Pending {
                state.phase = RunPhase::Paused;
            }
            for attempt in self
                .hooks
                .native
                .values()
                .filter(|a| a.identity.run == identity.run)
            {
                attempt.control.close();
            }
        }
        next.validate_successor(&previous).map_err(refused)?;
        let sealed = self
            .owner_mut()?
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(next)),
            })?;
        self.replace_accounting_reference(&mut state, sealed.reference)?;
        self.publish_native(producer, state, identity, vec![])?;
        clock_result.and(outcome).map_err(refused)
    }
    pub(crate) fn clear_due_native_waits(
        &mut self,
        producer: &HostAuth,
        run: &crate::orchestration::orchestration_records::RunId,
        now: u64,
    ) -> Result<(), HostError> {
        let state = self.spending_scope(producer, run)?;
        let accounting = self
            .native_accounting(&state)?
            .ok_or_else(|| refused("run has no durable clock"))?;
        for task in accounting.tasks.iter().filter(|t| {
            t.wait_reason.is_some() && t.wait_until_unix_ms.is_none_or(|until| now >= until)
        }) {
            // Clock/wait context survives archival; it grants no native authority.
            let identity = state
                .attempts
                .iter()
                .rev()
                .find(|a| a.identity.task.0 == task.task)
                .map_or_else(
                    || ExecutionIdentity {
                        version: 1,
                        project: accounting.project.clone(),
                        run: run.clone(),
                        task: crate::orchestration::orchestration_records::TaskId(
                            task.task.clone(),
                        ),
                        attempt: crate::orchestration::orchestration_records::AttemptId(
                            run.0.clone(),
                        ),
                        nonce: run.0.clone(),
                        owner_instance: run.0.clone(),
                    },
                    |a| a.identity.clone(),
                );
            // Legacy waits receive the same bounded backoff before their first clear.
            let change = if task.wait_until_unix_ms.is_some() {
                SpendingChange::Wait(None)
            } else {
                SpendingChange::Wait(task.wait_reason.clone())
            };
            self.change_spending(
                producer,
                &identity,
                change,
                ClockObservation {
                    unix_ms: now,
                    monotonic_delta_ms: 0,
                    trusted: true,
                },
            )?;
        }
        Ok(())
    }
    fn check_native_commit_budget(
        &mut self,
        producer: &HostAuth,
        invocation: &AuthorizedInvocation,
    ) -> Result<(), HostError> {
        let identity = invocation.invocation().binding().identity();
        let lifetime = invocation.lifetime();
        let state = self.spending_scope(producer, &identity.run)?;
        let current = self
            .native_accounting(&state)?
            .ok_or_else(|| refused("native clock missing"))?;
        self.change_spending(
            producer,
            identity,
            SpendingChange::Clock,
            ClockObservation {
                unix_ms: super::spending::unix_ms()?,
                monotonic_delta_ms: lifetime.elapsed_floor().saturating_sub(current.elapsed_ms),
                trusted: true,
            },
        )?;
        let state = self.spending_scope(producer, &identity.run)?;
        let mut current = self
            .native_accounting(&state)?
            .ok_or_else(|| refused("native clock missing"))?;
        current
            .tasks
            .iter()
            .find(|t| t.task == identity.task.0)
            .ok_or_else(|| refused("native accounting task missing"))?
            .check_release(invocation.acknowledgment().consumption())
            .map_err(refused)?;
        let now = super::spending::unix_ms()?;
        if let Err(reason) = current.observe_clock(now, 0, true) {
            self.change_spending(
                producer,
                identity,
                SpendingChange::Clock,
                ClockObservation {
                    unix_ms: now,
                    monotonic_delta_ms: 0,
                    trusted: true,
                },
            )?;
            return Err(refused(reason));
        }
        if std::time::Instant::now() >= lifetime.expires {
            return Err(refused(AccountingRefusal::Time));
        }
        Ok(())
    }
    pub(crate) fn ingest_native_raw(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
        raw: &serde_json::Value,
        ordinal: u64,
        eof: bool,
    ) -> Result<(), HostError> {
        let state = self.spending_scope(producer, &identity.run)?;
        if !state.attempts.iter().any(|a| a.identity == *identity) {
            return Err(refused("usage attempt missing"));
        }
        let current = self
            .native_accounting(&state)?
            .ok_or_else(|| refused("usage has no charge"))?;
        let task = current
            .tasks
            .iter()
            .find(|t| t.task == identity.task.0)
            .ok_or_else(|| refused("usage task missing"))?;
        let kind = raw
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("native.eof");
        if kind == "rate_limit"
            || raw
                .pointer("/error/code")
                .and_then(serde_json::Value::as_str)
                == Some("rate_limit_exceeded")
        {
            return self.change_spending(
                producer,
                identity,
                SpendingChange::Wait(Some("provider rate limit".into())),
                ClockObservation {
                    unix_ms: super::spending::unix_ms()?,
                    monotonic_delta_ms: 0,
                    trusted: true,
                },
            );
        }
        if kind == "turn.failed"
            || raw
                .pointer("/item/status")
                .and_then(serde_json::Value::as_str)
                == Some("failed")
        {
            let effect = state
                .effects
                .iter()
                .find(|effect| effect.state == EffectState::Claimed(identity.attempt.clone()))
                .ok_or_else(|| refused("repair has no charged delivery"))?;
            let delivery = state
                .deliveries
                .iter()
                .find(|delivery| delivery.wake == effect.id)
                .map_or_else(|| effect.id.0.clone(), |delivery| delivery.id.0.clone());
            let failure = raw
                .pointer("/error/message")
                .or_else(|| raw.pointer("/item/error/message"))
                .or_else(|| raw.pointer("/item/error"))
                .or_else(|| raw.get("error"))
                .ok_or_else(|| refused("unclassified provider failure"))?;
            return self.change_spending(
                producer,
                identity,
                SpendingChange::Repair {
                    delivery,
                    failure: kinewright_media::sha256_bytes(&serde_json::to_vec(failure)?),
                },
                ClockObservation {
                    unix_ms: super::spending::unix_ms()?,
                    monotonic_delta_ms: 0,
                    trusted: true,
                },
            );
        }
        if !eof && raw.get("usage").is_none() && kind != "turn.completed" {
            return Ok(());
        }
        if eof
            && task
                .usage
                .iter()
                .any(|s| s.attempt == identity.attempt.0 && s.turn == 1 && s.complete)
        {
            return Ok(());
        }
        let event = format!(
            "{}:{}",
            kind.chars().take(64).collect::<String>(),
            kinewright_media::sha256_bytes(&serde_json::to_vec(raw.get("id").unwrap_or(raw))?)
        );
        let old = task
            .usage
            .iter()
            .find(|s| s.attempt == identity.attempt.0 && s.turn == 1 && s.event == event);
        let sample = decode_usage(
            UsageAdapter::Codex,
            raw,
            UsageSample {
                attempt: identity.attempt.0.clone(),
                turn: 1,
                event,
                ordinal: old.map_or(ordinal, |s| s.ordinal),
                mode: UsageMode::Cumulative,
                values: UsageValues::default(),
                complete: eof || kind == "turn.completed",
                input_includes_cache: true,
                output_includes_reasoning: true,
            },
        )
        .map_err(refused)?;
        if let Some(old) = old {
            return if *old == sample {
                Ok(())
            } else {
                Err(refused("usage event changed"))
            };
        }
        self.change_spending(
            producer,
            identity,
            SpendingChange::Usage(sample),
            ClockObservation {
                unix_ms: current.last_unix_ms,
                monotonic_delta_ms: 0,
                trusted: true,
            },
        )
    }
    pub(crate) fn native_started(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
    ) -> Result<(), HostError> {
        use crate::orchestration::orchestration_records::{TaskState, TaskTurn, TurnPurpose};
        let mut state = self.native_projection(producer, identity)?;
        let attempt = state
            .attempts
            .iter_mut()
            .find(|a| a.identity == *identity)
            .ok_or_else(|| refused("started attempt missing"))?;
        if attempt.state != AttemptState::Contained {
            return Err(refused("started attempt already consumed"));
        }
        attempt.state = AttemptState::Executing;
        let task = state
            .tasks
            .iter_mut()
            .find(|t| t.id == identity.task)
            .ok_or_else(|| refused("started task missing"))?;
        task.state = TaskState::Running(identity.attempt.clone());
        task.progress.turn_ended = false;
        task.progress.turn = Some(TaskTurn {
            task: identity.task.clone(),
            attempt: identity.attempt.clone(),
            session: identity.nonce.clone(),
            ordinal: 1,
            owner_epoch: self.owner()?.store()?.owner_epoch(),
            expected_head: task.brief.scope.base.head.clone(),
            purpose: TurnPurpose::Work,
        });
        self.publish_native(producer, state, identity, vec![])
    }
    pub(crate) fn native_completed(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
        completed: bool,
        proof: &crate::guardian::CleanupProof,
    ) -> Result<(), HostError> {
        use crate::orchestration::orchestration_records::{
            AttemptOutcome, DependencyReservation, TaskState,
        };
        let native = proof.identity();
        if native.project != identity.project.as_str()
            || native.run != identity.run.0.as_str()
            || native.task != identity.task.0.as_str()
            || native.attempt != identity.attempt.0.as_str()
            || native.nonce != identity.nonce.as_str()
            || native.owner_instance != identity.owner_instance.as_str()
        {
            return Err(refused("cleanup proof scope changed"));
        }
        let mut state = self.spending_scope(producer, &identity.run)?;
        if self
            .recovery_attempt(&state, native)?
            .is_some_and(|a| matches!(a.state, AttemptState::Retired(_)))
        {
            return Ok(());
        }

        self.persist_runtime_task(&mut state, identity, true)?;
        let attempt = state
            .attempts
            .iter_mut()
            .find(|a| a.identity == *identity)
            .ok_or_else(|| refused("completion attempt missing"))?;
        if matches!(attempt.state, AttemptState::Retired(_)) {
            return Ok(());
        }
        attempt.state = AttemptState::Retired(if completed {
            AttemptOutcome::Completed
        } else {
            AttemptOutcome::Interrupted
        });
        let task = state
            .tasks
            .iter_mut()
            .find(|t| t.id == identity.task)
            .ok_or_else(|| refused("completion task missing"))?;
        task.progress.capacity = DependencyReservation::Free;
        task.progress.turn_ended = !task.progress.expired;
        if task.progress.expired {
            task.progress.turn = None;
        }
        if task.state == TaskState::Cancelling {
            task.state = TaskState::Cancelled;
        } else if matches!(task.state, TaskState::Preparing | TaskState::Running(_)) {
            task.state = TaskState::Queued;
        }
        for effect in state
            .effects
            .iter_mut()
            .filter(|e| e.state == EffectState::Claimed(identity.attempt.clone()))
        {
            effect.state = if completed {
                EffectState::Settled
            } else {
                EffectState::OutcomeUnknown
            };
        }
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        self.publish_native(producer, state, identity, vec![])
    }
}
