use super::*;

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
#[expect(
    clippy::too_many_lines,
    reason = "one explicit command grammar with consent admission before the corresponding transition"
)]
fn message<S: ModelStore>(session: &mut Session<S>, id: u64, choice: u64) -> Message {
    let mut approval = None;
    let mut transition = None;
    let splittable = session
        .gui
        .history()
        .document()
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .find(|c| c.source_range.end.0 - c.source_range.start.0 > 1)
        .map(|c| (c.id, TimeCode(c.timeline_start.0 + 1)));
    let removable = session
        .gui
        .history()
        .document()
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .next()
        .map(|c| c.id);
    let action = match (choice % 12, splittable, removable) {
        (0, _, _) => Action::Undo,
        (1, _, _) => Action::Redo,
        (2, _, _) => Action::Close,
        (3, _, _) if !session.gui.history().document().tracks.is_empty() => {
            // Native approvals are committed synchronously. GUI hand edits
            // may queue; native task consent never lives in an unacked GUI
            // suffix which would need an approval rebind after restart.
            session.drain();
            let reference = session.store.blob(
                format!("Do consent {id}").as_bytes(),
                RecordKind::OwnedEvidence,
            );
            approval = Some(Approval {
                reference,
                targets: vec![crate::operation_history::RemovedTarget::Track(TrackId(81))],
            });
            Action::Do {
                operations: vec![Operation::RemoveTrack { track: TrackId(81) }],
                gesture: None,
            }
        }
        (4, _, _) => Action::Do {
            operations: vec![Operation::AddTrack {
                track: document().tracks.remove(0),
            }],
            gesture: None,
        },
        (5, _, _) => {
            let reference = session
                .store
                .blob(format!("owned LUT {id}").as_bytes(), RecordKind::OwnedLut);
            Action::Do {
                operations: vec![lut(&reference, id)],
                gesture: None,
            }
        }
        (6 | 7, _, _) => Action::Do {
            operations: vec![marker(id)],
            gesture: Some(format!("gesture {}", id / 4)),
        },
        (8, _, _) if !session.gui.history().document().lut_assets.is_empty() => Action::Do {
            operations: vec![Operation::RemoveLutAsset {
                lut_asset: session.gui.history().document().lut_assets[0].id,
            }],
            gesture: None,
        },
        (9, _, _) => Action::Do {
            operations: vec![Operation::AddSolidClip {
                track: TrackId(81),
                timeline_start: TimeCode(i64::try_from(id * 20).unwrap()),
                duration: TimeCode(10),
                color: kinewright_core::SolidColor::default(),
            }],
            gesture: None,
        },
        (10, Some((clip, at)), _) => Action::Do {
            operations: vec![Operation::SplitClip { clip, at }],
            gesture: None,
        },
        (11, _, Some(clip)) => Action::Do {
            operations: vec![Operation::DeleteClip { clip }],
            gesture: None,
        },
        _ => Action::Do {
            operations: vec![marker(id)],
            gesture: None,
        },
    };
    if matches!(action, Action::Undo | Action::Redo) && (choice >> 8).is_multiple_of(2) {
        session.drain();
        let direction = if matches!(action, Action::Undo) {
            Direction::Undo
        } else {
            Direction::Redo
        };
        if session.gui.transition_target(direction).is_some() {
            transition = Some(super::super::super::TransitionApproval {
                id: session.store.blob(
                    format!("transition consent {id}").as_bytes(),
                    RecordKind::OwnedEvidence,
                ),
                direction,
                target: session.gui.transition_target(direction).unwrap(),
            });
        }
    }
    Message {
        id,
        action,
        creation: origin(id + 2),
        approval,
        transition,
    }
}

fn prime_group<S: ModelStore>(session: &mut Session<S>, seed: u64, state: &mut u64) {
    let capacity = crate::operation_protocol::PENDING_COMMANDS;
    for offset in 0..capacity as u64 {
        let id = 1_000_000 + seed * capacity as u64 + offset;
        session.send(Message {
            id,
            action: Action::Close,
            creation: origin(id + 2),
            approval: None,
            transition: None,
        });
    }
    assert_eq!(
        session.pending.len(),
        capacity,
        "GROUP_PRIMER_FILLS_THE_COUNT_WINDOW"
    );
    let choice = random(state);
    let width = 1 + usize::try_from(choice % capacity as u64).unwrap();
    let point = usize::try_from((choice >> 16) % capacity as u64).unwrap();
    session.group(width, GroupFault::RefuseAt(point));
    session.drain();
}

fn run_seed<S: ModelStore>(seed: u64, length: u64) -> Coverage {
    struct FailureContext {
        seed: u64,
        attempt: u64,
        length: u64,
    }
    impl Drop for FailureContext {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!(
                    "HX_SESSION_MODEL_FAILURE seed={} attempt={} random_attempts={}",
                    self.seed, self.attempt, self.length
                );
            }
        }
    }
    // Attempt zero is the primer; length + 1 is final drain/collection.
    let mut context = FailureContext {
        seed,
        attempt: 0,
        length,
    };
    let started = std::time::Instant::now();
    let mut session = Session::new(S::default());
    prime_redo_payload(&mut session, seed);
    let mut state = seed;
    prime_group(&mut session, seed, &mut state);
    for index in 0..length {
        context.attempt = index + 1;
        let id = seed * (length + 1) + index;
        let choice = random(&mut state);
        let input = message(&mut session, id, choice);
        let synchronous = input.approval.is_some() || input.transition.is_some();
        session.send(input);
        if synchronous {
            session.drain();
        }
        let width = 1 + usize::try_from((choice >> 32) % 64).unwrap();
        let point = usize::try_from((choice >> 48) % 64).unwrap();
        match (choice >> 16) % 12 {
            0 => session.group(width, GroupFault::LoseAcknowledgement),
            1 => session.group(width, GroupFault::StopBeforeRoot(point)),
            2 => session.restart(false),
            3 => session.restart(true),
            4 => session.save(false),
            5 => session.save(true),
            6 => session.install_checkpoint(usize::from(choice & 1 != 0)),
            7 => {
                session.drain();
                let root = session.store.cold_root();
                session.cold_open(&root.snapshot);
                session.coverage.opens += 1;
                session.assert_equal();
            }
            8 => session.collect(),
            9 => session.group(width, GroupFault::RefuseAt(point)),
            10 => session.group(width, GroupFault::StopAfterRoot),
            _ => {}
        }
        session.assert_acknowledged();
    }
    context.attempt = length + 1;
    session.drain();
    session.collect();
    eprintln!(
        "HX_SESSION_SEED_COMPLETE seed={seed} random_attempts={length} elapsed_ms={}",
        started.elapsed().as_millis()
    );
    session.coverage
}

pub(super) fn run<S: ModelStore>(seeds: u64, length: u64) {
    run_range::<S>(1, seeds, length);
}

fn title_message<S: ModelStore>(
    session: &mut Session<S>,
    next: &mut u64,
    operations: Vec<Operation>,
    gesture: Option<String>,
    context: &str,
) {
    let id = *next;
    *next += 1;
    session.send(Message {
        id,
        action: Action::Do {
            operations,
            gesture,
        },
        creation: origin(id + 2),
        approval: None,
        transition: None,
    });
    assert!(
        session.accepted.contains_key(&id),
        "IMPLICIT_TITLE_SESSION_ACCEPTED {context} id={id}"
    );
    session.drain();
    session.assert_acknowledged();
}

pub(super) fn implicit_title_streams<S: ModelStore>() {
    use crate::operation_history::resource_cases::{ImplicitEdit, session_operations, title_half};
    let mut streams = 0;
    for kind in ImplicitEdit::ALL {
        for original in [false, true] {
            for coalesced in [false, true] {
                let context = format!("kind={kind:?} original={original} coalesced={coalesced}");
                let mut store = S::default();
                let font = store.blob(kinewright_core::title_font_bytes(), RecordKind::OwnedFont);
                let mut session = Session::new(store);
                let mut next = 0;
                let (setup, insert) = session_operations(kind);
                title_message(&mut session, &mut next, setup, None, &context);
                for marker_id in 1..=4 {
                    title_message(
                        &mut session,
                        &mut next,
                        vec![marker(marker_id)],
                        None,
                        &context,
                    );
                }
                let gesture = || coalesced.then(|| "implicit-title".to_owned());
                title_message(&mut session, &mut next, vec![insert], gesture(), &context);
                let removed = title_half(session.gui.history().document(), original);
                title_message(
                    &mut session,
                    &mut next,
                    vec![Operation::DeleteClip { clip: removed }],
                    gesture(),
                    &context,
                );
                for marker_id in 5..=10 {
                    title_message(
                        &mut session,
                        &mut next,
                        vec![marker(marker_id)],
                        None,
                        &context,
                    );
                }
                assert!(
                    session
                        .gui
                        .resources
                        .baseline
                        .contains(&Resource::TitleFont),
                    "IMPLICIT_TITLE_SURVIVOR_KEEPS_FONT {context}"
                );
                session.save(false);
                session.collect();
                session.restart(false);
                session.assert_equal();
                session.store.assert_exists(&font);
                session.save(true);
                session.collect();
                session.restart(false);
                let remaining = session
                    .gui
                    .history()
                    .document()
                    .tracks
                    .iter()
                    .flat_map(|track| &track.clips)
                    .find(|clip| matches!(clip.content, kinewright_core::ClipContent::Title(_)))
                    .unwrap()
                    .id;
                title_message(
                    &mut session,
                    &mut next,
                    vec![Operation::DeleteClip { clip: remaining }],
                    None,
                    &context,
                );
                for marker_id in 11..=16 {
                    title_message(
                        &mut session,
                        &mut next,
                        vec![marker(marker_id)],
                        None,
                        &context,
                    );
                }
                session.checkpoints.clear();
                session.collect();
                session.restart(false);
                session.store.assert_absent(&font);
                assert!(
                    !session.gui.resources().contains(&Resource::TitleFont),
                    "IMPLICIT_TITLE_FONT_RETIRES_AFTER_LAST_REACHABLE_TITLE {context}"
                );
                streams += 1;
            }
        }
    }
    assert_eq!(streams, 12, "ALL_IMPLICIT_TITLE_SESSION_CLASSES_EXECUTED");
}

pub(super) fn run_range<S: ModelStore>(first: u64, last: u64, length: u64) {
    let started = std::time::Instant::now();
    let seeds = last - first + 1;
    let cases: Vec<_> = if length >= 1024 {
        // Cases share no replicas, store, pool or mutable oracle. Keep the
        // complete corpus while using the existing four-thread test budget.
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|lane| {
                    scope.spawn(move || {
                        (first..=last)
                            .filter(|seed| (seed - first) % 4 == lane)
                            .map(|seed| (seed, run_seed::<S>(seed, length)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().expect("SESSION_CASE_THREAD_SUCCEEDED"))
                .collect()
        })
    } else {
        (first..=last)
            .map(|seed| (seed, run_seed::<S>(seed, length)))
            .collect()
    };
    let mut totals = Coverage::default();
    let mut completed = BTreeSet::new();
    for (seed, coverage) in cases {
        assert!(
            completed.insert(seed),
            "SESSION_EACH_SEED_RUNS_EXACTLY_ONCE"
        );
        totals.accumulate(&coverage);
    }
    assert_eq!(
        completed,
        (first..=last).collect(),
        "SESSION_COMPLETE_CORPUS_WAS_EXECUTED"
    );
    for (name, count) in [
        ("edit", totals.edits),
        ("fold", totals.folds),
        ("undo", totals.undos),
        ("redo", totals.redos),
        ("close", totals.closes),
        ("Do approval", totals.approvals),
        ("transition approval", totals.transitions),
        ("restart", totals.restarts),
        ("lost ack", totals.lost_acks),
        ("pre-publication crash", totals.before_publish),
        ("post-publication crash", totals.after_publish),
        ("group refusal", totals.group_refusals),
        (
            "group width greater than one",
            totals.largest_group.saturating_sub(1),
        ),
        ("handoff", totals.handoffs),
        ("Save", totals.saves),
        ("Save As", totals.save_as),
        ("rescue", totals.rescues),
        ("cold open", totals.opens),
        ("collection", totals.collections),
        ("nonempty baseline lineage", totals.baseline_lineage),
        ("folded split ancestry", totals.split_ancestry),
        ("Redo-only payload collection", totals.redo_payloads),
    ] {
        assert!(count > 0, "SESSION_GRAMMAR_REACHES_{name}");
    }
    println!(
        "HX_SESSION_MODEL first_seed={first} last_seed={last} seeds={seeds} random_attempts={} primed_transitions={} elapsed_ms={} folds={} restart={} lost_ack={} prepublication_crash={} postpublication_crash={} group_refusal={} largest_group={} handoff={} save={} save_as={} rescue={} cold_open={} collect={} approvals={} transition_approvals={} baseline_lineage={} split_ancestry={} redo_payloads={} ordinary_lease_peak={}",
        seeds * length,
        seeds * 2,
        started.elapsed().as_millis(),
        totals.folds,
        totals.restarts,
        totals.lost_acks,
        totals.before_publish,
        totals.after_publish,
        totals.group_refusals,
        totals.largest_group,
        totals.handoffs,
        totals.saves,
        totals.save_as,
        totals.rescues,
        totals.opens,
        totals.collections,
        totals.approvals,
        totals.transitions,
        totals.baseline_lineage,
        totals.split_ancestry,
        totals.redo_payloads,
        totals.ordinary_lease_peak
    );
}

fn prime_redo_payload<S: ModelStore>(session: &mut Session<S>, seed: u64) {
    let reference = session.store.blob(
        format!("primer LUT {seed}").as_bytes(),
        RecordKind::OwnedLut,
    );
    for (id, action) in [
        (
            1,
            Action::Do {
                operations: vec![lut(&reference, 1)],
                gesture: None,
            },
        ),
        (2, Action::Undo),
    ] {
        session.send(Message {
            id,
            action,
            creation: origin(id + 2),
            approval: None,
            transition: None,
        });
    }
    session.save(false);
    session.cold_open(&session.store.root().snapshot);
    session.assert_equal();
    session.collect();
}
