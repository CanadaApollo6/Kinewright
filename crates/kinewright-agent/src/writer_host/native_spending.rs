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
    /// C5 fix 10: one refused product MCP call, by tool and fixed code.
    ToolRefusal {
        tool: &'static str,
        code: kinewright_project::orchestration_accounting::ToolRefusalCode,
    },
}

/// C5 fix 10: the classification of a counted provider failure, or `None`
/// when the event is not one. Only Codex 0.166's `turn.failed`
/// (`{"error":{"message"}}`) and top-level `error` (`{"message"}`) events
/// count. A failed tool-call item is the product refusing one call, which
/// the model may answer by trying something else; it is not a provider
/// failure. The classification is the event kind plus the first non-empty
/// code or message; null, absent and empty values are never hashed, and a
/// failure without either gets the fixed unclassified token.
///
/// A top-level `error` that is exactly one of Codex 0.166's retry notices
/// ([`codex_retry_notice`]) is Codex retrying its own transport while the
/// turn goes on, not a failure (lead ruling on C5 fix 10, narrowed in fix
/// 11). The two notices are bounded differently:
/// - `Reconnecting... N/M` is capped by Codex: M is at most 100
///   (`MAX_STREAM_MAX_RETRIES`), and when the retries run out Codex sends
///   `turn.failed`, which counts.
/// - `Reconnecting... waiting for network` has no count cap upstream: Codex's
///   `unbounded_connection_retries` feature, on by default, retries a failed
///   connection with a delay of at most 60 s and never reaches `turn.failed`
///   on its own. Its bound is the product's run clock: the accounting run
///   deadline and elapsed budget the effect pump enforces, and the session
///   wall time.
///
/// The first-progress deadline bounds only the silence before the first
/// event, so it does not bound either kind of notice. Every other top-level
/// `error` counts, network text or not.
pub(crate) fn provider_failure(kind: &str, raw: &serde_json::Value) -> Option<String> {
    let detail = match kind {
        "turn.failed" => raw.get("error"),
        "error"
            if raw
                .get("message")
                .and_then(serde_json::Value::as_str)
                .is_some_and(codex_retry_notice) =>
        {
            return None;
        }
        "error" => Some(raw),
        _ => return None,
    };
    let field = |name: &str| {
        detail
            .and_then(|detail| detail.get(name))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| (name.to_owned(), value))
    };
    Some(field("code").or_else(|| field("message")).map_or_else(
        || kinewright_project::orchestration_accounting::UNCLASSIFIED_FAILURE.into(),
        |(name, value)| {
            kinewright_media::sha256_bytes(format!("{kind}\0{name}\0{value}").as_bytes())
        },
    ))
}
/// S4b C6: a Claude `result` that is not a clean success. The classifier is
/// the fixed `subtype` (`error_during_execution`, `error_max_turns`, or
/// `success` with `is_error` for an API failure); no provider text is kept.
pub(crate) fn claude_provider_failure(kind: &str, raw: &serde_json::Value) -> Option<String> {
    if kind != "result" {
        return None;
    }
    let subtype = raw
        .get("subtype")
        .and_then(serde_json::Value::as_str)
        .filter(|subtype| {
            !subtype.is_empty()
                && subtype.len() <= 64
                && subtype.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        });
    let is_error = raw.get("is_error").and_then(serde_json::Value::as_bool) == Some(true);
    if subtype == Some("success") && !is_error {
        return None;
    }
    Some(subtype.map_or_else(
        || kinewright_project::orchestration_accounting::UNCLASSIFIED_FAILURE.into(),
        |subtype| {
            kinewright_media::sha256_bytes(
                format!("result\0subtype\0{subtype}\0{is_error}").as_bytes(),
            )
        },
    ))
}
/// C5 fix 11: exactly Codex 0.166's retry notices, as its JSONL stream
/// writes them. `core/src/responses_retry.rs` sends `Reconnecting...
/// {retry_count}/{max_retries}` and `Reconnecting... waiting for network`
/// through `notify_stream_error`, which always sets the underlying error as
/// `additional_details`; `exec/src/event_processor_with_jsonl_output.rs`
/// then writes `{message} ({details})`. The counts are Codex's: 1 <= N <= M
/// <= 100 (its hard retry cap), in decimal with no leading zero. Nothing
/// else matches: the broad `transport_error` classifier stays a display
/// sanitizer only.
pub(crate) fn codex_retry_notice(message: &str) -> bool {
    let Some(rest) = message.strip_prefix("Reconnecting... ") else {
        return false;
    };
    let tail = if let Some(tail) = rest.strip_prefix("waiting for network") {
        tail
    } else {
        let count = |text: &str| -> Option<(u64, usize)> {
            let digits = text.bytes().take_while(u8::is_ascii_digit).count();
            if !(1..=3).contains(&digits) || text.starts_with('0') {
                return None;
            }
            Some((text[..digits].parse().ok()?, digits))
        };
        let Some((retry, used)) = count(rest) else {
            return false;
        };
        let Some(rest) = rest[used..].strip_prefix('/') else {
            return false;
        };
        let Some((limit, used)) = count(rest) else {
            return false;
        };
        if retry > limit || limit > 100 {
            return false;
        }
        &rest[used..]
    };
    tail.is_empty()
        || tail
            .strip_prefix(" (")
            .and_then(|details| details.strip_suffix(')'))
            .is_some_and(|details| !details.is_empty())
}
impl HostCommandOwner {
    pub(super) fn native_usage_event(
        kind: &str,
        raw: &serde_json::Value,
        ordinal: u64,
    ) -> Result<String, HostError> {
        Ok(format!(
            "{}:{}",
            kind.chars().take(64).collect::<String>(),
            raw.get(if kind == "result" { "uuid" } else { "id" })
                .map_or_else(
                    || Ok(format!("{ordinal:016x}")),
                    |id| serde_json::to_vec(id).map(|bytes| kinewright_media::sha256_bytes(&bytes))
                )?
        ))
    }

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
            let record = self.owner()?.store()?.read_run_record(previous)?;
            if !matches!(record.payload, OrchestrationPayload::Accounting(_)) {
                keep.push(previous.clone());
            }
        }
        keep.push(reference);
        state.usage = keep;
        Ok(())
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
        let mut next = previous.successor().map_err(refused)?;
        let clock_result =
            next.observe_clock(clock.unix_ms, clock.monotonic_delta_ms, clock.trusted);
        if !next.tasks.iter().any(|t| t.task == identity.task.0) {
            next.add_task(identity.task.0.clone(), None)
                .map_err(refused)?;
        }
        let deadline_unix_ms = next.deadline_unix_ms;
        let task = next.task_mut(&identity.task.0).map_err(refused)?;
        if let Some(observed) = self
            .hooks
            .runs
            .get(&identity.run)
            .and_then(|run| run.measurements.get(&identity.task))
        {
            let mut observed = match observed.lock() {
                Ok(observed) => observed.clone(),
                Err(error) => {
                    let mut observed = error.into_inner().clone();
                    observed.incomplete = true;
                    observed
                }
            };
            if let Some(saved) = &task.measurements {
                if !observed.results.starts_with(&saved.results)
                    || observed.result_count < saved.result_count
                    || observed.result_bytes < saved.result_bytes
                {
                    let count = observed.result_count.max(saved.result_count);
                    let bytes = observed.result_bytes.max(saved.result_bytes);
                    observed = saved.clone();
                    observed.result_count = count;
                    observed.result_bytes = bytes;
                    observed.incomplete = true;
                }
                observed.incomplete |= saved.incomplete;
                observed.sends.clone_from(&saved.sends);
            }
            task.measurements = Some(observed);
        }
        let observes_usage = matches!(&change, SpendingChange::Usage(_));
        let observes_projection =
            matches!(&change, SpendingChange::Usage(sample) if sample.snapshot.is_some());
        let measurements_changed = task.measurements
            != previous
                .tasks
                .iter()
                .find(|t| t.task == identity.task.0)
                .and_then(|t| t.measurements.clone());
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
            SpendingChange::ToolRefusal { tool, code } => task.refuse_tool(tool, code),
        };
        if (observes_projection || measurements_changed)
            && (outcome.is_ok() || outcome == Err(AccountingRefusal::Replan))
        {
            Self::finish_accounting_observation(&previous, &mut next)?;
        }
        let validated_size = (observes_usage || measurements_changed)
            .then(|| next.validate_successor_size(&previous));
        if outcome == Err(AccountingRefusal::Capacity)
            || matches!(validated_size, Some(Err(AccountingRefusal::Capacity)))
            || matches!(validated_size, Some(Ok(size)) if size > kinewright_project::run_admission::ACCOUNTING_BYTES - 4096)
        {
            return self.refuse_measurement_capacity(producer, identity, &previous, clock);
        }
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
        if let Some(result) = validated_size {
            result.map_err(refused)?;
        } else {
            next.validate_successor(&previous).map_err(refused)?;
        }
        let revision = next.revision;
        next.revision = previous.revision;
        if next == previous {
            return clock_result.and(outcome).map_err(refused);
        }
        next.revision = revision;
        self.publish_accounting(producer, identity, &next)?;
        clock_result.and(outcome).map_err(refused)
    }
    fn refuse_measurement_capacity(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
        previous: &AccountingRecord,
        clock: ClockObservation,
    ) -> Result<(), HostError> {
        let mut bounded = previous.successor().map_err(refused)?;
        let _clock = bounded.observe_clock(clock.unix_ms, clock.monotonic_delta_ms, clock.trusted);
        if !bounded.tasks.iter().any(|t| t.task == identity.task.0) {
            bounded
                .add_task(identity.task.0.clone(), None)
                .map_err(refused)?;
        }
        let task = bounded.task_mut(&identity.task.0).map_err(refused)?;
        let measurements = task.measurements.get_or_insert_with(Default::default);
        if let Some(observed) = self
            .hooks
            .runs
            .get(&identity.run)
            .and_then(|r| r.measurements.get(&identity.task))
        {
            let observed = observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            measurements.result_count = measurements.result_count.max(observed.result_count);
            measurements.result_bytes = measurements.result_bytes.max(observed.result_bytes);
        }
        measurements.incomplete = true;
        task.pause.get_or_insert(AccountingRefusal::Capacity);
        bounded.validate_successor(previous).map_err(refused)?;
        self.publish_accounting(producer, identity, &bounded)?;
        Err(refused(AccountingRefusal::Capacity))
    }

    pub(super) fn publish_accounting(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
        next: &AccountingRecord,
    ) -> Result<(), HostError> {
        use super::orchestration::PendingPublication;
        if let Some(pending) = &self.hooks.pending {
            if !matches!(pending, PendingPublication::Cumulative(original, record) if **original == *identity && **record == *next)
            {
                return Err(refused("another publication must reconcile first"));
            }
            self.owner_mut()?.reconcile_metadata()?;
            self.hooks.pending = None;
        }
        let state = self.spending_scope(producer, &identity.run)?;
        if identity.project != self.owner()?.project_id()?
            || !state.tasks.iter().any(|t| t.id == identity.task)
        {
            return Err(refused("accounting authority changed"));
        }
        self.hooks.pending = Some(PendingPublication::Cumulative(
            Box::new(identity.clone()),
            Box::new(next.clone()),
        ));
        let result = self
            .owner_mut()?
            .publish_run_accounting(&identity.run, next);
        if !matches!(
            result,
            Err(kinewright_project::bundle::BundleError::DurabilityUnknown
                | kinewright_project::bundle::BundleError::DurabilityUnknownAt(_)
                | kinewright_project::bundle::BundleError::Unsettled)
        ) {
            self.hooks.pending = None;
        }
        result.map_err(Into::into)
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
    /// C5 fix 10: count one refused product MCP call, by the product's tool
    /// name and fixed code, in the calling task's durable accounting.
    pub(crate) fn record_tool_refusal(
        &mut self,
        authority: &crate::orchestration::tasks::RuntimeTaskAuthority,
        tool: &'static str,
        code: kinewright_project::orchestration_accounting::ToolRefusalCode,
    ) -> Result<(), HostError> {
        let producer = self.run_producer(&authority.run)?;
        let state = self.spending_scope(&producer, &authority.run)?;
        // C5 fix 11: only an attempt the projection admitted is counted; a
        // refusal from an endpoint whose attempt or task is gone is not.
        let Some(identity) = state
            .attempts
            .iter()
            .find(|a| a.identity.attempt == authority.attempt && a.identity.task == authority.task)
            .map(|a| a.identity.clone())
        else {
            return Ok(());
        };
        if !state.tasks.iter().any(|t| t.id == authority.task) {
            return Ok(());
        }
        self.change_spending(
            &producer,
            &identity,
            SpendingChange::ToolRefusal { tool, code },
            ClockObservation {
                unix_ms: super::spending::unix_ms()?,
                monotonic_delta_ms: 0,
                trusted: true,
            },
        )
    }
    pub(crate) fn ingest_native_raw(
        &mut self,
        producer: &HostAuth,
        identity: &ExecutionIdentity,
        raw: &serde_json::Value,
        ordinal: u64,
        eof: bool,
    ) -> Result<(), HostError> {
        self.ingest_native_raw_as(
            native_launcher::StreamDialect::Codex,
            producer,
            identity,
            raw,
            ordinal,
            eof,
        )
    }
    /// Both dialects use TE1.1's authoritative, stream-bound usage adapter.
    pub(crate) fn ingest_native_raw_as(
        &mut self,
        dialect: native_launcher::StreamDialect,
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
        let claude = dialect == native_launcher::StreamDialect::Claude;
        if !claude
            && (kind == "rate_limit"
                || raw
                    .pointer("/error/code")
                    .and_then(serde_json::Value::as_str)
                    == Some("rate_limit_exceeded"))
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
        let failure = if claude {
            claude_provider_failure(kind, raw)
        } else {
            provider_failure(kind, raw)
        };
        if let Some(failure) = failure {
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
            return self.change_spending(
                producer,
                identity,
                SpendingChange::Repair { delivery, failure },
                ClockObservation {
                    unix_ms: super::spending::unix_ms()?,
                    monotonic_delta_ms: 0,
                    trusted: true,
                },
            );
        }
        let terminal = if claude { "result" } else { "turn.completed" };
        if kind == "turn.started"
            && !eof
            && raw.get("usage").is_none()
            && raw.get("total_cost_usd").is_none()
            && (task.usage.is_empty()
                || task
                    .usage
                    .iter()
                    .rev()
                    .find(|sample| sample.attempt == identity.attempt.0 && sample.turn == 1)
                    .is_some_and(|sample| !sample.complete))
        {
            // Coverage is already incomplete in the durable record.
            return Ok(());
        }
        if !eof
            && if claude {
                kind != terminal
            } else {
                raw.get("usage").is_none() && !matches!(kind, "turn.completed" | "turn.started")
            }
        {
            return Ok(());
        }
        if eof
            && task
                .usage
                .iter()
                .rev()
                .find(|s| s.attempt == identity.attempt.0 && s.turn == 1)
                .is_some_and(|s| s.complete)
        {
            return Ok(());
        }
        let event = Self::native_usage_event(kind, raw, ordinal)?;
        let old = task
            .usage
            .iter()
            .find(|s| s.attempt == identity.attempt.0 && s.turn == 1 && s.event == event);
        let sample = decode_usage(
            if claude {
                UsageAdapter::Claude
            } else {
                UsageAdapter::Codex
            },
            raw,
            UsageSample {
                snapshot: task
                    .streams
                    .iter()
                    .find(|b| b.attempt == identity.attempt.0)
                    .map(|b| kinewright_project::usage_stream::Snapshot {
                        payload_sha256: "0".repeat(64),
                        stream: b.stream.clone(),
                        adapter: b.adapter,
                        models: vec![],
                    }),
                attempt: identity.attempt.0.clone(),
                turn: 1,
                event,
                ordinal: old.map_or(ordinal, |s| s.ordinal),
                mode: UsageMode::Cumulative,
                values: UsageValues::default(),
                complete: eof || kind == terminal,
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
        let wake = state
            .effects
            .iter()
            .find(|effect| {
                effect.state
                    == crate::orchestration::orchestration_records::EffectState::Claimed(
                        identity.attempt.clone(),
                    )
            })
            .map(|effect| effect.id.clone());
        if let Some(wake) = wake {
            crate::orchestration::mailbox::offer_attempt_deliveries(
                &mut state,
                &identity.task,
                &identity.attempt,
                &wake,
            )
            .map_err(refused)?;
        }
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
        if state.outcome == crate::orchestration::orchestration_records::RunOutcome::Pending
            && state.cleanup == crate::orchestration::orchestration_records::RunCleanup::Settled
            && let Some(reference) = state
                .tasks
                .iter()
                .find(|task| {
                    task.brief.role == crate::orchestration::orchestration_records::Role::Director
                })
                .and_then(|task| {
                    if let TaskState::Terminal(reference) = &task.state {
                        Some(reference.clone())
                    } else {
                        None
                    }
                })
        {
            let record = self.owner()?.store()?.read_run_record(&reference)?;
            if let OrchestrationPayload::Result(result) = &record.payload {
                use crate::orchestration::orchestration_records::{
                    OutcomeDetail, RunOutcome, RunPhase, RunWait,
                };
                state.outcome = match result.outcome {
                    AttemptOutcome::Completed => RunOutcome::Ready,
                    AttemptOutcome::Failed => RunOutcome::Failed(OutcomeDetail {
                        code: "director_failed".into(),
                        detail_ref: Some(result.explanation.clone()),
                    }),
                    AttemptOutcome::Cancelled | AttemptOutcome::Interrupted => {
                        RunOutcome::Cancelled(OutcomeDetail {
                            code: "director_cancelled".into(),
                            detail_ref: Some(result.explanation.clone()),
                        })
                    }
                };
                state.phase = RunPhase::Finished;
                state.wait = RunWait::None;
            }
        }
        self.publish_native(producer, state, identity, vec![])
    }
}
