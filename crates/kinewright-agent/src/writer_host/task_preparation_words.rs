//! Person-facing preparation recovery instructions, shared with Studio.
macro_rules! recovery {
    ($why:literal, $continue:literal) => {
        concat!($why, " This start sent nothing. Press Stop. To review kept task edits, choose \"Save current edit, then open <branch>\", then \"Save current work, then open recovery point\". ", $continue)
    };
}
pub const SENT_TASK_RECOVERY: &str = recovery!(
    "The writer host cannot resume this task.",
    "Continue cannot retry this task."
);
pub const DIRECTOR_RECOVERY: &str = recovery!(
    "The writer host could not prepare the Director.",
    "Continue cannot repair it."
);
pub const FROZEN_RECOVERY: &str = recovery!(
    "The task's frozen checkpoint changed.",
    "Continue cannot repair this task."
);
pub const MISSING_RECOVERY: &str = recovery!(
    "The task's saved history is unavailable.",
    "Continue cannot repair this task."
);
pub const LEGACY_RECOVERY: &str = recovery!(
    "The task's previous preparation cannot be verified.",
    "Continue cannot repair this task."
);
pub const BASE_RECOVERY: &str = recovery!(
    "The task's saved history has conflicting bases.",
    "Continue cannot repair this task."
);
pub const UPGRADE_RECOVERY: &str = "This run was admitted with a different application version or launch recipe. Press Stop, then Reopen to keep and review your notes, and start a new run. Continue cannot retry this send.";
#[must_use]
pub fn is_recovery_reason(reason: &str) -> bool {
    [
        SENT_TASK_RECOVERY,
        DIRECTOR_RECOVERY,
        FROZEN_RECOVERY,
        MISSING_RECOVERY,
        LEGACY_RECOVERY,
        BASE_RECOVERY,
        UPGRADE_RECOVERY,
    ]
    .contains(&reason)
}
