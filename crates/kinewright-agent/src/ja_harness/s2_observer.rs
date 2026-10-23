//! C1 observations of ordinary production save outcomes, identical in both harness modes.
#[cfg(target_os = "linux")]
use kinewright_core::TimelineRevision;
use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
#[cfg(target_os = "linux")]
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle, PublicationState},
    bundle_commit::{BundleCommit, CommitOutcome},
    bundle_copy::{CopyStatus, SessionStore},
    io_observer,
    owned_inputs::OwnedInputManifest,
    storage_budget::StorageLimits,
};
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use std::path::Path;

#[cfg(target_os = "linux")]
pub(super) fn save_phase_cleanup(root: &Path) -> Result<Value, String> {
    observe(root).map_err(|e| e.to_string())
}
#[cfg(target_os = "linux")]
fn observe(root: &Path) -> Result<Value, BundleError> {
    let path = root.join("phase.kinewright");
    let document = Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let initial = Core::spawn(document.clone()).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document())?;
    let inputs = owner.empty_inputs()?.reference.clone();
    owner.save(&initial, &inputs)?;
    drop(owner);
    let limits = StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    };
    let mut owner = LoadedBundle::read_with_limits_probe(&path, limits)?.into_writer()?;
    let core = Core::spawn_at(document, TimelineRevision(u64::MAX - 40))
        .map_err(|e| BundleError::Corrupt(e.to_string()))?;
    for i in 0_u64..40 {
        if !matches!(
            core.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: i.is_multiple_of(2)
            }))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?,
            Event::DocumentChanged { .. }
        ) {
            return Err(BundleError::Corrupt(
                "phase control command rejected".into(),
            ));
        }
    }
    let outcome = BundleCommit::run(&mut owner, &core, &inputs);
    let phase = owner.publication_state();
    let control = LoadedBundle::read(&path)?;
    let control_reopened = control.admission().inspection_document()
        == Some(
            &*core
                .export_history()
                .map_err(|e| BundleError::Corrupt(e.to_string()))?
                .document,
        );
    let phase_preserved = phase == PublicationState::Committed
        && matches!(
            outcome,
            CommitOutcome::CommittedWithError(BundleError::HistoryCommitInvariant)
        );
    let cleanup = root.join("cleanup.kinewright");
    let cleanup_core =
        Core::spawn(Document::default()).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    let mut session = SessionStore::create(root, StorageLimits::default())?;
    let inputs = session.admit_inputs(&OwnedInputManifest {
        project_id: None,
        session_id: Some(session.session_id().clone()),
        owner_epoch: session.store().owner_epoch(),
        entries: vec![],
    })?;
    let mut copy = io_observer::fail_once("root_file_sync", || {
        io_observer::fail_path("root_cleanup_unlink", &cleanup, || {
            session.first_save(&cleanup, &cleanup_core, &inputs.reference)
        })
    })?;
    let cleanup_classified = matches!(copy.status(), CopyStatus::CleanupPending { .. });
    let cleanup_diagnostic = format!("{:?}", copy.status());
    copy.reconcile()?;
    Ok(
        json!({"backend":"linux-ordinary-save", "control_reopened":control_reopened,
        "phase_preserved":phase_preserved, "phase":super::report::publication_phase(phase),
        "outcome":format!("{outcome:?}"), "cleanup_classified":cleanup_classified,
        "cleanup_diagnostic":cleanup_diagnostic, "cleanup_root_absent":!cleanup.exists()}),
    )
}

/// The baseline observes retained-handle editability; it never pretends to call a seal.
pub(super) fn core_seal(revised: bool) -> Result<Value, String> {
    struct ObserverProof;
    let core = Core::spawn(Document::default()).map_err(|e| e.to_string())?;
    let retained = core.clone();
    let operation = |id| Operation::AddTrack {
        track: Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        },
    };
    let control = matches!(
        retained
            .request(Command::Do(operation(1)))
            .map_err(|e| e.to_string())?,
        Event::DocumentChanged { .. }
    );
    if revised {
        let attachment = core
            .history_save_owner(&ObserverProof)
            .map_err(|e| e.to_string())?;
        let cut = attachment
            .freeze(&ObserverProof)
            .map_err(|e| e.to_string())?;
        attachment
            .seal(&ObserverProof, &cut)
            .map_err(|e| e.to_string())?;
    }
    let before = core.export_history().map_err(|e| e.to_string())?;
    let result = retained
        .request(Command::Do(operation(2)))
        .map_err(|e| e.to_string())?;
    let unchanged = core.export_history().map_err(|e| e.to_string())? == before;
    Ok(
        json!({"backend":"production-core", "control_edit_accepted":control,
        "sealed_refuses_mutation":matches!(result, Event::MutationRefused { reason: kinewright_core::CoreMutationRefusal::Sealed, .. }) && unchanged,
        "continued_edit_accepted":matches!(result, Event::DocumentChanged { .. }),
        "surface":if revised { "sealed actor" } else { "retained clone without seal" }}),
    )
}
