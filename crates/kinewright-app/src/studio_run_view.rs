//! Notes reads the host's durable run/accounting cut. No display counters own it.
use kinewright_agent::writer_host::RunViewPage;
use kinewright_project::orchestration_accounting::UsageTotals;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunView {
    pub(crate) elapsed: String,
    pub(crate) stop_reason: Option<String>,
    pub(crate) tasks: Vec<TaskView>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskView {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) usage: String,
}
impl RunView {
    pub(crate) fn of(page: &RunViewPage) -> Self {
        let accounting = page.accounting.as_ref();
        Self {
            elapsed: accounting.map_or_else(
                || "Elapsed unavailable / 6 hours".into(),
                |record| {
                    format!(
                        "{} / 6 hours{}",
                        duration(record.elapsed_ms),
                        if record.clock_uncertain {
                            " · clock unavailable"
                        } else {
                            ""
                        }
                    )
                },
            ),
            stop_reason: accounting.and_then(|record| record.stop_reason.clone()),
            tasks: page
                .projection
                .tasks
                .iter()
                .map(|task| {
                    let record = accounting.and_then(|record| {
                        record.tasks.iter().find(|record| record.task == task.id)
                    });
                    let provider = record.and_then(|record| record.provider.as_ref());
                    TaskView {
                        provider: provider.map_or_else(
                            || "unavailable".into(),
                            |provider| provider.resolved.connector.clone(),
                        ),
                        model: provider.map_or_else(
                            || "unavailable".into(),
                            |provider| provider.resolved.model.clone(),
                        ),
                        usage: usage(record.map(|record| &record.totals)),
                    }
                })
                .collect(),
        }
    }
}
fn duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}
fn value(value: Option<u64>, complete: bool) -> String {
    match value {
        None => "unavailable".into(),
        Some(value) if complete => value.to_string(),
        Some(value) => format!("{value} known · total unavailable"),
    }
}
fn usage(totals: Option<&UsageTotals>) -> String {
    let empty = UsageTotals::default();
    let totals = totals.unwrap_or(&empty);
    let values = totals.known.values();
    let labels = [
        "Input",
        "Output",
        "Cache read",
        "Cache write",
        "Reasoning",
        "Cost (USD)",
    ];
    labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let display = if index == 5 {
                values[index].map_or_else(
                    || "unavailable".into(),
                    |amount| {
                        let amount = format!("${}.{:06}", amount / 1_000_000, amount % 1_000_000);
                        if totals.complete[index] {
                            amount
                        } else {
                            format!("{amount} known · total unavailable")
                        }
                    },
                )
            } else {
                value(values[index], totals.complete[index])
            };
            format!("{label}: {display}")
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c4_run_view_unknown_usage_stays_unavailable() {
        let rendered = usage(None);
        assert_eq!(rendered.matches("unavailable").count(), 6);
        assert!(!rendered.contains('0'), "C4_UNKNOWN_USAGE_NEVER_ZERO");
        let mut totals = UsageTotals::default();
        totals.known.input = Some(9);
        totals.known.cost_micros = Some(4);
        assert!(usage(Some(&totals)).contains("9 known · total unavailable"));
        totals.complete[0] = true;
        totals.complete[5] = true;
        assert!(usage(Some(&totals)).contains("Input: 9 ·"));
        assert!(usage(Some(&totals)).contains("$0.000004"));
        assert_eq!(duration(21_600_000), "6:00:00");
    }
}
