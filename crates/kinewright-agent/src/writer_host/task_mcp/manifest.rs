//! Host-owned v1 manifests. Scope and caller input never change these schemas.
use super::Role;

pub(crate) const VERSION: u32 = 1;
pub(crate) const DIRECTOR: &[&str] = &[
    "get_timeline_state",
    "read_task_evidence",
    "read_task_result",
    "task_status",
    "wait_task",
    "assign_task",
    "freeze_task",
    "publish_result",
    "merge_task",
    "observe_delivery",
    "cancel_task",
    "answer_question",
    "send_message",
    "finish_task",
];
pub(crate) const EDITOR: &[&str] = &[
    "get_timeline_state",
    "read_task_evidence",
    "read_task_result",
    "prepare_edit_plan",
    "commit_edit_plan",
    "discard_edit_plan",
    "undo_task_edit",
    "redo_task_edit",
    "task_status",
    "wait_task",
    "freeze_task",
    "publish_result",
    "finish_task",
];
pub(crate) const REVIEWER: &[&str] = &[
    "get_timeline_state",
    "read_task_evidence",
    "read_task_result",
    "task_status",
    "wait_task",
    "freeze_task",
    "publish_result",
    "finish_task",
];
pub(crate) fn names(role: Role) -> &'static [&'static str] {
    match role {
        Role::Director => DIRECTOR,
        Role::Reviewer => REVIEWER,
        Role::Editor | Role::Motion | Role::Audio | Role::Colour => EDITOR,
    }
}
