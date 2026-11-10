//! Differential proof for the save's reuse of its own earlier passes over the history.
//!
//! Each case runs the same edits and saves twice, in fresh bundles: once as the product
//! runs and once with `io_observer::without_save_reuse` (the old path). With reuse, the
//! replay shadow and the sealing take the check decode's preflight charge instead of
//! preflighting the same bytes again, and the history's support fields, walked by the
//! check, travel with the checked bytes into `admit_checked_history` instead of the support
//! inventory decoding the just-sealed history again. After every save it requires:
//!
//! - the same written history and document objects, byte for byte (other records carry
//!   fresh random identifiers, so the same kinds and byte lengths stand in for them);
//! - the same support inventory in the newest store manifest;
//! - every memoized history's fields equal to a fresh decode of that object's bytes on
//!   disk (the fields belong to the sealed bytes, also when bounding rewrote the history);
//! - exactly one history decode fewer per changed-history save with reuse on, and with it
//!   three preflights fewer (that decode's, the replay shadow's and the sealing's), and
//!   the same counts on a save that sealed no new history;
//! - a reopen of the bundle, whose reader decodes every record and refuses an inventory
//!   that differs from the one written.

use std::{collections::BTreeMap, path::Path};

use kinewright_core::{
    Clip, ClipId, Command, Core, Document, Event, HistorySnapshot, Operation, TimeCode, Track,
    TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;

use super::*;
use crate::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_commit::bound_history,
    io_observer,
};

fn apply(core: &Core, command: Command) -> bool {
    matches!(core.request(command), Ok(Event::DocumentChanged { .. }))
}

fn toggles(core: &Core, track: TrackId, from: u64, count: u64) {
    for i in from..from + count {
        assert!(apply(
            core,
            Command::Do(Operation::SetTrackSyncLock {
                track,
                locked: i.is_multiple_of(2),
            }),
        ));
    }
}

fn tracks(count: u64) -> Document {
    Document {
        tracks: (1..=count)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    }
}

fn fixture(name: &str) -> Document {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../kinewright-agent/tests/fixtures/ja")
        .join(name);
    crate::project::load_document(&path)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .0
}

/// A document of solid clips carrying every CC7 clip id, so the CC7 grading batches
/// (effects whose serialized forms are canonicalized on read) apply without media.
fn cc7_document() -> Document {
    let solid = fixture("synthetic-blue-project.kinewright").tracks[0].clips[0].clone();
    let mut ids: Vec<u64> = vec![
        kinewright_core::cc7_scenarios::CC7_SINGLE_CLIP_ID.0,
        kinewright_core::cc7_scenarios::CC7_CANDIDATE_CLIP_ID.0,
    ];
    for scenario in kinewright_core::CC7_SCENARIOS {
        ids.extend(
            kinewright_core::cc7_spec(scenario)
                .clips
                .iter()
                .map(|clip| clip.clip_id),
        );
    }
    ids.sort_unstable();
    ids.dedup();
    let clips: Vec<Clip> = ids
        .iter()
        .zip(0_i64..)
        .map(|(&id, index)| Clip {
            id: ClipId(id),
            source_range: TimeCode(0)..TimeCode(60),
            timeline_start: TimeCode(index * 60),
            ..solid.clone()
        })
        .collect();
    Document {
        duration: TimeCode(60 * i64::try_from(clips.len()).unwrap()),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips,
        }],
        ..Document::default()
    }
}

/// What one save left on disk, as far as it can be compared across two bundles.
#[derive(Debug, PartialEq)]
struct Written {
    /// History and document objects: sha256 to bytes.
    content: BTreeMap<String, Vec<u8>>,
    /// Every object's kind and byte length (but the input manifest's), in order.
    shapes: Vec<(RecordKind, u64)>,
    /// The newest store manifest's support inventory.
    inventory: BTreeSet<String>,
    /// History decodes the save ran.
    decodes: u64,
    /// Record preflights the save ran.
    preflights: u64,
    /// The store's cache of support fields and the memo's fields, for every history and
    /// document object (by sha256): the state the next save starts from.
    support_cache: BTreeMap<String, BTreeSet<String>>,
    memo_fields: BTreeMap<String, Option<BTreeSet<String>>>,
    /// Resting storage accounting after the save (peaks may differ: the reuse skips a
    /// decode's transient reservations), and the count of verified reads (their byte count
    /// includes the input manifest, whose length varies; see `shapes`).
    resting: [u64; 7],
}

/// The object's bytes as written, read through the bundle's namespace (not the store's
/// record read path, so no memo entry is touched).
fn object_bytes(store: &ProjectStore, reference: &ObjectRef) -> Vec<u8> {
    let path = ProjectStore::object_path(reference).unwrap();
    let bytes = store
        .with_namespace(|n| n.read(&path, reference.byte_length))
        .unwrap()
        .expect("object exists");
    assert_eq!(bytes.len() as u64, reference.byte_length);
    bytes
}

fn decoded_fields(store: &ProjectStore, bytes: &[u8]) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    crate::bundle_recovery::decode::<HistorySnapshot>(
        bytes,
        &store.inner.usage,
        RecordKind::History,
        &mut fields,
        store.limits(),
    )
    .unwrap();
    fields
}

/// Read back one save, and require every memoized history's fields to equal a fresh
/// decode of the bytes that history's object holds. Returns how many memoized histories
/// were checked.
fn written(bundle: &ProjectBundle, (decodes, preflights): (u64, u64)) -> (Written, usize) {
    let store = &bundle.store;
    let resting = {
        let usage = store.inner.usage.lock().unwrap();
        [
            usage.reserved_bytes,
            usage.decoded_bytes,
            usage.decoder_bytes,
            usage.live_history_bytes,
            usage.objects,
            usage.edges,
            usage.verified_calls,
        ]
    };
    let mut support_cache: BTreeMap<String, BTreeSet<String>> = store
        .inner
        .ledger
        .lock()
        .unwrap()
        .support_fields
        .iter()
        .map(|(sha, fields)| (sha.clone(), BTreeSet::clone(fields)))
        .collect();
    let mut memo_fields = BTreeMap::new();
    let objects: Vec<ObjectRef> = store
        .inner
        .ledger
        .lock()
        .unwrap()
        .objects
        .values()
        .map(|(reference, _)| reference.clone())
        .collect();
    let mut content = BTreeMap::new();
    let mut shapes = Vec::new();
    let mut newest: Option<(u64, BTreeSet<String>)> = None;
    let mut memoized = 0;
    for reference in &objects {
        // The input manifest is written by `empty_inputs` before the first save; its owner
        // epoch is random and printed in decimal, so its length differs from bundle to bundle.
        if reference.record_kind != RecordKind::InputManifest {
            shapes.push((reference.record_kind, reference.byte_length));
        }
        match reference.record_kind {
            RecordKind::History | RecordKind::Document => {
                memo_fields.insert(
                    reference.sha256.clone(),
                    store
                        .inner
                        .verified_memo
                        .lock()
                        .unwrap()
                        .cached_fields(reference, store.limits()),
                );
                let bytes = object_bytes(store, reference);
                if reference.record_kind == RecordKind::History {
                    let cached = store
                        .inner
                        .verified_memo
                        .lock()
                        .unwrap()
                        .cached_fields(reference, store.limits());
                    if let Some(cached) = cached {
                        assert_eq!(
                            cached,
                            decoded_fields(store, &bytes),
                            "memoized fields of {} differ from its sealed bytes",
                            reference.sha256
                        );
                        memoized += 1;
                    }
                }
                content.insert(reference.sha256.clone(), bytes);
            }
            RecordKind::StoreManifest => {
                let manifest = store.read_record::<StoreManifest>(reference).unwrap();
                if newest
                    .as_ref()
                    .is_none_or(|(generation, _)| *generation < manifest.generation)
                {
                    newest = Some((manifest.generation, manifest.required_fields.clone()));
                }
            }
            _ => {}
        }
    }
    shapes.sort();
    // Other records carry fresh identifiers, so only these objects have the same digest in
    // both bundles.
    support_cache.retain(|sha, _| content.contains_key(sha));
    let inventory = newest.expect("a store manifest").1;
    (
        Written {
            content,
            shapes,
            inventory,
            decodes,
            preflights,
            support_cache,
            memo_fields,
            resting,
        },
        memoized,
    )
}

/// The edits before one save.
type Step = Box<dyn Fn(&Core)>;

/// One case: an initial document, an optional reopen at smaller limits after the first
/// save, and the edits before each later save.
struct Case {
    name: &'static str,
    document: Document,
    limits: Option<StorageLimits>,
    steps: Vec<Step>,
}

struct Run {
    saves: Vec<Written>,
    memoized: usize,
    /// For each save: was the sealed history different from the history the core exports?
    rewritten: Vec<bool>,
    floored: bool,
}

fn run(case: &Case, root: &Path, reuse: bool) -> Run {
    let temp = TempDirectory::new_in(root, case.name);
    let path = temp.path("p.kinewright");
    let core = Core::spawn(case.document.clone()).unwrap();
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    let save = |bundle: &mut ProjectBundle| {
        let (decodes, preflights) = (
            io_observer::history_decode_count(),
            io_observer::preflight_count(),
        );
        if reuse {
            bundle.save(&core, &inputs).unwrap();
        } else {
            io_observer::without_save_reuse(|| bundle.save(&core, &inputs).unwrap());
        }
        (
            io_observer::history_decode_count() - decodes,
            io_observer::preflight_count() - preflights,
        )
    };
    let counts = save(&mut bundle);
    let mut saves = vec![written(&bundle, counts).0];
    let mut memoized = 0;
    if let Some(limits) = &case.limits {
        drop(bundle);
        bundle = LoadedBundle::read_with_limits_probe(&path, limits.clone())
            .unwrap()
            .into_writer()
            .unwrap();
    }
    let limits = case.limits.clone().unwrap_or_default();
    let mut rewritten = vec![false];
    for step in &case.steps {
        step(&core);
        let exported = core.export_history().unwrap();
        rewritten.push(bound_history(&exported, &limits).unwrap() != exported);
        let counts = save(&mut bundle);
        let (this_save, memoized_here) = written(&bundle, counts);
        memoized += memoized_here;
        saves.push(this_save);
    }
    drop(bundle);
    // The reader decodes every record and refuses an inventory that differs from its own.
    let loaded = LoadedBundle::read_with_limits_probe(&path, limits).unwrap();
    let floored = loaded
        .manual_checkpoint()
        .is_ok_and(|checkpoint| checkpoint.floor_generation > 0);
    Run {
        saves,
        memoized,
        rewritten,
        floored,
    }
}

fn root() -> std::path::PathBuf {
    kinewright_media::test_support::workspace_target_dir().join("support-reuse")
}

/// Run the case both ways and require the same writes; returns the reuse run.
fn differential(case: &Case) -> Run {
    let root = root();
    let old = run(case, &root, false);
    let new = run(case, &root, true);
    assert_eq!(new.saves.len(), old.saves.len());
    for (index, (new_save, old_save)) in new.saves.iter().zip(&old.saves).enumerate() {
        let label = format!("{} save {index}", case.name);
        assert_eq!(new_save.content, old_save.content, "{label}: objects");
        assert_eq!(new_save.shapes, old_save.shapes, "{label}: object shapes");
        assert_eq!(new_save.inventory, old_save.inventory, "{label}: inventory");
        assert_eq!(
            new_save.support_cache, old_save.support_cache,
            "{label}: support cache"
        );
        assert_eq!(
            new_save.memo_fields, old_save.memo_fields,
            "{label}: memo fields"
        );
        assert_eq!(new_save.resting, old_save.resting, "{label}: resting usage");
        assert!(!new_save.inventory.is_empty(), "{label}: empty inventory");
        // One decode fewer when the save sealed a new history, none otherwise; and with
        // it three preflights of the checked bytes: the skipped decode's, the replay
        // shadow's and the sealing's.
        assert!(
            (new_save.decodes + 1 == old_save.decodes
                && new_save.preflights + 3 == old_save.preflights)
                || (new_save.decodes == old_save.decodes
                    && new_save.preflights == old_save.preflights),
            "{label}: decodes {} and preflights {} with reuse, {} and {} without",
            new_save.decodes,
            new_save.preflights,
            old_save.decodes,
            old_save.preflights
        );
    }
    let saved = |run: &Run| run.saves.iter().map(|s| s.decodes).sum::<u64>();
    let preflights = |run: &Run| run.saves.iter().map(|s| s.preflights).sum::<u64>();
    assert!(
        saved(&new) < saved(&old),
        "{}: reuse never skipped a decode",
        case.name
    );
    assert_eq!(new.rewritten, old.rewritten);
    assert_eq!(new.floored, old.floored);
    assert!(
        new.memoized > 0,
        "{}: no memoized history checked",
        case.name
    );
    eprintln!(
        "SUPPORT_REUSE case={} saves={} decodes_old={} decodes_new={} preflights_old={} \
         preflights_new={} memoized_checked={} rewritten_saves={} floored={}",
        case.name,
        new.saves.len(),
        saved(&old),
        saved(&new),
        preflights(&old),
        preflights(&new),
        new.memoized,
        new.rewritten.iter().filter(|r| **r).count(),
        new.floored
    );
    new
}

fn step(edit: impl Fn(&Core) + 'static) -> Step {
    Box::new(edit)
}

#[test]
fn fixtures_and_cc7_grading_write_the_same_bytes_and_inventory() {
    for name in [
        "s1-migration-input.kinewright",
        "synthetic-blue-project.kinewright",
        "synthetic-red-project.kinewright",
        "writer-v1-empty.kinewright",
        "writer-v1-track.kinewright",
        "writer-v2-adjustment.kinewright",
    ] {
        let document = fixture(name);
        let track = document.tracks.first().map(|track| track.id);
        let mut steps = vec![step(|_| {})];
        if let Some(track) = track {
            steps.push(step(move |core| toggles(core, track, 0, 3)));
            steps.push(step(move |core| toggles(core, track, 3, 1)));
        } else {
            steps.push(step(|core| {
                assert!(apply(
                    core,
                    Command::Do(Operation::AddTrack {
                        track: Track {
                            id: TrackId(9),
                            kind: TrackKind::Video,
                            sync_lock: true,
                            clips: vec![],
                        },
                    })
                ));
            }));
        }
        differential(&Case {
            name,
            document,
            limits: None,
            steps,
        });
    }
    let mut steps: Vec<Step> = Vec::new();
    for scenario in kinewright_core::CC7_SCENARIOS {
        steps.push(step(move |core| {
            for operation in kinewright_core::cc7_canonical_operations(scenario) {
                apply(core, Command::Do(operation));
            }
        }));
    }
    steps.push(step(|core| {
        for operation in kinewright_core::cc7_b1_canonical_operations()
            .into_iter()
            .chain(kinewright_core::cc7_d2_canonical_operations())
        {
            apply(core, Command::Do(operation));
        }
    }));
    steps.push(step(|core| {
        apply(core, Command::Undo);
    }));
    // The grading batches really apply here: count accepted operations and graded clips.
    let probe = Core::spawn(cc7_document()).unwrap();
    let mut accepted = Vec::new();
    for scenario in kinewright_core::CC7_SCENARIOS {
        accepted.push(
            kinewright_core::cc7_canonical_operations(scenario)
                .into_iter()
                .filter(|operation| apply(&probe, Command::Do(operation.clone())))
                .count(),
        );
    }
    let graded = probe
        .export_history()
        .unwrap()
        .document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter(|clip| !clip.effects.is_empty())
        .count();
    eprintln!("SUPPORT_REUSE cc7 accepted_per_scenario={accepted:?} graded_clips={graded}");
    assert!(graded >= 2, "too few graded clips: {graded}");
    differential(&Case {
        name: "cc7-grading",
        document: cc7_document(),
        limits: None,
        steps,
    });
}

/// The 308-batch history of the walk differential, saved along the way and at the end.
#[test]
fn long_history_writes_the_same_bytes_and_inventory() {
    differential(&Case {
        name: "long-308",
        document: tracks(8),
        limits: None,
        steps: vec![
            step(|core| toggles(core, TrackId(1), 0, 150)),
            step(|core| toggles(core, TrackId(2), 150, 150)),
            step(|core| toggles(core, TrackId(3), 300, 8)),
            step(|core| {
                apply(core, Command::Undo);
            }),
        ],
    });
}

/// A history held at a small cap: saves go through the floor path, so the sealed history
/// is a bounded rewrite of what the core exports, not the first bytes serialized.
#[test]
fn bounded_history_at_the_cap_writes_the_same_bytes_and_inventory() {
    let limits = StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    };
    let mut steps: Vec<Step> = Vec::new();
    for chunk in 0..12 {
        steps.push(step(move |core| {
            toggles(core, TrackId(1 + chunk % 8), chunk * 40, 40);
        }));
    }
    let run = differential(&Case {
        name: "cap-floor",
        document: tracks(8),
        limits: Some(limits),
        steps,
    });
    assert!(run.floored, "the case never reached the floor path");
    assert!(
        run.rewritten.iter().filter(|r| **r).count() >= 3,
        "bounding rewrote too few saves: {:?}",
        run.rewritten
    );
}

/// The binding: fields offered with other bytes are not used. A history admitted with a
/// check of a different history's bytes yields no reusable fields, and its support read
/// decodes it; fields bound to one sealed object are not used for another; and the same
/// history admitted with its own check is read without a decode, with exactly the fields
/// its sealed bytes decode to.
#[test]
fn fields_checked_for_other_bytes_are_not_used() {
    let temp = TempDirectory::new_in(&root(), "binding");
    let core = Core::spawn(tracks(2)).unwrap();
    let mut bundle = ProjectBundle::activate(
        &temp.path("p.kinewright"),
        BundleWriteAdmission::new_document(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    let history = |track: u64, from: u64, count: u64| {
        toggles(&core, TrackId(track), from, count);
        core.export_history().unwrap()
    };
    let (first, second, third, fourth) = (
        history(1, 0, 2),
        history(2, 2, 3),
        history(1, 5, 1),
        history(2, 6, 1),
    );
    let store = &mut bundle.store;
    let decodes = |store: &ProjectStore, roots: &[ObjectRef], sealed| {
        let before = io_observer::history_decode_count();
        let fields = store.support_fields(roots, sealed).unwrap();
        (fields, io_observer::history_decode_count() - before)
    };

    let preflights = |store: &mut ProjectStore, record: &HistorySnapshot, checked| {
        let before = io_observer::preflight_count();
        let admitted = store.admit_checked_history(record, checked).unwrap();
        (admitted, io_observer::preflight_count() - before)
    };
    // An unchecked admission of a fresh history (`first`), for the preflights it runs.
    let (_, unchecked) = preflights(store, &first, None);

    // Checked bytes of `first`, sealed bytes of `second`: nothing to reuse; the sealing
    // preflights its own bytes, as an unchecked admission does.
    let (_, other) = store
        .check_history_bytes(serde_json::to_vec(&first).unwrap())
        .unwrap();
    let ((sealed, support), ran) = preflights(store, &second, Some(other));
    assert!(support.is_none(), "fields of other bytes were offered");
    assert_eq!(
        ran, unchecked,
        "the check's charge was used for other bytes"
    );
    let bytes = object_bytes(store, &sealed.reference);
    let (fields, ran) = decodes(store, std::slice::from_ref(&sealed.reference), None);
    assert_eq!(ran, 1, "the support read did not decode the history");
    assert_eq!(fields, decoded_fields(store, &bytes));

    // `third`'s own check: its charge stands in for the sealing's preflight, and its
    // fields are offered, bound to its sealed object.
    let (_, own) = store
        .check_history_bytes(serde_json::to_vec(&third).unwrap())
        .unwrap();
    let ((third_sealed, third_support), ran) = preflights(store, &third, Some(own));
    assert_eq!(
        ran + 1,
        unchecked,
        "the sealing preflighted the checked bytes again"
    );
    let third_support = third_support.expect("own fields are offered");
    // Offered for a different object (`fourth`), they are not used: `fourth` is decoded.
    let fourth_sealed = store.admit_record(&fourth).unwrap();
    let bytes = object_bytes(store, &fourth_sealed.reference);
    let (fields, ran) = decodes(
        store,
        std::slice::from_ref(&fourth_sealed.reference),
        Some(third_support),
    );
    assert_eq!(ran, 1, "fields bound to another object were used");
    assert_eq!(fields, decoded_fields(store, &bytes));

    // Offered for their own object, they are used: no decode, the sealed bytes' fields,
    // and the support cache holds them as a decode would have left it.
    let (_, own) = store
        .check_history_bytes(serde_json::to_vec(&third).unwrap())
        .unwrap();
    let (_, again) = store.admit_checked_history(&third, Some(own)).unwrap();
    let bytes = object_bytes(store, &third_sealed.reference);
    let (fields, ran) = decodes(store, std::slice::from_ref(&third_sealed.reference), again);
    assert_eq!(ran, 0, "the bound fields were not used");
    assert_eq!(fields, decoded_fields(store, &bytes));
    assert_eq!(
        store
            .inner
            .ledger
            .lock()
            .unwrap()
            .support_fields
            .get(&third_sealed.reference.sha256)
            .map(|cached| BTreeSet::clone(cached)),
        Some(fields)
    );
}

/// A limit profile that restricts one field to the given value.
type Restrict = fn(u64) -> StorageLimits;

/// One save of a 158-batch history under `restrict(limit)`, after a first save at default
/// limits, with reuse on or off. Returns the save's result and, when it succeeded, the
/// history and document objects it left.
fn save_at_limit(
    reuse: bool,
    restrict: Restrict,
    limit: u64,
) -> (Result<(), String>, BTreeMap<String, Vec<u8>>) {
    let temp = TempDirectory::new_in(&root(), "exact-limit");
    let core = Core::spawn(tracks(8)).unwrap();
    toggles(&core, TrackId(1), 0, 150);
    let mut bundle = ProjectBundle::activate(
        &temp.path("p.kinewright"),
        BundleWriteAdmission::new_document(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    toggles(&core, TrackId(2), 150, 8);
    let result = bundle
        .restrict_limits_probe(restrict(limit))
        .and_then(|()| {
            if reuse {
                bundle.save(&core, &inputs)
            } else {
                io_observer::without_save_reuse(|| bundle.save(&core, &inputs))
            }
        })
        .map(drop)
        .map_err(|error| format!("{error:?}"));
    let content = if result.is_ok() {
        written(&bundle, (0, 0)).0.content
    } else {
        BTreeMap::new()
    };
    (result, content)
}

/// The smallest limit at which the save succeeds, with reuse on or off, searched below
/// `ceiling` (which must accept).
fn smallest_limit(reuse: bool, restrict: Restrict, ceiling: u64) -> u64 {
    let (mut refused, mut accepted) = (0, ceiling);
    assert!(save_at_limit(reuse, restrict, accepted).0.is_ok());
    while accepted - refused > 1 {
        let middle = refused + (accepted - refused) / 2;
        if save_at_limit(reuse, restrict, middle).0.is_ok() {
            accepted = middle;
        } else {
            refused = middle;
        }
    }
    accepted
}

/// Refusal identity at one exact cumulative limit. Reuse skips the support decode's
/// transient reservations and takes none of its own, so at an exact limit it could accept
/// where the old path refuses, and never the reverse: wherever the old path accepts, reuse
/// accepts and writes the same bytes; one byte under reuse's own exact limit, both refuse
/// with the same error. Returns the two smallest accepting limits (old, new).
fn exact_limit_differential(name: &str, restrict: Restrict, ceiling: u64) -> (u64, u64) {
    let old = smallest_limit(false, restrict, ceiling);
    let new = smallest_limit(true, restrict, ceiling);
    eprintln!("SUPPORT_REUSE exact_limit={name} smallest_old={old} smallest_new={new}");
    assert!(
        new <= old,
        "{name}: reuse needs a larger limit: {new} > {old}"
    );

    let (old_result, old_content) = save_at_limit(false, restrict, old);
    let (new_result, new_content) = save_at_limit(true, restrict, old);
    assert_eq!((old_result, new_result), (Ok(()), Ok(())), "{name}");
    assert!(!new_content.is_empty());
    assert_eq!(
        new_content, old_content,
        "{name}: objects at the exact limit"
    );

    let (old_refusal, _) = save_at_limit(false, restrict, new - 1);
    let (new_refusal, _) = save_at_limit(true, restrict, new - 1);
    assert!(
        new_refusal.is_err(),
        "{name}: reuse accepted under its smallest limit"
    );
    assert_eq!(
        old_refusal, new_refusal,
        "{name}: refusals under the exact limit"
    );
    (old, new)
}

/// The live-history limit (where an earlier draft of the support reuse refused): the save's
/// live-history peak is set by the commit check, not by the support read, so both paths
/// need exactly the same limit here. No one-way case (reuse accepting where the old path
/// refuses) was found: not here, and not at `metadata_bytes`, whose usage varies by a few
/// bytes from bundle to bundle (random identifiers printed in decimal), so an exact
/// metadata limit is not reproducible; both paths' smallest metadata limits were within
/// that noise of each other.
#[test]
fn exact_live_history_limit_refuses_the_same_with_and_without_reuse() {
    let (old, new) = exact_limit_differential(
        "live_history_bytes",
        |limit| StorageLimits {
            live_history_bytes: limit,
            ..StorageLimits::default()
        },
        StorageLimits::default().live_history_bytes,
    );
    assert_eq!(new, old);
}
