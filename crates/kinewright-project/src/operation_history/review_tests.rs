use super::tests::{approval, document, origin, remove};
use super::*;
use kinewright_core::{
    Command, Document, LutAssetId, Marker, MarkerId, Operation, TimeCode, TrackId,
    undo::RetentionPolicy,
};

fn owner() -> OwnerHistory {
    owner_in(&super::tests::story_pool())
}
fn owner_in(pool: &HistoryPool) -> OwnerHistory {
    OwnerHistory::new(
        pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap()
}

#[test]
fn approval_references_are_validated_before_commit() {
    let mut owner = owner();
    let before = serde_json::to_vec(&owner.capture().unwrap().data).unwrap();
    let mut consent = approval();
    consent.reference.sha256 = "bad".into();
    let result = owner.execute(remove(), origin(2), vec![], Some(consent));
    assert!(
        matches!(&result, Err(Error::InvalidLedger)),
        "MALFORMED_APPROVAL_REFERENCE_REFUSED outcome={result:?} reopen={:?}",
        owner.capture().unwrap().validate().err()
    );
    assert_eq!(
        serde_json::to_vec(&owner.capture().unwrap().data).unwrap(),
        before,
        "MALFORMED_APPROVAL_REFERENCE_REFUSAL_ATOMIC"
    );
    super::roundtrip_tests::saved_open(&owner);
}

fn state(owner: &OwnerHistory) -> Vec<u8> {
    serde_json::to_vec(&owner.capture().unwrap().data).unwrap()
}
fn marker(id: u64) -> Command {
    Command::Do(Operation::AddMarker {
        marker: Marker {
            id: MarkerId(id),
            position: TimeCode(0),
            label: format!("review {id}"),
            color_token: 0,
        },
    })
}

#[test]
fn approval_reference_collisions_are_refused_in_both_admission_orders() {
    for transition_first in [true, false] {
        let mut owner = owner();
        let reference = approval().reference;
        owner
            .execute(
                remove(),
                origin(2),
                vec![],
                (!transition_first).then(approval),
            )
            .unwrap();
        if transition_first {
            owner
                .approve_transition(
                    reference,
                    Direction::Undo,
                    owner.transition_target(Direction::Undo).unwrap(),
                )
                .unwrap();
        }
        let before = state(&owner);
        let result = if transition_first {
            owner.execute(remove(), origin(3), vec![], Some(approval()))
        } else {
            owner.approve_transition(
                approval().reference,
                Direction::Undo,
                owner.transition_target(Direction::Undo).unwrap(),
            )
        };
        assert_eq!(
            result,
            Err(Error::InvalidLedger),
            "CROSS_KIND_APPROVAL_REFERENCE_REFUSED_AT_ADMISSION order={transition_first}"
        );
        assert_eq!(
            state(&owner),
            before,
            "CROSS_KIND_APPROVAL_ADMISSION_REFUSAL_ATOMIC"
        );
        super::roundtrip_tests::saved_open(&owner);
    }
}

#[test]
fn one_reference_cannot_name_two_do_approvals() {
    let mut owner = owner();
    owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap();
    owner
        .execute(
            Command::Do(Operation::AddTrack {
                track: document().tracks.remove(0),
            }),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    let before = state(&owner);
    assert_eq!(
        owner.execute(remove(), origin(4), vec![], Some(approval())),
        Err(Error::InvalidLedger),
        "DO_APPROVAL_REFERENCE_IS_UNIQUE"
    );
    assert_eq!(
        state(&owner),
        before,
        "DUPLICATE_DO_REFERENCE_REFUSAL_ATOMIC"
    );
    super::roundtrip_tests::saved_open(&owner);
}

#[test]
fn approval_reference_collisions_are_refused_in_both_install_orders() {
    for transition_in_destination in [true, false] {
        let pool = super::tests::story_pool();
        let mut destination = owner_in(&pool);
        let mut source = owner_in(&pool);
        for (history, transition) in [
            (&mut destination, transition_in_destination),
            (&mut source, !transition_in_destination),
        ] {
            history
                .execute(remove(), origin(2), vec![], (!transition).then(approval))
                .unwrap();
            if transition {
                history
                    .approve_transition(
                        approval().reference,
                        Direction::Undo,
                        history.transition_target(Direction::Undo).unwrap(),
                    )
                    .unwrap();
            }
        }
        let selected = source.capture().unwrap().validate().unwrap();
        let before = state(&destination);
        assert_eq!(
            destination.install_snapshot(selected),
            Err(Error::InvalidLedger),
            "CROSS_KIND_APPROVAL_REFERENCE_REFUSED_AT_INSTALL order={transition_in_destination}"
        );
        assert_eq!(
            state(&destination),
            before,
            "CROSS_KIND_APPROVAL_INSTALL_REFUSAL_ATOMIC"
        );
        super::roundtrip_tests::saved_open(&destination);
    }
}

#[test]
fn typed_and_encoded_ledgers_refuse_duplicate_approval_references() {
    let mut owner = owner();
    owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap();
    owner
        .execute(
            Command::Do(Operation::AddTrack {
                track: document().tracks.remove(0),
            }),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    let mut reference = approval().reference;
    reference.sha256 = "b".repeat(64);
    owner
        .approve_transition(
            reference,
            Direction::Undo,
            owner.transition_target(Direction::Undo).unwrap(),
        )
        .unwrap();
    let before = state(&owner);
    let mut capture = owner.capture().unwrap();
    capture.data.ledger.transitions[0].id = approval().reference;
    let bytes = serde_json::to_vec(&capture.data).unwrap();
    assert!(
        serde_json::from_slice::<super::snapshot::SnapshotData>(&bytes).is_err(),
        "ENCODED_DUPLICATE_APPROVAL_REFERENCE_REFUSED"
    );
    assert_eq!(
        capture.validate().err(),
        Some(Error::InvalidLedger),
        "TYPED_DUPLICATE_APPROVAL_REFERENCE_REFUSED"
    );
    assert_eq!(
        state(&owner),
        before,
        "TYPED_DUPLICATE_REFERENCE_DESTINATION_UNCHANGED"
    );
}

#[test]
fn duplicate_approval_targets_are_refused_atomically() {
    let mut owner = owner();
    let before = state(&owner);
    let mut consent = approval();
    consent.targets.push(RemovedTarget::Track(TrackId(81)));
    assert_eq!(
        owner.execute(remove(), origin(2), vec![], Some(consent)),
        Err(Error::InvalidLedger),
        "DUPLICATE_APPROVAL_TARGETS_REFUSED"
    );
    assert_eq!(
        state(&owner),
        before,
        "DUPLICATE_APPROVAL_TARGETS_REFUSAL_ATOMIC"
    );
}

#[test]
fn typed_origin_key_must_match_its_command() {
    let mut owner = owner();
    owner
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    let before = state(&owner);
    let mut capture = owner.capture().unwrap();
    capture.data.origins[0].key.position += 1;
    assert_eq!(
        capture.validate().err(),
        Some(Error::InvalidOrigin),
        "TYPED_ORIGIN_KEY_MUST_MATCH_COMMAND"
    );
    assert_eq!(
        state(&owner),
        before,
        "ORIGIN_KEY_REFUSAL_DESTINATION_UNCHANGED"
    );
}

#[test]
fn cross_pool_install_refuses_without_mutation_or_leaked_lease() {
    let first = super::tests::story_pool();
    let second = super::tests::story_pool();
    let make = |pool: &HistoryPool| {
        OwnerHistory::new(
            pool,
            Role::Task,
            RetentionPolicy::NeverFold,
            &origin(1),
            || Ok(document()),
        )
        .unwrap()
    };
    let mut destination = make(&first);
    let source = make(&second);
    let first_before = first.usage(Role::Task).unwrap();
    let second_before = second.usage(Role::Task).unwrap();
    let before = state(&destination);
    let selected = source.capture().unwrap().validate().unwrap();
    assert_eq!(
        destination.install_snapshot(selected),
        Err(Error::Reservation),
        "CROSS_POOL_INSTALL_REFUSED"
    );
    assert_eq!(
        state(&destination),
        before,
        "CROSS_POOL_INSTALL_DESTINATION_UNCHANGED"
    );
    assert_eq!(
        first.usage(Role::Task).unwrap(),
        first_before,
        "CROSS_POOL_INSTALL_DESTINATION_LEASE_UNCHANGED"
    );
    assert_eq!(
        second.usage(Role::Task).unwrap(),
        second_before,
        "CROSS_POOL_REFUSAL_RETURNS_SOURCE_LEASE"
    );
}

#[test]
fn sibling_line_mid_step_transition_is_uncertain_and_reopenable() {
    let pool = super::tests::story_pool();
    let make = || {
        OwnerHistory::new(
            &pool,
            Role::Task,
            RetentionPolicy::NeverFold,
            &origin(1),
            || Ok(document()),
        )
        .unwrap()
    };
    let seed = make();
    let mut destination = make();
    let mut sibling = make();
    destination
        .install_snapshot(seed.capture().unwrap().validate().unwrap())
        .unwrap();
    sibling
        .install_snapshot(seed.capture().unwrap().validate().unwrap())
        .unwrap();
    for (i, locked) in [false, true].into_iter().enumerate() {
        let operation = Operation::SetTrackSyncLock {
            track: TrackId(81),
            locked,
        };
        destination
            .execute(
                Command::Do(operation.clone()),
                origin(i as u64 + 2),
                vec![],
                None,
            )
            .unwrap();
        sibling
            .execute(
                Command::DoBatchCoalesced {
                    operations: vec![operation],
                    coalesce_key: "sibling".into(),
                },
                origin(i as u64 + 2),
                vec![],
                None,
            )
            .unwrap();
    }
    let target = destination.transition_target(Direction::Undo).unwrap();
    destination
        .approve_transition(approval().reference, Direction::Undo, target)
        .unwrap();
    destination
        .install_snapshot(sibling.capture().unwrap().validate().unwrap())
        .expect("MID_STEP_SHAPE_INSTALL_IS_ALLOWED");
    assert_eq!(
        destination.transition_fate(&approval().reference).unwrap(),
        Some(TransitionFate::Uncertain(Uncertainty::ShapeMismatch)),
        "MID_STEP_SHAPE_IS_UNCERTAIN"
    );
    assert_eq!(
        destination.transition_counts().unwrap(),
        TransitionCounts {
            uncertain: 1,
            ..TransitionCounts::default()
        },
        "MID_STEP_UNCERTAINTY_COUNTS_ONLY"
    );
    super::roundtrip_tests::saved_open(&destination);
    let mut malformed = destination.capture().unwrap();
    malformed.data.origins.clear();
    assert_eq!(
        malformed.validate().err(),
        Some(Error::InvalidOrigin),
        "MID_STEP_MISSING_ORIGIN_STILL_REFUSED"
    );
}

fn resource_fixture() -> (Document, Operation, Operation) {
    let mut cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    (
        serde_json::from_value(cases["AddTitle"]["document"].take()).unwrap(),
        serde_json::from_value(cases["AddLutAsset"]["operation"].take()).unwrap(),
        serde_json::from_value(cases["AddTitle"]["operation"].take()).unwrap(),
    )
}
fn lut_reference(operation: &Operation) -> Resource {
    let Operation::AddLutAsset { asset } = operation else {
        unreachable!()
    };
    Resource::Lut {
        sha256: asset.sha256.clone(),
        byte_len: asset.byte_len,
    }
}

#[test]
fn shared_lut_and_font_survive_one_contributor_and_leave_after_the_last() {
    let (document, lut, title) = resource_fixture();
    let required = [lut_reference(&lut), Resource::TitleFont];
    let mut owner = OwnerHistory::new(
        &super::tests::story_pool(),
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document),
    )
    .unwrap();
    // These are two independent commands sharing their resource identities.
    let mut second_lut = lut.clone();
    let Operation::AddLutAsset { asset } = &mut second_lut else {
        unreachable!()
    };
    asset.id = LutAssetId(2);
    let mut second_title = title.clone();
    let Operation::AddTitle { at, .. } = &mut second_title else {
        unreachable!()
    };
    *at = TimeCode(30);
    owner
        .execute(Command::DoBatch(vec![lut, title]), origin(2), vec![], None)
        .unwrap();
    owner
        .execute(
            Command::DoBatch(vec![second_lut, second_title]),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    owner
        .execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    owner.execute(marker(1), origin(5), vec![], None).unwrap();
    for resource in &required {
        assert!(
            owner.resources().contains(resource),
            "SHARED_PAYLOAD_RESOURCE_SURVIVES_ONE_DISCARD {resource:?}"
        );
    }
    owner
        .execute(Command::Undo, origin(6), vec![], None)
        .unwrap();
    owner
        .execute(Command::Undo, origin(7), vec![], None)
        .unwrap();
    owner.execute(marker(2), origin(8), vec![], None).unwrap();
    for resource in &required {
        assert!(
            !owner.resources().contains(resource),
            "LAST_PAYLOAD_RESOURCE_RELEASED_ON_DISCARD {resource:?}"
        );
        assert!(
            !owner.resources.counts.contains_key(resource),
            "LAST_PAYLOAD_RESOURCE_COUNT_REMOVED_ON_DISCARD"
        );
    }
    super::roundtrip_tests::saved_open(&owner);
}

#[test]
fn folded_resource_counts_transfer_and_unreachable_baseline_resources_leave() {
    let (document, lut, title) = resource_fixture();
    let required = [lut_reference(&lut), Resource::TitleFont];
    let mut owner = OwnerHistory::measured(
        &super::tests::story_pool(),
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        kinewright_core::undo::RoleBounds {
            step_cap: 1,
            ..Role::Ordinary.bounds()
        },
        || Ok(document),
    )
    .unwrap();
    owner
        .execute(Command::DoBatch(vec![lut, title]), origin(2), vec![], None)
        .unwrap();
    let clip = owner.history().document().tracks[0].clips[0].id;
    owner
        .execute(
            Command::DoBatch(vec![
                Operation::RemoveLutAsset {
                    lut_asset: LutAssetId(1),
                },
                Operation::DeleteClip { clip },
            ]),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    for resource in &required {
        assert!(
            owner.resources().contains(resource),
            "FOLDED_BASELINE_STILL_OWNS_UNDO_RESOURCE"
        );
        assert!(
            !owner.resources.counts.contains_key(resource),
            "FOLDED_PAYLOAD_CONTRIBUTION_RELEASED"
        );
    }
    owner.execute(marker(1), origin(4), vec![], None).unwrap();
    for resource in &required {
        assert!(
            !owner.resources().contains(resource),
            "UNREACHABLE_RESOURCE_RELEASED_AT_FOLD {resource:?}"
        );
        assert!(
            !owner.resources.counts.contains_key(resource),
            "UNREACHABLE_RESOURCE_COUNT_REMOVED_AT_FOLD"
        );
    }
    super::roundtrip_tests::saved_open(&owner);
}
