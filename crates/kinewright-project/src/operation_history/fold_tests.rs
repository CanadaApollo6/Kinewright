use super::tests::{document, origin};
use super::*;
use kinewright_core::{
    Command, Document, Marker, MarkerId, Operation, TimeCode, TrackId, undo::RoleBounds,
};
#[path = "../../../kinewright-core/tests/support/undo_fixture.rs"]
mod fixture;
fn edit(track: TrackId, n: u64) -> Command {
    Command::Do(Operation::SetTrackSyncLock {
        track,
        locked: n % 2 == 1,
    })
}
fn marker(n: u64) -> Command {
    Command::Do(Operation::AddMarker {
        marker: Marker {
            id: MarkerId(n),
            position: TimeCode(0),
            label: format!("fold {n}"),
            color_token: 0,
        },
    })
}
fn make(doc: Document, bounds: RoleBounds) -> OwnerHistory {
    OwnerHistory::measured(
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
        Role::Ordinary.policy(),
        &origin(1),
        bounds,
        || Ok(doc),
    )
    .unwrap()
}
fn check(owner: &OwnerHistory) {
    owner.fold.as_ref().unwrap().assert_matches(
        &owner.seeds,
        owner.history().baseline(),
        &owner.resources().baseline,
    );
    super::roundtrip_tests::saved_open(owner);
}
fn no_aggregate(c: OwnerCounters) {
    assert_eq!(c.document_scans, 0, "FOLD_HAS_NO_OWNER_DOCUMENT_SCAN");
    assert_eq!(c.exact_measurements, 0, "FOLD_HAS_NO_AGGREGATE_MEASUREMENT");
    assert_eq!(c.seed_map_copies, 0, "FOLD_DOES_NOT_COPY_SEEDS");
    assert_eq!(
        c.seed_capacity_scans + c.seed_prune_scans + c.seed_verify_scans,
        0,
        "FOLD_DOES_NOT_SCAN_SEEDS"
    );
    assert_eq!(
        c.resource_document_scans, 0,
        "FOLD_DOES_NOT_SCAN_BASELINE_RESOURCES"
    );
    assert_eq!(c.lineage_encodings, 0, "FOLD_DOES_NOT_ENCODE_SEED_MAP");
    assert_eq!(
        c.owner_identity_hashes, 0,
        "FOLD_DOES_NOT_HASH_OWNER_RECORD"
    );
}
#[test]
fn a_fold_touches_only_its_step_with_unrelated_seeds_resources_and_approvals() {
    for large in [false, true] {
        let mut doc = if large { fixture::heavy() } else { document() };
        let track = doc.tracks[0].id;
        if large {
            let cases: serde_json::Value = serde_json::from_str(include_str!(
                "../../../kinewright-core/tests/support/history_operation_cases.json"
            ))
            .unwrap();
            let asset: kinewright_core::LutAsset = serde_json::from_value(
                cases["AddLutAsset"]["operation"]["AddLutAsset"]["asset"].clone(),
            )
            .unwrap();
            for id in 1..=100 {
                let mut asset = asset.clone();
                asset.id = kinewright_core::LutAssetId(id);
                asset.sha256 = format!("{id:064x}");
                doc.lut_assets.push(asset);
            }
        }
        let mut owner = make(
            doc,
            RoleBounds {
                operation_bytes: 500,
                ..Role::Ordinary.bounds()
            },
        );
        if large {
            for n in 0..128 {
                owner
                    .execute(edit(track, n), origin(2 + n * 3), vec![], None)
                    .unwrap();
                let id = crate::bundle_records::ObjectRef {
                    sha256: format!("{n:064x}"),
                    byte_length: 1,
                    record_kind: crate::bundle_records::RecordKind::OwnedEvidence,
                };
                owner
                    .approve_transition(
                        id,
                        Direction::Undo,
                        owner.transition_target(Direction::Undo).unwrap(),
                    )
                    .unwrap();
                owner
                    .execute(Command::Redo, origin(3 + n * 3), vec![], None)
                    .unwrap();
            }
        }
        for n in 0..32 {
            owner
                .execute(edit(track, n), origin(1000 + n), vec![], None)
                .unwrap();
        }
        let before = OwnerCounters::read();
        let effects = owner
            .execute(edit(track, 32), origin(2000), vec![], None)
            .unwrap();
        let c = OwnerCounters::read().since(before);
        assert_eq!(effects.folded.len(), 1, "COUNTER_GUARD_FOLDED_ONE_COMMAND");
        no_aggregate(c);
        assert_eq!(c.folded_commands, 1, "FOLD_COMMAND_WORK_IS_ONE_STEP");
        assert_eq!(
            c.folded_seed_ids
                + c.folded_ancestry
                + c.folded_resource_ids
                + c.folded_payload_references,
            0,
            "UNRELATED_BASELINE_METADATA_IS_NOT_VISITED"
        );
        assert_eq!(c.ledger_entries, 0, "UNRELATED_APPROVALS_ARE_NOT_VISITED");
        assert!(c.origin_lookups <= 4, "ONLY_FOLDED_ORIGINS_ARE_LOOKED_UP");
        assert_eq!(c.payload_encodings, 1, "COMMAND_PAYLOAD_IS_ENCODED_ONCE");
        check(&owner);
    }
}
fn split_case() -> (Document, Operation) {
    let mut cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    (
        serde_json::from_value(cases["SplitClip"]["document"].take()).unwrap(),
        serde_json::from_value(cases["SplitClip"]["operation"].take()).unwrap(),
    )
}
#[test]
fn fold_reclaims_only_dead_ancestry_and_frees_its_empty_array() {
    let (doc, split) = split_case();
    let track = doc.tracks[0].id;
    let mut owner = make(
        doc,
        RoleBounds {
            step_cap: 1,
            ..Role::Ordinary.bounds()
        },
    );
    owner
        .execute(Command::Do(split), origin(2), vec![], None)
        .unwrap();
    owner
        .execute(edit(track, 0), origin(3), vec![], None)
        .unwrap();
    check(&owner);
    assert!(!owner.seeds.split_ancestry.is_empty());
    let deletes = owner.history().document().tracks[0]
        .clips
        .iter()
        .map(|clip| Operation::DeleteClip { clip: clip.id })
        .collect();
    owner
        .execute(Command::DoBatch(deletes), origin(4), vec![], None)
        .unwrap();
    let before = OwnerCounters::read();
    owner
        .execute(edit(track, 1), origin(5), vec![], None)
        .unwrap();
    let c = OwnerCounters::read().since(before);
    no_aggregate(c);
    assert!(
        c.folded_ancestry > 0,
        "FOLD_RECLAIMS_CAUSALLY_DEAD_ANCESTORS"
    );
    assert!(
        owner.seeds.occurrences.is_empty() && owner.seeds.split_ancestry.is_empty(),
        "NO_UNREACHABLE_ANCESTRY_RETAINED"
    );
    assert_eq!(
        owner.seeds.split_ancestry.capacity(),
        0,
        "EMPTY_ANCESTRY_ARRAY_RELEASED_WITHOUT_COPY"
    );
    check(&owner);
}
#[test]
fn split_then_remove_inside_one_folded_batch_keeps_no_temporary_edges() {
    let (doc, split) = split_case();
    let track = doc.tracks[0].id;
    let mut owner = make(
        doc,
        RoleBounds {
            step_cap: 1,
            ..Role::Ordinary.bounds()
        },
    );
    owner
        .execute(
            Command::DoBatch(vec![split, Operation::RemoveTrack { track }]),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    owner.execute(marker(1), origin(3), vec![], None).unwrap();
    assert!(
        owner.seeds.occurrences.is_empty() && owner.seeds.split_ancestry.is_empty(),
        "TEMPORARY_SPLIT_EDGES_NEVER_REACH_BASELINE"
    );
    check(&owner);
}

#[test]
fn repeated_splits_keep_shared_ancestors_then_prune_only_the_dead_branch() {
    let (doc, _) = split_case();
    let track = doc.tracks[0].id;
    let mut owner = make(
        doc,
        RoleBounds {
            step_cap: 1,
            ..Role::Ordinary.bounds()
        },
    );
    for n in 1_u64..12 {
        let at = i64::try_from(n * 5).unwrap();
        let clip = owner.history().document().tracks[0]
            .clips
            .last()
            .unwrap()
            .id;
        owner
            .execute(
                Command::Do(Operation::SplitClip {
                    clip,
                    at: TimeCode(at),
                }),
                origin(2 + n * 5),
                vec![],
                None,
            )
            .unwrap();
        owner
            .execute(edit(track, n * 5), origin(100 + n * 5), vec![], None)
            .unwrap();
        check(&owner);
    }
    let first = owner.history().document().tracks[0].clips[0].id;
    let deletes = owner.history().document().tracks[0]
        .clips
        .iter()
        .skip(1)
        .map(|clip| Operation::DeleteClip { clip: clip.id })
        .collect();
    owner
        .execute(Command::DoBatch(deletes), origin(200), vec![], None)
        .unwrap();
    let before = OwnerCounters::read();
    owner
        .execute(edit(track, 0), origin(201), vec![], None)
        .unwrap();
    let c = OwnerCounters::read().since(before);
    no_aggregate(c);
    assert!(c.folded_ancestry > 0, "DEAD_SPLIT_BRANCH_IS_PRUNED");
    assert_eq!(
        owner.seeds.occurrences.len(),
        1,
        "SHARED_ROOT_LEAF_SURVIVES"
    );
    assert_eq!(
        owner.seeds.split_ancestry.len(),
        1,
        "ONLY_LIVE_LEAF_PARENT_REMAINS"
    );
    check(&owner);
    owner
        .execute(
            Command::Do(Operation::DeleteClip { clip: first }),
            origin(202),
            vec![],
            None,
        )
        .unwrap();
    owner
        .execute(edit(track, 1), origin(203), vec![], None)
        .unwrap();
    assert!(
        owner.seeds.split_ancestry.is_empty(),
        "FINAL_SHARED_ROOT_IS_RELEASED"
    );
    check(&owner);
}

#[test]
fn sparse_fold_scratch_refusal_does_not_mutate_the_source() {
    let (doc, split) = split_case();
    let owner = make(doc, Role::Ordinary.bounds());
    let before = owner.capture().unwrap().snapshot_identity().unwrap();
    let mut replay = kinewright_core::undo::History::new(
        split_case().0,
        Role::Ordinary.policy(),
        Role::Ordinary.bounds(),
    )
    .unwrap();
    let effects = replay.execute(Command::Do(split.clone())).unwrap();
    let state = owner.fold.as_ref().unwrap();
    let mut patch = super::fold::FoldPatch::new(state, 1, Role::Ordinary.envelope().workspace);
    assert_eq!(
        patch.apply(
            state,
            &owner.seeds,
            &origin(2),
            &[split],
            &effects.applied[0].identities
        ),
        Err(Error::Capacity("fold metadata")),
        "SPARSE_FOLD_SCRATCH_IS_ADMITTED_BEFORE_ALLOCATION"
    );
    assert_eq!(
        owner.capture().unwrap().snapshot_identity().unwrap(),
        before,
        "SPARSE_FOLD_REFUSAL_DOES_NOT_MUTATE_SOURCE"
    );
    check(&owner);
}

#[test]
fn origin_array_spare_capacity_stays_charged_after_redo_discard() {
    let doc = document();
    let track = doc.tracks[0].id;
    let mut owner = make(doc, Role::Ordinary.bounds());
    for n in 0..256 {
        owner
            .execute(edit(track, n), origin(2 + n), vec![], None)
            .unwrap();
    }
    for n in 0..255 {
        owner
            .execute(Command::Undo, origin(1000 + n), vec![], None)
            .unwrap();
    }
    owner
        .execute(edit(track, 256), origin(2000), vec![], None)
        .unwrap();
    assert_eq!(owner.origins.0.len(), 2);
    let array =
        (owner.origins.0.capacity() * std::mem::size_of::<super::owner::LiveOrigin>()) as u64;
    assert!(
        owner.owner_bytes() >= array,
        "OWNER_BOUND_COUNTS_ORIGIN_ARRAY_SPARE_CAPACITY: bound={} array={array}",
        owner.owner_bytes()
    );
    check(&owner);
}

#[test]
fn an_empty_fold_releases_the_origin_array_without_copying() {
    let doc = document();
    let track = doc.tracks[0].id;
    let mut owner = make(
        doc,
        RoleBounds {
            step_cap: 0,
            ..Role::Ordinary.bounds()
        },
    );
    owner
        .execute(edit(track, 0), origin(2), vec![], None)
        .unwrap();
    assert_eq!(
        owner.origins.0.capacity(),
        0,
        "EMPTY_ORIGIN_ARRAY_RELEASED_WITHOUT_COPY"
    );
    check(&owner);
}
