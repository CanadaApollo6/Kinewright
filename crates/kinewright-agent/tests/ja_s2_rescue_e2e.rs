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

#[cfg(target_os = "linux")]
fn recopy_oracle(
    core: &kinewright_project::operation_actor::Core,
    expected: kinewright_project::operation_history::OwnerSnapshot,
) -> kinewright_project::operation_history::OwnerSnapshot {
    let copied = core.export_history().unwrap();
    // Consume the old capture: the JSON oracle needs no full history lease.
    let mut expected_copy = serde_json::to_value(expected).unwrap();
    let actual_copy = serde_json::to_value(&copied).unwrap();
    let epoch = expected_copy["history"]["epoch"].as_u64().unwrap() + 1;
    expected_copy["history"]["epoch"] = serde_json::json!(epoch);
    assert_eq!(
        actual_copy, expected_copy,
        "RESCUE_RECOPY_CHANGES_ONLY_INSTALL_EPOCH"
    );
    assert_eq!(
        core.history_pool()
            .lease_counts(kinewright_project::operation_history::Role::Ordinary)
            .unwrap()
            .0,
        2,
        "RESCUE_RECONCILE_HAS_ONLY_LIVE_AND_ORACLE_CAPTURES"
    );
    copied
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
        let expected = recopy_oracle(owner.core(), expected);
        std::fs::rename(hidden, drive).unwrap();
        agent.owner().reconcile_retained_destination(&path).unwrap();
        assert!(
            Ja1EditingSession::open(
                &path,
                &kinewright_project::operation_history::HistoryPool::new(
                    kinewright_project::operation_history::PoolLimits::default()
                )
            )
            .is_err()
        );
        drop(agent);
        let mut reopened = Ja1EditingSession::open(
            &temp.path("c.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        assert_eq!(
            reopened.core().export_history().unwrap(),
            expected,
            "round {round}"
        );
        reopened.save_active().unwrap();
        drop(reopened);
        LoadedBundle::read(
            &temp.path("b.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
    }
}
