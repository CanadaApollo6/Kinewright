//! AU2 EQ and dynamics, Part A — core contract tests (AU2 §7 items A1, A4, A7).

use std::collections::BTreeMap;

use kinewright_core::{
    AudioBus, AudioBusId, AudioMix, AutomationCurve, CHAIN_LOOKAHEAD_MILLISECONDS, ChainLookahead,
    ColorContext, Document, Effect, EffectId, EffectUniform, Keyframe, KeyframeInterpolation,
    MediaAsset, MediaCatalog, MediaKind, MediaSourceFingerprint, OpError, Operation, ParamValue,
    Rational, TimeCode, Track, TrackId, TrackKind, chain_lookahead_milliseconds, effect_descriptor,
    is_audio_effect, is_static_audio_parameter,
};

/// The eight bus-only audio node names, in `EFFECT_DESCRIPTORS` order (AU2 §2.1).
const AUDIO_EFFECT_NAMES: [&str; 8] = [
    "audio_gain",
    "audio_eq",
    "audio_compressor",
    "audio_ducking",
    "audio_limiter",
    "audio_parametric_eq",
    "audio_gate",
    "audio_true_peak_limiter",
];

/// Every parameter row AU2 §2.1 adds: effect, parameter, min, max, neutral,
/// uniform. Forty rows, transcribed from the contract tables.
const NEW_PARAMETER_ROWS: [(&str, &str, i64, i64, i64, EffectUniform); 40] = [
    // The one shared bypass row, on all eight descriptors.
    ("audio_gain", "bypass", 0, 1, 0, EffectUniform::AudioBypass),
    ("audio_eq", "bypass", 0, 1, 0, EffectUniform::AudioBypass),
    (
        "audio_compressor",
        "bypass",
        0,
        1,
        0,
        EffectUniform::AudioBypass,
    ),
    (
        "audio_ducking",
        "bypass",
        0,
        1,
        0,
        EffectUniform::AudioBypass,
    ),
    (
        "audio_limiter",
        "bypass",
        0,
        1,
        0,
        EffectUniform::AudioBypass,
    ),
    (
        "audio_parametric_eq",
        "bypass",
        0,
        1,
        0,
        EffectUniform::AudioBypass,
    ),
    ("audio_gate", "bypass", 0, 1, 0, EffectUniform::AudioBypass),
    (
        "audio_true_peak_limiter",
        "bypass",
        0,
        1,
        0,
        EffectUniform::AudioBypass,
    ),
    // `audio_compressor`, four additions.
    (
        "audio_compressor",
        "knee_tenth_db",
        0,
        240,
        0,
        EffectUniform::CompressorKnee,
    ),
    (
        "audio_compressor",
        "detector",
        0,
        1,
        0,
        EffectUniform::CompressorDetector,
    ),
    (
        "audio_compressor",
        "rms_window_milliseconds",
        1,
        100,
        10,
        EffectUniform::CompressorRmsWindow,
    ),
    (
        "audio_compressor",
        "lookahead_milliseconds",
        0,
        10,
        0,
        EffectUniform::CompressorLookahead,
    ),
    // `audio_parametric_eq`, eighteen rows beside the shared bypass.
    (
        "audio_parametric_eq",
        "high_pass_hertz",
        0,
        1_000,
        0,
        EffectUniform::ParametricEqHighPassHertz,
    ),
    (
        "audio_parametric_eq",
        "low_shelf_hertz",
        20,
        1_000,
        100,
        EffectUniform::ParametricEqLowShelfHertz,
    ),
    (
        "audio_parametric_eq",
        "low_shelf_gain_tenth_db",
        -240,
        240,
        0,
        EffectUniform::ParametricEqLowShelfGain,
    ),
    (
        "audio_parametric_eq",
        "band1_hertz",
        20,
        20_000,
        120,
        EffectUniform::ParametricEqBand1Hertz,
    ),
    (
        "audio_parametric_eq",
        "band1_gain_tenth_db",
        -240,
        240,
        0,
        EffectUniform::ParametricEqBand1Gain,
    ),
    (
        "audio_parametric_eq",
        "band1_q_hundredths",
        10,
        1_800,
        71,
        EffectUniform::ParametricEqBand1Q,
    ),
    (
        "audio_parametric_eq",
        "band2_hertz",
        20,
        20_000,
        500,
        EffectUniform::ParametricEqBand2Hertz,
    ),
    (
        "audio_parametric_eq",
        "band2_gain_tenth_db",
        -240,
        240,
        0,
        EffectUniform::ParametricEqBand2Gain,
    ),
    (
        "audio_parametric_eq",
        "band2_q_hundredths",
        10,
        1_800,
        71,
        EffectUniform::ParametricEqBand2Q,
    ),
    (
        "audio_parametric_eq",
        "band3_hertz",
        20,
        20_000,
        2_000,
        EffectUniform::ParametricEqBand3Hertz,
    ),
    (
        "audio_parametric_eq",
        "band3_gain_tenth_db",
        -240,
        240,
        0,
        EffectUniform::ParametricEqBand3Gain,
    ),
    (
        "audio_parametric_eq",
        "band3_q_hundredths",
        10,
        1_800,
        71,
        EffectUniform::ParametricEqBand3Q,
    ),
    (
        "audio_parametric_eq",
        "band4_hertz",
        20,
        20_000,
        8_000,
        EffectUniform::ParametricEqBand4Hertz,
    ),
    (
        "audio_parametric_eq",
        "band4_gain_tenth_db",
        -240,
        240,
        0,
        EffectUniform::ParametricEqBand4Gain,
    ),
    (
        "audio_parametric_eq",
        "band4_q_hundredths",
        10,
        1_800,
        71,
        EffectUniform::ParametricEqBand4Q,
    ),
    (
        "audio_parametric_eq",
        "high_shelf_hertz",
        1_000,
        20_000,
        8_000,
        EffectUniform::ParametricEqHighShelfHertz,
    ),
    (
        "audio_parametric_eq",
        "high_shelf_gain_tenth_db",
        -240,
        240,
        0,
        EffectUniform::ParametricEqHighShelfGain,
    ),
    (
        "audio_parametric_eq",
        "output_gain_tenth_db",
        -240,
        240,
        0,
        EffectUniform::ParametricEqOutputGain,
    ),
    // `audio_gate`, six rows beside the shared bypass.
    (
        "audio_gate",
        "threshold_tenth_db",
        -600,
        0,
        -600,
        EffectUniform::GateThreshold,
    ),
    (
        "audio_gate",
        "ratio_hundredths",
        100,
        2_000,
        100,
        EffectUniform::GateRatio,
    ),
    (
        "audio_gate",
        "range_tenth_db",
        0,
        800,
        0,
        EffectUniform::GateRange,
    ),
    (
        "audio_gate",
        "attack_milliseconds",
        1,
        1_000,
        1,
        EffectUniform::GateAttack,
    ),
    (
        "audio_gate",
        "hold_milliseconds",
        0,
        1_000,
        10,
        EffectUniform::GateHold,
    ),
    (
        "audio_gate",
        "release_milliseconds",
        10,
        5_000,
        100,
        EffectUniform::GateRelease,
    ),
    // `audio_true_peak_limiter`, four rows beside the shared bypass.
    (
        "audio_true_peak_limiter",
        "ceiling_tenth_db",
        -120,
        0,
        0,
        EffectUniform::TruePeakCeiling,
    ),
    (
        "audio_true_peak_limiter",
        "lookahead_milliseconds",
        1,
        10,
        5,
        EffectUniform::TruePeakLookahead,
    ),
    (
        "audio_true_peak_limiter",
        "release_milliseconds",
        1,
        1_000,
        50,
        EffectUniform::TruePeakRelease,
    ),
    (
        "audio_true_peak_limiter",
        "true_peak",
        0,
        1,
        1,
        EffectUniform::TruePeakDetector,
    ),
];

fn document_with_one_clip() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = Document {
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    };
    Operation::AddAsset {
        asset: MediaAsset {
            id: kinewright_core::AssetId(1),
            path: std::path::PathBuf::from("asset-1.mp4"),
            name: "asset-1".to_owned(),
            duration: TimeCode(300),
            fps,
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: kinewright_core::AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

/// One audio node carrying static integer parameters.
fn audio_effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(key, value)| ((*key).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

/// One audio node carrying a single-keyframe curve on `parameter`.
fn keyed_effect(
    id: u64,
    name: &str,
    parameter: &str,
    value: i64,
    interpolation: KeyframeInterpolation,
) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::from([(
            parameter.to_owned(),
            AutomationCurve {
                keyframes: vec![Keyframe {
                    at: TimeCode(10),
                    value,
                    interpolation,
                }],
            },
        )]),
    }
}

fn bus_with(effects: Vec<Effect>) -> AudioBus {
    AudioBus {
        id: AudioBusId(1),
        name: "Dialogue".to_owned(),
        tracks: vec![TrackId(1)],
        effects,
        ducking_sidechain_tracks: Vec::new(),
    }
}

/// The same chain reached through the document invariant rather than through
/// the operation: a hand-edited project must be rejected on load too.
fn hand_edited(effects: Vec<Effect>) -> Document {
    let mut doc = document_with_one_clip();
    doc.audio_mix.buses.push(bus_with(effects));
    doc
}

/// AU2 §7 item A1.
#[test]
fn audio_descriptor_tables_match_the_contract_exactly() {
    // `is_audio_effect` accepts exactly eight names.
    for name in AUDIO_EFFECT_NAMES {
        assert!(is_audio_effect(name), "{name} must be an audio effect");
        assert!(
            effect_descriptor(name).is_some(),
            "{name} must be registered"
        );
    }
    let registered_audio: Vec<&str> = kinewright_core::EFFECT_DESCRIPTORS
        .iter()
        .map(|descriptor| descriptor.name)
        .filter(|name| is_audio_effect(name))
        .collect();
    assert_eq!(registered_audio, AUDIO_EFFECT_NAMES);
    assert!(!is_audio_effect("audio_parametric"));
    assert!(!is_audio_effect("brightness"));

    // The three new descriptors are appended after `audio_limiter`, and the
    // registry grew from 22 entries to 25.
    assert_eq!(kinewright_core::EFFECT_DESCRIPTORS.len(), 25);
    let tail: Vec<&str> = kinewright_core::EFFECT_DESCRIPTORS
        .iter()
        .rev()
        .take(3)
        .map(|descriptor| descriptor.name)
        .collect();
    assert_eq!(
        tail,
        [
            "audio_true_peak_limiter",
            "audio_gate",
            "audio_parametric_eq"
        ]
    );

    // Every new row's min, max, neutral, and uniform are the §2.1 tables.
    for (effect, parameter, min, max, neutral, uniform) in NEW_PARAMETER_ROWS {
        let descriptor = effect_descriptor(effect)
            .and_then(|descriptor| descriptor.parameter(parameter))
            .unwrap_or_else(|| panic!("{effect} must expose {parameter}"));
        assert_eq!(descriptor.min, min, "{effect}.{parameter} min");
        assert_eq!(descriptor.max, max, "{effect}.{parameter} max");
        assert_eq!(descriptor.neutral, neutral, "{effect}.{parameter} neutral");
        assert_eq!(descriptor.uniform, uniform, "{effect}.{parameter} uniform");
    }

    // Forty new rows and no more: the eight audio descriptors carry the
    // fourteen pre-AU2 rows plus these.
    let audio_rows: usize = AUDIO_EFFECT_NAMES
        .into_iter()
        .map(|name| {
            effect_descriptor(name)
                .expect("registered")
                .parameters
                .len()
        })
        .sum();
    assert_eq!(audio_rows, 14 + NEW_PARAMETER_ROWS.len());

    // Thirty-three new uniforms, because the eight `bypass` rows share one.
    let mut uniforms: Vec<String> = NEW_PARAMETER_ROWS
        .iter()
        .map(|row| format!("{:?}", row.5))
        .collect();
    assert_eq!(uniforms.len(), 40);
    uniforms.sort();
    uniforms.dedup();
    assert_eq!(uniforms.len(), 33);

    // Read from the descriptors themselves, so reusing a pre-AU2 audio uniform
    // on a new row would be caught: 54 audio rows carry 47 distinct uniforms —
    // the 14 pre-AU2 ones plus these 33, the eight `bypass` rows collapsing to
    // one.
    let mut live_uniforms: Vec<String> = AUDIO_EFFECT_NAMES
        .into_iter()
        .flat_map(|name| effect_descriptor(name).expect("registered").parameters)
        .map(|parameter| format!("{:?}", parameter.uniform))
        .collect();
    live_uniforms.sort();
    live_uniforms.dedup();
    assert_eq!(live_uniforms.len(), 47);

    // One shared descriptor, not eight look-alikes: every `bypass` row is the
    // same record.
    let bypass = effect_descriptor("audio_gain")
        .and_then(|descriptor| descriptor.parameter("bypass"))
        .expect("audio_gain exposes bypass");
    for name in AUDIO_EFFECT_NAMES {
        let row = effect_descriptor(name)
            .and_then(|descriptor| descriptor.parameter("bypass"))
            .unwrap_or_else(|| panic!("{name} must expose bypass"));
        assert_eq!(
            row, bypass,
            "{name} must reuse the shared bypass descriptor"
        );
        assert_eq!(row.uniform, EffectUniform::AudioBypass);
    }
}

/// AU2 §2.1: a descriptor whose neutral sits outside its own domain would make
/// the runtime's fallback unrepresentable in the document.
#[test]
fn every_audio_descriptor_neutral_lies_inside_its_own_range() {
    for name in AUDIO_EFFECT_NAMES {
        let descriptor = effect_descriptor(name).expect("registered effect");
        for parameter in descriptor.parameters {
            assert!(
                parameter.min <= parameter.neutral && parameter.neutral <= parameter.max,
                "{name}.{} neutral {} is outside {}..={}",
                parameter.name,
                parameter.neutral,
                parameter.min,
                parameter.max
            );
        }
    }
}

/// AU2 §7 item A4.
#[test]
#[allow(clippy::too_many_lines)]
fn audio_switches_take_hold_keyframes_only_and_latency_takes_none() {
    let base = document_with_one_clip();

    // Rule 1: `bypass`, `detector`, and `true_peak` accept `Hold` only.
    for (effect, parameter, value) in [
        ("audio_gain", "bypass", 1),
        ("audio_compressor", "detector", 1),
        ("audio_true_peak_limiter", "true_peak", 0),
    ] {
        for interpolation in [
            KeyframeInterpolation::Linear,
            KeyframeInterpolation::EaseIn,
            KeyframeInterpolation::EaseOut,
            KeyframeInterpolation::EaseInOut,
        ] {
            let effects = vec![keyed_effect(1, effect, parameter, value, interpolation)];
            let mut doc = base.clone();
            let error = Operation::UpsertAudioBus {
                bus: bus_with(effects.clone()),
            }
            .apply(&mut doc)
            .unwrap_err();
            assert_eq!(
                error,
                OpError::NonHoldKeyframeParameter {
                    effect: effect.to_owned(),
                    name: parameter.to_owned(),
                }
            );
            assert_eq!(doc, base);
            // And again from the document invariant, on a hand-edited project.
            assert_eq!(hand_edited(effects).validate().unwrap_err(), error);
        }

        // A `Hold` curve on the same switch stays legal.
        let mut doc = base.clone();
        Operation::UpsertAudioBus {
            bus: bus_with(vec![keyed_effect(
                1,
                effect,
                parameter,
                value,
                KeyframeInterpolation::Hold,
            )]),
        }
        .apply(&mut doc)
        .unwrap();
        assert_eq!(doc.audio_mix.buses[0].effects[0].name, effect);
    }

    // Rule 2: a parameter read once at construction takes no curve at all, not
    // even `Hold`. The two reasons stay distinct — only the lookahead
    // parameters are latency; the RMS window merely sizes a buffer (AU2 §0
    // E16).
    for (effect, parameter, value, reason) in [
        (
            "audio_compressor",
            "lookahead_milliseconds",
            5,
            "sets processing latency and cannot be keyframed",
        ),
        (
            "audio_true_peak_limiter",
            "lookahead_milliseconds",
            5,
            "sets processing latency and cannot be keyframed",
        ),
        (
            "audio_compressor",
            "rms_window_milliseconds",
            20,
            "is read once when the chain is built and cannot be keyframed",
        ),
    ] {
        assert!(is_static_audio_parameter(effect, parameter));
        for interpolation in [KeyframeInterpolation::Hold, KeyframeInterpolation::Linear] {
            let effects = vec![keyed_effect(1, effect, parameter, value, interpolation)];
            let mut doc = base.clone();
            let error = Operation::UpsertAudioBus {
                bus: bus_with(effects.clone()),
            }
            .apply(&mut doc)
            .unwrap_err();
            assert_eq!(
                error,
                OpError::InvalidEffectAutomation {
                    effect: effect.to_owned(),
                    name: parameter.to_owned(),
                    reason: reason.to_owned(),
                }
            );
            assert_eq!(doc, base);
            assert_eq!(hand_edited(effects).validate().unwrap_err(), error);
        }

        // The same parameter is legal as a stored static value.
        let mut doc = base.clone();
        Operation::UpsertAudioBus {
            bus: bus_with(vec![audio_effect(1, effect, &[(parameter, value)])]),
        }
        .apply(&mut doc)
        .unwrap();
        doc.validate().unwrap();
    }

    // Nothing else on an audio node is hold-only: a frequency, a gain, and a
    // time constant all keep every interpolation.
    let mut doc = base.clone();
    Operation::UpsertAudioBus {
        bus: bus_with(vec![
            keyed_effect(
                1,
                "audio_parametric_eq",
                "band1_hertz",
                4_000,
                KeyframeInterpolation::Linear,
            ),
            keyed_effect(
                2,
                "audio_gate",
                "release_milliseconds",
                400,
                KeyframeInterpolation::EaseOut,
            ),
        ]),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.audio_mix.buses[0].effects.len(), 2);
    assert!(!is_static_audio_parameter(
        "audio_gate",
        "hold_milliseconds"
    ));
    assert!(!is_static_audio_parameter("audio_gain", "gain_tenth_db"));
    assert!(!is_static_audio_parameter(
        "audio_compressor",
        "attack_milliseconds"
    ));
    // The effect-name half is load-bearing: only two of the eight nodes carry
    // a `lookahead_milliseconds` row at all, so the predicate must test the
    // name as well as the parameter.
    assert!(!is_static_audio_parameter(
        "audio_gate",
        "lookahead_milliseconds"
    ));
    assert!(!is_static_audio_parameter(
        "audio_parametric_eq",
        "lookahead_milliseconds"
    ));
    // Only the compressor's RMS window is static; no other node has one, and
    // the compressor's other time constants stay freely automatable.
    assert!(!is_static_audio_parameter(
        "audio_true_peak_limiter",
        "rms_window_milliseconds"
    ));
    assert!(!is_static_audio_parameter(
        "audio_compressor",
        "release_milliseconds"
    ));

    // The positive direction of §2.1's shared row: a static `bypass = 1` is a
    // legal stored value on every one of the eight nodes, through the
    // operation and through the document invariant.
    for name in AUDIO_EFFECT_NAMES {
        let mut doc = base.clone();
        let mut bus = bus_with(vec![audio_effect(1, name, &[("bypass", 1)])]);
        // `audio_ducking` is the one node that needs a sidechain to be legal.
        bus.ducking_sidechain_tracks = vec![TrackId(1)];
        Operation::UpsertAudioBus { bus }.apply(&mut doc).unwrap();
        assert_eq!(
            doc.audio_mix.buses[0].effects[0].static_integer_parameter("bypass"),
            Some(1),
            "{name} must accept a static bypass"
        );
        doc.validate().unwrap();
    }
}

/// AU2 §7 item A7.
#[test]
fn a_chain_may_declare_twenty_milliseconds_of_lookahead_and_no_more() {
    assert_eq!(CHAIN_LOOKAHEAD_MILLISECONDS, 20);

    // The declared sum reads static values, falls back to the descriptor
    // neutral when the parameter is absent, and ignores bypass.
    assert_eq!(chain_lookahead_milliseconds(&[]), 0);
    assert_eq!(
        chain_lookahead_milliseconds(&[
            audio_effect(1, "audio_gain", &[("gain_tenth_db", -30)]),
            audio_effect(2, "audio_parametric_eq", &[("band1_gain_tenth_db", -40)]),
            audio_effect(3, "audio_gate", &[]),
        ]),
        0,
        "a node with no lookahead parameter declares none"
    );
    assert_eq!(
        chain_lookahead_milliseconds(&[
            audio_effect(1, "audio_compressor", &[]),
            audio_effect(2, "audio_true_peak_limiter", &[]),
        ]),
        5,
        "absent parameters fall back to the descriptor neutrals, 0 and 5"
    );
    assert_eq!(
        chain_lookahead_milliseconds(&[audio_effect(
            1,
            "audio_true_peak_limiter",
            &[("lookahead_milliseconds", 7), ("bypass", 1)],
        )]),
        7,
        "a bypassed node keeps its delay line, so it still declares its lookahead"
    );
    assert_eq!(
        chain_lookahead_milliseconds(&[audio_effect(
            1,
            "audio_compressor",
            &[("detector", 1), ("rms_window_milliseconds", 100)],
        )]),
        0,
        "the RMS window is static but is not latency (AU2 §0 E16)"
    );

    // Exactly the budget is accepted: a 10 ms compressor plus a 10 ms limiter.
    let base = document_with_one_clip();
    let mut doc = base.clone();
    Operation::UpsertAudioBus {
        bus: bus_with(vec![
            audio_effect(1, "audio_compressor", &[("lookahead_milliseconds", 10)]),
            audio_effect(
                2,
                "audio_true_peak_limiter",
                &[("lookahead_milliseconds", 10)],
            ),
        ]),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        doc.audio_mix.lookahead_milliseconds(),
        ChainLookahead {
            bus_stage: 20,
            master_stage: 0,
        }
    );
    doc.validate().unwrap();

    // Pre-AU2 documents declare nothing.
    assert_eq!(
        base.audio_mix.lookahead_milliseconds(),
        ChainLookahead::default()
    );

    // One millisecond more is rejected, atomically, and again at load.
    let over_budget = vec![
        audio_effect(1, "audio_compressor", &[("lookahead_milliseconds", 10)]),
        audio_effect(
            2,
            "audio_true_peak_limiter",
            &[("lookahead_milliseconds", 10)],
        ),
        audio_effect(3, "audio_compressor", &[("lookahead_milliseconds", 1)]),
    ];
    let mut doc = base.clone();
    let error = Operation::UpsertAudioBus {
        bus: bus_with(over_budget.clone()),
    }
    .apply(&mut doc)
    .unwrap_err();
    assert_eq!(
        error,
        OpError::AudioBusLookaheadExceeded {
            bus: AudioBusId(1),
            milliseconds: 21,
        }
    );
    assert_eq!(doc, base);
    assert_eq!(hand_edited(over_budget).validate().unwrap_err(), error);
    assert_eq!(
        error.to_string(),
        "audio bus 1 declares 21 ms of lookahead, beyond the 20 ms chain budget"
    );
}

/// AU2 §2.1: the three new nodes are ordinary `Effect` records, so the one bus
/// operation carries them over the wire with no new document field.
#[test]
fn upsert_audio_bus_round_trips_one_of_each_new_node() {
    let operation = Operation::UpsertAudioBus {
        bus: bus_with(vec![
            audio_effect(
                1,
                "audio_parametric_eq",
                &[
                    ("high_pass_hertz", 80),
                    ("band1_hertz", 4_000),
                    ("band1_gain_tenth_db", -45),
                    ("band1_q_hundredths", 300),
                ],
            ),
            audio_effect(
                2,
                "audio_gate",
                &[("threshold_tenth_db", -380), ("range_tenth_db", 180)],
            ),
            audio_effect(
                3,
                "audio_true_peak_limiter",
                &[
                    ("ceiling_tenth_db", -10),
                    ("lookahead_milliseconds", 5),
                    ("true_peak", 1),
                ],
            ),
            audio_effect(
                4,
                "audio_compressor",
                &[
                    ("knee_tenth_db", 120),
                    ("detector", 1),
                    ("rms_window_milliseconds", 20),
                    ("lookahead_milliseconds", 5),
                ],
            ),
        ]),
    };
    let encoded = serde_json::to_string(&operation).unwrap();
    let decoded: Operation = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, operation);

    // And the same chain applies and survives the document invariant.
    let mut doc = document_with_one_clip();
    operation.apply(&mut doc).unwrap();
    doc.validate().unwrap();
    assert_eq!(doc.audio_mix.buses[0].effects.len(), 4);
    assert_eq!(doc.audio_mix.lookahead_milliseconds().bus_stage, 10);
}
