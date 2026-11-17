//! Independent H1 state/fate model and direct-operation lineage/resource replay.
use super::tests::{document, origin};
use super::*;
use crate::bundle_records::{ObjectRef, OccurrenceId, RecordKind};
use kinewright_core::{
    Command, Document, Operation, TrackId,
    undo::{CommandKey, History, RetentionPolicy, RoleBounds, StepEvidence},
};
use std::collections::{BTreeMap, BTreeSet};
#[path = "../../../kinewright-core/tests/support/history_commands.rs"]
mod commands;
#[path = "../../../kinewright-core/tests/support/history_model.rs"]
mod model;
#[path = "../../../kinewright-core/tests/support/history_oracle.rs"]
mod oracle;
fn key((epoch, position): (u64, u64)) -> CommandKey {
    CommandKey { epoch, position }
}
fn digest(operations: &[Operation]) -> [u8; 32] {
    let mut hash = kinewright_hash::Sha256::new();
    hash.update(&serde_json::to_vec(operations).unwrap());
    hash.finalize()
}
fn evidence(step: &model::Step) -> StepEvidence {
    StepEvidence::from_commands(
        step.commands
            .iter()
            .map(|command| (key(command.id), digest(&command.operations))),
    )
    .unwrap()
}
fn operations(command: &Command) -> Option<&[Operation]> {
    match command {
        Command::Do(op) => Some(std::slice::from_ref(op)),
        Command::DoBatch(ops)
        | Command::DoBatchCoalesced {
            operations: ops, ..
        } => Some(ops),
        _ => None,
    }
}
fn removed(before: &Document, after: &Document) -> Vec<RemovedTarget> {
    before
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .filter(|clip| after.clip(clip.id).is_none())
        .map(|clip| RemovedTarget::Clip(clip.id))
        .chain(
            before
                .tracks
                .iter()
                .filter(|track| !after.tracks.iter().any(|other| other.id == track.id))
                .map(|track| RemovedTarget::Track(track.id)),
        )
        .collect()
}
#[derive(Clone)]
struct Fact {
    creation: CreationOrigin,
    operations: Option<Vec<Operation>>,
    approved_do: bool,
    prefix: Option<StepEvidence>,
}
#[derive(Default, Debug)]
struct Totals {
    save_open_checks: usize,
    transitions: usize,
    approvals: usize,
    conditional_do: usize,
    conditional_batch: usize,
    uncertain: usize,
    provenance_probes: usize,
    sibling_do_missing_fold_provenance: usize,
}
// The H1 model supplies command state; direct-operation replay below supplies
// reachable documents and lineage independently of the owner implementation.
#[derive(Clone, Copy)]
enum Subject {
    Applied,
    Undone,
    Folded,
    Absent,
    ShapeUnknown,
    MissingFoldUnknown,
}
fn subject_state(
    model: &model::Model,
    target: StepEvidence,
    terminal: &BTreeMap<StepEvidence, Subject>,
    frontier: Option<CommandKey>,
    facts: &BTreeMap<CommandKey, Fact>,
) -> Subject {
    for (applied, steps) in [(true, &model.undo), (false, &model.redo)] {
        for step in steps {
            if step
                .commands
                .iter()
                .any(|command| key(command.id) == target.first_key)
            {
                let count = usize::try_from(target.commands).unwrap();
                let actual = StepEvidence::from_commands(
                    step.commands
                        .iter()
                        .take(count)
                        .map(|command| (key(command.id), digest(&command.operations))),
                );
                if actual != Some(target) {
                    return Subject::ShapeUnknown;
                }
                return if applied {
                    Subject::Applied
                } else {
                    Subject::Undone
                };
            }
        }
    }
    terminal.get(&target).copied().unwrap_or_else(|| {
        if facts.iter().any(|(key, fact)| {
            fact.approved_do
                && model.fates.get(&(key.epoch, key.position)) == Some(&model::Fate::Folded)
                && fact.prefix == Some(target)
        }) {
            return Subject::Folded;
        }
        if facts.get(&target.first_key).is_some_and(|fact| {
            fact.approved_do
                && model
                    .fates
                    .get(&(target.first_key.epoch, target.first_key.position))
                    == Some(&model::Fate::Folded)
                && fact
                    .prefix
                    .is_some_and(|prefix| prefix.first_key != target.first_key)
        }) {
            return Subject::ShapeUnknown;
        }
        if frontier.is_some_and(|frontier| target.first_key < frontier) {
            Subject::MissingFoldUnknown
        } else {
            Subject::Absent
        }
    })
}
fn control_fate(direction: Direction, subject: Subject) -> TransitionFate {
    match (direction, subject) {
        (_, Subject::ShapeUnknown) => TransitionFate::Uncertain(Uncertainty::ShapeMismatch),
        (_, Subject::MissingFoldUnknown) => {
            TransitionFate::Uncertain(Uncertainty::MissingFoldProvenance)
        }
        (Direction::Undo, Subject::Applied | Subject::Folded)
        | (Direction::Redo, Subject::Undone) => TransitionFate::Reversed,
        (Direction::Undo, Subject::Undone | Subject::Absent)
        | (Direction::Redo, Subject::Applied | Subject::Folded) => TransitionFate::Holds,
        (Direction::Redo, Subject::Absent) => TransitionFate::Discarded,
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "compare model state and explicit approval evidence at each accepted transition"
)]
fn check_audit(
    owner: &OwnerHistory,
    model: &model::Model,
    line: &BTreeMap<CommandKey, Fact>,
    all: &BTreeMap<CommandKey, Fact>,
    controls: &BTreeMap<ObjectRef, (Direction, StepEvidence, Vec<RemovedTarget>, CommandKey)>,
    trace: &[String],
    terminal: &BTreeMap<StepEvidence, Subject>,
    frontier: Option<CommandKey>,
    totals: &mut Totals,
) {
    for entry in owner.ledger().entries() {
        let expected = model.fates.get(&(entry.key.epoch, entry.key.position));
        assert!(
            match expected {
                Some(model::Fate::Undo) => matches!(entry.fate, Fate::Applied | Fate::Redone),
                Some(model::Fate::Redo) => entry.fate == Fate::Undone,
                Some(model::Fate::Folded) => entry.fate == Fate::Folded,
                Some(model::Fate::Discarded) | None => entry.fate == Fate::Discarded,
            },
            "EVERY_APPROVED_COMMAND_FATE {:?} {expected:?}",
            entry.key
        );
        assert_eq!(
            entry.payload_digest,
            digest(all[&entry.key].operations.as_ref().unwrap()),
            "ORIGINAL_APPROVAL_PAYLOAD_DIGEST"
        );
        assert_eq!(
            entry.answer(),
            if matches!(expected, Some(model::Fate::Undo | model::Fate::Folded)) {
                ApprovalAnswer::Kept
            } else {
                ApprovalAnswer::Missing
            },
            "EVERY_APPROVED_COMMAND_ANSWER_IS_ITS_FATE"
        );
    }
    for (id, (direction, subject, _, _)) in controls {
        let fate = owner.transition_fate(id).unwrap().unwrap();
        assert_eq!(
            fate,
            control_fate(
                *direction,
                subject_state(model, *subject, terminal, frontier, line)
            ),
            "EVERY_TRANSITION_APPROVAL_FATE {trace:?}"
        );
        if matches!(fate, TransitionFate::Uncertain(_)) {
            totals.uncertain += 1;
        }
    }
}

struct Capture {
    model: model::Snapshot,
    owner: OwnerSnapshot,
    line: BTreeMap<CommandKey, Fact>,
    seeds: OracleLineage,
    bounds: RoleBounds,
    terminal: BTreeMap<StepEvidence, Subject>,
    frontier: Option<CommandKey>,
}
#[expect(
    clippy::too_many_lines,
    reason = "one chronological stream keeps the independent model, approvals and snapshot cuts aligned"
)]
fn stream(
    kind: &str,
    document: &Document,
    first: Operation,
    policy: model::Policy,
    seed_value: u64,
    totals: &mut Totals,
) {
    let role = if policy == model::Policy::Fold {
        Role::Ordinary
    } else {
        Role::Task
    };
    let retention = if policy == model::Policy::Fold {
        RetentionPolicy::Fold
    } else {
        RetentionPolicy::NeverFold
    };
    let budget = if kind.starts_with("Synthetic") {
        8192
    } else {
        serde_json::to_vec(&first).unwrap().len() + 190
    };
    let bounds = RoleBounds {
        operation_bytes: budget as u64,
        step_cap: 3,
        checkpoint_documents: 1,
        ..role.bounds()
    };
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner =
        OwnerHistory::new(&pool, role, retention, &origin(1), || Ok(document.clone())).unwrap();
    owner.core = History::new(document.clone(), retention, bounds).unwrap();
    let mut model = model::Model::new(document.clone(), budget, policy);
    model.step_cap = 3;
    model.document_limit = usize::try_from(role.bounds().document_bytes).unwrap();
    let mut seeds = seed(document);
    let cases = oracle::operation_cases();
    let all_ops = cases
        .values()
        .map(|case| &case.operation)
        .collect::<Vec<_>>();
    let mut random = commands::Random(0x4858_210b_0000_0000 | seed_value);
    let mut save_random = commands::Random(0x5341_5645_0000_0000 | seed_value);
    let mut line = BTreeMap::<CommandKey, Fact>::new();
    let mut all = BTreeMap::new();
    let mut controls = BTreeMap::new();
    let mut terminal = BTreeMap::new();
    let mut frontier = None;
    let mut captured = None;
    let mut trace = vec![];
    let prefix = if kind == "SyntheticRecreation" {
        vec![
            Command::DoBatch(vec![Operation::RemoveTrack { track: TrackId(81) }]),
            Command::DoBatch(vec![Operation::AddTrack {
                track: super::tests::document().tracks.remove(0),
            }]),
            Command::Undo,
            Command::Undo,
        ]
    } else if kind == "SyntheticTransition" {
        vec![
            Command::DoBatch(vec![Operation::AddTrack {
                track: kinewright_core::Track {
                    id: TrackId(82),
                    kind: kinewright_core::TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                },
            }]),
            Command::Undo,
            Command::Redo,
            Command::DoBatch(vec![Operation::RemoveTrack { track: TrackId(82) }]),
            Command::Undo,
            Command::Redo,
            Command::Undo,
            Command::DoBatch(vec![Operation::RemoveTrack { track: TrackId(82) }]),
            Command::Undo,
            Command::Undo,
            Command::DoBatch(vec![commands::marker(700)]),
            Command::DoBatch(vec![Operation::RemoveTrack { track: TrackId(81) }]),
        ]
    } else {
        commands::initial_commands(Command::DoBatch(vec![first])).into()
    };
    for index in 0..prefix.len() + 12 {
        let mut installed = false;
        let context = format!("kind={kind} seed={seed_value} policy={policy:?} index={index}");
        if index == 8 {
            captured = Some(Capture {
                model: model.snapshot(),
                owner: owner.capture().unwrap(),
                line: line.clone(),
                seeds: seeds.clone(),
                bounds: owner.history().bounds(),
                terminal: terminal.clone(),
                frontier,
            });
            trace.push("capture S".into());
        }
        if index == 17 {
            if let Some(cut) = captured.take() {
                model.install_snapshot(cut.model).unwrap();
                owner
                    .install_snapshot(cut.owner.validate_bounded(cut.bounds).unwrap())
                    .unwrap();
                line = cut.line;
                seeds = cut.seeds;
                terminal = cut.terminal;
                frontier = cut.frontier;
                trace.push("install S".into());
                installed = true;
            }
        } else if index % 11 == 10 {
            model.close_gesture();
            owner.close_gesture().unwrap();
            trace.push("close gesture".into());
        } else {
            let generated = prefix
                .get(index)
                .cloned()
                .unwrap_or_else(|| commands::command(&mut random, &all_ops, index));
            let plain = commands::plain_command(&generated);
            let before = model.clone();
            let expected = model.execute(&plain);
            let target = match plain {
                Command::Undo => before
                    .undo
                    .last()
                    .map(evidence)
                    .map(|e| (Direction::Undo, e)),
                Command::Redo => before
                    .redo
                    .last()
                    .map(evidence)
                    .map(|e| (Direction::Redo, e)),
                _ => None,
            };
            let targets = if expected.is_ok() {
                removed(&before.document(), &model.document())
            } else {
                vec![]
            };
            let accepted = key((before.epoch, before.position));
            let creation = origin(10 + index as u64);
            let consent = ObjectRef {
                sha256: kinewright_media::sha256_bytes(
                    format!("{kind}:{seed_value}:{policy:?}:{index}").as_bytes(),
                ),
                byte_length: 1,
                record_kind: RecordKind::OwnedEvidence,
            };
            let do_approval = matches!(plain, Command::DoBatch(_)) && !targets.is_empty();
            let control_approval = target.is_some() && !targets.is_empty();
            let identity = owner.history().snapshot_identity().unwrap();
            let ledger_before = owner.ledger().clone();
            let actual = if control_approval {
                let (direction, evidence) = target.unwrap();
                owner.approve_transition(consent.clone(), direction, evidence)
            } else {
                owner.execute(
                    plain.clone(),
                    creation.clone(),
                    vec![],
                    do_approval.then(|| Approval {
                        reference: consent.clone(),
                        targets: targets.clone(),
                    }),
                )
            };
            assert_eq!(
                actual.is_ok(),
                expected.is_ok(),
                "OWNER_MODEL_OUTCOME {context} {expected:?} {actual:?}"
            );
            trace.push(format!(
                "{plain:?} approval={}",
                do_approval || control_approval
            ));
            if let Ok(folded) = expected {
                if index == 2 {
                    assert!(
                        actual.is_ok(),
                        "OWNER_EVERY_OPERATION_KIND_ACCEPTED {context}"
                    );
                }
                totals.conditional_do +=
                    usize::from(matches!(generated, Command::DoIfRevision { .. }));
                totals.conditional_batch +=
                    usize::from(matches!(generated, Command::DoBatchIfRevision { .. }));
                let prefix = operations(&plain).map(|ops| {
                    let prior = if let Command::DoBatchCoalesced { coalesce_key, .. } = &plain {
                        before
                            .undo
                            .last()
                            .filter(|step| step.coalesce_key.as_ref() == Some(coalesce_key))
                    } else {
                        None
                    };
                    StepEvidence::from_commands(
                        prior
                            .into_iter()
                            .flat_map(|step| &step.commands)
                            .map(|command| (key(command.id), digest(&command.operations)))
                            .chain(std::iter::once((accepted, digest(ops)))),
                    )
                    .unwrap()
                });
                let fact = Fact {
                    creation,
                    prefix,
                    operations: operations(&plain).map(<[Operation]>::to_vec),
                    approved_do: do_approval,
                };
                line.insert(accepted, fact.clone());
                all.insert(accepted, fact);
                if do_approval {
                    totals.approvals += 1;
                }
                if control_approval {
                    let (direction, subject) = target.unwrap();
                    controls.insert(consent, (direction, subject, targets, accepted));
                    totals.approvals += 1;
                }
                if let Some(ops) = operations(&plain) {
                    let stored = owner
                        .history()
                        .undo()
                        .iter()
                        .chain(owner.history().redo())
                        .flat_map(kinewright_core::undo::Step::commands)
                        .find(|command| command.id == accepted);
                    if let Some(stored) = stored {
                        assert_eq!(
                            &*stored.operations, ops,
                            "CONDITIONAL_ACCEPTANCE_STORED_AS_PLAIN {context}"
                        );
                    }
                }
                for (_, target, _, _) in controls.values() {
                    let was = subject_state(&before, *target, &terminal, frontier, &line);
                    let now = model
                        .fates
                        .get(&(target.first_key.epoch, target.first_key.position));
                    if matches!(
                        was,
                        Subject::Applied | Subject::Undone | Subject::ShapeUnknown
                    ) && !model
                        .undo
                        .iter()
                        .chain(&model.redo)
                        .flat_map(|step| &step.commands)
                        .any(|command| key(command.id) == target.first_key)
                    {
                        terminal.insert(
                            *target,
                            if matches!(was, Subject::ShapeUnknown) {
                                Subject::ShapeUnknown
                            } else if now == Some(&model::Fate::Folded) {
                                Subject::Folded
                            } else {
                                Subject::Absent
                            },
                        );
                    }
                }
                if folded > 0 {
                    let last = before.undo[..folded]
                        .last()
                        .unwrap()
                        .commands
                        .last()
                        .unwrap();
                    let candidate = CommandKey {
                        epoch: last.id.0,
                        position: last.id.1 + 1,
                    };
                    frontier =
                        Some(frontier.map_or(candidate, |old: CommandKey| old.max(candidate)));
                }
                advance_baseline(&before, folded, &mut seeds, &line);
            } else {
                assert_eq!(
                    owner.history().snapshot_identity().unwrap(),
                    identity,
                    "OWNER_REFUSAL_DOCUMENT_ATOMIC {context}"
                );
                assert_eq!(
                    owner.ledger(),
                    &ledger_before,
                    "OWNER_REFUSAL_LEDGER_ATOMIC {context}"
                );
            }
        }
        assert_eq!(
            owner.fold_frontier, frontier,
            "INDEPENDENT_FOLD_FRONTIER {context}"
        );
        reached(&model, &owner, &seeds, &line, &context);
        check_audit(
            &owner, &model, &line, &all, &controls, &trace, &terminal, frontier, totals,
        );
        if installed {
            assert!(
                owner
                    .ledger()
                    .entries()
                    .all(|entry| entry.answer() != ApprovalAnswer::Uncertain),
                "DIFFERENTIAL_SAME_LINE_DO_IS_CERTAIN"
            );
        }
        if installed || save_random.next().is_multiple_of(7) {
            super::roundtrip_tests::saved_open(&owner);
            totals.save_open_checks += 1;
        }
        totals.transitions += 1;
    }
}
fn preapproval_folded_snapshot_differential(totals: &mut Totals) {
    let pool = HistoryPool::new(PoolLimits::default());
    let bounds = RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    };
    let mut owner = OwnerHistory::measured(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        bounds,
        || Ok(document()),
    )
    .unwrap();
    let c = Command::DoBatch(vec![Operation::AddTrack {
        track: kinewright_core::Track {
            id: TrackId(82),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        },
    }]);
    owner.execute(c.clone(), origin(2), vec![], None).unwrap();
    let t = owner.capture().unwrap();
    owner
        .execute(Command::Do(commands::marker(300)), origin(3), vec![], None)
        .unwrap();
    let s = owner.capture().unwrap();
    owner
        .install_snapshot(t.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    let target = owner.transition_target(Direction::Undo).unwrap();
    let id = ObjectRef {
        sha256: "f".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::OwnedEvidence,
    };
    owner
        .approve_transition(id.clone(), Direction::Undo, target)
        .unwrap();
    owner
        .install_snapshot(s.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    let fate = owner.transition_fate(&id).unwrap().unwrap();
    assert_eq!(
        fate,
        TransitionFate::Uncertain(Uncertainty::MissingFoldProvenance),
        "DIFFERENTIAL_PREAPPROVAL_FOLD_IS_UNCERTAIN"
    );
    assert_eq!(
        fate.answer(),
        ApprovalAnswer::Uncertain,
        "DIFFERENTIAL_UNCERTAIN_IS_NEVER_A_CERTAIN_ANSWER"
    );
    totals.uncertain += 1;
    totals.approvals += 1;
    totals.provenance_probes += 1;
}
fn preapproval_do_folded_snapshot_differential(totals: &mut Totals) {
    let pool = HistoryPool::new(PoolLimits::default());
    let bounds = RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    };
    let mut owner = OwnerHistory::measured(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        bounds,
        || Ok(document()),
    )
    .unwrap();
    owner
        .execute(
            super::tests::remove(),
            origin(2),
            vec![],
            Some(super::tests::approval()),
        )
        .unwrap();
    owner
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    let t = owner.capture().unwrap();
    owner
        .execute(Command::Redo, origin(4), vec![], None)
        .unwrap();
    owner
        .execute(Command::Do(commands::marker(350)), origin(5), vec![], None)
        .unwrap();
    let s = owner.capture().unwrap();
    owner
        .install_snapshot(t.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    let id = ObjectRef {
        sha256: "e".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::OwnedEvidence,
    };
    owner
        .approve_transition(
            id.clone(),
            Direction::Redo,
            owner.transition_target(Direction::Redo).unwrap(),
        )
        .unwrap();
    owner
        .install_snapshot(s.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    let fate = owner.transition_fate(&id).unwrap().unwrap();
    assert_eq!(
        fate,
        TransitionFate::Holds,
        "DIFFERENTIAL_MATCHING_DO_FOLD_PREFIX_IS_EXACT"
    );
    totals.approvals += 2;
    totals.provenance_probes += 1;
}

fn sibling_do_snapshot_differential(totals: &mut Totals) {
    let pool = HistoryPool::new(PoolLimits::default());
    let bounds = RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    };
    let make = || {
        OwnerHistory::measured(
            &pool,
            Role::Ordinary,
            RetentionPolicy::Fold,
            &origin(1),
            bounds,
            || Ok(document()),
        )
        .unwrap()
    };
    let mut owner = make();
    let mut sibling = make();
    sibling
        .execute(super::tests::remove(), origin(2), vec![], None)
        .unwrap();
    let folded = sibling
        .execute(Command::Do(commands::marker(399)), origin(3), vec![], None)
        .unwrap();
    let selected = sibling.capture().unwrap();
    let key = owner
        .execute(
            super::tests::remove(),
            origin(2),
            vec![],
            Some(super::tests::approval()),
        )
        .unwrap()
        .accepted;
    assert!(
        folded.folded.iter().any(|command| command.key == key),
        "DIFFERENTIAL_SIBLING_EXACT_COMMAND_FOLDED"
    );
    owner
        .install_snapshot(selected.validate_bounded(bounds).unwrap())
        .unwrap();
    assert_eq!(
        owner.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Uncertain,
        "DIFFERENTIAL_SIBLING_DO_MISSING_FOLD_PROVENANCE"
    );
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    totals.sibling_do_missing_fold_provenance += 1;
    totals.uncertain += 1;
    totals.approvals += 1;
}

#[test]
fn seeded_h1_streams_match_fates_lineage_and_retention() {
    let mut totals = Totals::default();
    for (kind, case) in oracle::operation_cases() {
        for policy in [model::Policy::Fold, model::Policy::NeverFold] {
            for seed in 1..=2 {
                stream(
                    &kind,
                    &case.document,
                    case.operation.clone(),
                    policy,
                    seed,
                    &mut totals,
                );
            }
        }
    }
    for policy in [model::Policy::Fold, model::Policy::NeverFold] {
        stream(
            "SyntheticTransition",
            &document(),
            Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            },
            policy,
            1,
            &mut totals,
        );
    }
    for policy in [model::Policy::Fold, model::Policy::NeverFold] {
        stream(
            "SyntheticRecreation",
            &document(),
            Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            },
            policy,
            1,
            &mut totals,
        );
    }
    sibling_do_snapshot_differential(&mut totals);
    assert!(
        totals.sibling_do_missing_fold_provenance > 0,
        "SIBLING_DO_UNCERTAINTY_CLASS_EXERCISED"
    );
    preapproval_folded_snapshot_differential(&mut totals);
    preapproval_do_folded_snapshot_differential(&mut totals);
    assert!(
        totals.uncertain > 0,
        "DIFFERENTIAL_UNCERTAIN_ANSWER_IS_EXERCISED"
    );
    assert!(
        totals.save_open_checks > 0,
        "DIFFERENTIAL_SAVE_OPEN_INVARIANT_EXERCISED"
    );
    assert!(
        totals.conditional_do > 0 && totals.conditional_batch > 0,
        "GENERATED_ACCEPTED_CONDITIONAL_COMMANDS"
    );
    let artifact = serde_json::json!({"sibling_do_missing_fold_provenance":totals.sibling_do_missing_fold_provenance,"save_open_checks":totals.save_open_checks,"transitions":totals.transitions,"approvals":totals.approvals,"accepted_do_if_revision":totals.conditional_do,"accepted_batch_if_revision":totals.conditional_batch,"uncertain_observations":totals.uncertain,"folded_provenance_probes":totals.provenance_probes});
    let directory = kinewright_media::test_support::workspace_target_dir().join("hx-part2");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("owner-differential.json"),
        serde_json::to_vec_pretty(&artifact).unwrap(),
    )
    .unwrap();
    println!("HX_OWNER_DIFFERENTIAL={artifact}");
}

#[derive(Clone)]
struct OracleLineage {
    baseline: BTreeMap<kinewright_core::ClipId, OccurrenceId>,
    current: BTreeMap<kinewright_core::ClipId, OccurrenceId>,
    split_ancestry: Vec<(OccurrenceId, OccurrenceId, kinewright_core::Clip)>,
}
fn seed(document: &Document) -> OracleLineage {
    let origin = origin(1);
    let current = document
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .enumerate()
        .map(|(ordinal, clip)| {
            (
                clip.id,
                OccurrenceId {
                    branch_origin: origin.branch.clone(),
                    creation_batch_id: origin.batch.clone(),
                    result_ordinal: ordinal as u64,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    OracleLineage {
        baseline: current.clone(),
        current,
        split_ancestry: vec![],
    }
}
fn reference(
    baseline: &Document,
    steps: &[&model::Step],
    depth: usize,
    seeds: &OracleLineage,
    facts: &BTreeMap<CommandKey, Fact>,
) -> (Document, OracleLineage) {
    let mut document = baseline.clone();
    let mut lineage = seeds.clone();
    lineage.current = lineage.baseline.clone();
    for step in steps.iter().take(depth) {
        for command in &step.commands {
            let origin = &facts[&key(command.id)].creation;
            let mut ordinal = 0;
            for operation in &command.operations {
                let before = document
                    .tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .map(|c| c.id)
                    .collect::<BTreeSet<_>>();
                let parent = if let Operation::SplitClip { clip, .. } = operation {
                    Some((*clip, lineage.current[clip].clone()))
                } else {
                    None
                };
                operation.apply(&mut document).unwrap();
                lineage.current.retain(|id, _| document.clip(*id).is_some());
                let children = document
                    .tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .filter(|c| {
                        parent.as_ref().is_some_and(|(id, _)| c.id == *id)
                            || !before.contains(&c.id)
                    })
                    .collect::<Vec<_>>();
                // Ordinary created IDs have sorted kernel order; split children
                // have timeline traversal order, including the reused parent ID.
                let mut ordinary = children
                    .iter()
                    .filter(|c| parent.is_none() && !before.contains(&c.id))
                    .map(|c| c.id)
                    .collect::<Vec<_>>();
                ordinary.sort_unstable();
                for id in ordinary {
                    lineage.current.insert(
                        id,
                        OccurrenceId {
                            branch_origin: origin.branch.clone(),
                            creation_batch_id: origin.batch.clone(),
                            result_ordinal: ordinal,
                        },
                    );
                    ordinal += 1;
                }
                if let Some((_, parent)) = &parent {
                    for child in children {
                        let occurrence = OccurrenceId {
                            branch_origin: origin.branch.clone(),
                            creation_batch_id: origin.batch.clone(),
                            result_ordinal: ordinal,
                        };
                        ordinal += 1;
                        lineage.split_ancestry.push((
                            occurrence.clone(),
                            parent.clone(),
                            child.clone(),
                        ));
                        lineage.current.insert(child.id, occurrence);
                    }
                }
            }
        }
    }
    let mut live = lineage.current.values().cloned().collect::<BTreeSet<_>>();
    loop {
        let n = live.len();
        for (child, parent, _) in &lineage.split_ancestry {
            if live.contains(child) {
                live.insert(parent.clone());
            }
        }
        if live.len() == n {
            break;
        }
    }
    lineage
        .split_ancestry
        .retain(|(child, _, _)| live.contains(child));
    (document, lineage)
}
fn advance_baseline(
    before: &model::Model,
    count: usize,
    seeds: &mut OracleLineage,
    facts: &BTreeMap<CommandKey, Fact>,
) {
    if count == 0 {
        return;
    }
    let steps = before.undo[..count].iter().collect::<Vec<_>>();
    let (_, mut lineage) = reference(&before.baseline, &steps, count, seeds, facts);
    lineage.baseline = lineage.current.clone();
    *seeds = lineage;
}
fn reached(
    model: &model::Model,
    history: &OwnerHistory,
    seeds: &OracleLineage,
    facts: &BTreeMap<CommandKey, Fact>,
    context: &str,
) {
    let steps = model
        .undo
        .iter()
        .chain(model.redo.iter().rev())
        .collect::<Vec<_>>();
    assert_eq!(
        **history.history().document(),
        model.document(),
        "OWNER_MODEL_DOCUMENT {context}"
    );
    assert_eq!(
        **history.history().baseline(),
        model.baseline,
        "OWNER_MODEL_BASELINE {context}"
    );
    assert_eq!(
        history.history().position(),
        key((model.epoch, model.position)),
        "OWNER_MODEL_COUNTER {context}"
    );
    assert_eq!(
        history.seeds().occurrences,
        seeds.baseline,
        "FOLD_SEEDS_EQUAL_INDEPENDENT_LINEAGE {context}"
    );
    for depth in 0..=steps.len() {
        let (document, lineage) = reference(&model.baseline, &steps, depth, seeds, facts);
        let actual = history.boundary(depth).unwrap();
        assert_eq!(
            actual.document(),
            &document,
            "EVERY_REACHABLE_MODEL_DOCUMENT {context} depth={depth}"
        );
        assert_eq!(
            actual.lineage().occurrences,
            lineage.current,
            "EVERY_REACHABLE_MODEL_LINEAGE {context} depth={depth}"
        );
        assert_eq!(
            actual.lineage().split_ancestry,
            lineage.split_ancestry,
            "EVERY_REACHABLE_MODEL_ANCESTRY {context} depth={depth}"
        );
        // Inspect actual model documents rather than the production Resources scanner.
        for asset in &document.media_pool {
            assert!(
                history.resources().contains(&Resource::Media {
                    asset: asset.id.0,
                    sha256: asset.source_fingerprint.content_sha256.clone(),
                    byte_len: asset.source_fingerprint.byte_len
                }),
                "EVERY_REACHABLE_MEDIA_RESOURCE {context}"
            );
        }
        for asset in &document.lut_assets {
            assert!(
                history.resources().contains(&Resource::Lut {
                    sha256: asset.sha256.clone(),
                    byte_len: asset.byte_len
                }),
                "EVERY_REACHABLE_LUT_RESOURCE {context}"
            );
        }
        if document
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .any(|c| matches!(c.content, kinewright_core::ClipContent::Title(_)))
        {
            assert!(
                history.resources().contains(&Resource::TitleFont),
                "EVERY_REACHABLE_FONT_RESOURCE {context}"
            );
        }
    }
}
