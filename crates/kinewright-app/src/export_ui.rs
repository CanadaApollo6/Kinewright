use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use kinewright_core::{
    CaptionCue, ColorDescription, DELIVERY_VERIFICATION_FRAME_COUNT, DeliveryAspect,
    DeliveryBudgets, DeliveryConformanceReport, DeliveryEncodeDepth, DeliveryLane, DeliveryProfile,
    DeliveryVariant, DeliveryVariantError, DeliveryVerification, DeliveryVerificationRequest,
    Document, ExportCancellation, ExportLutPreflightReport, ExportMediaPreflightReport,
    ExportProgress, ExportSettings, LutAsset, LutAssetSource, LutAvailabilityKind,
    LutAvailabilityStatus, MediaError, Operation, QaIssue, QaSeverity, Rational, TimeCode,
    delivery_conformance, document_for_delivery_variant, export_lut_preflight_with,
    export_media_preflight, srt, vtt,
};
use kinewright_media::{BuiltinLook, LutStore, LutStoreError, LutStoreErrorCode};

use crate::{
    app::KinewrightApp,
    color_ui::{color_pipeline_summary, managed_sdr_reset_needed},
    icons::Icon,
    theme::{self, color, size, space},
};

/// Advisory findings shown before the list is summarized.
///
/// The window is fixed-size; beyond roughly this many lines the export controls
/// stop being reachable without scrolling past the reason to read them.
const MAX_ADVISORY_LINES: usize = 6;

/// Height budget for the scrollable dialog body.
const EXPORT_DIALOG_MAX_BODY_HEIGHT: f32 = 420.0;

pub(crate) struct ExportDialog {
    pub(crate) open: bool,
    pub(crate) output: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps_numerator: u32,
    pub(crate) fps_denominator: u32,
    pub(crate) delivery_aspect: Option<DeliveryAspect>,
    pub(crate) focus_x_percent: u8,
    pub(crate) focus_y_percent: u8,
    /// Last conformance result and the exact inputs that produced it.
    ///
    /// The gate clones the document, materializes the delivery reframe, and
    /// stats every source file. That is far too much to redo on every
    /// immediate-mode repaint of an open dialog, and none of it can change
    /// unless one of the keyed inputs does.
    pub(crate) conformance_cache: Option<(ConformanceKey, Result<ExportConformance, String>)>,
    /// The delivery lane the next export encodes (CC6 §4.1/§8.4).
    ///
    /// A **job** parameter, not a document edit: the project keeps declaring
    /// its own 8-bit delivery contract and only
    /// `ExportSettings.delivery_color.bit_depth` moves.
    pub(crate) delivery_bit_depth: DeliveryEncodeDepth,
    /// The last finished export's verification (CC6 §6, §8.4).
    ///
    /// A measurement of a file that already exists. Whatever it says, the
    /// encode succeeded and the file is where the operator asked for it: a
    /// verification never blocks, moves, renames, or alters an export.
    pub(crate) verification: Option<ExportVerification>,
}

/// The outcome of the post-export verification pass (CC6 §6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportVerification {
    /// The file was decoded, re-probed, and compared.
    Measured(Box<DeliveryVerification>),
    /// Verification could not run at all, with the reason. It never invents a
    /// pass and never attributes the fact to a later measurement.
    Unavailable(String),
}

/// Everything `export_conformance` reads, plus the raster the gate is claiming
/// to have validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConformanceKey {
    revision: u64,
    aspect: Option<DeliveryAspect>,
    focus_x_percent: u8,
    focus_y_percent: u8,
    width: u32,
    height: u32,
    /// CC6 §0: the conformance carries the depth, so a cached 8-bit report
    /// cannot be served for a 10-bit export. `delivery_conformance` validates
    /// `settings.delivery_color`, whose `bit_depth` this selects, so the two
    /// lanes genuinely produce different reports.
    delivery_bit_depth: DeliveryEncodeDepth,
}

/// One finished export: where it went, whether it encoded, and — only when it
/// encoded — what decoding it back said.
pub(crate) type ExportOutcome = (PathBuf, Result<(), MediaError>, Option<ExportVerification>);

pub(crate) struct ExportJob {
    pub(crate) cancellation: ExportCancellation,
    pub(crate) progress_rx: crossbeam_channel::Receiver<ExportProgress>,
    pub(crate) result_rx: mpsc::Receiver<ExportOutcome>,
    pub(crate) progress: ExportProgress,
}

#[derive(Clone, Copy)]
enum CaptionFormat {
    Srt,
    Vtt,
}

impl CaptionFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    const fn filter_name(self) -> &'static str {
        match self {
            Self::Srt => "SubRip captions",
            Self::Vtt => "WebVTT captions",
        }
    }
}

/// The stable delivery profile the human export dialog's aspect choice maps to.
///
/// The dialog offers a master export plus the three CC/delivery aspects; the
/// agent export queue already gates on the matching profile, so the human path
/// runs the same conformance contract.
const fn export_delivery_profile(aspect: Option<DeliveryAspect>) -> DeliveryProfile {
    match aspect {
        None => DeliveryProfile::SourceMaster,
        Some(DeliveryAspect::Widescreen) => DeliveryProfile::Youtube1080p,
        Some(DeliveryAspect::Vertical) => DeliveryProfile::VerticalShort,
        Some(DeliveryAspect::Square) => DeliveryProfile::SquareSocial,
    }
}

/// The raster the encoder must render for the dialog's current settings.
///
/// A delivery aspect owns its raster: `delivery_conformance` validates
/// `DeliveryProfile::resolution`, which is exactly `aspect.resolution()`. If
/// the dialog's editable frame size were also allowed to apply, the gate would
/// validate one raster while the encoder rendered another. Master export has no
/// profile raster to conflict with, so it stays editable.
const fn export_frame_size(aspect: Option<DeliveryAspect>, width: u32, height: u32) -> (u32, u32) {
    match aspect {
        Some(aspect) => aspect.resolution(),
        None => (width, height),
    }
}

/// Delivery-conformance findings split into what refuses an export and what is
/// only advisory.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ExportConformance {
    pub(crate) blocking: Vec<QaIssue>,
    pub(crate) advisory: Vec<QaIssue>,
}

impl ExportConformance {
    /// Split a conformance report into what the export dialog must show.
    ///
    /// `QaSeverity::Info` findings are deliberately dropped. They are
    /// descriptive rather than actionable — `abrupt_cut` alone emits one line
    /// per hard cut in the timeline — so including them buries the warnings a
    /// person actually has to read before delivering. The full report is still
    /// available from `delivery_conformance` and from the agent export queue.
    fn from_report(report: &DeliveryConformanceReport) -> Self {
        let (blocking, advisory) = report
            .issues
            .iter()
            .filter(|issue| matches!(issue.severity, QaSeverity::Error | QaSeverity::Warning))
            .cloned()
            .partition(|issue| issue.severity == QaSeverity::Error);
        Self { blocking, advisory }
    }

    #[must_use]
    pub(crate) fn export_ready(&self) -> bool {
        self.blocking.is_empty()
    }

    /// One line per blocking issue, keeping the machine-readable code with the
    /// human sentence so a recorded error stays diagnosable.
    #[must_use]
    pub(crate) fn summary(&self) -> String {
        if self.export_ready() {
            return "Delivery conformance passed".to_owned();
        }
        let detail = self
            .blocking
            .iter()
            .map(|issue| format!("[{}] {}", issue.code, issue.message))
            .collect::<Vec<_>>()
            .join(" · ");
        format!("Delivery conformance refused this export: {detail}")
    }
}

/// Serve a conformance report from the cache, or compute and store one.
///
/// Split out of `cached_export_conformance` so the cache's identity rule is
/// provable without a `KinewrightApp`: CC6 §11.2.20 requires that a cached
/// 8-bit report is never served for a 10-bit key, and the only thing that
/// guarantees that is `ConformanceKey`'s equality.
fn cached_conformance(
    cache: &mut Option<(ConformanceKey, Result<ExportConformance, String>)>,
    key: ConformanceKey,
    compute: impl FnOnce(ConformanceKey) -> Result<ExportConformance, String>,
) -> Result<ExportConformance, String> {
    if let Some((cached_key, cached)) = cache.as_ref()
        && *cached_key == key
    {
        return cached.clone();
    }
    let conformance = compute(key);
    *cache = Some((key, conformance.clone()));
    conformance
}

/// Run the exact structural, pipeline, and source-colour contract the export
/// will render against.
#[allow(clippy::similar_names)]
fn export_conformance(
    document: &Document,
    aspect: Option<DeliveryAspect>,
    depth: DeliveryEncodeDepth,
    focus_x_percent: u8,
    focus_y_percent: u8,
) -> Result<ExportConformance, DeliveryVariantError> {
    delivery_conformance(
        document,
        export_delivery_profile(aspect),
        depth,
        focus_x_percent,
        focus_y_percent,
    )
    .map(|report| ExportConformance::from_report(&report))
}

/// The radio label for one delivery depth on one delivery lane (CC8 §8).
///
/// The SDR labels are unchanged. The HDR label names the lane it actually is —
/// §5.1's HLG Rec.2020 High 10 — and deliberately says nothing about how it
/// will look: §0.2 Q4 and §4 forbid any monitoring claim, and CC8 has no
/// calibrated HDR display path.
#[must_use]
pub(crate) const fn delivery_lane_depth_label(
    lane: DeliveryLane,
    depth: DeliveryEncodeDepth,
) -> &'static str {
    match (lane, depth) {
        (DeliveryLane::SdrRec709, DeliveryEncodeDepth::Eight) => "8-bit H.264",
        (DeliveryLane::SdrRec709, DeliveryEncodeDepth::Ten) => "10-bit H.264",
        (DeliveryLane::HdrHlgRec2020, _) => "10-bit H.264 · HLG Rec.2020 (HDR)",
    }
}

/// The delivery colour description this dialog's inline `ExportSettings`
/// carries at one lane (CC6 §8.4, R11).
///
/// The dialog keeps its inline construction — routing it through
/// `DeliveryProfile::export_settings` would derive the resolution from the
/// profile and the fps from the document, silently disabling the dialog's own
/// Frame size and FPS controls. Only `bit_depth` moves, and a test asserts
/// this agrees with `delivery_color_for_depth`, which is the agreement that
/// actually matters.
#[must_use]
fn dialog_delivery_color(document: &Document, depth: DeliveryEncodeDepth) -> ColorDescription {
    let mut delivery_color = document.color_context.delivery.clone();
    delivery_color.bit_depth = depth.color_bit_depth();
    delivery_color
}

/// The already-materialized delivery document re-checked as a master.
///
/// No reframe is applied twice and the colour contract is measured on the exact
/// rendered document.
fn export_conformance_report(
    document: &Document,
    depth: DeliveryEncodeDepth,
) -> Result<DeliveryConformanceReport, DeliveryVariantError> {
    delivery_conformance(document, DeliveryProfile::SourceMaster, depth, 50, 50)
}

/// Re-check every fail-closed gate on the worker, against the exact documents
/// and sources the encoder is about to read.
fn run_export_after_preflight(
    conformance: &ExportConformance,
    media: &ExportMediaPreflightReport,
    looks: &ExportLutPreflightReport,
    export: impl FnOnce() -> Result<(), MediaError>,
) -> Result<(), MediaError> {
    if !conformance.export_ready() {
        return Err(MediaError::Backend(conformance.summary()));
    }
    if !media.export_ready() {
        return Err(MediaError::Backend(media.summary()));
    }
    if !looks.export_ready() {
        return Err(MediaError::Backend(looks.summary()));
    }
    export()
}

/// The message a project with LUT nodes but no store root reports (CC4 §2.2).
pub(crate) const EXPORT_PROJECT_NOT_SAVED: &str = concat!(
    "project_not_saved: this timeline applies looks, but the project has never been saved, ",
    "so its <stem>.kinewright-assets store does not exist. Save the project, then export."
);

/// The recovery a refused store root reports at the export gate (CC4 §2.2).
///
/// A refused root is never `project_not_saved`: the project *was* saved, so
/// telling the operator to save it again is a loop that cannot terminate. The
/// only thing that clears the refusal is moving the project or clearing
/// whatever occupies the derived root.
pub(crate) const EXPORT_LUT_STORE_ROOT_RECOVERY: &str = concat!(
    "Move the project to a directory where its <stem>.kinewright-assets store can be created, ",
    "or remove the file or symlink occupying that path, then export."
);

/// Frame a session's typed store refusal as the export gate's blocking reason.
#[must_use]
pub(crate) fn export_store_refusal_reason(store_error: &str) -> String {
    format!("{store_error}; {EXPORT_LUT_STORE_ROOT_RECOVERY}")
}

/// Run the CC4 §2.3 LUT preflight against the store that owns the bytes.
///
/// Mirrors `export_media_preflight`: Core supplies the document half — which
/// assets a frame could actually need — and the store injects the observation,
/// because availability is machine-local runtime state and can change while a
/// project is open. A project with no store resolves every imported asset as
/// `missing`, which is exactly the blocking behaviour `project_not_saved`
/// describes.
///
/// `store_error` is the session's typed `lut_store_root_invalid` refusal, and
/// it is what separates the two ways `store` can be `None`. A project that has
/// never been saved has no root to check and is told to save; a project whose
/// derived root this process refuses to use was already saved, so it is told
/// what the root refused with and how to clear it (CC4 §2.2).
#[must_use]
pub(crate) fn export_lut_preflight(
    document: &Document,
    store: Option<&LutStore>,
    store_error: Option<&str>,
) -> ExportLutPreflightReport {
    if let Some(store) = store {
        return export_lut_preflight_with(document, &store.availability_resolver());
    }
    let imported_reason = store_error.map_or_else(
        || EXPORT_PROJECT_NOT_SAVED.to_owned(),
        export_store_refusal_reason,
    );
    export_lut_preflight_with(document, &|asset: &LutAsset| {
        storeless_availability(asset, &imported_reason)
    })
}

/// Availability for a project that has no usable store root.
///
/// A built-in is generated in the binary and is never written to a store, so
/// it stays `verified` here — a project that has only ever applied a built-in
/// look exports fine before its first save. Only an imported asset, whose
/// bytes the project claims to own, reports `imported_reason`.
fn storeless_availability(asset: &LutAsset, imported_reason: &str) -> LutAvailabilityStatus {
    if let LutAssetSource::Builtin { name } = &asset.source {
        // A built-in never lives in a store, so whether the project has been
        // saved cannot decide its availability: only this binary's bake can.
        // A recorded hash that matches no bake is `changed`, naming both
        // hashes, exactly as the store-backed resolver reports it — calling it
        // `missing` with a `project_not_saved` reason would send the operator
        // to save a project that would still not export (CC4 §2.3).
        return match BuiltinLook::from_name(name) {
            Some(builtin) if builtin.sha256() == asset.sha256 => LutAvailabilityStatus {
                kind: LutAvailabilityKind::Verified,
                observed_sha256: Some(asset.sha256.clone()),
                reason: None,
                path: None,
            },
            Some(builtin) => LutAvailabilityStatus {
                kind: LutAvailabilityKind::Changed,
                observed_sha256: Some(builtin.sha256().to_owned()),
                reason: Some(
                    LutStoreError {
                        code: LutStoreErrorCode::ChangedLutAsset,
                        detail: format!(
                            "this build's {name} bake differs from the recorded content"
                        ),
                        observed: Some(builtin.sha256().to_owned()),
                        allowed: Some(asset.sha256.clone()),
                    }
                    .to_string(),
                ),
                path: None,
            },
            None => LutAvailabilityStatus {
                kind: LutAvailabilityKind::Missing,
                observed_sha256: None,
                reason: Some(
                    LutStoreError {
                        code: LutStoreErrorCode::UnknownBuiltinLook,
                        detail: "this build has no bake for the recorded built-in look".to_owned(),
                        observed: Some(name.clone()),
                        allowed: None,
                    }
                    .to_string(),
                ),
                path: None,
            },
        };
    }
    LutAvailabilityStatus {
        kind: LutAvailabilityKind::Missing,
        observed_sha256: None,
        reason: Some(imported_reason.to_owned()),
        path: None,
    }
}

// ---------------------------------------------------------------------------
// CC6 §8.4: the post-export verification block
// ---------------------------------------------------------------------------

/// The one-word verdict of a verification.
///
/// Four labels, three styles: an over-budget difference and a mis-tagged file
/// are both `Error`-severity findings (CC6 §3.8), so they share `STATUS_DANGER`
/// and are told apart by their words. `NOT VERIFIED` is a warning, not a
/// failure: it means nobody measured, which is never the same as failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VerificationStatus {
    pub(crate) label: &'static str,
    pub(crate) color: egui::Color32,
}

/// The status one verification reports.
///
/// Precedence: nothing measured, then a tag mismatch, then a budget overrun.
/// A mis-tagged file is put first because it is never a creative choice and
/// every downstream tool will misread it, whereas a difference budget is a
/// codec measurement of a file that is still a valid deliverable.
#[must_use]
pub(crate) fn verification_status(verification: Option<&ExportVerification>) -> VerificationStatus {
    let Some(ExportVerification::Measured(verification)) = verification else {
        return VerificationStatus {
            label: "NOT VERIFIED",
            color: color::STATUS_WARNING,
        };
    };
    if !verification.tags.conforming {
        return VerificationStatus {
            label: "TAG MISMATCH",
            color: color::STATUS_DANGER,
        };
    }
    // `!technical_pass` is folded in here rather than given a fifth label:
    // CC6 §3.8 gives a verification exactly two `Error`-severity codes,
    // `delivery_tag_mismatch` — already answered above — and
    // `decoded_difference_over_budget`, which is a budget overrun. A
    // `technical_pass = false` that reaches this line is therefore
    // budget-shaped, and §8.4 pins the four labels.
    if !verification.comparison.within_budgets || !verification.technical_pass {
        return VerificationStatus {
            label: "OVER BUDGET",
            color: color::STATUS_DANGER,
        };
    }
    VerificationStatus {
        label: "VERIFIED",
        color: color::STATUS_SUCCESS,
    }
}

/// One rendered line of the verification block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerificationLine {
    pub(crate) text: String,
    pub(crate) color: egui::Color32,
}

impl VerificationLine {
    fn muted(text: String) -> Self {
        Self {
            text,
            color: color::TEXT_MUTED,
        }
    }
}

/// Hundredths of a decibel as a signed decimal, without inventing a float.
///
/// The sign is taken from the whole value rather than from its integer part:
/// `-50` hundredths is `-0.50 dB`, and `hundredths / 100` is `0` there, so
/// composing the string from the quotient's sign silently drops the minus for
/// every value in `(-1, 0)`.
#[must_use]
fn decibels(hundredths: i32) -> String {
    let sign = if hundredths < 0 { "-" } else { "" };
    let magnitude = hundredths.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

/// A measurement against its budget, with the budget shown next to it.
fn budget_line(label: &str, measured: i64, budget: i64, units: &str) -> VerificationLine {
    let within = measured <= budget;
    VerificationLine {
        text: format!(
            "{label} {measured} {units} · budget {budget} · {}",
            if within { "within" } else { "OVER" }
        ),
        color: if within {
            color::TEXT_SECONDARY
        } else {
            color::STATUS_DANGER
        },
    }
}

/// Every line the verification block renders, in order.
///
/// A pure function of the outcome so the block is provable in a headless
/// test rather than only observable by eye. `MAX_ADVISORY_LINES` deliberately
/// does **not** apply: it governs preflight advisories, and a truncated
/// verification result would be worse than none.
#[must_use]
#[allow(clippy::too_many_lines)]
pub(crate) fn verification_lines(
    verification: Option<&ExportVerification>,
) -> Vec<VerificationLine> {
    let status = verification_status(verification);
    let mut lines = vec![VerificationLine {
        text: status.label.to_owned(),
        color: status.color,
    }];
    let verification = match verification {
        None => {
            lines.push(VerificationLine::muted(
                "No export has been verified in this session.".to_owned(),
            ));
            return lines;
        }
        Some(ExportVerification::Unavailable(reason)) => {
            // Never a pass, and never attributed to a later measurement.
            lines.push(VerificationLine::muted(format!(
                "The encode succeeded and the file is untouched; verification could not run: \
                 {reason}"
            )));
            return lines;
        }
        Some(ExportVerification::Measured(verification)) => verification,
    };
    lines.push(VerificationLine::muted(format!(
        "{} · {} lane · decoded {}",
        verification.output_path.display(),
        match verification.delivery_bit_depth {
            DeliveryEncodeDepth::Eight => "8-bit",
            DeliveryEncodeDepth::Ten => "10-bit",
        },
        verification.decoded_pixel_format,
    )));

    lines.push(VerificationLine::muted(format!(
        "PROBED TAGS · source {}",
        verification.tags.tag_source
    )));
    // Every checked field, whether or not it disagrees — the same rows the
    // Colour QC window draws, from the same two functions, so the two surfaces
    // cannot describe one `DeliveryTagCheck` differently. A field that is right
    // is evidence too, and a field the container has no syntax for is drawn in
    // its own muted tone rather than as a wrong tag (CC6 §3.6).
    for (field, expected, probed) in crate::color_qc_ui::tag_field_rows(&verification.tags) {
        lines.push(VerificationLine {
            text: format!("{field} · expected {expected} · probed {probed}"),
            color: crate::color_qc_ui::tag_field_color(&verification.tags, field),
        });
    }
    if verification.tags.conforming {
        lines.push(VerificationLine {
            text: "every probed delivery tag matches the export settings".to_owned(),
            color: color::STATUS_SUCCESS,
        });
    }
    for mismatch in &verification.tags.mismatches {
        lines.push(VerificationLine {
            text: format!(
                "MISMATCH · {} · probed {} · expected {}",
                mismatch.field, mismatch.observed, mismatch.allowed
            ),
            color: color::STATUS_DANGER,
        });
    }
    for entry in &verification.tags.not_representable {
        // Not a wrong tag: a field the container has no syntax for. Muted and
        // labelled so it reads differently from a mismatch.
        lines.push(VerificationLine::muted(format!(
            "NOT REPRESENTABLE · {} · expected {} · {}",
            entry.field, entry.expected, entry.reason
        )));
    }

    let comparison = &verification.comparison;
    let budgets: DeliveryBudgets = comparison.budgets;
    lines.push(VerificationLine::muted(format!(
        "DECODED vs REFERENCE · frames {}",
        comparison
            .frames
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )));
    lines.push(budget_line(
        "luma max",
        i64::from(comparison.luma.maximum_code_diff),
        i64::from(budgets.luma_max_code),
        "code",
    ));
    lines.push(budget_line(
        "luma P99",
        comparison.luma.p99_code_diff_millionths,
        budgets.luma_p99_code_millionths,
        "µcode",
    ));
    lines.push(budget_line(
        "luma mean",
        comparison.luma.mean_code_diff_millionths,
        budgets.luma_mean_code_millionths,
        "µcode",
    ));
    lines.push(budget_line(
        "RGB mean",
        comparison.combined.mean_code_diff_millionths,
        budgets.rgb_mean_code_millionths,
        "µcode (8-bit equivalent)",
    ));
    lines.push(match comparison.psnr_db_hundredths {
        Some(psnr) => VerificationLine {
            text: format!(
                "PSNR {} dB · floor {} dB · {}",
                decibels(psnr),
                decibels(budgets.psnr_floor_db_hundredths),
                if psnr >= budgets.psnr_floor_db_hundredths {
                    "within"
                } else {
                    "OVER"
                }
            ),
            color: if psnr >= budgets.psnr_floor_db_hundredths {
                color::TEXT_SECONDARY
            } else {
                color::STATUS_DANGER
            },
        },
        // `None` is the degenerate zero-MSE case, not a missing measurement.
        None => VerificationLine {
            text: "PSNR — · the 8-bit-equivalent MSE was exactly zero".to_owned(),
            color: color::STATUS_SUCCESS,
        },
    });
    for (label, channel) in [
        ("R", &comparison.red),
        ("G", &comparison.green),
        ("B", &comparison.blue),
    ] {
        lines.push(VerificationLine::muted(format!(
            "{label} max {} · P99 {} µcode · mean {} µcode (not gated)",
            channel.maximum_code_diff,
            channel.p99_code_diff_millionths,
            channel.mean_code_diff_millionths
        )));
    }
    lines.push(VerificationLine::muted(
        comparison.rgb_extremes_note.clone(),
    ));

    let ycbcr = &comparison.decoded_ycbcr;
    lines.push(VerificationLine::muted(format!(
        "DECODED Y′CbCr · {}-bit native planes",
        ycbcr.bit_depth
    )));
    for (label, plane) in [("Y′", &ycbcr.luma), ("Cb", &ycbcr.cb), ("Cr", &ycbcr.cr)] {
        lines.push(VerificationLine::muted(format!(
            "{label} below {} px ({} bp) · above {} px ({} bp)",
            plane.below_count,
            plane.below_basis_points,
            plane.above_count,
            plane.above_basis_points
        )));
    }
    for exception in &verification.exceptions {
        lines.push(VerificationLine {
            text: format!(
                "{:?} · {} · {}",
                exception.severity, exception.code, exception.message
            ),
            color: crate::color_qc_ui::severity_color(exception.severity),
        });
    }
    lines
}

/// The reason a cancelled export reports no verification (CC6 §6.5).
pub(crate) const EXPORT_CANCELLED_BEFORE_VERIFICATION: &str = "cancelled before verification";

/// The verification of a finished encode the operator cancelled, if they did.
///
/// `Some` means the operator pressed Cancel while the encode was finishing, so
/// no verification was started: the file is written and untouched, and the
/// dialog says `NOT VERIFIED` with the reason rather than a pass, a failure,
/// or a frozen progress bar. Cancellation cannot un-write a finished file, and
/// this never tries to.
#[must_use]
fn cancelled_before_verification(cancellation: &ExportCancellation) -> Option<ExportVerification> {
    cancellation
        .is_cancelled()
        .then(|| ExportVerification::Unavailable(EXPORT_CANCELLED_BEFORE_VERIFICATION.to_owned()))
}

/// What the export worker reports about the file it just wrote (CC6 §6.5).
///
/// `None` when there is nothing to describe: the encode did not succeed, so no
/// file exists to measure.
///
/// **A verification never fails an export, and it never takes the worker down
/// with it.** The measurement decodes a file with a third-party backend, and a
/// backend that unwinds must not lose an encode that already succeeded, so the
/// call is contained exactly as `export_queue::verify_output` contains the
/// agent's: a panic becomes `Unavailable`, the dialog says `NOT VERIFIED`, and
/// the finished file is reported at the path the operator chose. The file is
/// never moved, renamed, or altered here for any outcome.
///
/// Cancellation is checked before the measurement rather than inside it:
/// cancelling is the operator saying "stop working", the encode has already
/// succeeded, and skipping the verification is the only thing left that
/// cancellation can honour.
fn worker_verification(
    encode: &Result<(), MediaError>,
    cancellation: &ExportCancellation,
    measure: impl FnOnce() -> Result<DeliveryVerification, MediaError>,
) -> Option<ExportVerification> {
    if encode.is_err() {
        return None;
    }
    if let Some(cancelled) = cancelled_before_verification(cancellation) {
        return Some(cancelled);
    }
    let measured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(measure));
    Some(match measured {
        Ok(Ok(verification)) => ExportVerification::Measured(Box::new(verification)),
        Ok(Err(error)) => ExportVerification::Unavailable(error.to_string()),
        Err(payload) => ExportVerification::Unavailable(format!(
            "delivery verification panicked: {}",
            panic_message(payload.as_ref())
        )),
    })
}

/// The message a panic payload carries, when it carries one at all.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
        .unwrap_or_else(|| "unknown panic payload".to_owned())
}

/// Which half of the export job the dialog is showing.
///
/// A verification decodes the finished file and re-renders a bounded frame
/// sample, which sends no `ExportProgress` at all. Left as a progress bar, that
/// reads as a hung encode at 100 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportStage {
    Encoding,
    Verifying,
}

/// The stage one progress report describes.
///
/// `total_frames == 0` is the pre-roll before the encoder has counted the
/// document, not a finished encode, so it stays `Encoding`.
#[must_use]
pub(crate) const fn export_stage(progress: &ExportProgress) -> ExportStage {
    if progress.total_frames > 0 && progress.completed_frames >= progress.total_frames {
        ExportStage::Verifying
    } else {
        ExportStage::Encoding
    }
}

/// What the dialog says while the finished file is being measured (CC6 §8.4).
pub(crate) const VERIFYING_STAGE_NOTE: &str = "Verifying export… the file is written and is never \
     moved, renamed, or altered by this measurement.";

/// What Cancel means once the encode has finished (CC6 §6.5).
pub(crate) const CANCEL_DURING_VERIFICATION_HINT: &str = "Cancelling during verification skips the measurement; a file that has already been written \
     stays exactly where it is.";

/// The running job's half of the dialog: which wait this is, and Cancel.
///
/// Returns whether Cancel was pressed, so the caller keeps the borrow of `self`
/// the window closure cannot have.
///
/// Free rather than a method so a headless test can paint it: this is the one
/// part of the body whose text is prose, and a sentence assembled from
/// implicitly concatenated literals silently keeps the indentation between
/// them, which reaches the operator as a run of spaces mid-sentence.
fn export_job_body(ui: &mut egui::Ui, progress: &ExportProgress) -> bool {
    match export_stage(progress) {
        ExportStage::Encoding => {
            #[allow(clippy::cast_precision_loss)]
            let fraction = if progress.total_frames == 0 {
                0.0
            } else {
                progress.completed_frames as f32 / progress.total_frames as f32
            };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .show_percentage()
                    .text(format!(
                        "{} / {} frames",
                        progress.completed_frames, progress.total_frames
                    )),
            );
            ui.colored_label(color::TEXT_SECONDARY, "Encoding on background worker");
        }
        ExportStage::Verifying => {
            // The encode is done and the file is written: a bar pinned at
            // 100 % would read as a hung encoder.
            ui.colored_label(color::TEXT_SECONDARY, VERIFYING_STAGE_NOTE);
        }
    }
    ui.add(egui::Button::image_and_text(
        Icon::Stop.image(size::ICON_MD),
        "Cancel export",
    ))
    .on_hover_text(CANCEL_DURING_VERIFICATION_HINT)
    .clicked()
}

/// The line under the verification block that reaches the Colour QC window.
///
/// A **button**, not a label: it names another surface by its menu path, and a
/// sentence that tells the operator where to click is a control that has
/// forgotten it is one. Returned rather than acted on so the caller keeps the
/// borrow of `self` the window closure cannot have.
pub(crate) fn color_qc_link(ui: &mut egui::Ui) -> egui::Response {
    ui.add(egui::Button::new(
        "Per-pixel range, gamut, skin, and per-node detail…",
    ))
    .on_hover_text(crate::color_qc_ui::COLOR_QC_BANNER)
}

/// Draw the verification block. Every line, uncapped, in the order
/// [`verification_lines`] produced them.
pub(crate) fn verification_block(ui: &mut egui::Ui, verification: Option<&ExportVerification>) {
    ui.label(theme::caps_label(
        "DELIVERY VERIFICATION",
        color::TEXT_MUTED,
    ));
    for line in verification_lines(verification) {
        ui.add(egui::Label::new(egui::RichText::new(line.text).color(line.color)).wrap());
    }
}

impl KinewrightApp {
    pub(crate) fn open_export_dialog(&mut self) {
        let resolution = self.export_dialog.delivery_aspect.map_or(
            self.focused().document.resolution,
            DeliveryAspect::resolution,
        );
        self.export_dialog.width = resolution.0;
        self.export_dialog.height = resolution.1;
        self.export_dialog.fps_numerator = self.focused().document.fps.numerator();
        self.export_dialog.fps_denominator = self.focused().document.fps.denominator();
        if let Some(project_path) = &self.focused().project_path {
            self.export_dialog.output = project_path.with_extension("mp4").display().to_string();
        }
        self.export_dialog.open = true;
    }

    pub(crate) fn choose_export_output(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("MPEG-4 video", &["mp4"])
            .set_file_name("export.mp4")
            .save_file()
        else {
            return;
        };
        self.export_dialog.output = path.display().to_string();
    }

    /// Materialize the exact document the export will render and run both
    /// fail-closed gates before any encoder work starts: the delivery
    /// colour/structural conformance contract, then source media availability.
    ///
    /// Returns `None` after recording the human-readable refusal.
    fn export_delivery_document(&mut self) -> Option<Arc<Document>> {
        let document = if let Some(aspect) = self.export_dialog.delivery_aspect {
            let variant = match DeliveryVariant::new(
                aspect,
                self.export_dialog.focus_x_percent,
                self.export_dialog.focus_y_percent,
            ) {
                Ok(variant) => variant,
                Err(error) => {
                    self.record_error("Export", error.to_string());
                    return None;
                }
            };
            match document_for_delivery_variant(&self.focused().document, variant) {
                Ok(document) => Arc::new(document),
                Err(error) => {
                    self.record_error("Export", error.to_string());
                    return None;
                }
            }
        } else {
            Arc::clone(&self.focused().document)
        };
        let conformance = match export_conformance(
            &self.focused().document,
            self.export_dialog.delivery_aspect,
            self.export_dialog.delivery_bit_depth,
            self.export_dialog.focus_x_percent,
            self.export_dialog.focus_y_percent,
        ) {
            Ok(conformance) => conformance,
            Err(error) => {
                self.record_error("Export", error.to_string());
                return None;
            }
        };
        if !conformance.export_ready() {
            self.record_error("Export", conformance.summary());
            return None;
        }
        let media_preflight = export_media_preflight(&document, self.analysis.as_ref());
        if !media_preflight.export_ready() {
            self.record_error("Export", media_preflight.summary());
            return None;
        }
        // Every look a frame could need is rehashed here, alongside the media
        // preflight, so a missing or changed LUT blocks the export with the
        // asset id, title, hash, expected store path, and recovery action
        // rather than failing at render time (CC4 §2.3).
        let lut_preflight = export_lut_preflight(
            &document,
            self.focused().lut_store.as_ref(),
            self.focused().lut_store_error.as_deref(),
        );
        if !lut_preflight.export_ready() {
            self.record_error("Export", lut_preflight.summary());
            return None;
        }
        Some(document)
    }

    /// Keep the dialog's frame size equal to the raster the delivery gate
    /// validates. A no-op for a Master export, whose frame size is the
    /// operator's to choose.
    fn lock_frame_size_to_delivery_aspect(&mut self) {
        (self.export_dialog.width, self.export_dialog.height) = export_frame_size(
            self.export_dialog.delivery_aspect,
            self.export_dialog.width,
            self.export_dialog.height,
        );
    }

    /// The conformance report for the dialog's current settings, recomputed
    /// only when one of its inputs actually changes.
    ///
    /// `delivery_conformance` clones the document, materializes the delivery
    /// reframe, and touches the filesystem once per source asset. An open
    /// dialog repaints continuously, so running it unconditionally put all of
    /// that on the UI thread every frame. `start_export` re-validates
    /// independently, so a stale cache can never admit an export.
    ///
    /// The error is kept as a `String`: `DeliveryVariantError` is not `Clone`,
    /// and the dialog only ever renders it.
    fn cached_export_conformance(&mut self) -> Result<ExportConformance, String> {
        let key = ConformanceKey {
            revision: self.focused().revision.0,
            aspect: self.export_dialog.delivery_aspect,
            focus_x_percent: self.export_dialog.focus_x_percent,
            focus_y_percent: self.export_dialog.focus_y_percent,
            width: self.export_dialog.width,
            height: self.export_dialog.height,
            delivery_bit_depth: self.export_dialog.delivery_bit_depth,
        };
        let document = Arc::clone(&self.focused().document);
        cached_conformance(&mut self.export_dialog.conformance_cache, key, |key| {
            export_conformance(
                &document,
                key.aspect,
                key.delivery_bit_depth,
                key.focus_x_percent,
                key.focus_y_percent,
            )
            .map_err(|error| error.to_string())
        })
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn start_export(&mut self) {
        if self.export_job.is_some() {
            return;
        }
        // The encoder renders this raster and the gate below validates the
        // delivery profile's. Re-apply the lock so they cannot disagree even if
        // the export was started without the dialog having drawn a frame.
        self.lock_frame_size_to_delivery_aspect();
        if self.focused().document.duration <= TimeCode::ZERO {
            self.record_error("Export", "Add a clip to the timeline before exporting");
            return;
        }
        if !self.export_dialog.width.is_multiple_of(2)
            || !self.export_dialog.height.is_multiple_of(2)
        {
            self.record_error("Export", "H.264 export width and height must be even");
            return;
        }
        let fps = match Rational::new(
            self.export_dialog.fps_numerator,
            self.export_dialog.fps_denominator,
        ) {
            Ok(fps) => fps,
            Err(error) => {
                self.record_error("Export", format!("Invalid export frame rate: {error}"));
                return;
            }
        };
        let mut output = PathBuf::from(self.export_dialog.output.trim());
        if output.as_os_str().is_empty() {
            self.record_error("Export", "Choose an export output path");
            return;
        }
        if output.extension().is_none() {
            output.set_extension("mp4");
            self.export_dialog.output = output.display().to_string();
        }
        let cancellation = ExportCancellation::default();
        let Some(document) = self.export_delivery_document() else {
            return;
        };
        let depth = self.export_dialog.delivery_bit_depth;
        // R11: the dialog keeps its inline construction and moves exactly one
        // field. Routing this through `DeliveryProfile::export_settings` would
        // take the resolution from the profile and the fps from the document,
        // silently disabling the Frame size and FPS controls above.
        let settings = ExportSettings {
            fps,
            resolution: (self.export_dialog.width, self.export_dialog.height),
            delivery_color: dialog_delivery_color(&document, depth),
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 8_000_000,
            audio_bitrate: 192_000,
            cancellation: cancellation.clone(),
        };
        let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = mpsc::channel();
        let media = Arc::clone(&self.exporter);
        let worker_analysis = Arc::clone(&self.analysis);
        let worker_store = self.focused().lut_store.clone();
        let worker_store_error = self.focused().lut_store_error.clone();
        let worker_document = document;
        let worker_output = output.clone();
        let spawn = thread::Builder::new()
            .name("kinewright-export".to_owned())
            .spawn(move || {
                let media_preflight =
                    export_media_preflight(&worker_document, worker_analysis.as_ref());
                let lut_preflight = export_lut_preflight(
                    &worker_document,
                    worker_store.as_ref(),
                    worker_store_error.as_deref(),
                );
                // The delivery document is already materialized, so the worker
                // re-checks it as a master: no reframe is applied twice and the
                // colour contract is measured on the exact rendered document.
                let verify_document = Arc::clone(&worker_document);
                let verify_settings = settings.clone();
                let result = export_conformance_report(&worker_document, depth)
                    .map_err(|error| MediaError::Backend(error.to_string()))
                    .and_then(|report| {
                        run_export_after_preflight(
                            &ExportConformance::from_report(&report),
                            &media_preflight,
                            &lut_preflight,
                            || {
                                media.export_document(
                                    worker_document,
                                    &worker_output,
                                    settings,
                                    progress_tx,
                                )
                            },
                        )
                    });
                let request = DeliveryVerificationRequest {
                    frame_count: DELIVERY_VERIFICATION_FRAME_COUNT,
                    budgets: DeliveryBudgets::for_depth(depth),
                    expected_delivery: verify_settings.delivery_color.clone(),
                };
                let verification =
                    worker_verification(&result, &verify_settings.cancellation, || {
                        worker_analysis.verify_delivery_output(
                            verify_document,
                            &worker_output,
                            &verify_settings,
                            request,
                        )
                    });
                let _ = result_tx.send((worker_output, result, verification));
            });
        if let Err(error) = spawn {
            self.record_error("Export", format!("Could not start export: {error}"));
            return;
        }
        self.status = format!("Exporting {}…", output.display());
        // A new run's verification is the new run's; the previous file's
        // measurement must never be read as this one's.
        self.export_dialog.verification = None;
        self.export_job = Some(ExportJob {
            cancellation,
            progress_rx,
            result_rx,
            progress: ExportProgress {
                completed_frames: 0,
                total_frames: 0,
            },
        });
    }

    fn save_caption_sidecar(&mut self, format: CaptionFormat, cues: &[CaptionCue]) {
        let extension = format.extension();
        let default_name = self.caption_default_file_name(extension);
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter(format.filter_name(), &[extension])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension(extension);
        }
        let contents = match format {
            CaptionFormat::Srt => srt(cues, self.focused().document.fps),
            CaptionFormat::Vtt => vtt(cues, self.focused().document.fps),
        };
        match std::fs::write(&path, contents) {
            Ok(()) => self.status = format!("Saved captions to {}", path.display()),
            Err(error) => self.record_error(
                "Captions",
                format!("Could not save {}: {error}", path.display()),
            ),
        }
    }

    fn caption_default_file_name(&self, extension: &str) -> String {
        let output = PathBuf::from(self.export_dialog.output.trim());
        let stem = output
            .file_stem()
            .filter(|stem| !stem.is_empty())
            .map(|stem| stem.to_string_lossy().into_owned())
            .or_else(|| {
                self.focused().document.media_pool.first().map(|asset| {
                    std::path::Path::new(&asset.name)
                        .file_stem()
                        .unwrap_or(asset.name.as_ref())
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .unwrap_or_else(|| "captions".to_owned());
        format!("{stem}.{extension}")
    }

    pub(crate) fn poll_export(&mut self, ctx: &egui::Context) {
        let mut completed = None;
        if let Some(job) = &mut self.export_job {
            while let Ok(progress) = job.progress_rx.try_recv() {
                job.progress = progress;
            }
            match job.result_rx.try_recv() {
                Ok(result) => completed = Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    completed = Some((
                        PathBuf::from(&self.export_dialog.output),
                        Err(MediaError::Backend("export worker stopped".to_owned())),
                        None,
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }
        if let Some((path, result, verification)) = completed {
            self.export_job = None;
            match result {
                Ok(()) => {
                    // CC6 §8.4: the bare "Exported …" line is replaced by the
                    // verification block in the dialog. The status bar keeps a
                    // one-word verdict so a closed dialog still says something.
                    self.export_dialog.verification = verification;
                    self.status = format!(
                        "Exported {} · {}",
                        path.display(),
                        verification_status(self.export_dialog.verification.as_ref()).label
                    );
                }
                Err(MediaError::Cancelled) => {
                    "Export cancelled".clone_into(&mut self.status);
                }
                Err(error) => self.record_error("Export", format!("Export failed: {error}")),
            }
        }
    }

    // Export settings, validation, progress, and cancellation share one immediate-mode dialog.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn show_export_dialog(&mut self, ctx: &egui::Context) {
        if !self.export_dialog.open {
            return;
        }
        let mut open = self.export_dialog.open;
        let mut browse = false;
        let mut start = false;
        let mut cancel = false;
        let mut reset_color_pipeline = false;
        let mut open_color_qc = false;
        let caption_cues = self.timeline_caption_cues();
        let mut caption_format = None;
        let project_color_pipeline = color_pipeline_summary(&self.focused().document.color_context);
        // CC8 §8: "The export dialog offers the HDR lane only when the
        // document's delivery description selects it." The lane is a function
        // of that description and of nothing else (§5.2 clause 1), and §5.1's
        // `Bit depth | Ten` row gives the HDR lane exactly one depth, so the
        // radio group below offers what the lane admits rather than a fixed
        // pair. A selection the lane does not admit is corrected here, before
        // the gate runs, so the cache key and the encoder see the same lane.
        let delivery_lane =
            DeliveryLane::for_description(&self.focused().document.color_context.delivery);
        let offered_depths = delivery_lane.encode_depths();
        if !offered_depths.contains(&self.export_dialog.delivery_bit_depth)
            && let Some(depth) = offered_depths.first()
        {
            self.export_dialog.delivery_bit_depth = *depth;
        }
        let color_pipeline_reset_needed =
            managed_sdr_reset_needed(&self.focused().document.color_context);
        // Applied before the gate runs so the cache key, the displayed frame
        // size, and the raster the encoder will render are the same value.
        self.lock_frame_size_to_delivery_aspect();
        // Immediate mode: this reflects the aspect chosen on the previous
        // frame. `start_export` re-runs the same gate before it spawns.
        let conformance = self.cached_export_conformance();
        let conformance_ready = conformance
            .as_ref()
            .is_ok_and(ExportConformance::export_ready);
        let export_blocked = color_pipeline_reset_needed || !conformance_ready;
        // Cloned out before the window borrows `self` mutably: the block is a
        // read-only report of a file that already exists.
        let verification = self.export_dialog.verification.clone();
        egui::Window::new("Export")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
              // The window is not resizable and the findings list is
              // data-dependent, so the body scrolls rather than growing past
              // the screen and hiding the Export button.
              egui::ScrollArea::vertical()
                .max_height(EXPORT_DIALOG_MAX_BODY_HEIGHT)
                .show(ui, |ui| {
                ui.label(theme::caps_label("DELIVERABLE", color::TEXT_MUTED));
                ui.label(
                    egui::RichText::new("H.264 video · AAC audio · MP4 container")
                        .color(color::TEXT_SECONDARY),
                );
                ui.add_space(space::TWO);
                ui.label(theme::caps_label("COLOR PIPELINE", color::TEXT_MUTED));
                for stage in &project_color_pipeline {
                    ui.add(
                        egui::Label::new(egui::RichText::new(stage).color(color::TEXT_SECONDARY))
                            .wrap(),
                    );
                }
                if color_pipeline_reset_needed {
                    ui.colored_label(
                        color::STATUS_DANGER,
                        "BLOCKED · Managed SDR export requires a compatible project colour pipeline.",
                    );
                    if ui
                        .add(
                            egui::Button::new("Reset to Managed SDR")
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::STATUS_DANGER)),
                        )
                        .clicked()
                    {
                        reset_color_pipeline = true;
                    }
                }
                match &conformance {
                    Ok(conformance) => {
                        for issue in &conformance.blocking {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "BLOCKED · {} ({})",
                                        issue.message, issue.code
                                    ))
                                    .color(color::STATUS_DANGER),
                                )
                                .wrap(),
                            );
                        }
                        // The window is fixed-size, so an unbounded advisory
                        // list would push the Export button out of reach. The
                        // remainder is counted rather than silently dropped.
                        for issue in conformance.advisory.iter().take(MAX_ADVISORY_LINES) {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "REVIEW · {} ({})",
                                        issue.message, issue.code
                                    ))
                                    .color(color::STATUS_WARNING),
                                )
                                .wrap(),
                            );
                        }
                        if let Some(hidden) = conformance
                            .advisory
                            .len()
                            .checked_sub(MAX_ADVISORY_LINES)
                            .filter(|hidden| *hidden > 0)
                        {
                            ui.colored_label(
                                color::TEXT_MUTED,
                                format!("… and {hidden} more advisory finding(s)"),
                            );
                        }
                    }
                    Err(error) => {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "BLOCKED · delivery conformance could not run: {error}"
                                ))
                                .color(color::STATUS_DANGER),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.add_space(space::TWO);
                let before_aspect = self.export_dialog.delivery_aspect;
                ui.horizontal(|ui| {
                    ui.label("Delivery");
                    egui::ComboBox::from_id_salt("export-delivery-aspect")
                        .selected_text(
                            self.export_dialog
                                .delivery_aspect
                                .map_or("Master", DeliveryAspect::as_str),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.export_dialog.delivery_aspect,
                                None,
                                "Master",
                            );
                            for aspect in DeliveryAspect::ALL {
                                ui.selectable_value(
                                    &mut self.export_dialog.delivery_aspect,
                                    Some(aspect),
                                    aspect.as_str(),
                                );
                            }
                        });
                    if self.export_dialog.delivery_aspect.is_some() {
                        ui.label("Focal point");
                        ui.add(
                            egui::DragValue::new(&mut self.export_dialog.focus_x_percent)
                                .range(0..=100)
                                .suffix("% x"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.export_dialog.focus_y_percent)
                                .range(0..=100)
                                .suffix("% y"),
                        );
                    }
                });
                if self.export_dialog.delivery_aspect != before_aspect
                    && let Some(aspect) = self.export_dialog.delivery_aspect
                {
                    (self.export_dialog.width, self.export_dialog.height) = aspect.resolution();
                }
                // CC6 §4.1/§8.4: one orthogonal lane choice, not eight
                // profiles. It writes `ExportSettings.delivery_color.bit_depth`
                // and nothing else — the project's own delivery contract is
                // untouched, and `get_color_context` keeps reporting it.
                ui.horizontal_wrapped(|ui| {
                    ui.label("Delivery depth");
                    for depth in offered_depths {
                        ui.radio_value(
                            &mut self.export_dialog.delivery_bit_depth,
                            *depth,
                            delivery_lane_depth_label(delivery_lane, *depth),
                        );
                    }
                    ui.colored_label(
                        color::TEXT_MUTED,
                        if delivery_lane.is_hdr() {
                            // §4: nothing in CC8's UI may imply a monitoring
                            // claim, so this says what the lane *is* and stops.
                            "CC8 §5.1's HDR lane, selected by the project's delivery description"
                        } else {
                            "a job parameter, not a document edit"
                        },
                    );
                });
                ui.add_space(space::TWO);
                egui::Grid::new("export-settings")
                    .num_columns(2)
                    .spacing(egui::vec2(space::THREE, space::TWO))
                    .show(ui, |ui| {
                        ui.label("Output");
                        ui.horizontal(|ui| {
                            ui.scope(|ui| {
                                theme::apply_input_visuals(ui);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.export_dialog.output)
                                        .desired_width(320.0),
                                );
                            });
                            if ui
                                .add(
                                    egui::Button::image_and_text(
                                        Icon::Folder.image(size::ICON_MD),
                                        "Browse…",
                                    )
                                    .fill(color::SURFACE_RAISED),
                                )
                                .clicked()
                            {
                                browse = true;
                            }
                        });
                        ui.end_row();
                        ui.label("Frame size");
                        ui.horizontal(|ui| {
                            // The conformance gate validates the delivery
                            // profile's raster, but the encoder renders this
                            // value. An editable frame size under a delivery
                            // aspect lets those disagree, so the profile's
                            // raster is shown read-only instead.
                            if let Some(aspect) = self.export_dialog.delivery_aspect {
                                let (width, height) = aspect.resolution();
                                ui.colored_label(
                                    color::TEXT_SECONDARY,
                                    format!("{width} × {height}"),
                                );
                                ui.colored_label(
                                    color::TEXT_MUTED,
                                    format!(
                                        "locked by the {} delivery profile",
                                        aspect.as_str()
                                    ),
                                );
                            } else {
                                ui.add(
                                    egui::DragValue::new(&mut self.export_dialog.width)
                                        .range(2..=16_384),
                                );
                                ui.label("×");
                                ui.add(
                                    egui::DragValue::new(&mut self.export_dialog.height)
                                        .range(2..=16_384),
                                );
                            }
                        });
                        ui.end_row();
                        ui.label("FPS");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut self.export_dialog.fps_numerator)
                                    .range(1..=120_000),
                            );
                            ui.label("/");
                            ui.add(
                                egui::DragValue::new(&mut self.export_dialog.fps_denominator)
                                    .range(1..=10_000),
                            );
                        });
                        ui.end_row();
                        ui.label("Captions");
                        ui.horizontal(|ui| {
                            let enabled = caption_cues.is_ok();
                            let disabled_reason =
                                caption_cues.as_ref().err().map_or("", String::as_str);
                            if ui
                                .add_enabled(enabled, egui::Button::new("Save .srt"))
                                .on_disabled_hover_text(disabled_reason)
                                .clicked()
                            {
                                caption_format = Some(CaptionFormat::Srt);
                            }
                            if ui
                                .add_enabled(enabled, egui::Button::new("Save .vtt"))
                                .on_disabled_hover_text(disabled_reason)
                                .clicked()
                            {
                                caption_format = Some(CaptionFormat::Vtt);
                            }
                        });
                        ui.end_row();
                    });
                ui.separator();
                if let Some(job) = &self.export_job {
                    cancel = export_job_body(ui, &job.progress);
                } else {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !export_blocked,
                                egui::Button::image_and_text(
                                    Icon::Export.image(size::ICON_MD),
                                    "Export MP4",
                                )
                                .fill(color::ACCENT_WASH)
                                .stroke(egui::Stroke::new(1.0, color::ACCENT_DIM_BORDER)),
                            )
                            .on_disabled_hover_text(if color_pipeline_reset_needed {
                                "Reset the project colour pipeline before exporting."
                            } else {
                                "Resolve every blocking delivery-conformance issue before exporting."
                            })
                            .clicked()
                        {
                            start = true;
                        }
                    });
                }
                // Nothing to report before the first export of the session.
                if verification.is_some() {
                    ui.separator();
                    // Uncapped, deliberately: MAX_ADVISORY_LINES governs
                    // preflight advisories, and a truncated verification result
                    // would be worse than none (CC6 §8.4).
                    verification_block(ui, verification.as_ref());
                    if color_qc_link(ui).clicked() {
                        open_color_qc = true;
                    }
                }
                });
            });
        // The dialog is a window, and closing it is not cancelling: the export
        // worker keeps going and the status bar keeps reporting it. Forcing it
        // open for the life of the job left the close button inert for the
        // whole verification pass, which sends no progress at all.
        self.export_dialog.open = open;
        if open_color_qc {
            self.color_qc.open = true;
        }
        if browse {
            self.choose_export_output();
        }
        if reset_color_pipeline {
            self.send_operation(Operation::SetColorContext {
                color_context: kinewright_core::ColorContext::sdr_rec709(),
            });
        }
        if start {
            self.start_export();
        }
        if let (Some(format), Ok(cues)) = (caption_format, caption_cues) {
            self.save_caption_sidecar(format, &cues);
        }
        if cancel && let Some(job) = &self.export_job {
            job.cancellation.cancel();
            "Cancelling export…".clone_into(&mut self.status);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use std::collections::BTreeMap;

    use kinewright_core::delivery_color_for_depth;
    use kinewright_core::{
        AssetId, Clip, ClipContent, ClipId, ColorContext, ColorDescription, ColorPrimaries,
        ColorProvenance, ColorTransfer, Effect, EffectId, ExportMediaPreflightIssue,
        LUT_ASSET_ID_PARAMETER, LutAssetId, MediaAsset, MediaAvailabilityKind,
        MediaAvailabilityStatus, MediaKind, ParamValue, Track, TrackId, TrackKind,
    };
    use kinewright_media::test_support::TempDirectory;

    use super::*;

    /// A LUT preflight with nothing to block on.
    fn ready_lut_preflight() -> ExportLutPreflightReport {
        ExportLutPreflightReport {
            checked_lut_assets: Vec::new(),
            issues: Vec::new(),
        }
    }

    fn ready_conformance() -> ExportConformance {
        ExportConformance::default()
    }

    /// One BT.2020/PQ source on the timeline. Its file exists so the only
    /// blocking finding is the source colour contract.
    fn document_with_hdr_source() -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: "hdr-master.mov".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1_920, 1_080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: ColorDescription {
                primaries: ColorPrimaries::Bt2020,
                transfer: ColorTransfer::Smpte2084,
                provenance: ColorProvenance::UserOverride,
                ..ColorContext::sdr_rec709().delivery
            },
        };
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: asset.id,
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
            color_context: ColorContext::sdr_rec709(),
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    #[test]
    fn unsupported_source_colour_refuses_the_export_with_its_code_and_field() {
        let conformance = export_conformance(
            &document_with_hdr_source(),
            None,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("source colour findings are reported, not returned as an error");

        assert!(!conformance.export_ready());
        let issue = conformance
            .blocking
            .iter()
            .find(|issue| issue.code == "unsupported_source_color")
            .expect("BT.2020/PQ source must block the export");
        assert_eq!(issue.asset, Some(AssetId(1)));
        assert!(issue.message.contains("hdr-master.mov"));
        // The fixture's tuple is Rec.2020/PQ with a BT.709 matrix at 8 bits.
        // CC8 §2.1 makes that an HDR-adjacent tuple *outside* the closed set —
        // its matrix is not in the `bt2020_ncl`/`rgb` column — so it is
        // refused on the matrix rather than, as before CC8, on the primaries.
        // The subject of this test is unchanged: an unsupported source colour
        // refuses the export and names its code, field, observed value,
        // allowed values, and recovery action.
        assert!(
            issue.message.contains("code=unsupported_hdr_source_matrix"),
            "{}",
            issue.message
        );
        assert!(issue.message.contains("field=matrix"));
        assert!(issue.message.contains("observed=Bt709"));
        assert!(issue.message.contains("allowed="));
        assert!(
            issue
                .message
                .contains("Apply an explicit supported source-colour override")
        );

        let summary = conformance.summary();
        assert!(summary.contains("unsupported_source_color"));
        assert!(summary.contains("field=matrix"));

        // The encoder is never reached for a refused document.
        let export_called = Cell::new(false);
        let result = run_export_after_preflight(
            &conformance,
            &ExportMediaPreflightReport {
                checked_assets: vec![AssetId(1)],
                issues: Vec::new(),
            },
            &ready_lut_preflight(),
            || {
                export_called.set(true);
                Ok(())
            },
        );
        assert!(!export_called.get());
        assert!(matches!(
            result,
            Err(MediaError::Backend(message)) if message.contains("unsupported_source_color")
        ));
    }

    #[test]
    fn a_conformant_managed_sdr_document_stays_exportable_and_keeps_advisories_visible() {
        let mut document = document_with_hdr_source();
        document.media_pool[0].color_description = ColorContext::sdr_rec709().delivery;
        document.tracks[0].clips[0]
            .effects
            .push(kinewright_core::Effect {
                id: kinewright_core::EffectId(1),
                name: "brightness".to_owned(),
                parameters: std::collections::BTreeMap::new(),
                keyframes: std::collections::BTreeMap::new(),
            });

        let conformance = export_conformance(&document, None, DeliveryEncodeDepth::Eight, 50, 50)
            .expect("conformance must run");

        assert!(conformance.export_ready());
        assert!(
            conformance
                .advisory
                .iter()
                .any(|issue| issue.code == "legacy_colour_semantics"),
            "legacy colour semantics stay visible without blocking the export"
        );
    }

    #[test]
    fn every_dialog_aspect_maps_to_the_delivery_profile_the_agent_queue_gates_on() {
        assert_eq!(export_delivery_profile(None), DeliveryProfile::SourceMaster);
        for aspect in DeliveryAspect::ALL {
            let profile = export_delivery_profile(Some(aspect));
            assert_eq!(profile.aspect(), Some(aspect));
        }
    }

    /// The gate validates the delivery profile's raster while the encoder
    /// renders the dialog's frame size. Under a delivery aspect those must be
    /// the same number, so the frame size is locked to the profile.
    #[test]
    fn a_delivery_aspect_locks_the_frame_size_to_the_raster_the_gate_validates() {
        let source = (3_840, 2_160);
        for aspect in DeliveryAspect::ALL {
            let locked = export_frame_size(Some(aspect), 1_234, 5_678);
            assert_eq!(
                locked,
                aspect.resolution(),
                "an edited frame size cannot override a delivery aspect"
            );
            assert_eq!(
                locked,
                export_delivery_profile(Some(aspect)).resolution(source),
                "the locked raster is exactly the one delivery_conformance checks"
            );
        }

        // Master export has no profile raster to conflict with.
        assert_eq!(export_frame_size(None, 1_234, 5_678), (1_234, 5_678));
        assert_eq!(
            DeliveryProfile::SourceMaster.resolution(source),
            source,
            "a master export renders the project raster, whatever the dialog holds"
        );
    }

    /// `Info` findings are descriptive and emitted once per cut. Showing them
    /// beside real warnings in a fixed-size window buries what has to be read.
    #[test]
    fn informational_findings_are_dropped_and_warnings_are_kept() {
        let report = DeliveryConformanceReport {
            issues: vec![
                qa_issue(QaSeverity::Error, "unsupported_delivery_color"),
                qa_issue(QaSeverity::Warning, "legacy_colour_semantics"),
                qa_issue(QaSeverity::Info, "abrupt_cut"),
                qa_issue(QaSeverity::Info, "abrupt_cut"),
            ],
            ..conformance_report_shell()
        };

        let conformance = ExportConformance::from_report(&report);
        assert_eq!(conformance.blocking.len(), 1);
        assert_eq!(
            conformance.advisory.len(),
            1,
            "only actionable warnings reach the dialog: {:?}",
            conformance.advisory
        );
        assert_eq!(conformance.advisory[0].code, "legacy_colour_semantics");
        assert!(!conformance.export_ready());
    }

    /// The advisory list is capped, and the remainder is counted rather than
    /// silently dropped.
    #[test]
    fn the_advisory_list_is_capped_and_reports_the_remainder() {
        let report = DeliveryConformanceReport {
            issues: (0..MAX_ADVISORY_LINES + 3)
                .map(|_| qa_issue(QaSeverity::Warning, "retimed_audio_muted"))
                .collect(),
            ..conformance_report_shell()
        };

        let conformance = ExportConformance::from_report(&report);
        assert!(conformance.export_ready());
        assert_eq!(conformance.advisory.len(), MAX_ADVISORY_LINES + 3);
        assert_eq!(
            conformance
                .advisory
                .len()
                .checked_sub(MAX_ADVISORY_LINES)
                .unwrap(),
            3,
            "the dialog shows the cap and says how many it withheld"
        );
    }

    fn qa_issue(severity: QaSeverity, code: &str) -> QaIssue {
        QaIssue {
            severity,
            code: code.to_owned(),
            message: format!("{code} finding"),
            asset: None,
            track: None,
            clip: None,
            range: None,
        }
    }

    /// A conformance report carrying no issues, for tests that only care about
    /// how the dialog partitions them.
    fn conformance_report_shell() -> DeliveryConformanceReport {
        let mut document = document_with_hdr_source();
        document.media_pool[0].color_description = ColorContext::sdr_rec709().delivery;
        delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("the fixture conforms")
    }

    #[test]
    fn worker_preflight_failure_reaches_the_result_and_skips_export() {
        let export_called = Cell::new(false);
        let blocked = ExportMediaPreflightReport {
            checked_assets: vec![AssetId(7)],
            issues: vec![ExportMediaPreflightIssue {
                asset: AssetId(7),
                asset_name: "changed-source".to_owned(),
                availability: MediaAvailabilityStatus {
                    kind: MediaAvailabilityKind::Changed,
                    observed_fingerprint: None,
                    reason: Some("source changed after the export was queued".to_owned()),
                },
            }],
        };

        let result = run_export_after_preflight(
            &ready_conformance(),
            &blocked,
            &ready_lut_preflight(),
            || {
                export_called.set(true);
                Ok(())
            },
        );

        assert!(!export_called.get());
        assert!(matches!(
            result,
            Err(MediaError::Backend(message))
                if message.contains("changed-source") && message.contains("Changed")
        ));
    }

    // -----------------------------------------------------------------------
    // CC4 §2.3 LUT export gate
    // -----------------------------------------------------------------------

    const SAMPLE_CUBE: &str = "TITLE \"Gate look\"\n\
         LUT_3D_SIZE 2\n\
         DOMAIN_MIN 0.000000 0.000000 0.000000\n\
         DOMAIN_MAX 1.000000 1.000000 1.000000\n\
         0.000000 0.000000 0.000000\n\
         0.500000 0.000000 0.000000\n\
         0.000000 0.500000 0.000000\n\
         0.500000 0.500000 0.000000\n\
         0.000000 0.000000 0.500000\n\
         0.500000 0.000000 0.500000\n\
         0.000000 0.500000 0.500000\n\
         1.000000 1.000000 1.000000\n";

    /// A one-clip document whose clip carries a `creative_look` bound to the
    /// supplied asset.
    fn look_gate_document(asset: kinewright_core::LutAsset) -> Document {
        let mut document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: AssetId(1),
                    timeline_start: TimeCode::ZERO,
                    source_range: TimeCode::ZERO..TimeCode(24),
                    content: ClipContent::Media,
                    effects: vec![Effect {
                        id: EffectId(1),
                        name: "creative_look".to_owned(),
                        parameters: BTreeMap::from([(
                            LUT_ASSET_ID_PARAMETER.to_owned(),
                            ParamValue::Integer(1),
                        )]),
                        keyframes: BTreeMap::new(),
                    }],
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: PathBuf::from("shot.mov"),
                name: "Shot".to_owned(),
                duration: TimeCode(24),
                fps: Rational::new(24, 1).expect("valid fps"),
                kind: MediaKind::Video,
                resolution: Some((1920, 1080)),
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: ColorDescription::default(),
            }],
            fps: Rational::new(24, 1).expect("valid fps"),
            resolution: (1920, 1080),
            lut_assets: vec![asset],
            duration: TimeCode(24),
            ..Document::default()
        };
        document.color_context = ColorContext::default();
        document.validate().expect("the gate fixture is valid");
        document
    }

    #[test]
    fn the_export_gate_blocks_on_a_missing_store_file_and_names_the_recovery() {
        let temporary = TempDirectory::new("cc4-export-gate");
        let project = temporary.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let source = temporary.path("look.cube");
        std::fs::write(&source, SAMPLE_CUBE).expect("fixture .cube writes");
        let import = store.import_lut_asset(&source).expect("import");
        let sha256 = import.sha256.clone();
        let document = look_gate_document(import.into_lut_asset(LutAssetId(1)));

        // A present, correctly hashed store file passes the gate.
        let ready = export_lut_preflight(&document, Some(&store), None);
        assert!(ready.export_ready(), "{}", ready.summary());
        assert_eq!(ready.checked_lut_assets, vec![LutAssetId(1)]);

        // Removing the bytes the project claims to own blocks the export with
        // the asset id, title, hash, and expected store path.
        std::fs::remove_file(store.luts_dir().join(format!("{sha256}.cube")))
            .expect("the store file is removable");
        let blocked = export_lut_preflight(&document, Some(&store), None);
        assert!(!blocked.export_ready());
        assert_eq!(blocked.issues.len(), 1);
        assert_eq!(blocked.issues[0].lut_asset, LutAssetId(1));
        assert_eq!(blocked.issues[0].sha256, sha256);
        assert_eq!(blocked.issues[0].kind, LutAvailabilityKind::Missing);
        assert_eq!(
            blocked.issues[0].path.as_deref(),
            Some(store.luts_dir().join(format!("{sha256}.cube")).as_path())
        );
        assert_eq!(
            blocked.issues[0].referenced_by,
            vec![(ClipId(1), EffectId(1))]
        );

        // And the worker gate refuses to reach the encoder.
        let export_called = Cell::new(false);
        let result = run_export_after_preflight(
            &ready_conformance(),
            &ExportMediaPreflightReport {
                checked_assets: Vec::new(),
                issues: Vec::new(),
            },
            &blocked,
            || {
                export_called.set(true);
                Ok(())
            },
        );
        assert!(!export_called.get());
        assert!(matches!(
            result,
            Err(MediaError::Backend(message))
                if message.contains("Export blocked") && message.contains("Gate look")
        ));
    }

    #[test]
    fn a_project_with_looks_and_no_store_reports_project_not_saved() {
        let temporary = TempDirectory::new("cc4-export-unsaved");
        let store = LutStore::for_project(&temporary.path("edit.kinewright")).expect("store root");
        let source = temporary.path("look.cube");
        std::fs::write(&source, SAMPLE_CUBE).expect("fixture .cube writes");
        let import = store.import_lut_asset(&source).expect("import");
        let document = look_gate_document(import.into_lut_asset(LutAssetId(1)));

        let report = export_lut_preflight(&document, None, None);

        assert!(!report.export_ready());
        assert_eq!(report.issues.len(), 1);
        assert!(
            report.issues[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("project_not_saved")),
            "{:?}",
            report.issues[0].reason
        );
    }

    /// CC4 §2.2: a *saved* project whose derived store root this process
    /// refuses to use is not `project_not_saved`. The export gate reports the
    /// typed `lut_store_root_invalid` refusal and the recovery that can
    /// actually clear it — telling the operator to save a project they already
    /// saved is a loop that cannot terminate.
    #[test]
    fn a_refused_store_root_blocks_the_export_gate_with_its_reason_not_the_save_recovery() {
        let temporary = TempDirectory::new("cc4-export-refused-root");
        let project = temporary.path("edit.kinewright");
        let store = LutStore::for_project(&temporary.path("staging.kinewright"))
            .expect("the staging root is usable");
        let source = temporary.path("look.cube");
        std::fs::write(&source, SAMPLE_CUBE).expect("fixture .cube writes");
        let import = store.import_lut_asset(&source).expect("import");
        let document = look_gate_document(import.into_lut_asset(LutAssetId(1)));

        // A regular file occupies exactly where the store directory belongs,
        // so the project is saved but its root is refused.
        std::fs::write(temporary.path("edit.kinewright-assets"), b"not a directory")
            .expect("the blocking file writes");
        let refusal = crate::project::derive_lut_store(Some(&project))
            .expect_err("a file is not a store root");
        assert!(refusal.contains("lut_store_root_invalid: "), "{refusal}");

        let report = export_lut_preflight(&document, None, Some(&refusal));

        assert!(!report.export_ready());
        assert_eq!(report.issues.len(), 1);
        let reason = report.issues[0]
            .reason
            .as_deref()
            .expect("a blocked asset explains itself");
        assert!(reason.contains("lut_store_root_invalid: "), "{reason}");
        assert!(reason.contains(EXPORT_LUT_STORE_ROOT_RECOVERY), "{reason}");
        assert!(
            !reason.contains("project_not_saved"),
            "a saved project must never be told to save itself: {reason}"
        );
        assert!(
            !report.summary().contains("project_not_saved"),
            "{}",
            report.summary()
        );
    }

    /// CC4 §2.3: a built-in never lives in a store, so whether the project has
    /// been saved cannot decide its availability. A recorded hash that matches
    /// no bake in this binary is `changed`, naming both hashes — reporting it
    /// as `missing` with `project_not_saved` would send the operator off to
    /// save a project that would still refuse to export.
    #[test]
    fn an_unsaved_projects_built_in_is_typed_by_its_bake_not_by_the_save_state() {
        let verified = BuiltinLook::Warm.to_lut_asset(LutAssetId(1));
        let status = storeless_availability(&verified, EXPORT_PROJECT_NOT_SAVED);
        assert_eq!(status.kind, LutAvailabilityKind::Verified);
        assert_eq!(
            status.observed_sha256.as_deref(),
            Some(BuiltinLook::Warm.sha256())
        );
        assert!(status.reason.is_none());

        let mut stale = verified.clone();
        stale.sha256 = "0".repeat(64);
        let status = storeless_availability(&stale, EXPORT_PROJECT_NOT_SAVED);
        assert_eq!(status.kind, LutAvailabilityKind::Changed);
        assert_eq!(
            status.observed_sha256.as_deref(),
            Some(BuiltinLook::Warm.sha256()),
            "the observation is this build's bake"
        );
        let reason = status.reason.expect("a changed built-in explains itself");
        assert!(reason.starts_with("changed_lut_asset: "), "{reason}");
        assert!(reason.contains(BuiltinLook::Warm.sha256()), "{reason}");
        assert!(reason.contains(&stale.sha256), "{reason}");
        assert!(
            !reason.contains("project_not_saved"),
            "a built-in never depends on the store: {reason}"
        );

        // A name this build has no bake for stays `missing`, with its own
        // typed reason rather than the save recovery.
        let mut unknown = verified;
        unknown.source = LutAssetSource::Builtin {
            name: "sepia".to_owned(),
        };
        let status = storeless_availability(&unknown, EXPORT_PROJECT_NOT_SAVED);
        assert_eq!(status.kind, LutAvailabilityKind::Missing);
        let reason = status.reason.expect("an unknown built-in explains itself");
        assert!(reason.starts_with("unknown_builtin_look: "), "{reason}");
        assert!(reason.contains("sepia"), "{reason}");

        // An imported asset is the one shape that still reports the save
        // recovery, because its bytes really do need a store.
        let mut imported = BuiltinLook::Warm.to_lut_asset(LutAssetId(2));
        imported.source = LutAssetSource::Imported {
            source_path: "/looks/fixture.cube".to_owned(),
        };
        let status = storeless_availability(&imported, EXPORT_PROJECT_NOT_SAVED);
        assert_eq!(status.kind, LutAvailabilityKind::Missing);
        assert_eq!(status.reason.as_deref(), Some(EXPORT_PROJECT_NOT_SAVED));
    }

    #[test]
    fn a_bypassed_look_never_blocks_a_delivery() {
        let temporary = TempDirectory::new("cc4-export-bypassed");
        let project = temporary.path("edit.kinewright");
        let store = LutStore::for_project(&project).expect("store root");
        let source = temporary.path("look.cube");
        std::fs::write(&source, SAMPLE_CUBE).expect("fixture .cube writes");
        let import = store.import_lut_asset(&source).expect("import");
        let sha256 = import.sha256.clone();
        let mut document = look_gate_document(import.into_lut_asset(LutAssetId(1)));
        document.tracks[0].clips[0].effects[0]
            .parameters
            .insert("bypass".to_owned(), ParamValue::Integer(1));
        std::fs::remove_file(store.luts_dir().join(format!("{sha256}.cube")))
            .expect("the store file is removable");

        // A look the operator switched off is never evaluated, so its absent
        // bytes cannot block a delivery (CC4 §2.3).
        let report = export_lut_preflight(&document, Some(&store), None);
        assert!(report.export_ready(), "{}", report.summary());
        assert!(report.checked_lut_assets.is_empty());
    }

    // -----------------------------------------------------------------------
    // CC6 §8.4: delivery depth, the conformance lane, and verification
    // -----------------------------------------------------------------------

    /// A plane with no excursion at all.
    fn clean_plane() -> kinewright_core::PlaneLegalExcursion {
        kinewright_core::PlaneLegalExcursion {
            below_count: 0,
            above_count: 0,
            below_basis_points: 0,
            above_basis_points: 0,
            minimum_code_hundredths: 1_600,
            maximum_code_hundredths: 23_500,
        }
    }

    fn difference(max: u32, p99: i64, mean: i64) -> kinewright_core::DeliveryChannelDifference {
        kinewright_core::DeliveryChannelDifference {
            maximum_code_diff: max,
            p99_code_diff_millionths: p99,
            mean_code_diff_millionths: mean,
        }
    }

    /// A managed 8-bit delivery description as the export settings materialise
    /// it, and the same description as an H.264 probe would report it back.
    fn probed_description() -> ColorDescription {
        ColorDescription {
            provenance: kinewright_core::ColorProvenance::StreamMetadata,
            white_point: kinewright_core::ColorWhitePoint::Unknown,
            ..ColorContext::sdr_rec709().delivery
        }
    }

    /// One verification, built with the requested outcome.
    fn verification(tags_conform: bool, within_budgets: bool) -> ExportVerification {
        let expected = ColorContext::sdr_rec709().delivery;
        let observed = if tags_conform {
            probed_description()
        } else {
            ColorDescription {
                primaries: ColorPrimaries::Bt2020,
                ..probed_description()
            }
        };
        let tags = kinewright_core::delivery_tag_check(
            &expected,
            &observed,
            kinewright_core::DeliveryTagSource::ProbedOutputFile,
        );
        assert_eq!(
            tags.conforming, tags_conform,
            "the fixture's tag check has the direction it claims"
        );
        // Every gated number is derived from the lane's own budgets rather
        // than transcribed: a re-baselined constant must move this fixture with
        // it, not turn a "within" case into a silent overrun.
        let budgets = DeliveryBudgets::for_depth(DeliveryEncodeDepth::Eight);
        let luma = if within_budgets {
            difference(
                budgets.luma_max_code / 2,
                budgets.luma_p99_code_millionths / 2,
                budgets.luma_mean_code_millionths / 2,
            )
        } else {
            // Comfortably past every luma budget, in the same proportions.
            difference(
                budgets.luma_max_code.saturating_mul(8),
                budgets.luma_p99_code_millionths.saturating_mul(20),
                budgets.luma_mean_code_millionths.saturating_mul(30),
            )
        };
        // Reported, never gated: the RGB extremes are deliberately larger than
        // any luma budget, which is the point of the note beside them.
        let comparison = kinewright_core::DeliveryComparison {
            frames: vec![0, 14, 29, 44, 59],
            luma,
            red: difference(133, 2_000_000, 400_000),
            green: difference(120, 2_000_000, 380_000),
            blue: difference(134, 2_000_000, 420_000),
            combined: difference(134, 2_000_000, budgets.rgb_mean_code_millionths / 3),
            psnr_db_hundredths: Some(if within_budgets {
                budgets.psnr_floor_db_hundredths + 2_048
            } else {
                budgets.psnr_floor_db_hundredths - 1_900
            }),
            decoded_ycbcr: kinewright_core::YCbCrLegalReport {
                bit_depth: 8,
                luma: clean_plane(),
                cb: clean_plane(),
                cr: clean_plane(),
                source: kinewright_core::YCbCrLegalSource::DecodedNativePlanes,
            },
            rgb_extremes_note: kinewright_core::DELIVERY_RGB_EXTREMES_NOTE.to_owned(),
            budgets,
            within_budgets,
        };
        ExportVerification::Measured(Box::new(DeliveryVerification {
            output_path: PathBuf::from("/tmp/export.mp4"),
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
            probed: observed,
            tags,
            decoded_pixel_format: "yuv420p".to_owned(),
            comparison,
            exceptions: Vec::new(),
            technical_pass: tags_conform && within_budgets,
        }))
    }

    /// Every gated line the passing fixture must print, formatted from the
    /// lane's own budgets.
    ///
    /// The fixture measures a fixed fraction of each budget, so the expected
    /// text is derived the same way rather than transcribed: a re-baselined
    /// constant then moves the measurement and its expectation together.
    fn within_budget_lines(budgets: DeliveryBudgets) -> [String; 5] {
        [
            format!(
                "luma max {} code · budget {} · within",
                budgets.luma_max_code / 2,
                budgets.luma_max_code
            ),
            format!(
                "luma P99 {} µcode · budget {} · within",
                budgets.luma_p99_code_millionths / 2,
                budgets.luma_p99_code_millionths
            ),
            format!(
                "luma mean {} µcode · budget {} · within",
                budgets.luma_mean_code_millionths / 2,
                budgets.luma_mean_code_millionths
            ),
            format!(
                "RGB mean {} µcode (8-bit equivalent) · budget {} · within",
                budgets.rgb_mean_code_millionths / 3,
                budgets.rgb_mean_code_millionths
            ),
            format!(
                "PSNR {} dB · floor {} dB · within",
                decibels(budgets.psnr_floor_db_hundredths + 2_048),
                decibels(budgets.psnr_floor_db_hundredths)
            ),
        ]
    }

    /// CC6 §8.4 and §11.2.21: the dialog reports what decoding the file back
    /// said, with one unambiguous verdict per outcome.
    #[test]
    fn cc6_export_dialog_reports_the_verification_result() {
        let passing = verification(true, true);
        let over_budget = verification(true, false);
        let tag_mismatch = verification(false, true);
        let unavailable =
            ExportVerification::Unavailable("no GPU adapter for the reference render".to_owned());

        let statuses = [
            Some(&passing),
            Some(&over_budget),
            Some(&tag_mismatch),
            Some(&unavailable),
        ]
        .map(|verification| verification_status(verification).label);
        assert_eq!(
            statuses,
            ["VERIFIED", "OVER BUDGET", "TAG MISMATCH", "NOT VERIFIED"],
            "each outcome gets its own word"
        );
        let distinct: std::collections::HashSet<_> = statuses.iter().collect();
        assert_eq!(distinct.len(), 4, "four outcomes, four distinct statuses");
        assert_eq!(
            verification_status(None).label,
            "NOT VERIFIED",
            "no measurement is never a pass"
        );

        // The passing block names every gated number with its budget beside
        // it, and is not capped by MAX_ADVISORY_LINES.
        let lines = verification_lines(Some(&passing));
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            lines.len() > MAX_ADVISORY_LINES,
            "the verification block is uncapped: {} lines",
            lines.len()
        );
        // Formatted from the lane's constants, never transcribed: a
        // re-baselined budget is a change to what the dialog prints, and this
        // test has to keep proving it prints the current one.
        let budgets = DeliveryBudgets::for_depth(DeliveryEncodeDepth::Eight);
        for expected in within_budget_lines(budgets) {
            assert!(text.contains(&expected), "missing {expected:?} in:\n{text}");
        }
        assert!(
            text.contains("R max 133"),
            "the ungated RGB extremes are reported"
        );
        assert!(
            text.contains(kinewright_core::DELIVERY_RGB_EXTREMES_NOTE),
            "with the note that says why they are not a gate"
        );
        assert!(
            text.contains("Y′ below 0 px"),
            "the decoded native-plane excursions are shown"
        );
        // A field H.264 has no syntax for reads differently from a mismatch.
        let not_representable = lines
            .iter()
            .find(|line| line.text.starts_with("NOT REPRESENTABLE"))
            .expect("white_point is reported as not representable");
        assert_eq!(not_representable.color, color::TEXT_MUTED);
        assert!(
            !lines.iter().any(|line| line.text.starts_with("MISMATCH")),
            "and it is not counted as one"
        );

        let mismatch_lines = verification_lines(Some(&tag_mismatch));
        let mismatch = mismatch_lines
            .iter()
            .find(|line| line.text.starts_with("MISMATCH"))
            .expect("a mis-tagged file reports its field");
        assert_eq!(mismatch.color, color::STATUS_DANGER);
        assert_ne!(
            mismatch.color, not_representable.color,
            "a mismatch and a not-representable row are visually distinct"
        );

        let over_budget_lines = verification_lines(Some(&over_budget));
        let overrun = format!(
            "luma max {} code · budget {} · OVER",
            budgets.luma_max_code.saturating_mul(8),
            budgets.luma_max_code
        );
        assert!(
            over_budget_lines
                .iter()
                .any(|line| line.text.contains(&overrun) && line.color == color::STATUS_DANGER),
            "an overrun names the measurement, the budget, and the direction"
        );

        // And every case lays out through a headless context, the way the
        // dialog will draw it.
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        for case in [
            Some(&passing),
            Some(&over_budget),
            Some(&tag_mismatch),
            Some(&unavailable),
            None,
        ] {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                verification_block(ui, case);
            });
        }
    }

    /// CC6 §3.6/§8.4: the verification block reports the probed tags **per
    /// field**, not only as a verdict and a list of complaints.
    ///
    /// The rows come from the Colour QC window's own two functions, so one
    /// `DeliveryTagCheck` cannot be described differently by the two surfaces,
    /// and a field the container has no syntax for keeps its own muted tone.
    #[test]
    fn cc6_verification_block_reports_every_probed_tag_field() {
        let ExportVerification::Measured(measured) = verification(false, true) else {
            panic!("the fixture measures");
        };
        let lines = verification_lines(Some(&ExportVerification::Measured(measured.clone())));
        let text = |field: &str| {
            lines
                .iter()
                .find(|line| line.text.starts_with(&format!("{field} · ")))
                .unwrap_or_else(|| panic!("{field} has no row in the verification block"))
        };

        for (field, expected, probed) in crate::color_qc_ui::tag_field_rows(&measured.tags) {
            let line = text(field);
            assert_eq!(
                line.text,
                format!("{field} · expected {expected} · probed {probed}"),
                "the row names the field, what was asked for, and what the file carries"
            );
            assert_eq!(
                line.color,
                crate::color_qc_ui::tag_field_color(&measured.tags, field),
                "the two surfaces colour one tag check the same way"
            );
        }

        // Three states, three tones: this fixture is mis-tagged on primaries
        // and carries a white point H.264 cannot express.
        assert_eq!(text("primaries").color, color::STATUS_DANGER);
        assert_eq!(text("white_point").color, color::TEXT_MUTED);
        assert_eq!(text("transfer").color, color::TEXT_SECONDARY);
        assert_ne!(text("primaries").color, text("white_point").color);

        // And a conforming file still gets every row: a field that is right is
        // evidence too.
        let conforming = verification(true, true);
        let rows = verification_lines(Some(&conforming));
        for field in [
            "primaries",
            "transfer",
            "matrix",
            "range",
            "white_point",
            "bit_depth",
            "provenance",
            "confidence_basis_points",
        ] {
            assert!(
                rows.iter()
                    .any(|line| line.text.starts_with(&format!("{field} · "))),
                "{field} is missing from a conforming file's block"
            );
            assert_ne!(
                rows.iter()
                    .find(|line| line.text.starts_with(&format!("{field} · ")))
                    .expect("the row exists")
                    .color,
                color::STATUS_DANGER,
                "{field} agrees and must not be drawn as a mismatch"
            );
        }
    }

    /// CC6 §6.5 and §8.4: Cancel is answerable during verification. The encode
    /// already succeeded, so cancelling skips the measurement and says so —
    /// never a pass, never a failure, and never a frozen progress bar.
    #[test]
    fn cc6_cancelling_before_verification_reports_not_verified_with_the_reason() {
        let cancellation = ExportCancellation::default();
        assert_eq!(
            cancelled_before_verification(&cancellation),
            None,
            "an uncancelled export verifies"
        );

        cancellation.cancel();
        let verification =
            cancelled_before_verification(&cancellation).expect("a cancelled export skips it");
        assert_eq!(
            verification,
            ExportVerification::Unavailable(EXPORT_CANCELLED_BEFORE_VERIFICATION.to_owned())
        );
        assert_eq!(
            verification_status(Some(&verification)).label,
            "NOT VERIFIED",
            "nobody measured, which is never the same as failing"
        );
        assert_eq!(
            verification_status(Some(&verification)).color,
            color::STATUS_WARNING
        );
        let text = verification_lines(Some(&verification))
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains(EXPORT_CANCELLED_BEFORE_VERIFICATION),
            "the reason is stated: {text}"
        );
        assert!(
            text.contains("the file is untouched"),
            "and so is the fact that the encode succeeded: {text}"
        );
    }

    /// CC6 §6.5: the app's twin of the queue's
    /// `cc6_a_panicking_verification_is_contained_and_leaves_the_output_alone`.
    ///
    /// A verification decodes a finished file with a third-party backend. When
    /// that backend unwinds, the panic must not cross the export worker and
    /// take the finished encode with it: the file is written, the encode is
    /// still reported as a success, and the dialog says `NOT VERIFIED` with
    /// the reason instead of inventing a pass or losing the export.
    #[test]
    fn cc6_a_panicking_verification_is_contained_and_the_encode_is_still_reported() {
        let cancellation = ExportCancellation::default();
        let encode: Result<(), MediaError> = Ok(());
        let contained = worker_verification(&encode, &cancellation, || {
            panic!("the delivery verifier fell over")
        })
        .expect("a finished encode reports something about itself");
        let ExportVerification::Unavailable(reason) = &contained else {
            panic!("a panic invents no measurement: {contained:?}");
        };
        assert!(
            reason.contains("delivery verification panicked"),
            "the reason names what happened: {reason}"
        );
        assert!(
            reason.contains("the delivery verifier fell over"),
            "with the backend's own words: {reason}"
        );
        assert_eq!(
            verification_status(Some(&contained)).label,
            "NOT VERIFIED",
            "nobody measured, which is never a pass and never a failure"
        );
        assert_eq!(
            verification_status(Some(&contained)).color,
            color::STATUS_WARNING
        );
        assert!(
            encode.is_ok(),
            "a panicking measurement must not fail a finished encode"
        );

        // The other three outcomes go through the same seam unchanged.
        let ExportVerification::Measured(measured) = verification(true, true) else {
            panic!("the fixture measures");
        };
        let expected = (*measured).clone();
        assert_eq!(
            worker_verification(&Ok(()), &cancellation, || Ok(expected.clone())),
            Some(ExportVerification::Measured(measured)),
            "a measurement that returns is published as it stands"
        );
        assert_eq!(
            worker_verification(&Ok(()), &cancellation, || Err(MediaError::NotImplemented)),
            Some(ExportVerification::Unavailable(
                MediaError::NotImplemented.to_string()
            )),
            "and a refusal is reported as its own reason"
        );
        assert_eq!(
            worker_verification(&Err(MediaError::Cancelled), &cancellation, || {
                panic!("no file exists, so nothing may be measured")
            }),
            None,
            "an encode that did not finish wrote no file to verify"
        );

        // Cancellation short-circuits the measurement rather than containing
        // one: the verifier is never called at all.
        let cancelled = ExportCancellation::default();
        cancelled.cancel();
        assert_eq!(
            worker_verification(&Ok(()), &cancelled, || panic!(
                "a cancelled export starts no verification"
            )),
            Some(ExportVerification::Unavailable(
                EXPORT_CANCELLED_BEFORE_VERIFICATION.to_owned()
            ))
        );
    }

    /// CC6 §8.4: a verification sends no progress at all, so the bar is
    /// replaced by a line once the encode has finished.
    #[test]
    fn cc6_the_dialog_names_the_verifying_stage_instead_of_freezing_the_bar() {
        let stage = |completed, total| {
            export_stage(&ExportProgress {
                completed_frames: completed,
                total_frames: total,
            })
        };
        assert_eq!(
            stage(0, 0),
            ExportStage::Encoding,
            "no total yet is the pre-roll, not a finished encode"
        );
        assert_eq!(stage(0, 60), ExportStage::Encoding);
        assert_eq!(stage(59, 60), ExportStage::Encoding);
        assert_eq!(
            stage(60, 60),
            ExportStage::Verifying,
            "the last frame is encoded: whatever comes next is the verification"
        );
        assert_eq!(
            stage(61, 60),
            ExportStage::Verifying,
            "and an over-count is not a way back to a progress bar"
        );

        // And the body an operator actually reads is painted, at both stages.
        // A sentence assembled from implicitly concatenated string literals
        // keeps the source indentation between them, so the run of spaces
        // reaches the screen; the only way to catch that is to read the
        // painted galleys rather than the source.
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut painted = Vec::new();
        for progress in [
            ExportProgress {
                completed_frames: 30,
                total_frames: 60,
            },
            ExportProgress {
                completed_frames: 60,
                total_frames: 60,
            },
        ] {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(
                    !export_job_body(ui, &progress),
                    "nothing pressed Cancel in a headless frame"
                );
            });
            painted.extend(crate::theme::painted_text(&output));
        }
        for line in &painted {
            assert!(
                !line.contains("  "),
                "a painted line carries a run of spaces from the source: {line:?}"
            );
        }
        let body = painted.join("\n");
        assert!(
            body.contains(VERIFYING_STAGE_NOTE),
            "the verifying stage names itself: {body}"
        );
        assert!(
            body.contains("30 / 60 frames"),
            "and the encoding stage still counts frames: {body}"
        );
        // The Cancel hover text is a tooltip, so it is never in a painted
        // frame; it is prose all the same and is held to the same rule.
        assert!(
            !CANCEL_DURING_VERIFICATION_HINT.contains("  "),
            "{CANCEL_DURING_VERIFICATION_HINT:?}"
        );
    }

    /// CC6 §8.4: PSNR is hundredths of a dB, and a value in `(-1, 0)` has to
    /// keep its sign. `psnr / 100` is `0` there, so composing the string from
    /// the quotient dropped the minus on exactly the readings that matter.
    #[test]
    fn a_sub_decibel_psnr_keeps_its_sign() {
        assert_eq!(decibels(-50), "-0.50");
        assert_eq!(decibels(-1), "-0.01");
        assert_eq!(decibels(-3_300), "-33.00");
        assert_eq!(decibels(0), "0.00");
        assert_eq!(decibels(5_348), "53.48");
        assert_eq!(decibels(i32::MIN), "-21474836.48", "and it never overflows");

        // And it reaches the rendered line.
        let ExportVerification::Measured(mut measured) = verification(true, true) else {
            panic!("the fixture measures");
        };
        measured.comparison.psnr_db_hundredths = Some(-50);
        let text = verification_lines(Some(&ExportVerification::Measured(measured)))
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("PSNR -0.50 dB"),
            "the sign survives into the block: {text}"
        );
    }

    /// CC6 §8.4: the way to the Colour QC window is a control, not a sentence
    /// describing where to click.
    ///
    /// Driven by a real pointer press and release rather than by inspecting
    /// the widget: what the operator needs is that clicking the line *does*
    /// something, and a discarded response is exactly what the sentence used
    /// to be.
    #[test]
    fn the_colour_qc_line_is_a_button_a_click_can_reach() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut rect = egui::Rect::NOTHING;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            rect = color_qc_link(ui).rect;
        });
        assert!(
            rect.is_positive(),
            "the link occupies a rectangle a pointer can land on"
        );

        let centre = rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: centre,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let input = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(centre),
                button(true),
                button(false),
            ],
            ..egui::RawInput::default()
        };
        let mut clicked = false;
        let _ = ctx.run_ui(input, |ui| {
            clicked = color_qc_link(ui).clicked();
        });
        assert!(
            clicked,
            "clicking the line is what opens the Colour QC window"
        );
    }

    /// CC6 §11.2.20: the conformance cache is keyed by the delivery lane, so
    /// an 8-bit report can never be served for a 10-bit export.
    #[test]
    fn cc6_conformance_cache_does_not_cross_delivery_lanes() {
        let eight = ConformanceKey {
            revision: 3,
            aspect: None,
            focus_x_percent: 50,
            focus_y_percent: 50,
            width: 1920,
            height: 1080,
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
        };
        let ten = ConformanceKey {
            delivery_bit_depth: DeliveryEncodeDepth::Ten,
            ..eight
        };
        assert_ne!(eight, ten, "the lane is part of the cache identity");

        // The computed report is tagged with the lane it was computed for, so
        // a crossed cache would be visible rather than merely wrong.
        let computed = Cell::new(0_usize);
        let compute = |key: ConformanceKey| {
            computed.set(computed.get() + 1);
            Ok(ExportConformance {
                blocking: Vec::new(),
                advisory: vec![QaIssue {
                    severity: QaSeverity::Warning,
                    code: "lane".to_owned(),
                    message: key.delivery_bit_depth.as_str().to_owned(),
                    asset: None,
                    track: None,
                    clip: None,
                    range: None,
                }],
            })
        };

        let mut cache = None;
        let first = cached_conformance(&mut cache, eight, compute).expect("computed");
        assert_eq!(computed.get(), 1);
        assert_eq!(first.advisory[0].message, "eight");

        let cached = cached_conformance(&mut cache, eight, compute).expect("served from cache");
        assert_eq!(computed.get(), 1, "an identical key is served from cache");
        assert_eq!(cached.advisory[0].message, "eight");

        let crossed = cached_conformance(&mut cache, ten, compute).expect("recomputed");
        assert_eq!(computed.get(), 2, "a different lane recomputes");
        assert_eq!(
            crossed.advisory[0].message, "ten",
            "the 8-bit report was not served for the 10-bit key"
        );

        // And going back is a recompute too: the cache holds one entry.
        let back = cached_conformance(&mut cache, eight, compute).expect("recomputed");
        assert_eq!(computed.get(), 3);
        assert_eq!(back.advisory[0].message, "eight");
    }

    /// CC6 §11.2.19 (R11): the dialog keeps its inline `ExportSettings` and
    /// still agrees with the one function that owns the delivery depth.
    #[test]
    fn cc6_export_dialog_and_queue_agree_on_delivery_color() {
        let document = Document {
            color_context: ColorContext::sdr_rec709(),
            ..Document::default()
        };
        for depth in DeliveryEncodeDepth::ALL {
            assert_eq!(
                dialog_delivery_color(&document, depth),
                delivery_color_for_depth(&document, depth),
                "the dialog and delivery_color_for_depth disagree at {depth:?}"
            );
            assert_eq!(
                dialog_delivery_color(&document, depth).bit_depth,
                depth.color_bit_depth(),
                "only bit_depth moves"
            );
        }
        assert_ne!(
            dialog_delivery_color(&document, DeliveryEncodeDepth::Eight),
            dialog_delivery_color(&document, DeliveryEncodeDepth::Ten),
            "the two lanes are genuinely different descriptions"
        );
        // Every other field is the document's own delivery contract, untouched.
        assert_eq!(
            document.color_context.delivery.bit_depth,
            kinewright_core::ColorBitDepth::Eight,
            "and the document keeps declaring its own 8-bit delivery"
        );
    }
}
