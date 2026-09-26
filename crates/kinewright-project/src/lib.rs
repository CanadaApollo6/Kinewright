//! Shared project IO for the GUI and the headless CLI (AW1 §2), moved
//! from the app in AW1 S1 (zero logic change except the named
//! [`SidecarSession`] extraction delta).

pub mod headless;
pub mod lockfile;
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
    canonical_project_identity, canonical_session_key, derive_lut_store, load_document,
    min_required_format_version, project_newer_format_observation, serialize_project_document,
    write_file_atomic, write_project_bytes, write_project_document,
};
pub use recovery::{
    JOURNAL_MAGIC, allocate_journal_path, default_recovery_directory, fnv1a_64, journal_file_name,
    pending_journal_for_project, restore_status, retire_journal_for_project,
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
    if std::env::var("REV2_HOOK").ok().as_deref() != Some(point) {
        return;
    }
    let dir = std::path::PathBuf::from(std::env::var_os("REV2_SIGNALS").unwrap());
    std::fs::write(dir.join("paused"), point).unwrap();
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
