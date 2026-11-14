#[path = "support/history_commands.rs"]
mod commands;
#[path = "support/history_model.rs"]
mod model;
#[path = "support/history_oracle.rs"]
mod oracle;
use kinewright_core::undo::{
    CommandEffects, CommandKey, Effects, Error, History, RetentionPolicy, RoleBounds,
};
use kinewright_core::{Command, Document, Operation, OperationIdentityEffect};
use model::{Fate, Model, Policy, Refusal};
use std::collections::{BTreeMap, BTreeSet};

fn bounds(bytes: u64, checkpoints: usize) -> RoleBounds {
    RoleBounds {
        operation_bytes: bytes,
        step_cap: 3,
        checkpoint_documents: checkpoints,
        document_live_bytes: 16 << 20,
        document_bytes: 16 << 20,
        decoded_history_bytes: 64 << 20,
        replay_workspace: 128 << 20,
    }
}
fn key(id: (u64, u64)) -> CommandKey {
    CommandKey {
        epoch: id.0,
        position: id.1,
    }
}
fn assert_state(
    history: &History,
    model: &Model,
    fates: &BTreeMap<(u64, u64), Fate>,
    context: &str,
) {
    assert_eq!(**history.document(), model.document(), "DOCUMENT {context}");
    assert_eq!(**history.baseline(), model.baseline, "BASELINE {context}");
    assert_eq!(
        history.position(),
        key((model.epoch, model.position)),
        "POSITION_EPOCH {context}"
    );
    assert_eq!(
        history.charge().encoded,
        model.cost() as u64,
        "INCREMENTAL_CHARGE {context}"
    );
    let independent: u64 = history
        .undo()
        .iter()
        .chain(history.redo())
        .flat_map(kinewright_core::undo::Step::operations)
        .map(|op| serde_json::to_vec(op).unwrap().len() as u64)
        .sum();
    assert_eq!(
        history.charge().encoded,
        independent,
        "INDEPENDENT_CHARGE {context}"
    );
    assert_eq!(
        history.applied_delta().cloned().collect::<Vec<_>>(),
        model.applied_operations(),
        "APPLIED_DELTA {context}"
    );
    assert_eq!(fates, &model.fates, "EFFECT_FATES {context}");
    for (observed, expected) in [
        (history.undo().iter().collect::<Vec<_>>(), &model.undo),
        (history.redo().iter().collect(), &model.redo),
    ] {
        assert_eq!(observed.len(), expected.len(), "STACK_DEPTH {context}");
        for (observed, expected) in observed.iter().zip(expected) {
            assert_eq!(
                observed.coalesce_key(),
                expected.coalesce_key.as_deref(),
                "GESTURE {context}"
            );
            assert_eq!(
                observed.commands().len(),
                expected.commands.len(),
                "COMMAND_BOUNDARIES {context}"
            );
            for (a, b) in observed.commands().iter().zip(&expected.commands) {
                assert_eq!(a.id, key(b.id), "ORIGINAL_KEY {context}");
                assert_eq!(a.position, b.position, "COMMAND_POSITION {context}");
                assert_eq!(
                    &*a.operations, &b.operations,
                    "COMMAND_OPERATIONS {context}"
                );
            }
        }
    }
}
fn identities(document: &mut Document, operations: &[Operation]) -> Vec<OperationIdentityEffect> {
    let mut result = vec![];
    for operation in operations {
        let old: BTreeSet<_> = document
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        let parent = if let Operation::SplitClip { clip, .. } = operation {
            document.clip(*clip).cloned()
        } else {
            None
        };
        operation.apply(document).unwrap();
        let new: BTreeSet<_> = document
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        let children = parent
            .iter()
            .flat_map(|parent| {
                document
                    .tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .filter(|c| c.id == parent.id || !old.contains(&c.id))
                    .cloned()
            })
            .collect();
        result.push(OperationIdentityEffect {
            created: new.difference(&old).copied().collect(),
            deleted: old.difference(&new).copied().collect(),
            split_parent: parent,
            split_results: children,
        });
    }
    result
}
fn observe(
    effects: &Effects,
    before: &Model,
    after: &Model,
    fates: &mut BTreeMap<(u64, u64), Fate>,
    context: &str,
) {
    assert_eq!(
        effects.accepted,
        key((before.epoch, before.position)),
        "ACCEPTED_KEY {context}"
    );
    for (keys, fate) in [
        (&effects.to_undo, Fate::Undo),
        (&effects.to_redo, Fate::Redo),
        (&effects.discarded, Fate::Discarded),
    ] {
        for key in keys {
            fates.insert((key.epoch, key.position), fate);
        }
    }
    let mut baseline = before.baseline.clone();
    let mut folded = vec![];
    for command in before.undo.iter().flat_map(|s| &s.commands) {
        if after.fates[&command.id] == Fate::Folded {
            folded.push(CommandEffects {
                key: key(command.id),
                identities: identities(&mut baseline, &command.operations),
            });
        }
    }
    assert_eq!(effects.folded, folded, "FOLDED_IDENTITY_EFFECTS {context}");
    for command in &effects.folded {
        fates.insert((command.key.epoch, command.key.position), Fate::Folded);
    }
    let mut document = before.document();
    let mut applied = vec![];
    if let Some(step) = after.undo.last() {
        for command in &step.commands {
            if effects.to_undo.contains(&key(command.id)) {
                applied.push(CommandEffects {
                    key: key(command.id),
                    identities: identities(&mut document, &command.operations),
                });
            }
        }
    }
    assert_eq!(
        effects.applied, applied,
        "APPLIED_IDENTITY_EFFECTS {context}"
    );
}
fn outcome(expected: &Result<usize, Refusal>, actual: &Result<Effects, Error>, context: &str) {
    match (expected, actual) {
        (Ok(_), Ok(_))
        | (Err(Refusal::DocumentTooLarge), Err(Error::DocumentCapacity))
        | (Err(Refusal::StepTooLarge), Err(Error::StepTooLarge))
        | (Err(Refusal::HistoryFull), Err(Error::HistoryFull)) => {}
        (Err(Refusal::Batch(a)), Err(Error::Batch(b))) => {
            assert_eq!(a, b, "BATCH_REFUSAL {context}");
        }
        (
            Err(Refusal::Operation(a)),
            Err(Error::Batch(kinewright_core::BatchError::OperationFailed {
                op_number: 1,
                error: b,
            })),
        ) => assert_eq!(a, b, "OP_REFUSAL {context}"),
        _ => panic!("OUTCOME {context}: {expected:?} != {actual:?}"),
    }
}
fn differential(seeds: u64, length: usize) {
    let cases = oracle::operation_cases();
    let operations: Vec<_> = cases.values().map(|c| &c.operation).collect();
    for seed in 1..=seeds {
        for (kind, case) in &cases {
            for policy in [Policy::Fold, Policy::NeverFold] {
                for (step_cap, checkpoints) in [(1, 0), (1, 1), (1, 4), (3, 0), (3, 1), (3, 4)] {
                    let budget = serde_json::to_vec(&case.operation).unwrap().len() as u64 + 190;
                    let mut history = History::new(
                        case.document.clone(),
                        match policy {
                            Policy::Fold => RetentionPolicy::Fold,
                            Policy::NeverFold => RetentionPolicy::NeverFold,
                        },
                        RoleBounds {
                            step_cap,
                            ..bounds(budget, checkpoints)
                        },
                    )
                    .unwrap();
                    let mut model = Model::new(
                        case.document.clone(),
                        usize::try_from(budget).unwrap(),
                        policy,
                    );
                    model.step_cap = step_cap;
                    let mut fates = BTreeMap::new();
                    let mut random = commands::Random(0x4858_200a_0000_0000 | seed);
                    let first = match seed % 3 {
                        0 => Command::Do(case.operation.clone()),
                        1 => Command::DoBatch(vec![case.operation.clone()]),
                        _ => Command::DoBatchCoalesced {
                            operations: vec![case.operation.clone()],
                            coalesce_key: "drag-a".into(),
                        },
                    };
                    let prefix = commands::initial_commands(first);
                    for index in 0..prefix.len() + length {
                        let context = format!(
                            "seed={seed} kind={kind} policy={policy:?} cap={step_cap} cache={checkpoints} index={index}"
                        );
                        // These are independent before states for transactional refusal and effects.
                        let before = model.clone();
                        let old = history.clone();
                        if index % 17 == 16 {
                            model.close_gesture();
                            let effects = history.close_gesture().unwrap();
                            observe(&effects, &before, &model, &mut fates, &context);
                        } else {
                            let generated;
                            let command = if let Some(c) = prefix.get(index) {
                                c
                            } else {
                                generated = commands::command(&mut random, &operations, index);
                                &generated
                            };
                            let plain = commands::plain_command(command);
                            let expected = model.execute(&plain);
                            let actual = history.execute(plain);
                            outcome(&expected, &actual, &context);
                            if index == 2 {
                                assert!(actual.is_ok(), "EVERY_KIND_ACCEPTED {context}");
                            }
                            if let Ok(effects) = actual {
                                observe(&effects, &before, &model, &mut fates, &context);
                            } else {
                                assert_eq!(history, old, "REFUSAL_ALL_STATE_UNCHANGED {context}");
                            }
                        }
                        assert_state(&history, &model, &fates, &context);
                        if index == 6 || index == 9 || random.next().is_multiple_of(13) {
                            let record = history.record();
                            assert_eq!(
                                record.snapshot_identity().unwrap(),
                                history.snapshot_identity().unwrap(),
                                "ROUNDTRIP_CANONICAL {context}"
                            );
                            let snapshot = kinewright_core::undo::Record::decode(
                                &record.encode().unwrap(),
                                history.bounds(),
                            )
                            .unwrap();
                            let mut restored = History::new(
                                Document::default(),
                                history.policy(),
                                history.bounds(),
                            )
                            .unwrap();
                            restored.install_snapshot(snapshot).unwrap();
                            model.install_snapshot(model.snapshot()).unwrap();
                            history = restored;
                            assert_state(&history, &model, &fates, &context);
                        }
                        if random.next().is_multiple_of(7) {
                            history.drop_cache();
                            assert_state(&history, &model, &fates, &context);
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn differential_all_operations_both_policies_all_cache_modes() {
    differential(1, 12);
}
#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn differential_long() {
    differential(3, 32);
}

fn literal_document() -> Document {
    Document {
        tracks: vec![kinewright_core::Track {
            id: kinewright_core::TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    }
}
fn lock(locked: bool) -> Operation {
    Operation::SetTrackSyncLock {
        track: kinewright_core::TrackId(1),
        locked,
    }
}
#[test]
fn literal_charge_and_redo_refusal_are_independent() {
    for policy in [RetentionPolicy::Fold, RetentionPolicy::NeverFold] {
        for (bytes, fits) in [(47, true), (46, false)] {
            let mut plain = History::new(literal_document(), policy, bounds(bytes, 8)).unwrap();
            let before = plain.clone();
            let result = plain.execute(Command::Do(lock(false)));
            if fits {
                assert!(result.is_ok(), "PRODUCTION_PLAIN_EXACT_47");
                assert_eq!(
                    plain.charge().encoded,
                    47,
                    "PRODUCTION_PLAIN_LITERAL_CHARGE"
                );
            } else {
                assert_eq!(
                    result,
                    Err(Error::StepTooLarge),
                    "PRODUCTION_PLAIN_ONE_OVER"
                );
                assert_eq!(plain, before, "PRODUCTION_PLAIN_REFUSAL_ATOMIC");
            }
            let mut history = History::new(literal_document(), policy, bounds(bytes, 8)).unwrap();
            history.execute(Command::Do(lock(true))).unwrap();
            assert_eq!(history.charge().encoded, 46, "PRODUCTION_LITERAL_CHARGE_46");
            history.execute(Command::Undo).unwrap();
            let before = history.clone();
            let result = history.execute(Command::Do(lock(false)));
            if fits {
                assert!(result.is_ok(), "PRODUCTION_LITERAL_EXACT_47");
                assert_eq!(history.charge().encoded, 47);
            } else {
                assert_eq!(
                    result,
                    Err(Error::StepTooLarge),
                    "PRODUCTION_LITERAL_ONE_OVER"
                );
                assert_eq!(history, before, "PRODUCTION_REDO_REFUSAL_ATOMIC");
            }
        }
        for (bytes, fits) in [(93, true), (92, false)] {
            let mut history = History::new(literal_document(), policy, bounds(bytes, 8)).unwrap();
            history
                .execute(Command::DoBatchCoalesced {
                    operations: vec![lock(true)],
                    coalesce_key: "drag".into(),
                })
                .unwrap();
            let before = history.clone();
            let result = history.execute(Command::DoBatchCoalesced {
                operations: vec![lock(false)],
                coalesce_key: "drag".into(),
            });
            if fits {
                assert!(result.is_ok(), "PRODUCTION_COALESCED_EXACT_93");
                assert_eq!(history.charge().encoded, 93);
            } else {
                assert_eq!(
                    result,
                    Err(Error::StepTooLarge),
                    "PRODUCTION_COALESCED_ONE_OVER"
                );
                assert_eq!(history, before, "PRODUCTION_COALESCED_ATOMIC");
            }
        }
    }
}
#[test]
fn empty_first_document_limit_zero_cap_and_recording() {
    let mut limits = bounds(0, 1);
    limits.step_cap = 0;
    let mut history = History::new(literal_document(), RetentionPolicy::NeverFold, limits).unwrap();
    assert_eq!(
        history.execute(Command::DoBatch(vec![])),
        Err(Error::Batch(kinewright_core::BatchError::Empty)),
        "EMPTY_BEFORE_HISTORY_CAPACITY"
    );
    let mut limits = bounds(1000, 1);
    limits.document_bytes = serde_json::to_vec(&literal_document()).unwrap().len() as u64;
    let mut history = History::new(literal_document(), RetentionPolicy::Fold, limits).unwrap();
    history.execute(Command::Do(lock(true))).unwrap();
    history.execute(Command::Undo).unwrap();
    let before = history.clone();
    assert_eq!(
        history.execute(Command::Do(lock(false))),
        Err(Error::DocumentCapacity),
        "DOCUMENT_SIZE_LIMIT"
    );
    assert_eq!(history, before, "DOCUMENT_REFUSAL_ATOMIC");
    let mut limits = bounds(1000, 1);
    limits.step_cap = 0;
    let mut history =
        History::with_recorder(literal_document(), RetentionPolicy::Fold, limits).unwrap();
    let effect = history.execute(Command::Do(lock(false))).unwrap();
    assert_eq!(
        effect.folded[0].key,
        CommandKey {
            epoch: 0,
            position: 0
        },
        "ZERO_CAP_FOLDED_KEY"
    );
    assert!(history.undo().is_empty());
    assert_eq!(history.charge().encoded, 0, "ZERO_CAP_CHARGE_DROPPED");
    assert!(!history.document().tracks[0].sync_lock);
    assert_eq!(history.op_log().unwrap(), [lock(false)]);
}
#[test]
fn single_undo_after_edit_and_coalescing_is_a_pointer_swap() {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(10_000, 1)).unwrap();
    history.execute(Command::Do(lock(false))).unwrap();
    // The capture and the pre-top cache share the document from before the gesture.
    let before = std::sync::Arc::clone(history.document());
    for locked in [true, false, true] {
        history
            .execute(Command::DoBatchCoalesced {
                operations: vec![lock(locked)],
                coalesce_key: "drag".into(),
            })
            .unwrap();
    }
    history.execute(Command::Undo).unwrap();
    assert!(
        std::sync::Arc::ptr_eq(history.document(), &before),
        "SINGLE_UNDO_POINTER_SWAP"
    );
}

#[test]
fn folding_releases_charge_at_the_exact_bound() {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(47, 8)).unwrap();
    history.execute(Command::Do(lock(true))).unwrap();
    let effects = history.execute(Command::Do(lock(false))).unwrap();
    assert_eq!(history.charge().encoded, 47, "FOLD_DROPS_CHARGE");
    assert_eq!(effects.folded.len(), 1, "FOLD_ONE_COMMAND");
    assert_eq!(history.undo().len(), 1);
    assert_eq!(history.redo().len(), 0);
}
#[test]
fn noop_close_advances_without_editing_or_touching_cache() {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 8)).unwrap();
    for filled in [false, true] {
        if filled {
            history.execute(Command::Do(lock(false))).unwrap();
        }
        let before = history.clone();
        let effects = history.close_gesture().unwrap();
        assert_eq!(
            effects.accepted,
            before.position(),
            "NOOP_CLOSE_ACCEPTED_KEY"
        );
        assert_eq!(
            history.position().position,
            before.position().position + 1,
            "NOOP_CLOSE_POSITION"
        );
        assert_eq!(history.undo(), before.undo());
        assert_eq!(history.redo(), before.redo());
        assert_eq!(history.document(), before.document());
        assert_eq!(history.charge(), before.charge());
        assert_eq!(history.cache_depths(), before.cache_depths());
    }
}
fn replace_operations(
    record: &mut kinewright_core::undo::Record,
    redo: bool,
    operations: &[Operation],
) {
    let step = if redo {
        record.redo.last_mut().unwrap()
    } else {
        record.undo.back_mut().unwrap()
    };
    // Round-trip malformed records through the public wire schema: checksums remain
    // internally consistent so these cases exercise semantic replay as well as metadata.
    let mut value = serde_json::to_value(&*step).unwrap();
    value["commands"][0]["operations"] = serde_json::to_value(operations).unwrap();
    let mut digest = kinewright_hash::Sha256::new();
    digest.update(&serde_json::to_vec(operations).unwrap());
    value["commands"][0]["payload_digest"] = serde_json::to_value(digest.finalize()).unwrap();
    *step = serde_json::from_value(value).unwrap();
}
fn record_fixture() -> History {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 8)).unwrap();
    history
        .execute(Command::DoBatch(vec![lock(false), lock(true)]))
        .unwrap();
    history.execute(Command::Do(lock(false))).unwrap();
    history.execute(Command::Undo).unwrap();
    history
}
fn refused_open(
    record: &kinewright_core::undo::Record,
    limits: RoleBounds,
    expected: Error,
    assertion: &str,
) {
    let mut destination = record_fixture();
    let before = destination.clone();
    let result = kinewright_core::undo::Record::decode(&record.encode().unwrap(), limits)
        .and_then(|snapshot| destination.install_snapshot(snapshot));
    assert_eq!(result, Err(expected), "{assertion}");
    assert_eq!(
        destination, before,
        "INVALID_INSTALL_DESTINATION_UNCHANGED {assertion}"
    );
}
#[test]
fn open_refuses_removed_undo_operation() {
    let mut record = record_fixture().record();
    replace_operations(&mut record, false, &[lock(false)]);
    refused_open(
        &record,
        bounds(1000, 8),
        Error::DocumentMismatch,
        "OPEN_REMOVED_UNDO_OPERATION",
    );
}
#[test]
fn open_refuses_changed_undo_operation() {
    let mut record = record_fixture().record();
    replace_operations(&mut record, false, &[lock(false), lock(false)]);
    refused_open(
        &record,
        bounds(1000, 8),
        Error::DocumentMismatch,
        "OPEN_CHANGED_UNDO_OPERATION",
    );
}
#[test]
fn open_refuses_wrong_saved_document() {
    let mut record = record_fixture().record();
    // The inconsistent saved document is independently valid.
    record.document = std::sync::Arc::new(Document::default());
    refused_open(
        &record,
        bounds(1000, 8),
        Error::DocumentMismatch,
        "OPEN_WRONG_SAVED_DOCUMENT",
    );
}
#[test]
fn open_refuses_invalid_redo() {
    let mut record = record_fixture().record();
    let operation = Operation::RemoveMarker {
        marker: kinewright_core::MarkerId(999),
    };
    let error = operation.apply(&mut literal_document()).unwrap_err();
    replace_operations(&mut record, true, &[operation]);
    refused_open(
        &record,
        bounds(1000, 8),
        Error::InvalidRedo(kinewright_core::BatchError::OperationFailed {
            op_number: 1,
            error,
        }),
        "OPEN_INVALID_REDO",
    );
}
#[test]
fn open_refuses_bounds_and_metadata_before_install() {
    let record = record_fixture().record();
    refused_open(&record, bounds(46, 8), Error::Capacity, "OPEN_OVER_BOUNDS");
    let mut bad = record.clone();
    bad.position = 0;
    refused_open(
        &bad,
        bounds(1000, 8),
        Error::InvalidMetadata,
        "OPEN_INVALID_POSITION",
    );
    let mut bad = record.clone();
    replace_operations(&mut bad, false, &[]);
    refused_open(
        &bad,
        bounds(1000, 8),
        Error::InvalidMetadata,
        "OPEN_EMPTY_COMMAND",
    );
    let mut destination = record_fixture();
    let before = destination.clone();
    let mut exhausted = record;
    exhausted.epoch = u64::MAX;
    assert_eq!(
        destination.install_snapshot(exhausted.validate(bounds(1000, 8)).unwrap()),
        Err(Error::CounterExhausted),
        "INSTALL_EPOCH_OVERFLOW"
    );
    assert_eq!(destination, before, "INSTALL_OVERFLOW_ATOMIC");
}
#[test]
fn canonical_identity_and_original_keys_survive_install() {
    let mut history = record_fixture();
    let record = history.record();
    let encoded = record.encode().unwrap();
    let identity = record.snapshot_identity().unwrap();
    assert_eq!(
        history.snapshot_identity().unwrap(),
        identity,
        "CANONICAL_RECORD_IDENTITY"
    );
    history.drop_cache();
    assert_eq!(
        history.snapshot_identity().unwrap(),
        identity,
        "CACHE_NOT_IN_IDENTITY"
    );
    assert!(
        !String::from_utf8(encoded.clone())
            .unwrap()
            .contains("document_before")
    );
    let keys: Vec<_> = history
        .undo()
        .iter()
        .chain(history.redo())
        .flat_map(kinewright_core::undo::Step::commands)
        .map(|c| c.id)
        .collect();
    let mut target = History::new(
        Document::default(),
        RetentionPolicy::NeverFold,
        bounds(1000, 0),
    )
    .unwrap();
    target
        .install_snapshot(kinewright_core::undo::Record::decode(&encoded, bounds(1000, 8)).unwrap())
        .unwrap();
    assert_eq!(
        target.position(),
        CommandKey {
            epoch: 1,
            position: history.position().position
        },
        "INSTALL_STARTS_EPOCH"
    );
    let retained: Vec<_> = target
        .undo()
        .iter()
        .chain(target.redo())
        .flat_map(kinewright_core::undo::Step::commands)
        .map(|c| c.id)
        .collect();
    assert_eq!(retained, keys, "INSTALL_NEVER_REKEYS_COMMANDS");
    let redone = target.execute(Command::Redo).unwrap();
    assert_eq!(redone.to_undo[0].epoch, 0, "REDO_ORIGINAL_EPOCH");
    target.execute(Command::Undo).unwrap();
    let next = target.execute(Command::Do(lock(false))).unwrap();
    assert_eq!(next.accepted.epoch, 1, "NEW_COMMAND_NEW_EPOCH");
    assert_eq!(next.discarded[0].epoch, 0, "DISCARD_ORIGINAL_EPOCH");
}

#[test]
fn opt_in_recorder_has_a_bounded_atomic_refusal() {
    let mut limits = bounds(47, 1);
    limits.decoded_history_bytes = 10_000;
    let mut history =
        History::with_recorder(literal_document(), RetentionPolicy::Fold, limits).unwrap();
    let mut refused = false;
    for index in 0_usize..1000 {
        let before = history.clone();
        match history.execute(Command::Do(lock(index.is_multiple_of(2)))) {
            Ok(_) => assert_eq!(history.op_log().unwrap().len(), index + 1),
            Err(Error::Capacity) => {
                assert_eq!(history, before, "RECORDER_REFUSAL_ATOMIC");
                history.execute(Command::Undo).unwrap();
                history.execute(Command::Redo).unwrap();
                refused = true;
                break;
            }
            other => panic!("RECORDER_UNEXPECTED {other:?}"),
        }
    }
    assert!(refused, "RECORDER_IS_BOUNDED");
}
#[test]
fn coalesced_step_reserves_its_future_redo_observations() {
    let mut limits = bounds(100_000, 1);
    limits.document_live_bytes = 4096;
    limits.replay_workspace = 64 << 10;
    let mut history = History::new(literal_document(), RetentionPolicy::Fold, limits).unwrap();
    let mut refused = false;
    for index in 0_usize..1000 {
        let before = history.clone();
        match history.execute(Command::DoBatchCoalesced {
            operations: vec![lock(index.is_multiple_of(2))],
            coalesce_key: "drag".into(),
        }) {
            Ok(_) => {}
            Err(Error::Capacity) => {
                assert_eq!(history, before, "REDO_WORKSPACE_REFUSAL_ATOMIC");
                let commands = history.undo().back().unwrap().commands().len();
                // A near-full admitted step must remain replayable as a prefix
                // with all checkpoints dropped, without charging its margin twice.
                let expected = std::sync::Arc::clone(history.document());
                history
                    .execute(Command::Do(lock(!expected.tracks[0].sync_lock)))
                    .unwrap();
                history.drop_cache();
                assert!(
                    history.execute(Command::Undo).is_ok(),
                    "ADMITTED_PREFIX_REPLAY_FITS"
                );
                assert_eq!(history.document(), &expected, "ADMITTED_PREFIX_DOCUMENT");
                assert_eq!(
                    history.execute(Command::Undo).unwrap().to_redo.len(),
                    commands,
                    "UNDO_ALL_COALESCED_COMMANDS"
                );
                assert_eq!(
                    history.execute(Command::Redo).unwrap().to_undo.len(),
                    commands,
                    "REDO_ADMITTED_STEP_FITS"
                );
                refused = true;
                break;
            }
            other => panic!("REDO_WORKSPACE_UNEXPECTED {other:?}"),
        }
    }
    assert!(refused, "COALESCED_REDO_IS_BOUNDED");
}

#[test]
fn checkpoint_depth_is_the_document_below_the_step() {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 8)).unwrap();
    for locked in [false, true, false] {
        history.execute(Command::Do(lock(locked))).unwrap();
    }
    history.execute(Command::Undo).unwrap();
    assert!(
        history.document().tracks[0].sync_lock,
        "CHECKPOINT_DEPTH_DOCUMENT"
    );
}

#[test]
fn both_coalesced_commands_report_undo_redo_and_discard_fates() {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 8)).unwrap();
    let mut model = Model::new(literal_document(), 1000, Policy::Fold);
    model.step_cap = 3;
    let mut fates = BTreeMap::new();
    let drag = |locked| Command::DoBatchCoalesced {
        operations: vec![lock(locked)],
        coalesce_key: "drag".into(),
    };
    for command in [
        drag(true),
        drag(false),
        Command::Undo,
        Command::Redo,
        Command::Undo,
        Command::Do(lock(true)),
    ] {
        let before = model.clone();
        model.execute(&command).unwrap();
        let effects = history.execute(command).unwrap();
        observe(
            &effects,
            &before,
            &model,
            &mut fates,
            "COALESCED_FATE_TRANSITIONS",
        );
        assert_state(&history, &model, &fates, "COALESCED_FATE_TRANSITIONS");
    }
    assert_eq!(fates[&(0, 0)], Fate::Discarded, "COALESCED_FIRST_DISCARDED");
    assert_eq!(
        fates[&(0, 1)],
        Fate::Discarded,
        "COALESCED_SECOND_DISCARDED"
    );
}
#[test]
fn boundary_queries_preserve_the_pre_top_pointer() {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 1)).unwrap();
    history.execute(Command::Do(lock(false))).unwrap();
    history.execute(Command::Do(lock(true))).unwrap();
    // The test capture shares the pre-edit document with the dedicated cache slot.
    let before = std::sync::Arc::clone(history.document());
    history.execute(Command::Do(lock(false))).unwrap();
    assert!(!history.boundary_document(1).unwrap().tracks[0].sync_lock);
    history.execute(Command::Undo).unwrap();
    assert!(
        std::sync::Arc::ptr_eq(history.document(), &before),
        "BOUNDARY_QUERY_PRESERVES_PRE_TOP"
    );
}
#[test]
fn optional_warming_cannot_refuse_a_valid_replay() {
    let mut document = literal_document();
    document.markers = (0..10)
        .map(|id| kinewright_core::Marker {
            id: kinewright_core::MarkerId(id),
            position: kinewright_core::TimeCode(0),
            label: "marker".repeat(40),
            color_token: 0,
        })
        .collect();
    let live = History::new(document.clone(), RetentionPolicy::Fold, bounds(32_000, 16))
        .unwrap()
        .document_bound()
        .decoded;
    let mut limits = bounds(32_000, 16);
    limits.step_cap = 512;
    limits.document_live_bytes = 2 * live;
    limits.replay_workspace = 10 * live;
    let mut history = History::new(document, RetentionPolicy::Fold, limits).unwrap();
    for index in 0..300 {
        history.execute(Command::Do(lock(index % 2 == 1))).unwrap();
    }
    history.drop_cache();
    let identity = history.snapshot_identity().unwrap();
    assert!(
        !history.boundary_document(299).unwrap().tracks[0].sync_lock,
        "TIGHT_REPLAY_SUCCEEDS"
    );
    assert!(
        history.cache_depths().is_empty(),
        "WARMING_DROPPED_WITHOUT_REFUSAL"
    );
    assert_eq!(
        history.snapshot_identity().unwrap(),
        identity,
        "WARMING_QUERY_LEAVES_RECORD_UNCHANGED"
    );
    history.execute(Command::Undo).unwrap();
    assert!(!history.document().tracks[0].sync_lock);
}

#[test]
fn retained_payload_capacity_is_charged_without_copying() {
    let mut label = String::with_capacity(1 << 20);
    label.push_str("marker");
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 4)).unwrap();
    history
        .execute(Command::Do(Operation::AddMarker {
            marker: kinewright_core::Marker {
                id: kinewright_core::MarkerId(1),
                position: kinewright_core::TimeCode(0),
                label,
                color_token: 0,
            },
        }))
        .unwrap();
    let Operation::AddMarker { marker } =
        &history.undo().back().unwrap().commands()[0].operations[0]
    else {
        panic!("marker")
    };
    assert!(
        marker.label.capacity() >= 1 << 20 && history.charge().decoded >= 1 << 20,
        "PAYLOAD_CAPACITY_CHARGED"
    );
}

#[test]
fn escaped_gesture_keys_are_charged_and_released_consistently() {
    let mut history =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 4)).unwrap();
    history
        .execute(Command::DoBatchCoalesced {
            operations: vec![lock(true)],
            coalesce_key: "\u{0}".repeat(100),
        })
        .unwrap();
    let before = history.charge();
    let record = history.record();
    let mut reopened =
        History::new(Document::default(), RetentionPolicy::Fold, history.bounds()).unwrap();
    reopened
        .install_snapshot(record.validate(history.bounds()).unwrap())
        .unwrap();
    assert_eq!(reopened.charge(), before, "ESCAPED_KEY_OPEN_CHARGE");
    history.close_gesture().unwrap();
    reopened.close_gesture().unwrap();
    assert_eq!(
        before.decoded - history.charge().decoded,
        602,
        "ESCAPED_KEY_LITERAL_CHARGE"
    );
    assert_eq!(
        history.charge(),
        reopened.charge(),
        "ESCAPED_KEY_RELEASE_CHARGE"
    );
}

#[test]
fn typed_snapshot_does_not_install_vacant_queue_allocations() {
    let mut record = record_fixture().record();
    record.undo.reserve(1 << 16);
    record.redo.reserve(1 << 16);
    let identity = record.snapshot_identity().unwrap();
    let mut history =
        History::new(Document::default(), RetentionPolicy::Fold, bounds(1000, 4)).unwrap();
    let snapshot = record.validate(history.bounds()).unwrap();
    history.install_snapshot(snapshot).unwrap();
    assert!(
        history.retained_live_bound().unwrap() < 1 << 20,
        "NORMALIZED_SNAPSHOT_STACK_STORAGE"
    );
    let mut installed = history.record();
    installed.epoch = 0;
    assert_eq!(
        installed.snapshot_identity().unwrap(),
        identity,
        "COMPACTED_RECORD_IDENTITY"
    );
}

fn marker_document() -> Document {
    let mut document = literal_document();
    document.markers.push(kinewright_core::Marker {
        id: kinewright_core::MarkerId(1),
        position: kinewright_core::TimeCode(0),
        label: "marker".into(),
        color_token: 0,
    });
    document
}
#[test]
fn initial_document_spare_capacity_is_charged() {
    let mut document = marker_document();
    document.markers[0].label.reserve(1 << 20);
    let mut limits = bounds(1000, 4);
    limits.document_live_bytes = 64 << 10;
    assert!(
        matches!(
            History::new(document, RetentionPolicy::Fold, limits),
            Err(Error::DocumentCapacity)
        ),
        "INITIAL_CAPACITY_REFUSED_WITHOUT_COPY"
    );
}
#[test]
fn typed_snapshot_baseline_spare_capacity_is_charged() {
    let mut limits = bounds(1000, 4);
    limits.document_live_bytes = 64 << 10;
    let history = History::new(marker_document(), RetentionPolicy::Fold, limits).unwrap();
    let mut record = history.record();
    // The malformed allocation is independent of the source's immutable document.
    let mut baseline = (**history.baseline()).clone();
    baseline.markers[0].label.reserve(1 << 20);
    record.baseline = std::sync::Arc::new(baseline);
    let target =
        History::new(Document::default(), RetentionPolicy::Fold, history.bounds()).unwrap();
    let before = target.clone();
    assert!(
        matches!(
            record.validate(history.bounds()),
            Err(Error::DocumentCapacity)
        ),
        "SNAPSHOT_BASELINE_CAPACITY_CHARGED"
    );
    assert_eq!(target, before, "SNAPSHOT_CAPACITY_REFUSAL_ATOMIC");
}

#[test]
fn typed_snapshot_payload_spare_capacity_is_charged() {
    let mut source =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 4)).unwrap();
    source
        .execute(Command::Do(Operation::AddMarker {
            marker: marker_document().markers.remove(0),
        }))
        .unwrap();
    let mut value = serde_json::to_value(source.record()).unwrap();
    let mut label = String::with_capacity(1 << 20);
    label.push_str("marker");
    value["undo"][0]["commands"][0]["operations"][0]["AddMarker"]["marker"]["label"] =
        serde_json::Value::String(label);
    let record: kinewright_core::undo::Record = serde_json::from_value(value).unwrap();
    let Operation::AddMarker { marker } = &record.undo[0].commands()[0].operations[0] else {
        panic!("marker")
    };
    assert!(
        marker.label.capacity() >= 1 << 20,
        "TYPED_PAYLOAD_HEADROOM_WITNESS"
    );
    let mut target =
        History::new(Document::default(), RetentionPolicy::Fold, source.bounds()).unwrap();
    target
        .install_snapshot(record.validate(source.bounds()).unwrap())
        .unwrap();
    let Operation::AddMarker { marker } = &target.undo()[0].commands()[0].operations[0] else {
        panic!("marker")
    };
    assert!(
        marker.label.capacity() >= 1 << 20 && target.charge().decoded >= 1 << 20,
        "SNAPSHOT_PAYLOAD_CAPACITY_CHARGED"
    );
}

#[test]
fn warm_ordinary_edit_has_one_copy_and_no_measurement() {
    let mut history = History::new(
        literal_document(),
        RetentionPolicy::Fold,
        RoleBounds::ORDINARY,
    )
    .unwrap();
    for n in 0..64 {
        history.execute(Command::Do(lock(n % 2 == 0))).unwrap();
    }
    for n in 0..64 {
        let before = kinewright_core::undo::PhaseCounters::read();
        history.execute(Command::Do(lock(n % 2 == 0))).unwrap();
        let count = kinewright_core::undo::PhaseCounters::read().since(before);
        assert_eq!(count.document_copies, 1, "ONE_ATOMIC_DOCUMENT_COPY");
        assert_eq!(count.exact_measurements, 0, "NO_WARM_EXACT_MEASUREMENT");
        assert_eq!(
            count.document_bytes_serialised, 0,
            "NO_WARM_DOCUMENT_SERIALIZATION"
        );
    }
}

fn growing_witness(mut case: oracle::OperationCase) -> oracle::OperationCase {
    // Choose a growing witness where the original fixture clears or
    // shortens a value. Pure removals are checked by the bound invariant.
    match &mut case.operation {
        Operation::MoveMarker { to, .. } => *to = kinewright_core::TimeCode(1_000_000),
        Operation::SetAudioMaster { master } => {
            case.document.audio_mix.master = kinewright_core::AudioMaster::default();
            master.gain_tenth_db = 100;
        }
        Operation::SetEffectKeyframes { curve, .. } => {
            curve.keyframes = (0..32)
                .map(|at| kinewright_core::Keyframe {
                    at: kinewright_core::TimeCode(at),
                    value: 0,
                    interpolation: kinewright_core::KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect();
        }
        Operation::SetMarkerParam { name, value, .. } => {
            *name = "label".into();
            *value = kinewright_core::ParamValue::Text("x".repeat(256));
        }
        Operation::SetTitleParam { value, .. } => {
            *value = kinewright_core::ParamValue::Text("x".repeat(256));
        }
        Operation::SetPanLaw { law } => {
            case.document.audio_mix.pan_law = kinewright_core::PanLaw::Balance;
            *law = kinewright_core::PanLaw::ConstantPower;
        }
        Operation::SetTrackSyncLock { track, locked } => {
            case.document
                .tracks
                .iter_mut()
                .find(|t| t.id == *track)
                .unwrap()
                .sync_lock = true;
            *locked = false;
        }
        Operation::TrimClip { new_source, .. } => {
            new_source.start = kinewright_core::TimeCode(10);
            new_source.end = kinewright_core::TimeCode(60);
        }
        Operation::RemoveEffectKeyframe { name, .. } => {
            let effect = &mut case.document.tracks[0].clips[0].effects[0];
            effect.parameters.clear();
            effect
                .keyframes
                .get_mut(name)
                .unwrap()
                .keyframes
                .truncate(1);
            effect
                .keyframes
                .get_mut(name)
                .unwrap()
                .keyframes
                .shrink_to_fit();
        }
        _ => {}
    }
    case
}

#[test]
fn every_growing_kind_reaches_the_document_limit() {
    let mut growing = BTreeSet::new();
    for (kind, case) in oracle::operation_cases() {
        let case = growing_witness(case);
        let mut document = case.document;
        for prefix in 0..4 {
            let mut after = document.clone();
            case.operation.apply(&mut after).unwrap();
            let before_bytes = serde_json::to_vec(&document).unwrap().len() as u64;
            let after_bytes = serde_json::to_vec(&after).unwrap().len() as u64;
            let mut limits = bounds(1 << 20, 0);
            let mut history =
                History::new(document.clone(), RetentionPolicy::Fold, limits).unwrap();
            history
                .execute(Command::Do(case.operation.clone()))
                .unwrap();
            if after_bytes > before_bytes {
                growing.insert(kind.clone());
                limits.document_bytes = after_bytes - 1;
                let mut capped =
                    History::new(document.clone(), RetentionPolicy::Fold, limits).unwrap();
                let before = capped.clone();
                assert!(
                    matches!(
                        capped.execute(Command::Do(case.operation.clone())),
                        Err(Error::DocumentCapacity)
                    ),
                    "EVERY_GROWING_KIND_LIMIT {kind} prefix={prefix}"
                );
                assert_eq!(capped, before, "GROWING_LIMIT_ATOMIC {kind}");
            }
            assert!(
                history.document_bound().encoded >= after_bytes,
                "EVERY_KIND_RUNNING_BOUND {kind} prefix={prefix}"
            );
            let exact = History::new(after, RetentionPolicy::Fold, bounds(1 << 20, 0))
                .unwrap()
                .document_bound();
            assert!(
                history.document_bound().decoded >= exact.decoded,
                "EVERY_KIND_LIVE_BOUND {kind} prefix={prefix}"
            );
            let before_live =
                History::new(document.clone(), RetentionPolicy::Fold, bounds(1 << 20, 0))
                    .unwrap()
                    .document_bound()
                    .decoded;
            if exact.decoded > before_live {
                growing.insert(kind.clone());
                let mut live_limits = bounds(1 << 20, 0);
                live_limits.document_live_bytes = exact.decoded - 1;
                let mut capped =
                    History::new(document.clone(), RetentionPolicy::Fold, live_limits).unwrap();
                let before = capped.clone();
                assert!(
                    matches!(
                        capped.execute(Command::Do(case.operation.clone())),
                        Err(Error::DocumentCapacity)
                    ),
                    "EVERY_GROWING_KIND_LIVE_LIMIT {kind} prefix={prefix}"
                );
                assert_eq!(capped, before, "GROWING_LIVE_LIMIT_ATOMIC {kind}");
            }
            document.markers.push(kinewright_core::Marker {
                id: kinewright_core::MarkerId(1_000_000 + prefix),
                position: kinewright_core::TimeCode(0),
                label: "padding towards the limit".repeat(8),
                color_token: 0,
            });
            document.markers.sort_by_key(|m| (m.position, m.id));
        }
    }
    eprintln!("GROWING_KIND_COVERAGE {} {growing:?}", growing.len());
    let non_growing: BTreeSet<_> = [
        "ClearEffectKeyframes",
        "DeleteClip",
        "RemoveAudioBus",
        "RemoveBin",
        "RemoveEffect",
        "RemoveEffectKeyframe",
        "RemoveLutAsset",
        "RemoveMarker",
        "RemoveStringOut",
        "RemoveSyncGroup",
        "RemoveTrack",
        "RemoveTransition",
        "RippleDeleteClip",
        "UnlinkClips",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let expected: BTreeSet<_> = oracle::operation_cases()
        .into_keys()
        .filter(|k| !non_growing.contains(k))
        .collect();
    assert_eq!(growing, expected, "EVERY_GROWING_KIND_COVERED");
}

#[test]
fn task_profile_accepts_two_thousand_ordinary_commands() {
    let mut history = History::new(
        literal_document(),
        RetentionPolicy::NeverFold,
        RoleBounds::TASK,
    )
    .unwrap();
    for n in 0..2000 {
        history.execute(Command::Do(lock(n % 2 == 0))).unwrap();
    }
    assert_eq!(history.undo().len(), 2000, "TASK_2000_COMMANDS");
    assert!(history.charge().encoded <= 256 << 10, "TASK_OPERATION_CAP");
    history.execute(Command::Undo).unwrap();
    history.execute(Command::Redo).unwrap();
}

#[test]
fn malformed_redo_cannot_acquire_a_runtime_proof() {
    let mut source =
        History::new(literal_document(), RetentionPolicy::Fold, bounds(1000, 4)).unwrap();
    source
        .execute(Command::Do(Operation::RemoveMarker {
            marker: kinewright_core::MarkerId(999),
        }))
        .unwrap_err();
    source.execute(Command::Do(lock(false))).unwrap();
    source.execute(Command::Undo).unwrap();
    let mut value = serde_json::to_value(source.record()).unwrap();
    value["redo"][0]["commands"][0]["operations"][0] =
        serde_json::to_value(Operation::RemoveMarker {
            marker: kinewright_core::MarkerId(999),
        })
        .unwrap();
    let operations: Vec<Operation> =
        serde_json::from_value(value["redo"][0]["commands"][0]["operations"].clone()).unwrap();
    let bytes = serde_json::to_vec(&operations).unwrap();
    value["redo"][0]["commands"][0]["payload_digest"] = serde_json::to_value({
        let mut hash = kinewright_hash::Sha256::new();
        hash.update(&bytes);
        hash.finalize()
    })
    .unwrap();
    let record: kinewright_core::undo::Record = serde_json::from_value(value).unwrap();
    assert!(
        matches!(record.validate(source.bounds()), Err(Error::InvalidRedo(_))),
        "UNTRUSTED_REDO_VALIDATED_BEFORE_TRUST"
    );
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn task_profile_accepts_two_thousand_cuts() {
    let mut case = oracle::operation_cases().remove("SplitClip").unwrap();
    let mut clip = case.document.tracks[0].clips[0].clone();
    clip.id = kinewright_core::ClipId(1);
    clip.timeline_start = kinewright_core::TimeCode(0);
    clip.source_range = kinewright_core::TimeCode(0)..kinewright_core::TimeCode(10_000);
    clip.effects.clear();
    case.document.media_pool.retain(|a| a.id == clip.asset);
    case.document.media_pool[0].duration = kinewright_core::TimeCode(10_000);
    case.document.tracks.truncate(1);
    case.document.tracks[0].clips = vec![clip];
    case.document.duration = kinewright_core::TimeCode(10_000);
    let mut history =
        History::new(case.document, RetentionPolicy::NeverFold, RoleBounds::TASK).unwrap();
    for n in 0..2000 {
        history
            .execute(Command::Do(Operation::SplitClip {
                clip: kinewright_core::ClipId(n + 1),
                at: kinewright_core::TimeCode(i64::try_from(n).unwrap() + 1),
            }))
            .unwrap();
    }
    assert_eq!(history.undo().len(), 2000, "TASK_2000_CUTS");
    assert_eq!(
        history.document().tracks[0].clips.len(),
        2001,
        "TASK_CUTS_APPLIED"
    );
    assert!(
        history.charge().encoded <= 256 << 10,
        "TASK_CUTS_OPERATION_CAP"
    );
    assert!(
        history.retained_live_bound().unwrap() < 41 << 20,
        "TASK_NATIVE_CAP"
    );
    history.execute(Command::Undo).unwrap();
    history.execute(Command::Redo).unwrap();
}

#[test]
fn atomic_copy_preserves_capacity_for_trusted_replay() {
    let case = oracle::operation_cases().remove("SplitClip").unwrap();
    let mut document = case.document;
    let mut template = document.tracks[0].clips[0].clone();
    template.source_range = kinewright_core::TimeCode(0)..kinewright_core::TimeCode(1);
    template.effects.clear();
    template.audio_gain_curve = None;
    template.enabled_curve = None;
    template.transition_in = None;
    template.link = None;
    let asset = template.asset;
    let mut track = document.tracks[0].clone();
    track.id = kinewright_core::TrackId(1);
    track.clips = (0..100)
        .map(|n| {
            let mut clip = template.clone();
            clip.id = kinewright_core::ClipId(n + 1);
            clip.timeline_start = kinewright_core::TimeCode(i64::try_from(n).unwrap());
            clip
        })
        .collect();
    let mut empty = track.clone();
    empty.id = kinewright_core::TrackId(2);
    empty.clips.clear();
    empty.clips.shrink_to_fit();
    document.tracks = vec![track, empty];
    document.markers.clear();
    document.duration = kinewright_core::TimeCode(100);
    let mut limits = bounds(1 << 20, 4);
    limits.document_live_bytes = History::new(document.clone(), RetentionPolicy::Fold, limits)
        .unwrap()
        .document_bound()
        .decoded
        * 3
        / 2;
    let mut history = History::new(document, RetentionPolicy::Fold, limits).unwrap();
    for id in 1..100 {
        history
            .execute(Command::DoBatchCoalesced {
                operations: vec![Operation::DeleteClip {
                    clip: kinewright_core::ClipId(id),
                }],
                coalesce_key: "capacity-walk".into(),
            })
            .unwrap();
    }
    assert!(
        history.document().tracks[0].clips.capacity() >= 100,
        "ATOMIC_COPY_PRESERVES_CAPACITY"
    );
    let operations = (0..100)
        .map(|n| Operation::AddClip {
            track: kinewright_core::TrackId(2),
            asset,
            at: kinewright_core::TimeCode(n),
            source: kinewright_core::TimeCode(0)..kinewright_core::TimeCode(1),
        })
        .collect();
    let before = history.clone();
    assert!(
        matches!(
            history.execute(Command::DoBatchCoalesced {
                operations,
                coalesce_key: "capacity-walk".into()
            }),
            Err(Error::DocumentCapacity)
        ),
        "CAPACITY_LIMIT_BEFORE_TRUSTED_REDO"
    );
    assert_eq!(history, before, "CAPACITY_REFUSAL_ATOMIC");
    history.execute(Command::Undo).unwrap();
    history.execute(Command::Redo).unwrap();
    assert!(
        history.record().validate(limits).is_ok(),
        "ADMITTED_REDO_RECORD_REOPENS"
    );
}
