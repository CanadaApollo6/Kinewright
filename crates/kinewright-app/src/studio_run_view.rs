//! Notes reads the host's durable run/accounting cut. No display counters own it.
use kinewright_agent::writer_host::RunViewPage;
use kinewright_project::orchestration_accounting::UsageTotals;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunView {
    pub(crate) edit_approvals: Vec<String>,
    pub(crate) elapsed: String,
    pub(crate) runtime_status: Option<String>,
    pub(crate) stop_reason: Option<String>,
    pub(crate) wait_reason: Option<String>,
    pub(crate) continue_unavailable_reason: Option<String>,
    pub(crate) tasks: Vec<TaskView>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskView {
    pub(crate) completion: Option<String>,
    pub(crate) wait_reason: Option<String>,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) usage: String,
}
impl RunView {
    pub(crate) fn of(page: &RunViewPage) -> Self {
        let accounting = page.accounting.as_ref();
        Self {
            edit_approvals: edit_lines(&page.edit_approvals, &page.missing_approved_edits),
            runtime_status: page.runtime_status.clone(),
            wait_reason: page.wait_reason.clone(),
            continue_unavailable_reason: page.continue_unavailable_reason.clone(),
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
                        wait_reason: record.and_then(|record| record.wait_reason.clone()),
                        completion: page
                            .completions
                            .iter()
                            .find(|status| status.task == task.id)
                            .map(|status| {
                                use kinewright_agent::writer_host::{
                                    CompletionPhase, CompletionState,
                                };
                                match status.state {
                                    CompletionState::Completed => "Completion complete".into(),
                                    CompletionState::Blocked => "Completion needs review".into(),
                                    CompletionState::Pending => match status.phase {
                                        CompletionPhase::Freeze => {
                                            "Completion pending: freeze".into()
                                        }
                                        CompletionPhase::Publication => {
                                            "Completion pending: publication".into()
                                        }
                                        CompletionPhase::Delivery => {
                                            "Completion pending: parent delivery".into()
                                        }
                                        CompletionPhase::Settled => "Completion pending".into(),
                                    },
                                }
                            }),
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
pub(crate) fn plain_wait_reason(reason: &str, settings: bool) -> &str {
    if reason.contains("The send outcome is unknown.") {
        "The send outcome is unknown. Press Stop, then Reopen to review your notes before starting another run. Continue cannot retry this send."
    } else if reason == kinewright_agent::job_guardian::protocol::INPUT_TOO_LONG {
        reason
    } else if reason.starts_with("silent_stall:") {
        "The run has no scheduled work. Press Stop, then Reopen to review your notes before starting another run."
    } else if reason.starts_with("run_limit:") {
        "This run reached its limit. Press Stop to finish cleanup."
    } else if reason.starts_with("native runtime refusal:") {
        "The run could not continue. Check Run details for the reason."
    } else if reason == "The provider check failed before anything was sent. Continue to try again."
    {
        "The provider check failed before anything was sent. Continue to try again."
    } else if reason
        == "The writer host could not start the task yet. This start sent nothing. Continue to try again."
    {
        "The writer host could not start the task yet. This start sent nothing. Continue to try again."
    } else if kinewright_agent::writer_host::task_preparation_words::is_recovery_reason(reason) {
        reason
    } else if settings {
        "The provider check needs attention. Fix Director and Specialists in Settings, then press Continue."
    } else {
        "The run needs attention. Check Run details for the reason."
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
fn removal_words(clips: usize, tracks: usize) -> String {
    let mut parts = Vec::new();
    if clips > 0 {
        parts.push(format!(
            "{clips} {}",
            if clips == 1 { "clip" } else { "clips" }
        ));
    }
    if tracks > 0 {
        parts.push(format!(
            "{tracks} {}",
            if tracks == 1 { "track" } else { "tracks" }
        ));
    }
    parts.join(" and ")
}
fn edit_lines(
    rows: &[kinewright_agent::writer_host::EditApproval],
    missing: &[kinewright_project::edit_audit::MissingApprovedEdit],
) -> Vec<String> {
    let is_missing = |row: &kinewright_agent::writer_host::EditApproval| {
        missing.iter().any(|edit| edit.digest == row.digest)
    };
    let applied = rows
        .iter()
        .filter(|r| r.applied && r.issue.is_none() && !is_missing(r))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    if applied.len() > 5 {
        lines.push(format!("{} edits removed {} in the draft version. Your project is unchanged until you choose it.",applied.len(),removal_words(applied.iter().map(|r|r.removed_clips).sum(),applied.iter().map(|r|r.removed_tracks).sum())));
    } else {
        for r in applied {
            lines.push(format!(
                "Removed {} in the draft version. Your project is unchanged until you choose it.",
                removal_words(r.removed_clips, r.removed_tracks)
            ));
        }
    }
    let pending = rows
        .iter()
        .filter(|r| !r.applied && r.issue.is_none() && !is_missing(r))
        .count();
    if pending > 0 {
        lines.push(format!(
            "{pending} {} approved, but {} changes are not confirmed.",
            if pending == 1 {
                "edit was"
            } else {
                "edits were"
            },
            if pending == 1 { "its" } else { "their" }
        ));
    }
    if rows.iter().any(|r| r.issue.is_some() && !is_missing(r)) {
        lines.push("Some edit records could not be verified.".into());
    }
    if !missing.is_empty() {
        lines.push(kinewright_project::edit_audit::missing_edits_disclosure(
            missing,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c4b_missing_approved_edits_are_named_and_never_displayed_as_applied() {
        use kinewright_project::{
            bundle_records::{ObjectRef, RecordKind, SessionId},
            edit_audit::MissingApprovedEdit,
            orchestration_records::TaskId,
        };
        let task = TaskId(SessionId::fresh().unwrap());
        let missing = [1, 2]
            .into_iter()
            .map(|clip| MissingApprovedEdit {
                task: task.clone(),
                approval: ObjectRef {
                    sha256: format!("{clip:064x}"),
                    byte_length: 1,
                    record_kind: RecordKind::OwnedEvidence,
                },
                digest: format!("{clip:064x}"),
                description: format!("Remove clip {clip} on track 81"),
            })
            .collect::<Vec<_>>();
        let rows = missing
            .iter()
            .map(|edit| kinewright_agent::writer_host::EditApproval {
                task: task.clone(),
                removed_clips: 1,
                removed_tracks: 0,
                applied: false,
                issue: Some("The approved edit is not in the kept version.".into()),
                digest: edit.digest.clone(),
            })
            .collect::<Vec<_>>();
        let lines = edit_lines(&rows, &missing);
        assert_eq!(lines.len(), 1);
        let words = &lines[0];
        assert!(words.contains("2 approved edits are not in the kept version"));
        assert!(words.contains("clip 1") && words.contains("clip 2"));
        assert!(words.contains("Re-run") && words.contains("continue the task"));
        assert!(!words.contains("Removed") && !words.contains("could not be verified"));
    }
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
