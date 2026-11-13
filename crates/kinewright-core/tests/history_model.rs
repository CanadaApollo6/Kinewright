#[path = "support/history_model.rs"]
mod history_model;
#[path = "support/history_oracle.rs"]
mod history_oracle;

#[path = "support/history_commands.rs"]
mod commands;
use commands::{Random, command, initial_commands, marker};

use history_model::{Fate, InstallRefusal, Model, Policy, Refusal, StepCommand};
use kinewright_core::{
    Command, Core, Document, Event, HistorySnapshot, MarkerId, Operation, Query, QueryResult,
};
use std::collections::BTreeMap;

fn assert_event(result: &Result<usize, Refusal>, event: &Event, context: &str) {
    match (result, event) {
        (Ok(_), Event::DocumentChanged { .. }) => {}
        (Err(Refusal::Operation(expected)), Event::OpRejected { error, .. }) => {
            assert_eq!(error, expected, "OP_REFUSAL {context}");
        }
        (Err(Refusal::Batch(expected)), Event::BatchRejected { error, .. }) => {
            assert_eq!(error, expected, "BATCH_REFUSAL {context}");
        }
        _ => panic!("COMMAND_OUTCOME {context}: {result:?} != {event:?}"),
    }
}

fn assert_stacks(model: &Model, history: &HistorySnapshot, context: &str) {
    assert_eq!(
        *history.document,
        model.document(),
        "CURRENT_DOCUMENT {context}"
    );
    assert_eq!(history.floor_offset, 0, "NO_FLOOR {context}");
    assert_eq!(*history.baseline, model.baseline, "BASELINE {context}");
    assert_eq!(
        history.cursor.undo.len(),
        model.undo.len(),
        "UNDO_DEPTH {context}"
    );
    assert_eq!(
        history.cursor.redo.len(),
        model.redo.len(),
        "REDO_DEPTH {context}"
    );
    for (depth, (entry, step)) in history.cursor.undo.iter().zip(&model.undo).enumerate() {
        assert_eq!(
            entry.operations,
            step.operations().cloned().collect::<Vec<_>>(),
            "UNDO_OPERATIONS {context} depth={depth}"
        );
        assert_eq!(
            entry.coalesce_key, step.coalesce_key,
            "UNDO_KEY {context} depth={depth}"
        );
        assert_eq!(
            *entry.document,
            model.at_depth(depth),
            "I1_DOCUMENT_BEFORE {context} depth={depth}"
        );
    }
    let mut redone = model.document();
    for (entry, step) in history
        .cursor
        .redo
        .iter()
        .rev()
        .zip(model.redo.iter().rev())
    {
        assert_eq!(
            entry.operations,
            step.operations().cloned().collect::<Vec<_>>(),
            "REDO_OPERATIONS {context}"
        );
        assert_eq!(entry.coalesce_key, step.coalesce_key, "REDO_KEY {context}");
        for operation in step.operations() {
            operation.apply(&mut redone).expect("REDO_REPLAY_VALID");
        }
        assert_eq!(*entry.document, redone, "I1_REDO_DOCUMENT_AFTER {context}");
    }
}

fn assert_command_boundaries(
    model: &Model,
    history: &HistorySnapshot,
    identities: &BTreeMap<u64, (u64, u64)>,
    context: &str,
) {
    let mut undo: Vec<Vec<StepCommand>> = Vec::new();
    let mut redo = Vec::new();
    for (index, batch) in history.batches.iter().enumerate() {
        assert_eq!(
            batch.order,
            u64::try_from(index).unwrap(),
            "BATCH_ORDER {context}"
        );
        let operations = match &batch.command {
            kinewright_core::JournalCommand::Undo => {
                if let Some(step) = undo.pop() {
                    redo.push(step);
                }
                assert_eq!(batch.redo_entries, redo.len(), "BATCH_REDO_DEPTH {context}");
                continue;
            }
            kinewright_core::JournalCommand::Redo => {
                if let Some(step) = redo.pop() {
                    undo.push(step);
                }
                assert_eq!(batch.redo_entries, redo.len(), "BATCH_REDO_DEPTH {context}");
                continue;
            }
            kinewright_core::JournalCommand::Do(operation) => std::slice::from_ref(operation),
            kinewright_core::JournalCommand::DoBatch(operations)
            | kinewright_core::JournalCommand::DoBatchCoalesced { operations, .. } => operations,
        };
        let command = StepCommand {
            id: identities[&batch.order],
            position: identities[&batch.order].1,
            operations: operations.into(),
        };
        if batch.undo_entries == undo.len() + 1 {
            undo.push(vec![command]);
        } else {
            assert_eq!(batch.undo_entries, undo.len(), "COALESCED_DEPTH {context}");
            undo.last_mut().unwrap().push(command);
        }
        redo.clear();
        assert_eq!(batch.redo_entries, redo.len(), "BATCH_REDO_DEPTH {context}");
    }
    let mut fates: BTreeMap<_, _> = history
        .batches
        .iter()
        .filter(|batch| {
            !matches!(
                batch.command,
                kinewright_core::JournalCommand::Undo | kinewright_core::JournalCommand::Redo
            )
        })
        .map(|batch| (identities[&batch.order], Fate::Discarded))
        .collect();
    for (steps, fate) in [(&undo, Fate::Undo), (&redo, Fate::Redo)] {
        for command in steps.iter().flatten() {
            fates.insert(command.id, fate);
        }
    }
    assert_eq!(model.fates, fates, "EVERY_COMMAND_FATE {context}");
    let encoded: usize = history
        .cursor
        .undo
        .iter()
        .chain(&history.cursor.redo)
        .flat_map(|step| &step.operations)
        .map(|op| serde_json::to_vec(op).unwrap().len())
        .sum();
    assert_eq!(model.cost(), encoded, "INDEPENDENT_HISTORY_BYTES {context}");
    for (observed, expected, name) in [(&undo, &model.undo, "UNDO"), (&redo, &model.redo, "REDO")] {
        assert_eq!(
            observed.len(),
            expected.len(),
            "COMMAND_STACK_DEPTH {context}"
        );
        for (commands, step) in observed.iter().zip(expected) {
            assert_eq!(
                commands, &step.commands,
                "STEP_COMMAND_BOUNDARIES {name} {context}"
            );
        }
    }
}

fn assert_identity_observations(
    before: &kinewright_core::Document,
    batch: &kinewright_core::HistoryBatch,
    context: &str,
) {
    use std::collections::BTreeSet;
    let operations = match &batch.command {
        kinewright_core::JournalCommand::Do(operation) => std::slice::from_ref(operation),
        kinewright_core::JournalCommand::DoBatch(operations)
        | kinewright_core::JournalCommand::DoBatchCoalesced { operations, .. } => operations,
        kinewright_core::JournalCommand::Undo | kinewright_core::JournalCommand::Redo => &[],
    };
    let mut document = before.clone();
    assert_eq!(
        batch.identity_effects.len(),
        operations.len(),
        "IDENTITY_EFFECT_COUNT {context}"
    );
    for (operation, effect) in operations.iter().zip(&batch.identity_effects) {
        let old: BTreeSet<_> = document
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(|clip| clip.id))
            .collect();
        let parent = if let Operation::SplitClip { clip, .. } = operation {
            document.clip(*clip).cloned()
        } else {
            None
        };
        operation.apply(&mut document).unwrap();
        let new: BTreeSet<_> = document
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(|clip| clip.id))
            .collect();
        assert_eq!(
            effect.created,
            new.difference(&old).copied().collect::<Vec<_>>(),
            "IDENTITY_CREATED {context}"
        );
        assert_eq!(
            effect.deleted,
            old.difference(&new).copied().collect::<Vec<_>>(),
            "IDENTITY_DELETED {context}"
        );
        assert_eq!(
            effect.split_parent, parent,
            "IDENTITY_SPLIT_PARENT {context}"
        );
        let children: Vec<_> = parent
            .iter()
            .flat_map(|parent| {
                document
                    .tracks
                    .iter()
                    .flat_map(|track| &track.clips)
                    .filter(|clip| clip.id == parent.id || !old.contains(&clip.id))
                    .cloned()
            })
            .collect();
        assert_eq!(
            effect.split_results, children,
            "IDENTITY_SPLIT_RESULTS {context}"
        );
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one seeded stream keeps each transition and its oracle checks together"
)]
fn differential(seeds: u64, length: usize) {
    let cases = history_oracle::operation_cases();
    let operations: Vec<_> = cases.values().map(|case| &case.operation).collect();
    let mut hits = std::collections::BTreeSet::new();
    let mut installed_redo = 0;
    let mut installed_gesture = 0;
    for seed in 1..=seeds {
        let mut random = Random(0x4858_a11c_0000_0000 | seed);
        for (kind, case) in &cases {
            // Separate starting documents are the independent Core and oracle states.
            let mut core = Core::spawn(case.document.clone()).unwrap();
            let mut identities = BTreeMap::new();
            let mut model = Model::new(case.document.clone(), usize::MAX, Policy::NeverFold);
            let first = match seed % 3 {
                0 => Command::Do(case.operation.clone()),
                1 => Command::DoBatch(vec![case.operation.clone()]),
                _ => Command::DoBatchCoalesced {
                    operations: vec![case.operation.clone()],
                    coalesce_key: "drag-a".into(),
                },
            };
            let install_at = 10 + random.index(length);
            let prefix = initial_commands(first);
            for index in 0..prefix.len() + length {
                let generated;
                let command = if let Some(command) = prefix.get(index) {
                    command
                } else {
                    generated = command(&mut random, &operations, index);
                    &generated
                };
                let context = format!(
                    "seed={:#x} kind={kind} command={index} {command:?}",
                    0x4858_a11c_0000_0000 | seed
                );
                let mut submitted = command.clone();
                match &mut submitted {
                    Command::DoIfRevision { expected, .. }
                    | Command::DoBatchIfRevision { expected, .. } => {
                        *expected = core.history_change_id().unwrap().revision();
                    }
                    _ => {}
                }
                let plain = commands::plain_command(&submitted);
                let before = model.document();
                let identity = (model.epoch, model.position);
                let conditional = matches!(
                    submitted,
                    Command::DoIfRevision { .. } | Command::DoBatchIfRevision { .. }
                );
                let result = model.execute(&plain);
                let event = core.request(submitted).unwrap();
                assert_event(&result, &event, &context);
                if result.is_ok() {
                    identities.insert(identity.1, identity);
                }
                if index == 2 {
                    assert!(result.is_ok(), "KIND_REACHABLE {context}");
                    hits.insert(kind);
                }
                let history = core.export_history().unwrap();
                if result.is_ok() {
                    if conditional {
                        assert_eq!(
                            Command::from(history.batches.last().unwrap().command.clone()),
                            plain,
                            "CONDITIONAL_RECORDED_PLAIN {context}"
                        );
                    }
                    assert_identity_observations(
                        &before,
                        history.batches.last().unwrap(),
                        &context,
                    );
                }
                assert_stacks(&model, &history, &context);
                assert_command_boundaries(&model, &history, &identities, &context);
                assert_eq!(
                    model.position,
                    u64::try_from(history.batches.len()).unwrap(),
                    "ACCEPTED_POSITION {context}"
                );
                let Event::QueryResult(QueryResult::AppliedOperations(applied)) = core
                    .request(Command::Query(Query::AppliedOperations))
                    .unwrap()
                else {
                    panic!("APPLIED_QUERY {context}")
                };
                assert_eq!(
                    *applied,
                    model.applied_operations(),
                    "APPLIED_OPERATIONS {context}"
                );
                assert_eq!(
                    history.applied_operations(),
                    *applied,
                    "EXPORTED_APPLIED_OPERATIONS {context}"
                );
                if index == install_at {
                    installed_redo += usize::from(!model.redo.is_empty());
                    installed_gesture += usize::from(
                        model
                            .undo
                            .last()
                            .is_some_and(|step| step.coalesce_key.is_some()),
                    );
                    let snapshot = model.snapshot();
                    let epoch = model.epoch;
                    let replacement = Core::spawn(kinewright_core::Document::default()).unwrap();
                    replacement.restore_history(history).unwrap();
                    core = replacement;
                    model = Model::new(kinewright_core::Document::default(), 1, Policy::Fold);
                    model.install_snapshot(snapshot).unwrap();
                    assert_eq!(model.epoch, epoch + 1, "INSTALL_NEW_EPOCH {context}");
                    let restored = core.export_history().unwrap();
                    assert_stacks(&model, &restored, &context);
                    assert_command_boundaries(&model, &restored, &identities, &context);
                }
            }
        }
    }
    assert_eq!(hits.len(), 67, "EVERY_KIND_ACCEPTED_IN_DIFFERENTIAL");
    assert!(installed_redo > 0, "INSTALL_RETAINS_REDO_EXERCISED");
    assert!(installed_gesture > 0, "INSTALL_RETAINS_GESTURE_EXERCISED");
}

#[test]
fn seeded_commands_match_core_and_i1() {
    differential(3, 32);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn seeded_commands_match_core_and_i1_long() {
    differential(12, 64);
}

fn assert_undo_redo_roundtrip(model: &Model, context: &str) {
    let mut undone = model.clone();
    for _ in 0..model.undo.len() {
        assert!(
            undone.execute(&Command::Undo).is_ok(),
            "UNDO_NEVER_REFUSED {context}"
        );
    }
    assert_eq!(
        undone.document(),
        model.baseline,
        "UNDO_ALL_TO_BASELINE {context}"
    );
    for _ in 0..model.undo.len() {
        assert!(
            undone.execute(&Command::Redo).is_ok(),
            "REDO_NEVER_REFUSED {context}"
        );
    }
    assert_eq!(
        undone.document(),
        model.document(),
        "REDO_ALL_TO_CURRENT {context}"
    );
}

#[test]
fn tiny_budgets_fold_or_refuse_transactionally() {
    let cases = history_oracle::operation_cases();
    let operations: Vec<_> = cases.values().map(|case| &case.operation).collect();
    let mut folds = 0;
    let mut large = 0;
    let mut full = 0;
    for (kind, case) in &cases {
        for policy in [Policy::Fold, Policy::NeverFold] {
            let budget = Model::step_cost(std::slice::from_ref(&case.operation))
                + Model::step_cost(&[marker(1), marker(2)]);
            let mut model = Model::new(case.document.clone(), budget, policy);
            model.step_cap = 3;
            let core = Core::spawn(case.document.clone()).unwrap();
            let mut random = Random(0x4858_f01d_0000_0001);
            for index in 0..48 {
                let command = if index == 0 {
                    Command::Do(case.operation.clone())
                } else if index < 12 {
                    Command::Do(marker(30_000 + u64::try_from(index).unwrap()))
                } else {
                    command(&mut random, &operations, index)
                };
                let command = commands::plain_command(&command);
                let context = format!(
                    "seed=0x4858f01d00000001 kind={kind} policy={policy:?} command={index} {command:?}"
                );
                // Transactional refusal and complete Undo are tested against independent copies.
                let before = model.clone();
                let is_history_step = matches!(command, Command::Undo | Command::Redo);
                let was_empty_undo = model.undo.is_empty();
                let result = model.execute(&command);
                if let Err(refusal) = &result {
                    assert_eq!(model, before, "REFUSAL_UNCHANGED {context}");
                    assert!(
                        !matches!(command, Command::Undo | Command::Redo),
                        "UNDO_REDO_NEVER_REFUSED {context}"
                    );
                    match refusal {
                        Refusal::StepTooLarge => large += 1,
                        Refusal::HistoryFull => full += 1,
                        _ => {}
                    }
                    if !matches!(refusal, Refusal::StepTooLarge | Refusal::HistoryFull) {
                        assert_event(&result, &core.request(command).unwrap(), &context);
                    }
                } else {
                    // A Core without a floor retains older steps; stop it at the model's baseline.
                    if !(was_empty_undo && matches!(command, Command::Undo)) {
                        assert_event(&result, &core.request(command).unwrap(), &context);
                    }
                    folds += result.unwrap();
                }
                assert!(model.cost() <= budget, "COST_WITHIN_BUDGET {context}");
                assert!(
                    model.undo.len() + model.redo.len() <= model.step_cap,
                    "STEP_CAP_HOLDS {context}"
                );
                if is_history_step {
                    assert_eq!(
                        model.cost(),
                        before.cost(),
                        "UNDO_REDO_COST_UNCHANGED {context}"
                    );
                }
                let history = core.export_history().unwrap();
                assert_eq!(
                    *history.document,
                    model.document(),
                    "FOLD_PRESERVES_DOCUMENT {context}"
                );
                let folded_depth = history.cursor.undo.len() - model.undo.len();
                let baseline = history
                    .cursor
                    .undo
                    .get(folded_depth)
                    .map_or(&*history.document, |entry| &*entry.document);
                assert_eq!(
                    baseline, &model.baseline,
                    "FOLDED_BASELINE_BEFORE {context}"
                );
                assert_undo_redo_roundtrip(&model, &context);
                if policy == Policy::NeverFold {
                    assert_eq!(
                        model.baseline, case.document,
                        "NEVER_FOLD_BASELINE {context}"
                    );
                }
            }
        }
    }
    assert!(folds > 67, "FOLD_RULE_EXERCISED");
    assert!(large > 0, "OVERSIZED_STEP_EXERCISED");
    assert!(full > 0, "NEVER_FOLD_REFUSAL_EXERCISED");
}

#[test]
fn coalesced_refusal_preserves_redo_counter_and_whole_state() {
    for policy in [Policy::Fold, Policy::NeverFold] {
        let mut model = Model::new(
            Document::default(),
            Model::step_cost(&[marker(1), marker(2)]),
            policy,
        );
        model.execute(&Command::Do(marker(1))).unwrap();
        model.execute(&Command::Do(marker(2))).unwrap();
        model.execute(&Command::Undo).unwrap();
        let before = model.clone();
        assert_eq!(
            model.execute(&Command::DoBatch(vec![marker(3), marker(4), marker(5)])),
            Err(Refusal::StepTooLarge)
        );
        assert_eq!(model, before, "OVERSIZED_WITH_REDO_ATOMIC");
        model.execute(&Command::Redo).unwrap();
        model.execute(&Command::Undo).unwrap();
        let mut gesture = Model::new(
            Document::default(),
            Model::step_cost(&[marker(1), marker(2)]),
            policy,
        );
        gesture
            .execute(&Command::DoBatchCoalesced {
                operations: vec![marker(1)],
                coalesce_key: "drag".into(),
            })
            .unwrap();
        let before = gesture.clone();
        assert_eq!(
            gesture.execute(&Command::DoBatchCoalesced {
                operations: vec![marker(2), marker(3)],
                coalesce_key: "drag".into()
            }),
            Err(Refusal::StepTooLarge)
        );
        assert_eq!(gesture, before, "COALESCED_WHOLE_STEP_ATOMIC");
    }
}

#[test]
fn fold_many_active_steps_both_caps_and_command_fates() {
    let mut model = Model::new(Document::default(), usize::MAX, Policy::Fold);
    for id in 1..=5 {
        model.execute(&Command::Do(marker(id))).unwrap();
    }
    let large: Vec<_> = (10..=18).map(marker).collect();
    model.budget = Model::step_cost(&large);
    assert_eq!(
        model.execute(&Command::DoBatch(large)),
        Ok(5),
        "ONE_EDIT_FOLDS_MANY"
    );
    assert_eq!(model.undo.len(), 1);
    assert_eq!(
        model
            .fates
            .values()
            .filter(|fate| **fate == Fate::Folded)
            .count(),
        5
    );

    let mut capped = Model::new(Document::default(), usize::MAX, Policy::Fold);
    capped.step_cap = 1;
    capped.execute(&Command::Do(marker(1))).unwrap();
    assert_eq!(
        capped.execute(&Command::Do(marker(2))),
        Ok(1),
        "STEP_CAP_ALONE_FOLDS"
    );
    assert_eq!(capped.baseline.markers[0].id, MarkerId(1));
    let mut bounded = Model::new(Document::default(), usize::MAX, Policy::NeverFold);
    bounded.step_cap = 1;
    bounded.execute(&Command::Do(marker(1))).unwrap();
    let before = bounded.clone();
    assert_eq!(
        bounded.execute(&Command::Do(marker(2))),
        Err(Refusal::HistoryFull)
    );
    assert_eq!(bounded, before, "NEVER_FOLD_STEP_CAP_ATOMIC");
    bounded.execute(&Command::Undo).unwrap();
    bounded.execute(&Command::Redo).unwrap();

    let mut fates = Model::new(Document::default(), usize::MAX, Policy::Fold);
    fates.execute(&Command::Do(marker(1))).unwrap(); // A at position 0.
    fates.execute(&Command::Do(marker(2))).unwrap(); // B at position 1.
    assert_eq!(fates.fates[&(0, 1)], Fate::Undo);
    fates.execute(&Command::Undo).unwrap();
    assert_eq!(fates.fates[&(0, 1)], Fate::Redo);
    fates.execute(&Command::Redo).unwrap();
    assert_eq!(
        fates.fates[&(0, 1)],
        Fate::Undo,
        "REDO_RESTORES_APPLIED_FATE"
    );
    fates.execute(&Command::Undo).unwrap();
    fates.execute(&Command::Do(marker(3))).unwrap(); // C at position 3.
    assert_eq!(
        fates.fates[&(0, 1)],
        Fate::Discarded,
        "B_DISCARDED_NOT_FOLDED"
    );
    fates.step_cap = 1;
    assert_eq!(fates.execute(&Command::Do(marker(4))), Ok(2));
    assert_eq!(fates.fates[&(0, 0)], Fate::Folded, "A_FOLDED");
    assert_eq!(fates.fates[&(0, 5)], Fate::Folded, "C_FOLDED");
    assert_eq!(fates.fates[&(0, 1)], Fate::Discarded, "B_STAYS_DISCARDED");
    assert_eq!(
        fates
            .baseline
            .markers
            .iter()
            .map(|marker| marker.id)
            .collect::<Vec<_>>(),
        [MarkerId(1), MarkerId(3)]
    );
}

#[test]
fn close_gesture_matches_recovery_close_and_reused_key_opens_step() {
    let core = Core::spawn(kinewright_core::Document::default()).unwrap();
    let mut model = Model::new(Document::default(), usize::MAX, Policy::NeverFold);
    let gesture = |id| Command::DoBatchCoalesced {
        operations: vec![marker(id)],
        coalesce_key: "drag".into(),
    };
    for command in [
        Command::DoBatchCoalesced {
            operations: vec![marker(1)],
            coalesce_key: "previous".into(),
        },
        gesture(2),
        gesture(3),
    ] {
        model.execute(&command).unwrap();
        core.request(command).unwrap();
    }
    let history = core.export_history().unwrap();
    let transition =
        kinewright_core::HistoryTransition::recovery_close(history.clone(), &history).unwrap();
    core.install_history_transition(transition).unwrap();
    let position = model.position;
    model.close_gesture();
    assert_eq!(model.position, position + 1, "CLOSE_ACCEPTED_POSITION");
    assert_stacks(&model, &core.export_history().unwrap(), "CloseGesture");
    model.execute(&gesture(4)).unwrap();
    core.request(gesture(4)).unwrap();
    assert_stacks(
        &model,
        &core.export_history().unwrap(),
        "REUSED_KEY_AFTER_CLOSE",
    );
    assert_eq!(model.undo.len(), 3, "CLOSE_STARTS_NEW_STEP");
    // Recovery's two internal commands have no StepCommand in the new model.
    let identities = BTreeMap::from([(0, (0, 0)), (1, (0, 1)), (2, (0, 2)), (5, (0, 4))]);
    assert_command_boundaries(
        &model,
        &core.export_history().unwrap(),
        &identities,
        "CLOSE_COMMAND_BOUNDARIES",
    );
}

#[test]
fn snapshot_install_replaces_state_and_starts_epoch_without_a_step() {
    let mut source = Model::new(Document::default(), 1000, Policy::NeverFold);
    source.step_cap = 3;
    source.execute(&Command::Do(marker(1))).unwrap();
    source.execute(&Command::Do(marker(2))).unwrap();
    source.execute(&Command::Undo).unwrap();
    let mut destination = Model::new(Document::default(), 2000, Policy::Fold);
    destination.execute(&Command::Do(marker(99))).unwrap();
    destination.execute(&Command::Undo).unwrap();
    destination.install_snapshot(source.snapshot()).unwrap();
    source.epoch = 1;
    assert_eq!(destination, source, "INSTALL_REPLACES_ALL_STATE");
    assert_eq!(destination.position, 3, "INSTALL_NOT_A_COMMAND_STEP");
    destination.execute(&Command::Redo).unwrap();
    assert_eq!(
        destination.document().markers.len(),
        2,
        "INSTALLED_REDO_REPLAYS"
    );
    destination.execute(&Command::Do(marker(3))).unwrap();
    assert_eq!(
        destination.undo.last().unwrap().commands[0].id,
        (1, 4),
        "NEW_COMMAND_USES_NEW_EPOCH"
    );
}

#[test]
fn command_fates_survive_numeric_object_id_reuse() {
    let mut model = Model::new(Document::default(), usize::MAX, Policy::Fold);
    model.step_cap = 1;
    model.execute(&Command::Do(marker(1))).unwrap();
    model.execute(&Command::Undo).unwrap();
    model.execute(&Command::Do(marker(1))).unwrap();
    model.execute(&Command::Do(marker(2))).unwrap();
    assert_eq!(
        model.fates[&(0, 0)],
        Fate::Discarded,
        "ORIGINAL_COMMAND_STAYS_DISCARDED"
    );
    assert_eq!(
        model.fates[&(0, 2)],
        Fate::Folded,
        "REUSED_ID_COMMAND_FOLDED"
    );
    assert_eq!(model.position, 4, "POSITIONS_INCLUDE_UNDO");
}

#[test]
fn malformed_snapshot_install_is_typed_and_atomic() {
    let mut source = Model::new(Document::default(), 1000, Policy::NeverFold);
    source.execute(&Command::Do(marker(1))).unwrap();
    source.execute(&Command::Undo).unwrap();
    let mut destination = Model::new(Document::default(), 2000, Policy::Fold);
    destination.execute(&Command::Do(marker(2))).unwrap();
    let before = destination.clone();
    let mut invalid = source.snapshot();
    invalid.state.redo[0].commands[0].operations[0] = Operation::RemoveMarker {
        marker: MarkerId(999),
    };
    assert_eq!(
        destination.install_snapshot(invalid),
        Err(InstallRefusal::RedoReplay(
            kinewright_core::OpError::MissingMarker(MarkerId(999))
        )),
        "INSTALL_REFUSES_INVALID_REDO"
    );
    assert_eq!(destination, before, "INVALID_REDO_INSTALL_UNCHANGED");
    source.execute(&Command::Redo).unwrap();
    let mut invalid = source.snapshot();
    invalid.document.markers.clear();
    assert_eq!(
        destination.install_snapshot(invalid),
        Err(InstallRefusal::DocumentMismatch),
        "INSTALL_REFUSES_SAVED_DOCUMENT_MISMATCH"
    );
    assert_eq!(destination, before, "WRONG_DOCUMENT_INSTALL_UNCHANGED");
}

fn literal_baseline() -> Document {
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
fn literal_bytes_exact_fit_one_over_and_redo_tail() {
    // These are the literal lengths of the compact JSON, independent of Model::step_cost.
    assert_eq!(serde_json::to_vec(&lock(true)).unwrap().len(), 46);
    assert_eq!(serde_json::to_vec(&lock(false)).unwrap().len(), 47);
    for policy in [Policy::Fold, Policy::NeverFold] {
        for (budget, fits) in [(47, true), (46, false)] {
            let mut model = Model::new(literal_baseline(), budget, policy);
            let before = model.clone();
            let result = model.execute(&Command::Do(lock(false)));
            if fits {
                assert_eq!(result, Ok(0), "LITERAL_PLAIN_EXACT_FIT");
                assert_eq!(model.cost(), 47, "LITERAL_CHARGE_47");
            } else {
                assert_eq!(result, Err(Refusal::StepTooLarge), "LITERAL_PLAIN_ONE_OVER");
                assert_eq!(model, before);
            }
            let mut model = Model::new(literal_baseline(), budget, policy);
            model.execute(&Command::Do(lock(true))).unwrap();
            model.execute(&Command::Undo).unwrap();
            let before = model.clone();
            let result = model.execute(&Command::Do(lock(false)));
            if fits {
                assert_eq!(result, Ok(0), "LITERAL_REDO_EXACT_FIT");
                assert_eq!(model.cost(), 47);
                assert!(model.redo.is_empty());
            } else {
                assert_eq!(result, Err(Refusal::StepTooLarge), "LITERAL_REDO_ONE_OVER");
                assert_eq!(model, before, "LITERAL_REDO_REFUSAL_ATOMIC");
            }
        }
        for (budget, fits) in [(92, false), (93, true)] {
            let mut model = Model::new(literal_baseline(), budget, policy);
            model
                .execute(&Command::DoBatchCoalesced {
                    operations: vec![lock(true)],
                    coalesce_key: "drag".into(),
                })
                .unwrap();
            let before = model.clone();
            let result = model.execute(&Command::DoBatchCoalesced {
                operations: vec![lock(false)],
                coalesce_key: "drag".into(),
            });
            if fits {
                assert_eq!(result, Ok(0), "LITERAL_COALESCED_EXACT_FIT");
                assert_eq!(model.cost(), 93);
            } else {
                assert_eq!(
                    result,
                    Err(Refusal::StepTooLarge),
                    "LITERAL_COALESCED_ONE_OVER"
                );
                assert_eq!(model, before);
            }
        }
    }
}

#[test]
fn literal_budget_generated_streams_account_every_prefix() {
    let operations = [lock(true), lock(false)];
    for policy in [Policy::Fold, Policy::NeverFold] {
        for budget in [46, 47, 92, 93] {
            let mut model = Model::new(literal_baseline(), budget, policy);
            model.step_cap = 3;
            let mut random = Random(0x4858_0046_0047_0093);
            for index in 0..64 {
                let command = commands::plain_command(&command(
                    &mut random,
                    &[&operations[0], &operations[1]],
                    index,
                ));
                let before = model.clone();
                if model.execute(&command).is_err() {
                    assert_eq!(model, before, "LITERAL_STREAM_REFUSAL_ATOMIC");
                }
                let independent: usize = model
                    .undo
                    .iter()
                    .chain(&model.redo)
                    .flat_map(history_model::Step::operations)
                    .map(|op| serde_json::to_vec(op).unwrap().len())
                    .sum();
                assert_eq!(model.cost(), independent, "LITERAL_STREAM_ACCOUNTING");
                assert!(independent <= budget, "LITERAL_STREAM_BUDGET");
            }
        }
    }
}

#[test]
fn empty_batch_precedes_capacity_and_zero_step_cap_folds() {
    for policy in [Policy::Fold, Policy::NeverFold] {
        let mut model = Model::new(Document::default(), 0, policy);
        model.step_cap = 0;
        for command in [
            Command::DoBatch(vec![]),
            Command::DoBatchCoalesced {
                operations: vec![],
                coalesce_key: "empty".into(),
            },
        ] {
            let before = model.clone();
            assert_eq!(
                model.execute(&command),
                Err(Refusal::Batch(kinewright_core::BatchError::Empty)),
                "EMPTY_BEFORE_CAPACITY"
            );
            assert_eq!(model, before);
        }
    }
    let mut model = Model::new(Document::default(), 1000, Policy::Fold);
    model.step_cap = 0;
    assert_eq!(
        model.execute(&Command::Do(marker(1))),
        Ok(1),
        "ZERO_CAP_FOLDS_NEW_STEP"
    );
    assert_eq!(model.fates[&(0, 0)], Fate::Folded);
    assert!(model.undo.is_empty());
    assert_eq!(model.document().markers[0].id, MarkerId(1));
}

#[test]
fn noop_close_advances_position_without_changing_stacks() {
    let mut model = Model::new(Document::default(), 1000, Policy::Fold);
    for filled in [false, true] {
        if filled {
            model.execute(&Command::Do(marker(1))).unwrap();
        }
        let mut expected = model.clone();
        expected.position += 1;
        model.close_gesture();
        assert_eq!(model, expected, "NOOP_CLOSE_ADVANCES_ONLY_POSITION");
    }
}

#[test]
fn every_command_in_coalesced_step_has_a_fate() {
    let mut model = Model::new(Document::default(), 1000, Policy::Fold);
    for id in [1, 2] {
        model
            .execute(&Command::DoBatchCoalesced {
                operations: vec![marker(id)],
                coalesce_key: "drag".into(),
            })
            .unwrap();
    }
    for id in [(0, 0), (0, 1)] {
        assert_eq!(model.fates[&id], Fate::Undo, "COALESCED_APPLIED_FATES");
    }
    model.execute(&Command::Undo).unwrap();
    for id in [(0, 0), (0, 1)] {
        assert_eq!(model.fates[&id], Fate::Redo, "COALESCED_UNDONE_FATES");
    }
    model.execute(&Command::Redo).unwrap();
    for id in [(0, 0), (0, 1)] {
        assert_eq!(model.fates[&id], Fate::Undo, "COALESCED_REDONE_FATES");
    }
    model.execute(&Command::Undo).unwrap();
    model.execute(&Command::Do(marker(3))).unwrap();
    for id in [(0, 0), (0, 1)] {
        assert_eq!(
            model.fates[&id],
            Fate::Discarded,
            "COALESCED_DISCARDED_FATES"
        );
    }
}

#[test]
fn document_limit_refusal_is_atomic() {
    let baseline = literal_baseline();
    let mut model = Model::new(baseline.clone(), 1000, Policy::Fold);
    model.document_limit = serde_json::to_vec(&baseline).unwrap().len();
    let before = model.clone();
    assert_eq!(
        model.execute(&Command::Do(lock(false))),
        Err(Refusal::DocumentTooLarge),
        "DOCUMENT_LIMIT_REFUSAL"
    );
    assert_eq!(model, before, "DOCUMENT_LIMIT_ATOMIC");
}
