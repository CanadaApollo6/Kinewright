//! One bounded raw observer for both versioned provider protocols.
use kinewright_core::{AgentEvent, AgentUsage};
use kinewright_project::{
    bundle_records::SessionId,
    orchestration_accounting::{AccountingRefusal, UsageMode, UsageSample, UsageValues, normalize},
    usage_stream::{Adapter, Snapshot},
};

#[allow(
    clippy::cast_precision_loss,
    reason = "provider USD estimates use f64; retained counters remain integer micros"
)]
pub(crate) fn cost_usd(micros: u64) -> f64 {
    micros as f64 / 1_000_000.0
}

pub(crate) struct UsageObserver {
    stream: SessionId,
    samples: Vec<UsageSample>,
    ordinal: u64,
    current: Option<AgentUsage>,
    projected: Option<AgentUsage>,
}
impl Default for UsageObserver {
    fn default() -> Self {
        Self {
            stream: SessionId::fresh().expect("host usage identity"),
            samples: vec![],
            ordinal: 0,
            current: None,
            projected: None,
        }
    }
}
impl UsageObserver {
    pub(crate) fn has_known_tokens(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|usage| usage.input.is_some() && usage.output.is_some())
    }
    pub(crate) fn project(&mut self, event: &mut AgentEvent) {
        let AgentEvent::Cost {
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            cost_usd: projected_cost,
            ..
        } = event
        else {
            return;
        };
        let Some(now) = &self.current else {
            return;
        };
        let delta = |value: Option<u64>, previous: Option<u64>| {
            value.map(|v| v.saturating_sub(previous.unwrap_or(0)))
        };
        let old = self.projected.as_ref();
        let old_cost = old.and_then(|u| u.cost_micros);
        let cost = now.cost_complete.then_some(now.cost_micros).flatten();
        *projected_cost = delta(cost, old_cost).map(cost_usd);
        *input_tokens = delta(now.input, old.and_then(|u| u.input)).unwrap_or(0);
        *output_tokens = delta(now.output, old.and_then(|u| u.output)).unwrap_or(0);
        *cached_input_tokens = delta(now.cache_read, old.and_then(|u| u.cache_read));
        *cache_creation_input_tokens = delta(now.cache_write, old.and_then(|u| u.cache_write));
        *reasoning_output_tokens = delta(now.reasoning, old.and_then(|u| u.reasoning));
        let mut watermark = self.projected.take().unwrap_or_else(|| now.clone());
        watermark.input = now.input.or(watermark.input);
        watermark.output = now.output.or(watermark.output);
        watermark.cache_read = now.cache_read.or(watermark.cache_read);
        watermark.cache_write = now.cache_write.or(watermark.cache_write);
        watermark.reasoning = now.reasoning.or(watermark.reasoning);
        watermark.cost_micros = cost.or(old_cost);
        watermark.cost_complete = now.cost_complete;
        self.projected = Some(watermark);
    }

    pub(crate) fn observe(
        &mut self,
        adapter: Adapter,
        raw: &serde_json::Value,
    ) -> Result<Option<AgentEvent>, AccountingRefusal> {
        let kind = raw["type"].as_str().unwrap_or_default();
        if !matches!(
            (adapter, kind),
            (
                Adapter::Codex01660V1,
                "turn.completed" | "turn.usage" | "turn.started"
            ) | (Adapter::Claude21316V1, "result")
        ) {
            return Ok(None);
        }
        let ordinal = self
            .ordinal
            .checked_add(1)
            .ok_or(AccountingRefusal::Capacity)?;
        let identity_field = match adapter {
            Adapter::Codex01660V1 => "id",
            Adapter::Claude21316V1 => "uuid",
        };
        let event = raw.get(identity_field).map_or_else(
            || format!("{kind}:{ordinal:064x}"),
            |id| {
                format!(
                    "{kind}:{}",
                    kinewright_media::sha256_bytes(&serde_json::to_vec(id).unwrap_or_default())
                )
            },
        );
        let old = self.samples.iter().find(|s| s.event == event);
        if old.is_none() && self.samples.len() >= 1024 {
            return Err(AccountingRefusal::Capacity);
        }
        let sample = crate::protocol::limits::usage::decode_usage(
            match adapter {
                Adapter::Codex01660V1 => crate::protocol::limits::usage::UsageAdapter::Codex,
                Adapter::Claude21316V1 => crate::protocol::limits::usage::UsageAdapter::Claude,
            },
            raw,
            UsageSample {
                snapshot: Some(Snapshot {
                    payload_sha256: "0".repeat(64),
                    stream: self.stream.clone(),
                    adapter,
                    models: vec![],
                }),
                attempt: self.stream.clone(),
                turn: 1,
                event,
                ordinal: old.map_or(ordinal, |s| s.ordinal),
                mode: UsageMode::Cumulative,
                values: UsageValues::default(),
                complete: matches!(kind, "turn.completed" | "result"),
                input_includes_cache: adapter == Adapter::Codex01660V1,
                output_includes_reasoning: true,
            },
        )?;
        if let Some(old) = old {
            if old != &sample {
                return Err(AccountingRefusal::Scope);
            }
            return Ok(None);
        }
        self.samples.push(sample);
        let usage = match self.observation(adapter) {
            Ok(usage) => usage,
            Err(reason) => {
                self.samples.pop();
                return Err(reason);
            }
        };
        self.ordinal = ordinal;
        self.current = Some(usage.clone());
        Ok(Some(AgentEvent::Usage(Box::new(usage))))
    }
    fn observation(&self, adapter: Adapter) -> Result<AgentUsage, AccountingRefusal> {
        let totals = normalize(&self.samples)?;
        let category =
            |index: usize, value: Option<u64>| totals.complete[index].then_some(value).flatten();
        let direct = (adapter == Adapter::Claude21316V1)
            .then_some(category(0, totals.known.input))
            .flatten();
        let input = if adapter == Adapter::Claude21316V1 {
            category(0, totals.known.input)
                .zip(category(2, totals.known.cache_read))
                .zip(category(3, totals.known.cache_write))
                .and_then(|((d, r), w)| d.checked_add(r)?.checked_add(w))
        } else {
            category(0, totals.known.input)
        };
        Ok(AgentUsage {
            stream: self.stream.as_str().to_owned(),
            adapter: match adapter {
                Adapter::Codex01660V1 => "codex-0.166.0-v1",
                Adapter::Claude21316V1 => "claude-2.1.316-v1",
            }
            .into(),
            input,
            direct,
            cache_read: category(2, totals.known.cache_read),
            cache_write: category(3, totals.known.cache_write),
            output: category(1, totals.known.output),
            reasoning: category(4, totals.known.reasoning),
            lower_bound: totals.token_lower_bound,
            complete: totals.tokens_complete,
            cost_micros: totals.known.cost_micros,
            cost_complete: totals.complete[5],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn cost() -> AgentEvent {
        AgentEvent::Cost {
            input_tokens: 0,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens: 0,
            reasoning_output_tokens: None,
            cost_usd: None,
        }
    }
    fn claude_result(
        uuid: &str,
        direct: u64,
        read: u64,
        write: u64,
        output: u64,
    ) -> serde_json::Value {
        json!({
            "type":"result", "subtype":"success", "uuid":uuid,
            "session_id":"fixture-session", "is_error":false,
            "duration_ms":1, "duration_api_ms":1, "num_turns":1,
            "result":"fixture response",
            "modelUsage":{"main":{"inputTokens":direct,"cacheReadInputTokens":read,
                "cacheCreationInputTokens":write,"outputTokens":output}}
        })
    }
    #[test]
    fn te1_claude_uuid_replay_and_changed_payload_are_checked_at_ingress() {
        let mut observer = UsageObserver::default();
        let raw = claude_result("f347b5fa-c2a4-45a4-96e7-341109556d01", 14, 50, 7, 8);
        assert!(
            matches!(observer.observe(Adapter::Claude21316V1, &raw).unwrap(),
            Some(AgentEvent::Usage(u)) if u.lower_bound == 79 && u.complete)
        );
        assert!(
            observer
                .observe(Adapter::Claude21316V1, &raw)
                .unwrap()
                .is_none(),
            "TE1_UUID_REPLAY_HAS_NO_NEW_OBSERVATION"
        );
        let mut changed_text = raw.clone();
        changed_text["result"] = json!("changed fixture response");
        assert_eq!(
            observer.observe(Adapter::Claude21316V1, &changed_text),
            Err(AccountingRefusal::Scope),
            "TE1_UUID_PAYLOAD_HASH_REFUSED"
        );
        let changed = claude_result("f347b5fa-c2a4-45a4-96e7-341109556d01", 20, 60, 10, 10);
        assert_eq!(
            observer.observe(Adapter::Claude21316V1, &changed),
            Err(AccountingRefusal::Scope),
            "TE1_UUID_CHANGED_PAYLOAD_REFUSED"
        );
        assert_eq!(observer.current.as_ref().unwrap().lower_bound, 79);
        assert_eq!(observer.samples.len(), 1);
    }
    #[test]
    fn te1_claude_uuid_duplicates_do_not_consume_capacity_even_when_full() {
        let mut observer = UsageObserver::default();
        for n in 0..1024 {
            observer
                .observe(
                    Adapter::Claude21316V1,
                    &claude_result(
                        &format!("{n:08x}-c2a4-45a4-96e7-341109556d01"),
                        14,
                        50,
                        7,
                        8,
                    ),
                )
                .unwrap();
        }
        let last = claude_result("000003ff-c2a4-45a4-96e7-341109556d01", 14, 50, 7, 8);
        for _ in 0..1025 {
            assert!(
                observer
                    .observe(Adapter::Claude21316V1, &last)
                    .unwrap()
                    .is_none(),
                "TE1_UUID_DUPLICATE_AT_CAPACITY"
            );
        }
        let changed = claude_result("000003ff-c2a4-45a4-96e7-341109556d01", 20, 60, 10, 10);
        assert_eq!(
            observer.observe(Adapter::Claude21316V1, &changed),
            Err(AccountingRefusal::Scope)
        );
        let extra = claude_result("00000400-c2a4-45a4-96e7-341109556d01", 14, 50, 7, 8);
        assert_eq!(
            observer.observe(Adapter::Claude21316V1, &extra),
            Err(AccountingRefusal::Capacity)
        );
        assert_eq!(observer.current.as_ref().unwrap().lower_bound, 79);
        assert_eq!(observer.samples.len(), 1024);
    }
    #[test]
    fn te1_refused_regression_does_not_poison_later_valid_observations() {
        for adapter in [Adapter::Codex01660V1, Adapter::Claude21316V1] {
            let raw = |identity: &str, input| match adapter {
                Adapter::Codex01660V1 => {
                    json!({"type":"turn.completed", "id":identity, "usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":10}})
                }
                Adapter::Claude21316V1 => claude_result(identity, input, 0, 0, 10),
            };
            let mut observer = UsageObserver::default();
            assert!(
                matches!(observer.observe(adapter, &raw("first", 90)).unwrap(), Some(AgentEvent::Usage(u)) if u.complete && u.lower_bound==100)
            );
            assert_eq!(
                observer.observe(adapter, &raw("regressed", 40)),
                Err(AccountingRefusal::Scope)
            );
            assert!(
                matches!(observer.observe(adapter, &raw("recovered", 190)).unwrap(), Some(AgentEvent::Usage(u)) if u.complete && u.lower_bound==200),
                "TE1_REFUSAL_ROLLS_BACK_OBSERVER"
            );
        }
    }
    #[test]
    fn te1_missing_category_is_unobserved_without_resetting_display_watermarks() {
        let mut observer = UsageObserver::default();
        observer.observe(Adapter::Codex01660V1,&json!({"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":10}})).unwrap();
        let mut projection = cost();
        observer.project(&mut projection);
        let raw = observer
            .observe(
                Adapter::Codex01660V1,
                &json!({"type":"turn.completed","usage":{"output_tokens":18}}),
            )
            .unwrap()
            .unwrap();
        assert!(
            matches!(raw,AgentEvent::Usage(u) if u.input.is_none() && u.cache_read.is_none() && !u.complete && u.lower_bound==118)
        );
        observer.project(&mut cost());
        observer.observe(Adapter::Codex01660V1,&json!({"type":"turn.completed","usage":{"input_tokens":200,"cached_input_tokens":160,"output_tokens":20}})).unwrap();
        let mut projection = cost();
        observer.project(&mut projection);
        assert!(matches!(
            projection,
            AgentEvent::Cost {
                input_tokens: 100,
                output_tokens: 2,
                cached_input_tokens: Some(80),
                ..
            }
        ));
    }
}
