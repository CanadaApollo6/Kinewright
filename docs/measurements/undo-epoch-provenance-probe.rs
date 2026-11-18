#[test]
fn probe_equal_records_erase_folded_epoch_membership() {
    let initial = document();
    let pool = HistoryPool::new(PoolLimits::default());
    let bounds = kinewright_core::undo::RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    };
    let mut records = Vec::new();
    for keep_epoch_one in [true, false] {
        let mut history = OwnerHistory::measured(
            &pool,
            Role::Ordinary,
            RetentionPolicy::Fold,
            &origin(1),
            bounds,
            || Ok(initial.clone()),
        )
        .unwrap();
        let first = history.capture().unwrap();
        let original = history.capture().unwrap();
        history
            .install_snapshot(first.validate_bounded(bounds).unwrap())
            .unwrap();
        let old = history
            .execute(remove(), origin(2), vec![], None)
            .unwrap()
            .accepted;
        assert_eq!(
            old,
            kinewright_core::undo::CommandKey {
                epoch: 1,
                position: 0
            }
        );
        if keep_epoch_one {
            let folded = history
                .execute(marker(99), origin(3), vec![], None)
                .unwrap();
            assert!(
                folded.folded.iter().any(|command| command.key == old),
                "OLD_EPOCH_ACTUALLY_FOLDED"
            );
            drop(original);
            let selected = history.capture().unwrap().validate_bounded(bounds).unwrap();
            history.install_snapshot(selected).unwrap();
        } else {
            history
                .install_snapshot(original.validate_bounded(bounds).unwrap())
                .unwrap();
            assert!(
                history.history().undo().is_empty() && history.history().redo().is_empty(),
                "OLD_EPOCH_REPLACED_BY_EMPTY_HISTORY"
            );
            assert_eq!(history.history().document().tracks.len(), 1);
            let newer = history
                .execute(remove(), origin(2), vec![], None)
                .unwrap()
                .accepted;
            assert_eq!(
                newer,
                kinewright_core::undo::CommandKey {
                    epoch: 2,
                    position: 0
                }
            );
            let folded = history
                .execute(marker(99), origin(3), vec![], None)
                .unwrap();
            assert!(
                folded.folded.iter().all(|command| command.key.epoch == 2),
                "ONLY_NEW_EPOCH_FOLDED_AFTER_REPLACEMENT"
            );
        }
        history.execute(marker(2), origin(4), vec![], None).unwrap();
        history.execute(marker(3), origin(5), vec![], None).unwrap();
        let snapshot = history.capture().unwrap();
        println!(
            "EPOCH_PROVENANCE keep_old_epoch={keep_epoch_one} current={:?} frontier={:?} retained_epochs={:?}",
            history.history().position(),
            snapshot.data.fold_frontier,
            snapshot
                .data
                .history
                .undo
                .iter()
                .flat_map(|step| step.commands())
                .map(|command| command.id.epoch)
                .collect::<Vec<_>>()
        );
        records.push(serde_json::to_vec(&snapshot.data).unwrap());
    }
    assert_eq!(
        records[0], records[1],
        "SERIALIZED_RECORD_HAS_NO_OLD_EPOCH_MEMBERSHIP_INFORMATION"
    );
    println!(
        "EPOCH_PROVENANCE byte_identical=true bytes={}",
        records[0].len()
    );
}
