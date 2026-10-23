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

/// Ordinary S1 save/reopen controls precede observations of C3's new lifecycle surface.
#[cfg(target_os = "linux")]
pub(super) fn rescue_owners(root: &Path, revised: bool, slots: bool) -> Result<Value, String> {
    observe_rescue_owners(root, revised, slots).map_err(|e| e.to_string())
}
#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    reason = "one paired ordinary-save control and lifecycle-owner observation"
)]
fn observe_rescue_owners(root: &Path, revised: bool, slots: bool) -> Result<Value, BundleError> {
    use kinewright_project::{
        bundle_rescue::MissingOwnedUse,
        editing_session::{Ja1EditingSession, RescueSlots},
    };
    use std::io::Read as _;
    let parent = root.join("a");
    std::fs::create_dir(&parent)?;
    let path = parent.join(if slots {
        "slots.kinewright"
    } else {
        "source.kinewright"
    });
    let core = Core::spawn(Document::default()).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    if !revised {
        let mut source = SessionStore::create(root, StorageLimits::default())?;
        let inputs = source
            .admit_inputs(&OwnedInputManifest {
                project_id: None,
                session_id: Some(source.session_id().clone()),
                owner_epoch: source.store().owner_epoch(),
                entries: vec![],
            })?
            .reference
            .clone();
        let copy = source.first_save(&path, &core, &inputs)?;
        let control = matches!(copy.status(), CopyStatus::Committed(_))
            && LoadedBundle::read(&path)?.admission().inspection_document()
                == Some(&Document::default());
        return Ok(
            json!({"control_reopened":control,"surface":"pending_baseline_witness","rescue_publication":false}),
        );
    }
    let mut session = Ja1EditingSession::new(root, &core, StorageLimits::default())?;
    let source = session
        .source()
        .ok_or(BundleError::HistoryBarrierInvariant)?;
    let objects = source.store().manifest()?.objects;
    let reference = objects
        .first()
        .ok_or(BundleError::HistoryBarrierInvariant)?
        .clone();
    let reader = source
        .store()
        .admit_retained_reads(&[(reference.clone(), MissingOwnedUse::Current)])?;
    io_observer::fail_once("root_parent_sync", || {
        session.first_save(&path, StorageLimits::default())
    })?;
    let hidden = root.join("a-held");
    std::fs::rename(&parent, &hidden)?;
    let preparation = (|| {
        let offer = session
            .probe_active()?
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let capture = session
            .prepare_rescue(&offer)?
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        let consent = capture.consent();
        let destination = session.inspect_rescue_destination(&root.join("b.kinewright"))?;
        session.adopt_attempt_probe(capture, &consent, &destination, None)
    })();
    std::fs::rename(&hidden, &parent)?;
    preparation?;
    session.resume_probe()?;
    session.reconcile_active()?;
    if slots {
        let mut ledger = RescueSlots::default();
        for attempt in 1..=3 {
            let tag = kinewright_core::HistorySaveTag {
                destination: path.clone(),
                parent_device: 1,
                parent_inode: 1,
                root_sha256: format!("{attempt:064x}"),
                previous_root_sha256: None,
            };
            let reservation = ledger.begin(tag)?;
            let unknown = (attempt == 3).then(|| kinewright_core::HistorySaveTag {
                destination: root.join("modeled-unknown.kinewright"),
                parent_device: 1,
                parent_inode: 1,
                root_sha256: format!("{:064x}", 4),
                previous_root_sha256: None,
            });
            ledger.finish(reservation, unknown)?;
        }
        session.set_slots_probe(ledger);
    }
    let mut bytes = vec![];
    reader.open(&reference)?.read_to_end(&mut bytes)?;
    drop(reader);
    session.save_active()?;
    let usage = session
        .source()
        .ok_or(BundleError::HistoryBarrierInvariant)?
        .store()
        .usage()?;
    let exact = bytes.len() as u64 == reference.byte_length
        && kinewright_media::sha256_bytes(&bytes) == reference.sha256;
    let retained = usage.physical_bytes >= reference.byte_length && usage.reserved_copy_bytes > 0;
    let held = kinewright_project::namespace::ProjectNamespace::acquire(&path).is_err();
    Ok(
        json!({"control_reopened":LoadedBundle::read(&path)?.admission().inspection_document() == Some(&Document::default()), "lifecycle_holds":exact && retained && held, "ordinary_save_at_four_slots":session.rescue_slots() == 4 && !session.dirty()?, "rescue_publication":false, "slot_model":slots}),
    )
}

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
