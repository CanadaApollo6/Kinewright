//! Identified provider usage and acknowledgment-gated sends for the H1 owner.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {ProviderEventIngestor,EventSink}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright retains missing categories and requires an exact committed cut.
#![allow(dead_code, reason = "H1 owner integration is pending")]
#![allow(clippy::missing_errors_doc)]

use crate::orchestration::{
    orchestration_commit::{CommandDecision, OrchestrationReceipt, ValidatedRunUpdate},
    orchestration_records::OrchestrationEnvelope,
};
use kinewright_project::{
    bundle_records::{ObjectRef, SessionId},
    orchestration_accounting::{
        AccountingRecord, AccountingRefusal, RUN_MILLIS, SendConsumption, TaskAccounting,
        USAGE_EVENTS, UsageSample, UsageValues, encode, normalize,
    },
};

#[derive(Debug, Clone, Copy)]
pub enum UsageAdapter {
    Claude,
    Codex,
    Muse,
}

/// Read raw usage before the legacy display Cost event fills unavailable values.
/// The owner supplies certified mode and event identity, never inferred quotas.
pub fn decode_usage(
    adapter: UsageAdapter,
    raw: &serde_json::Value,
    mut identity: UsageSample,
) -> Result<UsageSample, AccountingRefusal> {
    encode(raw, super::FRAME_BYTES)?;
    let usage = raw.get("usage").unwrap_or(raw);
    let token = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| usage.get(*key).and_then(serde_json::Value::as_u64))
    };
    identity.values = match adapter {
        UsageAdapter::Claude => UsageValues {
            input: token(&["input_tokens", "inputTokens"]),
            output: token(&["output_tokens", "outputTokens"]),
            cache_read: token(&["cache_read_input_tokens", "cacheReadInputTokens"]),
            cache_write: token(&["cache_creation_input_tokens", "cacheCreationInputTokens"]),
            reasoning: token(&["reasoning_output_tokens", "reasoningOutputTokens"]),
            cost_micros: None,
        },
        UsageAdapter::Codex => UsageValues {
            input: token(&["input_tokens", "inputTokens"]),
            output: token(&["output_tokens", "outputTokens"]),
            cache_read: token(&["cached_input_tokens", "cachedInputTokens"]).or_else(|| {
                usage
                    .pointer("/input_tokens_details/cached_tokens")
                    .and_then(serde_json::Value::as_u64)
            }),
            cache_write: token(&["cache_write_input_tokens", "cacheWriteInputTokens"]),
            reasoning: token(&["reasoning_output_tokens", "reasoningOutputTokens"]).or_else(|| {
                usage
                    .pointer("/output_tokens_details/reasoning_tokens")
                    .and_then(serde_json::Value::as_u64)
            }),
            cost_micros: None,
        },
        UsageAdapter::Muse => UsageValues {
            input: raw
                .get("promptTokens")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| token(&["inputTokens"])),
            output: token(&["outputTokens"]),
            cache_read: token(&["cacheReadTokens", "cachedTokens"]),
            cache_write: token(&["cacheWriteTokens"]),
            reasoning: token(&["reasoningTokens"]),
            cost_micros: None,
        },
    };
    identity.input_includes_cache = !matches!(adapter, UsageAdapter::Claude);
    identity.output_includes_reasoning = true;
    if let Some(cost) = raw
        .get("total_cost_usd")
        .or_else(|| raw.get("cost_usd"))
        .or_else(|| usage.get("cost_usd"))
        .and_then(serde_json::Value::as_f64)
    {
        if !cost.is_finite() || cost < 0.0 {
            return Err(AccountingRefusal::Invalid);
        }
        identity.values.cost_micros = Some(
            format!("{:.0}", cost * 1_000_000.0)
                .parse()
                .map_err(|_| AccountingRefusal::Invalid)?,
        );
    }
    Ok(identity)
}

pub fn ingest_usage(
    task: &mut TaskAccounting,
    sample: UsageSample,
) -> Result<bool, AccountingRefusal> {
    if sample.event.is_empty() || sample.event.len() > 256 || sample.ordinal == 0 {
        return Err(AccountingRefusal::Invalid);
    }
    if let Some(old) = task.usage.iter().find(|old| {
        old.attempt == sample.attempt && old.turn == sample.turn && old.event == sample.event
    }) {
        return if *old == sample {
            Ok(false)
        } else {
            Err(AccountingRefusal::Scope)
        };
    }
    if task.usage.len() >= USAGE_EVENTS {
        return Err(AccountingRefusal::Capacity);
    }
    if !task.providers.iter().any(|p| p.attempt == sample.attempt) {
        return Err(AccountingRefusal::Scope);
    }
    let mut proposed = task.usage.clone();
    proposed.push(sample);
    let totals = normalize(&proposed)?;
    task.usage = proposed;
    task.totals = totals;
    if task.pause.is_none()
        && let Some(cap) = task.token_limit
    {
        if task.totals.token_lower_bound >= cap {
            task.pause = Some(AccountingRefusal::Tokens);
        } else if task.usage.last().is_some_and(|s| s.complete) && !task.totals.tokens_complete {
            task.pause = Some(AccountingRefusal::UsageUnavailable);
        }
    }
    Ok(true)
}

/// There is no public constructor or deserializer for a send permit.
pub struct SendPermit {
    attempt: SessionId,
    payload: ObjectRef,
    prepared_at: std::time::Instant,
    remaining_ms: u64,
}

#[derive(Clone, Copy)]
pub struct ClockObservation {
    pub unix_ms: u64,
    pub monotonic_delta_ms: u64,
    pub trusted: bool,
}

impl SendPermit {
    /// Consuming this value allows one attempt-bound send of the admitted bytes.
    pub fn dispatch(
        self,
        attempt: &SessionId,
        session: &mut dyn kinewright_core::AgentSession,
        text: String,
    ) -> Result<(), kinewright_core::AgentError> {
        if self.prepared_at.elapsed().as_millis() >= u128::from(self.remaining_ms) {
            return Err(kinewright_core::AgentError::Protocol(
                "accounting time limit".into(),
            ));
        }
        if self.attempt != *attempt
            || self.payload.byte_length != text.len() as u64
            || self.payload.sha256 != kinewright_media::sha256_bytes(text.as_bytes())
        {
            return Err(kinewright_core::AgentError::Protocol(
                "accounting send scope mismatch".into(),
            ));
        }
        session.send_user_message(text)
    }
}

pub struct PreparedSend {
    record: AccountingRecord,
    reference: ObjectRef,
    send: SendConsumption,
    envelope: OrchestrationEnvelope,
    task: SessionId,
    prepared_at: std::time::Instant,
}

impl PreparedSend {
    pub fn new(
        current: &AccountingRecord,
        task: &SessionId,
        send: SendConsumption,
        envelope: &OrchestrationEnvelope,
        clock: ClockObservation,
    ) -> Result<Self, AccountingRefusal> {
        let prepared_at = std::time::Instant::now();
        envelope.validate().map_err(|_| AccountingRefusal::Scope)?;
        if envelope.project != current.project
            || envelope.id != send.command
            || envelope.authority.attempt.as_ref().map(|a| &a.0) != Some(&send.attempt)
        {
            return Err(AccountingRefusal::Scope);
        }
        if !matches!(&envelope.command, crate::orchestration::orchestration_records::OrchestrationCommand::ControlRun(intent)
            if intent.run_id.0 == current.run && matches!(&intent.action, crate::orchestration::orchestration_records::RunAction::SendMessage { text_ref } if *text_ref == send.payload))
        {
            return Err(AccountingRefusal::Scope);
        }
        current.check_clock()?;
        if current
            .tasks
            .iter()
            .flat_map(|t| &t.sends)
            .any(|s| s.command == send.command)
        {
            // Matching receipt replay belongs to H1 and cannot release a second send.
            return Err(AccountingRefusal::PublicationUnknown);
        }
        let mut record = current.successor()?;
        record.observe_clock(clock.unix_ms, clock.monotonic_delta_ms, clock.trusted)?;
        record.task_mut(task)?.consume_send(send.clone())?;
        record.validate_successor(current)?;
        let reference = record.record_reference()?;
        Ok(Self {
            record,
            reference,
            send,
            envelope: envelope.clone(),
            task: task.clone(),
            prepared_at,
        })
    }
    #[must_use]
    pub fn record(&self) -> &AccountingRecord {
        &self.record
    }
    #[must_use]
    pub fn reference(&self) -> &ObjectRef {
        &self.reference
    }

    /// Owner-only receiving port. Call after B5, never on a prepared/unknown cut.
    pub(crate) fn acknowledge(
        self,
        update: &ValidatedRunUpdate,
        returned: &OrchestrationReceipt,
    ) -> Result<SendPermit, AccountingRefusal> {
        update
            .validate_returned_receipt(returned)
            .map_err(|_| AccountingRefusal::PublicationUnknown)?;
        if returned.decision != CommandDecision::Applied
            || update.envelope() != &self.envelope
            || update.projection().run.0 != self.record.run
            || !update.projection().usage.contains(&self.reference)
            || !update.projection().tasks.iter().any(|task| task.id.0 == self.task
                && matches!(&task.state,crate::orchestration::orchestration_records::TaskState::Running(attempt) if attempt.0==self.send.attempt))
            || !update.projection().attempts.iter().any(|attempt| attempt.identity.project==self.record.project
                && attempt.identity.run.0==self.record.run && attempt.identity.task.0==self.task
                && attempt.identity.attempt.0==self.send.attempt && attempt.owner_epoch==self.envelope.authority.owner_epoch
                && matches!(attempt.state,crate::orchestration::orchestration_records::AttemptState::Executing | crate::orchestration::orchestration_records::AttemptState::Waiting))
        {
            return Err(AccountingRefusal::Scope);
        }
        Ok(SendPermit {
            attempt: self.send.attempt,
            payload: self.send.payload,
            prepared_at: self.prepared_at,
            remaining_ms: RUN_MILLIS.saturating_sub(self.record.elapsed_ms),
        })
    }
}
