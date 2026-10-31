//! Shared project IO for the GUI and the headless CLI (AW1 §2), moved
//! from the app in AW1 S1 (zero logic change except the named
//! [`SidecarSession`] extraction delta).

pub mod bundle;
mod bundle_branch;
pub mod bundle_commit;
pub mod bundle_copy;
pub mod bundle_records;
pub mod bundle_recovery;
pub mod bundle_rescue;
pub mod bundle_switch;
pub mod editing_session;
// JA-1 S3 C4
pub mod feedback_mapping;
mod history_save;
pub mod rescue_record;
pub use history_save::{AttemptBinding, ProjectAttachment};
#[cfg(feature = "test-util")]
pub mod c0_fixtures;
mod certified_resources;
pub mod owned_inputs;
mod owned_media;
mod read_only_inputs;
pub use read_only_inputs::{ReadOnlyInputBinding, ReadOnlyInputFiles, ReadOnlyInputs};
// S4 C5
pub mod orchestration_accounting;
pub mod orchestration_commit;
pub mod orchestration_records;
pub mod receipts;
pub mod source_boundary;
pub mod storage_budget;
pub mod store;
// JA-1 S3 C1 shared metadata and S4 receiving contracts.
pub mod studio_records;
pub mod studio_store;
// JA-1 S3 C5 fix 2
pub mod voice_ingress;
pub mod voice_store;

pub mod headless;
#[cfg(any(test, feature = "test-util"))]
pub mod io_observer;
pub mod legacy_io;
pub mod lockfile;
pub mod namespace;
pub mod project;
pub mod recovery;
pub mod session;
pub mod sidecar;

pub use headless::{HeadlessSaveReport, save_headless};
pub use lockfile::{
    AcquiredLock, LOCK_ACQUIRE_ATTEMPTS, LOCK_ACQUIRE_RETRY_DELAY, LOCKFILE_FORMAT_VERSION,
    LOCKFILE_SUFFIX, LockMode, LockfileClaim, LockfileError, LockfileHandle, ReclaimedOwner,
    acquire_project_lock, acquire_project_lock_with_policy, discovery_path_for_project,
    lockfile_path_for_project, reclaim_warning_json, reclaim_warning_json_unreadable,
};
pub use project::{
    ProjectFile, ProjectIdentityError, ProjectSaveError, ProjectSaveReport, can_overwrite_save,
    canonical_project_identity, derive_lut_store, load_document, min_required_format_version,
    project_newer_format_observation, same_project_file, serialize_project_document,
    write_file_atomic, write_project_bytes, write_project_document,
};
pub use recovery::{
    FeedbackStateLock, FeedbackStateRefusal, JOURNAL_MAGIC, allocate_journal_path,
    default_recovery_directory, feedback_state_directory, feedback_state_directory_with, fnv1a_64,
    journal_file_name, lock_feedback_state, pending_journal_for_project, restore_status,
    retire_journal_for_project,
};
pub use session::{
    IncidentLogHandle, LoadedSidecarSession, SidecarRollback, SidecarSession,
    rollback_sidecar_write, snapshot_sidecar_rollback,
};
pub use sidecar::{
    FlushOutcome, LoadedSidecar, RefuseRename, SIDECAR_FORMAT_VERSION, SIDECAR_SUFFIX, SidecarLoad,
    SidecarMode, SidecarWriter, build_sidecar_bytes, digest_bytes, load_sidecar, refuse_sidecar,
    refuse_sidecar_with, sidecar_matches_project, sidecar_path_for_project,
    sidecar_refused_observation, sidecar_write_failed_observation, write_synced,
};

// Lock-test scheduling (F3); per-child REV2_HOOK/SIGNALS/EXIT.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn test_hook(point: &str) {
    io_observer::at_test_hook(point);
    if std::env::var("REV2_HOOK").ok().as_deref() != Some(point) {
        return;
    }
    let dir = std::path::PathBuf::from(std::env::var_os("REV2_SIGNALS").unwrap());
    std::fs::write(dir.join("paused.partial"), point).unwrap();
    std::fs::rename(dir.join("paused.partial"), dir.join("paused")).unwrap();
    if std::env::var("REV2_EXIT").is_ok() {
        std::process::exit(77);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !dir.join("resume").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "test hook timed out: {point}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

pub mod manual_history;

mod copy_publication;
mod task_checkpoint;

mod candidate_recovery;

mod orchestration_cumulative;
