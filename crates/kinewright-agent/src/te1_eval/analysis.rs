//! Candidate-only measurement summaries. Simulated rows cannot qualify a model.
use super::error;
use kinewright_project::orchestration_accounting::AccountingRecord;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    Candidate,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub provider: String,
    pub case: u8,
    pub arm: Arm,
    pub inclusive_lower_bound: u64,
    pub usage_complete: bool,
    pub correct: bool,
    pub shared_safety: bool,
}
impl Row {
    /// Accounting is the full durable run, including failed/retired attempts
    /// and every task. Caller must retain rows even when correctness fails.
    pub fn from_accounting(
        provider: &str,
        case: u8,
        arm: Arm,
        accounting: &AccountingRecord,
        correct: bool,
    ) -> io::Result<Self> {
        let inclusive_lower_bound = accounting.tasks.iter().try_fold(0u64, |sum, task| {
            sum.checked_add(task.totals.token_lower_bound)
                .ok_or_else(|| error("inclusive token overflow"))
        })?;
        accounting.validate().map_err(|e| error(format!("{e:?}")))?;
        let mut usage_complete = accounting.tasks.iter().any(|task| !task.sends.is_empty());
        for task in &accounting.tasks {
            let reports = task.stream_reports().map_err(|e| error(format!("{e:?}")))?;
            usage_complete &= !task.measurements.as_ref().is_some_and(|m| m.incomplete)
                && task.sends.iter().all(|send| {
                    reports.iter().any(|report| {
                        report.reported_complete && report.attempts.contains(&send.attempt)
                    })
                });
        }
        Ok(Self {
            provider: provider.into(),
            case,
            arm,
            inclusive_lower_bound,
            usage_complete,
            correct,
            shared_safety: true,
        })
    }
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    DeclaredSamplePass,
    Incomplete,
    StopProvider,
    StopAll,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub verdict: Verdict,
    pub provider: String,
    pub candidate_sum: u64,
    pub mean_denominator: u8,
    pub candidate_median: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Analysis {
    pub verdict: Verdict,
    pub rows: Vec<Row>,
    pub providers: Vec<ProviderSummary>,
}
/// Requires the declared six candidate rows. It never drops, reorders or substitutes
/// a supplied row. Safety and correctness stop rules take priority over cost.
pub fn analyze(rows: &[Row]) -> io::Result<Analysis> {
    let mut keys = BTreeSet::new();
    for row in rows {
        if !["codex", "claude"].contains(&row.provider.as_str())
            || ![1, 5, 6].contains(&row.case)
            || !keys.insert((row.provider.as_str(), row.case))
        {
            return Err(error("unsupported or duplicate measurement row"));
        }
    }
    let early = if rows.iter().any(|r| !r.shared_safety) {
        Some(Verdict::StopAll)
    } else if rows.iter().any(|r| !r.correct) {
        Some(Verdict::StopProvider)
    } else if rows.len() != 6 || rows.iter().any(|r| !r.usage_complete) {
        Some(Verdict::Incomplete)
    } else {
        None
    };
    if let Some(verdict) = early {
        return Ok(Analysis {
            verdict,
            rows: rows.to_vec(),
            providers: vec![],
        });
    }
    let mut providers = Vec::new();
    for provider in ["codex", "claude"] {
        let mut candidate: Vec<_> = rows
            .iter()
            .filter(|r| r.provider == provider)
            .map(|r| r.inclusive_lower_bound)
            .collect();
        candidate.sort_unstable();
        let candidate_sum = candidate.iter().try_fold(0u64, |sum, value| {
            sum.checked_add(*value)
                .ok_or_else(|| error("candidate token overflow"))
        })?;
        providers.push(ProviderSummary {
            verdict: Verdict::DeclaredSamplePass,
            provider: provider.into(),
            candidate_sum,
            mean_denominator: 3,
            candidate_median: candidate[1],
        });
    }
    let verdict = Verdict::DeclaredSamplePass;
    Ok(Analysis {
        verdict,
        rows: rows.to_vec(),
        providers,
    })
}
