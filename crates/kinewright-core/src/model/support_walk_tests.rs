//! Differential proof for `walk_serialized_support`: the walk that decides which saved
//! records are refused must give the same answer as the walk it replaced, on every input.
//!
//! `walk_serialized_support_reference` is that previous walk, kept verbatim (only its own
//! recursive calls are renamed). It copies the whole accumulated set for each
//! `anyOf`/`oneOf` alternative; the product walk gives each alternative an empty set and
//! merges it on success. Every check below requires the identical `Result`: the same
//! `Ok`/`Err`, the same error string and the same field set.
//!
//! The default run takes a few seconds in a debug build. Set
//! `KINEWRIGHT_SUPPORT_DIFF_SAMPLES=<n>` for a larger on-demand run: `n` seeded mutations of
//! the long history, `10 * n` of each scenario history, and every mutation at every node of
//! a whole scenario history (about 19,000 more inputs, minutes in a debug build).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{matches_type, walk_serialized_support};
use crate::{
    AssetId, Au6Scenario, Clip, ClipContent, ColorDescription, Command, Core, Document, Event,
    HistorySnapshot, MediaAsset, MediaKind, MediaSourceFingerprint, Operation, Rational, TimeCode,
    Track,
};

#[allow(
    clippy::too_many_lines,
    reason = "verbatim copy of the replaced walk, kept as the oracle"
)]
fn walk_serialized_support_reference(
    value: &Value,
    schema: &Value,
    root: &Value,
    path: &str,
    depth: u32,
    fields: &mut BTreeSet<String>,
) -> Result<(), String> {
    if depth > 128 {
        return Err("raw support depth exceeded".into());
    }
    if schema == &Value::Bool(false) {
        return Err(format!("unsupported value at {path}"));
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let local = reference
            .strip_prefix('#')
            .and_then(|r| root.pointer(r))
            .ok_or_else(|| "nonlocal compiled schema reference".to_owned())?;
        return walk_serialized_support_reference(value, local, root, path, depth + 1, fields);
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(alternatives) = schema.get(key).and_then(Value::as_array) {
            for alternative in alternatives {
                let mut candidate = fields.clone();
                if walk_serialized_support_reference(
                    value,
                    alternative,
                    root,
                    path,
                    depth + 1,
                    &mut candidate,
                )
                .is_ok()
                {
                    *fields = candidate;
                    return Ok(());
                }
            }
            return Err(format!("unsupported variant at {path}"));
        }
    }
    if let Some(variants) = schema.get("enum").and_then(Value::as_array)
        && !variants.contains(value)
    {
        return Err(format!("unsupported enum at {path}"));
    }
    if let Some(constant) = schema.get("const")
        && value != constant
    {
        return Err(format!("unsupported tag at {path}"));
    }
    if let Some(kind) = schema.get("type") {
        let matches = kind.as_str().is_some_and(|k| matches_type(value, k))
            || kind.as_array().is_some_and(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .any(|k| matches_type(value, k))
            });
        if !matches {
            return Err(format!("unsupported shape at {path}"));
        }
    }
    if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
        for member in all {
            walk_serialized_support_reference(value, member, root, path, depth + 1, fields)?;
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array)
            && required
                .iter()
                .filter_map(Value::as_str)
                .any(|k| !object.contains_key(k))
        {
            return Err(format!("missing required field at {path}"));
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, child) in object {
            if let Some(property) = properties.and_then(|p| p.get(key)) {
                let field = format!("{path}.{key}");
                fields.insert(field.clone());
                walk_serialized_support_reference(
                    child,
                    property,
                    root,
                    &field,
                    depth + 1,
                    fields,
                )?;
            } else if let Some(additional) = schema.get("additionalProperties") {
                if additional == &Value::Bool(false)
                    || (properties.is_some() && additional == &Value::Bool(true))
                {
                    return Err(format!("unknown field at {path}"));
                }
                let field = format!("{path}.*");
                fields.insert(field.clone());
                walk_serialized_support_reference(
                    child,
                    additional,
                    root,
                    &field,
                    depth + 1,
                    fields,
                )?;
            } else if properties.is_some() {
                return Err(format!("unknown field at {path}"));
            }
        }
    }
    if let Some(array) = value.as_array() {
        let tuple = schema.get("prefixItems").and_then(Value::as_array);
        if let Some(tuple) = tuple {
            for (child, item) in array.iter().zip(tuple) {
                walk_serialized_support_reference(
                    child,
                    item,
                    root,
                    &format!("{path}[]"),
                    depth + 1,
                    fields,
                )?;
            }
        }
        if let Some(items) = schema.get("items") {
            for child in array.iter().skip(tuple.map_or(0, Vec::len)) {
                walk_serialized_support_reference(
                    child,
                    items,
                    root,
                    &format!("{path}[]"),
                    depth + 1,
                    fields,
                )?;
            }
        }
    }

    Ok(())
}

type Walk = fn(&Value, &Value, &Value, &str, u32, &mut BTreeSet<String>) -> Result<(), String>;

fn run(walk: Walk, raw: &Value, name: &str, schema: &Value) -> Result<BTreeSet<String>, String> {
    let mut fields = BTreeSet::new();
    walk(raw, schema, schema, name, 0, &mut fields)?;
    Ok(fields)
}

/// One walk of `raw` against `schema` (resolving references in `root`) from a given field
/// set; returns the outcome and the set as the walk left it, also after an `Err`.
fn run_keeping(
    walk: Walk,
    raw: &Value,
    name: &str,
    schema: &Value,
    root: &Value,
    mut fields: BTreeSet<String>,
) -> (Result<(), String>, BTreeSet<String>) {
    let outcome = walk(raw, schema, root, name, 0, &mut fields);
    (outcome, fields)
}

/// Old and new on one input; panics with the input's label on any difference.
fn same(raw: &Value, name: &str, schema: &Value, label: &str) -> bool {
    let reference = run(walk_serialized_support_reference, raw, name, schema);
    let product = run(walk_serialized_support, raw, name, schema);
    assert_eq!(product, reference, "walks differ on {label}");
    reference.is_ok()
}

#[derive(Default)]
struct Tally {
    inputs: usize,
    accepted: usize,
}

impl Tally {
    fn check(&mut self, raw: &Value, schema: &Schema, label: &str) {
        self.inputs += 1;
        if same(raw, schema.name, &schema.value, label) {
            self.accepted += 1;
        }
    }
}

struct Schema {
    name: &'static str,
    value: Value,
}

fn schemas() -> [Schema; 4] {
    [
        Schema {
            name: "document",
            value: schemars::schema_for!(Document).to_value(),
        },
        Schema {
            name: "operation",
            value: schemars::schema_for!(Operation).to_value(),
        },
        Schema {
            name: "clip",
            value: schemars::schema_for!(Clip).to_value(),
        },
        Schema {
            name: "history",
            value: schemars::schema_for!(HistorySnapshot).to_value(),
        },
    ]
}

// ---- inputs --------------------------------------------------------------------------

/// The saved-project fixtures the support tests and the project tests read.
fn fixtures() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "pre_m13_project",
            include_str!("../../tests/fixtures/pre_m13_project.json"),
        ),
        (
            "legacy-input",
            include_str!("../../../kinewright-agent/tests/fixtures/ja/legacy-input.kinewright"),
        ),
        (
            "s1-migration-input",
            include_str!(
                "../../../kinewright-agent/tests/fixtures/ja/s1-migration-input.kinewright"
            ),
        ),
        (
            "synthetic-blue-project",
            include_str!(
                "../../../kinewright-agent/tests/fixtures/ja/synthetic-blue-project.kinewright"
            ),
        ),
        (
            "synthetic-red-project",
            include_str!(
                "../../../kinewright-agent/tests/fixtures/ja/synthetic-red-project.kinewright"
            ),
        ),
        (
            "unsupported-v3-code",
            include_str!(
                "../../../kinewright-agent/tests/fixtures/ja/unsupported-v3-code.kinewright"
            ),
        ),
        (
            "unsupported-v3-studio",
            include_str!(
                "../../../kinewright-agent/tests/fixtures/ja/unsupported-v3-studio.kinewright"
            ),
        ),
        (
            "writer-v1-empty",
            include_str!("../../../kinewright-agent/tests/fixtures/ja/writer-v1-empty.kinewright"),
        ),
        (
            "writer-v1-legacy",
            include_str!("../../../kinewright-agent/tests/fixtures/ja/writer-v1-legacy.kinewright"),
        ),
        (
            "writer-v1-track",
            include_str!("../../../kinewright-agent/tests/fixtures/ja/writer-v1-track.kinewright"),
        ),
        (
            "writer-v2-adjustment",
            include_str!(
                "../../../kinewright-agent/tests/fixtures/ja/writer-v2-adjustment.kinewright"
            ),
        ),
    ]
}

fn au6_asset(id: AssetId, kind: MediaKind, rate: u32, frames: i64) -> MediaAsset {
    let fps = Rational::new(rate, 1).expect("a positive frame rate");
    MediaAsset {
        id,
        path: std::path::PathBuf::from(format!("au6-asset-{}", id.0)),
        name: format!("au6 asset {}", id.0),
        duration: TimeCode(frames),
        fps,
        kind,
        resolution: (kind != MediaKind::Audio).then_some((
            crate::au6_scenarios::AU6_SOURCE_WIDTH,
            crate::au6_scenarios::AU6_SOURCE_HEIGHT,
        )),
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: ColorDescription::default(),
        assumed_from: None,
    }
}

/// An AU6 scenario's base document, built from its spec the way `tests/au6_core.rs` does.
fn au6_base(scenario: Au6Scenario) -> Document {
    let spec = crate::au6_spec(scenario);
    let tracks = spec
        .tracks
        .iter()
        .map(|track| Track {
            id: track.track,
            kind: track.kind,
            sync_lock: track.sync_lock,
            clips: spec
                .clips
                .iter()
                .filter(|clip| clip.track == track.track)
                .map(|clip| Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: clip.clip,
                    asset: clip.asset,
                    source_range: clip.range(),
                    content: ClipContent::Media,
                    timeline_start: clip.start,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                    blend_mode: crate::BlendMode::Normal,
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut media_pool: Vec<MediaAsset> = Vec::new();
    for clip in spec.clips {
        if media_pool.iter().any(|entry| entry.id == clip.asset) {
            continue;
        }
        let kind = match spec.tracks.iter().find(|t| t.track == clip.track) {
            Some(track) if track.kind == crate::TrackKind::Video => MediaKind::Video,
            _ => MediaKind::Audio,
        };
        media_pool.push(au6_asset(
            clip.asset,
            kind,
            spec.fps,
            i64::from(spec.asset_frames),
        ));
    }
    let mut document = Document {
        tracks,
        media_pool,
        fps: Rational::new(spec.fps, 1).expect("a positive frame rate"),
        resolution: (
            crate::au6_scenarios::AU6_SOURCE_WIDTH,
            crate::au6_scenarios::AU6_SOURCE_HEIGHT,
        ),
        duration: TimeCode(i64::from(spec.frames)),
        ..Document::default()
    };
    if scenario == Au6Scenario::Multicam {
        document.catalog.sync_groups.push(crate::au6_d_sync_group(
            crate::au6_scenarios::AU6_D_ANGLE_ASSETS,
        ));
    }
    document
}

fn apply(core: &Core, command: Command) -> bool {
    matches!(core.request(command), Ok(Event::DocumentChanged { .. }))
}

/// A real exported history: the scenario's canonical operations one batch each, then
/// `toggles` sync-lock toggles, then two undos so the redo side is populated too.
fn au6_history(scenario: Au6Scenario, toggles: u32) -> (Value, usize) {
    let core = Core::spawn(au6_base(scenario)).expect("base document is valid");
    let mut accepted = 0;
    for operation in crate::au6_canonical_operations(scenario) {
        accepted += usize::from(apply(&core, Command::Do(operation)));
    }
    let track = au6_base(scenario).tracks[0].id;
    for index in 0..toggles {
        accepted += usize::from(apply(
            &core,
            Command::Do(Operation::SetTrackSyncLock {
                track,
                locked: index % 2 == 0,
            }),
        ));
    }
    apply(&core, Command::Undo);
    apply(&core, Command::Undo);
    let history = core.export_history().expect("history exports");
    (
        serde_json::to_value(&history).expect("history serializes"),
        accepted,
    )
}

fn operations() -> Vec<Operation> {
    let mut operations = Vec::new();
    for scenario in crate::AU6_SCENARIOS {
        operations.extend(crate::au6_canonical_operations(scenario));
    }
    for scenario in crate::CC7_SCENARIOS {
        operations.extend(crate::cc7_canonical_operations(scenario));
    }
    operations.extend(crate::cc7_b1_canonical_operations());
    operations.extend(crate::cc7_d2_canonical_operations());
    operations
}

// ---- mutations -----------------------------------------------------------------------

/// Every node's JSON pointer, root first.
fn pointers(value: &Value) -> Vec<String> {
    fn walk(value: &Value, here: &str, out: &mut Vec<String>) {
        out.push(here.to_owned());
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    walk(child, &format!("{here}/{escaped}"), out);
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    walk(child, &format!("{here}/{index}"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

const MUTATIONS: usize = 10;

/// Mutation `kind` applied at `pointer`; `None` when it does not apply to that node.
fn mutate(value: &Value, pointer: &str, kind: usize) -> Option<Value> {
    let mut out = value.clone();
    if kind == 0 {
        // Drop the node from its parent (a required key, an array element).
        let (parent, last) = pointer.rsplit_once('/')?;
        let last = last.replace("~1", "/").replace("~0", "~");
        match out.pointer_mut(parent)? {
            Value::Object(object) => {
                object.remove(&last)?;
            }
            Value::Array(array) => {
                let index: usize = last.parse().ok()?;
                array.remove(index);
            }
            _ => return None,
        }
        return Some(out);
    }
    let node = out.pointer_mut(pointer)?;
    let replacement = match (kind, &*node) {
        (1, Value::Object(object)) => {
            let mut object = object.clone();
            object.insert("zz_unknown_field".into(), json!(1));
            Value::Object(object)
        }
        (2, _) => json!("Bogus"),
        // Swap the type.
        (3, Value::Number(_)) => json!("7"),
        (3, Value::String(_)) => json!(7),
        (3, Value::Bool(_)) => json!("true"),
        (3, Value::Null) => json!(false),
        (3, Value::Array(_)) => json!({}),
        (3, Value::Object(_)) => json!([]),
        (4, _) => json!(-1),
        (5, _) => json!(1.5),
        // A different externally tagged variant around the same payload.
        (6, current) => json!({ "Bogus": current }),
        // A changed tag or enum spelling.
        (7, Value::String(text)) => json!(format!("{text}x")),
        (8, Value::Object(object)) if object.len() == 1 => {
            let (key, child) = object.iter().next()?;
            json!({ format!("{key}X"): child })
        }
        (9, _) => Value::Null,
        _ => return None,
    };
    *node = replacement;
    Some(out)
}

/// Every mutation at every node of `value`.
fn exhaustive(tally: &mut Tally, value: &Value, schema: &Schema, label: &str) {
    for pointer in pointers(value) {
        for kind in 0..MUTATIONS {
            if let Some(mutated) = mutate(value, &pointer, kind) {
                tally.check(
                    &mutated,
                    schema,
                    &format!("{label} {pointer} mutation {kind}"),
                );
            }
        }
    }
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).expect("fits")).expect("fits")
    }
}

/// `count` seeded mutations of `value` that apply.
fn sampled(
    tally: &mut Tally,
    value: &Value,
    schema: &Schema,
    label: &str,
    count: usize,
    seed: u64,
) {
    let pointers = pointers(value);
    let mut random = XorShift(seed);
    let mut done = 0;
    while done < count {
        let pointer = &pointers[random.below(pointers.len())];
        let kind = random.below(MUTATIONS);
        if let Some(mutated) = mutate(value, pointer, kind) {
            tally.check(
                &mutated,
                schema,
                &format!("{label} seed {seed} {pointer} mutation {kind}"),
            );
            done += 1;
        }
    }
}

fn nested(depth: usize) -> Value {
    let mut value = json!(0);
    for _ in 0..depth {
        value = json!([value]);
    }
    value
}

// ---- tests ---------------------------------------------------------------------------

/// Fixtures, built documents and every scenario operation, each unchanged and under every
/// mutation at every node; every clip; real exported histories unchanged and under seeded
/// mutations (every node of one history in the larger run). All four compiled schemas.
#[test]
#[allow(clippy::too_many_lines)]
fn product_walk_matches_reference_walk_on_real_records_and_mutations() {
    let requested = std::env::var("KINEWRIGHT_SUPPORT_DIFF_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let samples = requested.unwrap_or(4);
    let [document, operation, clip, history] = schemas();
    let mut originals = Tally::default();
    let mut mutated = Tally::default();

    for (label, text) in fixtures() {
        let raw: Value = serde_json::from_str(text).expect("fixture is JSON");
        originals.check(&raw, &document, label);
        exhaustive(&mut mutated, &raw, &document, label);
        // The envelope's version key is not a document field; also try the bare document.
        if let Some(object) = raw.as_object()
            && object.contains_key("format_version")
        {
            let mut bare = raw.clone();
            bare.as_object_mut()
                .expect("object")
                .remove("format_version");
            originals.check(&bare, &document, &format!("{label} without format_version"));
        }
    }

    let documents = [
        Document::default(),
        au6_base(Au6Scenario::Interview),
        au6_base(Au6Scenario::Multicam),
    ];
    for (index, built) in documents.iter().enumerate() {
        let raw = serde_json::to_value(built).expect("document serializes");
        let label = format!("built document {index}");
        originals.check(&raw, &document, &label);
        exhaustive(&mut mutated, &raw, &document, &label);
        for (clip_index, raw_clip) in raw["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|track| track["clips"].as_array().into_iter().flatten())
            .enumerate()
        {
            originals.check(raw_clip, &clip, &format!("{label} clip {clip_index}"));
        }
    }

    for (index, op) in operations().iter().enumerate() {
        let raw = serde_json::to_value(op).expect("operation serializes");
        let label = format!("scenario operation {index}");
        originals.check(&raw, &operation, &label);
        exhaustive(&mut mutated, &raw, &operation, &label);
    }

    // Small real histories (every node mutated) and one long one (seeded mutations).
    let mut replayed = 0;
    for scenario in crate::AU6_SCENARIOS {
        let (raw, accepted) = au6_history(scenario, 3);
        replayed += accepted;
        let label = format!("{scenario:?} history");
        originals.check(&raw, &history, &label);
        // Every document and clip inside the history against its own schema, too.
        for key in ["baseline", "document"] {
            originals.check(&raw[key], &document, &format!("{label} {key}"));
        }
        sampled(
            &mut mutated,
            &raw,
            &history,
            &label,
            10 * samples,
            0x5eed_0001,
        );
    }
    let (small, _) = au6_history(Au6Scenario::Interview, 3);
    exhaustive(
        &mut mutated,
        &small["journal"]["batches"][0],
        &history,
        "first batch alone",
    );
    if requested.is_some() {
        exhaustive(&mut mutated, &small, &history, "Interview history");
    }

    let (long, _) = au6_history(Au6Scenario::Podcast, 300);
    let batches = long["journal"]["batches"].as_array().map_or(0, Vec::len);
    assert!(batches > 300, "the long history has {batches} batches");
    originals.check(&long, &history, "long history");
    sampled(
        &mut mutated,
        &long,
        &history,
        "long history",
        samples,
        0x5eed_0002,
    );

    eprintln!(
        "SUPPORT_DIFF originals={} accepted={} mutated={} mutated_accepted={} \
         replayed_operations={replayed} long_batches={batches} long_samples={samples}",
        originals.inputs, originals.accepted, mutated.inputs, mutated.accepted
    );
    // The inputs must exercise both answers, or the comparison proves little.
    assert!(originals.accepted >= originals.inputs / 2);
    assert!(mutated.accepted > 0 && mutated.accepted < mutated.inputs);
}

/// Nesting past the depth limit is refused with the same error by both walks, on a
/// compiled schema and a recursive synthetic one.
#[test]
fn depth_limit_is_identical() {
    let recursive = json!({
        "$ref": "#/definitions/n",
        "definitions": { "n": { "type": "array", "items": { "$ref": "#/definitions/n" } } }
    });
    for depth in [10, 60, 63, 64, 65, 200] {
        let value = nested(depth);
        let reference = run(walk_serialized_support_reference, &value, "x", &recursive);
        let product = run(walk_serialized_support, &value, "x", &recursive);
        assert_eq!(product, reference, "depth {depth}");
    }
    assert_eq!(
        run(walk_serialized_support, &nested(200), "x", &recursive),
        Err("raw support depth exceeded".to_owned())
    );
    let [document, ..] = schemas();
    let mut deep = serde_json::to_value(Document::default()).expect("serializes");
    deep["tracks"] = nested(200);
    assert!(!same(&deep, document.name, &document.value, "deep tracks"));
}

/// An alternative that records fields and then fails contributes none of them, in the
/// old walk and the new; the next alternative that succeeds wins, and every field
/// collected before the `anyOf` is kept.
#[test]
fn failed_alternative_leaks_no_fields() {
    let schema = json!({
        "type": "object",
        "properties": {
            "before": {},
            "x": {
                "anyOf": [
                    // Records x.a and x.b, then fails on x.b's type.
                    {
                        "type": "object",
                        "properties": { "a": {}, "b": { "type": "string" } }
                    },
                    // Nested: the first `allOf` member records x.a and x.b, and x.b's inner
                    // `anyOf` succeeds and records x.b.d; then the second member fails on a
                    // missing key, so the whole outer alternative fails.
                    {
                        "allOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "a": {},
                                    "b": { "anyOf": [
                                        { "type": "object", "properties": { "d": {} } }
                                    ] }
                                }
                            },
                            { "required": ["c"] }
                        ]
                    },
                    // Succeeds through additionalProperties: only x.* is recorded.
                    { "type": "object", "additionalProperties": {} }
                ]
            }
        }
    });
    let value = json!({ "before": 1, "x": { "a": 1, "b": { "d": 1 } } });
    // The nested alternative really runs its inner `anyOf` before failing: walked on its
    // own, both walks fail on the missing key and leave the same partial fields, x.b.d
    // among them.
    let nested = &schema["properties"]["x"]["anyOf"][1];
    let partial: BTreeSet<String> = ["v.x.a", "v.x.b", "v.x.b.d"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for walk in [
        walk_serialized_support_reference as Walk,
        walk_serialized_support,
    ] {
        assert_eq!(
            run_keeping(walk, &value["x"], "v.x", nested, &schema, BTreeSet::new()),
            (
                Err("missing required field at v.x".to_owned()),
                partial.clone()
            )
        );
    }
    let expected: BTreeSet<String> = ["v.before", "v.x", "v.x.*"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        run(walk_serialized_support_reference, &value, "v", &schema),
        Ok(expected.clone())
    );
    assert_eq!(
        run(walk_serialized_support, &value, "v", &schema),
        Ok(expected)
    );

    // Every alternative fails: the same error, nothing returned.
    let none = json!({ "before": 1, "x": 3 });
    let refused = Err("unsupported variant at v.x".to_owned());
    assert_eq!(
        run(walk_serialized_support_reference, &none, "v", &schema),
        refused
    );
    assert_eq!(run(walk_serialized_support, &none, "v", &schema), refused);

    // The first alternative wins when both would succeed (order is unchanged).
    let first = json!({ "before": 1, "x": { "a": 1, "b": "s" } });
    let first_fields: BTreeSet<String> = ["v.before", "v.x", "v.x.a", "v.x.b"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        run(walk_serialized_support, &first, "v", &schema),
        Ok(first_fields.clone())
    );
    assert_eq!(
        run(walk_serialized_support_reference, &first, "v", &schema),
        Ok(first_fields)
    );
}

fn set(fields: &[&str]) -> BTreeSet<String> {
    fields.iter().copied().map(str::to_owned).collect()
}

/// A failed alternative reached through a local `$ref`, and one inside `prefixItems`,
/// contributes none of its fields; the alternative after it wins. Old and new agree, and
/// both equal the expected set.
#[test]
fn failed_alternative_through_ref_and_prefix_items_leaks_no_fields() {
    let schema = json!({
        "definitions": {
            "strict": { "type": "object", "properties": { "a": {}, "b": { "type": "string" } } },
            "loose": { "type": "object", "properties": { "a": {} }, "additionalProperties": {} }
        },
        "type": "object",
        "properties": {
            "r": { "anyOf": [
                { "$ref": "#/definitions/strict" },
                { "$ref": "#/definitions/loose" }
            ] },
            "t": { "anyOf": [
                { "type": "array", "prefixItems": [ { "$ref": "#/definitions/strict" } ] },
                { "type": "array", "prefixItems": [ { "$ref": "#/definitions/loose" } ] }
            ] }
        }
    });
    let value = json!({ "r": { "a": 1, "b": 2 }, "t": [ { "a": 1, "b": 2 } ] });
    let expected = set(&["v.r", "v.r.a", "v.r.*", "v.t", "v.t[].a", "v.t[].*"]);
    assert_eq!(
        run(walk_serialized_support_reference, &value, "v", &schema),
        Ok(expected.clone())
    );
    assert_eq!(
        run(walk_serialized_support, &value, "v", &schema),
        Ok(expected)
    );
}

/// After an `Err`, both walks leave the same fields in the caller's set: the fields an
/// `allOf` member recorded before a later member failed (no alternative involved), and the
/// incoming and sibling fields when every alternative of a child fails.
#[test]
fn partial_fields_after_an_error_are_identical() {
    let all_of = json!({
        "type": "object",
        "properties": {
            "x": { "allOf": [
                { "type": "object", "properties": { "a": {} } },
                { "required": ["c"] }
            ] }
        }
    });
    let failing_alternatives = json!({
        "type": "object",
        "properties": {
            "before": {},
            "x": { "anyOf": [
                { "type": "object", "properties": { "a": {}, "b": { "type": "string" } } },
                { "type": "string" }
            ] }
        }
    });
    let cases = [
        (
            &all_of,
            json!({ "x": { "a": 1 } }),
            set(&["pre"]),
            "missing required field at v.x",
            set(&["pre", "v.x", "v.x.a"]),
        ),
        (
            &failing_alternatives,
            json!({ "before": 1, "x": { "a": 1, "b": 2 } }),
            // v.x.a is also recorded by the failed alternative: it stays, as it came in.
            set(&["pre", "v.x.a"]),
            "unsupported variant at v.x",
            set(&["pre", "v.x.a", "v.before", "v.x"]),
        ),
    ];
    for (schema, value, incoming, error, left) in cases {
        for walk in [
            walk_serialized_support_reference as Walk,
            walk_serialized_support,
        ] {
            assert_eq!(
                run_keeping(walk, &value, "v", schema, schema, incoming.clone()),
                (Err(error.to_owned()), left.clone())
            );
        }
    }
}

// ---- seeded synthetic schemas ----------------------------------------------------------

const KEYS: [&str; 4] = ["a", "b", "c", "d"];

/// A random schema whose `anyOf` alternatives overlap: several alternatives accept the
/// same keys, so one can record fields, fail deeper down, and leave the next to succeed.
/// That is the case the product change touches; no compiled-schema input in the default
/// corpus reaches a later winner after such a failure. Besides objects, arrays and
/// alternatives it makes `allOf` lists, `prefixItems` tuples and local `$ref`s to the two
/// definitions `random_definitions` puts at the root.
fn random_schema(random: &mut XorShift, depth: u32) -> Value {
    let leaf = depth >= 3;
    match random.below(if leaf { 4 } else { 10 }) {
        0 => {
            let kind = ["string", "number", "boolean", "null", "object", "array"][random.below(6)];
            json!({ "type": kind })
        }
        1 => json!({ "enum": [1, "x", null] }),
        2 => {
            let constant = [json!("x"), json!(1)][random.below(2)].clone();
            json!({ "const": constant })
        }
        3 => json!({}),
        4 | 5 => {
            let mut properties = serde_json::Map::new();
            for key in KEYS {
                if random.below(2) == 0 {
                    properties.insert(key.to_owned(), random_schema(random, depth + 1));
                }
            }
            let mut schema = json!({ "type": "object", "properties": properties });
            if random.below(3) == 0 {
                schema["required"] = json!([KEYS[random.below(KEYS.len())]]);
            }
            match random.below(4) {
                0 => schema["additionalProperties"] = json!(false),
                1 => schema["additionalProperties"] = random_schema(random, depth + 1),
                2 => {
                    schema.as_object_mut().expect("object").remove("properties");
                    schema["additionalProperties"] = random_schema(random, depth + 1);
                }
                _ => {}
            }
            schema
        }
        6 => {
            let count = 2 + random.below(3);
            let key = ["anyOf", "oneOf"][random.below(2)];
            let alternatives: Vec<Value> = (0..count)
                .map(|_| random_schema(random, depth + 1))
                .collect();
            json!({ key: alternatives })
        }
        7 => {
            let mut schema = json!({ "type": "array", "items": random_schema(random, depth + 1) });
            if random.below(2) == 0 {
                let tuple: Vec<Value> = (0..=random.below(2))
                    .map(|_| random_schema(random, depth + 1))
                    .collect();
                schema["prefixItems"] = Value::Array(tuple);
            }
            schema
        }
        8 => {
            let members: Vec<Value> = (0..2 + random.below(2))
                .map(|_| random_schema(random, depth + 1))
                .collect();
            json!({ "allOf": members })
        }
        _ => json!({ "$ref": format!("#/definitions/d{}", random.below(2)) }),
    }
}

/// The definitions a generated `$ref` points to. They may refer to each other, so a cycle
/// ends at the depth limit, with the same error in both walks.
fn random_definitions(random: &mut XorShift) -> Value {
    json!({ "d0": random_schema(random, 2), "d1": random_schema(random, 2) })
}

fn random_value(random: &mut XorShift, depth: u32) -> Value {
    match random.below(if depth >= 3 { 5 } else { 7 }) {
        0 => json!(1),
        1 => json!("x"),
        2 => Value::Null,
        3 => json!(true),
        4 => json!(2.5),
        5 => {
            let mut object = serde_json::Map::new();
            for key in KEYS {
                if random.below(2) == 0 {
                    object.insert(key.to_owned(), random_value(random, depth + 1));
                }
            }
            Value::Object(object)
        }
        _ => Value::Array(
            (0..random.below(3))
                .map(|_| random_value(random, depth + 1))
                .collect(),
        ),
    }
}

/// Old and new agree on seeded random schemas and values, including the inputs where a
/// failed alternative recorded fields before a later one succeeded (counted, and required
/// to occur, so the run is known to reach the changed branch).
#[test]
fn product_walk_matches_reference_walk_on_seeded_synthetic_schemas() {
    let cases = std::env::var("KINEWRIGHT_SUPPORT_DIFF_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(20_000, |samples| samples * 5_000);
    let mut random = XorShift(0x5eed_0003);
    let (mut accepted, mut reached) = (0, 0);
    for case in 0..cases {
        let mut schema = json!({
            "type": "object",
            "properties": { "x": { "anyOf": [] } }
        });
        let alternatives: Vec<Value> = (0..2 + random.below(3))
            .map(|_| random_schema(&mut random, 1))
            .collect();
        schema["properties"]["x"]["anyOf"] = Value::Array(alternatives.clone());
        schema["definitions"] = random_definitions(&mut random);
        let value = json!({ "x": random_value(&mut random, 1) });
        if same(&value, "v", &schema, &format!("synthetic case {case}")) {
            accepted += 1;
            // Did an alternative before the winner record fields and then fail?
            let mut recorded_then_failed = false;
            for alternative in &alternatives {
                let mut scratch = BTreeSet::new();
                let outcome = walk_serialized_support_reference(
                    &value["x"],
                    alternative,
                    &schema,
                    "v.x",
                    2,
                    &mut scratch,
                );
                if outcome.is_ok() {
                    break;
                }
                recorded_then_failed |= !scratch.is_empty();
            }
            reached += usize::from(recorded_then_failed);
        }
    }
    eprintln!(
        "SUPPORT_DIFF_SYNTHETIC cases={cases} accepted={accepted} failed_alternative_with_fields_then_success={reached}"
    );
    assert!(accepted > 0 && accepted < cases);
    assert!(reached > 0, "no case reached the changed branch");
}
