//! CC6 §7: `get_color_qc`, the agent's read-only working-stage QC surface.
//!
//! This is the only agent tool that measures
//! [`kinewright_core::ScopeStage::WorkingLinearPostComposite`]. Everything it
//! publishes is evidence: it constructs no [`kinewright_core::Operation`],
//! commits nothing, gates no export, and leaves the timeline revision exactly
//! where it found it.
//!
//! **There is no `resolution` argument.** `working_proof_for_document` binds
//! full resolution and takes no scale, so a proxy working proof cannot be
//! produced at all; a proof whose `full_resolution` is `false` is refused with
//! `color_qc_proxy_proof_refused` rather than measured. No CC6 surface carries
//! `proxy_sampling`.

use std::sync::Arc;

use kinewright_core::{
    Analysis, ClipId, ColorQcCheck, ColorQcError, ColorQcReport, ColorQcRequest,
    DeliveryEncodeDepth, Document, EffectId, MAX_QC_NODE_CONTRIBUTIONS, MatteRegionDescription,
    MatteRegionScope, MediaError, NormalizedRoi, TimeCode, TimelineRevision, WORKING_PROOF_STAGE,
    delivery_color_for_depth, document_hdr_source_profile, matte_coverage_statistics,
    measure_color_qc, nodes, validate_node_budget,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::color_scopes::{MATTE_REGION_UNAVAILABLE, ScopeError, clip_midpoint, visual_clip};

/// The `get_color_qc` tool description, kept under M36's 1 KB budget.
///
/// **Unchanged by CC8, deliberately.** §8 requires `get_color_qc` to *report*
/// the lane-aware gamut and legality, the ungated `MaxCLL`/`MaxFALL`, and the
/// withheld-skin reason; it does not require the tool blurb to advertise them,
/// and the blurb is not free. CC7 §5.4 / R2-MAJ-3 pins the registry's total
/// serialized byte count at `1_280_060` in
/// `server::tests::served_surface_is_small_and_keeps_the_internal_registry_discoverable`,
/// so a longer description here moves a pinned CC7 constant — which CC8 §9.1
/// fixture 6 makes a condition of accepting CC8. The new facts therefore land
/// in the response's `report` and `assumptions`, which is where §8 puts them.
pub(crate) const COLOR_QC_DESCRIPTION: &str = "Measure colour QC evidence on the composited scene-linear working surface at one exact project frame: \
per-channel over/under-range counts and basis points against the delivery encode, out-of-gamut pixels, optional \
skin-band circular statistics, optional pre-export delivery-tag conformance, and optional per-node clipping \
attribution by node removal. This is the only stage where a legal-range or gamut excursion is observable at all; \
the RGBA8 monitor proof get_video_scopes_v2 measures is already display-clamped. Always full resolution: there is \
no proxy working proof and no resolution argument. Select the frame with timecode, frame, or clip_id (mutually \
exclusive; default the clip midpoint, else frame 0), scope with roi and/or matte_region, and pick checks from \
range, gamut, skin, tags, per_node. skin requires a region; per_node costs up to 17 renders and is never a \
default. Read-only: it mutates nothing and gates no export.";

/// The default `checks` set: the two measurements the report is *for*, plus the
/// pre-export tag check, which costs no extra pixel work.
const DEFAULT_CHECKS: [ColorQcCheck; 3] =
    [ColorQcCheck::Range, ColorQcCheck::Gamut, ColorQcCheck::Tags];

/// Raised when `checks` asks for `skin` without an `roi` or a `matte_region`.
///
/// CC6 §3.5 makes skin a diagnostic of a region the operator chose. Measuring
/// it over a whole raster would publish a hue statistic of everything in shot
/// as if it described a face, so the tool refuses rather than inventing a
/// population.
pub(crate) const COLOR_QC_REGION_REQUIRED: &str = "color_qc_region_required";

/// Raised when more than one of `timecode`, `frame`, and `clip_id` is sent.
pub(crate) const COLOR_QC_FRAME_SELECTOR_CONFLICT: &str = "color_qc_frame_selector_conflict";

/// Raised when this build's renderer cannot produce a working proof at all.
///
/// Distinct from every `color_qc_*` refusal: nothing was measured and nothing
/// was wrong with the request. The CC5 `matte_proof_unavailable` precedent.
pub(crate) const WORKING_PROOF_UNAVAILABLE: &str = "working_proof_unavailable";

/// Raised when the resolved project frame is outside the project.
///
/// The working proof of a frame the project does not have is a composite of
/// nothing: the target is cleared to opaque black and every clip misses, so
/// the measurement would be a clean legal-range pass over a black raster that
/// no export will ever contain. Mirrors `ColorProofError::ProjectFrameOutOfRange`,
/// which refuses the same request on the CC1 proof path.
pub(crate) const COLOR_QC_FRAME_OUT_OF_RANGE: &str = "color_qc_frame_out_of_range";

/// The matte a CC6 QC measurement is scoped to (CC5's shape, unchanged).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColorQcMatteRegionArgs {
    /// Clip carrying the matte-scoping colour node.
    pub clip_id: ClipId,
    /// The matte-carrying colour node's effect id.
    pub effect_id: EffectId,
}

/// Canonical request envelope for `get_color_qc`.
///
/// Deliberately carries **no** `resolution`, `proxy_sampling`, or `max_width`:
/// a working-stage measurement is full-resolution or it is refused.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColorQcArgs {
    /// Optional revision returned by `get_timeline_state`. This is an
    /// inspector, not a planner, so omitting it succeeds; supplying a stale
    /// one fails with the uniform `stale_revision`.
    #[serde(default)]
    pub expected_revision: Option<TimelineRevision>,
    /// One exact project frame. Mutually exclusive with `frame` and `clip_id`.
    #[serde(default)]
    pub timecode: Option<TimeCode>,
    /// The same exact project frame, for clients that model it as `frame`.
    /// Mutually exclusive with `timecode` and `clip_id`.
    #[serde(default)]
    pub frame: Option<TimeCode>,
    /// Measure this clip's midpoint. Mutually exclusive with `timecode` and
    /// `frame`.
    #[serde(default)]
    pub clip_id: Option<ClipId>,
    /// CC2's half-open basis-point rectangle. Omission measures the whole
    /// raster.
    #[serde(default)]
    pub roi: Option<NormalizedRoi>,
    /// Restrict the measured population to one colour node's matte coverage.
    /// Composable with `roi`, in which case the population is the
    /// intersection.
    #[serde(default)]
    pub matte_region: Option<ColorQcMatteRegionArgs>,
    /// `range`, `gamut`, `skin`, `tags`, `per_node`. Defaults to
    /// `[range, gamut, tags]`.
    #[serde(default)]
    pub checks: Option<Vec<ColorQcCheck>>,
    /// `1..=16`, validated on every call and not only when `checks` contains
    /// `per_node`: an out-of-range budget is a malformed request whether or
    /// not this call would have spent it.
    #[serde(default)]
    pub max_nodes: Option<u8>,
    /// `eight` (default) or `ten`. Selects the delivery code scale and the
    /// pre-export expected tag set.
    #[serde(default)]
    pub delivery_bit_depth: Option<DeliveryEncodeDepth>,
}

/// Measure one working proof and publish the CC6 §3.8 report.
///
/// # Errors
///
/// Returns a typed [`ScopeError`] for a stale revision, a conflicting frame
/// selector, a frame outside the project, a skin check with no region, a node
/// budget outside `1..=16`, an unrenderable working proof or matte proof, and
/// every [`ColorQcError`] refusal.
pub(crate) fn get_color_qc(
    document: &Arc<Document>,
    revision: TimelineRevision,
    analysis: &dyn Analysis,
    args: &ColorQcArgs,
) -> Result<Value, ScopeError> {
    if let Some(expected) = args.expected_revision
        && expected != revision
    {
        return Err(ScopeError::stale(expected, revision));
    }
    let at = resolve_frame(document, args)?;
    let checks = args
        .checks
        .clone()
        .unwrap_or_else(|| DEFAULT_CHECKS.to_vec());
    let has_region = args.roi.is_some() || args.matte_region.is_some();
    if checks.contains(&ColorQcCheck::Skin) && !has_region {
        return Err(region_required());
    }
    let max_nodes = args
        .max_nodes
        .unwrap_or_else(|| u8::try_from(MAX_QC_NODE_CONTRIBUTIONS).unwrap_or(u8::MAX));
    validate_node_budget(max_nodes).map_err(|error| qc_refusal(&error))?;
    let depth = args.delivery_bit_depth.unwrap_or_default();
    let per_node = checks.contains(&ColorQcCheck::PerNode);

    let matte_region = match args.matte_region {
        None => None,
        Some(region) => Some(matte_region_scope(document, analysis, at, region)?),
    };
    let request = ColorQcRequest {
        roi: args.roi,
        matte_region,
        checks: checks.clone(),
        delivery_bit_depth: depth,
        // CC6 §3.6 pre-export mode: the expected description is
        // `ExportSettings.delivery_color` materialised from this document at
        // the requested depth, and `observed` is the same value. A post-export
        // check needs a written file and is only available through
        // `verify_delivery_output` and `get_export_jobs`.
        expected_delivery: Some(delivery_color_for_depth(document, depth)),
        observed_delivery: None,
        max_nodes,
        project_frame: at.0,
        // CC8 §6 items 2 and 4. Resolved by core's one classifier rather than
        // here, so this tool and the app's Colour QC window cannot disagree
        // about whether the frame in front of them is HDR. The *lane* is not
        // sent: `ColorQcRequest::delivery_lane` derives it from
        // `expected_delivery` above (§5.2 clause 1), so the two cannot diverge.
        source_profile: document_hdr_source_profile(document),
    };

    let report = if per_node {
        nodes::measure_color_qc_with_nodes(analysis, Arc::clone(document), at, &request)
            .map_err(|error| media_refusal(&error, "per_node"))?
    } else {
        let proof = analysis
            .working_proof_for_document(Arc::clone(document), at)
            .map_err(|error| media_refusal(&error, "working_proof"))?;
        measure_color_qc(&proof, &request).map_err(|error| qc_refusal(&error))?
    };
    Ok(response(revision, &report, &checks, depth, per_node))
}

/// CC8 §8's `get_color_qc` rows, appended to the stated assumptions.
///
/// §8, verbatim: "`get_color_qc` reports the lane-aware gamut and legality of
/// §6, the `MaxCLL` and `MaxFALL` measurements as ungated rows, and the
/// withheld-skin reason."
///
/// The three rows are stated as *assumptions* rather than invented into the
/// report, because the report already carries every number: what an agent
/// reading this envelope cannot see from the numbers alone is which matrix
/// produced them, which triangle it is looking at, and why a skin section is
/// missing. Every one is read from the report in hand, so an envelope that
/// says "lane-aware" on a report measured some other way is impossible.
///
/// **They appear only when the measurement has an HDR fact to state.** §8's
/// rows are `MaxCLL`/`MaxFALL` "as ungated rows" and the withheld-skin reason,
/// and both of those live in the `report` this envelope already carries —
/// `light_level` on every measurement, with its own `gated: false` and its own
/// boundary sentence, and `skin_withheld` on an HDR source. Prose *about* them
/// is a second statement, and an SDR measurement has nothing new to say: CC6's
/// six-assumption envelope is asserted exactly in
/// `mcp_server::cc6_get_color_qc_is_evidence_only_and_revision_gated`, which
/// §9.1 fixture 6 requires to pass unmoved. So the gate is the report's own
/// shape rather than a flag: an HDR lane, a published second triangle, or a
/// withheld diagnostic.
fn hdr_assumptions(report: &ColorQcReport, assumptions: &mut Vec<String>) {
    let hdr_measurement = report.delivery_lane != kinewright_core::DeliveryLane::SdrRec709.as_str()
        || report.gamut_rec2020.is_some()
        || report.skin_withheld.is_some();
    if !hdr_measurement {
        return;
    }
    assumptions.push(format!(
        "Delivery lane {}: the range report's encode, and the Y'CbCr legality prediction, are taken through this lane's own primaries, transfer, and matrix (CC8 §6 item 1). Reusing the BT.709 reference on a BT.2020 file would be a wrong number, not an approximate one.",
        report.delivery_lane,
    ));
    if let Some(rec2020) = &report.gamut_rec2020 {
        assumptions.push(format!(
            "Two gamut triangles are reported because they differ: {} pixels are outside Rec.709 and {} are outside Rec.2020. {}",
            report.gamut.out_of_gamut_pixel_count,
            rec2020.out_of_gamut_pixel_count,
            report
                .gamut_triangle_relation
                .as_deref()
                .unwrap_or(kinewright_core::GAMUT_TRIANGLE_RELATION),
        ));
    }
    assumptions.push(report.light_level.boundary.clone());
    if let Some(withheld) = &report.skin_withheld {
        assumptions.push(withheld.reason.clone());
    }
}

/// The CC6 §7 response envelope: `analyze_color_shot`'s shape, minus the
/// resolution fields a working-stage measurement no longer has.
fn response(
    revision: TimelineRevision,
    report: &ColorQcReport,
    checks: &[ColorQcCheck],
    depth: DeliveryEncodeDepth,
    per_node: bool,
) -> Value {
    json!({
        "timeline_revision": revision.0,
        "evidence_only": true,
        "applied": false,
        "stage": WORKING_PROOF_STAGE,
        // Echoed from the measurement rather than restated as a constant, so
        // the envelope cannot disagree with the report it carries. In practice
        // always true: a proof that is not full-resolution is refused with
        // color_qc_proxy_proof_refused before it can be measured.
        "full_resolution": report.full_resolution,
        "report": report,
        "assumptions": assumptions(report, checks, depth, per_node),
        "exceptions": report.exceptions,
    })
}

/// The stated assumptions behind one measurement, in a fixed order.
fn assumptions(
    report: &ColorQcReport,
    checks: &[ColorQcCheck],
    depth: DeliveryEncodeDepth,
    per_node: bool,
) -> Vec<String> {
    let mut assumptions = vec![
        format!(
            "Measured at {WORKING_PROOF_STAGE}: the composited scene-linear surface before any monitor or delivery encode. A legal-range or gamut excursion is not observable at monitoring_post_composite, because that raster is already display-clamped."
        ),
        "Always full resolution. working_proof_for_document takes no scale and binds full resolution, so no proxy working proof exists; a proof whose full_resolution is false is refused with color_qc_proxy_proof_refused rather than measured.".to_owned(),
        "The composite target is cleared to opaque black and blended One / OneMinusSrcAlpha, so alpha is 1 everywhere at this stage: transparent_pixel_count is always 0 and must not be read as a check.".to_owned(),
        format!(
            "Delivery code units are reported at the {} lane ({} bits); rates are integer-floor basis points of the visible pixels inside the requested region.",
            depth.as_str(),
            depth.bits()
        ),
    ];
    if checks.contains(&ColorQcCheck::Tags) {
        assumptions.push(
            "Delivery tags are checked in pre-export mode: expected is ExportSettings.delivery_color materialised from this document at the requested delivery_bit_depth, and observed is the same value, so the check answers whether this document would be accepted by the delivery gates. A post-export tag check requires a written file and is published only by get_export_jobs.".to_owned(),
        );
    }
    if checks.contains(&ColorQcCheck::Skin) && report.skin.is_some() {
        assumptions.push(kinewright_core::SKIN_DIAGNOSTIC_BOUNDARY.to_owned());
    }
    // CC8 §8's three rows, placed after CC6's own and before the per-node and
    // evidence-only closers, so the fixed order stays fixed.
    hdr_assumptions(report, &mut assumptions);
    if per_node {
        assumptions.push(
            format!("Per-node attribution is by removal, never bypass: each candidate node's effect is removed from a scratch clone and the frame is re-measured. It costs at most {MAX_QC_NODE_CONTRIBUTIONS} scratch renders plus one baseline, the live document is never touched, and clipping is not additive - the deltas do not sum to the total."),
        );
    }
    assumptions.push(
        "Evidence only. This tool constructs no operation, changes no document, and never gates an export: technical_pass is not export_ready.".to_owned(),
    );
    assumptions
}

/// Resolve the one project frame this measurement is taken at.
///
/// `timecode`, `frame`, and `clip_id` are mutually exclusive; `clip_id`
/// measures the clip's midpoint, and sending none measures frame 0. Whichever
/// selector was used, the resolved frame is checked against the project range
/// before any render is attempted.
fn resolve_frame(document: &Document, args: &ColorQcArgs) -> Result<TimeCode, ScopeError> {
    let selected = [
        ("timecode", args.timecode.is_some()),
        ("frame", args.frame.is_some()),
        ("clip_id", args.clip_id.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(name, _)| name)
    .collect::<Vec<_>>();
    if let Some(first) = selected.first().filter(|_| selected.len() > 1) {
        return Err(ScopeError::new(
            COLOR_QC_FRAME_SELECTOR_CONFLICT,
            format!(
                "timecode, frame, and clip_id select the same one frame, so exactly one may be sent; received {}",
                selected.join(", ")
            ),
        )
        .with_details(json!({
            // The first offending selector in the fixed order above, so the
            // named field is one the caller actually sent.
            "field": first,
            "observed": selected,
            "allowed": "at most one of timecode, frame, clip_id",
            "recovery_action": "Send one frame selector. Omit all three to measure project frame 0.",
        })));
    }
    let (field, at) = if let Some(clip_id) = args.clip_id {
        let clip = visual_clip(document, clip_id)?;
        ("clip_id", clip_midpoint(document, clip)?.0)
    } else if let Some(timecode) = args.timecode {
        ("timecode", timecode)
    } else if let Some(frame) = args.frame {
        ("frame", frame)
    } else {
        ("timecode", TimeCode::ZERO)
    };
    frame_in_project(document, at, field)?;
    Ok(at)
}

/// Refuse a frame the project does not have, before any render.
///
/// Without this the compositor happily returns the cleared target — opaque
/// black, every clip missed — and the report reads as a clean technical pass
/// of a frame that will never be delivered. A measurement of nothing must be a
/// refusal, not a pass.
fn frame_in_project(
    document: &Document,
    at: TimeCode,
    field: &'static str,
) -> Result<(), ScopeError> {
    if at >= TimeCode::ZERO && at < document.duration {
        return Ok(());
    }
    Err(ScopeError::new(
        COLOR_QC_FRAME_OUT_OF_RANGE,
        format!(
            "project frame {at} is outside project range 0..{}",
            document.duration
        ),
    )
    .with_details(json!({
        "field": field,
        "observed": at.0,
        "allowed": format!("0..{}", document.duration.0),
        "recovery_action": "Call get_timeline_state for the project duration and measure a frame inside 0..duration. A frame the project does not have composites to opaque black, which would report as a clean pass.",
    })))
}

/// Obtain the CC5 matte coverage this measurement is scoped to.
///
/// The coverage raster is not something core can produce: it comes from
/// `Analysis::matte_proof_for_document`, exactly as the CC5 matte-scoped scope
/// path obtains it, and the pinned `MATTE_SCOPE_THRESHOLD` selects the pixels
/// the correction touched at all.
fn matte_region_scope(
    document: &Arc<Document>,
    analysis: &dyn Analysis,
    at: TimeCode,
    region: ColorQcMatteRegionArgs,
) -> Result<MatteRegionScope, ScopeError> {
    let proof = analysis
        .matte_proof_for_document(Arc::clone(document), at, region.clip_id, region.effect_id)
        .map_err(|error| {
            ScopeError::new(
                MATTE_REGION_UNAVAILABLE,
                format!(
                    "could not render the matte proof for clip {} effect {} at project frame {at}: {error}",
                    region.clip_id, region.effect_id
                ),
            )
            .with_details(json!({
                "field": "matte_region",
                "observed": {
                    "clip_id": region.clip_id.0,
                    "effect_id": region.effect_id.0,
                    "project_frame": at.0,
                    "message": error.to_string(),
                },
                "allowed": "an active matte-carrying colour node this build's renderer can proof",
                "recovery_action": "Call inspect_grade_matte for the node's coverage, or drop matte_region; no population is invented here.",
            }))
        })?;
    let covered_pixel_count = matte_coverage_statistics(&proof.coverage)
        .map(|statistics| statistics.covered_pixel_count)
        .map_err(|error| {
            ScopeError::new(error.code(), error.to_string()).with_details(json!({
                "field": "matte_region",
                "observed": error.to_string(),
                "allowed": "a coverage raster with R = G = B and an opaque alpha (CC5 §4.1)",
                "recovery_action": "The renderer returned a raster that is not a coverage proof; report this build's provenance.",
            }))
        })?;
    Ok(MatteRegionScope {
        description: MatteRegionDescription::new(
            region.clip_id,
            region.effect_id,
            covered_pixel_count,
        ),
        coverage: proof.coverage,
    })
}

/// `checks: ["skin"]` with no `roi` and no `matte_region`.
fn region_required() -> ScopeError {
    ScopeError::new(
        COLOR_QC_REGION_REQUIRED,
        "the skin check is a diagnostic of a region the operator chose, so it requires an roi, a matte_region, or both",
    )
    .with_details(json!({
        "field": "checks",
        "observed": "skin with no roi and no matte_region",
        "allowed": "skin alongside roi and/or matte_region",
        "recovery_action": "Send an roi covering the skin, or a matte_region naming the node whose matte selects it. Measuring skin hue over a whole raster would describe everything in shot as if it were a face.",
    }))
}

/// One typed core QC refusal, in the CC1/CC2 field/observed/allowed shape.
fn qc_refusal(error: &ColorQcError) -> ScopeError {
    ScopeError::new(error.code(), error.to_string()).with_details(json!({
        "field": error.field(),
        "observed": error.observed(),
        "allowed": error.allowed_values(),
        "recovery_action": error.recovery_action(),
    }))
}

/// One renderer failure on a QC path, with every typed refusal kept typed.
///
/// A [`ColorQcError`] raised inside `color_qc::nodes` travels structurally as
/// [`MediaError::ColorQc`], so it is republished through the *same*
/// [`qc_refusal`] the direct path uses: identical code, field, observed,
/// allowed, and recovery action whichever path measured. Nothing here parses a
/// rendered message back apart. Any other media failure that owns a recovery
/// code keeps it — the treatment the export queue gives a LUT store refusal —
/// and everything else is honestly [`WORKING_PROOF_UNAVAILABLE`].
fn media_refusal(error: &MediaError, field: &str) -> ScopeError {
    if let MediaError::ColorQc(refusal) = error {
        return qc_refusal(refusal);
    }
    let rendered = error.to_string();
    if let Some(code) = error.recovery_code() {
        return ScopeError::new(code, rendered.clone()).with_details(json!({
            "field": field,
            "observed": rendered,
            "allowed": "a working proof this build's renderer can produce and measure",
            "recovery_action": "Resolve the reported media failure; no measurement is invented here.",
        }));
    }
    ScopeError::new(
        WORKING_PROOF_UNAVAILABLE,
        format!("could not render the working proof: {rendered}"),
    )
    .with_details(json!({
        "field": field,
        "observed": rendered,
        "allowed": "a build whose renderer can composite one full-resolution scene-linear frame",
        "recovery_action": "Check get_media_status and this build's GPU adapter; no measurement is invented here.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_stays_inside_the_m36_kilobyte_budget() {
        assert!(
            COLOR_QC_DESCRIPTION.len() < 1_024,
            "get_color_qc description is {} bytes",
            COLOR_QC_DESCRIPTION.len()
        );
    }

    #[test]
    fn the_schema_has_no_resolution_or_proxy_property() {
        let schema = crate::schema::schema_object::<ColorQcArgs>();
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("ColorQcArgs must publish properties");
        for absent in ["resolution", "proxy_sampling", "max_width"] {
            assert!(
                !properties.contains_key(absent),
                "CC6 R13: get_color_qc must not carry {absent}"
            );
        }
        for present in [
            "expected_revision",
            "timecode",
            "frame",
            "clip_id",
            "roi",
            "matte_region",
            "checks",
            "max_nodes",
            "delivery_bit_depth",
        ] {
            assert!(properties.contains_key(present), "missing {present}");
        }
        assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    }

    /// A project with frames `0..60` and nothing in it, so `resolve_frame` can
    /// be exercised without a renderer.
    fn timed_document() -> Document {
        Document {
            duration: TimeCode(60),
            ..Document::default()
        }
    }

    #[test]
    fn conflicting_frame_selectors_are_refused_before_any_render() {
        let document = timed_document();
        let args = ColorQcArgs {
            timecode: Some(TimeCode(3)),
            frame: Some(TimeCode(4)),
            ..ColorQcArgs::default()
        };
        let error = resolve_frame(&document, &args).unwrap_err();
        assert_eq!(error.code(), COLOR_QC_FRAME_SELECTOR_CONFLICT);
        assert_eq!(error.details()["observed"], json!(["timecode", "frame"]));
        // The named field is the first selector actually sent, not a constant.
        assert_eq!(error.details()["field"], "timecode");
        let aliased = ColorQcArgs {
            frame: Some(TimeCode(4)),
            clip_id: Some(ClipId(1)),
            ..ColorQcArgs::default()
        };
        let error = resolve_frame(&document, &aliased).unwrap_err();
        assert_eq!(error.code(), COLOR_QC_FRAME_SELECTOR_CONFLICT);
        assert_eq!(error.details()["field"], "frame");
        // One selector, and none at all, both resolve.
        let single = ColorQcArgs {
            timecode: Some(TimeCode(3)),
            ..ColorQcArgs::default()
        };
        assert_eq!(resolve_frame(&document, &single).unwrap(), TimeCode(3));
        let alias = ColorQcArgs {
            frame: Some(TimeCode(9)),
            ..ColorQcArgs::default()
        };
        assert_eq!(resolve_frame(&document, &alias).unwrap(), TimeCode(9));
        assert_eq!(
            resolve_frame(&document, &ColorQcArgs::default()).unwrap(),
            TimeCode::ZERO
        );
    }

    /// CC6 §7 / errata E32: a frame the project does not have is refused, not
    /// measured. Without the guard the compositor returns its cleared target
    /// and the report is a clean legal-range pass over opaque black.
    #[test]
    fn a_frame_outside_the_project_is_refused_rather_than_measured_as_black() {
        let document = timed_document();
        for (field, args) in [
            (
                "timecode",
                ColorQcArgs {
                    timecode: Some(TimeCode(-1)),
                    ..ColorQcArgs::default()
                },
            ),
            (
                "timecode",
                ColorQcArgs {
                    timecode: Some(TimeCode(60)),
                    ..ColorQcArgs::default()
                },
            ),
            (
                "frame",
                ColorQcArgs {
                    frame: Some(TimeCode(9_999)),
                    ..ColorQcArgs::default()
                },
            ),
        ] {
            let error = resolve_frame(&document, &args).unwrap_err();
            assert_eq!(error.code(), COLOR_QC_FRAME_OUT_OF_RANGE);
            let details = error.details();
            assert_eq!(details["field"], field);
            assert_eq!(details["allowed"], "0..60");
            assert!(details["observed"].is_i64(), "{details}");
            assert!(
                details["recovery_action"]
                    .as_str()
                    .is_some_and(|action| action.contains("get_timeline_state")),
                "{details}"
            );
        }
        // The last frame the project has is inside the half-open range.
        let last = ColorQcArgs {
            timecode: Some(TimeCode(59)),
            ..ColorQcArgs::default()
        };
        assert_eq!(resolve_frame(&document, &last).unwrap(), TimeCode(59));
        // An empty project has no frame 0 to measure, so the default selector
        // is refused too rather than silently proofing nothing.
        let empty = Document::default();
        assert_eq!(
            resolve_frame(&empty, &ColorQcArgs::default())
                .unwrap_err()
                .code(),
            COLOR_QC_FRAME_OUT_OF_RANGE
        );
    }

    /// CC6 §3.8 / errata E32: every `ColorQcError` variant survives the
    /// per-node renderer round trip with the *same* refusal the direct path
    /// publishes — code, field, observed, allowed, and recovery action — and
    /// nothing is recovered by parsing a rendered message.
    #[test]
    fn a_typed_core_refusal_keeps_its_code_through_the_renderer_label() {
        for refusal in [
            ColorQcError::ProxyProofRefused {
                observed: "false".to_owned(),
                allowed: "true (a working proof always binds full resolution)",
            },
            ColorQcError::RasterLengthMismatch {
                observed: "230399 samples".to_owned(),
                allowed: "230400 samples (320 x 180 x 4)".to_owned(),
            },
            ColorQcError::EmptyPopulation {
                observed: "0 visible pixels".to_owned(),
                allowed: "at least one visible pixel",
            },
            ColorQcError::NodeBudgetExceeded {
                observed: "17".to_owned(),
                allowed: "1..=16",
            },
            ColorQcError::MatteRegionRasterMismatch {
                observed: "160 x 90".to_owned(),
                allowed: "320 x 180".to_owned(),
            },
            // The per-node scratch removal is a document-model rejection, and
            // it reports as one: never as `working_proof_unavailable`, which
            // would describe a render that in fact succeeded.
            ColorQcError::NodeRemovalRejected {
                clip: ClipId(1),
                effect: EffectId(2),
                reason: "clips are not sorted".to_owned(),
            },
        ] {
            let direct = qc_refusal(&refusal);
            let through_nodes = media_refusal(&MediaError::ColorQc(refusal.clone()), "per_node");
            assert_eq!(through_nodes.code(), refusal.code());
            assert_eq!(through_nodes.code(), direct.code());
            assert_ne!(
                through_nodes.code(),
                WORKING_PROOF_UNAVAILABLE,
                "a typed refusal is never reported as an unavailable proof"
            );
            assert_eq!(through_nodes.details(), direct.details());
            assert_eq!(through_nodes.to_string(), direct.to_string());
            let details = through_nodes.details();
            assert_eq!(details["field"], refusal.field());
            assert_eq!(details["observed"], refusal.observed());
            assert_eq!(details["allowed"], refusal.allowed_values());
            assert_eq!(details["recovery_action"], refusal.recovery_action());
        }
        // A media failure that owns a recovery code keeps it.
        let typed = MediaError::from(
            kinewright_core::DeliveryColorError::PixelFormatDepthMismatch {
                observed: "8".to_owned(),
                allowed: "10".to_owned(),
            },
        );
        assert_eq!(
            media_refusal(&typed, "working_proof").code(),
            "delivery_pixel_format_depth_mismatch"
        );
        // A failure with no typed code stays honestly untyped.
        let opaque = MediaError::Backend("no usable GPU adapter".to_owned());
        assert_eq!(
            media_refusal(&opaque, "working_proof").code(),
            WORKING_PROOF_UNAVAILABLE
        );
    }

    /// Two visible pixels of mid-grey, measurable without a renderer.
    fn measurable_proof() -> kinewright_core::WorkingProof {
        kinewright_core::WorkingProof {
            image: kinewright_core::LinearRgbaImage {
                width: 2,
                height: 1,
                pixels: vec![0.5, 0.5, 0.5, 1.0, 0.5, 0.5, 0.5, 1.0],
            },
            metadata: kinewright_core::WorkingProofMetadata {
                render: kinewright_core::MonitorProofMetadata::test_double(),
                stage: WORKING_PROOF_STAGE.to_owned(),
                encoding: kinewright_core::WORKING_PROOF_ENCODING.to_owned(),
                raster_aspect_millionths: 2_000_000,
            },
        }
    }

    /// CC6 §7: the envelope's `full_resolution` is an echo of the report it
    /// carries, not a constant restated beside it.
    ///
    /// In production the value is always `true` — a proof that is not
    /// full-resolution is refused before it can be measured — which is exactly
    /// why a hard-coded `true` cannot be caught by any live call. The echo is
    /// asserted against a report that says otherwise.
    #[test]
    fn the_envelope_echoes_the_reports_own_full_resolution_flag() {
        let mut report = measure_color_qc(&measurable_proof(), &ColorQcRequest::default())
            .expect("a full-resolution proof measures");
        assert!(report.full_resolution);
        let envelope = response(
            TimelineRevision(4),
            &report,
            &DEFAULT_CHECKS,
            DeliveryEncodeDepth::Eight,
            false,
        );
        assert_eq!(envelope["full_resolution"], json!(true));
        assert_eq!(envelope["report"]["full_resolution"], json!(true));
        assert_eq!(envelope["timeline_revision"], json!(4));
        assert_eq!(envelope["stage"], WORKING_PROOF_STAGE);
        assert_eq!(envelope["evidence_only"], json!(true));
        assert_eq!(envelope["applied"], json!(false));

        report.full_resolution = false;
        let envelope = response(
            TimelineRevision(4),
            &report,
            &DEFAULT_CHECKS,
            DeliveryEncodeDepth::Eight,
            false,
        );
        assert_eq!(
            envelope["full_resolution"],
            json!(false),
            "the envelope must not claim a resolution the report denies"
        );
        assert_eq!(envelope["report"]["full_resolution"], json!(false));
    }

    /// CC8 §8: "`get_color_qc` reports the lane-aware gamut and legality of §6,
    /// the `MaxCLL` and `MaxFALL` measurements as ungated rows, and the
    /// withheld-skin reason."
    ///
    /// The three rows are asserted on the envelope, in both directions: present
    /// on an HDR measurement, and — because CC6's six-assumption envelope is a
    /// pinned surface (`mcp_server::cc6_get_color_qc_is_evidence_only_and_revision_gated`)
    /// and §9.1 fixture 6 requires it unmoved — absent on an SDR one, where the
    /// `report` carries the light-level row and there is nothing more to say.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_envelope_carries_cc8_section_8s_hdr_rows_and_leaves_the_sdr_one_alone() {
        let hdr_delivery = kinewright_core::ColorDescription {
            primaries: kinewright_core::ColorPrimaries::Bt2020,
            transfer: kinewright_core::ColorTransfer::AribStdB67,
            matrix: kinewright_core::ColorMatrix::Bt2020Ncl,
            range: kinewright_core::ColorRange::Limited,
            white_point: kinewright_core::ColorWhitePoint::D65,
            bit_depth: DeliveryEncodeDepth::Ten.color_bit_depth(),
            confidence_basis_points: 10_000,
            provenance: kinewright_core::ColorProvenance::UserOverride,
        };
        assert_eq!(
            kinewright_core::DeliveryLane::for_description(&hdr_delivery),
            kinewright_core::DeliveryLane::HdrHlgRec2020,
        );
        // The Rec.2020 blue primary carried into the working space, so the two
        // triangles genuinely differ and the second report is published.
        let wide = kinewright_core::cc8_apply_matrix(
            kinewright_core::CC8_REC2020_TO_BT709,
            [0.0, 0.0, 1.0],
        );
        let proof = kinewright_core::WorkingProof {
            image: kinewright_core::LinearRgbaImage {
                width: 2,
                height: 1,
                pixels: vec![1.0, 1.0, 1.0, 1.0, wide[0], wide[1], wide[2], 1.0],
            },
            metadata: measurable_proof().metadata,
        };
        let request = ColorQcRequest {
            checks: vec![ColorQcCheck::Range, ColorQcCheck::Gamut, ColorQcCheck::Skin],
            delivery_bit_depth: DeliveryEncodeDepth::Ten,
            expected_delivery: Some(hdr_delivery),
            source_profile: Some(kinewright_core::ColorSourceProfile::HlgRec2020),
            ..ColorQcRequest::default()
        };
        let report = measure_color_qc(&proof, &request).expect("the HDR proof measures");
        let envelope = response(
            TimelineRevision(4),
            &report,
            &request.checks,
            DeliveryEncodeDepth::Ten,
            false,
        );
        let assumptions = envelope["assumptions"]
            .as_array()
            .expect("assumptions is an array")
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let says = |needle: &str| assumptions.iter().any(|line| line.contains(needle));
        assert!(
            says(kinewright_core::DeliveryLane::HdrHlgRec2020.as_str()) && says("CC8 §6 item 1"),
            "the lane-aware legality row is missing: {assumptions:#?}"
        );
        assert!(
            says("Two gamut triangles are reported because they differ")
                && says("must never be summed"),
            "the dual-triangle row and its relation are missing: {assumptions:#?}"
        );
        assert!(
            says(kinewright_core::LIGHT_LEVEL_BOUNDARY),
            "the ungated MaxCLL/MaxFALL row is missing: {assumptions:#?}"
        );
        assert!(
            says(kinewright_core::SKIN_WITHHELD_REASON),
            "the withheld-skin reason is missing: {assumptions:#?}"
        );
        assert!(
            !says(kinewright_core::SKIN_DIAGNOSTIC_BOUNDARY),
            "a withheld diagnostic must not carry the boundary of a diagnostic that was not \
             published: {assumptions:#?}"
        );
        // The rows the report itself carries, which is where §8 puts the numbers.
        assert_eq!(
            envelope["report"]["delivery_lane"],
            json!(kinewright_core::DeliveryLane::HdrHlgRec2020.as_str())
        );
        assert_eq!(envelope["report"]["light_level"]["gated"], json!(false));
        assert_eq!(
            envelope["report"]["light_level"]["sampled_frame_count"],
            json!(1)
        );
        assert!(envelope["report"]["gamut_rec2020"].is_object());
        assert_eq!(
            envelope["report"]["gamut_triangle_relation"],
            json!(kinewright_core::GAMUT_TRIANGLE_RELATION)
        );
        assert_eq!(envelope["report"]["skin"], json!(null));
        assert!(envelope["report"]["skin_withheld"].is_object());

        // The SDR direction: CC6's envelope, unmoved, with the light-level row
        // still in the report because §6 item 3 measures it on every lane.
        let sdr_report = measure_color_qc(&measurable_proof(), &ColorQcRequest::default())
            .expect("the SDR proof measures");
        let sdr = response(
            TimelineRevision(4),
            &sdr_report,
            &DEFAULT_CHECKS,
            DeliveryEncodeDepth::Eight,
            false,
        );
        let sdr_assumptions = sdr["assumptions"].as_array().unwrap();
        assert_eq!(
            sdr_assumptions.len(),
            6,
            "an SDR envelope gains nothing from CC8 — the four that hold for every measurement, \
             the pre-export tag note, and the evidence-only boundary, exactly as \
             `mcp_server::cc6_get_color_qc_is_evidence_only_and_revision_gated` pins them: \
             {sdr_assumptions:#?}"
        );
        assert_eq!(sdr["report"]["light_level"]["gated"], json!(false));
        assert!(sdr["report"]["gamut_rec2020"].is_null());
        assert!(sdr["report"]["skin_withheld"].is_null());
    }

    #[test]
    fn the_skin_check_refuses_without_a_region() {
        let error = region_required();
        assert_eq!(error.code(), COLOR_QC_REGION_REQUIRED);
        assert_eq!(error.details()["field"], "checks");
    }
}
