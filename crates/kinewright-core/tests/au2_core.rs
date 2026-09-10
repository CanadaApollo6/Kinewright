//! AU2 EQ and dynamics — core contract tests (AU2 §7 items A1, A4, A7 and
//! B2 to B6).

use std::collections::BTreeMap;

use kinewright_core::{
    AUDIO_BUS_GAIN_MAX, AUDIO_BUS_GAIN_MIN, AUDIO_MASTER_GAIN_MAX, AUDIO_MASTER_GAIN_MIN, AudioBus,
    AudioBusId, AudioMaster, AudioMix, AutomationCurve, CHAIN_LOOKAHEAD_MILLISECONDS,
    ChainLookahead, ColorContext, Command, Core, Document, Effect, EffectId, EffectUniform, Event,
    Keyframe, KeyframeInterpolation, MediaAsset, MediaCatalog, MediaKind, MediaSourceFingerprint,
    OpError, Operation, ParamValue, Rational, TimeCode, Track, TrackId, TrackKind,
    chain_lookahead_milliseconds, effect_descriptor, is_audio_effect, is_static_audio_parameter,
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
        gain_tenth_db: 0,
        effects,
        ducking_sidechain_tracks: Vec::new(),
        gain_curve: None,
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

// ---------------------------------------------------------------------------
// AU2 Part B — bus and master control (§5.1 to §5.4).
// ---------------------------------------------------------------------------

/// A master chain whose effect ids and names the caller chooses.
fn master_with(gain_tenth_db: i32, effects: Vec<Effect>) -> AudioMaster {
    AudioMaster {
        gain_tenth_db,
        effects,
        gain_curve: None,
    }
}

/// The same master reached through the document invariant rather than through
/// the operation: a hand-edited project must be rejected on load too.
fn hand_edited_master(master: AudioMaster) -> Document {
    let mut doc = document_with_one_clip();
    doc.audio_mix.master = master;
    doc
}

/// `AudioMix::is_empty` and `AudioMaster::is_neutral` are only usable here if
/// they are still `const fn` (AU2 §5.4, R20).
const fn mix_is_empty(mix: &AudioMix) -> bool {
    mix.is_empty()
}

const fn master_is_neutral(master: &AudioMaster) -> bool {
    master.is_neutral()
}

/// AU2 §7 item B2.
#[test]
fn a_neutral_master_and_the_balance_law_never_reach_the_wire() {
    let untouched = document_with_one_clip();
    let untouched_bytes = serde_json::to_string(&untouched).unwrap();

    let mut only_neutral = document_with_one_clip();
    Operation::SetAudioMaster {
        master: AudioMaster::default(),
    }
    .apply(&mut only_neutral)
    .unwrap();
    Operation::SetPanLaw {
        law: kinewright_core::PanLaw::Balance,
    }
    .apply(&mut only_neutral)
    .unwrap();

    // A document that only ever set neutral is byte-identical to one that
    // never touched either field.
    assert_eq!(only_neutral, untouched);
    assert_eq!(
        serde_json::to_string(&only_neutral).unwrap(),
        untouched_bytes
    );
    assert!(!untouched_bytes.contains("master"));
    assert!(!untouched_bytes.contains("pan_law"));

    // The two predicates stay `const`, so the serde skip and `is_empty` stay
    // const too.
    assert!(master_is_neutral(&only_neutral.audio_mix.master));
    assert!(mix_is_empty(&only_neutral.audio_mix));

    // A non-neutral master or law is written, and both survive a round trip.
    let mut touched = document_with_one_clip();
    Operation::SetAudioMaster {
        master: master_with(-15, vec![audio_effect(1, "audio_gain", &[])]),
    }
    .apply(&mut touched)
    .unwrap();
    Operation::SetPanLaw {
        law: kinewright_core::PanLaw::ConstantPower,
    }
    .apply(&mut touched)
    .unwrap();
    assert!(!mix_is_empty(&touched.audio_mix));
    let bytes = serde_json::to_string(&touched).unwrap();
    assert!(bytes.contains(r#""pan_law":"constant_power""#));
    assert_eq!(
        serde_json::from_str::<Document>(&bytes).unwrap(),
        touched.clone()
    );

    // Both operations are idempotent whole sets.
    let repeated = touched.clone();
    Operation::SetAudioMaster {
        master: touched.audio_mix.master.clone(),
    }
    .apply(&mut touched)
    .unwrap();
    Operation::SetPanLaw {
        law: touched.audio_mix.pan_law,
    }
    .apply(&mut touched)
    .unwrap();
    assert_eq!(touched, repeated);
}

/// AU2 §7 item B3.
#[test]
fn bus_and_master_gain_share_the_audio_gain_domain_and_reject_outside_it() {
    // The bounds are the `audio_gain` descriptor's own, as AU1 pinned the
    // track-mix bounds.
    let descriptor = effect_descriptor("audio_gain")
        .unwrap()
        .parameter("gain_tenth_db")
        .unwrap();
    assert_eq!(i64::from(AUDIO_BUS_GAIN_MIN), descriptor.min);
    assert_eq!(i64::from(AUDIO_BUS_GAIN_MAX), descriptor.max);
    assert_eq!(AUDIO_MASTER_GAIN_MIN, AUDIO_BUS_GAIN_MIN);
    assert_eq!(AUDIO_MASTER_GAIN_MAX, AUDIO_BUS_GAIN_MAX);
    // The same domain AU1 gave the track fader, so one control reads the same
    // everywhere in the mixer.
    assert_eq!(AUDIO_BUS_GAIN_MIN, kinewright_core::TRACK_MIX_GAIN_MIN);
    assert_eq!(AUDIO_BUS_GAIN_MAX, kinewright_core::TRACK_MIX_GAIN_MAX);

    let mut doc = document_with_one_clip();
    for gain in [AUDIO_BUS_GAIN_MIN, 0, AUDIO_BUS_GAIN_MAX] {
        let mut bus = bus_with(Vec::new());
        bus.gain_tenth_db = gain;
        Operation::UpsertAudioBus { bus }.apply(&mut doc).unwrap();
        assert_eq!(doc.audio_mix.buses[0].gain_tenth_db, gain);
        doc.validate().unwrap();
    }
    for gain in [AUDIO_MASTER_GAIN_MIN, 0, AUDIO_MASTER_GAIN_MAX] {
        Operation::SetAudioMaster {
            master: master_with(gain, Vec::new()),
        }
        .apply(&mut doc)
        .unwrap();
        assert_eq!(doc.audio_mix.master.gain_tenth_db, gain);
        doc.validate().unwrap();
    }

    let before = doc.clone();
    for gain in [AUDIO_BUS_GAIN_MIN - 1, AUDIO_BUS_GAIN_MAX + 1] {
        let mut bus = bus_with(Vec::new());
        bus.gain_tenth_db = gain;
        assert_eq!(
            Operation::UpsertAudioBus { bus }.apply(&mut doc),
            Err(OpError::AudioBusGainOutOfRange {
                bus: AudioBusId(1),
                gain_tenth_db: gain,
            })
        );
        assert_eq!(doc, before);

        assert_eq!(
            Operation::SetAudioMaster {
                master: master_with(gain, Vec::new()),
            }
            .apply(&mut doc),
            Err(OpError::AudioMasterGainOutOfRange {
                gain_tenth_db: gain
            })
        );
        assert_eq!(doc, before);
    }

    // The rendered messages name the shared domain.
    assert_eq!(
        OpError::AudioBusGainOutOfRange {
            bus: AudioBusId(1),
            gain_tenth_db: 121,
        }
        .to_string(),
        "audio bus 1 gain is 121 tenth-dB, outside the inclusive range -600..=120"
    );
    assert_eq!(
        OpError::AudioMasterGainOutOfRange {
            gain_tenth_db: -601,
        }
        .to_string(),
        "audio master gain is -601 tenth-dB, outside the inclusive range -600..=120"
    );

    // A hand-edited out-of-range fader is rejected on load, not just by the
    // operation.
    let mut hand_edited = before.clone();
    hand_edited.audio_mix.buses[0].gain_tenth_db = -900;
    assert_eq!(
        hand_edited.validate(),
        Err(OpError::AudioBusGainOutOfRange {
            bus: AudioBusId(1),
            gain_tenth_db: -900,
        })
    );
    assert_eq!(
        hand_edited_master(master_with(900, Vec::new())).validate(),
        Err(OpError::AudioMasterGainOutOfRange { gain_tenth_db: 900 })
    );
}

/// AU2 §7 item B4.
#[test]
#[allow(clippy::too_many_lines)]
fn the_master_chain_carries_the_bus_rules_with_master_flavoured_errors() {
    let mut doc = document_with_one_clip();
    let rejected: Vec<(AudioMaster, OpError)> = vec![
        (
            master_with(
                0,
                vec![Effect {
                    id: EffectId(1),
                    name: "brightness".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                }],
            ),
            OpError::VisualEffectOnAudioMaster {
                effect: "brightness".to_owned(),
            },
        ),
        (
            master_with(
                0,
                vec![
                    audio_effect(4, "audio_gain", &[]),
                    audio_effect(4, "audio_limiter", &[]),
                ],
            ),
            OpError::DuplicateAudioMasterEffect {
                effect: EffectId(4),
            },
        ),
        (
            master_with(0, vec![audio_effect(1, "audio_ducking", &[])]),
            OpError::AudioMasterDuckingUnsupported,
        ),
        (
            master_with(
                0,
                vec![Effect {
                    id: EffectId(1),
                    name: "audio_gain".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::from([(
                        "gain_tenth_db".to_owned(),
                        AutomationCurve {
                            keyframes: vec![Keyframe {
                                at: TimeCode(60),
                                value: -60,
                                interpolation: KeyframeInterpolation::Linear,
                            }],
                        },
                    )]),
                }],
            ),
            OpError::AudioMasterKeyframeOutsideProject {
                effect: EffectId(1),
                name: "gain_tenth_db".to_owned(),
                at: TimeCode(60),
                duration: TimeCode(60),
            },
        ),
        (
            master_with(
                0,
                vec![audio_effect(1, "audio_gain", &[("gain_tenth_db", 121)])],
            ),
            OpError::EffectParamOutOfRange {
                effect: "audio_gain".to_owned(),
                name: "gain_tenth_db".to_owned(),
                min: -600,
                max: 120,
                actual: 121,
            },
        ),
        (
            master_with(
                0,
                vec![
                    audio_effect(1, "audio_compressor", &[("lookahead_milliseconds", 10)]),
                    audio_effect(2, "audio_compressor", &[("lookahead_milliseconds", 10)]),
                    audio_effect(
                        3,
                        "audio_true_peak_limiter",
                        &[("lookahead_milliseconds", 1)],
                    ),
                ],
            ),
            OpError::AudioMasterLookaheadExceeded { milliseconds: 21 },
        ),
        // AU2 §2.2 rule 1, shared with the bus path.
        (
            master_with(
                0,
                vec![keyed_effect(
                    1,
                    "audio_gain",
                    "bypass",
                    1,
                    KeyframeInterpolation::Linear,
                )],
            ),
            OpError::NonHoldKeyframeParameter {
                effect: "audio_gain".to_owned(),
                name: "bypass".to_owned(),
            },
        ),
        // AU2 §2.2 rule 2, shared with the bus path.
        (
            master_with(
                0,
                vec![keyed_effect(
                    1,
                    "audio_true_peak_limiter",
                    "lookahead_milliseconds",
                    3,
                    KeyframeInterpolation::Hold,
                )],
            ),
            OpError::InvalidEffectAutomation {
                effect: "audio_true_peak_limiter".to_owned(),
                name: "lookahead_milliseconds".to_owned(),
                reason: "sets processing latency and cannot be keyframed".to_owned(),
            },
        ),
    ];
    let before = doc.clone();
    for (master, expected) in rejected {
        // From the operation ...
        assert_eq!(
            Operation::SetAudioMaster {
                master: master.clone(),
            }
            .apply(&mut doc),
            Err(expected.clone())
        );
        assert_eq!(doc, before);
        // ... and again from the document invariant, so a hand-edited project
        // is refused on load.
        assert_eq!(hand_edited_master(master).validate(), Err(expected));
    }

    // Exactly the budget is legal, and the effect-id scope is the chain: a bus
    // may reuse the master's ids.
    Operation::SetAudioMaster {
        master: master_with(
            -30,
            vec![
                audio_effect(1, "audio_compressor", &[("lookahead_milliseconds", 10)]),
                audio_effect(
                    2,
                    "audio_true_peak_limiter",
                    &[("lookahead_milliseconds", 10)],
                ),
            ],
        ),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::UpsertAudioBus {
        bus: bus_with(vec![audio_effect(1, "audio_gain", &[])]),
    }
    .apply(&mut doc)
    .unwrap();
    doc.validate().unwrap();
    assert_eq!(
        doc.audio_mix.lookahead_milliseconds(),
        ChainLookahead {
            bus_stage: 0,
            master_stage: CHAIN_LOOKAHEAD_MILLISECONDS,
        }
    );

    // The two master messages read exactly as the contract writes them.
    assert_eq!(
        OpError::AudioMasterLookaheadExceeded { milliseconds: 21 }.to_string(),
        "audio master chain declares 21 ms of lookahead, beyond the 20 ms chain budget"
    );
    assert_eq!(
        OpError::AudioMasterDuckingUnsupported.to_string(),
        "audio_ducking has no sidechain on the master chain and cannot be used there"
    );
    assert_eq!(
        OpError::DuplicateAudioMasterEffect {
            effect: EffectId(4),
        }
        .to_string(),
        "audio master chain has more than one effect with id 4"
    );
    assert_eq!(
        OpError::VisualEffectOnAudioMaster {
            effect: "brightness".to_owned(),
        }
        .to_string(),
        "effect \"brightness\" is not an audio effect and cannot sit on the audio master chain"
    );
    assert_eq!(
        OpError::AudioMasterKeyframeOutsideProject {
            effect: EffectId(1),
            name: "gain_tenth_db".to_owned(),
            at: TimeCode(60),
            duration: TimeCode(60),
        }
        .to_string(),
        "audio master effect 1 keyframes gain_tenth_db at frame 60, outside the project duration 60"
    );
}

/// AU2 §7 item B5.
#[test]
fn next_bus_id_allocates_max_plus_one_and_agrees_with_the_former_inline_scan() {
    // The allocator `normalization_context` spelled inline before AU2 §5.4.
    fn former_inline_allocator(mix: &AudioMix) -> AudioBusId {
        AudioBusId(
            mix.buses
                .iter()
                .map(|bus| bus.id.0)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        )
    }

    let mut doc = document_with_one_clip();
    assert_eq!(doc.audio_mix.next_bus_id(), AudioBusId(1));
    assert_eq!(
        doc.audio_mix.next_bus_id(),
        former_inline_allocator(&doc.audio_mix)
    );

    for id in [2, 3] {
        Operation::AddTrack {
            track: Track {
                id: TrackId(id),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            },
        }
        .apply(&mut doc)
        .unwrap();
    }
    Operation::UpsertAudioBus {
        bus: bus_with(Vec::new()),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.audio_mix.next_bus_id(), AudioBusId(2));

    // A gap in the ids does not make the allocator reuse one.
    let mut sparse = AudioBus {
        id: AudioBusId(9),
        name: "Music".to_owned(),
        tracks: vec![TrackId(2)],
        gain_tenth_db: 0,
        effects: Vec::new(),
        ducking_sidechain_tracks: Vec::new(),
        gain_curve: None,
    };
    Operation::UpsertAudioBus {
        bus: sparse.clone(),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.audio_mix.next_bus_id(), AudioBusId(10));
    assert_eq!(
        doc.audio_mix.next_bus_id(),
        former_inline_allocator(&doc.audio_mix)
    );

    // The two AU2 §5.4 accessors read the same routing the validator enforces.
    assert_eq!(
        doc.audio_mix.bus(AudioBusId(9)).map(|bus| bus.id),
        Some(AudioBusId(9))
    );
    assert_eq!(doc.audio_mix.bus(AudioBusId(3)), None);
    assert_eq!(doc.audio_mix.bus_for_track(TrackId(2)), Some(AudioBusId(9)));
    assert_eq!(doc.audio_mix.bus_for_track(TrackId(1)), Some(AudioBusId(1)));
    // Track 3 exists but routes to no bus, so it reaches the master directly
    // (AU2 §5.6); an unknown track answers the same way.
    assert!(doc.tracks.iter().any(|track| track.id == TrackId(3)));
    assert_eq!(doc.audio_mix.bus_for_track(TrackId(3)), None);
    assert_eq!(doc.audio_mix.bus_for_track(TrackId(404)), None);

    sparse.tracks = Vec::new();
    assert!(matches!(
        Operation::UpsertAudioBus { bus: sparse }.apply(&mut doc),
        Err(OpError::InvalidAudioBus(AudioBusId(9)))
    ));
}

/// AU2 §7 item B6.
#[test]
fn a_coalesced_bus_fader_drag_is_one_undo_entry() {
    let mut initial = document_with_one_clip();
    Operation::UpsertAudioBus {
        bus: bus_with(vec![audio_effect(
            1,
            "audio_compressor",
            &[("threshold_tenth_db", -100)],
        )]),
    }
    .apply(&mut initial)
    .unwrap();
    let core = Core::spawn(initial.clone()).unwrap();

    let mut tenth = None;
    for step in 1..=10 {
        let mut bus = bus_with(vec![audio_effect(
            1,
            "audio_compressor",
            &[("threshold_tenth_db", -100 - step * 10)],
        )]);
        bus.gain_tenth_db = i32::try_from(-step).unwrap();
        let Event::DocumentChanged { doc, .. } = core
            .request(Command::DoBatchCoalesced {
                operations: vec![Operation::UpsertAudioBus { bus }],
                coalesce_key: "audio_bus:1#1".to_owned(),
            })
            .unwrap()
        else {
            panic!("a coalesced bus batch should be accepted");
        };
        assert_eq!(
            doc.audio_mix.buses[0].gain_tenth_db,
            i32::try_from(-step).unwrap()
        );
        tenth = Some(doc);
    }
    let tenth = tenth.unwrap();
    assert_eq!(tenth.audio_mix.buses[0].gain_tenth_db, -10);

    let Event::DocumentChanged {
        doc,
        revision: first,
        ..
    } = core.request(Command::Undo).unwrap()
    else {
        panic!("the gesture should be undoable");
    };
    assert_eq!(&*doc, &initial);

    let Event::DocumentChanged {
        doc,
        revision: second,
        ..
    } = core.request(Command::Undo).unwrap()
    else {
        panic!("a second undo should still report the document");
    };
    assert_eq!(&*doc, &initial);
    assert_eq!(first, second);

    let Event::DocumentChanged { doc, .. } = core.request(Command::Redo).unwrap() else {
        panic!("the gesture should be redoable");
    };
    assert_eq!(&*doc, &*tenth);
}
