//! AU5 repair and room tone — core contract tests (AU5 §7 items A1, A3, A13
//! and the core halves of A12, A14 and A18).

use std::collections::BTreeMap;

use kinewright_core::{
    AssetId, AudioBus, AudioBusId, AudioMaster, AudioMix, AudioQcException,
    AudioRepairMeasurements, AudioRepairProvenance, AudioRepairReport, AudioRepairRequest,
    AutomationCurve, ColorContext, Document, EFFECT_DESCRIPTORS, Effect, EffectId, EffectUniform,
    Keyframe, KeyframeInterpolation, MediaAsset, MediaCatalog, MediaKind, MediaSourceFingerprint,
    MixSpectrumPoint, NOISE_PROFILE_BAND_COUNT, NOISE_PROFILE_PARAMETER_NAMES, OpError, Operation,
    PROFILE_BAND_NEUTRAL_TENTH_DB, ParamValue, QaSeverity, REPAIR_CLICK_DENSITY_PER_MINUTE,
    REPAIR_HUM_EXCESS_HUNDREDTHS, REPAIR_LOW_SNR_HUNDREDTHS, REPAIR_MINIMUM_WINDOWS,
    REPAIR_WINDOW_MILLISECONDS, Rational, TimeCode, Track, TrackId, TrackKind,
    audio_repair_exceptions, chain_lookahead_milliseconds, effect_descriptor, has_gain_computer,
    is_audio_effect, is_hold_only_parameter, is_noise_profile_parameter, is_static_audio_parameter,
    qa_document,
};

// ---------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------

/// The eleven bus-only audio node names, in `EFFECT_DESCRIPTORS` order.
const AUDIO_EFFECT_NAMES: [&str; 11] = [
    "audio_gain",
    "audio_eq",
    "audio_compressor",
    "audio_ducking",
    "audio_limiter",
    "audio_parametric_eq",
    "audio_gate",
    "audio_true_peak_limiter",
    "audio_denoise",
    "audio_hum_removal",
    "audio_declick",
];

/// Every non-profile row AU5 §2.1 adds: effect, parameter, min, max, neutral,
/// uniform. Eleven rows, transcribed from the contract tables; the shared
/// `bypass` row is the twelfth and is checked by record equality instead.
const NEW_PARAMETER_ROWS: [(&str, &str, i64, i64, i64, EffectUniform); 11] = [
    (
        "audio_denoise",
        "reduction_tenth_db",
        0,
        400,
        0,
        EffectUniform::DenoiseReduction,
    ),
    (
        "audio_denoise",
        "floor_offset_tenth_db",
        -200,
        200,
        0,
        EffectUniform::DenoiseFloorOffset,
    ),
    (
        "audio_denoise",
        "smoothing_milliseconds",
        0,
        200,
        50,
        EffectUniform::DenoiseSmoothing,
    ),
    (
        "audio_denoise",
        "lookahead_milliseconds",
        12,
        12,
        12,
        EffectUniform::DenoiseLookahead,
    ),
    (
        "audio_hum_removal",
        "fundamental_hertz",
        50,
        60,
        50,
        EffectUniform::HumFundamental,
    ),
    (
        "audio_hum_removal",
        "harmonic_count",
        1,
        10,
        1,
        EffectUniform::HumHarmonicCount,
    ),
    (
        "audio_hum_removal",
        "depth_tenth_db",
        -600,
        0,
        0,
        EffectUniform::HumDepth,
    ),
    (
        "audio_hum_removal",
        "notch_q_hundredths",
        100,
        1_800,
        1_200,
        EffectUniform::HumNotchQ,
    ),
    (
        "audio_declick",
        "max_click_milliseconds",
        0,
        1,
        0,
        EffectUniform::DeclickMaxClick,
    ),
    (
        "audio_declick",
        "detector_threshold_tenth_db",
        60,
        400,
        240,
        EffectUniform::DeclickThreshold,
    ),
    (
        "audio_declick",
        "lookahead_milliseconds",
        3,
        3,
        3,
        EffectUniform::DeclickLookahead,
    ),
];

fn fps() -> Rational {
    Rational::new(30, 1).unwrap()
}

fn document_with_one_clip() -> Document {
    let mut doc = Document {
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps: fps(),
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    };
    Operation::AddAsset {
        asset: MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from("asset-1.wav"),
            name: "asset-1".to_owned(),
            duration: TimeCode(300),
            fps: fps(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

fn effect_with(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
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

// ---------------------------------------------------------------------------
// A1: the three descriptors
// ---------------------------------------------------------------------------

/// AU5 §7 item A1.
#[test]
fn au5_descriptor_tables_match_the_contract_exactly() {
    // `is_audio_effect` accepts exactly eleven names, in registry order.
    for name in AUDIO_EFFECT_NAMES {
        assert!(is_audio_effect(name), "{name} must be an audio effect");
        assert!(
            effect_descriptor(name).is_some(),
            "{name} must be registered"
        );
    }
    let registered_audio: Vec<&str> = EFFECT_DESCRIPTORS
        .iter()
        .map(|descriptor| descriptor.name)
        .filter(|name| is_audio_effect(name))
        .collect();
    assert_eq!(registered_audio, AUDIO_EFFECT_NAMES);
    assert!(!is_audio_effect("audio_denoiser"));
    assert!(!is_audio_effect("audio_hum"));
    assert!(!is_audio_effect("audio_click"));

    // The registry grew from 25 entries to 28, and the three repair nodes are
    // appended after `audio_true_peak_limiter`.
    assert_eq!(EFFECT_DESCRIPTORS.len(), 28);
    let tail: Vec<&str> = EFFECT_DESCRIPTORS
        .iter()
        .rev()
        .take(4)
        .map(|descriptor| descriptor.name)
        .collect();
    assert_eq!(
        tail,
        [
            "audio_declick",
            "audio_hum_removal",
            "audio_denoise",
            "audio_true_peak_limiter"
        ]
    );

    // 36 + 5 + 4 rows, counting the shared `bypass` once per descriptor.
    for (name, rows) in [
        ("audio_denoise", 36),
        ("audio_hum_removal", 5),
        ("audio_declick", 4),
    ] {
        assert_eq!(
            effect_descriptor(name)
                .expect("registered")
                .parameters
                .len(),
            rows,
            "{name} row count"
        );
    }

    // Every new row's min, max, neutral and uniform are the §2.1 tables.
    for (effect, parameter, min, max, neutral, uniform) in NEW_PARAMETER_ROWS {
        let descriptor = effect_descriptor(effect)
            .and_then(|descriptor| descriptor.parameter(parameter))
            .unwrap_or_else(|| panic!("{effect} must expose {parameter}"));
        assert_eq!(descriptor.min, min, "{effect}.{parameter} min");
        assert_eq!(descriptor.max, max, "{effect}.{parameter} max");
        assert_eq!(descriptor.neutral, neutral, "{effect}.{parameter} neutral");
        assert_eq!(descriptor.uniform, uniform, "{effect}.{parameter} uniform");
    }

    // The Q row is `notch_q_hundredths` (R17) and nothing else, so the app's
    // `mixer_unit` cannot read it as a ratio.
    let hum = effect_descriptor("audio_hum_removal").expect("registered");
    assert!(hum.parameter("q_hundredths").is_none());
    assert!(hum.parameter("notch_q_hundredths").is_some());
    // `audio_hum_removal` carries no `lookahead_milliseconds` row at all, so
    // it contributes exactly 0 to a chain's declared latency.
    assert!(hum.parameter("lookahead_milliseconds").is_none());

    // All three `bypass` rows are the one shared record.
    let bypass_rows: Vec<_> = ["audio_denoise", "audio_hum_removal", "audio_declick"]
        .into_iter()
        .map(|name| {
            effect_descriptor(name)
                .and_then(|descriptor| descriptor.parameter("bypass"))
                .expect("bypass row")
        })
        .collect();
    for row in &bypass_rows {
        assert_eq!(row, &bypass_rows[0]);
        assert_eq!(row.uniform, EffectUniform::AudioBypass);
        assert_eq!((row.min, row.max, row.neutral), (0, 1, 0));
    }
    // The shared row is the pre-AU5 one, not a new copy of it.
    assert_eq!(
        bypass_rows[0],
        effect_descriptor("audio_gate")
            .and_then(|descriptor| descriptor.parameter("bypass"))
            .expect("bypass row")
    );

    // Twelve new uniforms, all 31 profile rows sharing one of them.
    let mut new_uniforms: Vec<String> = NEW_PARAMETER_ROWS
        .iter()
        .map(|row| format!("{:?}", row.5))
        .collect();
    new_uniforms.push(format!("{:?}", EffectUniform::DenoiseProfileBand));
    new_uniforms.sort();
    new_uniforms.dedup();
    assert_eq!(new_uniforms.len(), 12);

    // Read from the descriptors themselves, so reusing a pre-AU5 audio uniform
    // on a new row would be caught: 45 new rows carry 13 distinct uniforms —
    // the twelve new ones plus the shared `AudioBypass`.
    let mut live: Vec<String> = ["audio_denoise", "audio_hum_removal", "audio_declick"]
        .into_iter()
        .flat_map(|name| effect_descriptor(name).expect("registered").parameters)
        .map(|parameter| format!("{:?}", parameter.uniform))
        .collect();
    assert_eq!(live.len(), 45);
    live.sort();
    live.dedup();
    assert_eq!(live.len(), 13);
}

/// AU5 §2.1 rules 5 and 7: the 31-row profile block and its predicate.
#[test]
fn au5_profile_rows_are_thirty_one_neutral_minus_twelve_hundred_rows() {
    let denoise = effect_descriptor("audio_denoise").expect("registered");
    let profile: Vec<_> = denoise
        .parameters
        .iter()
        .filter(|parameter| is_noise_profile_parameter(parameter.name))
        .collect();
    assert_eq!(profile.len(), NOISE_PROFILE_BAND_COUNT);
    assert_eq!(profile.len(), 31);
    for (index, parameter) in profile.iter().enumerate() {
        assert_eq!(parameter.name, NOISE_PROFILE_PARAMETER_NAMES[index]);
        assert_eq!(
            parameter.name,
            format!("profile_band{:02}_tenth_db", index + 1)
        );
        assert_eq!(parameter.min, PROFILE_BAND_NEUTRAL_TENTH_DB);
        assert_eq!(parameter.min, -1_200);
        assert_eq!(parameter.max, 0);
        assert_eq!(parameter.neutral, PROFILE_BAND_NEUTRAL_TENTH_DB);
        assert_eq!(parameter.uniform, EffectUniform::DenoiseProfileBand);
    }

    // The profile block is contiguous and last, so the five controls read
    // first in every generated prose and every inspector.
    assert!(
        denoise.parameters[..5]
            .iter()
            .all(|parameter| !is_noise_profile_parameter(parameter.name))
    );

    // The predicate is true for exactly the 31 registered names and for no
    // other parameter in the whole registry.
    let matching: Vec<&str> = EFFECT_DESCRIPTORS
        .iter()
        .flat_map(|descriptor| descriptor.parameters)
        .map(|parameter| parameter.name)
        .filter(|name| is_noise_profile_parameter(name))
        .collect();
    assert_eq!(matching.len(), 31);
    assert!(!is_noise_profile_parameter("profile_band01"));
    assert!(!is_noise_profile_parameter("reduction_tenth_db"));
    assert!(!is_noise_profile_parameter("band1_gain_tenth_db"));
}

/// AU5 §7 item A1: an all-neutral repair node serialises with no profile rows.
#[test]
fn an_all_neutral_denoise_node_carries_no_profile_rows_on_the_wire() {
    let mut doc = document_with_one_clip();
    Operation::UpsertAudioBus {
        bus: bus_with(vec![effect_with(
            1,
            "audio_denoise",
            &[
                ("bypass", 0),
                ("reduction_tenth_db", 0),
                ("floor_offset_tenth_db", 0),
                ("smoothing_milliseconds", 50),
                ("lookahead_milliseconds", 12),
            ],
        )]),
    }
    .apply(&mut doc)
    .unwrap();
    let encoded = serde_json::to_string(&doc).unwrap();
    assert!(!encoded.contains("profile_band"));
    let round_tripped: Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(round_tripped, doc);

    // A learned node writes all 31 and round-trips them.
    let learned: Vec<(&str, i64)> = NOISE_PROFILE_PARAMETER_NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| (*name, -600 - i64::try_from(index).unwrap()))
        .collect();
    let mut parameters = vec![("reduction_tenth_db", 200)];
    parameters.extend(learned.iter().copied());
    let mut doc = document_with_one_clip();
    Operation::UpsertAudioBus {
        bus: bus_with(vec![effect_with(1, "audio_denoise", &parameters)]),
    }
    .apply(&mut doc)
    .unwrap();
    let encoded = serde_json::to_string(&doc).unwrap();
    assert!(encoded.contains("profile_band01_tenth_db"));
    assert!(encoded.contains("profile_band31_tenth_db"));
    assert_eq!(serde_json::from_str::<Document>(&encoded).unwrap(), doc);

    // A band outside `-1200..=0` is an ordinary descriptor-domain refusal:
    // AU5 adds no `OpError` variant (rule 14).
    let mut doc = document_with_one_clip();
    let error = Operation::UpsertAudioBus {
        bus: bus_with(vec![effect_with(
            1,
            "audio_denoise",
            &[("profile_band01_tenth_db", 1)],
        )]),
    }
    .apply(&mut doc)
    .unwrap_err();
    assert!(
        matches!(error, OpError::EffectParamOutOfRange { .. }),
        "a band outside the domain is a descriptor failure, got {error:?}"
    );
}

// ---------------------------------------------------------------------------
// A3: static, hold-only and the two reason strings
// ---------------------------------------------------------------------------

/// AU5 §7 item A3: `is_static_audio_parameter`'s exact set.
#[test]
fn au5_static_audio_parameters_are_the_five_pairs_plus_the_profile_block() {
    // Every (effect, parameter) pair in the whole registry, partitioned by the
    // predicate: nothing outside the expected set may creep in.
    let mut accepted: Vec<(&str, &str)> = Vec::new();
    for descriptor in EFFECT_DESCRIPTORS {
        for parameter in descriptor.parameters {
            if is_static_audio_parameter(descriptor.name, parameter.name) {
                accepted.push((descriptor.name, parameter.name));
            }
        }
    }
    let mut expected: Vec<(&str, &str)> = vec![
        ("audio_compressor", "lookahead_milliseconds"),
        ("audio_compressor", "rms_window_milliseconds"),
        ("audio_true_peak_limiter", "lookahead_milliseconds"),
        ("audio_denoise", "lookahead_milliseconds"),
        ("audio_declick", "lookahead_milliseconds"),
    ];
    expected.extend(
        NOISE_PROFILE_PARAMETER_NAMES
            .iter()
            .map(|name| ("audio_denoise", *name)),
    );
    accepted.sort_unstable();
    expected.sort_unstable();
    assert_eq!(accepted, expected);
    assert_eq!(accepted.len(), 5 + 31);

    // `max_click_milliseconds` is deliberately absent (R6): it is a threshold,
    // not an allocation.
    assert!(!is_static_audio_parameter(
        "audio_declick",
        "max_click_milliseconds"
    ));
    // The profile arm is guarded on the effect name, so the same row spelling
    // on another node is not static.
    assert!(!is_static_audio_parameter(
        "audio_hum_removal",
        "profile_band01_tenth_db"
    ));
    assert!(!is_static_audio_parameter(
        "audio_gate",
        "profile_band01_tenth_db"
    ));

    // The two lookahead arms are load-bearing twice over: they are what
    // `chain_lookahead_milliseconds` sums.
    for (name, declared) in [
        ("audio_denoise", 12),
        ("audio_declick", 3),
        ("audio_hum_removal", 0),
        ("audio_true_peak_limiter", 5),
    ] {
        assert_eq!(
            chain_lookahead_milliseconds(&[effect_with(1, name, &[])]),
            declared,
            "{name} declares {declared} ms from the descriptor neutral alone"
        );
    }
}

/// AU5 §7 item A3: `harmonic_count` joins the hold-only branch.
#[test]
fn au5_hum_harmonic_count_is_hold_only() {
    assert!(is_hold_only_parameter(
        "audio_hum_removal",
        "harmonic_count"
    ));
    // The branch is keyed on `is_audio_effect`, so it reads
    // `bypass | detector | true_peak | harmonic_count` for every audio node.
    for name in AUDIO_EFFECT_NAMES {
        assert!(is_hold_only_parameter(name, "bypass"));
        assert!(is_hold_only_parameter(name, "harmonic_count"));
        assert!(!is_hold_only_parameter(name, "depth_tenth_db"));
        assert!(!is_hold_only_parameter(name, "reduction_tenth_db"));
    }
    assert!(!is_hold_only_parameter("brightness", "harmonic_count"));

    let base = document_with_one_clip();
    for interpolation in [
        KeyframeInterpolation::Linear,
        KeyframeInterpolation::EaseInOut,
    ] {
        let mut doc = base.clone();
        let error = Operation::UpsertAudioBus {
            bus: bus_with(vec![keyed_effect(
                1,
                "audio_hum_removal",
                "harmonic_count",
                4,
                interpolation,
            )]),
        }
        .apply(&mut doc)
        .unwrap_err();
        assert_eq!(
            error,
            OpError::NonHoldKeyframeParameter {
                effect: "audio_hum_removal".to_owned(),
                name: "harmonic_count".to_owned(),
            }
        );
        assert_eq!(doc, base);
    }
    // `Hold` is accepted: a section count is a switch, not a scalar.
    let mut doc = base.clone();
    Operation::UpsertAudioBus {
        bus: bus_with(vec![keyed_effect(
            1,
            "audio_hum_removal",
            "harmonic_count",
            4,
            KeyframeInterpolation::Hold,
        )]),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.audio_mix.buses[0].effects[0].name, "audio_hum_removal");
}

/// AU5 §7 item A3: the two reason strings, each raised by the right rows.
#[test]
fn au5_static_rejections_carry_the_two_distinct_reasons() {
    let base = document_with_one_clip();
    for (effect, parameter, value, reason) in [
        (
            "audio_denoise",
            "lookahead_milliseconds",
            12,
            "sets processing latency and cannot be keyframed",
        ),
        (
            "audio_declick",
            "lookahead_milliseconds",
            3,
            "sets processing latency and cannot be keyframed",
        ),
        (
            "audio_denoise",
            "profile_band01_tenth_db",
            -600,
            "is read when the chain is built or retuned and cannot be keyframed",
        ),
        (
            "audio_denoise",
            "profile_band31_tenth_db",
            -600,
            "is read when the chain is built or retuned and cannot be keyframed",
        ),
        (
            "audio_compressor",
            "rms_window_milliseconds",
            20,
            "is read when the chain is built or retuned and cannot be keyframed",
        ),
    ] {
        assert!(is_static_audio_parameter(effect, parameter));
        for interpolation in [KeyframeInterpolation::Hold, KeyframeInterpolation::Linear] {
            let mut doc = base.clone();
            let error = Operation::UpsertAudioBus {
                bus: bus_with(vec![keyed_effect(
                    1,
                    effect,
                    parameter,
                    value,
                    interpolation,
                )]),
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
        }
        // The same parameter is legal as a stored static value.
        let mut doc = base.clone();
        Operation::UpsertAudioBus {
            bus: bus_with(vec![effect_with(1, effect, &[(parameter, value)])]),
        }
        .apply(&mut doc)
        .unwrap();
        assert_eq!(
            doc.audio_mix.buses[0].effects[0].parameters[parameter],
            ParamValue::Integer(value)
        );
    }

    // The master chain raises the same two reasons.
    let mut doc = base.clone();
    let error = Operation::SetAudioMaster {
        master: AudioMaster {
            gain_tenth_db: 0,
            gain_curve: None,
            effects: vec![keyed_effect(
                1,
                "audio_denoise",
                "profile_band05_tenth_db",
                -700,
                KeyframeInterpolation::Hold,
            )],
        },
    }
    .apply(&mut doc)
    .unwrap_err();
    assert_eq!(
        error,
        OpError::InvalidEffectAutomation {
            effect: "audio_denoise".to_owned(),
            name: "profile_band05_tenth_db".to_owned(),
            reason: "is read when the chain is built or retuned and cannot be keyframed".to_owned(),
        }
    );
}

// ---------------------------------------------------------------------------
// A14 (core half): the 20 ms table
// ---------------------------------------------------------------------------

/// AU5 §2.3 rule 16's table, transcribed.
#[test]
fn au5_repair_chains_spend_the_twenty_millisecond_budget_exactly() {
    let repair = || {
        vec![
            effect_with(1, "audio_denoise", &[]),
            effect_with(2, "audio_hum_removal", &[]),
            effect_with(3, "audio_declick", &[]),
        ]
    };

    // Row 1 of rule 16's table: the repair chain on its own, 12 + 0 + 3.
    let fifteen = repair();

    // Row 2: the same bus after `plan_audio_normalization` adds a
    // compressor at its neutral 0 and the true-peak limiter at its neutral 5.
    let mut twenty_by_neutral = repair();
    twenty_by_neutral.push(effect_with(4, "audio_compressor", &[]));
    twenty_by_neutral.push(effect_with(5, "audio_true_peak_limiter", &[]));

    // Row 3: repair plus a 5 ms lookahead compressor and **no** bus limiter —
    // distinct from row 2 because the 5 ms is written, not inherited.
    let mut twenty_by_compressor = repair();
    twenty_by_compressor.push(effect_with(
        4,
        "audio_compressor",
        &[("lookahead_milliseconds", 5)],
    ));

    // One millisecond over the budget: the sharpest refusal boundary, and the
    // one A14 names. A regression that compared `> 21` would still pass the
    // 35 ms arm below.
    let mut twenty_one = repair();
    twenty_one.push(effect_with(
        4,
        "audio_compressor",
        &[("lookahead_milliseconds", 6)],
    ));

    // Row 4: AU2's worst legal chain plus any repair.
    let mut thirty_five = repair();
    thirty_five.push(effect_with(
        4,
        "audio_compressor",
        &[("lookahead_milliseconds", 10)],
    ));
    thirty_five.push(effect_with(
        5,
        "audio_true_peak_limiter",
        &[("lookahead_milliseconds", 10)],
    ));

    let base = document_with_one_clip();
    for (effects, declared, accepted) in [
        (fifteen, 15, true),
        (twenty_by_neutral, 20, true),
        (twenty_by_compressor, 20, true),
        (twenty_one, 21, false),
        (thirty_five, 35, false),
    ] {
        // The declared figure is the contract's, transcribed, not the
        // implementation's read back out of itself.
        assert_eq!(chain_lookahead_milliseconds(&effects), declared);
        let mut doc = base.clone();
        let result = Operation::UpsertAudioBus {
            bus: bus_with(effects),
        }
        .apply(&mut doc);
        if accepted {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err(),
                OpError::AudioBusLookaheadExceeded {
                    bus: AudioBusId(1),
                    milliseconds: declared,
                }
            );
            assert_eq!(doc, base);
        }
    }
}

/// AU5 §2.3 rule 16's last row: the master chain against its own budget.
#[test]
fn au5_the_master_chain_spends_its_own_twenty_millisecond_budget() {
    let base = document_with_one_clip();
    let master = |effects: Vec<Effect>| Operation::SetAudioMaster {
        master: AudioMaster {
            gain_tenth_db: 0,
            gain_curve: None,
            effects,
        },
    };

    // Row 5: a master delivery chain is one true-peak limiter at 5 ms.
    let delivery = vec![effect_with(1, "audio_true_peak_limiter", &[])];
    assert_eq!(chain_lookahead_milliseconds(&delivery), 5);
    let mut doc = base.clone();
    master(delivery).apply(&mut doc).unwrap();

    // A repair prefix on the master is legal at 15, and at 20 with the
    // limiter, exactly as it is on a bus.
    let repair = vec![
        effect_with(1, "audio_denoise", &[]),
        effect_with(2, "audio_hum_removal", &[]),
        effect_with(3, "audio_declick", &[]),
    ];
    assert_eq!(chain_lookahead_milliseconds(&repair), 15);
    let mut doc = base.clone();
    master(repair.clone()).apply(&mut doc).unwrap();

    let mut at_twenty = repair.clone();
    at_twenty.push(effect_with(4, "audio_true_peak_limiter", &[]));
    assert_eq!(chain_lookahead_milliseconds(&at_twenty), 20);
    let mut doc = base.clone();
    master(at_twenty).apply(&mut doc).unwrap();

    // One millisecond over is the master-flavoured refusal, which carries no
    // bus id.
    let mut over = repair;
    over.push(effect_with(
        4,
        "audio_compressor",
        &[("lookahead_milliseconds", 6)],
    ));
    assert_eq!(chain_lookahead_milliseconds(&over), 21);
    let mut doc = base.clone();
    assert_eq!(
        master(over).apply(&mut doc).unwrap_err(),
        OpError::AudioMasterLookaheadExceeded { milliseconds: 21 }
    );
    assert_eq!(doc, base);
}

// ---------------------------------------------------------------------------
// A13: `has_gain_computer`
// ---------------------------------------------------------------------------

/// AU5 §7 item A13, core half.
#[test]
fn au5_has_gain_computer_is_five_names_in_core() {
    let accepted: Vec<&str> = AUDIO_EFFECT_NAMES
        .into_iter()
        .filter(|name| has_gain_computer(name))
        .collect();
    assert_eq!(
        accepted,
        [
            "audio_compressor",
            "audio_ducking",
            "audio_gate",
            "audio_true_peak_limiter",
            "audio_denoise",
        ]
    );
    // Hum removal and de-click are not gain computers and get no bar.
    assert!(!has_gain_computer("audio_hum_removal"));
    assert!(!has_gain_computer("audio_declick"));
    assert!(!has_gain_computer("audio_gain"));
    assert!(!has_gain_computer("brightness"));
    // Every accepted name is an audio node.
    assert!(accepted.iter().all(|name| is_audio_effect(name)));
}

// ---------------------------------------------------------------------------
// A18 (core half): the `noise_profile_missing` warning
// ---------------------------------------------------------------------------

fn denoise_bus_document(effects: Vec<Effect>) -> Document {
    let mut doc = document_with_one_clip();
    Operation::UpsertAudioBus {
        bus: bus_with(effects),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

fn noise_profile_issues(doc: &Document) -> Vec<String> {
    qa_document(doc)
        .issues
        .into_iter()
        .filter(|issue| issue.code == "noise_profile_missing")
        .map(|issue| {
            assert_eq!(issue.severity, QaSeverity::Warning);
            issue.message
        })
        .collect()
}

/// AU5 §7 item A18, core half: rule 18's warning fires and does not.
#[test]
fn au5_noise_profile_missing_is_advice_about_a_wasted_node() {
    // Reducing, no profile written at all: absent is the neutral.
    let doc = denoise_bus_document(vec![effect_with(
        7,
        "audio_denoise",
        &[("reduction_tenth_db", 200)],
    )]);
    let issues = noise_profile_issues(&doc);
    assert_eq!(issues.len(), 1);
    assert_eq!(
        issues[0],
        "denoise node 7 on bus 1 has a reduction of 200 tenth dB but no learned profile; learn one or the node does nothing"
    );
    // It is advice, not a gate: `export_ready` counts `Error` only, so the
    // warning cannot move it. Compared against the same document with the
    // node's reduction at 0, which raises no warning at all.
    let quiet = denoise_bus_document(vec![effect_with(
        7,
        "audio_denoise",
        &[("reduction_tenth_db", 0)],
    )]);
    assert_eq!(
        qa_document(&doc).export_ready(),
        qa_document(&quiet).export_ready()
    );
    assert_eq!(
        qa_document(&doc).count(QaSeverity::Error),
        qa_document(&quiet).count(QaSeverity::Error)
    );

    // Reducing with all 31 explicitly at -1200: the same document, spelled out.
    let mut parameters = vec![("reduction_tenth_db", 200)];
    let neutral: Vec<(&str, i64)> = NOISE_PROFILE_PARAMETER_NAMES
        .iter()
        .map(|name| (*name, PROFILE_BAND_NEUTRAL_TENTH_DB))
        .collect();
    parameters.extend(neutral.iter().copied());
    let doc = denoise_bus_document(vec![effect_with(7, "audio_denoise", &parameters)]);
    assert_eq!(noise_profile_issues(&doc).len(), 1);

    // One learned band clears it: `Write all 31 or none` is a document rule,
    // not a QA one.
    let mut parameters = vec![("reduction_tenth_db", 200)];
    parameters.extend(neutral.iter().copied());
    parameters.push(("profile_band17_tenth_db", -650));
    let doc = denoise_bus_document(vec![effect_with(7, "audio_denoise", &parameters)]);
    assert!(noise_profile_issues(&doc).is_empty());

    // `reduction_tenth_db = 0` is an identity, so an unlearned node there is
    // not a wasted node.
    let doc = denoise_bus_document(vec![effect_with(
        7,
        "audio_denoise",
        &[("reduction_tenth_db", 0)],
    )]);
    assert!(noise_profile_issues(&doc).is_empty());
    // The row's absence resolves to the descriptor neutral, which is 0.
    let doc = denoise_bus_document(vec![effect_with(7, "audio_denoise", &[])]);
    assert!(noise_profile_issues(&doc).is_empty());

    // A curve that rides the reduction up still earns the warning: "resolves
    // above 0" reads the whole curve.
    let doc = denoise_bus_document(vec![keyed_effect(
        7,
        "audio_denoise",
        "reduction_tenth_db",
        300,
        KeyframeInterpolation::Linear,
    )]);
    assert_eq!(noise_profile_issues(&doc).len(), 1);

    // The master chain is checked too, and the message names its owner.
    let mut doc = document_with_one_clip();
    Operation::SetAudioMaster {
        master: AudioMaster {
            gain_tenth_db: 0,
            gain_curve: None,
            effects: vec![effect_with(
                9,
                "audio_denoise",
                &[("reduction_tenth_db", 120)],
            )],
        },
    }
    .apply(&mut doc)
    .unwrap();
    let issues = noise_profile_issues(&doc);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("on the master"));

    // Neither of the other two repair nodes raises it.
    let doc = denoise_bus_document(vec![
        effect_with(1, "audio_hum_removal", &[("depth_tenth_db", -300)]),
        effect_with(2, "audio_declick", &[("max_click_milliseconds", 1)]),
    ]);
    assert!(noise_profile_issues(&doc).is_empty());
}

// ---------------------------------------------------------------------------
// A10 (core half): `audio_repair.rs`
// ---------------------------------------------------------------------------

fn measurements() -> AudioRepairMeasurements {
    AudioRepairMeasurements {
        windows: 600,
        noise_floor_dbfs_hundredths: Some(-6_000),
        signal_dbfs_hundredths: Some(-1_800),
        snr_db_hundredths: Some(4_200),
        hum_50_excess_db_hundredths: Some(0),
        hum_60_excess_db_hundredths: Some(0),
        click_count: 0,
        click_density_per_minute: 0,
    }
}

fn codes(exceptions: &[AudioQcException]) -> Vec<&str> {
    exceptions
        .iter()
        .map(|exception| exception.code.as_str())
        .collect()
}

/// AU5 §2.4 rule 24: four thresholds, and a clean measurement raises nothing.
#[test]
fn au5_repair_exceptions_fire_over_each_threshold_and_not_under_it() {
    assert!(audio_repair_exceptions(&measurements()).is_empty());

    // `low_signal_to_noise`: strictly under the threshold.
    let mut measured = measurements();
    measured.snr_db_hundredths = Some(REPAIR_LOW_SNR_HUNDREDTHS);
    assert!(audio_repair_exceptions(&measured).is_empty());
    measured.snr_db_hundredths = Some(REPAIR_LOW_SNR_HUNDREDTHS - 1);
    let raised = audio_repair_exceptions(&measured);
    assert_eq!(codes(&raised), ["low_signal_to_noise"]);
    assert_eq!(raised[0].severity, QaSeverity::Warning);
    assert_eq!(raised[0].field.as_deref(), Some("snr_db_hundredths"));
    assert_eq!(raised[0].observed.as_deref(), Some("1199"));
    // The published wording names the percentile and its direction.
    assert!(raised[0].message.contains("percentile"));

    // `mains_hum_present`: strictly over the threshold, once per frequency.
    let mut measured = measurements();
    measured.hum_50_excess_db_hundredths = Some(REPAIR_HUM_EXCESS_HUNDREDTHS);
    assert!(audio_repair_exceptions(&measured).is_empty());
    measured.hum_50_excess_db_hundredths = Some(REPAIR_HUM_EXCESS_HUNDREDTHS + 1);
    let raised = audio_repair_exceptions(&measured);
    assert_eq!(codes(&raised), ["mains_hum_present"]);
    assert_eq!(
        raised[0].field.as_deref(),
        Some("hum_50_excess_db_hundredths")
    );
    measured.hum_60_excess_db_hundredths = Some(4_000);
    let raised = audio_repair_exceptions(&measured);
    assert_eq!(codes(&raised), ["mains_hum_present", "mains_hum_present"]);
    // The field-ascending tiebreak orders 50 before 60.
    assert_eq!(
        raised
            .iter()
            .map(|exception| exception.field.clone().unwrap())
            .collect::<Vec<_>>(),
        ["hum_50_excess_db_hundredths", "hum_60_excess_db_hundredths"]
    );
    // A hum excess that was never measured raises nothing.
    let mut measured = measurements();
    measured.hum_50_excess_db_hundredths = None;
    measured.hum_60_excess_db_hundredths = None;
    assert!(audio_repair_exceptions(&measured).is_empty());

    // `click_density_high`: strictly over the threshold.
    let mut measured = measurements();
    measured.click_count = 30;
    measured.click_density_per_minute = REPAIR_CLICK_DENSITY_PER_MINUTE;
    assert!(audio_repair_exceptions(&measured).is_empty());
    measured.click_density_per_minute = REPAIR_CLICK_DENSITY_PER_MINUTE + 1;
    let raised = audio_repair_exceptions(&measured);
    assert_eq!(codes(&raised), ["click_density_high"]);
    assert_eq!(raised[0].severity, QaSeverity::Warning);

    // `low_window_count` is `Info`, and rule 22 makes it exclusive of the SNR
    // warning: below the minimum the percentiles are `None`.
    let measured = AudioRepairMeasurements {
        windows: REPAIR_MINIMUM_WINDOWS - 1,
        noise_floor_dbfs_hundredths: None,
        signal_dbfs_hundredths: None,
        snr_db_hundredths: None,
        ..measurements()
    };
    let raised = audio_repair_exceptions(&measured);
    assert_eq!(codes(&raised), ["low_window_count"]);
    assert_eq!(raised[0].severity, QaSeverity::Info);
    assert_eq!(raised[0].observed.as_deref(), Some("9"));
    // Pinned whole, and with a phrase the pre-AU5-review wording ("Only 9 of
    // the 10 ms windows carried energy") did not satisfy: a reader must be
    // able to tell the window *count* from the window *length* when both are
    // 10.
    assert!(raised[0].message.contains("windows of 10 ms"));
    assert_eq!(
        raised[0].message,
        "Only 9 windows of 10 ms carried energy, under the 10 windows a percentile needs, so the noise floor, signal and SNR are not reported."
    );
    let measured = AudioRepairMeasurements {
        windows: REPAIR_MINIMUM_WINDOWS,
        ..measurements()
    };
    assert!(audio_repair_exceptions(&measured).is_empty());
}

/// AU5 §2.4 rule 24: the sort order is `audio_qc_exceptions`', shared.
#[test]
fn au5_repair_exceptions_sort_by_severity_then_code_then_field() {
    let measured = AudioRepairMeasurements {
        windows: REPAIR_MINIMUM_WINDOWS - 1,
        noise_floor_dbfs_hundredths: None,
        signal_dbfs_hundredths: None,
        snr_db_hundredths: None,
        hum_50_excess_db_hundredths: Some(5_000),
        hum_60_excess_db_hundredths: Some(4_000),
        click_count: 900,
        click_density_per_minute: 900,
    };
    let raised = audio_repair_exceptions(&measured);
    assert_eq!(
        codes(&raised),
        [
            // Warnings first, by code ascending, then by field ascending.
            "click_density_high",
            "mains_hum_present",
            "mains_hum_present",
            // Then the one `Info`.
            "low_window_count",
        ]
    );
    assert_eq!(
        raised
            .iter()
            .map(|exception| exception.field.clone().unwrap())
            .collect::<Vec<_>>(),
        [
            "click_density_per_minute",
            "hum_50_excess_db_hundredths",
            "hum_60_excess_db_hundredths",
            "windows",
        ]
    );

    // With every SNR field populated the low-SNR warning joins them in code
    // order, between the click density and the hum.
    let measured = AudioRepairMeasurements {
        windows: 600,
        noise_floor_dbfs_hundredths: Some(-3_000),
        signal_dbfs_hundredths: Some(-2_400),
        snr_db_hundredths: Some(600),
        ..measured
    };
    assert_eq!(
        codes(&audio_repair_exceptions(&measured)),
        [
            "click_density_high",
            "low_signal_to_noise",
            "mains_hum_present",
            "mains_hum_present",
        ]
    );
}

fn report() -> AudioRepairReport {
    AudioRepairReport {
        range: TimeCode(0)..TimeCode(300),
        point: MixSpectrumPoint::Bus(AudioBusId(1)),
        sample_rate: 48_000,
        sample_frames: 480_000,
        window_milliseconds: REPAIR_WINDOW_MILLISECONDS,
        windows: 1_000,
        noise_floor_dbfs_hundredths: Some(-5_500),
        signal_dbfs_hundredths: Some(-1_500),
        snr_db_hundredths: Some(4_000),
        hum_50_excess_db_hundredths: Some(1_200),
        hum_60_excess_db_hundredths: Some(0),
        hum_50_harmonic_excess_db_hundredths: vec![800, 300, 100, 0],
        hum_60_harmonic_excess_db_hundredths: vec![0, 0, 0, 0],
        click_count: 19,
        click_density_per_minute: 114,
        findings: audio_repair_exceptions(&measurements()),
        evidence_only: true,
        provenance: AudioRepairProvenance::default(),
    }
}

/// AU5 §2.4 rule 20 and §7 item A15: the wire types round-trip and the request
/// refuses a misspelled field rather than defaulting it.
#[test]
fn au5_repair_types_round_trip_and_deny_unknown_fields() {
    let request = AudioRepairRequest {
        range: Some(TimeCode(10)..TimeCode(200)),
        point: MixSpectrumPoint::Master,
    };
    let encoded = serde_json::to_string(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<AudioRepairRequest>(&encoded).unwrap(),
        request
    );
    // An omitted range measures the whole document and is omitted on the wire.
    let whole = AudioRepairRequest {
        range: None,
        point: MixSpectrumPoint::Track(TrackId(3)),
    };
    let encoded = serde_json::to_string(&whole).unwrap();
    assert!(!encoded.contains("range"));
    assert_eq!(
        serde_json::from_str::<AudioRepairRequest>(&encoded).unwrap(),
        whole
    );
    // `deny_unknown_fields`: a misspelling is refused, not defaulted.
    let error = serde_json::from_str::<AudioRepairRequest>(
        r#"{"point":"master","rnage":{"start":0,"end":10}}"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");

    let report = report();
    let encoded = serde_json::to_string(&report).unwrap();
    assert_eq!(
        serde_json::from_str::<AudioRepairReport>(&encoded).unwrap(),
        report
    );
    // Every optional leaf is omitted when it is absent.
    let sparse = AudioRepairReport {
        noise_floor_dbfs_hundredths: None,
        signal_dbfs_hundredths: None,
        snr_db_hundredths: None,
        hum_50_excess_db_hundredths: None,
        hum_60_excess_db_hundredths: None,
        hum_50_harmonic_excess_db_hundredths: Vec::new(),
        hum_60_harmonic_excess_db_hundredths: Vec::new(),
        findings: Vec::new(),
        ..report.clone()
    };
    let encoded = serde_json::to_string(&sparse).unwrap();
    for absent in [
        "noise_floor_dbfs_hundredths",
        "signal_dbfs_hundredths",
        "snr_db_hundredths",
        "hum_50_excess_db_hundredths",
        "hum_60_excess_db_hundredths",
        "hum_50_harmonic_excess_db_hundredths",
        "hum_60_harmonic_excess_db_hundredths",
        "findings",
    ] {
        assert!(!encoded.contains(absent), "{absent} must be omitted");
    }
    assert_eq!(
        serde_json::from_str::<AudioRepairReport>(&encoded).unwrap(),
        sparse
    );

    // The provenance is fixed strings with a manual `Default`.
    let provenance = AudioRepairProvenance::default();
    assert_eq!(provenance.engine, "kinewright_audio_repair_v1");
    assert_eq!(provenance.measurement_rate, 48_000);
    assert_eq!(
        provenance.exception_order,
        "severity_desc_code_asc_field_asc"
    );
    assert!(provenance.signal_to_noise.contains("percentile"));

    // Rule 20: every leaf is an integer, which is what lets every serialized
    // AU5 type derive `Eq`.
    let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_integer_leaves(&value, "report");
    let value: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    assert_integer_leaves(&value, "report");
}

/// No `f32`/`f64` reaches any AU5 wire type, so no leaf is fractional.
fn assert_integer_leaves(value: &serde_json::Value, path: &str) {
    match value {
        serde_json::Value::Number(number) => assert!(
            number.is_i64() || number.is_u64(),
            "{path} is not an integer: {number}"
        ),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_integer_leaves(item, &format!("{path}[{index}]"));
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, item) in fields {
                assert_integer_leaves(item, &format!("{path}.{key}"));
            }
        }
        _ => {}
    }
}

/// AU5 §2.4 rule 25: the point type is reused, not re-spelled.
#[test]
fn au5_repair_reuses_the_mix_spectrum_point_spelling() {
    for point in [
        MixSpectrumPoint::Master,
        MixSpectrumPoint::Track(TrackId(2)),
        MixSpectrumPoint::Bus(AudioBusId(4)),
    ] {
        let request = AudioRepairRequest { range: None, point };
        let encoded = serde_json::to_string(&request).unwrap();
        let spectrum =
            serde_json::to_string(&kinewright_core::MixSpectrumRequest { range: None, point })
                .unwrap();
        assert_eq!(encoded, spectrum);
    }
}
