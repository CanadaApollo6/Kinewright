use crate::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_records::RecordKind,
    editing_session::Ja1EditingSession,
    run_admission::SPECIALIST_BASES,
    storage_budget::StorageLimits,
};
use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "valid branch history reaches both ordinary and raw specialist-version boundaries, including an identical retry"
)]
fn run_limit_cleanup_reserves_specialist_versions_and_reuses_a_full_raw_slot() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "cleanup-specialist-slots",
    );
    let path = temp.path("project.kinewright");
    let initial = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        }],
        ..Default::default()
    })
    .unwrap();
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&initial, &inputs).unwrap();
    let history = initial.export_history().unwrap();
    drop(bundle);
    drop(initial);
    let limits = StorageLimits {
        versions: SPECIALIST_BASES + 4,
        ..Default::default()
    };
    let raw_versions = limits.versions;
    let bundle = LoadedBundle::read(&path)
        .unwrap()
        .into_writer_with_limits_probe(limits)
        .unwrap();
    let core = Core::spawn(std::sync::Arc::clone(&history.baseline)).unwrap();
    core.restore_history(history).unwrap();
    let mut owner = Ja1EditingSession::from_bundle(bundle, &core, inputs).unwrap();
    owner.save_active().unwrap();
    let mut branch = owner
        .fork_durable_branch(
            "quota witness",
            crate::editing_session::DurableBranchRole::Session,
        )
        .unwrap();
    owner.persist_durable_branch(&mut branch).unwrap();
    let count = |owner: &Ja1EditingSession| {
        owner
            .active_bundle()
            .unwrap()
            .evidence
            .iter()
            .filter(|reference| reference.record_kind == RecordKind::SpecialistBase)
            .count() as u64
    };
    let edit = |core: &Core| {
        let locked = core.export_history().unwrap().document.tracks[0].sync_lock;
        assert!(matches!(
            core.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: !locked
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    };
    while count(&owner) < raw_versions - SPECIALIST_BASES {
        edit(branch.core());
        owner.persist_durable_branch(&mut branch).unwrap();
    }
    // Unit-only control: no H1 run is forged. The agent witness exercises the
    // actual proof-bearing cleanup scope; here the quota itself is isolated.
    owner.store().unwrap().set_run_budget(true);
    edit(branch.core());
    let dirty = branch.core().export_history().unwrap();
    assert!(matches!(
        owner.persist_durable_branch(&mut branch),
        Err(BundleError::RunLimit)
    ));
    assert!(owner.store().unwrap().run_limit_reached());
    assert_eq!(branch.core().export_history().unwrap(), dirty);
    assert_eq!(count(&owner), raw_versions - SPECIALIST_BASES);
    owner.store().unwrap().set_run_budget(false);
    owner.persist_durable_branch(&mut branch).unwrap();
    while count(&owner) < raw_versions {
        edit(branch.core());
        owner.persist_durable_branch(&mut branch).unwrap();
    }
    let full = branch.core().export_history().unwrap();
    owner
        .persist_durable_branch(&mut branch)
        .expect("C3_FULL_RAW_IDENTICAL_RETRY");
    assert_eq!(branch.core().export_history().unwrap(), full);
    assert_eq!(count(&owner), raw_versions);
    edit(branch.core());
    assert!(matches!(
        owner.persist_durable_branch(&mut branch),
        Err(BundleError::Capacity("retained specialist versions"))
    ));
    assert_eq!(count(&owner), raw_versions);
}
