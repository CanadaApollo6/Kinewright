#[cfg(target_os = "linux")]
use kinewright_project::{
    bundle::LoadedBundle, bundle_copy::CopyStatus, storage_budget::StorageLimits,
};
use std::path::Path;
#[path = "../../kinewright-project/tests/support/ja_s2_c6_admission.rs"]
mod admission;
fn durable_test_root() -> &'static Path {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn rescue_reopen_recopy_retention_stress() {
    admission::matrix(|path| {
        kinewright_agent::LocalEditingSession::open(path)?
            .save()
            .map(|_| ())
    });
    #[cfg(target_os = "linux")]
    for round in 0..4 {
        use kinewright_project::{editing_session::Ja1EditingSession, io_observer};
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            durable_test_root(),
            "c6-agent-stress",
        );
        let drive = temp.path("drive");
        std::fs::create_dir(&drive).unwrap();
        let path = admission::fixture(&drive, "rescue-copy", "supported");
        let mut agent = kinewright_agent::LocalEditingSession::open(&path).unwrap();
        agent.save().unwrap();
        assert!(io_observer::fail_once("root_parent_sync", || agent.save()).is_err());
        // Keep the real pinned directory handles, while the original locator is unavailable.
        let hidden = temp.path("hidden-drive");
        std::fs::rename(&drive, &hidden).unwrap();
        let owner = agent.owner();
        let offer = owner.probe_active().unwrap().unwrap();
        let capture = owner.prepare_rescue(&offer).unwrap().unwrap();
        let selection = owner
            .preview_rescue(capture, StorageLimits::default())
            .unwrap();
        let mut consent = selection.consent();
        if selection.removed_undo() > 0 {
            selection.consent_trim(&mut consent).unwrap();
        }
        if !selection.omissions().is_empty() {
            selection.consent_omission(&mut consent).unwrap();
        }
        let destination = owner
            .inspect_rescue_destination(&temp.path("b.kinewright"))
            .unwrap();
        assert!(
            owner
                .rescue(selection, &consent, &destination)
                .unwrap()
                .activated
        );
        let expected = owner.core().export_history().unwrap();
        let slots = owner.rescue_slots();
        for _ in 0..8 {
            agent.save().unwrap();
        }
        let owner = agent.owner();
        assert_eq!(owner.rescue_slots(), slots);
        assert!(matches!(
            owner
                .save_as(&temp.path("c.kinewright"), StorageLimits::default())
                .unwrap(),
            CopyStatus::Committed(_)
        ));
        assert_eq!(owner.core().export_history().unwrap(), expected);
        std::fs::rename(hidden, drive).unwrap();
        agent.owner().reconcile_retained_destination(&path).unwrap();
        assert!(Ja1EditingSession::open(&path).is_err());
        drop(agent);
        let mut reopened = Ja1EditingSession::open(&temp.path("c.kinewright")).unwrap();
        assert_eq!(
            reopened.core().export_history().unwrap(),
            expected,
            "round {round}"
        );
        reopened.save_active().unwrap();
        drop(reopened);
        LoadedBundle::read(&temp.path("b.kinewright")).unwrap();
    }
}
