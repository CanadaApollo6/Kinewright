//! One bounded raw observer for both versioned provider protocols.
use kinewright_core::{AgentEvent, AgentUsage};
use kinewright_project::{
    bundle_records::SessionId,
    orchestration_accounting::{AccountingRefusal, UsageMode, UsageSample, UsageValues, normalize},
    usage_stream::{Adapter, Snapshot},
};

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
    pub(crate) fn project(&mut self, event: &mut AgentEvent) {
        let AgentEvent::Cost {
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
            reasoning_output_tokens,
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
        if self.samples.len() >= 1024 {
            return Err(AccountingRefusal::Capacity);
        }
        self.ordinal += 1;
        let event = raw.get("id").map_or_else(
            || format!("{kind}:{:064x}", self.ordinal),
            |id| {
                format!(
                    "{kind}:{}",
                    kinewright_media::sha256_bytes(&serde_json::to_vec(id).unwrap_or_default())
                )
            },
        );
        let old = self.samples.iter().find(|s| s.event == event);
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
                ordinal: old.map_or(self.ordinal, |s| s.ordinal),
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
        } else {
            self.samples.push(sample);
        }
        let usage = self.observation(adapter)?;
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
