//! Frozen, equal-weight paired analysis. Simulated rows cannot qualify a model.
use super::error;
use kinewright_project::orchestration_accounting::AccountingRecord;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    Baseline,
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
        let usage_complete = !accounting.tasks.is_empty()
            && accounting
                .tasks
                .iter()
                .all(|task| task.totals.tokens_complete);
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
    MixedInconclusive,
    Fail,
    Incomplete,
    StopProvider,
    BaselineOracleFailure,
    StopAll,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub verdict: Verdict,
    pub provider: String,
    pub baseline_sum: u64,
    pub candidate_sum: u64,
    pub mean_denominator: u8,
    pub baseline_median: u64,
    pub candidate_median: u64,
    pub mean_improves: bool,
    pub median_improves: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Analysis {
    pub verdict: Verdict,
    pub rows: Vec<Row>,
    pub providers: Vec<ProviderSummary>,
}
/// Requires the declared 12-row sample. It never drops, reorders or substitutes
/// a supplied row. Safety and correctness stop rules take priority over cost.
pub fn analyze(rows: &[Row]) -> io::Result<Analysis> {
    let mut keys = BTreeSet::new();
    for row in rows {
        if !["codex", "claude"].contains(&row.provider.as_str())
            || ![1, 5, 6].contains(&row.case)
            || !keys.insert((
                row.provider.as_str(),
                row.case,
                matches!(row.arm, Arm::Candidate),
            ))
        {
            return Err(error("unsupported or duplicate measurement row"));
        }
    }
    let early = if rows.iter().any(|r| !r.shared_safety) {
        Some(Verdict::StopAll)
    } else if rows.iter().any(|r| !r.correct && r.arm == Arm::Baseline) {
        Some(Verdict::BaselineOracleFailure)
    } else if rows.iter().any(|r| !r.correct) {
        Some(Verdict::StopProvider)
    } else if rows.len() != 12 || rows.iter().any(|r| !r.usage_complete) {
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
        let mut baseline = Vec::new();
        let mut candidate = Vec::new();
        for row in rows.iter().filter(|r| r.provider == provider) {
            match row.arm {
                Arm::Baseline => baseline.push(row.inclusive_lower_bound),
                Arm::Candidate => candidate.push(row.inclusive_lower_bound),
            }
        }
        baseline.sort_unstable();
        candidate.sort_unstable();
        let sum = |values: &[u64]| {
            values.iter().try_fold(0u64, |sum, value| {
                sum.checked_add(*value)
                    .ok_or_else(|| error("paired token overflow"))
            })
        };
        let baseline_sum = sum(&baseline)?;
        let candidate_sum = sum(&candidate)?;
        let mean_improves = candidate_sum < baseline_sum;
        let median_improves = candidate[1] < baseline[1];
        let verdict = match (mean_improves, median_improves) {
            (true, true) => Verdict::DeclaredSamplePass,
            (false, false) => Verdict::Fail,
            _ => Verdict::MixedInconclusive,
        };
        providers.push(ProviderSummary {
            verdict,
            provider: provider.into(),
            baseline_sum,
            candidate_sum,
            mean_denominator: 3,
            baseline_median: baseline[1],
            candidate_median: candidate[1],
            mean_improves,
            median_improves,
        });
    }
    let improvements = providers
        .iter()
        .filter(|p| p.mean_improves && p.median_improves)
        .count();
    let verdict = match improvements {
        2 => Verdict::DeclaredSamplePass,
        1 => Verdict::MixedInconclusive,
        _ if providers
            .iter()
            .any(|p| p.mean_improves || p.median_improves) =>
        {
            Verdict::MixedInconclusive
        }
        _ => Verdict::Fail,
    };
    Ok(Analysis {
        verdict,
        rows: rows.to_vec(),
        providers,
    })
}
