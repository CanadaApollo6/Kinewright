//! Host-owned versioned manifests. Scope and caller input never change these schemas.
use super::orchestration_records::Role;

pub(crate) const VERSION: u32 = 3;
pub(crate) const DIRECTOR: &[&str] = &[
    "get_timeline_transcript",
    "check_dialogue_cut",
    "read_child_result",
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
    "propose_rulebook_revision",
];
macro_rules! editing_tools {
    ($($extra:literal),*) => {
        &[
            "get_timeline_transcript",
            "cut_dialogue",
            "check_dialogue_cut",
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
            $($extra),*
        ]
    };
}
pub(crate) const EDITOR: &[&str] = editing_tools!("record_editorial_audit");
const SPECIALIST: &[&str] = editing_tools!();
pub(crate) const REVIEWER: &[&str] = &[
    "get_timeline_transcript",
    "check_dialogue_cut",
    "get_timeline_state",
    "read_task_evidence",
    "read_task_result",
    "read_review_stills",
    "task_status",
    "wait_task",
    "freeze_task",
    "publish_result",
    "finish_task",
    "record_editorial_audit",
];
pub(crate) fn names(role: Role) -> &'static [&'static str] {
    match role {
        Role::Director => DIRECTOR,
        Role::Reviewer => REVIEWER,
        Role::Editor => EDITOR,
        Role::Motion | Role::Audio | Role::Colour => SPECIALIST,
    }
}
