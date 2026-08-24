//! CC1 managed SDR conformance contracts.
//!
//! These tests hold the document-level behaviour that the CC1 spec makes
//! normative: legacy colour semantics are reported and never silently
//! translated, pre-CC0/CC0 projects migrate exactly as §4 describes, and every
//! §2.1 source failure is reported with a field, an observed value, and the
//! allowed values.

use std::{collections::BTreeMap, path::PathBuf};

use kinewright_core::{
    AssetId, Clip, ClipContent, ClipId, ColorBitDepth, ColorContext, ColorDescription, ColorMatrix,
    ColorPipelineState, ColorPrimaries, ColorProvenance, ColorRange, ColorSourceError,
    ColorTransfer, ColorWhitePoint, DeliveryProfile, Document, Effect, EffectId, MediaAsset,
    MediaKind, Operation, ParamValue, QaSeverity, Rational, TimeCode, Track, TrackId, TrackKind,
    classify_source, delivery_conformance, qa_document,
};

/// A path that always exists so `missing_media` never masks a colour issue.
fn present_media_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

fn supported_source_description() -> ColorDescription {
    ColorContext::sdr_rec709().delivery
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

#[test]
fn legacy_brightness_is_reported_by_qa_and_delivery_without_silent_translation() {
    let mut document = managed_document();
    Operation::AddEffect {
        clip: ClipId(1),
        effect: Effect {
            id: EffectId(7),
            name: "brightness".to_owned(),
            parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(35))]),
            keyframes: BTreeMap::new(),
        },
    }
    .apply(&mut document)
    .expect("a legacy display-coded effect must remain loadable and editable");

    let qa = qa_document(&document);
    let qa_issue = qa
        .issues
        .iter()
        .find(|issue| issue.code == "legacy_colour_semantics")
        .expect("qa_document must report legacy colour semantics");
    assert_eq!(qa_issue.severity, QaSeverity::Warning);
    assert_eq!(qa_issue.clip, Some(ClipId(1)));
    assert!(qa_issue.message.contains("brightness"));
    assert!(
        qa.export_ready(),
        "CC1 §4.4 keeps legacy colour semantics reportable but non-blocking"
    );

    let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
        .expect("delivery conformance must produce a report");
    let delivery_issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "legacy_colour_semantics")
        .expect("delivery conformance must report legacy colour semantics");
    assert_eq!(delivery_issue.severity, QaSeverity::Warning);
    assert!(
        report.export_ready(),
        "legacy colour semantics must not block export: {:?}",
        report
            .issues
            .iter()
            .filter(|issue| issue.severity == QaSeverity::Error)
            .collect::<Vec<_>>()
    );

    // CC1 §4.4: no silent translation to the managed primary controls.
    let saved = serde_json::to_string(&document).expect("document should save");
    let reopened: Document = serde_json::from_str(&saved).expect("document should reopen");
    let before = &document.clip(ClipId(1)).expect("clip").effects;
    let after = &reopened.clip(ClipId(1)).expect("reopened clip").effects;

    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name, "brightness");
    assert_eq!(after[0].parameters["percent"], ParamValue::Integer(35));
    assert_eq!(
        serde_json::to_string(before).expect("effects should serialize"),
        serde_json::to_string(after).expect("reopened effects should serialize"),
        "legacy effect name and parameters must survive save/reopen byte-identically"
    );
    assert!(
        after
            .iter()
            .all(|effect| effect.name != "primary_correction"),
        "a legacy effect must never be rewritten into a managed primary"
    );
}

#[test]
fn pre_cc0_project_without_color_context_receives_the_managed_defaults() {
    let document: Document =
        serde_json::from_str(include_str!("fixtures/pre_m13_project.json")).unwrap();

    // CC1 §4.1: a pre-CC0 project with no `color_context` receives the CC0
    // explicit unknown source descriptions plus the current SDR Rec.709
    // monitor/delivery defaults. Combined with §4.2 (the CC0 working
    // placeholder becomes the managed linear/float16 working description) every
    // stage of the absent context is exactly the managed target, so the
    // resulting context is `managed_sdr_v1`. "Absent means legacy" governs an
    // absent `pipeline_state` inside a *present*, project-custom context; it
    // does not strand a project that has no stored context at all.
    assert_eq!(
        document.color_context.pipeline_state,
        ColorPipelineState::ManagedSdrV1
    );
    assert_eq!(document.color_context, ColorContext::sdr_rec709());
    assert!(document.color_context.is_managed_sdr_compatible());

    let working = &document.color_context.working;
    assert_eq!(working.primaries, ColorPrimaries::Bt709);
    assert_eq!(working.transfer, ColorTransfer::Linear);
    assert_eq!(working.matrix, ColorMatrix::Rgb);
    assert_eq!(working.range, ColorRange::Full);
    assert_eq!(working.white_point, ColorWhitePoint::D65);
    assert_eq!(working.bit_depth, ColorBitDepth::Float16);

    // §4.1 again: sources are explicitly unknown, never assumed Rec.709.
    assert_eq!(document.media_pool.len(), 1);
    assert_eq!(
        document.media_pool[0].color_description,
        ColorDescription::unknown()
    );
    assert!(classify_source(&document.media_pool[0].color_description).is_err());
}

#[test]
fn cc0_document_with_old_working_placeholder_migrates_to_the_managed_working_target() {
    // The CC0 working placeholder: BT.709 primaries/transfer, rgb matrix, full
    // range, 8-bit, application default, with no `pipeline_state` field.
    let cc0_working = serde_json::json!({
        "primaries": "bt709",
        "transfer": "bt709",
        "matrix": "rgb",
        "range": "full",
        "white_point": "d65",
        "bit_depth": 8,
        "confidence_basis_points": 10_000,
        "provenance": "application_default"
    });
    let mut saved = serde_json::to_value(managed_document()).expect("document should serialize");
    saved["color_context"] = serde_json::json!({
        "working": cc0_working,
        "monitoring": cc0_working,
        "delivery": {
            "primaries": "bt709",
            "transfer": "bt709",
            "matrix": "bt709",
            "range": "limited",
            "white_point": "d65",
            "bit_depth": 8,
            "confidence_basis_points": 10_000,
            "provenance": "application_default"
        }
    });

    let document: Document =
        serde_json::from_value(saved).expect("a CC0 document should remain readable");

    // CC1 §4.2: the placeholder was never an executed transform, so it becomes
    // the fixed linear `Rgba16Float` working description.
    assert_eq!(
        document.color_context.working,
        ColorContext::sdr_rec709().working
    );
    assert_eq!(
        document.color_context.working.bit_depth,
        ColorBitDepth::Float16
    );
    assert_eq!(
        document.color_context.pipeline_state,
        ColorPipelineState::ManagedSdrV1
    );
    assert_eq!(document.color_context, ColorContext::sdr_rec709());
}

#[test]
fn section_2_1_source_failures_report_field_observed_and_allowed_values() {
    let supported = supported_source_description();
    let cases = [
        (
            ColorDescription {
                primaries: ColorPrimaries::DciP3,
                ..supported.clone()
            },
            "unsupported_source_primaries",
            "primaries",
            "DciP3",
        ),
        (
            ColorDescription {
                primaries: ColorPrimaries::DisplayP3,
                ..supported.clone()
            },
            "unsupported_source_primaries",
            "primaries",
            "DisplayP3",
        ),
        (
            ColorDescription {
                transfer: ColorTransfer::Log3G10,
                ..supported.clone()
            },
            "unsupported_source_transfer",
            "transfer",
            "Log3G10",
        ),
        (
            ColorDescription {
                bit_depth: ColorBitDepth::Integer(17),
                ..supported.clone()
            },
            "unsupported_source_bit_depth",
            "bit_depth",
            "Integer(17)",
        ),
    ];

    for (description, code, field, observed) in cases {
        let error = classify_source(&description)
            .expect_err("CC1 §2.1 must reject this source description");

        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
        assert_eq!(error.observed(), observed);
        assert!(!error.field().is_empty());
        assert!(!error.observed().is_empty());
        assert!(!error.allowed_values().is_empty());
        let message = error.actionable_message();
        assert!(message.contains(&format!("field={field}")));
        assert!(message.contains(&format!("observed={observed}")));
        assert!(message.contains(error.allowed_values()));
    }

    assert!(
        classify_source(&ColorDescription {
            bit_depth: ColorBitDepth::Integer(16),
            ..supported
        })
        .is_ok(),
        "integer depth 8..=16 stays supported"
    );
}

#[test]
fn user_override_that_is_still_unsupported_keeps_blocking_managed_delivery() {
    let mut document = managed_document();
    let baseline = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
        .expect("baseline conformance");
    assert!(baseline.export_ready());

    let unsupported_override = ColorDescription {
        primaries: ColorPrimaries::Bt2020,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt2020Ncl,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Ten,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    Operation::SetAssetColorDescription {
        asset: AssetId(1),
        color_description: unsupported_override.clone(),
    }
    .apply(&mut document)
    .expect("an explicit override must be an ordinary, journaled edit");
    assert_eq!(
        document.asset(AssetId(1)).expect("asset").color_description,
        unsupported_override,
        "the override is stored verbatim, not normalised"
    );

    let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
        .expect("an unsupported override must be reported, not returned as an error");
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "unsupported_source_color")
        .expect("an explicit but unsupported override must still block delivery");

    assert_eq!(issue.severity, QaSeverity::Error);
    assert_eq!(issue.asset, Some(AssetId(1)));
    assert!(issue.message.contains("code=unsupported_source_primaries"));
    assert!(issue.message.contains("field=primaries"));
    assert!(issue.message.contains("observed=Bt2020"));
    let expected = classify_source(&unsupported_override)
        .expect_err("BT.2020 must remain unsupported")
        .allowed_values()
        .to_owned();
    assert!(issue.message.contains(&format!("allowed={expected}")));
    assert!(
        !report.export_ready(),
        "a high-confidence user override does not make an unsupported tuple supported"
    );
}

/// CC1 §2.1 requires every source failure to name its field, the observed
/// value, and the allowed values.
///
/// The classifier short-circuits on the first bad field, so a fully unknown
/// description only ever reports `UnknownPrimaries`. Each field therefore gets
/// its own description that is supported everywhere except the one field under
/// test, which is the only way to reach the other variants at all.
#[test]
fn source_error_field_observed_and_allowed_cover_every_unknown_field() {
    let unknown_by_field: [(&str, ColorSourceError, ColorDescription); 6] = [
        ("primaries", ColorSourceError::UnknownPrimaries, {
            let mut description = supported_source_description();
            description.primaries = ColorPrimaries::Unknown;
            description
        }),
        ("transfer", ColorSourceError::UnknownTransfer, {
            let mut description = supported_source_description();
            description.transfer = ColorTransfer::Unknown;
            description
        }),
        ("matrix", ColorSourceError::UnknownMatrix, {
            let mut description = supported_source_description();
            description.matrix = ColorMatrix::Unknown;
            description
        }),
        ("range", ColorSourceError::UnknownRange, {
            let mut description = supported_source_description();
            description.range = ColorRange::Unknown;
            description
        }),
        ("white_point", ColorSourceError::UnknownWhitePoint, {
            let mut description = supported_source_description();
            description.white_point = ColorWhitePoint::Unknown;
            description
        }),
        ("bit_depth", ColorSourceError::UnknownBitDepth, {
            let mut description = supported_source_description();
            description.bit_depth = ColorBitDepth::Unknown;
            description
        }),
    ];

    classify_source(&supported_source_description())
        .expect("the baseline description must be supported so each case isolates one field");

    for (field, expected, description) in unknown_by_field {
        let error = classify_source(&description)
            .expect_err("an unknown field must be classified as a source error");
        assert_eq!(error, expected, "unknown {field}");
        assert_eq!(error.field(), field);
        assert_eq!(error.observed(), "unknown");
        assert!(!error.code().is_empty(), "unknown {field} code");
        assert!(
            !error.allowed_values().is_empty(),
            "unknown {field} allowed values"
        );
        assert!(
            !error.recovery_action().is_empty(),
            "unknown {field} recovery action"
        );
    }

    // The short-circuit itself is part of the contract: a wholly unknown
    // description reports the first field only.
    let error =
        classify_source(&ColorDescription::unknown()).expect_err("unknown metadata is not Rec.709");
    assert_eq!(error, ColorSourceError::UnknownPrimaries);
}
