//! Independent H1 model, old audit, old lineage and old resource-retention oracles.
use super::tests::{document, origin};
use super::*;
use crate::bundle_records::{BatchId, LineageRecord, ObjectRef, OccurrenceId, RecordKind};
use kinewright_core::{
    Command, Core, Document, Event, HistorySnapshot, Operation, TrackId,
    undo::{CommandKey, History, RetentionPolicy, RoleBounds, StepEvidence},
};
use std::collections::BTreeMap;
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
fn targets_present(targets: &[RemovedTarget], document: &Document) -> bool {
    targets.iter().any(|target| match target {
        RemovedTarget::Clip(id) => document.clip(*id).is_some(),
        RemovedTarget::Track(id) => document.tracks.iter().any(|track| track.id == *id),
    })
}
#[derive(Clone)]
struct Fact {
    creation: CreationOrigin,
    operations: Option<Vec<Operation>>,
    removed: Vec<RemovedTarget>,
    created: Vec<RemovedTarget>,
    old_position: usize,
    control: Option<(Direction, StepEvidence)>,
    approved_do: bool,
    prefix: Option<StepEvidence>,
}
#[derive(Default, Debug)]
struct Totals {
    save_open_checks: usize,
    transitions: usize,
    approvals: usize,
    shortcut_exemptions: usize,
    conditional_do: usize,
    conditional_batch: usize,
    uncertain: usize,
    provenance_probes: usize,
    control_certain_differences: BTreeMap<String, Vec<String>>,
    control_difference_observations: usize,
}
fn seed(document: &Document) -> LineageRecord {
    let base = origin(1);
    let map = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .enumerate()
        .map(|(ordinal, clip)| {
            (
                clip.id,
                OccurrenceId {
                    branch_origin: base.branch.clone(),
                    creation_batch_id: base.batch.clone(),
                    result_ordinal: ordinal as u64,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    LineageRecord {
        branch_origin: base.branch,
        baseline: map.clone(),
        current: map,
        undo: vec![],
        redo: vec![],
        split_ancestry: vec![],
    }
}
fn origin_record(facts: &BTreeMap<CommandKey, Fact>) -> LineageRecord {
    let mut record = seed(&Document::default());
    record.current = facts
        .values()
        .enumerate()
        .map(|(index, fact)| {
            (
                kinewright_core::ClipId(index as u64 + 1),
                OccurrenceId {
                    branch_origin: fact.creation.branch.clone(),
                    creation_batch_id: fact.creation.batch.clone(),
                    result_ordinal: 0,
                },
            )
        })
        .collect();
    record
}
// A separate old Core rebuilds every reachable state. Its payloads are copies
// for the oracle actor; none is shared with the owner/model under test.
fn reference(
    baseline: &Document,
    steps: &[&model::Step],
    depth: usize,
    seeds: &LineageRecord,
    facts: &BTreeMap<CommandKey, Fact>,
) -> (HistorySnapshot, LineageRecord) {
    let core = Core::spawn(baseline.clone()).unwrap();
    let mut batches = Vec::<BatchId>::new();
    for (index, step) in steps.iter().enumerate() {
        for command in &step.commands {
            let edit = if step.commands.len() > 1 || step.coalesce_key.is_some() {
                Command::DoBatchCoalesced {
                    operations: command.operations.clone(),
                    coalesce_key: format!("oracle-{index}"),
                }
            } else {
                Command::DoBatch(command.operations.clone())
            };
            assert!(
                matches!(core.request(edit).unwrap(), Event::DocumentChanged { .. }),
                "OLD_REFERENCE_EDIT"
            );
            batches.push(facts[&key(command.id)].creation.batch.clone());
        }
    }
    for _ in depth..steps.len() {
        core.request(Command::Undo).unwrap();
        batches.push(origin(900_000).batch);
    }
    let history = core.export_history().unwrap();
    let lineage = crate::bundle_commit::lineage(
        &origin(1).branch,
        &history,
        &batches,
        Some(seeds),
        Some(&origin_record(facts)),
    )
    .unwrap();
    (history, lineage)
}
fn advance_baseline(
    before: &model::Model,
    count: usize,
    seeds: &mut LineageRecord,
    facts: &BTreeMap<CommandKey, Fact>,
) {
    if count == 0 {
        return;
    }
    let steps = before.undo[..count].iter().collect::<Vec<_>>();
    let (_, lineage) = reference(&before.baseline, &steps, count, seeds, facts);
    seeds.baseline = lineage.current.clone();
    seeds.current = lineage.current;
    seeds.split_ancestry = lineage.split_ancestry;
    seeds.undo.clear();
    seeds.redo.clear();
}
fn reached(
    model: &model::Model,
    history: &OwnerHistory,
    seeds: &LineageRecord,
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
        "FOLD_SEEDS_EQUAL_OLD_LINEAGE {context}"
    );
    for depth in 0..=steps.len() {
        let (old, lineage) = reference(&model.baseline, &steps, depth, seeds, facts);
        let actual = history.boundary(depth).unwrap();
        assert_eq!(
            actual.document(),
            &*old.document,
            "EVERY_REACHABLE_OLD_DOCUMENT {context} depth={depth}"
        );
        assert_eq!(
            actual.lineage().occurrences,
            lineage.current,
            "EVERY_REACHABLE_OLD_LINEAGE {context} depth={depth}"
        );
        assert_eq!(
            actual.lineage().split_ancestry,
            lineage.split_ancestry,
            "EVERY_REACHABLE_OLD_ANCESTRY {context} depth={depth}"
        );
        for resource in Resources::document(&old.document) {
            assert!(
                history.resources().contains(&resource),
                "EVERY_REACHABLE_RESOURCE_SUPERSET {context} {resource:?}"
            );
        }
        let mut inputs = old_inputs(&old);
        crate::bundle_commit::retain_history_inputs(&old, &mut inputs);
        for entry in inputs.entries {
            let required = match entry.object.record_kind {
                RecordKind::OwnedLut => Resource::Lut {
                    sha256: entry.object.sha256,
                    byte_len: entry.object.byte_length,
                },
                RecordKind::OwnedFont => Resource::TitleFont,
                RecordKind::OwnedMedia => Resource::Media {
                    asset: entry.entry_id.split(':').nth(1).unwrap().parse().unwrap(),
                    sha256: Some(entry.object.sha256),
                    byte_len: Some(entry.object.byte_length),
                },
                _ => continue,
            };
            assert!(
                history.resources().contains(&required),
                "OLD_RETENTION_RESOURCE_SUPERSET {context}"
            );
        }
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "build genuine candidate manifest entries for every resource category before calling old retention"
)]
fn old_inputs(history: &HistorySnapshot) -> crate::owned_inputs::OwnedInputManifest {
    let mut entries = vec![];
    for doc in std::iter::once(&history.baseline)
        .chain(std::iter::once(&history.document))
        .chain(
            history
                .cursor
                .undo
                .iter()
                .chain(&history.cursor.redo)
                .map(|entry| &entry.document),
        )
    {
        for asset in &doc.media_pool {
            let (Some(sha256), Some(byte_length)) = (
                &asset.source_fingerprint.content_sha256,
                asset.source_fingerprint.byte_len,
            ) else {
                continue;
            };
            let video = matches!(
                asset.kind,
                kinewright_core::MediaKind::Video
                    | kinewright_core::MediaKind::AudioVideo
                    | kinewright_core::MediaKind::Image
            );
            let audio = matches!(
                asset.kind,
                kinewright_core::MediaKind::Audio | kinewright_core::MediaKind::AudioVideo
            );
            let stream = |index| crate::owned_media::StreamInterpretation {
                index,
                time_base: (1, 48000),
                timestamp_origin: 0,
            };
            let interpretation = crate::owned_media::MediaInterpretation {
                version: 1,
                video: video.then(|| stream(0)),
                audio: audio.then(|| stream(1)),
                fps: asset.fps,
                duration: asset.duration,
                kind: asset.kind,
                resolution: asset.resolution,
                effective_color: asset.color_description.clone(),
                probed_color: asset.color_description.clone(),
                source_profile_assumption: if video {
                    kinewright_media::source_profile_assumption(&asset.color_description)
                } else {
                    None
                },
            };
            let profile = interpretation.canonical().unwrap();
            let entry = crate::owned_inputs::OwnedInputEntry {
                entry_id: format!(
                    "media:{}:{sha256}:{byte_length}:{}",
                    asset.id.0,
                    kinewright_media::sha256_bytes(profile.as_bytes())
                ),
                object: ObjectRef {
                    sha256: sha256.clone(),
                    byte_length,
                    record_kind: RecordKind::OwnedMedia,
                },
                dependencies: vec![ObjectRef {
                    sha256: "d".repeat(64),
                    byte_length: 1,
                    record_kind: RecordKind::InputChunkTable,
                }],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: Some(profile),
            };
            assert!(
                entry.matches_media_asset(asset),
                "OLD_MEDIA_RETENTION_CANDIDATE_IS_EXACT"
            );
            entries.push(entry);
        }
        for asset in &doc.lut_assets {
            entries.push(crate::owned_inputs::OwnedInputEntry {
                entry_id: format!("lut:{}:{}:{}", asset.id.0, asset.sha256, asset.byte_len),
                object: ObjectRef {
                    sha256: asset.sha256.clone(),
                    byte_length: asset.byte_len,
                    record_kind: RecordKind::OwnedLut,
                },
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            });
        }
        if doc
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .any(|clip| matches!(clip.content, kinewright_core::ClipContent::Title(_)))
        {
            entries.push(crate::owned_inputs::OwnedInputEntry {
                entry_id: "font:Inter".into(),
                object: ObjectRef {
                    sha256: kinewright_media::sha256_bytes(kinewright_core::title_font_bytes()),
                    byte_length: kinewright_core::title_font_bytes().len() as u64,
                    record_kind: RecordKind::OwnedFont,
                },
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            });
        }
    }
    entries.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
    entries.dedup();
    crate::owned_inputs::OwnedInputManifest {
        project_id: None,
        session_id: None,
        owner_epoch: 0,
        entries,
    }
}
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
    clippy::too_many_lines,
    clippy::too_many_arguments,
    reason = "compare the three independent actors and explicit evidence, retaining each permitted discrepancy witness"
)]
fn check_audit(
    owner: &OwnerHistory,
    model: &model::Model,
    old: &Core,
    line: &BTreeMap<CommandKey, Fact>,
    all: &BTreeMap<CommandKey, Fact>,
    controls: &BTreeMap<ObjectRef, (Direction, StepEvidence, Vec<RemovedTarget>, CommandKey)>,
    trace: &[String],
    terminal: &BTreeMap<StepEvidence, Subject>,
    frontier: Option<CommandKey>,
    totals: &mut Totals,
) {
    let old = old.export_history().unwrap();
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
        let recreated = line.iter().any(|(key, fact)| {
            *key > entry.key
                && entry
                    .approval
                    .targets
                    .iter()
                    .any(|target| fact.created.contains(target))
        });
        assert_eq!(
            entry.recreated, recreated,
            "GENERATED_RECREATION_EVIDENCE {:?}",
            entry.key
        );
        let fact = line.get(&entry.key);
        let answer = crate::edit_audit::owner_differential_proof(
            &old,
            fact.map(|fact| fact.old_position),
            fact.and_then(|fact| fact.operations.as_deref()),
            &entry.approval.targets,
            fact.is_some(),
        );
        if answer != ApprovalAnswer::Uncertain && answer != entry.answer() {
            assert!(
                !targets_present(&entry.approval.targets, &old.document),
                "EXEMPTION_IS_TARGET_ABSENCE_SHORTCUT"
            );
            assert_eq!(
                answer,
                ApprovalAnswer::Kept,
                "ONLY_OLD_TARGET_SHORTCUT_IS_EXEMPT"
            );
            assert!(
                !matches!(expected, Some(model::Fate::Undo | model::Fate::Folded)),
                "EXEMPT_APPROVAL_IS_NOT_APPLIED"
            );
            let other = entry.approval.targets.iter().all(|target| {
                line.iter().any(|(key, fact)| {
                    *key != entry.key
                        && fact.removed.contains(target)
                        && match fact.control {
                            None => matches!(
                                model.fates.get(&(key.epoch, key.position)),
                                Some(model::Fate::Undo | model::Fate::Folded)
                            ),
                            Some((Direction::Undo, subject)) => matches!(
                                model
                                    .fates
                                    .get(&(subject.first_key.epoch, subject.first_key.position)),
                                Some(model::Fate::Redo | model::Fate::Discarded)
                            ),
                            Some((Direction::Redo, subject)) => matches!(
                                model
                                    .fates
                                    .get(&(subject.first_key.epoch, subject.first_key.position)),
                                Some(model::Fate::Undo | model::Fate::Folded)
                            ),
                        }
                })
            });
            assert!(
                other,
                "EXEMPTION_REQUIRES_DIFFERENT_REMOVING_COMMAND {:?} {trace:?}",
                entry.key
            );
            totals.shortcut_exemptions += 1;
        }
    }
    for (id, (direction, subject, targets, accepted)) in controls {
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
        let fact = line.get(accepted);
        let answer = crate::edit_audit::owner_differential_proof(
            &old,
            fact.map(|fact| fact.old_position),
            None,
            targets,
            fact.is_some(),
        );
        if answer != ApprovalAnswer::Uncertain && answer != fate.answer() {
            // These contract differences are collected and reported, never
            // treated as Do-command differential exemptions.
            assert!(
                (answer == ApprovalAnswer::Kept
                    && fact.is_some()
                    && !targets_present(targets, &old.document))
                    || (answer == ApprovalAnswer::Missing && fact.is_none()),
                "CONTROL_DIFFERENCE_HAS_EXPLICIT_OLD_CAUSE"
            );
            totals.control_difference_observations += 1;
            let reason = format!("{direction:?}/{fate:?}/old={answer:?}");
            totals
                .control_certain_differences
                .entry(reason)
                .or_insert_with(|| trace.to_vec());
        }
        let _ = subject;
    }
}

struct Capture {
    model: model::Snapshot,
    owner: OwnerSnapshot,
    old: HistorySnapshot,
    line: BTreeMap<CommandKey, Fact>,
    seeds: LineageRecord,
    bounds: RoleBounds,
    terminal: BTreeMap<StepEvidence, Subject>,
    frontier: Option<CommandKey>,
}
#[expect(
    clippy::too_many_lines,
    reason = "one chronological stream keeps model, old actor, approvals and snapshot cuts aligned"
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
    let budget = if kind == "SyntheticTransition" {
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
    let old = Core::spawn(document.clone()).unwrap();
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
    let prefix = if kind == "SyntheticTransition" {
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
                old: old.export_history().unwrap(),
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
                old.restore_history(cut.old).unwrap();
                line = cut.line;
                seeds = cut.seeds;
                terminal = cut.terminal;
                frontier = cut.frontier;
                trace.push("install S".into());
                installed = true;
            }
        } else if index % 11 == 10 {
            let current = old.export_history().unwrap();
            if current
                .cursor
                .undo
                .last()
                .is_some_and(|step| step.coalesce_key.is_some())
                && !model.undo.is_empty()
            {
                old.request(Command::Undo).unwrap();
                old.request(Command::Redo).unwrap();
            }
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
            let old_before = old.export_history().unwrap();
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
                // Old Core keeps the folded prefix for the audit oracle. A
                // modeled empty Undo must not pop that immutable prefix.
                let effective = !matches!(plain, Command::Undo) || !before.undo.is_empty();
                let effective =
                    effective && (!matches!(plain, Command::Redo) || !before.redo.is_empty());
                let mut old_command = generated.clone();
                if matches!(
                    generated,
                    Command::DoIfRevision { .. } | Command::DoBatchIfRevision { .. }
                ) {
                    let Event::QueryResult(kinewright_core::QueryResult::Snapshot {
                        revision, ..
                    }) = old
                        .request(Command::Query(kinewright_core::Query::Snapshot))
                        .unwrap()
                    else {
                        unreachable!()
                    };
                    match &mut old_command {
                        Command::DoIfRevision { expected, .. } => {
                            *expected = revision;
                            totals.conditional_do += 1;
                        }
                        Command::DoBatchIfRevision { expected, .. } => {
                            *expected = revision;
                            totals.conditional_batch += 1;
                        }
                        _ => unreachable!(),
                    }
                }
                if effective {
                    assert!(
                        matches!(
                            old.request(old_command).unwrap(),
                            Event::DocumentChanged { .. }
                        ),
                        "OLD_ACCEPTED_STREAM {context}"
                    );
                }
                let old_after = old.export_history().unwrap();
                assert_eq!(
                    *old_after.document,
                    model.document(),
                    "OLD_CURRENT_MATCHES_MODEL {context}"
                );
                let mut deleted = targets.clone();
                let mut created = vec![];
                if let Some(ops) = operations(&plain) {
                    let identities = &old_after.batches.last().unwrap().identity_effects;
                    deleted.extend(
                        identities
                            .iter()
                            .flat_map(|effect| effect.deleted.iter())
                            .copied()
                            .map(RemovedTarget::Clip),
                    );
                    created.extend(
                        identities
                            .iter()
                            .flat_map(|effect| {
                                effect
                                    .created
                                    .iter()
                                    .copied()
                                    .chain(effect.split_results.iter().map(|clip| clip.id))
                            })
                            .map(RemovedTarget::Clip),
                    );
                    created.extend(ops.iter().filter_map(|op| {
                        if let Operation::AddTrack { track } = op {
                            Some(RemovedTarget::Track(track.id))
                        } else {
                            None
                        }
                    }));
                }
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
                    removed: deleted,
                    created,
                    old_position: old_before.batches.len(),
                    control: target,
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
            &owner, &model, &old, &line, &all, &controls, &trace, &terminal, frontier, totals,
        );
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
    let old = Core::spawn(document()).unwrap();
    let c = Command::DoBatch(vec![Operation::AddTrack {
        track: kinewright_core::Track {
            id: TrackId(82),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        },
    }]);
    owner.execute(c.clone(), origin(2), vec![], None).unwrap();
    old.request(c).unwrap();
    let t = owner.capture().unwrap();
    let old_t = old.export_history().unwrap();
    owner
        .execute(Command::Do(commands::marker(300)), origin(3), vec![], None)
        .unwrap();
    old.request(Command::Do(commands::marker(300))).unwrap();
    let s = owner.capture().unwrap();
    let old_s = old.export_history().unwrap();
    owner
        .install_snapshot(t.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    old.restore_history(old_t).unwrap();
    let target = owner.transition_target(Direction::Undo).unwrap();
    let id = ObjectRef {
        sha256: "f".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::OwnedEvidence,
    };
    owner
        .approve_transition(id.clone(), Direction::Undo, target)
        .unwrap();
    old.request(Command::Undo).unwrap();
    owner
        .install_snapshot(s.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    old.restore_history(old_s).unwrap();
    let fate = owner.transition_fate(&id).unwrap().unwrap();
    assert_eq!(
        fate,
        TransitionFate::Uncertain(Uncertainty::MissingFoldProvenance),
        "DIFFERENTIAL_PREAPPROVAL_FOLD_IS_UNCERTAIN"
    );
    let answer = crate::edit_audit::owner_differential_proof(
        &old.export_history().unwrap(),
        None,
        None,
        &[RemovedTarget::Track(TrackId(82))],
        false,
    );
    assert_eq!(
        answer,
        ApprovalAnswer::Missing,
        "OLD_UNSELECTED_APPROVAL_CUT_IS_MISSING"
    );
    assert_eq!(
        fate.answer(),
        ApprovalAnswer::Uncertain,
        "DIFFERENTIAL_UNCERTAIN_IS_NEVER_A_CERTAIN_ANSWER"
    );
    totals.uncertain += 1;
    totals.approvals += 1;
    totals.provenance_probes += 1;
    totals.control_difference_observations += 1;
    totals.control_certain_differences.insert(
        "Undo/Uncertain(MissingFoldProvenance)/old=Missing".into(),
        [
            "Do C=AddTrack82",
            "capture T",
            "Do marker folds C; capture S before approval",
            "install T",
            "approve Undo C",
            "install S",
        ]
        .map(str::to_owned)
        .into(),
    );
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
    let old = Core::spawn(document()).unwrap();
    owner
        .execute(
            super::tests::remove(),
            origin(2),
            vec![],
            Some(super::tests::approval()),
        )
        .unwrap();
    old.request(super::tests::remove()).unwrap();
    owner
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    old.request(Command::Undo).unwrap();
    let t = owner.capture().unwrap();
    let old_t = old.export_history().unwrap();
    owner
        .execute(Command::Redo, origin(4), vec![], None)
        .unwrap();
    old.request(Command::Redo).unwrap();
    owner
        .execute(Command::Do(commands::marker(350)), origin(5), vec![], None)
        .unwrap();
    old.request(Command::Do(commands::marker(350))).unwrap();
    let s = owner.capture().unwrap();
    let old_s = old.export_history().unwrap();
    owner
        .install_snapshot(t.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    old.restore_history(old_t).unwrap();
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
    old.request(Command::Redo).unwrap();
    owner
        .install_snapshot(s.validate_bounded(bounds).unwrap())
        .unwrap();
    super::roundtrip_tests::saved_open(&owner);
    totals.save_open_checks += 1;
    old.restore_history(old_s).unwrap();
    let fate = owner.transition_fate(&id).unwrap().unwrap();
    assert_eq!(
        fate,
        TransitionFate::Holds,
        "DIFFERENTIAL_MATCHING_DO_FOLD_PREFIX_IS_EXACT"
    );
    let answer = crate::edit_audit::owner_differential_proof(
        &old.export_history().unwrap(),
        None,
        None,
        &[RemovedTarget::Track(TrackId(81))],
        false,
    );
    assert_eq!(
        answer,
        ApprovalAnswer::Missing,
        "OLD_PREAPPROVAL_REDO_CUT_IS_MISSING"
    );
    totals.approvals += 2;
    totals.provenance_probes += 1;
    totals.control_difference_observations += 1;
    totals.control_certain_differences.insert(
        "Redo/Holds/old=Missing".into(),
        [
            "approve Do C=RemoveTrack81",
            "Undo C; capture T",
            "Redo C",
            "Do marker folds C; capture S before Redo approval",
            "install T",
            "approve Redo C",
            "install S; matching Do prefix proof survives",
        ]
        .map(str::to_owned)
        .into(),
    );
}
#[test]
fn seeded_h1_streams_match_old_audit_lineage_and_retention() {
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
        totals.shortcut_exemptions > 0,
        "TARGET_SHORTCUT_EXEMPTION_IS_EXERCISED"
    );
    assert!(
        totals.conditional_do > 0 && totals.conditional_batch > 0,
        "GENERATED_ACCEPTED_CONDITIONAL_COMMANDS"
    );
    let artifact = serde_json::json!({"save_open_checks":totals.save_open_checks,"transitions":totals.transitions,"approvals":totals.approvals,"shortcut_exemptions":totals.shortcut_exemptions,"accepted_do_if_revision":totals.conditional_do,"accepted_batch_if_revision":totals.conditional_batch,"uncertain_observations":totals.uncertain,"folded_provenance_probes":totals.provenance_probes,"control_difference_observations":totals.control_difference_observations,"control_difference_witnesses":totals.control_certain_differences});
    let directory = kinewright_media::test_support::workspace_target_dir().join("hx-part2");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("owner-differential.json"),
        serde_json::to_vec_pretty(&artifact).unwrap(),
    )
    .unwrap();
    println!("HX_OWNER_DIFFERENTIAL={artifact}");
}
