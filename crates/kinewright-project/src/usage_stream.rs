#![allow(clippy::missing_errors_doc)]
//! TE1 versioned cumulative usage. Stream bindings are persisted before a send.
use crate::{
    bundle_records::SessionId,
    orchestration_accounting::{AccountingRefusal, UsageValues},
};
use serde::{Deserialize, Serialize};

pub const MODELS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    #[serde(rename = "codex-0.166.0-v1")]
    Codex01660V1,
    #[serde(rename = "claude-2.1.316-v1")]
    Claude21316V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamBinding {
    pub attempt: SessionId,
    pub stream: SessionId,
    pub adapter: Adapter,
    pub resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSnapshot {
    /// Hash of the reported model identifier, never provider text.
    pub model: String,
    pub values: UsageValues,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub payload_sha256: String,
    pub stream: SessionId,
    pub adapter: Adapter,
    pub models: Vec<ModelSnapshot>,
}

impl Snapshot {
    /// Refuse category regressions, disappearing models, and changed adapters.
    /// Missing categories stay unknown until a later complete snapshot.
    pub fn validate_after(&self, old: Option<&Self>) -> Result<(), AccountingRefusal> {
        if self.payload_sha256.len() != 64
            || !self.payload_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(AccountingRefusal::Invalid);
        }
        if self.models.len() > MODELS {
            return Err(AccountingRefusal::Capacity);
        }
        for (n, model) in self.models.iter().enumerate() {
            if model.model.len() != 64
                || model.values.cost_micros.is_some()
                || !model.model.bytes().all(|b| b.is_ascii_hexdigit())
                || self.models[..n].iter().any(|m| m.model == model.model)
            {
                return Err(AccountingRefusal::Invalid);
            }
        }
        for model in &self.models {
            if (self.adapter == Adapter::Codex01660V1
                && model
                    .values
                    .cache_read
                    .zip(model.values.input)
                    .is_some_and(|(r, i)| r > i))
                || model
                    .values
                    .reasoning
                    .zip(model.values.output)
                    .is_some_and(|(r, o)| r > o)
            {
                return Err(AccountingRefusal::Invalid);
            }
        }
        if let Some(old) = old {
            if self.adapter != old.adapter || self.stream != old.stream {
                return Err(AccountingRefusal::Scope);
            }
            // An absent final snapshot records a gap, not a reset.
            if self.models.is_empty() {
                return Ok(());
            }
            for previous in &old.models {
                let next = self
                    .models
                    .iter()
                    .find(|m| m.model == previous.model)
                    .ok_or(AccountingRefusal::Scope)?;
                for (a, b) in previous
                    .values
                    .values()
                    .into_iter()
                    .zip(next.values.values())
                {
                    if a.zip(b).is_some_and(|(a, b)| b < a) {
                        return Err(AccountingRefusal::Scope);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn advance_watermark(&self, old: Option<&Self>) -> Result<Self, AccountingRefusal> {
        self.validate_after(old)?;
        if self.models.is_empty() {
            return Ok(old.unwrap_or(self).clone());
        }
        let mut next = self.clone();
        if let Some(old) = old {
            for model in &mut next.models {
                if let Some(previous) = old.models.iter().find(|m| m.model == model.model) {
                    let values = model.values.values();
                    let previous = previous.values.values();
                    model.values = UsageValues::from_values(std::array::from_fn(|i| {
                        values[i].or(previous[i])
                    }));
                }
            }
        }
        Ok(next)
    }

    pub fn values(&self) -> Result<UsageValues, AccountingRefusal> {
        let mut result = [None::<u64>; 6];
        for model in &self.models {
            for (total, value) in result.iter_mut().zip(model.values.values()) {
                *total = match (*total, value) {
                    (a, Some(b)) => Some(
                        a.unwrap_or(0)
                            .checked_add(b)
                            .ok_or(AccountingRefusal::Capacity)?,
                    ),
                    (a, None) => a,
                };
            }
        }
        Ok(UsageValues::from_values(result))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub stream: SessionId,
    pub adapter: Adapter,
    pub attempts: Vec<SessionId>,
    pub inclusive_input: Option<u64>,
    pub direct_input: Option<u64>,
    pub uncached_input: Option<u64>,
    pub output: Option<u64>,
    pub lower_bound: u64,
    pub reported_complete: bool,
    pub unreported_helpers: Option<u64>,
}

impl crate::orchestration_accounting::TaskAccounting {
    /// Report only raw streams. Legacy Cost projections cannot fill a gap.
    pub fn stream_reports(&self) -> Result<Vec<Report>, AccountingRefusal> {
        let mut reports = Vec::new();
        for binding in self.streams.iter().filter(|b| !b.resumed) {
            let bindings = self
                .streams
                .iter()
                .filter(|b| b.stream == binding.stream)
                .collect::<Vec<_>>();
            let samples = self
                .usage
                .iter()
                .filter(|s| {
                    s.snapshot
                        .as_ref()
                        .is_some_and(|s| s.stream == binding.stream)
                })
                .cloned()
                .collect::<Vec<_>>();
            let totals = crate::orchestration_accounting::normalize(&samples)?;
            let final_position = samples.last().and_then(|s| {
                bindings
                    .iter()
                    .position(|b| b.attempt == s.attempt)
                    .map(|p| (p, s.turn))
            });
            let covered = self
                .sends
                .iter()
                .filter_map(|s| {
                    bindings
                        .iter()
                        .position(|b| b.attempt == s.attempt)
                        .map(|p| (p, s.turn))
                })
                .all(|required| {
                    final_position.is_some_and(|final_position| final_position >= required)
                });
            let complete = covered
                && totals.tokens_complete
                && !self.measurements.as_ref().is_some_and(|m| m.incomplete);
            let direct = (binding.adapter == Adapter::Claude21316V1)
                .then_some(totals.known.input)
                .flatten();
            let inclusive = if binding.adapter == Adapter::Codex01660V1 {
                totals.known.input
            } else {
                direct
                    .zip(totals.known.cache_read)
                    .zip(totals.known.cache_write)
                    .and_then(|((d, r), w)| d.checked_add(r)?.checked_add(w))
            };
            let uncached = if totals.complete[2] {
                inclusive
                    .zip(totals.known.cache_read)
                    .and_then(|(i, r)| i.checked_sub(r))
            } else {
                None
            };
            reports.push(Report {
                stream: binding.stream.clone(),
                adapter: binding.adapter,
                attempts: bindings.iter().map(|b| b.attempt.clone()).collect(),
                inclusive_input: complete.then_some(inclusive).flatten(),
                direct_input: complete.then_some(direct).flatten(),
                uncached_input: complete.then_some(uncached).flatten(),
                output: complete.then_some(totals.known.output).flatten(),
                lower_bound: totals.token_lower_bound,
                reported_complete: complete,
                unreported_helpers: None,
            });
        }
        Ok(reports)
    }
}
