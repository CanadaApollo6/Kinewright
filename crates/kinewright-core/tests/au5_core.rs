//! AU5 repair and room tone — core contract tests (AU5 §7 items A1, A3, A13
//! and the core halves of A12, A14 and A18).

use std::collections::BTreeMap;

use kinewright_core::{
    AssetId, AudioBus, AudioBusId, AudioMaster, AudioMix, AudioQcException,
    AudioRepairMeasurements, AudioRepairProvenance, AudioRepairReport, AudioRepairRequest,
    AutomationCurve, Clip, ClipContent, ClipId, ColorContext, Document, EFFECT_DESCRIPTORS, Effect,
    EffectId, EffectUniform, Keyframe, KeyframeInterpolation, MediaAsset, MediaCatalog, MediaKind,
    MediaSourceFingerprint, MixSpectrumPoint, NOISE_PROFILE_BAND_COUNT,
    NOISE_PROFILE_PARAMETER_NAMES, OpError, Operation, PROFILE_BAND_NEUTRAL_TENTH_DB, ParamValue,
    QaSeverity, REPAIR_CLICK_DENSITY_PER_MINUTE, REPAIR_HUM_EXCESS_HUNDREDTHS,
    REPAIR_LOW_SNR_HUNDREDTHS, REPAIR_MINIMUM_WINDOWS, REPAIR_WINDOW_MILLISECONDS, Rational,
    TimeCode, TimeMappingError, Track, TrackId, TrackKind, audio_repair_exceptions,
    chain_lookahead_milliseconds, covering_source_range_for_project_duration, effect_descriptor,
    has_gain_computer, is_audio_effect, is_hold_only_parameter, is_noise_profile_parameter,
    is_static_audio_parameter, longest_coverable_project_tile, map_project_duration_to_source,
    map_source_range_to_project, qa_document,
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

// ---------------------------------------------------------------------------
// B3: `Document::track_gaps` and the `qa_document` refactor (AU5 §5.2)
// ---------------------------------------------------------------------------

/// AU5 §5.2's corpus, built once and shared by the gap test and the QA
/// byte-identity pin.
///
/// Track 1 carries a **leading** gap (`0..10`), an **adjacent** join at 20 and
/// an **interior** gap (`30..40`), and stops at 50 while the document runs to
/// 90 — the **trailing** gap `track_gaps` must never report. Track 2 is
/// **empty**. Track 3's **only clip starts at 0**. Track 4 proves the walk is
/// content-agnostic: a **title closes** the span between two media clips, so
/// the track has no gap at all.
fn gap_corpus() -> Document {
    Document {
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: gap_corpus_tracks(),
        media_pool: gap_corpus_assets(),
        markers: Vec::new(),
        fps: fps(),
        resolution: (1_920, 1_080),
        duration: TimeCode(90),
    }
}

fn gap_corpus_assets() -> Vec<MediaAsset> {
    let audio = MediaAsset {
        id: AssetId(1),
        path: std::path::PathBuf::from("au5-part-b-gap-corpus.wav"),
        name: "corpus-audio".to_owned(),
        duration: TimeCode(300),
        fps: fps(),
        kind: MediaKind::Audio,
        resolution: None,
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    let video = MediaAsset {
        id: AssetId(2),
        path: std::path::PathBuf::from("au5-part-b-gap-corpus.mp4"),
        name: "corpus-video".to_owned(),
        duration: TimeCode(300),
        fps: fps(),
        kind: MediaKind::AudioVideo,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
    };
    vec![audio, video]
}

fn gap_corpus_tracks() -> Vec<Track> {
    let media_clip = |id: u64, asset: AssetId, at: i64, source: std::ops::Range<i64>| Clip {
        id: ClipId(id),
        asset,
        source_range: TimeCode(source.start)..TimeCode(source.end),
        content: ClipContent::Media,
        timeline_start: TimeCode(at),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        audio_gain_curve: None,
        speed_percent: 100,
    };
    vec![
        Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![
                media_clip(1, AssetId(1), 10, 0..10),
                media_clip(2, AssetId(1), 20, 10..20),
                media_clip(3, AssetId(1), 40, 20..30),
            ],
        },
        Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        },
        Track {
            id: TrackId(3),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![media_clip(4, AssetId(1), 0, 0..30)],
        },
        Track {
            id: TrackId(4),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                media_clip(5, AssetId(2), 0, 0..10),
                Clip {
                    id: ClipId(6),
                    asset: AssetId::default(),
                    source_range: TimeCode(0)..TimeCode(10),
                    content: ClipContent::Title(kinewright_core::Title::default()),
                    timeline_start: TimeCode(10),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    audio_gain_curve: None,
                    speed_percent: 100,
                },
                media_clip(7, AssetId(2), 20, 0..10),
            ],
        },
        // Track 5 exercises the `unwrap_or(TimeCode::ZERO)` reading: clip 8
        // names an asset this document does not carry, so `clip_duration`
        // fails and the clip contributes **zero** frames — which is why
        // clip 9 at 10 still opens a gap. Were the failure read as the
        // clip's source length instead, `previous_end` would reach 10 and
        // the gap would vanish. `validate_document` rejects a dangling
        // asset reference, so this only ever arrives from a hand-edited
        // file, which is the traffic `qa_document` is defensive about.
        Track {
            id: TrackId(5),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![
                media_clip(8, AssetId(9), 0, 0..10),
                media_clip(9, AssetId(1), 10, 0..10),
            ],
        },
    ]
}

/// AU5 §7 item B3, first half.
///
/// Leading and interior gaps, never the trailing one; content- and
/// kind-agnostic; an empty track and a track whose only clip starts at 0
/// answer an empty list; an unknown `TrackId` answers `None` rather than
/// panicking or answering an empty list (§0 R42).
#[test]
fn au5_track_gaps_are_leading_and_interior_but_never_trailing() {
    let doc = gap_corpus();

    // Track 1 stops at frame 50 while the document runs to 90. The leading gap
    // and the interior gap are reported; the trailing 50..90 is not a hole.
    assert_eq!(
        doc.track_gaps(TrackId(1)),
        Some(vec![TimeCode(0)..TimeCode(10), TimeCode(30)..TimeCode(40),]),
        "leading and interior gaps only, never the trailing one"
    );
    assert!(
        doc.tracks[0]
            .clips
            .iter()
            .all(|clip| clip.timeline_start < TimeCode(50)),
        "the corpus track really does end before the document does"
    );
    assert_eq!(doc.duration, TimeCode(90));

    // An empty track has no gaps — not one gap spanning the document.
    assert_eq!(doc.track_gaps(TrackId(2)), Some(Vec::new()));
    // A track whose only clip starts at 0 has no leading gap.
    assert_eq!(doc.track_gaps(TrackId(3)), Some(Vec::new()));
    // Content-agnostic: the title clip at 10..20 closes the span between two
    // media clips exactly as a media clip would.
    assert_eq!(doc.track_gaps(TrackId(4)), Some(Vec::new()));
    assert!(
        doc.tracks[3]
            .clips
            .iter()
            .any(|clip| matches!(clip.content, ClipContent::Title(_))),
        "the kind-agnostic arm is only meaningful with a title in it"
    );

    // A clip whose `clip_duration` fails contributes **zero** frames, so the
    // clip behind it still opens a gap. This is `qa_document`'s long-standing
    // `unwrap_or(TimeCode::ZERO)` reading, and the golden below confirms the
    // pre-refactor walk answered the same thing on this same track.
    assert!(
        doc.clip_duration(&doc.tracks[4].clips[0]).is_err(),
        "clip 8 names an asset the document does not carry"
    );
    assert_eq!(
        doc.track_gaps(TrackId(5)),
        Some(vec![TimeCode(0)..TimeCode(10)]),
        "a clip of undecidable length closes nothing"
    );

    // §0 R42: an unknown track is distinguishable from a track with no gaps.
    assert_eq!(doc.track_gaps(TrackId(99)), None);

    // §0 R94: the accessor resolves a track by `find`, which is exact only
    // because `validate_document` rejects a duplicate `TrackId` — a document
    // carrying two tracks with one id would otherwise drive the second track's
    // QA walk from the first track's gap list.
    let ids: Vec<_> = doc.tracks.iter().map(|track| track.id).collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(ids.len(), unique.len());
    let mut duplicated = doc.clone();
    duplicated.tracks[1].id = TrackId(1);
    assert!(
        duplicated.validate().is_err(),
        "validate_document is what makes the `find` unambiguous"
    );
}

/// AU5 §7 item B3, second half: `qa_document`'s whole `QaReport` is **equal
/// before and after** the `track_gaps` refactor.
///
/// The golden below was captured from `qa_document` **before** §5.2's refactor
/// landed and is pasted here unchanged, so this is a byte pin across the
/// change rather than a restatement of what the code now does. Re-deriving it
/// from the post-refactor code would prove nothing.
#[test]
fn au5_the_qa_report_is_byte_identical_across_the_track_gaps_refactor() {
    const PRE_REFACTOR_QA_REPORT: &str = r#"{"document_duration":90,"issues":[{"severity":"error","code":"missing_media","message":"Media file is missing: au5-part-b-gap-corpus.wav","asset":1},{"severity":"warning","code":"source_color_metadata_uncertain","message":"Asset 2 (\"corpus-video\") needs source colour review: primaries are unknown, transfer is unknown, matrix is unknown, range is unknown, bit depth is unknown, provenance is unknown.","asset":2},{"severity":"error","code":"missing_media","message":"Media file is missing: au5-part-b-gap-corpus.mp4","asset":2},{"severity":"warning","code":"track_gap","message":"Track 1 has a gap from frame 0 to 10.","track":1,"clip":1,"range":{"start":0,"end":10}},{"severity":"info","code":"abrupt_cut","message":"Clip 2 starts with a hard cut.","track":1,"clip":2,"range":{"start":20,"end":21}},{"severity":"warning","code":"track_gap","message":"Track 1 has a gap from frame 30 to 40.","track":1,"clip":3,"range":{"start":30,"end":40}},{"severity":"warning","code":"track_gap","message":"Track 5 has a gap from frame 0 to 10.","track":5,"clip":9,"range":{"start":0,"end":10}}]}"#;

    let serialized = serde_json::to_string(&qa_document(&gap_corpus())).unwrap();
    assert_eq!(
        serialized, PRE_REFACTOR_QA_REPORT,
        "the track_gaps refactor changed qa_document's output"
    );

    // The two findings the refactor could plausibly move, named rather than
    // left implicit inside the golden.
    let report = qa_document(&gap_corpus());
    let gaps: Vec<_> = report
        .issues
        .iter()
        .filter(|issue| issue.code == "track_gap")
        .map(|issue| (issue.message.clone(), issue.clip, issue.range.clone()))
        .collect();
    assert_eq!(
        gaps,
        vec![
            (
                "Track 1 has a gap from frame 0 to 10.".to_owned(),
                Some(ClipId(1)),
                Some(TimeCode(0)..TimeCode(10)),
            ),
            (
                "Track 1 has a gap from frame 30 to 40.".to_owned(),
                Some(ClipId(3)),
                Some(TimeCode(30)..TimeCode(40)),
            ),
            (
                "Track 5 has a gap from frame 0 to 10.".to_owned(),
                Some(ClipId(9)),
                Some(TimeCode(0)..TimeCode(10)),
            ),
        ],
        "each gap warning still names the clip that opens it"
    );
    assert_eq!(
        report
            .issues
            .iter()
            .filter(|issue| issue.code == "abrupt_cut")
            .count(),
        1,
        "the else-arm the gap check guards is unchanged"
    );
}

// ---------------------------------------------------------------------------
// B6: `map_project_duration_to_source` (AU5 §5.3 rule 97)
// ---------------------------------------------------------------------------

/// AU5 §7 item B6, first clause: the helper round-trips **every** duration in
/// `1..=120` at 30 → 30, 30 → 25 and 30 → 24, from three different source
/// starts, and the answer is always the *smallest* exact end.
#[test]
fn au5_map_project_duration_to_source_inverts_the_forward_mapping() {
    // The first three pairs are §5.3's own: an audio-only asset probes at 30/1
    // and the project is 30, 25 or 24. The last two invert the ratio — the
    // source running 2.5x the project — because that is where the exact ends
    // form a *run* rather than a single frame, and where a search window or a
    // non-minimal answer would show up. §0 R93.
    for (source, project) in [
        ((30, 1), (30, 1)),
        ((30, 1), (25, 1)),
        ((30, 1), (24, 1)),
        ((60, 1), (24, 1)),
        ((60_000, 1_001), (24_000, 1_001)),
    ] {
        let source_fps = Rational::new(source.0, source.1).unwrap();
        let project_fps = Rational::new(project.0, project.1).unwrap();
        for start in [0_i64, 1, 3, 37] {
            let source_start = TimeCode(start);
            for frames in 1..=120_i64 {
                let duration = TimeCode(frames);
                let end =
                    map_project_duration_to_source(source_start, duration, source_fps, project_fps)
                        .unwrap_or_else(|error| {
                            panic!(
                                "{source:?} -> {project:?} start {start} duration {frames}: {error}"
                            )
                        });
                assert!(end > source_start, "a fill range is never empty");
                assert_eq!(
                    map_source_range_to_project(source_start..end, source_fps, project_fps),
                    Ok(duration),
                    "{source:?} -> {project:?} start {start} duration {frames} did not round-trip"
                );
                // The smallest exact end: one frame less is never exact.
                let shorter = TimeCode(end.0 - 1);
                assert_ne!(
                    map_source_range_to_project(source_start..shorter, source_fps, project_fps),
                    Ok(duration),
                    "{source:?} -> {project:?} start {start} duration {frames} had a shorter exact end"
                );
                // And the run's **upper** edge, which is what makes a
                // multi-member run visible here rather than only in a
                // reviewer's sweep: the exact ends number `floor(r)` or
                // `ceil(r)`, because they are the integers in a half-open
                // interval of width exactly `r` (§0 R93).
                let mut run = 0_i64;
                while map_source_range_to_project(
                    source_start..TimeCode(end.0 + run),
                    source_fps,
                    project_fps,
                ) == Ok(duration)
                {
                    run += 1;
                }
                let ratio_numerator = i64::from(source.0) * i64::from(project.1);
                let ratio_denominator = i64::from(source.1) * i64::from(project.0);
                let floor_ratio = ratio_numerator / ratio_denominator;
                let ceil_ratio = (ratio_numerator + ratio_denominator - 1) / ratio_denominator;
                assert!(
                    run == floor_ratio || run == ceil_ratio,
                    "{source:?} -> {project:?} start {start} duration {frames}: run of {run} is neither {floor_ratio} nor {ceil_ratio}"
                );
            }
        }
    }
}

/// AU5 §0 R93's counterexample, pinned.
///
/// At 60 → 24 the exact ends form a run of up to three, and before the
/// bisection landed the search window returned the wrong member of it: a
/// one-frame span from `source_start = 1` is satisfied by `1..2`, but the
/// estimate `round(1 x 2.5) = 3` put the window on `1..3`, `1..4` and `1..5`
/// and the helper answered `3`.
#[test]
fn au5_map_project_duration_to_source_answers_the_smallest_end_of_the_run() {
    let source_fps = Rational::new(60, 1).unwrap();
    let project_fps = Rational::new(24, 1).unwrap();

    // Both are exact; the smaller is the answer.
    assert_eq!(
        map_source_range_to_project(TimeCode(1)..TimeCode(2), source_fps, project_fps),
        Ok(TimeCode(1))
    );
    assert_eq!(
        map_source_range_to_project(TimeCode(1)..TimeCode(3), source_fps, project_fps),
        Ok(TimeCode(1))
    );
    assert_eq!(
        map_project_duration_to_source(TimeCode(1), TimeCode(1), source_fps, project_fps),
        Ok(TimeCode(2)),
        "the smallest exact end, not the one the estimate points at"
    );

    // §5.3 rule 98's **own** rate pairs reach the multi-member run too — this
    // is the arm that would have caught §0 R93's withdrawn "they cannot meet
    // on the AU5 path". At 30 -> 25 a three-frame gap is satisfied by both
    // `0..3` and `0..4`; at 30 -> 24 a two-frame gap by both `0..2` and
    // `0..3`. The helper answers the smaller of each, and R93 records that as
    // a decision with a measured cost, not as an accident.
    for (project, duration, smallest, largest) in
        [((25_u32, 1_u32), 3_i64, 3_i64, 4_i64), ((24, 1), 2, 2, 3)]
    {
        let asset_fps = Rational::new(30, 1).unwrap();
        let project_fps = Rational::new(project.0, project.1).unwrap();
        for end in [smallest, largest] {
            assert_eq!(
                map_source_range_to_project(TimeCode(0)..TimeCode(end), asset_fps, project_fps),
                Ok(TimeCode(duration)),
                "30 -> {project:?}: 0..{end} should fill {duration} project frames exactly"
            );
        }
        assert_eq!(
            map_source_range_to_project(TimeCode(0)..TimeCode(largest + 1), asset_fps, project_fps),
            Ok(TimeCode(duration + 1)),
            "30 -> {project:?}: the run really does end at {largest}"
        );
        assert_eq!(
            map_project_duration_to_source(TimeCode(0), TimeCode(duration), asset_fps, project_fps),
            Ok(TimeCode(smallest)),
            "30 -> {project:?}: the smallest member of the run, by decision"
        );
    }

    // The whole neighbourhood the reviewer's sweep found, not one case:
    // `source_start = 0` was always minimal, which is why the 30 fps lanes
    // never caught this.
    for start in [0_i64, 1, 3, 7, 37, 1_000] {
        for frames in 1..=24_i64 {
            let end = map_project_duration_to_source(
                TimeCode(start),
                TimeCode(frames),
                source_fps,
                project_fps,
            )
            .unwrap();
            assert_ne!(
                map_source_range_to_project(
                    TimeCode(start)..TimeCode(end.0 - 1),
                    source_fps,
                    project_fps
                ),
                Ok(TimeCode(frames)),
                "start {start} duration {frames} answered {end:?}, but {} is also exact",
                end.0 - 1
            );
        }
    }
}

/// AU5 §7 item B6, the 60 fps clause: `map_frames` rounds to nearest, so a
/// 60 fps project reading a 30 fps audio-only asset can only express
/// even-frame spans. The caller is **told**, never silently given the wrong
/// length.
#[test]
fn au5_map_project_duration_to_source_refuses_a_duration_it_cannot_express() {
    let source_fps = Rational::new(30, 1).unwrap();
    let project_fps = Rational::new(60, 1).unwrap();

    assert_eq!(
        map_project_duration_to_source(TimeCode(0), TimeCode(7), source_fps, project_fps),
        Err(TimeMappingError::InexactDuration {
            source_start: 0,
            project_duration: 7,
        }),
        "an odd project span has no exact 30 fps source range"
    );
    // Every even span is expressible, and every odd one is not — the whole
    // class §5.3 rule 98 warns about, not one example of it.
    for frames in 1..=32_i64 {
        let mapped =
            map_project_duration_to_source(TimeCode(4), TimeCode(frames), source_fps, project_fps);
        if frames % 2 == 0 {
            assert_eq!(mapped, Ok(TimeCode(4 + frames / 2)), "even span {frames}");
        } else {
            assert!(mapped.is_err(), "odd span {frames} must refuse");
        }
    }

    // The rate and sign guards answer their own errors, not `InexactDuration`.
    assert_eq!(
        map_project_duration_to_source(TimeCode(-1), TimeCode(4), source_fps, project_fps),
        Err(TimeMappingError::NegativeFrames(TimeCode(-1)))
    );
    assert_eq!(
        map_project_duration_to_source(TimeCode(0), TimeCode(-4), source_fps, project_fps),
        Err(TimeMappingError::NegativeFrames(TimeCode(-4)))
    );

    // §0 R95: a zero-frame span is refused **uniformly**, at every rate pair.
    // A project slower than its source can map a non-empty source range to
    // zero project frames — `map_source_range_to_project(4..5, 60/1, 24/1)`
    // is `Ok(0)` — so answering zero would make the result rate-dependent for
    // an input no fill can use.
    assert_eq!(
        map_source_range_to_project(
            TimeCode(4)..TimeCode(5),
            Rational::new(60, 1).unwrap(),
            Rational::new(24, 1).unwrap()
        ),
        Ok(TimeCode::ZERO),
        "the rate-dependence the uniform refusal exists to hide"
    );
    for (source, project) in [
        ((30, 1), (30, 1)),
        ((30, 1), (60, 1)),
        ((60, 1), (24, 1)),
        ((24_000, 1_001), (24, 1)),
    ] {
        assert_eq!(
            map_project_duration_to_source(
                TimeCode(4),
                TimeCode::ZERO,
                Rational::new(source.0, source.1).unwrap(),
                Rational::new(project.0, project.1).unwrap(),
            ),
            Err(TimeMappingError::InvalidRange { start: 4, end: 4 }),
            "{source:?} -> {project:?} answered a zero-frame span"
        );
    }
}

/// AU5 §7 item B6, core half of the 25 fps arm: a fill written from the
/// helper's answer at `speed_percent = 100` has the gap's **exact** duration
/// (§5.3 rule 97 and §0 R44), closes the gap, and never collides with its
/// neighbour.
#[test]
fn au5_a_fill_built_from_the_inverse_has_the_gaps_exact_duration() {
    let source_fps = Rational::new(30, 1).unwrap();
    let project_fps = Rational::new(25, 1).unwrap();
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
        fps: project_fps,
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    };
    // An audio-only asset is probed at `Rational::default()` = 30/1
    // (§5.3 rule 98), which is why this is not a 30 fps detail.
    assert_eq!(Rational::default(), source_fps);
    Operation::AddAsset {
        asset: MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from("au5-part-b-fill.wav"),
            name: "room-tone".to_owned(),
            duration: TimeCode(300),
            fps: source_fps,
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    // Clip A occupies 0..25; clip B starts at 32, leaving a seven-frame gap.
    for at in [TimeCode(0), TimeCode(32)] {
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at,
            source: TimeCode(0)..TimeCode(30),
        }
        .apply(&mut doc)
        .unwrap();
    }
    let gaps = doc.track_gaps(TrackId(1)).unwrap();
    assert_eq!(gaps, vec![TimeCode(25)..TimeCode(32)]);

    let gap = gaps[0].clone();
    let wanted = TimeCode(gap.end.0 - gap.start.0);
    let source_end =
        map_project_duration_to_source(TimeCode(0), wanted, source_fps, project_fps).unwrap();
    // §5.4 arm (ii): `map_frames(8, 30, 25) = round(40/6) = 7`.
    assert_eq!(source_end, TimeCode(8));

    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: gap.start,
        source: TimeCode(0)..source_end,
    }
    .apply(&mut doc)
    .expect("a fill of the exact length never collides with its neighbour");

    let fill = doc.tracks[0]
        .clips
        .iter()
        .find(|clip| clip.timeline_start == gap.start)
        .unwrap();
    assert_eq!(fill.speed_percent, 100, "§0 R44: the fill is never retimed");
    assert_eq!(
        doc.clip_duration(fill),
        Ok(wanted),
        "clip_duration(fill) == gap.end - gap.start"
    );
    assert_eq!(
        doc.track_gaps(TrackId(1)),
        Some(Vec::new()),
        "the gap is gone, so a re-run proposes nothing (§5.5 rule 104)"
    );
    doc.validate().expect("the filled document still validates");
    assert!(
        !qa_document(&doc)
            .issues
            .iter()
            .any(|issue| issue.code == "track_gap"),
        "the track_gap warning disappearing is the only observable fill signal"
    );
}

// ---------------------------------------------------------------------------
// R96: the covering source range (AU5 §5.3, §5.4 rule 101(ii), §0 R96)
// ---------------------------------------------------------------------------

/// `kinewright-media`'s `clock::frame_to_samples`, spelled here so the lane
/// checks core's answer against media's rule rather than against core's own
/// re-spelling of it (§0 R96).
fn media_frame_to_samples(frame: TimeCode, sample_rate: u32, fps: Rational) -> u128 {
    if frame.0 <= 0 {
        return 0;
    }
    u128::try_from(frame.0).unwrap() * u128::from(sample_rate) * u128::from(fps.denominator())
        / u128::from(fps.numerator())
}

/// AU5 §0 R96, the phase rule, over the **whole workspace rate matrix**.
///
/// A fill has to be exact in project *frames* **and** carry enough sample
/// frames to fill the gap, because the mixer maps source samples to project
/// samples one for one, plays the mapped project span and stops. Which exact
/// ends exist depends on the phase of `source_start`, so a caller hard-coding
/// `TimeCode::ZERO` ships the one fill at 30 -> 25 that does not cover.
///
/// The phase window is the **period** of that pattern — the reduced
/// denominator of `r` — not `⌈r⌉`. This lane is what tells the two apart: at
/// 29.97 the period is 1 000 while `⌈r⌉ + 1` is 3, and every one of these gap
/// lengths was refused before the window rule was corrected (§0 R96, pass-3
/// finding 1).
#[test]
fn au5_a_covering_fill_exists_at_every_gap_length_the_room_tone_planner_meets() {
    // A room-tone asset is always `Rational::default()` = 30/1, because that
    // is what the store writes. The project may be any of the nine.
    let asset_fps = Rational::default();
    assert_eq!(asset_fps, Rational::new(30, 1).unwrap());
    let sample_rate = 48_000_u32;
    let asset_duration = TimeCode(96_000);

    let mut deepest_phase = 0_i64;
    for project in WORKSPACE_RATES {
        let project_fps = Rational::new(project.0, project.1).unwrap();
        let mut covered = Vec::new();
        for frames in 1..=200_i64 {
            let gap = TimeCode(frames);
            match covering_source_range_for_project_duration(
                gap,
                asset_fps,
                project_fps,
                asset_duration,
                sample_rate,
            ) {
                Ok(range) => {
                    assert_covering(&range, gap, asset_fps, project_fps, sample_rate);
                    assert!(range.end <= asset_duration, "the fill reads past the asset");
                    deepest_phase = deepest_phase.max(range.start.0);
                    covered.push(frames);
                }
                // Two legal refusals. `NoCoveringSourceRange` is the
                // one-for-one mapping's own limit. `InexactDuration` must mean
                // what it says — so it is checked against an independent scan
                // far deeper than the helper's own window, which is the claim
                // pass-3 finding 2 caught the first version getting wrong.
                Err(TimeMappingError::NoCoveringSourceRange { .. }) => {
                    assert_eq!(
                        first_covering_range(
                            gap,
                            asset_fps,
                            project_fps,
                            asset_duration,
                            sample_rate,
                            2_600,
                        ),
                        None,
                        "30 -> {project:?} gap {frames} refused, but a phase covers it"
                    );
                }
                Err(TimeMappingError::InexactDuration { .. }) => {
                    assert!(
                        (0..2_600_i64).all(|phase| map_project_duration_to_source(
                            TimeCode(phase),
                            gap,
                            asset_fps,
                            project_fps
                        )
                        .is_err()),
                        "30 -> {project:?} gap {frames} refused as inexact, but a deeper phase expresses it"
                    );
                }
                Err(error) => {
                    panic!("30 -> {project:?} gap {frames}: {error}")
                }
            }
        }
        // The five project rates a room-tone asset can always fill.
        if matches!(project, (30 | 25 | 24, 1) | (24_000 | 30_000, 1_001)) {
            assert_eq!(
                covered.len(),
                200,
                "30 -> {project:?} left {} of 200 gap lengths unfilled",
                200 - covered.len()
            );
        }
        // 59.94 is the documented residue: one source frame supplies 1 600
        // sample frames against a demand of 1 602, and two source frames map
        // to two project frames too many, so **every even** span is
        // uncoverable at any phase and any length while every odd one fills.
        if project == (60_000, 1_001) {
            assert_eq!(
                covered,
                (1..=200_i64)
                    .filter(|frames| frames % 2 == 1)
                    .collect::<Vec<_>>(),
                "30 -> 59.94 should fill exactly the odd spans"
            );
        }
    }
    // The NTSC pulldown pairs are the deep ones, and 499 is the floor under
    // any future cap on the sweep.
    assert_eq!(
        deepest_phase, 499,
        "the deepest phase a room-tone fill needs at a workspace rate"
    );
}

/// AU5 §0 R96: the whole 9 x 9 workspace rate matrix against a **formula-free**
/// phase scan.
///
/// An NTSC *asset* is a legal fill source — `plan_room_tone_fill` takes an
/// explicit `asset_id` for any non-`Video` pool asset — and these are the pairs
/// where the window rule was wrong twice over: at 29.97 -> 29.97 the true
/// period is `lcm(map 1, sample 5) = 5`, while the `⌈r⌉ + 1` rule swept 3 and
/// the reduced-denominator rule swept 2, so 40 of 200 gap lengths were refused
/// with a covering range sitting at phase 3.
///
/// Every refusal is re-scanned rather than accepted, which is the check that
/// makes this lane able to fail (pass-4 finding 5).
#[test]
fn au5_the_covering_fill_agrees_with_an_independent_phase_scan_at_every_workspace_rate() {
    let sample_rate = 48_000_u32;
    let asset_duration = TimeCode(96_000);

    // The whole 9 x 9 matrix against the formula-free ground truth. An NTSC
    // **asset** is a legal fill source — `plan_room_tone_fill` takes an
    // explicit `asset_id` for any non-`Video` pool asset — and these are the
    // pairs where the window rule was wrong: at 29.97 -> 29.97 the period is
    // lcm(1, 5) = 5 while the withdrawn rule swept 2, so 40 of 200 gap lengths
    // were refused with a covering range sitting at phase 3.
    for source in WORKSPACE_RATES {
        let source_fps = Rational::new(source.0, source.1).unwrap();
        for project in WORKSPACE_RATES {
            let project_fps = Rational::new(project.0, project.1).unwrap();
            for frames in 1..=60_i64 {
                let gap = TimeCode(frames);
                let answer = covering_source_range_for_project_duration(
                    gap,
                    source_fps,
                    project_fps,
                    asset_duration,
                    sample_rate,
                );
                match answer {
                    Ok(range) => {
                        assert_covering(&range, gap, source_fps, project_fps, sample_rate);
                        assert!(range.end <= asset_duration);
                        // A shallow scan that finds anything must find exactly
                        // this, which pins the (phase, end) ordering too.
                        if let Some(shallow) = first_covering_range(
                            gap,
                            source_fps,
                            project_fps,
                            asset_duration,
                            sample_rate,
                            64,
                        ) {
                            assert_eq!(
                                range, shallow,
                                "{source:?} -> {project:?} gap {frames}: not the first covering range"
                            );
                        }
                    }
                    // Every refusal is re-scanned to a depth past the widest
                    // workspace period (2 500, at 59.94 -> 24), so a window
                    // short of the true period fails here rather than in the
                    // field. This is the check pass-4 finding 5 asked for and
                    // the one that catches finding 1's class of bug.
                    Err(_) => assert_eq!(
                        first_covering_range(
                            gap,
                            source_fps,
                            project_fps,
                            asset_duration,
                            sample_rate,
                            2_600,
                        ),
                        None,
                        "{source:?} -> {project:?} gap {frames} refused, but a phase covers it"
                    ),
                }
            }
        }
    }
}

/// A **formula-free** ground truth for the phase sweep: scan `source_start`
/// from 0 upward, take each phase's smallest exact end and walk its exact run,
/// and answer the first range that covers inside the asset.
///
/// This deliberately does not re-derive the helper's period. It bounds the
/// scan with a flat constant instead, so it tests the *window rule* rather
/// than agreeing with it — which is the check that catches a window short of
/// the true period (§0 R96, pass-4 finding 5).
fn first_covering_range(
    gap: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
    source_duration: TimeCode,
    sample_rate: u32,
    phases: i64,
) -> Option<std::ops::Range<TimeCode>> {
    let needed = (u128::try_from(gap.0).unwrap()
        * u128::from(sample_rate)
        * u128::from(project_fps.denominator()))
    .div_ceil(u128::from(project_fps.numerator()));
    for phase in 0..phases {
        let start = TimeCode(phase);
        let Ok(smallest) = map_project_duration_to_source(start, gap, source_fps, project_fps)
        else {
            continue;
        };
        let mut end = smallest;
        while end <= source_duration {
            if media_frame_to_samples(end, sample_rate, source_fps)
                - media_frame_to_samples(start, sample_rate, source_fps)
                >= needed
            {
                return Some(start..end);
            }
            let next = TimeCode(end.0 + 1);
            if map_source_range_to_project(start..next, source_fps, project_fps) != Ok(gap) {
                break;
            }
            end = next;
        }
    }
    None
}

/// The nine frame rates the workspace uses, as `(numerator, denominator)`.
const WORKSPACE_RATES: [(u32, u32); 9] = [
    (30, 1),
    (25, 1),
    (24, 1),
    (24_000, 1_001),
    (30_000, 1_001),
    (60_000, 1_001),
    (60, 1),
    (50, 1),
    (48, 1),
];

/// Every property a fill has to have: exact in project frames, and carrying at
/// least the gap's sample frames on media's own floor rule.
fn assert_covering(
    range: &std::ops::Range<TimeCode>,
    gap: TimeCode,
    source_fps: Rational,
    project_fps: Rational,
    sample_rate: u32,
) {
    assert!(
        range.start >= TimeCode::ZERO,
        "{range:?} starts before zero"
    );
    assert_eq!(
        map_source_range_to_project(range.clone(), source_fps, project_fps),
        Ok(gap),
        "{range:?} is not an exact fill for {gap:?}"
    );
    let supplied = media_frame_to_samples(range.end, sample_rate, source_fps)
        - media_frame_to_samples(range.start, sample_rate, source_fps);
    let needed = (u128::try_from(gap.0).unwrap()
        * u128::from(sample_rate)
        * u128::from(project_fps.denominator()))
    .div_ceil(u128::from(project_fps.numerator()));
    assert!(
        supplied >= needed,
        "{range:?} supplies {supplied} of the {needed} sample frames {gap:?} needs"
    );
}

/// AU5 §0 R96: the NTSC pulldown case, pinned by value.
///
/// 30 fps room tone into a 29.97 fps project has map period **1 001** — `r` is
/// `30030/30000 = 1001/1000`, whose reduced numerator is 1 001 — so the
/// covering phase sits near 499 and a window of `⌈r⌉ + 1 = 3` finds nothing at
/// all. Before the window rule was corrected this whole rate pair refused
/// every gap length, and a refusal is a skipped gap to the callers that build
/// the fill — the agent's `room_tone_fill_tiles` and the app's
/// `room_tone_fill_operations` behind the timeline button, both of which now
/// read this helper — so an NTSC project got no room-tone fill at all.
#[test]
fn au5_a_covering_fill_reaches_the_ntsc_pulldown_phase() {
    let asset_fps = Rational::default();
    let project_fps = Rational::new(30_000, 1_001).unwrap();
    let sample_rate = 48_000_u32;

    for (frames, start, end) in [
        (1_i64, 499_i64, 501_i64),
        (2, 498, 501),
        (3, 497, 501),
        (12, 488, 501),
    ] {
        let gap = TimeCode(frames);
        let range = covering_source_range_for_project_duration(
            gap,
            asset_fps,
            project_fps,
            TimeCode(96_000),
            sample_rate,
        )
        .unwrap_or_else(|error| panic!("29.97 gap {frames}: {error}"));
        assert_eq!(
            range,
            TimeCode(start)..TimeCode(end),
            "29.97 gap {frames} should fill from phase {start}"
        );
        assert_covering(&range, gap, asset_fps, project_fps, sample_rate);
    }

    // The trap in detail, because it is subtler than "phase 0 is inexact":
    // phase 0 **is** exact — `0..1` maps to one 29.97 project frame — it is
    // just two sample frames short of the 1 602 that frame demands, and every
    // phase up to 498 is short the same way. The old `⌈r⌉ + 1 = 3` window
    // could only ever see three of the period's 1 000 phases.
    assert_eq!(
        map_project_duration_to_source(TimeCode::ZERO, TimeCode(1), asset_fps, project_fps),
        Ok(TimeCode(1)),
        "phase 0 is exact for a one-frame 29.97 gap"
    );
    let phase_zero_supply = media_frame_to_samples(TimeCode(1), sample_rate, asset_fps);
    assert_eq!(phase_zero_supply, 1_600);
    assert_eq!(
        (u128::from(sample_rate) * 1_001).div_ceil(30_000),
        1_602,
        "and a 29.97 frame demands 1 602 sample frames"
    );
    assert!(
        (0..499_i64).all(|phase| {
            let Ok(end) = map_project_duration_to_source(
                TimeCode(phase),
                TimeCode(1),
                asset_fps,
                project_fps,
            ) else {
                return true;
            };
            media_frame_to_samples(end, sample_rate, asset_fps)
                - media_frame_to_samples(TimeCode(phase), sample_rate, asset_fps)
                < 1_602
        }),
        "no phase below 499 covers a one-frame 29.97 gap"
    );
    // 59.94 is the same period story at half the depth.
    assert_eq!(
        covering_source_range_for_project_duration(
            TimeCode(1),
            asset_fps,
            Rational::new(60_000, 1_001).unwrap(),
            TimeCode(96_000),
            sample_rate,
        ),
        Ok(TimeCode(250)..TimeCode(251))
    );
}

/// AU5 §0 R96: the two failure modes answer **different** errors, because a
/// planner writes a different per-gap reason for each.
#[test]
fn au5_the_covering_fill_separates_an_unrepresentable_gap_from_a_short_asset() {
    let asset_fps = Rational::new(30, 1).unwrap();
    let sample_rate = 48_000_u32;

    // §5.4 arm (iii): a 60 fps project cannot express an odd span from a 30 fps
    // asset **at any phase**, because `map_frames(e, 30, 60) = 2e` is always
    // even. That is rule 97's "no exact source range", not a short asset.
    assert_eq!(
        covering_source_range_for_project_duration(
            TimeCode(7),
            asset_fps,
            Rational::new(60, 1).unwrap(),
            TimeCode(96_000),
            sample_rate,
        ),
        Err(TimeMappingError::InexactDuration {
            source_start: 0,
            project_duration: 7,
        })
    );

    // A room-tone asset too short for the gap is a **different** question, and
    // the only one whose answer is "record more room tone". It is decided
    // before the sweep runs, from the supply bound alone, so the number in the
    // message is a proof rather than the summary of a failed search: 59
    // project frames at 25 fps demand 113 280 sample frames, which no source
    // range shorter than 71 frames of a 30 fps asset can carry.
    assert_eq!(
        covering_source_range_for_project_duration(
            TimeCode(59),
            asset_fps,
            Rational::new(25, 1).unwrap(),
            TimeCode(4),
            sample_rate,
        ),
        Err(TimeMappingError::SourceTooShortToCover {
            project_duration: 59,
            source_duration: 4,
            minimum_source_frames: 71,
        })
    );
    assert_eq!((59_u128 * 48_000).div_ceil(25), 113_280);
    assert_eq!((113_280_u128 * 30).div_ceil(48_000), 71);

    // And the third answer: exact ranges exist, the asset is enormous, and
    // **nothing covers at any phase or any length** — a limit of the
    // one-for-one sample mapping, not of the recording. At 30 -> 59.94 one
    // source frame supplies 1 600 sample frames against a demand of 1 602,
    // while two source frames map to two project frames too many, so every
    // even span is uncoverable. Blaming the asset here would send an editor
    // to the wrong fix.
    assert_eq!(
        covering_source_range_for_project_duration(
            TimeCode(2),
            asset_fps,
            Rational::new(60_000, 1_001).unwrap(),
            TimeCode(96_000),
            sample_rate,
        ),
        Err(TimeMappingError::NoCoveringSourceRange {
            project_duration: 2,
            source_duration: 96_000,
        })
    );

    // Degenerate inputs refuse the way R95's do.
    for bad in [TimeCode::ZERO, TimeCode(-1)] {
        assert!(
            covering_source_range_for_project_duration(
                bad,
                asset_fps,
                Rational::new(25, 1).unwrap(),
                TimeCode(96_000),
                sample_rate,
            )
            .is_err(),
            "a {bad:?}-frame gap is not a gap"
        );
        assert!(
            covering_source_range_for_project_duration(
                TimeCode(7),
                asset_fps,
                Rational::new(25, 1).unwrap(),
                bad,
                sample_rate,
            )
            .is_err(),
            "a {bad:?}-frame asset carries nothing"
        );
    }
    assert!(
        covering_source_range_for_project_duration(
            TimeCode(7),
            asset_fps,
            Rational::new(25, 1).unwrap(),
            TimeCode(96_000),
            0,
        )
        .is_err(),
        "a zero sample rate is an invalid rate"
    );
}

/// AU5 §0 R96: the two constructions the media review measured, pinned side by
/// side — and the reason the run has to be walked upward rather than stopping
/// at each phase's smallest exact end.
#[test]
fn au5_the_covering_fill_walks_the_run_before_it_moves_the_phase() {
    let asset_fps = Rational::new(30, 1).unwrap();
    let sample_rate = 48_000_u32;
    let project_fps = Rational::new(25, 1).unwrap();

    // The media review's table for a seven-frame gap at 30 -> 25: phase 0
    // admits only `0..8`, which is 640 sample frames short of the gap's
    // 13 440; phase 1 admits `1..9` (short) and `1..10` (covers); phase 2
    // admits `2..11` (covers).
    let gap_samples = 7_u128 * 1_920;
    let supplied = |range: std::ops::Range<i64>| {
        media_frame_to_samples(TimeCode(range.end), sample_rate, asset_fps)
            - media_frame_to_samples(TimeCode(range.start), sample_rate, asset_fps)
    };
    assert_eq!(gap_samples, 13_440);
    assert_eq!(supplied(0..8), 12_800, "R101's short fill");
    assert_eq!(
        supplied(1..9),
        12_800,
        "phase 1's smallest end is short too"
    );
    assert_eq!(supplied(1..10), 14_400, "phase 1's second exact end covers");
    assert_eq!(supplied(2..11), 14_400, "the review's own covering fill");
    for range in [0..8_i64, 1..9, 1..10, 2..11] {
        assert_eq!(
            map_source_range_to_project(
                TimeCode(range.start)..TimeCode(range.end),
                asset_fps,
                project_fps
            ),
            Ok(TimeCode(7)),
            "{range:?} should fill exactly seven project frames"
        );
    }

    // The helper takes the first covering range in (phase, end) order, so it
    // answers `1..10` — one phase earlier than the review's `2..11`, and the
    // same seam.
    assert_eq!(
        covering_source_range_for_project_duration(
            TimeCode(7),
            asset_fps,
            project_fps,
            TimeCode(96_000),
            sample_rate,
        ),
        Ok(TimeCode(1)..TimeCode(10))
    );

    // Why the run is walked and not just sampled once per phase. At 23.976 a
    // four-frame gap demands 8 008 sample frames, and **no** phase's smallest
    // exact end supplies them — every one lands on 8 000, short by **8**. The
    // answer is `2..8`: phase 2's run is `{7, 8}`, and its *second* member is
    // the first thing that covers. A sweep that only ever asked each phase for
    // its smallest end would refuse a gap that is perfectly fillable.
    //
    // The same holds for every `D` divisible by four **in the range this lane
    // sweeps** — up to 92, measured; from 96 up, phases 2 and 7 do cover with
    // their smallest end (pass-3 finding 4 scoped this claim, which the first
    // text stated without a bound).
    let ntsc = Rational::new(24_000, 1_001).unwrap();
    assert_eq!(4 * 2_002, 8_008, "a four-frame 23.976 gap in sample frames");
    for phase in 0..=4_i64 {
        let smallest =
            map_project_duration_to_source(TimeCode(phase), TimeCode(4), asset_fps, ntsc).unwrap();
        assert!(
            supplied(phase..smallest.0) < 8_008,
            "phase {phase}'s smallest exact end {smallest:?} should be short"
        );
    }
    assert_eq!(
        map_source_range_to_project(TimeCode(2)..TimeCode(7), asset_fps, ntsc),
        Ok(TimeCode(4)),
        "phase 2's smallest exact end is 7"
    );
    assert_eq!(supplied(2..7), 8_000, "and it is short by exactly 8 frames");
    assert_eq!(
        map_source_range_to_project(TimeCode(2)..TimeCode(8), asset_fps, ntsc),
        Ok(TimeCode(4)),
        "8 is the run's second member, still exact"
    );
    assert_eq!(supplied(2..8), 9_600, "and it covers");

    let range = covering_source_range_for_project_duration(
        TimeCode(4),
        asset_fps,
        ntsc,
        TimeCode(96_000),
        sample_rate,
    )
    .unwrap();
    assert_eq!(range, TimeCode(2)..TimeCode(8));
    let smallest_for_phase =
        map_project_duration_to_source(range.start, TimeCode(4), asset_fps, ntsc).unwrap();
    assert!(
        range.end > smallest_for_phase,
        "the answer is past its phase's smallest exact end, which is the point"
    );
}

/// One row of R97's tile table: an asset length, a project rate, and the
/// tile the search should answer with the step down it took to get there.
struct TileCase {
    asset: i64,
    project: (u32, u32),
    want: i64,
    range: (i64, i64),
    step: i64,
}

/// AU5 §0 R97: the longest tile an asset can actually cover, which is not the
/// longest it maps to.
///
/// A tiler that walks down from the asset's own mapped length by a small
/// literal — both AU5 tilers used four frames — misses the covering tile at
/// the NTSC pairs by up to 499 frames, and a tiler that finds no tile skips
/// **every** gap on the track. This lane pins the step-downs that the
/// four-frame allowance could not reach.
#[test]
fn au5_the_longest_coverable_tile_steps_past_a_four_frame_allowance() {
    let asset_fps = Rational::default();
    let sample_rate = 48_000_u32;
    let unbounded = TimeCode(1_000_000);

    let cases: [TileCase; 6] = [
        // The covering tile at 30 -> 29.97 is the map period's `0..1001`:
        // 1 001 source frames worth 1 000 project frames. An asset of 1 200
        // maps to 1 199, so reaching it costs 199 steps; 1 500 maps to 1 499
        // and costs 499. Four frames reaches neither.
        TileCase {
            asset: 1_200,
            project: (30_000, 1_001),
            want: 1_000,
            range: (0, 1_001),
            step: 199,
        },
        TileCase {
            asset: 1_500,
            project: (30_000, 1_001),
            want: 1_000,
            range: (0, 1_001),
            step: 499,
        },
        // A 600-frame asset needs no step at all, which is why the lane that
        // used one never saw any of this.
        TileCase {
            asset: 600,
            project: (30_000, 1_001),
            want: 599,
            range: (0, 600),
            step: 0,
        },
        // The boundary the review found: the first length that needs more
        // than four.
        TileCase {
            asset: 1_006,
            project: (30_000, 1_001),
            want: 1_000,
            range: (0, 1_001),
            step: 5,
        },
        // A non-NTSC pair still steps: 61 source frames map to 51 project
        // frames at 25 fps, and 50 is the longest that covers.
        TileCase {
            asset: 61,
            project: (25, 1),
            want: 50,
            range: (0, 60),
            step: 1,
        },
        // Same rate, no step, whole asset.
        TileCase {
            asset: 90,
            project: (30, 1),
            want: 90,
            range: (0, 90),
            step: 0,
        },
    ];

    for TileCase {
        asset,
        project,
        want,
        range,
        step,
    } in cases
    {
        let project_fps = Rational::new(project.0, project.1).unwrap();
        let source_duration = TimeCode(asset);
        let (tile, source) = longest_coverable_project_tile(
            source_duration,
            asset_fps,
            project_fps,
            sample_rate,
            unbounded,
        )
        .unwrap_or_else(|error| panic!("asset {asset} at 30 -> {project:?}: {error}"));

        assert_eq!(
            (tile, source.clone()),
            (TimeCode(want), TimeCode(range.0)..TimeCode(range.1)),
            "asset {asset} at 30 -> {project:?}"
        );
        assert_covering(&source, tile, asset_fps, project_fps, sample_rate);
        assert!(
            source.end <= source_duration,
            "the tile reads past the asset"
        );

        // The step down really is what the case says, measured against the
        // asset's own mapped length — this is the number the four-frame
        // allowance was compared with.
        let whole =
            map_source_range_to_project(TimeCode::ZERO..source_duration, asset_fps, project_fps)
                .unwrap();
        assert_eq!(
            whole.0 - tile.0,
            step,
            "asset {asset} at 30 -> {project:?} should step down {step}"
        );
        // And nothing between the asset's mapped length and the answer covers,
        // so the answer really is the longest.
        for skipped in (tile.0 + 1)..=whole.0 {
            assert!(
                covering_source_range_for_project_duration(
                    TimeCode(skipped),
                    asset_fps,
                    project_fps,
                    source_duration,
                    sample_rate,
                )
                .is_err(),
                "asset {asset} at 30 -> {project:?} skipped a coverable tile of {skipped}"
            );
        }
    }
}

/// AU5 §0 R97: the four-frame allowance both tilers used, reproduced as
/// insufficient so nobody reintroduces the constant — plus the cap and the
/// refusal paths.
#[test]
fn au5_the_longest_coverable_tile_caps_refuses_and_beats_the_old_constant() {
    let asset_fps = Rational::default();
    let sample_rate = 48_000_u32;
    let unbounded = TimeCode(1_000_000);
    let ntsc = Rational::new(30_000, 1_001).unwrap();

    // The four-frame allowance, reproduced as insufficient so nobody
    // reintroduces the constant: at 30 -> 29.97 a 1 200-frame asset's first
    // covering tile is 199 frames below its mapped length.
    let source_duration = TimeCode(1_200);
    let whole =
        map_source_range_to_project(TimeCode::ZERO..source_duration, asset_fps, ntsc).unwrap();
    assert_eq!(whole, TimeCode(1_199));
    for step in 0..=4_i64 {
        assert!(
            covering_source_range_for_project_duration(
                TimeCode(whole.0 - step),
                asset_fps,
                ntsc,
                source_duration,
                sample_rate,
            )
            .is_err(),
            "a four-frame allowance would have found a tile at step {step}"
        );
    }

    // `max_project_frames` is the gap the caller wants filled, so the tile
    // never overshoots it — and a short cap is answered exactly, not by the
    // asset's own length.
    assert_eq!(
        longest_coverable_project_tile(TimeCode(1_200), asset_fps, ntsc, sample_rate, TimeCode(7)),
        Ok((TimeCode(7), TimeCode(493)..TimeCode(501)))
    );

    // An asset too short to cover anything propagates the covering helper's
    // own refusal rather than inventing one, so the caller's message names the
    // real reason. 60, 300 and 500 frames at 29.97 are dead at every step.
    for asset in [60_i64, 300, 500] {
        assert!(
            matches!(
                longest_coverable_project_tile(
                    TimeCode(asset),
                    asset_fps,
                    ntsc,
                    sample_rate,
                    unbounded
                ),
                Err(TimeMappingError::NoCoveringSourceRange { .. }
                    | TimeMappingError::SourceTooShortToCover { .. })
            ),
            "a {asset}-frame asset should refuse with a mapping reason"
        );
    }

    // Degenerate inputs refuse the way R95's do.
    for bad in [TimeCode::ZERO, TimeCode(-1)] {
        assert!(
            longest_coverable_project_tile(bad, asset_fps, ntsc, sample_rate, unbounded).is_err()
        );
        assert!(
            longest_coverable_project_tile(TimeCode(600), asset_fps, ntsc, sample_rate, bad)
                .is_err()
        );
    }
}
