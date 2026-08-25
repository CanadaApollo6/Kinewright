//! CC4 look-management core contracts.
//!
//! These tests hold the parts of `docs/CC4-LOOK-MANAGEMENT.md` that Core owns:
//! the §2.1 asset record and its validation, the §2.7 operations, the §3.1
//! kinds and roles, the §3.2 stage-ordering rule, the §3.3 integer asset
//! reference, the §3.6 inactivity rules, the §5 control tables, and the §6
//! keyframing policy. Every expected value is transcribed from the document by
//! hand rather than read back out of the descriptor tables.

use std::{collections::BTreeMap, path::PathBuf};

use kinewright_core::{
    AssetId, AutomationCurve, Clip, ClipContent, ClipId, ColorContext, ColorDescription,
    ColorNodeInactiveReason, ColorNodeKind, ColorStage, Command, Core, Document, Effect, EffectId,
    EffectUniform, Event, JournalCommand, Keyframe, KeyframeInterpolation, LUT_ASSET_ID_MAX,
    LUT_NODE_LIMIT_PER_LAYER, LUT_SIZE_MAX, LUT_SIZE_MIN, LutAsset, LutAssetId, LutAssetKind,
    LutAssetSource, LutAvailabilityKind, LutAvailabilityStatus, LutNodeParams, MediaAsset,
    MediaKind, OpError, Operation, ParamValue, QaSeverity, Rational, TimeCode, Track, TrackId,
    TrackKind, active_color_nodes, classify_color_node, color_node_inactive_reason,
    color_stage_order_violation, effect_compatibility_stage, effect_descriptor,
    export_lut_preflight_with, is_lut_color_node, is_managed_color_node, lut_node_count,
    lut_node_may_be_active, qa_document, validate_lut_asset,
};

/// A path that always exists so `missing_media` never masks a colour issue.
fn present_media_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

fn supported_source_description() -> ColorDescription {
    ColorContext::sdr_rec709().delivery
}

/// A 64-character lowercase hexadecimal digest derived from an id.
fn digest(seed: u64) -> String {
    format!("{seed:064x}")
}

/// The §2.1 record for one imported look, valid in every field.
fn imported_asset(id: u64) -> LutAsset {
    LutAsset {
        id: LutAssetId(id),
        sha256: digest(id),
        title: format!("Look {id}"),
        kind: LutAssetKind::Cube3d,
        size: 33,
        byte_len: 1_174_896,
        domain_min_millionths: [0, 0, 0],
        domain_max_millionths: [1_000_000, 1_000_000, 1_000_000],
        source: LutAssetSource::Imported {
            source_path: format!("/looks/look{id}.cube"),
        },
    }
}

fn managed_document() -> Document {
    let asset = MediaAsset {
        id: AssetId(1),
        path: present_media_path(),
        name: "managed-source".to_owned(),
        duration: TimeCode(120),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: supported_source_description(),
    };
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: AssetId(1),
                source_range: TimeCode(0)..TimeCode(30),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        duration: TimeCode(30),
        ..Document::default()
    }
}

/// A managed document that already owns two registered LUT assets.
fn document_with_assets() -> Document {
    let mut document = managed_document();
    for id in [1, 2] {
        Operation::AddLutAsset {
            asset: imported_asset(id),
        }
        .apply(&mut document)
        .expect("a well-formed asset registers");
    }
    document
}

fn effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

/// A `creative_look` bound to `asset`, the shape the planners emit.
fn look(id: u64, asset: i64) -> Effect {
    effect(id, "creative_look", &[("lut_asset_id", asset)])
}

/// A `technical_lut` bound to `asset`.
fn technical(id: u64, asset: i64) -> Effect {
    effect(id, "technical_lut", &[("lut_asset_id", asset)])
}

fn add(document: &mut Document, effect: Effect) -> Result<(), OpError> {
    Operation::AddEffect {
        clip: ClipId(1),
        effect,
    }
    .apply(document)
}

fn insert(document: &mut Document, index: usize, effect: Effect) -> Result<(), OpError> {
    Operation::InsertEffect {
        clip: ClipId(1),
        index,
        effect,
    }
    .apply(document)
}

fn set_param(document: &mut Document, id: u64, name: &str, value: i64) -> Result<(), OpError> {
    Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(id),
        name: name.to_owned(),
        value: ParamValue::Integer(value),
    }
    .apply(document)
}

fn set_keyframes(
    document: &mut Document,
    id: u64,
    name: &str,
    keyframes: &[(i64, i64, KeyframeInterpolation)],
) -> Result<(), OpError> {
    Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(id),
        name: name.to_owned(),
        curve: AutomationCurve {
            keyframes: keyframes
                .iter()
                .map(|(at, value, interpolation)| Keyframe {
                    at: TimeCode(*at),
                    value: *value,
                    interpolation: *interpolation,
                })
                .collect(),
        },
    }
    .apply(document)
}

fn effect_names(document: &Document) -> Vec<&str> {
    document
        .clip(ClipId(1))
        .expect("clip")
        .effects
        .iter()
        .map(|effect| effect.name.as_str())
        .collect()
}

fn effect_ids(document: &Document) -> Vec<u64> {
    document
        .clip(ClipId(1))
        .expect("clip")
        .effects
        .iter()
        .map(|effect| effect.id.0)
        .collect()
}

// ---------------------------------------------------------------------------
// §5 control tables
// ---------------------------------------------------------------------------

/// CC4 §5.1: the `technical_lut` table, transcribed by hand.
#[test]
fn technical_lut_descriptor_matches_the_contract_table() {
    let descriptor = effect_descriptor("technical_lut").expect("technical_lut is registered");
    let expected: [(&str, i64, i64, i64); 4] = [
        ("lut_asset_id", 0, 9_007_199_254_740_991, 0),
        ("mix_basis_points", 10_000, 10_000, 10_000),
        ("input_encoding_token", 0, 2, 0),
        ("bypass", 0, 1, 0),
    ];
    assert_eq!(descriptor.parameters.len(), expected.len());
    for (parameter, (name, min, max, neutral)) in descriptor.parameters.iter().zip(expected) {
        assert_eq!(parameter.name, name);
        assert_eq!(
            (parameter.min, parameter.max, parameter.neutral),
            (min, max, neutral),
            "{name}"
        );
        assert_eq!(parameter.uniform, EffectUniform::ColorNode, "{name}");
    }
}

/// CC4 §5.2: the `creative_look` table differs only in the mix minimum.
#[test]
fn creative_look_descriptor_matches_the_contract_table() {
    let descriptor = effect_descriptor("creative_look").expect("creative_look is registered");
    let expected: [(&str, i64, i64, i64); 4] = [
        ("lut_asset_id", 0, 9_007_199_254_740_991, 0),
        ("mix_basis_points", 0, 10_000, 10_000),
        ("input_encoding_token", 0, 2, 0),
        ("bypass", 0, 1, 0),
    ];
    assert_eq!(descriptor.parameters.len(), expected.len());
    for (parameter, (name, min, max, neutral)) in descriptor.parameters.iter().zip(expected) {
        assert_eq!(parameter.name, name);
        assert_eq!(
            (parameter.min, parameter.max, parameter.neutral),
            (min, max, neutral),
            "{name}"
        );
        assert_eq!(parameter.uniform, EffectUniform::ColorNode, "{name}");
    }
}

/// CC4 §2.1 and §3.3: the descriptor bound is the model bound, not a copy that
/// can drift.
#[test]
fn the_asset_reference_bound_is_two_to_the_fifty_three_minus_one() {
    assert_eq!(LUT_ASSET_ID_MAX, 9_007_199_254_740_991);
    assert_eq!(LUT_ASSET_ID_MAX, (1_u64 << 53) - 1);
    for name in ["technical_lut", "creative_look"] {
        let parameter = effect_descriptor(name)
            .expect("registered")
            .parameter("lut_asset_id")
            .expect("asset reference");
        assert_eq!(u64::try_from(parameter.max).unwrap(), LUT_ASSET_ID_MAX);
    }
    assert_eq!((LUT_SIZE_MIN, LUT_SIZE_MAX), (2, 65));
    assert_eq!(LUT_NODE_LIMIT_PER_LAYER, 4);
}

/// CC4 §3.1: kinds, roles, stages, and storage tags.
#[test]
fn the_two_new_kinds_are_managed_stage_ordered_nodes() {
    assert_eq!(ColorNodeKind::TechnicalLut.effect_name(), "technical_lut");
    assert_eq!(ColorNodeKind::CreativeLook.effect_name(), "creative_look");
    assert_eq!(ColorNodeKind::TechnicalLut.storage_buffer_tag(), 4);
    assert_eq!(ColorNodeKind::CreativeLook.storage_buffer_tag(), 5);
    assert_eq!(ColorNodeKind::TechnicalLut.role(), "technical");
    assert_eq!(ColorNodeKind::Primary.role(), "correction");
    assert_eq!(ColorNodeKind::Wheels.role(), "correction");
    assert_eq!(ColorNodeKind::Curves.role(), "correction");
    assert_eq!(ColorNodeKind::CreativeLook.role(), "creative");
    assert_eq!(ColorNodeKind::TechnicalLut.stage(), ColorStage::Input);
    assert_eq!(ColorNodeKind::Curves.stage(), ColorStage::Correction);
    assert_eq!(ColorNodeKind::CreativeLook.stage(), ColorStage::Look);
    assert_eq!(
        ColorStage::ALL.map(ColorStage::rank),
        [0, 1, 2],
        "stage ranks are the §3.1 table numbers"
    );
    assert_eq!(
        ColorStage::ALL.map(ColorStage::as_str),
        ["input", "correction", "look"]
    );
    for name in ["technical_lut", "creative_look"] {
        assert!(is_managed_color_node(name));
        assert!(is_lut_color_node(name));
        assert_eq!(
            effect_compatibility_stage(name),
            None,
            "{name} is inside the managed conformance claim"
        );
    }
    for name in ["primary_correction", "color_wheels", "color_curves"] {
        assert!(!is_lut_color_node(name));
    }
}

// ---------------------------------------------------------------------------
// §2.1 asset record
// ---------------------------------------------------------------------------

/// CC4 §2.1: the serialized record has exactly the documented shape.
#[test]
fn lut_asset_serializes_with_the_documented_json_shape() {
    let imported = LutAsset {
        sha256: "3f5c9d0b".to_owned() + &"0".repeat(56),
        title: "Kodak 2383 D65".to_owned(),
        ..imported_asset(1)
    };
    let value = serde_json::to_value(&imported).expect("record serializes");
    assert_eq!(value["id"], 1);
    assert_eq!(value["sha256"], "3f5c9d0b".to_owned() + &"0".repeat(56));
    assert_eq!(value["title"], "Kodak 2383 D65");
    assert_eq!(value["kind"], "cube_3d");
    assert_eq!(value["size"], 33);
    assert_eq!(value["byte_len"], 1_174_896);
    assert_eq!(value["domain_min_millionths"], serde_json::json!([0, 0, 0]));
    assert_eq!(
        value["domain_max_millionths"],
        serde_json::json!([1_000_000, 1_000_000, 1_000_000])
    );
    assert_eq!(
        value["source"],
        serde_json::json!({ "imported": { "source_path": "/looks/look1.cube" } })
    );
    assert_eq!(
        serde_json::from_value::<LutAsset>(value).expect("record round-trips"),
        imported
    );
}

/// CC4 §2.6: a built-in asset records its coined name, not a path.
#[test]
fn builtin_asset_source_serializes_as_a_named_bake() {
    let builtin = LutAsset {
        id: LutAssetId(2),
        title: "Warm".to_owned(),
        size: 17,
        byte_len: 133_650,
        domain_min_millionths: [-1_000_000, -1_000_000, -1_000_000],
        domain_max_millionths: [2_000_000, 2_000_000, 2_000_000],
        source: LutAssetSource::Builtin {
            name: "warm".to_owned(),
        },
        ..imported_asset(2)
    };
    let value = serde_json::to_value(&builtin).expect("record serializes");
    assert_eq!(
        value["source"],
        serde_json::json!({ "builtin": { "name": "warm" } })
    );
    assert_eq!(
        value["domain_min_millionths"],
        serde_json::json!([-1_000_000, -1_000_000, -1_000_000])
    );
    assert_eq!(
        serde_json::from_value::<LutAsset>(value).expect("record round-trips"),
        builtin
    );
    assert_eq!(
        serde_json::to_value(LutAssetKind::Cube1d).expect("kind serializes"),
        "cube_1d"
    );
}

/// CC4 §2.1 and §9.1: a pre-CC4 project loads with no assets and re-saves
/// without the key.
#[test]
fn pre_cc4_projects_round_trip_without_a_lut_assets_key() {
    let document: Document =
        serde_json::from_str(include_str!("fixtures/pre_m13_project.json")).unwrap();
    assert!(document.lut_assets.is_empty());
    document.validate().expect("a pre-CC4 project stays valid");

    let value = serde_json::to_value(&document).expect("document serializes");
    assert!(
        !value
            .as_object()
            .expect("document is an object")
            .contains_key("lut_assets"),
        "an empty asset list must not appear in the saved project"
    );
}

// ---------------------------------------------------------------------------
// §2.7 AddLutAsset / RemoveLutAsset
// ---------------------------------------------------------------------------

/// CC4 §2.7: `AddLutAsset` rejects every malformed record by field.
#[test]
fn add_lut_asset_rejects_malformed_metadata_by_field() {
    let cases: [(LutAsset, &str, &str, &str); 7] = [
        (
            LutAsset {
                id: LutAssetId(0),
                ..imported_asset(3)
            },
            "id",
            "0",
            "1..=9007199254740991",
        ),
        (
            LutAsset {
                title: String::new(),
                ..imported_asset(3)
            },
            "title",
            "",
            "a non-empty title",
        ),
        (
            LutAsset {
                kind: LutAssetKind::Cube1d,
                ..imported_asset(3)
            },
            "kind",
            "cube_1d",
            "cube_3d",
        ),
        (
            LutAsset {
                size: 1,
                ..imported_asset(3)
            },
            "size",
            "1",
            "2..=65",
        ),
        (
            LutAsset {
                size: 66,
                ..imported_asset(3)
            },
            "size",
            "66",
            "2..=65",
        ),
        (
            LutAsset {
                byte_len: 0,
                ..imported_asset(3)
            },
            "byte_len",
            "0",
            "a positive byte length",
        ),
        (
            LutAsset {
                domain_min_millionths: [0, 1_000_000, 0],
                ..imported_asset(3)
            },
            "domain_g_millionths",
            "1000000..1000000",
            "domain_min_millionths < domain_max_millionths",
        ),
    ];
    let base = document_with_assets();
    for (asset, field, observed, allowed) in cases {
        let mut document = base.clone();
        let error = Operation::AddLutAsset {
            asset: asset.clone(),
        }
        .apply(&mut document)
        .expect_err("a malformed record must be rejected");
        assert_eq!(
            error,
            OpError::InvalidLutAssetMetadata {
                field,
                observed: observed.to_owned(),
                allowed,
            }
        );
        assert_eq!(document, base, "a rejection leaves the document untouched");
        assert_eq!(validate_lut_asset(&asset), Err(error));
    }
}

/// CC4 §2.1: the hash is validated exactly as M41 validates a source
/// fingerprint — 64 characters, lowercase.
#[test]
fn add_lut_asset_rejects_a_malformed_content_hash() {
    let base = document_with_assets();
    for spelling in [
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        format!("{}Z", "a".repeat(63)),
        String::new(),
    ] {
        let mut document = base.clone();
        let asset = LutAsset {
            sha256: spelling.clone(),
            ..imported_asset(3)
        };
        let error = Operation::AddLutAsset { asset }
            .apply(&mut document)
            .expect_err("a malformed hash must be rejected");
        assert_eq!(
            error,
            OpError::InvalidLutAssetHash {
                lut_asset: LutAssetId(3),
                observed: spelling,
                allowed: "exactly 64 lowercase hexadecimal characters",
            }
        );
        assert_eq!(document, base);
    }
}

/// CC4 §2.1 and §2.7: ids are unique, and the next id is `max(existing) + 1`.
#[test]
fn add_lut_asset_rejects_a_duplicate_id_and_allocates_the_next_one() {
    let mut document = document_with_assets();
    assert_eq!(document.next_lut_asset_id().unwrap(), LutAssetId(3));

    let error = Operation::AddLutAsset {
        asset: LutAsset {
            title: "A different look with the same id".to_owned(),
            sha256: digest(999),
            ..imported_asset(2)
        },
    }
    .apply(&mut document)
    .expect_err("a duplicate id must be rejected");
    assert_eq!(error, OpError::DuplicateLutAsset(LutAssetId(2)));
    assert_eq!(document.lut_assets.len(), 2);
    assert_eq!(document.lut_asset(LutAssetId(2)).unwrap().title, "Look 2");
    assert_eq!(document.lut_asset(LutAssetId(9)), None);
}

/// CC4 §2.7 and §10.3.12: removal is blocked by an active, a bypassed, and a
/// `Hold`-keyframed reference alike, and never cascades.
#[test]
fn remove_lut_asset_is_blocked_by_every_kind_of_reference() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");
    add(&mut document, look(2, 1)).expect("a second bound look is legal");
    set_param(&mut document, 2, "bypass", 1).expect("bypass is an ordinary parameter");
    add(&mut document, look(3, 2)).expect("a look bound to the second asset is legal");
    set_keyframes(
        &mut document,
        3,
        "lut_asset_id",
        &[
            (0, 2, KeyframeInterpolation::Hold),
            (10, 1, KeyframeInterpolation::Hold),
        ],
    )
    .expect("hold keyframes on the asset reference are legal");

    let remove = Operation::RemoveLutAsset {
        lut_asset: LutAssetId(1),
    };
    assert_eq!(
        remove
            .clone()
            .apply(&mut document.clone())
            .expect_err("in use"),
        OpError::LutAssetInUse {
            lut_asset: LutAssetId(1),
            clip: ClipId(1),
            effect: EffectId(1),
        }
    );

    // The active node goes; the bypassed node still blocks.
    Operation::RemoveEffect {
        clip: ClipId(1),
        effect: EffectId(1),
    }
    .apply(&mut document)
    .expect("removing an effect is legal");
    assert_eq!(
        remove
            .clone()
            .apply(&mut document.clone())
            .expect_err("bypassed nodes count"),
        OpError::LutAssetInUse {
            lut_asset: LutAssetId(1),
            clip: ClipId(1),
            effect: EffectId(2),
        }
    );

    // The bypassed node goes; the hold keyframe on the third node still blocks.
    Operation::RemoveEffect {
        clip: ClipId(1),
        effect: EffectId(2),
    }
    .apply(&mut document)
    .expect("removing an effect is legal");
    assert_eq!(
        remove
            .clone()
            .apply(&mut document.clone())
            .expect_err("hold values count"),
        OpError::LutAssetInUse {
            lut_asset: LutAssetId(1),
            clip: ClipId(1),
            effect: EffectId(3),
        }
    );

    Operation::ClearEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(3),
        name: "lut_asset_id".to_owned(),
    }
    .apply(&mut document)
    .expect("clearing the curve is legal");
    remove
        .apply(&mut document)
        .expect("the last reference is gone");
    assert_eq!(document.lut_asset(LutAssetId(1)), None);
    assert_eq!(document.lut_assets.len(), 1);
    assert_eq!(
        Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        }
        .apply(&mut document)
        .expect_err("the record is gone"),
        OpError::UnknownLutAsset(LutAssetId(1))
    );
}

/// CC4 §2.7 and §6: a reference is a reference whether it is stored or held.
#[test]
fn lut_asset_references_finds_static_and_hold_values() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");
    add(&mut document, look(2, 2)).expect("a second bound look is legal");
    set_keyframes(
        &mut document,
        2,
        "lut_asset_id",
        &[
            (0, 2, KeyframeInterpolation::Hold),
            (15, 1, KeyframeInterpolation::Hold),
        ],
    )
    .expect("hold keyframes are legal");

    assert_eq!(
        document.lut_asset_references(LutAssetId(1)),
        vec![(ClipId(1), EffectId(1)), (ClipId(1), EffectId(2))],
        "the second node references asset 1 only through a hold keyframe"
    );
    assert_eq!(
        document.lut_asset_references(LutAssetId(2)),
        vec![(ClipId(1), EffectId(2))]
    );
    assert_eq!(document.lut_asset_references(LutAssetId(3)), Vec::new());
}

// ---------------------------------------------------------------------------
// §2.7 InsertEffect
// ---------------------------------------------------------------------------

/// CC4 §2.7 and §10.3.6: an insertion is positional and order-preserving.
#[test]
fn insert_effect_places_a_node_at_an_exact_index() {
    let mut document = document_with_assets();
    add(
        &mut document,
        effect(10, "primary_correction", &[("exposure_milli_stops", 250)]),
    )
    .expect("a primary node is legal");
    add(&mut document, effect(11, "color_curves", &[])).expect("a curves node is legal");
    assert_eq!(
        effect_names(&document),
        ["primary_correction", "color_curves"]
    );

    // Index 0: a technical LUT ahead of every correction.
    insert(&mut document, 0, technical(12, 1)).expect("a technical LUT may lead the stack");
    assert_eq!(
        effect_names(&document),
        ["technical_lut", "primary_correction", "color_curves"]
    );
    assert_eq!(effect_ids(&document), [12, 10, 11]);

    // The middle: a second correction between the two existing ones.
    insert(&mut document, 2, effect(13, "color_wheels", &[])).expect("a wheels node is legal");
    assert_eq!(
        effect_names(&document),
        [
            "technical_lut",
            "primary_correction",
            "color_wheels",
            "color_curves"
        ]
    );
    assert_eq!(effect_ids(&document), [12, 10, 13, 11]);

    // `index == len` appends, exactly as `AddEffect` does.
    let len = document.clip(ClipId(1)).unwrap().effects.len();
    insert(&mut document, len, look(14, 2)).expect("appending through an index is legal");
    assert_eq!(effect_ids(&document), [12, 10, 13, 11, 14]);

    // `len + 1` is out of range and changes nothing.
    let before = document.clone();
    let len = document.clip(ClipId(1)).unwrap().effects.len();
    assert_eq!(
        insert(&mut document, len + 1, look(15, 2)).expect_err("past the end"),
        OpError::EffectIndexOutOfRange {
            clip: ClipId(1),
            index: 6,
            len: 5,
        }
    );
    assert_eq!(document, before);
}

/// CC4 §2.7: `InsertEffect` shares `AddEffect`'s validation.
#[test]
fn insert_effect_rejects_what_add_effect_rejects() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");
    assert_eq!(
        insert(&mut document, 0, look(1, 2)).expect_err("duplicate id"),
        OpError::DuplicateEffect {
            clip: ClipId(1),
            effect: EffectId(1),
        }
    );
    assert_eq!(
        insert(&mut document, 0, effect(2, "not_an_effect", &[])).expect_err("unknown effect"),
        OpError::UnknownEffect("not_an_effect".to_owned())
    );
    assert_eq!(
        insert(&mut document, 0, look(3, 99)).expect_err("dangling asset"),
        OpError::MissingLutAsset {
            clip: ClipId(1),
            effect: EffectId(3),
            lut_asset: LutAssetId(99),
        }
    );
}

// ---------------------------------------------------------------------------
// §3.2 stage ordering
// ---------------------------------------------------------------------------

/// CC4 §3.2 and §10.3.6: the legal five-kind stack is accepted in stage order.
#[test]
fn the_five_kind_stage_ordered_stack_is_accepted() {
    let mut document = document_with_assets();
    add(&mut document, technical(1, 1)).expect("technical first");
    add(
        &mut document,
        effect(2, "primary_correction", &[("exposure_milli_stops", 100)]),
    )
    .expect("primary second");
    add(
        &mut document,
        effect(3, "color_wheels", &[("gain_red_thousandths", 1_100)]),
    )
    .expect("wheels third");
    add(&mut document, effect(4, "color_curves", &[])).expect("curves fourth");
    add(&mut document, look(5, 2)).expect("creative look last");

    assert_eq!(
        effect_names(&document),
        [
            "technical_lut",
            "primary_correction",
            "color_wheels",
            "color_curves",
            "creative_look"
        ]
    );
    let effects = &document.clip(ClipId(1)).unwrap().effects;
    assert_eq!(color_stage_order_violation(effects), None);
    assert_eq!(lut_node_count(effects), 2);
    document.validate().expect("the stack is a valid document");
    assert_eq!(
        active_color_nodes(effects)
            .into_iter()
            .map(|(index, kind)| (index, kind.stage()))
            .collect::<Vec<_>>(),
        vec![
            (0, ColorStage::Input),
            (1, ColorStage::Correction),
            (2, ColorStage::Correction),
            (4, ColorStage::Look)
        ],
        "the neutral curves node is inactive and every other node keeps its index"
    );
}

/// CC4 §3.2: a look before a technical LUT is rejected through every path.
#[test]
fn a_technical_lut_after_a_creative_look_is_rejected() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a look alone is legal");

    let expected = OpError::ColorStageOrderViolation {
        clip: ClipId(1),
        effect: EffectId(2),
        kind: "technical_lut".to_owned(),
        color_stage_rank: 0,
        previous_effect: EffectId(1),
        previous_kind: "creative_look".to_owned(),
        previous_color_stage_rank: 2,
    };
    assert_eq!(
        add(&mut document.clone(), technical(2, 2)).expect_err("append is rejected"),
        expected
    );
    assert_eq!(
        insert(&mut document.clone(), 1, technical(2, 2)).expect_err("insert is rejected"),
        expected
    );

    let mut hand_edited = document.clone();
    hand_edited.tracks[0].clips[0].effects.push(technical(2, 2));
    assert_eq!(
        hand_edited.validate().expect_err("the invariant holds"),
        expected
    );
}

/// CC4 §3.2: the rule is stage rank, not kind identity — a correction blocks a
/// technical LUT just as a look does.
#[test]
fn a_technical_lut_after_a_correction_is_rejected() {
    let mut document = document_with_assets();
    add(&mut document, effect(1, "color_curves", &[])).expect("a curves node is legal");

    let expected = OpError::ColorStageOrderViolation {
        clip: ClipId(1),
        effect: EffectId(2),
        kind: "technical_lut".to_owned(),
        color_stage_rank: 0,
        previous_effect: EffectId(1),
        previous_kind: "color_curves".to_owned(),
        previous_color_stage_rank: 1,
    };
    assert_eq!(
        add(&mut document.clone(), technical(2, 1)).expect_err("append is rejected"),
        expected
    );
    assert_eq!(
        insert(&mut document.clone(), 1, technical(2, 1)).expect_err("insert is rejected"),
        expected
    );
    insert(&mut document, 0, technical(2, 1)).expect("index 0 satisfies the stage order");
    assert_eq!(effect_names(&document), ["technical_lut", "color_curves"]);
}

/// CC4 §3.2: non-colour effects are unconstrained and keep their positions.
#[test]
fn non_colour_effects_do_not_participate_in_stage_ordering() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a look is legal");
    add(&mut document, effect(2, "brightness", &[("percent", 10)]))
        .expect("a legacy control after a look is unconstrained");
    insert(&mut document, 0, effect(3, "crop", &[("left_percent", 5)]))
        .expect("a crop before a look is unconstrained");
    assert_eq!(
        effect_names(&document),
        ["crop", "creative_look", "brightness"]
    );
    document
        .validate()
        .expect("only managed nodes are stage ordered");
}

/// CC4 §3.1 and §10.3.8: the fifth LUT node on a layer is a typed error.
#[test]
fn a_fifth_lut_node_is_rejected() {
    let mut document = document_with_assets();
    for id in 1..=2 {
        add(&mut document, technical(id, 1)).expect("technical LUTs lead the stack");
    }
    for id in 3..=4 {
        add(&mut document, look(id, 2)).expect("creative looks close the stack");
    }
    assert_eq!(
        lut_node_count(&document.clip(ClipId(1)).unwrap().effects),
        4
    );

    let before = document.clone();
    assert_eq!(
        add(&mut document, look(5, 2)).expect_err("the fifth node is rejected"),
        OpError::TooManyLutNodes {
            clip: ClipId(1),
            limit: 4,
            actual: 5,
        }
    );
    assert_eq!(
        insert(&mut document, 2, technical(5, 1)).expect_err("insertion is rejected too"),
        OpError::TooManyLutNodes {
            clip: ClipId(1),
            limit: 4,
            actual: 5,
        }
    );
    assert_eq!(document, before);

    let mut hand_edited = document.clone();
    hand_edited.tracks[0].clips[0].effects.push(look(5, 2));
    assert_eq!(
        hand_edited.validate().expect_err("the invariant holds"),
        OpError::TooManyLutNodes {
            clip: ClipId(1),
            limit: 4,
            actual: 5,
        }
    );
}

// ---------------------------------------------------------------------------
// §3.3 and §6 asset reference and keyframing
// ---------------------------------------------------------------------------

/// CC4 §6: a `SetEffectParam` may never unbind or dangle a LUT node.
#[test]
fn set_effect_param_cannot_unbind_or_dangle_a_lut_node() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");

    let before = document.clone();
    assert_eq!(
        set_param(&mut document, 1, "lut_asset_id", 0).expect_err("unbinding is rejected"),
        OpError::MissingLutAsset {
            clip: ClipId(1),
            effect: EffectId(1),
            lut_asset: LutAssetId(0),
        }
    );
    assert_eq!(
        set_param(&mut document, 1, "lut_asset_id", 42).expect_err("dangling is rejected"),
        OpError::MissingLutAsset {
            clip: ClipId(1),
            effect: EffectId(1),
            lut_asset: LutAssetId(42),
        }
    );
    assert_eq!(
        document, before,
        "a rejection leaves the document untouched"
    );

    set_param(&mut document, 1, "lut_asset_id", 2).expect("retargeting at a registered asset");
    assert_eq!(
        LutNodeParams::from_effect(&document.clip(ClipId(1)).unwrap().effects[0]).lut_asset_id,
        LutAssetId(2)
    );
}

/// CC4 §5.1: `technical_lut` pins its mix through its bounds, not a special
/// case.
#[test]
fn technical_lut_mix_is_pinned_at_full_strength() {
    let mut document = document_with_assets();
    add(&mut document, technical(1, 1)).expect("a bound technical LUT is legal");
    assert_eq!(
        set_param(&mut document, 1, "mix_basis_points", 9_999).expect_err("mix is pinned"),
        OpError::EffectParamOutOfRange {
            effect: "technical_lut".to_owned(),
            name: "mix_basis_points".to_owned(),
            min: 10_000,
            max: 10_000,
            actual: 9_999,
        }
    );
    set_param(&mut document, 1, "mix_basis_points", 10_000)
        .expect("full strength is the only value");

    // A creative look is free across the whole range.
    add(&mut document, look(2, 2)).expect("a bound look is legal");
    set_param(&mut document, 2, "mix_basis_points", 0).expect("zero mix is legal on a look");
    set_param(&mut document, 2, "mix_basis_points", 3_750).expect("a partial look is legal");
    assert_eq!(
        set_param(&mut document, 2, "mix_basis_points", 10_001).expect_err("above full"),
        OpError::EffectParamOutOfRange {
            effect: "creative_look".to_owned(),
            name: "mix_basis_points".to_owned(),
            min: 0,
            max: 10_000,
            actual: 10_001,
        }
    );
}

/// CC4 §6: `lut_asset_id` and `input_encoding_token` are `Hold` only; the mix
/// is fully keyframable.
#[test]
fn the_asset_reference_and_encoding_accept_hold_keyframes_only() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");

    for name in ["lut_asset_id", "input_encoding_token"] {
        for interpolation in [
            KeyframeInterpolation::Linear,
            KeyframeInterpolation::EaseIn,
            KeyframeInterpolation::EaseOut,
        ] {
            let error = set_keyframes(
                &mut document,
                1,
                name,
                &[(0, 1, KeyframeInterpolation::Hold), (10, 1, interpolation)],
            )
            .expect_err("only hold keyframes are legal");
            assert_eq!(
                error,
                OpError::NonHoldKeyframeParameter {
                    effect: "creative_look".to_owned(),
                    name: name.to_owned(),
                }
            );
        }
    }

    set_keyframes(
        &mut document,
        1,
        "lut_asset_id",
        &[
            (0, 1, KeyframeInterpolation::Hold),
            (10, 2, KeyframeInterpolation::Hold),
        ],
    )
    .expect("hold keyframes on the asset reference are legal");
    set_keyframes(
        &mut document,
        1,
        "input_encoding_token",
        &[
            (0, 0, KeyframeInterpolation::Hold),
            (10, 2, KeyframeInterpolation::Hold),
        ],
    )
    .expect("hold keyframes on the encoding are legal");
    set_keyframes(
        &mut document,
        1,
        "mix_basis_points",
        &[
            (0, 0, KeyframeInterpolation::Linear),
            (20, 10_000, KeyframeInterpolation::EaseIn),
        ],
    )
    .expect("the mix is the audition control and takes any interpolation");
}

/// CC4 §6 and §2.7: a held asset reference must also name a registered asset.
#[test]
fn a_hold_keyframe_cannot_name_an_unregistered_asset() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");
    let before = document.clone();
    assert_eq!(
        set_keyframes(
            &mut document,
            1,
            "lut_asset_id",
            &[
                (0, 1, KeyframeInterpolation::Hold),
                (10, 77, KeyframeInterpolation::Hold)
            ],
        )
        .expect_err("a held id must exist too"),
        OpError::MissingLutAsset {
            clip: ClipId(1),
            effect: EffectId(1),
            lut_asset: LutAssetId(77),
        }
    );
    assert_eq!(document, before);
}

// ---------------------------------------------------------------------------
// §3.6 inactivity
// ---------------------------------------------------------------------------

/// CC4 §3.6: inactivity is decided on the stored integers, with three reasons.
#[test]
fn lut_node_inactivity_is_decided_on_the_stored_integers() {
    let bound = look(1, 1);
    let params = LutNodeParams::from_effect(&bound);
    assert_eq!(params.lut_asset_id, LutAssetId(1));
    assert_eq!(
        params.mix_basis_points, 10_000,
        "an omitted mix resolves to the neutral"
    );
    assert_eq!(params.input_encoding_token, 0, "display709 is the default");
    assert_eq!(params.bypass_token, 0);
    assert!(params.is_active());
    assert_eq!(params.inactive_reason(), None);
    assert!((params.mix() - 1.0).abs() < f32::EPSILON);

    let bypassed = effect(1, "creative_look", &[("lut_asset_id", 1), ("bypass", 1)]);
    assert_eq!(
        color_node_inactive_reason(&bypassed),
        Some(ColorNodeInactiveReason::Bypassed)
    );

    let zero_mix = effect(
        1,
        "creative_look",
        &[("lut_asset_id", 1), ("mix_basis_points", 0)],
    );
    assert_eq!(
        color_node_inactive_reason(&zero_mix),
        Some(ColorNodeInactiveReason::Neutral)
    );
    assert!(LutNodeParams::from_effect(&zero_mix).mix().abs() < f32::EPSILON);

    let unbound = effect(1, "creative_look", &[("mix_basis_points", 10_000)]);
    assert_eq!(
        color_node_inactive_reason(&unbound),
        Some(ColorNodeInactiveReason::Unbound)
    );
    assert!(LutNodeParams::from_effect(&unbound).is_unbound());
    assert_eq!(ColorNodeInactiveReason::Unbound.as_str(), "unbound");
    assert_eq!(ColorNodeInactiveReason::Bypassed.as_str(), "bypassed");
    assert_eq!(ColorNodeInactiveReason::Neutral.as_str(), "neutral");

    // Bypass wins over a zero mix, which wins over an unbound reference.
    let all_three = effect(
        1,
        "creative_look",
        &[("mix_basis_points", 0), ("bypass", 1)],
    );
    assert_eq!(
        color_node_inactive_reason(&all_three),
        Some(ColorNodeInactiveReason::Bypassed)
    );

    // A pinned technical LUT is never neutral by mix, only by bypass.
    let technical_bypassed = effect(
        1,
        "technical_lut",
        &[
            ("lut_asset_id", 1),
            ("mix_basis_points", 10_000),
            ("bypass", 1),
        ],
    );
    assert_eq!(
        color_node_inactive_reason(&technical_bypassed),
        Some(ColorNodeInactiveReason::Bypassed)
    );
    assert_eq!(
        classify_color_node(&technical_bypassed),
        Some(ColorNodeKind::TechnicalLut)
    );
}

/// CC4 §3.6 and §6: keyframes resolve first, so a held bypass makes the node
/// the identity for exactly the frames it covers.
#[test]
fn keyframes_resolve_before_inactivity_is_tested() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");
    set_keyframes(
        &mut document,
        1,
        "bypass",
        &[
            (0, 0, KeyframeInterpolation::Hold),
            (10, 1, KeyframeInterpolation::Hold),
        ],
    )
    .expect("bypass is keyframable");

    let node = &document.clip(ClipId(1)).unwrap().effects[0];
    assert_eq!(
        color_node_inactive_reason(&node.evaluated_at(TimeCode(0))),
        None
    );
    assert_eq!(
        color_node_inactive_reason(&node.evaluated_at(TimeCode(12))),
        Some(ColorNodeInactiveReason::Bypassed)
    );
    assert_eq!(
        active_color_nodes(&[node.evaluated_at(TimeCode(12))]),
        Vec::new()
    );
}

// ---------------------------------------------------------------------------
// §9 migration
// ---------------------------------------------------------------------------

/// CC4 §9.3: conversion replaces the legacy stage at its exact position and
/// keeps the effect id.
#[test]
fn convert_legacy_look_replaces_the_stage_in_place() {
    let mut document = document_with_assets();
    add(
        &mut document,
        effect(1, "primary_correction", &[("exposure_milli_stops", 250)]),
    )
    .expect("a primary node is legal");
    add(
        &mut document,
        effect(
            2,
            "look_lut",
            &[("preset_token", 1), ("intensity_percent", 65)],
        ),
    )
    .expect("a legacy look is still loadable");
    add(&mut document, effect(3, "brightness", &[("percent", 5)]))
        .expect("a legacy control is unconstrained");

    Operation::ConvertLegacyLook {
        clip: ClipId(1),
        effect: EffectId(2),
        lut_asset: LutAssetId(1),
        // §9.3: `intensity_percent` converts as `percent * 100`.
        mix_basis_points: 6_500,
    }
    .apply(&mut document)
    .expect("conversion is legal once the asset is registered");

    assert_eq!(
        effect_names(&document),
        ["primary_correction", "creative_look", "brightness"],
        "the managed node takes the legacy stage's exact vector position"
    );
    assert_eq!(effect_ids(&document), [1, 2, 3], "the effect id survives");
    let converted = &document.clip(ClipId(1)).unwrap().effects[1];
    let params = LutNodeParams::from_effect(converted);
    assert_eq!(params.lut_asset_id, LutAssetId(1));
    assert_eq!(params.mix_basis_points, 6_500);
    assert_eq!(params.input_encoding_token, 0);
    assert_eq!(params.bypass_token, 0);
    assert!(converted.keyframes.is_empty());
    assert_eq!(
        converted.parameters.keys().collect::<Vec<_>>(),
        vec!["lut_asset_id", "mix_basis_points"],
        "only the values the conversion determines are written"
    );
    document
        .validate()
        .expect("the converted document is valid");
}

/// CC4 §9.3: an external `cube_lut` converts the same way.
#[test]
fn convert_legacy_look_accepts_an_external_cube_lut() {
    let mut document = document_with_assets();
    let mut legacy = effect(1, "cube_lut", &[("intensity_percent", 100)]);
    legacy.parameters.insert(
        "path".to_owned(),
        ParamValue::Text("/looks/external.cube".to_owned()),
    );
    add(&mut document, legacy).expect("an external LUT is still loadable");

    Operation::ConvertLegacyLook {
        clip: ClipId(1),
        effect: EffectId(1),
        lut_asset: LutAssetId(2),
        mix_basis_points: 10_000,
    }
    .apply(&mut document)
    .expect("the caller imported the file first");
    assert_eq!(effect_names(&document), ["creative_look"]);
    assert_eq!(
        LutNodeParams::from_effect(&document.clip(ClipId(1)).unwrap().effects[0]).lut_asset_id,
        LutAssetId(2)
    );
}

/// CC4 §2.7 and §9.3: conversion refuses anything that is not a legacy stage,
/// and refuses to invent an asset.
#[test]
fn convert_legacy_look_rejects_a_non_legacy_effect_and_a_missing_asset() {
    let mut document = document_with_assets();
    add(
        &mut document,
        effect(1, "primary_correction", &[("exposure_milli_stops", 250)]),
    )
    .expect("a primary node is legal");
    add(
        &mut document,
        effect(
            2,
            "look_lut",
            &[("preset_token", 2), ("intensity_percent", 100)],
        ),
    )
    .expect("a legacy look is still loadable");
    let before = document.clone();

    assert_eq!(
        Operation::ConvertLegacyLook {
            clip: ClipId(1),
            effect: EffectId(1),
            lut_asset: LutAssetId(1),
            mix_basis_points: 10_000,
        }
        .apply(&mut document)
        .expect_err("a managed node is not a legacy look"),
        OpError::NotALegacyLook {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "primary_correction".to_owned(),
        }
    );
    assert_eq!(
        Operation::ConvertLegacyLook {
            clip: ClipId(1),
            effect: EffectId(2),
            lut_asset: LutAssetId(9),
            mix_basis_points: 10_000,
        }
        .apply(&mut document)
        .expect_err("the asset must be registered first"),
        OpError::MissingLutAsset {
            clip: ClipId(1),
            effect: EffectId(2),
            lut_asset: LutAssetId(9),
        }
    );
    assert_eq!(
        Operation::ConvertLegacyLook {
            clip: ClipId(1),
            effect: EffectId(2),
            lut_asset: LutAssetId(1),
            mix_basis_points: 10_001,
        }
        .apply(&mut document)
        .expect_err("the mix is a descriptor value"),
        OpError::EffectParamOutOfRange {
            effect: "creative_look".to_owned(),
            name: "mix_basis_points".to_owned(),
            min: 0,
            max: 10_000,
            actual: 10_001,
        }
    );
    assert_eq!(document, before, "every rejection is atomic");
}

// ---------------------------------------------------------------------------
// QA and delivery
// ---------------------------------------------------------------------------

/// CC4 §2.3 and §9.2: managed LUT nodes never report `legacy_lut_stage`, and a
/// dangling reference is a blocking QA error.
#[test]
fn qa_reports_a_dangling_reference_and_never_a_legacy_stage() {
    let mut document = document_with_assets();
    add(&mut document, technical(1, 1)).expect("technical first");
    add(&mut document, look(2, 2)).expect("look last");
    let report = qa_document(&document);
    assert!(
        report
            .issues
            .iter()
            .all(|issue| issue.code != "legacy_lut_stage"),
        "managed LUT nodes are inside the conformance claim"
    );
    assert!(
        report
            .issues
            .iter()
            .all(|issue| issue.code != "missing_lut_asset"),
        "every reference resolves"
    );

    // A hand-edited project that drops the record still loads into QA, and QA
    // names it rather than rendering a look-free frame.
    document
        .lut_assets
        .retain(|asset| asset.id != LutAssetId(2));
    let report = qa_document(&document);
    let dangling = report
        .issues
        .iter()
        .filter(|issue| issue.code == "missing_lut_asset")
        .collect::<Vec<_>>();
    assert_eq!(dangling.len(), 1);
    assert_eq!(dangling[0].severity, QaSeverity::Error);
    assert_eq!(dangling[0].clip, Some(ClipId(1)));
    assert!(dangling[0].message.contains("LUT asset 2"));
    assert!(!report.export_ready(), "an unresolvable look blocks export");
    assert_eq!(
        document
            .validate()
            .expect_err("the document invariant catches it too"),
        OpError::MissingLutAsset {
            clip: ClipId(1),
            effect: EffectId(2),
            lut_asset: LutAssetId(2),
        }
    );
}

/// CC4 §9.2: a legacy stage keeps reporting exactly as it did.
#[test]
fn a_legacy_stage_beside_a_managed_look_still_reports_once() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a managed look is legal");
    add(
        &mut document,
        effect(
            2,
            "look_lut",
            &[("preset_token", 3), ("intensity_percent", 100)],
        ),
    )
    .expect("a legacy stage coexists");
    let report = qa_document(&document);
    assert_eq!(
        report
            .issues
            .iter()
            .filter(|issue| issue.code == "legacy_lut_stage")
            .count(),
        1
    );
}

// ---------------------------------------------------------------------------
// §10.3.13 serialization and history
// ---------------------------------------------------------------------------

/// CC4 §10.3.13: every new operation saves, reopens, replays, undoes, and
/// redoes with values and vector positions preserved exactly.
#[test]
#[allow(clippy::too_many_lines)]
fn the_new_operations_survive_save_reopen_replay_and_undo() {
    let mut initial = managed_document();
    Operation::AddLutAsset {
        asset: imported_asset(1),
    }
    .apply(&mut initial)
    .expect("the first asset registers");

    let operations = vec![
        Operation::AddLutAsset {
            asset: imported_asset(2),
        },
        Operation::AddEffect {
            clip: ClipId(1),
            effect: effect(2, "primary_correction", &[("exposure_milli_stops", 250)]),
        },
        Operation::AddEffect {
            clip: ClipId(1),
            effect: effect(3, "color_wheels", &[("gain_red_thousandths", 1_100)]),
        },
        Operation::AddEffect {
            clip: ClipId(1),
            effect: effect(4, "color_curves", &[]),
        },
        Operation::InsertEffect {
            clip: ClipId(1),
            index: 0,
            effect: technical(1, 1),
        },
        Operation::InsertEffect {
            clip: ClipId(1),
            index: 4,
            effect: look(5, 2),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(5),
            name: "mix_basis_points".to_owned(),
            value: ParamValue::Integer(6_250),
        },
    ];

    let core = Core::spawn(initial.clone()).expect("core should spawn");
    let mut journaled = Vec::new();
    for operation in &operations {
        let Event::DocumentChanged {
            journal_command: Some(command),
            ..
        } = core.request(Command::Do(operation.clone())).unwrap()
        else {
            panic!("every CC4 operation must be accepted and journaled");
        };
        assert_eq!(command, JournalCommand::Do(operation.clone()));
        journaled.push(command);
    }
    let Event::DocumentChanged { doc: live, .. } = core
        .request(Command::Do(Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(5),
            name: "bypass".to_owned(),
            value: ParamValue::Integer(1),
        }))
        .unwrap()
    else {
        panic!("bypass is an ordinary parameter");
    };
    assert_eq!(
        effect_names(&live),
        [
            "technical_lut",
            "primary_correction",
            "color_wheels",
            "color_curves",
            "creative_look"
        ]
    );
    assert_eq!(live.lut_assets.len(), 2);

    // Save and reopen: the JSON is the whole state.
    let saved = serde_json::to_string(&*live).expect("document serializes");
    let reopened: Document = serde_json::from_str(&saved).expect("document reopens");
    assert_eq!(&reopened, &*live);
    assert_eq!(
        serde_json::to_string(&reopened).unwrap(),
        saved,
        "reopening and re-saving is byte-for-byte identical"
    );

    // Journal replay reproduces the same document from the same commands.
    let replay = Core::spawn(initial.clone()).expect("core should spawn");
    for command in &journaled {
        let encoded = serde_json::to_string(command).expect("journal command serializes");
        let parsed: JournalCommand = serde_json::from_str(&encoded).expect("and parses back");
        replay
            .request(parsed.into())
            .expect("a journaled CC4 operation replays");
    }
    let Event::DocumentChanged { doc: replayed, .. } = replay
        .request(Command::Do(Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(5),
            name: "bypass".to_owned(),
            value: ParamValue::Integer(1),
        }))
        .unwrap()
    else {
        panic!("replay should reach the same state");
    };
    assert_eq!(
        serde_json::to_string(&*replayed).unwrap(),
        saved,
        "replay is byte-for-byte identical"
    );

    // Undo every applied step, then redo every one of them. Eight operations
    // reached the core: the seven above plus the bypass.
    let applied = operations.len() + 1;
    let mut undone = None;
    for _ in 0..applied {
        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("every applied step must undo");
        };
        undone = Some(doc);
    }
    assert_eq!(
        &*undone.expect("undo produces a document"),
        &initial,
        "undo returns to the opening document"
    );
    let mut redone = None;
    for _ in 0..applied {
        let Event::DocumentChanged { doc, .. } = core.request(Command::Redo).unwrap() else {
            panic!("every undone step must redo");
        };
        redone = Some(doc);
    }
    assert_eq!(
        serde_json::to_string(&*redone.expect("redo produces a document")).unwrap(),
        saved,
        "undo and redo restore the stack byte-for-byte"
    );
}

/// CC4 §2.7: `RemoveLutAsset` is undoable and restores the record's position.
#[test]
fn removing_and_undoing_an_asset_restores_the_record() {
    let document = document_with_assets();
    let core = Core::spawn(document.clone()).expect("core should spawn");
    let Event::DocumentChanged { doc: removed, .. } = core
        .request(Command::Do(Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        }))
        .unwrap()
    else {
        panic!("an unreferenced asset may be removed");
    };
    assert_eq!(removed.lut_assets.len(), 1);
    let Event::DocumentChanged { doc: undone, .. } = core.request(Command::Undo).unwrap() else {
        panic!("removal undoes");
    };
    assert_eq!(&*undone, &document);
    assert_eq!(
        serde_json::to_string(&*undone).unwrap(),
        serde_json::to_string(&document).unwrap()
    );
}

// ---------------------------------------------------------------------------
// §2.3 availability preflight
// ---------------------------------------------------------------------------

fn verified() -> LutAvailabilityStatus {
    LutAvailabilityStatus {
        kind: LutAvailabilityKind::Verified,
        observed_sha256: None,
        reason: None,
        path: None,
    }
}

fn missing(asset: &LutAsset) -> LutAvailabilityStatus {
    LutAvailabilityStatus {
        kind: LutAvailabilityKind::Missing,
        observed_sha256: None,
        reason: Some("the store file is absent".to_owned()),
        path: Some(PathBuf::from(format!("/store/luts/{}.cube", asset.sha256))),
    }
}

/// CC4 §2.3: availability is runtime state Core never computes; the preflight
/// asks the caller and reports what it is told.
#[test]
fn export_lut_preflight_blocks_on_an_unverified_referenced_asset() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");

    let ready = export_lut_preflight_with(&document, &|_| verified());
    assert!(ready.export_ready());
    assert_eq!(ready.checked_lut_assets, vec![LutAssetId(1)]);
    assert!(ready.summary().contains("1 referenced LUT asset"));

    let blocked = export_lut_preflight_with(&document, &missing);
    assert!(!blocked.export_ready());
    assert_eq!(blocked.issues.len(), 1);
    let issue = &blocked.issues[0];
    assert_eq!(issue.lut_asset, LutAssetId(1));
    assert_eq!(issue.title, "Look 1");
    assert_eq!(issue.sha256, digest(1));
    assert_eq!(issue.kind, LutAvailabilityKind::Missing);
    assert_eq!(issue.reason.as_deref(), Some("the store file is absent"));
    assert_eq!(
        issue.path,
        Some(PathBuf::from(format!("/store/luts/{}.cube", digest(1))))
    );
    assert_eq!(issue.referenced_by, vec![(ClipId(1), EffectId(1))]);
    assert!(blocked.summary().contains("Export blocked"));
}

/// CC4 §2.3 and §3.6: a look that can never evaluate cannot block a delivery.
#[test]
fn export_lut_preflight_skips_assets_no_frame_can_need() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");
    set_param(&mut document, 1, "bypass", 1).expect("bypass is an ordinary parameter");
    add(&mut document, look(2, 2)).expect("a second bound look is legal");
    set_param(&mut document, 2, "mix_basis_points", 0).expect("a zero mix is legal");

    let report = export_lut_preflight_with(&document, &missing);
    assert!(report.export_ready(), "no frame needs either look");
    assert!(report.checked_lut_assets.is_empty());
    assert_eq!(report.issues, Vec::new());
    assert_eq!(
        document.lut_asset_references(LutAssetId(1)),
        vec![(ClipId(1), EffectId(1))],
        "an inactive node still blocks RemoveLutAsset"
    );
}

/// CC4 §6: a keyframed bypass or mix means the asset is still needed, because
/// the node evaluates on the frames the automation leaves it on.
#[test]
fn a_node_that_is_active_on_any_frame_still_needs_its_asset() {
    let mut document = document_with_assets();
    add(&mut document, look(1, 1)).expect("a bound look is legal");
    set_param(&mut document, 1, "bypass", 1).expect("bypass is an ordinary parameter");
    let bypassed = document.clip(ClipId(1)).unwrap().effects[0].clone();
    assert!(!lut_node_may_be_active(&bypassed));

    set_keyframes(
        &mut document,
        1,
        "bypass",
        &[
            (0, 1, KeyframeInterpolation::Hold),
            (10, 0, KeyframeInterpolation::Hold),
        ],
    )
    .expect("bypass is keyframable");
    let automated = document.clip(ClipId(1)).unwrap().effects[0].clone();
    assert!(
        lut_node_may_be_active(&automated),
        "the node is a real look for frames 10 onward"
    );
    let report = export_lut_preflight_with(&document, &missing);
    assert!(!report.export_ready());
    assert_eq!(
        report.issues[0].referenced_by,
        vec![(ClipId(1), EffectId(1))]
    );

    // The same for a mix that rises off zero.
    let mut document = document_with_assets();
    add(
        &mut document,
        effect(
            1,
            "creative_look",
            &[("lut_asset_id", 1), ("mix_basis_points", 0)],
        ),
    )
    .expect("a zero-mix look is legal");
    assert!(!lut_node_may_be_active(
        &document.clip(ClipId(1)).unwrap().effects[0]
    ));
    set_keyframes(
        &mut document,
        1,
        "mix_basis_points",
        &[
            (0, 0, KeyframeInterpolation::Linear),
            (20, 10_000, KeyframeInterpolation::Linear),
        ],
    )
    .expect("the mix is the audition control");
    assert!(lut_node_may_be_active(
        &document.clip(ClipId(1)).unwrap().effects[0]
    ));
    assert!(!export_lut_preflight_with(&document, &missing).export_ready());
}

/// CC4 §3.6: only LUT kinds answer the LUT activity question at all.
#[test]
fn only_lut_nodes_can_be_lut_active() {
    assert!(!lut_node_may_be_active(&effect(1, "color_curves", &[])));
    assert!(!lut_node_may_be_active(&effect(
        1,
        "brightness",
        &[("percent", 10)]
    )));
    assert!(lut_node_may_be_active(&technical(1, 1)));
    assert!(
        !lut_node_may_be_active(&effect(1, "creative_look", &[])),
        "an unbound node evaluates nothing"
    );
}

// ---------------------------------------------------------------------------
// §3.3 unbound rejection at the edit boundary
// ---------------------------------------------------------------------------

/// CC4 §3.3 and §6: a node with no asset reference never reaches the document,
/// through either placement operation.
#[test]
fn an_unbound_lut_node_is_rejected_by_both_placement_operations() {
    let document = document_with_assets();
    let expected = |id: u64| OpError::MissingLutAsset {
        clip: ClipId(1),
        effect: EffectId(id),
        lut_asset: LutAssetId(0),
    };
    for (index, node) in [
        // `lut_asset_id` omitted entirely, which resolves to the neutral `0`.
        effect(1, "creative_look", &[("mix_basis_points", 5_000)]),
        effect(2, "technical_lut", &[]),
        // `lut_asset_id` stored explicitly as the unbound sentinel.
        effect(3, "creative_look", &[("lut_asset_id", 0)]),
        effect(4, "technical_lut", &[("lut_asset_id", 0)]),
    ]
    .into_iter()
    .enumerate()
    {
        let id = node.id.0;
        let mut appended = document.clone();
        assert_eq!(
            add(&mut appended, node.clone()).expect_err("an unbound node is rejected"),
            expected(id),
            "case {index}"
        );
        assert_eq!(appended, document);
        let mut inserted = document.clone();
        assert_eq!(
            insert(&mut inserted, 0, node).expect_err("an unbound node is rejected"),
            expected(id),
            "case {index}"
        );
        assert_eq!(inserted, document);
    }
}

/// CC4 §9.3: the identity preset is `preset_token = 0`, the descriptor
/// default, and converts exactly like the other four.
#[test]
fn convert_legacy_look_accepts_the_default_preset_token() {
    let mut document = document_with_assets();
    add(
        &mut document,
        effect(1, "look_lut", &[("intensity_percent", 100)]),
    )
    .expect("a legacy look with the default preset is legal");
    assert_eq!(
        effect_descriptor("look_lut")
            .expect("registered")
            .parameter("preset_token")
            .expect("preset")
            .neutral,
        0,
        "the identity preset is the descriptor default"
    );

    Operation::ConvertLegacyLook {
        clip: ClipId(1),
        effect: EffectId(1),
        lut_asset: LutAssetId(1),
        mix_basis_points: 10_000,
    }
    .apply(&mut document)
    .expect("the caller binds token 0 to the built-in identity asset");
    assert_eq!(effect_names(&document), ["creative_look"]);

    // An explicit token 0 converts identically.
    let mut document = document_with_assets();
    add(
        &mut document,
        effect(
            1,
            "look_lut",
            &[("preset_token", 0), ("intensity_percent", 100)],
        ),
    )
    .expect("an explicit identity preset is legal");
    Operation::ConvertLegacyLook {
        clip: ClipId(1),
        effect: EffectId(1),
        lut_asset: LutAssetId(1),
        mix_basis_points: 10_000,
    }
    .apply(&mut document)
    .expect("token 0 is never a special case in core");
    assert_eq!(effect_names(&document), ["creative_look"]);
}
