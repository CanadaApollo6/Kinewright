use kinewright_core::{
    COLOR_CONFIDENCE_MAX_BASIS_POINTS, ColorBitDepth, ColorContext, ColorDescription, ColorMatrix,
    ColorPipelineState, ColorPrimaries, ColorProvenance, ColorRange, ColorSourceError,
    ColorSourceProfileAssumption, ColorTransfer, ColorWhitePoint, MediaAsset, MediaKind, Operation,
    classify_source, classify_source_with_assumption,
};

pub(crate) const ASSUME_SDR_REC709_TOOLTIP: &str = "This changes metadata only; it does not apply a pixel transform. Ctrl+Z restores the prior probed description.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceColorDisplay {
    pub(crate) summary: String,
    pub(crate) warning: bool,
    pub(crate) blocking: bool,
}

#[must_use]
pub(crate) fn source_color_display(asset: &MediaAsset) -> Option<SourceColorDisplay> {
    if asset.kind == MediaKind::Audio {
        return None;
    }
    let base = format!(
        "SOURCE COLOR · {}",
        color_description_summary(&asset.color_description)
    );
    let (summary, warning, blocking) = match classify_source(&asset.color_description) {
        Ok(_) => (
            base,
            matches!(
                asset.color_description.provenance,
                ColorProvenance::Unknown | ColorProvenance::Inferred
            ),
            false,
        ),
        Err(ColorSourceError::UnknownWhitePoint) => match classify_source_with_assumption(
            &asset.color_description,
            Some(ColorSourceProfileAssumption::D65),
        ) {
            Ok(_) => (
                format!(
                    "{base} · ASSUMPTION source_color_profile_assumption: raw white_point=unknown → D65"
                ),
                true,
                false,
            ),
            Err(error) => blocked_source_display(&base, &error),
        },
        Err(error) => blocked_source_display(&base, &error),
    };
    Some(SourceColorDisplay {
        summary,
        warning,
        blocking,
    })
}

fn blocked_source_display(base: &str, error: &ColorSourceError) -> (String, bool, bool) {
    (
        format!(
            "{base} · BLOCKED code={} {}",
            error.code(),
            error.actionable_message()
        ),
        true,
        true,
    )
}

#[must_use]
pub(crate) fn color_description_summary(description: &ColorDescription) -> String {
    format!(
        "P:{} T:{} M:{} R:{} W:{} D:{} · Prov:{} C:{}",
        color_primaries_label(&description.primaries),
        color_transfer_label(&description.transfer),
        color_matrix_label(&description.matrix),
        color_range_label(&description.range),
        color_white_point_label(&description.white_point),
        color_bit_depth_label(&description.bit_depth),
        color_provenance_label(&description.provenance),
        color_confidence_label(description.confidence_basis_points),
    )
}

#[must_use]
pub(crate) fn color_pipeline_summary(context: &ColorContext) -> [String; 4] {
    [
        format!("WORKING · {}", color_description_summary(&context.working)),
        format!(
            "MONITORING · {}",
            color_description_summary(&context.monitoring)
        ),
        format!(
            "DELIVERY · {}",
            color_description_summary(&context.delivery)
        ),
        format!(
            "PIPELINE · {}",
            color_pipeline_state_label(&context.pipeline_state)
        ),
    ]
}

#[must_use]
pub(crate) fn managed_sdr_reset_needed(context: &ColorContext) -> bool {
    !context.is_managed_sdr_compatible()
}

fn color_pipeline_state_label(state: &ColorPipelineState) -> String {
    match state {
        ColorPipelineState::Legacy => "LEGACY".to_owned(),
        ColorPipelineState::ManagedSdrV1 => "MANAGED SDR V1".to_owned(),
        ColorPipelineState::Other(value) => format!("FUTURE ({value})"),
    }
}

#[must_use]
pub(crate) fn assume_sdr_rec709_operation(asset: &MediaAsset) -> Operation {
    let range = match &asset.color_description.range {
        ColorRange::Unknown => ColorRange::Limited,
        known => known.clone(),
    };
    let bit_depth = match &asset.color_description.bit_depth {
        ColorBitDepth::Unknown => ColorBitDepth::Eight,
        known => known.clone(),
    };
    Operation::SetAssetColorDescription {
        asset: asset.id,
        color_description: ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range,
            white_point: ColorWhitePoint::D65,
            bit_depth,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::UserOverride,
        },
    }
}

fn color_primaries_label(value: &ColorPrimaries) -> String {
    match value {
        ColorPrimaries::Unknown => "unknown".to_owned(),
        ColorPrimaries::Srgb => "sRGB".to_owned(),
        ColorPrimaries::Bt709 => "BT.709".to_owned(),
        ColorPrimaries::Bt2020 => "BT.2020".to_owned(),
        ColorPrimaries::DisplayP3 => "Display P3".to_owned(),
        ColorPrimaries::DciP3 => "DCI P3".to_owned(),
        ColorPrimaries::Smpte170M => "SMPTE 170M".to_owned(),
        ColorPrimaries::Smpte240M => "SMPTE 240M".to_owned(),
        ColorPrimaries::Bt470M => "BT.470M".to_owned(),
        ColorPrimaries::Bt470Bg => "BT.470BG".to_owned(),
        ColorPrimaries::Film => "film".to_owned(),
        ColorPrimaries::Other(value) => value.clone(),
    }
}

fn color_transfer_label(value: &ColorTransfer) -> String {
    match value {
        ColorTransfer::Unknown => "unknown".to_owned(),
        ColorTransfer::Srgb => "sRGB".to_owned(),
        ColorTransfer::Bt709 => "BT.709".to_owned(),
        ColorTransfer::Bt1886 => "BT.1886".to_owned(),
        ColorTransfer::Linear => "linear".to_owned(),
        ColorTransfer::Gamma22 => "gamma 2.2".to_owned(),
        ColorTransfer::Gamma28 => "gamma 2.8".to_owned(),
        ColorTransfer::Smpte170M => "SMPTE 170M".to_owned(),
        ColorTransfer::Smpte2084 => "ST.2084".to_owned(),
        ColorTransfer::AribStdB67 => "HLG".to_owned(),
        ColorTransfer::Log => "log".to_owned(),
        ColorTransfer::LogC => "Log C".to_owned(),
        ColorTransfer::Log3G10 => "Log3G10".to_owned(),
        ColorTransfer::Other(value) => value.clone(),
    }
}

fn color_matrix_label(value: &ColorMatrix) -> String {
    match value {
        ColorMatrix::Unknown => "unknown".to_owned(),
        ColorMatrix::Identity => "identity".to_owned(),
        ColorMatrix::Rgb => "RGB".to_owned(),
        ColorMatrix::Bt709 => "BT.709".to_owned(),
        ColorMatrix::Bt2020Ncl => "BT.2020-NCL".to_owned(),
        ColorMatrix::Bt2020Cl => "BT.2020-CL".to_owned(),
        ColorMatrix::Smpte170M => "SMPTE 170M".to_owned(),
        ColorMatrix::Smpte240M => "SMPTE 240M".to_owned(),
        ColorMatrix::Ycgco => "YCgCo".to_owned(),
        ColorMatrix::ChromaDerivedNcl => "chroma-derived NCL".to_owned(),
        ColorMatrix::ChromaDerivedCl => "chroma-derived CL".to_owned(),
        ColorMatrix::Ictcp => "ICtCp".to_owned(),
        ColorMatrix::Other(value) => value.clone(),
    }
}

fn color_range_label(value: &ColorRange) -> String {
    match value {
        ColorRange::Unknown => "unknown".to_owned(),
        ColorRange::Full => "full".to_owned(),
        ColorRange::Limited => "limited".to_owned(),
        ColorRange::Other(value) => value.clone(),
    }
}

fn color_white_point_label(value: &ColorWhitePoint) -> String {
    match value {
        ColorWhitePoint::Unknown => "unknown".to_owned(),
        ColorWhitePoint::D50 => "D50".to_owned(),
        ColorWhitePoint::D55 => "D55".to_owned(),
        ColorWhitePoint::D60 => "D60".to_owned(),
        ColorWhitePoint::D65 => "D65".to_owned(),
        ColorWhitePoint::Dci => "DCI".to_owned(),
        ColorWhitePoint::Other(value) => value.clone(),
    }
}

fn color_bit_depth_label(value: &ColorBitDepth) -> String {
    match value {
        ColorBitDepth::Unknown => "unknown".to_owned(),
        ColorBitDepth::Eight => "8-bit".to_owned(),
        ColorBitDepth::Ten => "10-bit".to_owned(),
        ColorBitDepth::Twelve => "12-bit".to_owned(),
        ColorBitDepth::Sixteen => "16-bit".to_owned(),
        ColorBitDepth::Float16 => "float16".to_owned(),
        ColorBitDepth::Float32 => "float32".to_owned(),
        ColorBitDepth::Integer(bits) => format!("{bits}-bit"),
        ColorBitDepth::Other(value) => value.clone(),
    }
}

fn color_provenance_label(value: &ColorProvenance) -> String {
    match value {
        ColorProvenance::Unknown => "unknown".to_owned(),
        ColorProvenance::ContainerMetadata => "container".to_owned(),
        ColorProvenance::StreamMetadata => "stream".to_owned(),
        ColorProvenance::SidecarMetadata => "sidecar".to_owned(),
        ColorProvenance::UserOverride => "user override".to_owned(),
        ColorProvenance::Inferred => "inferred".to_owned(),
        ColorProvenance::ApplicationDefault => "app default".to_owned(),
        ColorProvenance::Other(value) => value.clone(),
    }
}

fn color_confidence_label(confidence_basis_points: u16) -> String {
    let percentage = confidence_basis_points / 100;
    let hundredths = confidence_basis_points % 100;
    format!("{percentage}.{hundredths:02}%")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{AssetId, MediaKind, Rational, TimeCode};

    use super::*;

    fn asset(kind: MediaKind, color_description: ColorDescription) -> MediaAsset {
        MediaAsset {
            id: AssetId(7),
            path: PathBuf::from("fixture.mov"),
            name: "fixture.mov".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind,
            resolution: (kind != MediaKind::Audio).then_some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description,
        }
    }

    #[test]
    fn audio_only_assets_hide_source_color() {
        assert_eq!(
            source_color_display(&asset(MediaKind::Audio, ColorDescription::unknown())),
            None
        );
        assert!(
            source_color_display(&asset(MediaKind::AudioVideo, ColorDescription::unknown()))
                .is_some()
        );
    }

    #[test]
    fn unknown_source_color_is_explicit_and_warns() {
        let display =
            source_color_display(&asset(MediaKind::Video, ColorDescription::unknown())).unwrap();
        assert!(display.warning);
        assert!(display.blocking);
        assert!(display.summary.contains("BLOCKED"));
        assert!(display.summary.contains("code=unknown_source_primaries"));
    }

    #[test]
    fn partial_source_color_preserves_each_unknown_field_and_warns() {
        let description = ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            range: ColorRange::Full,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: 8_750,
            provenance: ColorProvenance::StreamMetadata,
            ..ColorDescription::unknown()
        };
        let display = source_color_display(&asset(MediaKind::Video, description)).unwrap();
        assert!(display.warning);
        assert!(display.blocking);
        assert!(display.summary.contains("P:BT.2020"));
        assert!(
            display
                .summary
                .contains("code=unsupported_source_primaries")
        );
    }

    #[test]
    fn supported_bt709_with_unknown_white_point_shows_explicit_d65_assumption() {
        let description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 9_000,
            provenance: ColorProvenance::StreamMetadata,
        };
        let display = source_color_display(&asset(MediaKind::Video, description)).unwrap();
        assert!(display.warning);
        assert!(!display.blocking);
        assert!(
            display
                .summary
                .contains("ASSUMPTION source_color_profile_assumption")
        );
        assert!(display.summary.contains("W:unknown"));
    }

    /// A completely specified description that still matches no profile blocks.
    ///
    /// This used to use the full `bt2020` / `smpte2084` / `bt2020_ncl` /
    /// `limited` / `d65` / 10-bit tuple, which CC8 §2.1 now makes the
    /// `pq_rec2020` profile — see
    /// `a_supported_hdr_source_is_not_blocking_and_names_its_profile` for that
    /// tuple's behaviour. The tuple below keeps this test's subject by dropping
    /// the depth below §2.1's 10-bit floor, which §2.1 makes "a typed rejection
    /// naming the depth, not a warning".
    #[test]
    fn fully_known_unsupported_source_is_blocking() {
        let description = ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            transfer: ColorTransfer::Smpte2084,
            matrix: ColorMatrix::Bt2020Ncl,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::StreamMetadata,
        };
        let display = source_color_display(&asset(MediaKind::Video, description)).unwrap();
        assert!(display.warning);
        assert!(display.blocking);
        assert!(display.summary.contains("BLOCKED"));
        assert!(
            display
                .summary
                .contains("code=unsupported_hdr_source_bit_depth"),
            "{}",
            display.summary
        );
        assert!(
            display.summary.contains("observed=Eight"),
            "{}",
            display.summary
        );
        assert!(
            display
                .summary
                .contains("Apply an explicit supported source-colour override"),
            "{}",
            display.summary
        );
    }

    /// CC8 §2.1: the same tuple at 10 bits is the `pq_rec2020` profile, and the
    /// inspector stops calling it an error.
    #[test]
    fn a_supported_hdr_source_is_not_blocking_and_names_its_profile() {
        let description = ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            transfer: ColorTransfer::Smpte2084,
            matrix: ColorMatrix::Bt2020Ncl,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::StreamMetadata,
        };
        assert_eq!(
            kinewright_core::classify_source(&description),
            Ok(kinewright_core::ColorSourceProfile::PqRec2020)
        );
        let display = source_color_display(&asset(MediaKind::Video, description)).unwrap();
        assert!(!display.blocking, "{}", display.summary);
        assert!(!display.warning, "{}", display.summary);
        assert!(display.summary.contains("P:BT.2020"), "{}", display.summary);
        assert!(display.summary.contains("T:ST.2084"), "{}", display.summary);
    }

    #[test]
    fn unknown_white_point_does_not_hide_an_unsupported_source_combination() {
        let description = ColorDescription {
            primaries: ColorPrimaries::Srgb,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Rgb,
            range: ColorRange::Full,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::StreamMetadata,
        };
        let display = source_color_display(&asset(MediaKind::Video, description)).unwrap();
        assert!(display.blocking);
        assert!(
            display
                .summary
                .contains("code=unsupported_source_combination")
        );
    }

    #[test]
    fn inferred_provenance_warns_even_when_essential_fields_are_tagged() {
        let description = ColorDescription {
            provenance: ColorProvenance::Inferred,
            ..ColorContext::sdr_rec709().delivery
        };
        assert!(
            source_color_display(&asset(MediaKind::Video, description))
                .unwrap()
                .warning
        );
    }

    #[test]
    fn user_override_color_format_exposes_provenance_and_confidence() {
        let description = ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::UserOverride,
        };
        assert_eq!(
            color_description_summary(&description),
            "P:BT.709 T:BT.709 M:BT.709 R:limited W:D65 D:8-bit · Prov:user override C:100.00%"
        );
    }

    #[test]
    fn rec709_override_operation_preserves_known_range_and_depth_or_defaults_them() {
        let known_source = ColorDescription {
            range: ColorRange::Full,
            bit_depth: ColorBitDepth::Ten,
            ..ColorDescription::unknown()
        };
        let defaulted_source = ColorDescription::unknown();

        for (source, expected_range, expected_depth) in [
            (known_source, ColorRange::Full, ColorBitDepth::Ten),
            (defaulted_source, ColorRange::Limited, ColorBitDepth::Eight),
        ] {
            assert_eq!(
                assume_sdr_rec709_operation(&asset(MediaKind::Video, source)),
                Operation::SetAssetColorDescription {
                    asset: AssetId(7),
                    color_description: ColorDescription {
                        primaries: ColorPrimaries::Bt709,
                        transfer: ColorTransfer::Bt709,
                        matrix: ColorMatrix::Bt709,
                        range: expected_range,
                        white_point: ColorWhitePoint::D65,
                        bit_depth: expected_depth,
                        confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
                        provenance: ColorProvenance::UserOverride,
                    },
                }
            );
        }
    }

    #[test]
    fn project_pipeline_summary_exposes_working_monitoring_and_delivery() {
        let summaries = color_pipeline_summary(&ColorContext::sdr_rec709());
        assert!(summaries[0].starts_with("WORKING · "));
        assert!(summaries[0].contains("M:RGB R:full"));
        assert!(summaries[1].starts_with("MONITORING · "));
        assert!(summaries[1].contains("M:RGB R:full"));
        assert!(summaries[2].starts_with("DELIVERY · "));
        assert!(summaries[2].contains("M:BT.709 R:limited"));
        assert_eq!(summaries[3], "PIPELINE · MANAGED SDR V1");
    }

    #[test]
    fn pipeline_summary_and_reset_state_distinguish_legacy_and_future_contexts() {
        let mut legacy = ColorContext::sdr_rec709();
        legacy.pipeline_state = ColorPipelineState::Legacy;
        assert_eq!(color_pipeline_summary(&legacy)[3], "PIPELINE · LEGACY");
        assert!(managed_sdr_reset_needed(&legacy));

        let mut future = ColorContext::sdr_rec709();
        future.pipeline_state = ColorPipelineState::Other("managed_sdr_v2".to_owned());
        assert_eq!(
            color_pipeline_summary(&future)[3],
            "PIPELINE · FUTURE (managed_sdr_v2)"
        );
        assert!(managed_sdr_reset_needed(&future));
        assert!(!managed_sdr_reset_needed(&ColorContext::sdr_rec709()));

        let mut incompatible_delivery = ColorContext::sdr_rec709();
        incompatible_delivery.delivery.transfer = ColorTransfer::Smpte2084;
        assert!(managed_sdr_reset_needed(&incompatible_delivery));
    }
}
